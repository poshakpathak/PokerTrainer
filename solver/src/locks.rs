//! Node locking: fix how one player plays at chosen decisions, so the solver finds the best
//! response to that strategy instead of the equilibrium.
//!
//! A lock is built from a solved game of the same spot. It starts from the strategy at the
//! node, fixes any painted hands, and moves the other hands between actions until the
//! range-wide frequencies match the targets. The result is a strategy for every hand, which
//! [`apply`] sets on a fresh game before it is solved.

use postflop_solver::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which hands change action first when frequencies move toward a target.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    /// Hands that lose the least EV by switching, so the result stays close to the solver.
    #[default]
    Ev,
    /// Hands with the least equity.
    Weak,
    /// Hands with the most equity.
    Strong,
}

/// What to lock at one decision.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Recipe {
    /// Range-wide frequency for each action, scaled to add up to 1. `None` keeps the mix that
    /// results from `hands`.
    #[serde(default)]
    pub targets: Option<Vec<f64>>,
    /// Fixed strategies for hand classes ("AKs", "TT") or combos ("AsKs"): one weight per
    /// action, scaled to add up to 1.
    #[serde(default)]
    pub hands: BTreeMap<String, Vec<f64>>,
    #[serde(default)]
    pub order: Order,
}

/// A decision to lock and its strategy: `#actions * #hands` values in the order `strategy()`
/// uses. A hand whose values are all zero is left free.
#[derive(Clone, Debug)]
pub struct LockedNode {
    pub history: Vec<usize>,
    pub strategy: Vec<f32>,
}

/// The strategy a recipe gives at one decision.
pub struct Built {
    pub strategy: Vec<f32>,
    /// Range-wide action frequencies before and after.
    pub before: Vec<f64>,
    pub after: Vec<f64>,
    pub warnings: Vec<String>,
}

/// Checks that `step` (the `i`-th entry of a history) can be played at the current node.
fn check_step(game: &PostFlopGame, i: usize, step: usize) -> Result<(), String> {
    if game.is_terminal_node() {
        return Err(format!("history[{i}]: the hand is already over"));
    }
    if game.is_chance_node() {
        if step >= 52 || game.possible_cards() & (1u64 << step) == 0 {
            return Err(format!("history[{i}]: card {step} can't be dealt here"));
        }
    } else if step >= game.available_actions().len() {
        return Err(format!("history[{i}]: no action {step} here"));
    }
    Ok(())
}

/// Walks `history` from the root, checking every step: an action index at a decision, a card
/// id (0 to 51) at a chance node. Returns who acted at each step, or `None` where a card was
/// dealt.
pub fn navigate(game: &mut PostFlopGame, history: &[usize]) -> Result<Vec<Option<usize>>, String> {
    game.back_to_root();
    let mut actors = Vec::with_capacity(history.len());
    for (i, &step) in history.iter().enumerate() {
        check_step(game, i, step)?;
        actors.push((!game.is_chance_node()).then(|| game.current_player()));
        game.play(step);
    }
    Ok(actors)
}

const RANK_CHARS: &[u8; 13] = b"23456789TJQKA";

/// "AKs", "AKo" or "AA" for two hole cards.
pub fn hand_class(hole: (Card, Card)) -> String {
    let (hi, lo) = if hole.0 > hole.1 { hole } else { (hole.1, hole.0) };
    let (rh, rl) = ((hi >> 2) as usize, (lo >> 2) as usize);
    let mut s = String::with_capacity(3);
    s.push(RANK_CHARS[rh] as char);
    s.push(RANK_CHARS[rl] as char);
    if rh != rl {
        s.push(if hi & 3 == lo & 3 { 's' } else { 'o' });
    }
    s
}

enum HandKey {
    Class(String),
    Combo(Card, Card),
}

