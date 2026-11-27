//! Player types: lock how one player plays every decision of a solve at once, from a few
//! tendencies like "calls too much" or "folds too often to turn bets", so the solver finds the
//! best way to exploit that kind of player.
//!
//! A profile scales how often the player folds, checks or calls, and bets or raises compared
//! with GTO, separately for each street and for what the player faces: nothing to call, a bet
//! or a raise. At every decision of the player, the GTO frequencies are scaled by those weights
//! and hands move between actions until the range matches them. The strongest hands are the
//! first to continue or bet more and the weakest the first to give up, except that a player
//! who bluffs too much makes the extra bets with the weakest hands.
//!
//! The locks are built from the root down on the game that is about to be solved, so each
//! decision starts from the range the player really brings there after playing the profile on
//! the earlier streets.

use crate::locks;
use postflop_solver::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Street {
    Flop,
    Turn,
    River,
}

impl Street {
    /// The street a board of `cards` cards is on.
    pub fn of_board(cards: usize) -> Street {
        match cards {
            0..=3 => Street::Flop,
            4 => Street::Turn,
            _ => Street::River,
        }
    }
}

/// What the player faces at a decision.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Facing {
    /// Nothing to call: first to act on the street, or checked to.
    None,
    /// A bet.
    Bet,
    /// A raise, or a later raise.
    Raise,
}

impl Facing {
    /// From the number of bets and raises so far on the street.
    pub fn after(aggressive_actions: u8) -> Facing {
        match aggressive_actions {
            0 => Facing::None,
            1 => Facing::Bet,
            _ => Facing::Raise,
        }
    }
}

fn one() -> f64 {
    1.0
}

/// How a player type plays one kind of situation. The weights scale how often the player
/// folds, checks or calls, and bets or raises (all-ins included) compared with GTO; 1 leaves
/// that kind of action as it is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rule {
    pub street: Street,
    pub facing: Facing,
    #[serde(default = "one")]
    pub fold: f64,
    #[serde(default = "one")]
    pub passive: f64,
    #[serde(default = "one")]
    pub aggressive: f64,
    /// The extra bets and raises are bluffs with the weakest hands rather than value bets.
    #[serde(default)]
    pub bluff: bool,
}

/// A player type. Situations without a rule are played as in the GTO solution.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// How far the player is from GTO: every weight is raised to this power, so 0 is GTO, 1 is
    /// the weights as written and 2 twice as far.
    #[serde(default = "one")]
    pub strength: f64,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// The most a weight can scale an action by.
pub const MAX_WEIGHT: f64 = 100.0;

impl Profile {
    /// Checks the numbers and trims the labels.
    pub fn checked(mut self) -> Result<Profile, String> {
        if !self.strength.is_finite() || !(0.0..=3.0).contains(&self.strength) {
            return Err("The strength must be between 0 and 3.".into());
        }
        if self.rules.len() > 64 {
            return Err("A player type has at most 64 rules.".into());
        }
        for r in &self.rules {
            for w in [r.fold, r.passive, r.aggressive] {
                if !w.is_finite() || !(0.0..=MAX_WEIGHT).contains(&w) {
                    return Err(format!("Weights must be between 0 and {MAX_WEIGHT}."));
                }
            }
        }
        self.id = self.id.chars().take(40).collect();
        self.name = self.name.chars().take(80).collect();
        Ok(self)
    }

    /// How the player plays a situation: the last rule that matches, or GTO.
    pub fn adjust(&self, street: Street, facing: Facing) -> Adjust {
        match self.rules.iter().rev().find(|r| r.street == street && r.facing == facing) {
            Some(r) => Adjust {
                weights: [r.fold, r.passive, r.aggressive].map(|w| w.powf(self.strength)),
                bluff: r.bluff,
            },
            None => Adjust::GTO,
        }
    }
}

/// The weights for folds, checks and calls, and bets and raises at one decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adjust {
    pub weights: [f64; 3],
    pub bluff: bool,
}

impl Adjust {
    pub const GTO: Adjust = Adjust { weights: [1.0; 3], bluff: false };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Fold = 0,
    Passive = 1,
    Aggressive = 2,
}

pub fn kind(action: &Action) -> Kind {
    match action {
        Action::Fold => Kind::Fold,
        Action::Check | Action::Call => Kind::Passive,
        _ => Kind::Aggressive,
    }
}

/// How aggressive each action is: fold, then check or call, then bets and raises by size.
fn aggression(actions: &[Action]) -> Vec<i64> {
    actions
        .iter()
        .map(|a| match a {
            Action::Fold => -2,
            Action::Bet(x) | Action::Raise(x) | Action::AllIn(x) => *x as i64,
            _ => -1,
        })
        .collect()
}

/// Scales each action's frequency by the weight of its kind, then scales them back to add up
/// to 1. An action GTO never takes stays at 0. Keeps `freq` when nothing would be left.
pub fn tilt(freq: &[f64], kinds: &[Kind], weights: [f64; 3]) -> Vec<f64> {
    let scaled: Vec<f64> = freq.iter().zip(kinds).map(|(f, &k)| f * weights[k as usize]).collect();
    let sum: f64 = scaled.iter().sum();
    if sum > 0.0 && sum.is_finite() {
        scaled.iter().map(|x| x / sum).collect()
    } else {
        freq.to_vec()
    }
}

/// Hands whose strength is this close move together.
const STRENGTH_TOL: f64 = 0.002;

/// Moves one decision's strategy toward a player type.
///
/// `reach[h]` is how often the player brings hand `h` here, `s[h]` its strategy (adding up to
/// 1) and `strength[h]` how strong it is, e.g. its equity. Moves to a more aggressive action
/// take the strongest hands first (the weakest for bluffs), moves to a more passive one the
/// weakest. Returns the range-wide frequencies before and after.
pub fn adjust_strategy(
    actions: &[Action],
    adjust: &Adjust,
    reach: &[f64],
    s: &mut [Vec<f64>],
    strength: &[f64],
) -> (Vec<f64>, Vec<f64>) {
    let k = actions.len();
    let kinds: Vec<Kind> = actions.iter().map(kind).collect();
    let before = locks::frequencies(reach, s, k);
    if *adjust == Adjust::GTO {
        return (before.clone(), before);
    }
    let targets = tilt(&before, &kinds, adjust.weights);
    let rank = aggression(actions);
    let cost = |h: usize, a: usize, b: usize| {
        let bolder = rank[b] > rank[a];
        if bolder && !(adjust.bluff && kinds[b] == Kind::Aggressive) {
            -strength[h]
        } else {
            strength[h]
        }
    };
    let after = locks::rebalance(reach, s, &vec![false; reach.len()], &targets, cost, STRENGTH_TOL);
    (before, after)
}

/// A node still to visit.
struct Frame {
    history: Vec<usize>,
    /// Bets and raises so far on this street.
    aggressive: u8,
    /// Where this subtree starts and ends in the whole walk, for progress.
    lo: f64,
    hi: f64,
}

/// Locks every decision of one player to a profile, walking the tree from the root a few
/// nodes at a time so the caller can report progress and let other work in between.
pub struct Builder {
    player: usize,
    profile: Profile,
    stack: Vec<Frame>,
    /// Decisions locked so far.
    pub locked: usize,
    /// Memory the locks take, in bytes.
    pub bytes: usize,
    /// Roughly how much of the walk is done, from 0 to 1.
    pub progress: f64,
}

impl Builder {
    pub fn new(profile: Profile, player: usize) -> Builder {
        Builder {
            player,
            profile,
            stack: vec![Frame { history: Vec::new(), aggressive: 0, lo: 0.0, hi: 1.0 }],
            locked: 0,
            bytes: 0,
            progress: 0.0,
        }
    }

    pub fn done(&self) -> bool {
        self.stack.is_empty()
    }

    /// Visits up to `budget` nodes. `gto` is the solved game the profile starts from, `game` the
    /// game to lock: the same spot and board, memory allocated and not solved. Decisions of the
    /// player that are already locked (by hand) keep their lock. Returns whether it's done.
    pub fn step(&mut self, gto: &mut PostFlopGame, game: &mut PostFlopGame, budget: usize) -> Result<bool, String> {
        if !gto.is_solved() {
            return Err("The player type is built from a finished solve.".into());
        }
        for _ in 0..budget {
            let Some(frame) = self.stack.pop() else { break };
            self.progress = frame.lo;
            self.visit(gto, game, frame)?;
        }
        if self.stack.is_empty() {
            self.progress = 1.0;
        }
        Ok(self.stack.is_empty())
    }