impl HandKey {
    fn parse(key: &str) -> Result<HandKey, String> {
        let bad = || format!("{key:?} isn't a hand class like AKs or a combo like AsKs");
        let k = key.trim();
        if !k.is_ascii() {
            return Err(bad());
        }
        if k.len() == 4 {
            let a = card_from_str(&k[..2]).map_err(|_| bad())?;
            let b = card_from_str(&k[2..]).map_err(|_| bad())?;
            if a == b {
                return Err(bad());
            }
            return Ok(HandKey::Combo(a.max(b), a.min(b)));
        }
        let b = k.as_bytes();
        let rank = |c: u8| RANK_CHARS.iter().position(|&r| r == c.to_ascii_uppercase()).ok_or_else(bad);
        match b.len() {
            2 => {
                let r = rank(b[0])?;
                if rank(b[1])? != r {
                    return Err(bad());
                }
                Ok(HandKey::Class(format!("{0}{0}", RANK_CHARS[r] as char)))
            }
            3 => {
                let (r1, r2) = (rank(b[0])?, rank(b[1])?);
                let suit = b[2].to_ascii_lowercase();
                if r1 == r2 || (suit != b's' && suit != b'o') {
                    return Err(bad());
                }
                let (hi, lo) = (r1.max(r2), r1.min(r2));
                Ok(HandKey::Class(format!("{}{}{}", RANK_CHARS[hi] as char, RANK_CHARS[lo] as char, suit as char)))
            }
            _ => Err(bad()),
        }
    }

    fn matches(&self, hole: (Card, Card)) -> bool {
        match self {
            HandKey::Class(c) => hand_class(hole) == *c,
            HandKey::Combo(a, b) => (hole.0.max(hole.1), hole.0.min(hole.1)) == (*a, *b),
        }
    }
}

/// Scales non-negative weights to add up to 1, or `None` when they add up to 0.
fn normalized(v: &[f64]) -> Option<Vec<f64>> {
    let clean: Vec<f64> = v.iter().map(|&x| if x.is_finite() && x > 0.0 { x } else { 0.0 }).collect();
    let sum: f64 = clean.iter().sum();
    (sum > 0.0).then(|| clean.iter().map(|x| x / sum).collect())
}

/// Checks one weight per action and scales them to add up to 1.
fn check_mix(v: &[f64], k: usize) -> Result<Vec<f64>, String> {
    if v.len() != k {
        return Err(format!("needs {k} values, one per action, but got {}", v.len()));
    }
    if v.iter().any(|x| !x.is_finite() || *x < 0.0) {
        return Err("values can't be negative".into());
    }
    normalized(v).ok_or_else(|| "values add up to 0".into())
}

/// Range-wide action frequencies, weighting each hand by how often it gets here.
fn frequencies(w: &[f64], s: &[Vec<f64>], k: usize) -> Vec<f64> {
    let total: f64 = w.iter().sum();
    (0..k)
        .map(|a| {
            if total > 0.0 {
                w.iter().zip(s).map(|(w, s)| w * s[a]).sum::<f64>() / total
            } else {
                0.0
            }
        })
        .collect()
}

/// Moves strategy between actions until the range-wide frequencies match `targets`.
///
/// `w[h]` is how often hand `h` gets to the node and `s[h]` its strategy (adding up to 1).
/// Hands marked `fixed` don't change. Every step moves the hands with the lowest
/// `cost(hand, from, to)`; hands within `tol` of the lowest move together, each giving up the
/// same share of the action, so equal hands end up with equal strategies. Actions only lose or
/// only gain, and a target equal to the current mix changes nothing.
///
/// Returns the frequencies reached. They fall short of a target only when fixed hands alone
/// are more than it allows.
pub fn rebalance(
    w: &[f64],
    s: &mut [Vec<f64>],
    fixed: &[bool],
    targets: &[f64],
    cost: impl Fn(usize, usize, usize) -> f64,
    tol: f64,
) -> Vec<f64> {
    let k = targets.len();
    let total: f64 = w.iter().sum();
    if total <= 0.0 {
        return vec![0.0; k];
    }
    let mut mass = vec![0.0; k];
    for (wh, sh) in w.iter().zip(s.iter()) {
        for a in 0..k {
            mass[a] += wh * sh[a];
        }
    }
    let goal: Vec<f64> = targets.iter().map(|t| t * total).collect();
    let eps = total * 1e-9;
    let surplus = |mass: &[f64], a: usize| mass[a] > goal[a] + eps;
    let deficit = |mass: &[f64], b: usize| mass[b] < goal[b] - eps;

    // For every pair of actions that can trade, the movable hands from cheapest to dearest.
    let movable: Vec<usize> = (0..w.len()).filter(|&h| !fixed[h] && w[h] > 0.0).collect();
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); k * k];
    for a in (0..k).filter(|&a| surplus(&mass, a)) {
        for b in (0..k).filter(|&b| deficit(&mass, b)) {
            let mut list = movable.clone();
            list.sort_by(|&x, &y| cost(x, a, b).total_cmp(&cost(y, a, b)));
            order[a * k + b] = list;
        }
    }
    let mut next = vec![0usize; k * k];

    loop {
        let mut best: Option<(usize, usize, f64)> = None;
        for a in (0..k).filter(|&a| surplus(&mass, a)) {
            for b in (0..k).filter(|&b| deficit(&mass, b)) {
                let list = &order[a * k + b];
                let p = &mut next[a * k + b];
                while *p < list.len() && s[list[*p]][a] <= 0.0 {
                    *p += 1;
                }
                if let Some(&h) = list.get(*p) {
                    let c = cost(h, a, b);
                    if best.map_or(true, |(_, _, bc)| c < bc) {
                        best = Some((a, b, c));
                    }
                }
            }
        }
        let Some((a, b, cheapest)) = best else { break };

        let list = &order[a * k + b];
        let tier: Vec<usize> = list[next[a * k + b]..]
            .iter()
            .copied()
            .take_while(|&h| cost(h, a, b) <= cheapest + tol)
            .filter(|&h| s[h][a] > 0.0)
            .collect();
        let tier_mass: f64 = tier.iter().map(|&h| w[h] * s[h][a]).sum();
        let need = (mass[a] - goal[a]).min(goal[b] - mass[b]);
        if need >= tier_mass {
            for &h in &tier {
                s[h][b] += s[h][a];
                s[h][a] = 0.0;
            }
            mass[a] -= tier_mass;
            mass[b] += tier_mass;
        } else {
            let share = need / tier_mass;
            for &h in &tier {
                let moved = s[h][a] * share;
                s[h][a] -= moved;
                s[h][b] += moved;
            }
            mass[a] -= need;
            mass[b] += need;
        }
    }
    mass.iter().map(|m| m / total).collect()
}