    fn visit(&mut self, gto: &mut PostFlopGame, game: &mut PostFlopGame, frame: Frame) -> Result<(), String> {
        game.apply_history(&frame.history);
        if game.is_terminal_node() {
            return Ok(());
        }
        if game.is_chance_node() {
            // One card of each group of cards that play the same (suit isomorphism): the game
            // stores them as one node.
            let cards = game
                .available_actions()
                .iter()
                .filter_map(|a| match a {
                    Action::Chance(c) => Some((*c as usize, 0)),
                    _ => None,
                })
                .collect();
            self.push(&frame, cards);
            return Ok(());
        }

        let player = game.current_player();
        let actions = game.available_actions();
        let k = actions.len();
        let next = |a: &Action| if kind(a) == Kind::Aggressive { frame.aggressive.saturating_add(1) } else { frame.aggressive };
        if player != self.player {
            self.push(&frame, actions.iter().enumerate().map(|(i, a)| (i, next(a))).collect());
            return Ok(());
        }

        // How often the player brings each hand here, playing the profile (and any hand locks)
        // on the way.
        let n = game.num_private_hands(player);
        let reach: Vec<f64> = game.weights(player).iter().map(|&w| (w as f64).max(0.0)).collect();
        if reach.iter().sum::<f64>() <= 0.0 {
            // The player never gets here, so nothing below matters.
            return Ok(());
        }
        if game.current_locking_strategy().is_none() {
            gto.apply_history(&frame.history);
            if gto.is_terminal_node() || gto.is_chance_node() || gto.available_actions().len() != k {
                return Err("The GTO solve isn't the same game tree.".into());
            }
            let base = gto.strategy();
            let mut s: Vec<Vec<f64>> = (0..n)
                .map(|h| {
                    let row: Vec<f64> = (0..k).map(|a| base[a * n + h] as f64).collect();
                    locks::normalized(&row).unwrap_or_else(|| vec![1.0 / k as f64; k])
                })
                .collect();
            game.cache_normalized_weights();
            let strength: Vec<f64> = game.equity(player).iter().map(|&e| e as f64).collect();
            let street = Street::of_board(game.current_board().len());
            let adjust = self.profile.adjust(street, Facing::after(frame.aggressive));
            adjust_strategy(&actions, &adjust, &reach, &mut s, &strength);

            // Hands that can't be here stay unlocked: all zeros.
            let mut out = vec![0f32; k * n];
            for h in (0..n).filter(|&h| reach[h] > 0.0) {
                for a in 0..k {
                    out[a * n + h] = s[h][a] as f32;
                }
            }
            game.lock_current_strategy(&out);
            self.locked += 1;
            self.bytes += k * n * 4 + 64;
        }

        // Follow only the actions the player takes with some hand.
        let strategy = game.strategy();
        let children = actions
            .iter()
            .enumerate()
            .filter(|&(a, _)| (0..n).any(|h| reach[h] > 0.0 && strategy[a * n + h] > 0.0))
            .map(|(i, a)| (i, next(a)))
            .collect();
        self.push(&frame, children);
        Ok(())
    }

    fn push(&mut self, frame: &Frame, steps: Vec<(usize, u8)>) {
        if steps.is_empty() {
            return;
        }
        let width = (frame.hi - frame.lo) / steps.len() as f64;
        // In reverse, so the first step comes off the stack first and progress only goes up.
        for (i, (step, aggressive)) in steps.into_iter().enumerate().rev() {
            let mut history = frame.history.clone();
            history.push(step);
            let lo = frame.lo + width * i as f64;
            self.stack.push(Frame { history, aggressive, lo, hi: lo + width });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn tilt_scales_kinds_and_keeps_zeros() {
        let kinds = [Kind::Fold, Kind::Passive, Kind::Aggressive];
        // A calling station folds and raises less: 40/50/10 becomes 16/50/4 of 70.
        let t = tilt(&[0.4, 0.5, 0.1], &kinds, [0.4, 1.0, 0.4]);
        assert!(close(t[0], 0.16 / 0.7) && close(t[1], 0.5 / 0.7) && close(t[2], 0.04 / 0.7), "{t:?}");
        // An action GTO never takes stays at 0.
        let t = tilt(&[0.0, 1.0, 0.0], &kinds, [5.0, 1.0, 5.0]);
        assert_eq!(t, vec![0.0, 1.0, 0.0]);
        // Weights of 1 change nothing; weights that leave nothing keep GTO.
        assert_eq!(tilt(&[0.3, 0.7], &kinds[1..], [1.0; 3]), vec![0.3, 0.7]);
        assert_eq!(tilt(&[0.3, 0.7], &kinds[1..], [1.0, 0.0, 0.0]), vec![0.3, 0.7]);
    }

    #[test]
    fn the_last_matching_rule_wins_and_strength_scales_it() {
        let rule = |fold| Rule { street: Street::River, facing: Facing::Bet, fold, passive: 1.0, aggressive: 1.0, bluff: false };
        let mut p = Profile { id: "x".into(), name: "X".into(), strength: 1.0, rules: vec![rule(0.5), rule(0.25)] };
        assert_eq!(p.adjust(Street::River, Facing::Bet).weights, [0.25, 1.0, 1.0]);
        assert_eq!(p.adjust(Street::Turn, Facing::Bet), Adjust::GTO);
        p.strength = 0.5;
        assert!(close(p.adjust(Street::River, Facing::Bet).weights[0], 0.5));
        p.strength = 0.0;
        assert_eq!(p.adjust(Street::River, Facing::Bet), Adjust::GTO);
        assert!(Profile { strength: 4.0, ..p.clone() }.checked().is_err());
        assert!(Profile { rules: vec![rule(-1.0)], ..p.clone() }.checked().is_err());
        assert!(Profile { rules: vec![rule(f64::NAN)], ..p }.checked().is_err());
        assert_eq!(Facing::after(0), Facing::None);
        assert_eq!(Facing::after(1), Facing::Bet);
        assert_eq!(Facing::after(3), Facing::Raise);
    }

    /// Three hands of equal weight, strength 0.2, 0.5 and 0.8, facing a bet: fold, call, raise.
    fn facing_bet() -> (Vec<Action>, Vec<f64>, Vec<f64>) {
        (vec![Action::Fold, Action::Call, Action::Raise(60)], vec![1.0; 3], vec![0.2, 0.5, 0.8])
    }

    #[test]
    fn calling_more_starts_with_the_strongest_folds() {
        let (actions, reach, strength) = facing_bet();
        // The two weaker hands fold; the strongest calls.
        let mut s = vec![vec![1.0, 0.0, 0.0], vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
        let adjust = Adjust { weights: [0.25, 1.0, 1.0], bluff: false };
        let (before, after) = adjust_strategy(&actions, &adjust, &reach, &mut s, &strength);
        assert!(close(before[0], 2.0 / 3.0));
        // Fold goes from 2/3 to 0.5/1.5 = 1/3: the 0.5 hand calls, the 0.2 hand still folds.
        assert!(close(after[0], 1.0 / 3.0), "{after:?}");
        assert_eq!(s[0], vec![1.0, 0.0, 0.0]);
        assert_eq!(s[1], vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn folding_more_starts_with_the_weakest_calls() {
        let (actions, reach, strength) = facing_bet();
        let mut s = vec![vec![0.0, 1.0, 0.0]; 3];
        let adjust = Adjust { weights: [3.0, 1.0, 1.0], bluff: false };
        // Fold stays at 0: there's nothing to scale.
        let (_, after) = adjust_strategy(&actions, &adjust, &reach, &mut s.clone(), &strength);
        assert_eq!(after[0], 0.0);
        // From folding a third (the 0.5 hand), a nit folds 3/(3+2) = 60%.
        s[1] = vec![1.0, 0.0, 0.0];
        let (_, after) = adjust_strategy(&actions, &adjust, &reach, &mut s, &strength);
        assert!(close(after[0], 0.6), "{after:?}");
        // The 0.6 is 1.8 of the 3 hands: the weakest caller makes up the 0.8 missing and the
        // strongest keeps calling.
        assert!(close(s[0][0], 0.8), "{:?}", s[0]);
        assert_eq!(s[2], vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn bluffers_raise_their_weakest_hands_and_value_players_their_strongest() {
        let (actions, reach, strength) = facing_bet();
        let start = vec![vec![0.0, 0.9, 0.1]; 3];
        for (bluff, raiser) in [(true, 0), (false, 2)] {
            let mut s = start.clone();
            let adjust = Adjust { weights: [1.0, 1.0, 4.0], bluff };
            let (_, after) = adjust_strategy(&actions, &adjust, &reach, &mut s, &strength);
            // 0.1 raises become 0.4 / 1.3.
            assert!(close(after[2], 0.4 / 1.3), "{after:?}");
            let most = (0..3).max_by(|&a, &b| s[a][2].total_cmp(&s[b][2])).unwrap();
            assert_eq!(most, raiser, "bluff {bluff}: {s:?}");
        }
    }

    #[test]
    fn betting_less_drops_the_weakest_bets_first() {
        let actions = vec![Action::Check, Action::Bet(20), Action::Bet(60)];
        let reach = vec![1.0; 4];
        let strength = vec![0.1, 0.4, 0.6, 0.9];
        // The weakest and the strongest hands bet big, the others check.
        let mut s = vec![vec![0.0, 0.0, 1.0], vec![1.0, 0.0, 0.0], vec![1.0, 0.0, 0.0], vec![0.0, 0.0, 1.0]];
        let adjust = Adjust { weights: [1.0, 1.0, 1.0 / 3.0], bluff: false };
        let (_, after) = adjust_strategy(&actions, &adjust, &reach, &mut s, &strength);
        // Bets go from 1/2 to (1/6) / (1/2 + 1/6) = 1/4: only the bluff stops.
        assert!(close(after[2], 0.25), "{after:?}");
        assert_eq!(s[0], vec![1.0, 0.0, 0.0]);
        assert_eq!(s[3], vec![0.0, 0.0, 1.0]);
    }

    /// AA and QQ against KK on a river where the only bet is all-in.
    fn river_game() -> PostFlopGame {
        let card_config = CardConfig {
            range: ["AsAh,QsQh".parse().unwrap(), "KsKh".parse().unwrap()],
            flop: flop_from_str("2s3h4d").unwrap(),
            turn: card_from_str("6c").unwrap(),
            river: card_from_str("7c").unwrap(),
        };
        let tree_config = TreeConfig {
            initial_state: BoardState::River,
            starting_pot: 20,
            effective_stack: 10,
            river_bet_sizes: [("a", "").try_into().unwrap(), ("a", "").try_into().unwrap()],
            ..Default::default()
        };
        let mut game = PostFlopGame::with_config(card_config, ActionTree::new(tree_config).unwrap()).unwrap();
        game.allocate_memory(false);
        game
    }

    fn profile(fold: f64) -> Profile {
        Profile {
            id: "test".into(),
            name: "Test".into(),
            strength: 1.0,
            rules: vec![Rule { street: Street::River, facing: Facing::Bet, fold, passive: 1.0, aggressive: 1.0, bluff: false }],
        }
    }

    /// Solves the river game with KK (IP) playing `p`, and returns OOP's root strategy.
    fn solve_against(gto: &mut PostFlopGame, p: Profile) -> (Vec<f32>, usize) {
        let mut game = river_game();
        let mut b = Builder::new(p, 1);
        while !b.step(gto, &mut game, 3).unwrap() {}
        game.back_to_root();
        solve(&mut game, 1000, 0.001, false);
        game.cache_normalized_weights();
        (game.strategy(), b.locked)
    }

    #[test]
    fn a_calling_station_gets_no_bluffs_and_a_nit_gets_all_of_them() {
        let mut gto = river_game();
        solve(&mut gto, 1000, 0.001, false);
        // OOP's hands are QQ (0) and AA (1); check is action 0, all-in action 1.
        gto.back_to_root();
        let s = gto.strategy();
        assert!(s[2] > 0.1 && s[2] < 0.9, "GTO bluffs QQ some of the time: {s:?}");

        let (s, locked) = solve_against(&mut gto, profile(0.25));
        // KK decides after a check and after an all-in.
        assert_eq!(locked, 2);
        assert!(s[0] > 0.999 && s[3] > 0.999, "against a station QQ checks and AA shoves: {s:?}");

        let (s, _) = solve_against(&mut gto, profile(4.0));
        assert!(s[2] > 0.999, "against a nit QQ always bluffs: {s:?}");
    }

    #[test]
    fn every_decision_the_player_reaches_is_locked() {
        // A turn spot with bets and raises: AA, QQ, JJ against KK, TT.
        let card_config = CardConfig {
            range: ["AA,QQ,JJ".parse().unwrap(), "KK,TT".parse().unwrap()],
            flop: flop_from_str("2s3h4d").unwrap(),
            turn: card_from_str("6c").unwrap(),
            river: NOT_DEALT,
        };
        let sizes = || -> [BetSizeOptions; 2] { [("50%", "2x").try_into().unwrap(), ("50%", "2x").try_into().unwrap()] };
        let tree_config = TreeConfig {
            initial_state: BoardState::Turn,
            starting_pot: 20,
            effective_stack: 100,
            turn_bet_sizes: sizes(),
            river_bet_sizes: sizes(),
            ..Default::default()
        };
        let build = || {
            let mut game = PostFlopGame::with_config(card_config.clone(), ActionTree::new(tree_config.clone()).unwrap()).unwrap();
            game.allocate_memory(false);
            game
        };
        let mut gto = build();
        solve(&mut gto, 300, 0.01, false);

        let station = Profile {
            id: "station".into(),
            name: "Station".into(),
            strength: 1.0,
            rules: [Street::Turn, Street::River]
                .into_iter()
                .flat_map(|street| {
                    [
                        Rule { street, facing: Facing::None, fold: 1.0, passive: 1.0, aggressive: 0.5, bluff: false },
                        Rule { street, facing: Facing::Bet, fold: 0.3, passive: 1.0, aggressive: 0.3, bluff: false },
                        Rule { street, facing: Facing::Raise, fold: 0.4, passive: 1.0, aggressive: 0.3, bluff: false },
                    ]
                })
                .collect(),
        };
        let mut game = build();
        let mut b = Builder::new(station, 1);
        let mut last = 0.0;
        while !b.step(&mut gto, &mut game, 7).unwrap() {
            assert!(b.progress >= last);
            last = b.progress;
        }
        assert!(b.locked > 0 && b.bytes > 0);

        // Walk the whole tree: IP is locked wherever IP's range gets, OOP nowhere.
        let mut seen = 0;
        let mut stack = vec![Vec::new()];
        while let Some(h) = stack.pop() {
            game.apply_history(&h);
            if game.is_terminal_node() {
                continue;
            }
            let steps: Vec<usize> = if game.is_chance_node() {
                let mask = game.possible_cards();
                (0..52).filter(|c| mask & (1u64 << c) != 0).collect()
            } else {
                let p = game.current_player();
                let locked = game.current_locking_strategy().is_some();
                let reached = game.weights(p).iter().any(|&w| w > 0.0);
                if p == 0 {
                    assert!(!locked, "OOP locked at {h:?}");
                } else if reached {
                    assert!(locked, "IP not locked at {h:?}");
                    seen += 1;
                }
                (0..game.available_actions().len()).collect()
            };
            for s in steps {
                let mut next = h.clone();
                next.push(s);
                stack.push(next);
            }
        }
        // No isomorphic river cards on this board, so every locked node was seen once.
        assert_eq!(seen, b.locked);

        // Facing a turn bet, the station folds less than GTO.
        let fold_after = |g: &mut PostFlopGame| {
            g.apply_history(&[1]);
            g.cache_normalized_weights();
            crate::range_frequencies(g)[0]
        };
        let (gto_fold, station_fold) = (fold_after(&mut gto), fold_after(&mut game));
        assert!(gto_fold > 0.05, "GTO folds some: {gto_fold}");
        assert!(station_fold < gto_fold * 0.8, "station {station_fold} vs GTO {gto_fold}");
    }
}