/// Builds a locked strategy for the decision `game` is at, starting from its current
/// strategy. The game must be solved.
pub fn build(game: &mut PostFlopGame, starting_pot: i32, recipe: &Recipe) -> Result<Built, String> {
    if game.is_terminal_node() || game.is_chance_node() {
        return Err("Only a decision can be locked.".into());
    }
    game.cache_normalized_weights();
    let player = game.current_player();
    let n = game.num_private_hands(player);
    let k = game.available_actions().len();
    let cards = game.private_cards(player).to_vec();
    let board = game.current_board().iter().fold(0u64, |m, &c| m | 1u64 << c);
    let live: Vec<bool> = cards.iter().map(|&(a, b)| board & ((1u64 << a) | (1u64 << b)) == 0).collect();
    let reach = game.weights(player);
    let mut w: Vec<f64> = (0..n).map(|h| if live[h] { (reach[h] as f64).max(0.0) } else { 0.0 }).collect();
    if w.iter().sum::<f64>() <= 0.0 {
        // The player never gets here in this solution (it can happen on some of the cards a
        // lock covers). Weigh hands by the starting range instead, so the targets still say how
        // the range plays if a solve with locks does bring it here.
        let start = game.initial_weights(player);
        w = (0..n).map(|h| if live[h] { (start[h] as f64).max(0.0) } else { 0.0 }).collect();
    }

    let strategy = game.strategy();
    let mut s: Vec<Vec<f64>> = (0..n)
        .map(|h| {
            let row: Vec<f64> = (0..k).map(|a| strategy[a * n + h] as f64).collect();
            normalized(&row).unwrap_or_else(|| vec![1.0 / k as f64; k])
        })
        .collect();
    let before = frequencies(&w, &s, k);

    let mut fixed = vec![false; n];
    for (key, weights) in &recipe.hands {
        let mix = check_mix(weights, k).map_err(|e| format!("{key}: {e}"))?;
        let hand = HandKey::parse(key)?;
        for h in (0..n).filter(|&h| live[h] && hand.matches(cards[h])) {
            s[h] = mix.clone();
            fixed[h] = true;
        }
    }

    let mut warnings = Vec::new();
    if let Some(t) = &recipe.targets {
        let targets = check_mix(t, k).map_err(|e| format!("Target frequencies: {e}"))?;
        let reached = match recipe.order {
            Order::Ev => {
                let ev = game.expected_values_detail(player);
                let pot = starting_pot + game.total_bet_amount().iter().sum::<i32>();
                let cost = |h: usize, a: usize, b: usize| (ev[a * n + h] - ev[b * n + h]) as f64;
                rebalance(&w, &mut s, &fixed, &targets, cost, pot as f64 * 0.005)
            }
            Order::Weak | Order::Strong => {
                let eq = game.equity(player);
                let sign = if recipe.order == Order::Weak { 1.0 } else { -1.0 };
                rebalance(&w, &mut s, &fixed, &targets, |h, _, _| sign * eq[h] as f64, 0.002)
            }
        };
        let total: f64 = w.iter().sum();
        if total > 0.0 && reached.iter().zip(&targets).any(|(r, t)| (r - t).abs() > 0.005) {
            warnings.push(
                "The painted hands alone go over some of the target frequencies, so the lock gets as close as it can."
                    .into(),
            );
        }
    }

    let after = frequencies(&w, &s, k);
    let mut out = vec![0f32; k * n];
    for h in (0..n).filter(|&h| live[h]) {
        for a in 0..k {
            out[a * n + h] = s[h][a] as f32;
        }
    }
    Ok(Built { strategy: out, before, after, warnings })
}

/// Every history that matches `history` when the chance steps at the indexes in `wild` can be
/// any card. Branches where a fixed card can't be dealt are skipped.
pub fn expand(game: &mut PostFlopGame, history: &[usize], wild: &[usize]) -> Result<Vec<Vec<usize>>, String> {
    fn walk(
        game: &mut PostFlopGame,
        history: &[usize],
        wild: &[usize],
        prefix: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
    ) -> Result<(), String> {
        let i = prefix.len();
        if i == history.len() {
            out.push(prefix.clone());
            return Ok(());
        }
        navigate(game, prefix)?;
        if game.is_terminal_node() {
            return Err(format!("history[{i}]: the hand is already over"));
        }
        let chance = game.is_chance_node();
        let steps: Vec<usize> = if wild.contains(&i) {
            if !chance {
                return Err(format!("history[{i}] is an action, not a card"));
            }
            let mask = game.possible_cards();
            (0..52).filter(|&c| mask & (1u64 << c) != 0).collect()
        } else if chance {
            let card = history[i];
            if card < 52 && game.possible_cards() & (1u64 << card) != 0 {
                vec![card]
            } else {
                vec![]
            }
        } else if history[i] < game.available_actions().len() {
            vec![history[i]]
        } else {
            return Err(format!("history[{i}]: no action {} here", history[i]));
        };
        for step in steps {
            prefix.push(step);
            walk(game, history, wild, prefix, out)?;
            prefix.pop();
        }
        Ok(())
    }

    if let Some(&i) = wild.iter().find(|&&i| i >= history.len()) {
        return Err(format!("wild step {i} is past the end of the history"));
    }
    let mut out = Vec::new();
    walk(game, history, wild, &mut Vec::new(), &mut out)?;
    Ok(out)
}

/// The decisions where `player` acts on the way to `history`, not counting the last one.
pub fn decisions_on_path(game: &mut PostFlopGame, history: &[usize], player: usize) -> Result<Vec<Vec<usize>>, String> {
    let mut out = Vec::new();
    game.back_to_root();
    for (i, &step) in history.iter().enumerate() {
        check_step(game, i, step)?;
        if !game.is_chance_node() && game.current_player() == player {
            out.push(history[..i].to_vec());
        }
        game.play(step);
    }
    Ok(out)
}

/// Locks one decision of a game whose memory is allocated and that isn't solved yet.
pub fn apply(game: &mut PostFlopGame, node: &LockedNode) -> Result<(), String> {
    navigate(game, &node.history)?;
    if game.is_terminal_node() || game.is_chance_node() {
        return Err("a lock points at a node that isn't a decision".into());
    }
    let player = game.current_player();
    let len = game.available_actions().len() * game.num_private_hands(player);
    if node.strategy.len() != len {
        return Err("a lock doesn't fit this game tree".into());
    }
    game.lock_current_strategy(&node.strategy);
    Ok(())
}

/// Rounds frequencies for JSON.
pub fn rounded(v: &[f64]) -> Vec<f64> {
    v.iter().map(|&x| crate::round(x, 4)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strategies(rows: &[&[f64]]) -> Vec<Vec<f64>> {
        rows.iter().map(|r| r.to_vec()).collect()
    }

    #[test]
    fn a_target_equal_to_the_current_mix_changes_nothing() {
        let w = [1.0, 2.0, 1.0];
        let mut s = strategies(&[&[0.5, 0.5], &[1.0, 0.0], &[0.0, 1.0]]);
        let before = s.clone();
        let ev = [[1.0, 0.0], [2.0, 0.0], [0.0, 3.0]];
        // 0.5 + 2 + 0 = 2.5 of 4 for action 0.
        let reached = rebalance(&w, &mut s, &[false; 3], &[0.625, 0.375], |h, a, b| ev[h][a] - ev[h][b], 0.0);
        assert_eq!(s, before);
        assert!((reached[0] - 0.625).abs() < 1e-12);
    }

    #[test]
    fn the_hands_that_lose_the_least_switch_first() {
        // Fold (0) or call (1). Calls lose 3, 1 and 2 bb against folding.
        let w = [1.0, 1.0, 1.0];
        let mut s = strategies(&[&[0.0, 1.0], &[0.0, 1.0], &[0.0, 1.0]]);
        let ev = [[0.0, 3.0], [0.0, 1.0], [0.0, 2.0]];
        let reached = rebalance(&w, &mut s, &[false; 3], &[0.5, 0.5], |h, a, b| ev[h][a] - ev[h][b], 0.0);
        assert!((reached[0] - 0.5).abs() < 1e-12);
        assert_eq!(s[1], vec![1.0, 0.0]); // the cheapest call folds completely
        assert!((s[2][0] - 0.5).abs() < 1e-12); // the next one makes up the rest
        assert_eq!(s[0], vec![0.0, 1.0]);
    }

    #[test]
    fn equal_hands_move_together() {
        let w = [1.0, 1.0, 2.0];
        let mut s = strategies(&[&[1.0, 0.0], &[1.0, 0.0], &[1.0, 0.0]]);
        let reached = rebalance(&w, &mut s, &[false; 3], &[0.75, 0.25], |_, _, _| 0.0, 0.0);
        assert!((reached[1] - 0.25).abs() < 1e-12);
        for h in 0..3 {
            assert!((s[h][1] - 0.25).abs() < 1e-12, "hand {h}: {:?}", s[h]);
        }
    }

    #[test]
    fn fixed_hands_stay_and_the_rest_make_up_the_target() {
        let w = [1.0, 1.0, 1.0, 1.0];
        let mut s = strategies(&[&[0.0, 1.0], &[0.0, 1.0], &[0.0, 1.0], &[0.0, 1.0]]);
        let fixed = [true, false, false, false];
        // Moving everything except the fixed hand reaches 75%, not 100%.
        let reached = rebalance(&w, &mut s, &fixed, &[1.0, 0.0], |_, _, _| 0.0, 0.0);
        assert_eq!(s[0], vec![0.0, 1.0]);
        assert!((reached[0] - 0.75).abs() < 1e-12);
    }

    #[test]
    fn three_actions_trade_with_each_other() {
        // Check, small bet, big bet: cut the big bet to 0 and put it into checks.
        let w = [1.0, 1.0];
        let mut s = strategies(&[&[0.2, 0.3, 0.5], &[0.6, 0.0, 0.4]]);
        let ev = [[1.0, 1.1, 1.2], [0.5, 0.4, 0.6]];
        let targets = [0.85, 0.15, 0.0];
        let reached = rebalance(&w, &mut s, &[false; 2], &targets, |h, a, b| ev[h][a] - ev[h][b], 0.0);
        for a in 0..3 {
            assert!((reached[a] - targets[a]).abs() < 1e-9, "{reached:?}");
        }
        for row in &s {
            assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-9);
            assert!(row[2].abs() < 1e-12);
        }
    }

    #[test]
    fn hand_keys() {
        let ah = card_from_str("Ah").unwrap();
        let kh = card_from_str("Kh").unwrap();
        let kd = card_from_str("Kd").unwrap();
        assert_eq!(hand_class((kh, ah)), "AKs");
        assert_eq!(hand_class((ah, kd)), "AKo");
        assert_eq!(hand_class((kh, kd)), "KK");
        assert!(HandKey::parse("KAs").unwrap().matches((ah, kh)));
        assert!(HandKey::parse("kk").unwrap().matches((kd, kh)));
        assert!(HandKey::parse("KhAh").unwrap().matches((ah, kh)));
        assert!(!HandKey::parse("AhKd").unwrap().matches((ah, kh)));
        for bad in ["AKx", "A", "AAs", "AhAh", "XYZW", "A♠K♠"] {
            assert!(HandKey::parse(bad).is_err(), "{bad}");
        }
    }

    /// AA and QQ against KK on a river where the only bet is all-in (postflop-solver's own
    /// node locking example).
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

    #[test]
    fn a_built_lock_changes_the_best_response() {
        let mut gto = river_game();
        solve(&mut gto, 1000, 0.001, false);

        // At equilibrium KK calls the all-in half the time. Lock it to call 75%.
        navigate(&mut gto, &[1]).unwrap();
        let recipe = Recipe { targets: Some(vec![0.25, 0.75]), ..Default::default() };
        let built = build(&mut gto, 20, &recipe).unwrap();
        assert!((built.after[1] - 0.75).abs() < 1e-6, "{:?}", built.after);
        assert!(built.warnings.is_empty());

        let mut game = river_game();
        apply(&mut game, &LockedNode { history: vec![1], strategy: built.strategy }).unwrap();
        game.back_to_root();
        solve(&mut game, 1000, 0.001, false);
        game.cache_normalized_weights();

        // Against a player who calls too much, QQ stops bluffing and AA always shoves.
        let s = game.strategy();
        assert!((s[0] - 1.0).abs() < 1e-3, "QQ check: {s:?}");
        assert!((s[3] - 1.0).abs() < 1e-3, "AA all-in: {s:?}");
    }

    #[test]
    fn a_decision_nobody_reaches_still_gets_the_targets() {
        // OOP always shoves, so it never faces IP's shove after checking.
        let mut game = river_game();
        apply(&mut game, &LockedNode { history: vec![], strategy: vec![0.0, 0.0, 1.0, 1.0] }).unwrap();
        game.back_to_root();
        solve(&mut game, 200, 0.001, false);
        navigate(&mut game, &[0, 1]).unwrap();
        assert!(game.weights(0).iter().all(|&w| w == 0.0));
        let recipe = Recipe { targets: Some(vec![0.5, 0.5]), ..Default::default() };
        let built = build(&mut game, 20, &recipe).unwrap();
        assert!((built.after[0] - 0.5).abs() < 1e-6, "{:?}", built.after);
    }

    #[test]
    fn painted_hands_are_fixed() {
        let mut gto = river_game();
        solve(&mut gto, 1000, 0.001, false);
        navigate(&mut gto, &[]).unwrap();
        // OOP's hands are QQ (0) and AA (1); check is action 0, all-in action 1.
        let recipe = Recipe { hands: [("QQ".to_string(), vec![0.0, 1.0])].into(), ..Default::default() };
        let built = build(&mut gto, 20, &recipe).unwrap();
        assert_eq!(built.strategy[0], 0.0);
        assert_eq!(built.strategy[2], 1.0);
        // AA keeps its own strategy.
        let s = gto.strategy();
        assert!((built.strategy[1] - s[1]).abs() < 1e-6);
    }

    #[test]
    fn wild_cards_expand_to_every_river() {
        let card_config = CardConfig {
            range: ["AA,KK".parse().unwrap(), "QQ,JJ".parse().unwrap()],
            flop: flop_from_str("2s3h4d").unwrap(),
            turn: card_from_str("6c").unwrap(),
            river: NOT_DEALT,
        };
        let tree_config = TreeConfig {
            initial_state: BoardState::Turn,
            starting_pot: 20,
            effective_stack: 100,
            turn_bet_sizes: [("50%", "").try_into().unwrap(), ("50%", "").try_into().unwrap()],
            river_bet_sizes: [("50%", "").try_into().unwrap(), ("50%", "").try_into().unwrap()],
            ..Default::default()
        };
        let mut game = PostFlopGame::with_config(card_config, ActionTree::new(tree_config).unwrap()).unwrap();
        game.allocate_memory(false);

        // Check, check, then any river card, then OOP acts.
        let all = expand(&mut game, &[0, 0, 0], &[2]).unwrap();
        assert_eq!(all.len(), 52 - 4);
        assert!(all.iter().all(|h| h[..2] == [0, 0]));
        // A fixed card that is already on the board is skipped, not an error.
        assert_eq!(expand(&mut game, &[0, 0, card_from_str("6c").unwrap() as usize], &[]).unwrap().len(), 0);
        assert!(expand(&mut game, &[0, 0, 0], &[1]).is_err());

        assert_eq!(decisions_on_path(&mut game, &[0, 0, 7], 0).unwrap(), vec![Vec::<usize>::new()]);
        assert_eq!(decisions_on_path(&mut game, &[0, 0, 7], 1).unwrap(), vec![vec![0]]);
    }
}
