//! Discounted CFR in vector form for pots with two or three players: each pass walks the tree
//! for one player, carrying every player's reach for each of their hands.
//!
//! Updates alternate between the players, with the discounts postflop-solver uses
//! (alpha = 1.5, beta = 0.5, gamma = 3, the average strategy restarting at powers of 4). The
//! average strategy is weighted by how often each hand reaches the decision.
//!
//! Two shortcuts keep the work down. When the player being updated folds, what they lose is
//! fixed, so the rest of the hand isn't walked for them. And when an opponent's current strategy
//! never takes an action with any hand, everything after it is worth nothing to the player being
//! updated and is skipped; the decisions there catch up on the discounts they missed the next
//! time they're updated.

use super::eval::{self, Payoff};
use super::game::{Board, Game};
use super::spot::MAX_PLAYERS;
use super::tree::{Act, Kind, Node, Pot};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Decisions this close to the root solve their actions in parallel.
const PAR_DEPTH: usize = 3;

#[derive(Clone, Copy, Debug)]
pub struct Discount {
    alpha: f32,
    beta: f32,
    gamma: f32,
}

impl Discount {
    pub fn new(t: u32) -> Discount {
        // 0, 1, 4, 16, 64, 256, ...
        let nearest_lower_power_of_4 = match t {
            0 => 0,
            x => 1 << ((x.leading_zeros() ^ 31) & !1),
        };
        let t_alpha = (t as i32 - 1).max(0) as f64;
        let t_gamma = (t - nearest_lower_power_of_4) as f64;
        let pow_alpha = t_alpha * t_alpha.sqrt();
        let pow_gamma = (t_gamma / (t_gamma + 1.0)).powi(3);
        Discount { alpha: (pow_alpha / (pow_alpha + 1.0)) as f32, beta: 0.5, gamma: pow_gamma as f32 }
    }

    /// The discounts of every iteration after `last` (`u32::MAX` for none) up to `t`, for a
    /// decision that wasn't updated in between.
    fn since(last: u32, t: u32) -> Discount {
        if last == u32::MAX || last + 1 >= t {
            return Discount::new(t);
        }
        let mut d = Discount { alpha: 1.0, beta: 1.0, gamma: 1.0 };
        for i in last + 1..=t {
            let x = Discount::new(i);
            d.alpha *= x.alpha;
            d.beta *= x.beta;
            d.gamma *= x.gamma;
        }
        d
    }
}

/// The current strategy from cumulative regrets: positive regrets, normalized per hand.
fn regret_matching(regret: &[f32], k: usize, nh: usize) -> Vec<f32> {
    let mut s: Vec<f32> = regret.iter().map(|&r| r.max(0.0)).collect();
    normalize(&mut s, k, nh);
    s
}

/// Scales each hand's values over the actions to add up to 1 (uniform if they're all zero).
fn normalize(s: &mut [f32], k: usize, nh: usize) {
    let uniform = 1.0 / k as f32;
    for h in 0..nh {
        let sum: f32 = (0..k).map(|a| s[a * nh + h]).sum();
        for a in 0..k {
            let x = &mut s[a * nh + h];
            *x = if sum > 0.0 { *x / sum } else { uniform };
        }
    }
}

/// The average strategy at a decision.
pub fn average_strategy(game: &Game, node: &Node) -> Vec<f32> {
    let mut s = node.strat.clone();
    if !game.finalized {
        normalize(&mut s, node.actions.len(), game.hands[node.player as usize].len());
    }
    s
}

fn alive_mask(pot: &Pot, n: usize) -> u8 {
    (0..n).filter(|&p| pot.is_in(p)).fold(0, |m, p| m | (1 << p))
}

/// Everyone's reach, one slice per player.
type Reach<'a> = [&'a [f32]];

fn slices(reach: &[Vec<f32>]) -> Vec<&[f32]> {
    reach.iter().map(|r| r.as_slice()).collect()
}

fn terminal(game: &Game, pot: &Pot, board: &Board, p: usize, reach: &Reach, out: &mut [f32]) {
    let cfg = &game.cfg;
    let total = pot.total(cfg.starting_pot) as f64;
    let alive = alive_mask(pot, game.n);
    // When everyone else folds, the part of the last bet nobody called goes back before the rake.
    let uncalled = if alive.count_ones() == 1 {
        let w = alive.trailing_zeros() as usize;
        let called = (0..game.n).filter(|&i| i != w).map(|i| pot.invested[i]).max().unwrap_or(0);
        (pot.invested[w] - called).max(0)
    } else {
        0
    };
    let rake = ((total - uncalled as f64) * cfg.rake_rate).min(cfg.rake_cap);
    let pay = Payoff { net: total - rake, invested: pot.invested[p] as f64, alive };
    let entries = if alive.count_ones() > 1 && alive & (1 << p) != 0 { game.showdown(board) } else { None };
    eval::terminal(&game.players(), entries, board.mask, p, &pay, reach, 1.0 / game.mass, out);
}

/// What player `p` gets after folding at a decision with pot `pot`: minus what they've put in.
fn fold_value(game: &Game, pot: &Pot, board: &Board, p: usize, reach: &Reach, out: &mut [f32]) {
    let alive = alive_mask(pot, game.n) & !(1 << p);
    let pay = Payoff { net: 0.0, invested: pot.invested[p] as f64, alive };
    eval::terminal(&game.players(), None, board.mask, p, &pay, reach, 1.0 / game.mass, out);
}

/// Walks every card of a chance node (in parallel) and averages the children's values.
fn chance<F>(game: &Game, node: &mut Node, reach: &Reach, board: Board, out: &mut [f32], f: F)
where
    F: Fn(&mut Node, &Reach, Board, &mut [f32]) + Sync,
{
    let np = out.len();
    let mut buf = vec![0.0f32; node.cards.len() * np];
    node.children.par_iter_mut().zip(node.cards.par_iter()).zip(buf.par_chunks_mut(np)).for_each(|((child, &card), o)| {
        let dealt = game.deal(reach, card);
        f(child, &slices(&dealt), board.with(card), o)
    });
    let factor = 1.0 / game.chance_factor(board.count);
    for (h, o) in out.iter_mut().enumerate() {
        let s: f64 = (0..node.cards.len()).map(|c| buf[c * np + h] as f64).sum();
        *o = (s * factor) as f32;
    }
}

/// Runs `f` on each child with its own slice of `buf`, in parallel near the root.
fn each_child<F>(children: &mut [Node], buf: &mut [f32], np: usize, depth: usize, f: F)
where
    F: Fn(usize, &mut Node, &mut [f32]) + Sync,
{
    if depth < PAR_DEPTH {
        children.par_iter_mut().zip(buf.par_chunks_mut(np)).enumerate().for_each(|(a, (c, o))| f(a, c, o));
    } else {
        children.iter_mut().zip(buf.chunks_mut(np)).enumerate().for_each(|(a, (c, o))| f(a, c, o));
    }
}

/// Player `q`'s reach after taking action `a` with strategy `sigma`, or `None` if no hand takes
/// it.
fn action_reach(reach: &[f32], sigma: &[f32], a: usize) -> Option<Vec<f32>> {
    let nq = reach.len();
    let r: Vec<f32> = reach.iter().zip(&sigma[a * nq..(a + 1) * nq]).map(|(x, s)| x * s).collect();
    r.iter().any(|&x| x != 0.0).then_some(r)
}

/// Runs `f` with everyone's reach after player `q` takes action `a`, unless no hand of `q`'s
/// takes it: then nothing after it counts for anyone else, and `f` isn't run.
fn after_action(reach: &Reach, q: usize, sigma: &[f32], a: usize, f: impl FnOnce(&Reach)) {
    if let Some(rq) = action_reach(reach[q], sigma, a) {
        let mut r: [&[f32]; MAX_PLAYERS] = [&[]; MAX_PLAYERS];
        r[..reach.len()].copy_from_slice(reach);
        r[q] = &rq;
        f(&r[..reach.len()]);
    }
}

/// One CFR pass for player `p`: fills `out` with `p`'s counterfactual values and updates `p`'s
/// regrets and average strategy.
#[allow(clippy::too_many_arguments)]
fn train(game: &Game, node: &mut Node, p: usize, reach: &Reach, board: Board, t: u32, depth: usize, out: &mut [f32]) {
    match node.kind {
        Kind::Terminal => terminal(game, &node.pot, &board, p, reach, out),
        Kind::Chance => chance(game, node, reach, board, out, |c, r, b, o| train(game, c, p, r, b, t, depth + 1, o)),
        Kind::Decision => {
            let Node { player, actions, children, regret, strat, pot, last, .. } = node;
            let q = *player as usize;
            let k = actions.len();
            let np = out.len();
            let nq = game.hands[q].len();
            let sigma = regret_matching(regret, k, nq);
            let mut cfv = vec![0.0f32; k * np];
            if q == p {
                each_child(children, &mut cfv, np, depth, |a, c, o| {
                    if actions[a] == Act::Fold {
                        fold_value(game, pot, &board, p, reach, o);
                    } else {
                        train(game, c, p, reach, board, t, depth + 1, o);
                    }
                });
                for (h, o) in out.iter_mut().enumerate() {
                    *o = (0..k).map(|a| sigma[a * np + h] * cfv[a * np + h]).sum();
                }
                let d = Discount::since(*last, t);
                *last = t;
                let rp = reach[p];
                for (i, (s, &x)) in strat.iter_mut().zip(&sigma).enumerate() {
                    *s = *s * d.gamma + rp[i % np] * x;
                }
                for (i, r) in regret.iter_mut().enumerate() {
                    let coef = if r.is_sign_positive() { d.alpha } else { d.beta };
                    *r = *r * coef + cfv[i] - out[i % np];
                }
            } else {
                each_child(children, &mut cfv, np, depth, |a, c, o| {
                    after_action(reach, q, &sigma, a, |r| train(game, c, p, r, board, t, depth + 1, o));
                });
                for (h, o) in out.iter_mut().enumerate() {
                    *o = (0..k).map(|a| cfv[a * np + h]).sum();
                }
            }
        }
    }
}

/// Counterfactual values with everyone playing the average strategy (`avg`) and with player `p`
/// best-responding to it instead (`br`).
#[allow(clippy::too_many_arguments)]
fn best_response(game: &Game, node: &Node, p: usize, reach: &Reach, board: Board, depth: usize, br: &mut [f32], avg: &mut [f32]) {
    let np = br.len();
    match node.kind {
        Kind::Terminal => {
            terminal(game, &node.pot, &board, p, reach, br);
            avg.copy_from_slice(br);
        }
        Kind::Chance => {
            let nc = node.cards.len();
            let mut bb = vec![0.0f32; nc * np];
            let mut ba = vec![0.0f32; nc * np];
            node.children
                .par_iter()
                .zip(node.cards.par_iter())
                .zip(bb.par_chunks_mut(np).zip(ba.par_chunks_mut(np)))
                .for_each(|((c, &card), (ob, oa))| {
                    let dealt = game.deal(reach, card);
                    best_response(game, c, p, &slices(&dealt), board.with(card), depth + 1, ob, oa)
                });
            let factor = 1.0 / game.chance_factor(board.count);
            for h in 0..np {
                br[h] = ((0..nc).map(|c| bb[c * np + h] as f64).sum::<f64>() * factor) as f32;
                avg[h] = ((0..nc).map(|c| ba[c * np + h] as f64).sum::<f64>() * factor) as f32;
            }
        }
        Kind::Decision => {
            let q = node.player as usize;
            let k = node.actions.len();
            let sigma = average_strategy(game, node);
            let mut bb = vec![0.0f32; k * np];
            let mut ba = vec![0.0f32; k * np];
            let run = |a: usize, c: &Node, ob: &mut [f32], oa: &mut [f32]| {
                if q == p && node.actions[a] == Act::Fold {
                    fold_value(game, &node.pot, &board, p, reach, ob);
                    oa.copy_from_slice(ob);
                } else if q == p {
                    best_response(game, c, p, reach, board, depth + 1, ob, oa);
                } else {
                    after_action(reach, q, &sigma, a, |r| best_response(game, c, p, r, board, depth + 1, ob, oa));
                }
            };
            if depth < PAR_DEPTH {
                node.children.par_iter().zip(bb.par_chunks_mut(np).zip(ba.par_chunks_mut(np))).enumerate().for_each(|(a, (c, (ob, oa)))| run(a, c, ob, oa));
            } else {
                node.children.iter().zip(bb.chunks_mut(np).zip(ba.chunks_mut(np))).enumerate().for_each(|(a, (c, (ob, oa)))| run(a, c, ob, oa));
            }
            for h in 0..np {
                if q == p {
                    br[h] = (0..k).map(|a| bb[a * np + h]).fold(f32::NEG_INFINITY, f32::max);
                    avg[h] = (0..k).map(|a| sigma[a * np + h] * ba[a * np + h]).sum();
                } else {
                    br[h] = (0..k).map(|a| bb[a * np + h]).sum();
                    avg[h] = (0..k).map(|a| ba[a * np + h]).sum();
                }
            }
        }
    }
}

/// How far the average strategy is from an equilibrium.
#[derive(Clone, Debug, Default)]
pub struct Exploitability {
    /// What each player would gain by best-responding, in chips.
    pub gain: Vec<f64>,
    /// Each player's expected result with the average strategies (pot share at the end minus
    /// what they put in), in chips.
    pub value: Vec<f64>,
}

impl Exploitability {
    /// The average gain over the players, in chips.
    pub fn chips(&self) -> f64 {
        self.gain.iter().sum::<f64>() / self.gain.len().max(1) as f64
    }

    pub fn pct_of_pot(&self, starting_pot: i32) -> f64 {
        self.chips() / starting_pot as f64 * 100.0
    }
}

pub fn exploitability(game: &Game) -> Exploitability {
    let mut e = Exploitability::default();
    for p in 0..game.n {
        let np = game.hands[p].len();
        let (mut br, mut avg) = (vec![0.0f32; np], vec![0.0f32; np]);
        best_response(game, &game.root, p, &slices(&game.weights), game.start_board(), 0, &mut br, &mut avg);
        let w = &game.weights[p];
        let b: f64 = br.iter().zip(w).map(|(&x, &w)| x as f64 * w as f64).sum();
        let v: f64 = avg.iter().zip(w).map(|(&x, &w)| x as f64 * w as f64).sum();
        e.gain.push((b - v).max(0.0));
        e.value.push(v);
    }
    e
}

/// One iteration: a CFR pass for each player in turn.
pub fn iterate(game: &mut Game) {
    let mut root = std::mem::replace(&mut game.root, Node::placeholder());
    for p in 0..game.n {
        let mut out = vec![0.0f32; game.hands[p].len()];
        train(game, &mut root, p, &slices(&game.weights), game.start_board(), game.iteration, 0, &mut out);
    }
    game.root = root;
    game.iteration += 1;
}

fn normalize_all(node: &mut Node, hands: &[Vec<(u8, u8)>]) {
    if node.kind == Kind::Decision {
        normalize(&mut node.strat, node.actions.len(), hands[node.player as usize].len());
    }
    node.children.par_iter_mut().for_each(|c| normalize_all(c, hands));
}

/// Fills in each decision's action values for the player to act, and every node's `vals`, with
/// everyone playing the (normalized) average strategy. `vals[p]` is left at 0 after `p` folds.
fn values(game: &Game, node: &mut Node, p: usize, reach: &Reach, board: Board, depth: usize, out: &mut [f32]) {
    match node.kind {
        Kind::Terminal => terminal(game, &node.pot, &board, p, reach, out),
        Kind::Chance => chance(game, node, reach, board, out, |c, r, b, o| values(game, c, p, r, b, depth + 1, o)),
        Kind::Decision => {
            let Node { player, actions, children, regret, strat, pot, .. } = node;
            let q = *player as usize;
            let k = actions.len();
            let np = out.len();
            let nq = reach[q].len();
            let mut cfv = vec![0.0f32; k * np];
            let sigma: &[f32] = strat;
            // Everyone's reach after each action, the acting player's own too, so that `vals`
            // weighs each hand by how often it really gets to a node.
            each_child(children, &mut cfv, np, depth, |a, c, o| {
                let rq: Vec<f32> = reach[q].iter().zip(&sigma[a * nq..(a + 1) * nq]).map(|(x, s)| x * s).collect();
                let mut r: [&[f32]; MAX_PLAYERS] = [&[]; MAX_PLAYERS];
                r[..reach.len()].copy_from_slice(reach);
                r[q] = &rq;
                let r = &r[..reach.len()];
                if q == p && actions[a] == Act::Fold {
                    fold_value(game, pot, &board, p, r, o);
                } else {
                    values(game, c, p, r, board, depth + 1, o);
                }
            });
            if q == p {
                for (h, o) in out.iter_mut().enumerate() {
                    *o = (0..k).map(|a| sigma[a * np + h] * cfv[a * np + h]).sum();
                }
                regret.copy_from_slice(&cfv);
            } else {
                for (h, o) in out.iter_mut().enumerate() {
                    *o = (0..k).map(|a| cfv[a * np + h]).sum();
                }
            }
        }
    }
    node.vals[p] = reach[p].iter().zip(out.iter()).map(|(&r, &v)| r as f64 * v as f64).sum::<f64>() as f32;
}

/// Ends solving: turns the cumulative strategies into average strategies and stores each
/// decision's action values. The game can't be solved further after this.
pub fn finalize(game: &mut Game) {
    if game.finalized {
        return;
    }
    let mut root = std::mem::replace(&mut game.root, Node::placeholder());
    normalize_all(&mut root, &game.hands);
    game.finalized = true;
    for p in 0..game.n {
        let mut out = vec![0.0f32; game.hands[p].len()];
        values(game, &mut root, p, &slices(&game.weights), game.start_board(), 0, &mut out);
    }
    game.root = root;
}

/// Solves until `max_iters` iterations, the target exploitability (% of the starting pot) or
/// `cancel`. Calls `progress(iteration, exploitability %)` after each iteration, with the
/// exploitability every `check_every` iterations and at the end. Returns the last exploitability.
pub fn solve(game: &mut Game, max_iters: u32, target_pct: f64, check_every: u32, cancel: &AtomicBool, mut progress: impl FnMut(u32, Option<f64>)) -> Exploitability {
    let mut last = None;
    while game.iteration < max_iters && !cancel.load(Ordering::SeqCst) {
        iterate(game);
        if game.iteration % check_every.max(1) == 0 {
            let e = exploitability(game);
            let pct = e.pct_of_pot(game.cfg.starting_pot);
            last = Some(e);
            progress(game.iteration, Some(pct));
            if pct <= target_pct {
                break;
            }
        } else {
            progress(game.iteration, None);
        }
    }
    let e = match last {
        Some(e) if game.iteration % check_every.max(1) == 0 => e,
        _ => {
            let e = exploitability(game);
            progress(game.iteration, Some(e.pct_of_pot(game.cfg.starting_pot)));
            e
        }
    };
    finalize(game);
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiway::spot::{MultiSpot, Sizes, StreetSizes};
    use crate::Rake;

    fn street(bet: &str, raise: &str) -> StreetSizes {
        StreetSizes { bet: vec![bet.to_string(); 2], raise: raise.into() }
    }

    /// postflop-solver's value of the same heads-up spot: each player's average EV at the root, in
    /// chips.
    fn reference(spot: &MultiSpot, board: &str) -> [f64; 2] {
        use postflop_solver::*;
        let card = |i: usize| card_from_str(&board[i..i + 2]).unwrap();
        let card_config = CardConfig {
            range: [spot.ranges[0].parse().unwrap(), spot.ranges[1].parse().unwrap()],
            flop: flop_from_str(&board[..6]).unwrap(),
            turn: if board.len() >= 8 { card(6) } else { NOT_DEALT },
            river: if board.len() == 10 { card(8) } else { NOT_DEALT },
        };
        let sizes = |s: &StreetSizes| -> [BetSizeOptions; 2] { [(s.bet[0].as_str(), s.raise.as_str()).try_into().unwrap(), (s.bet[1].as_str(), s.raise.as_str()).try_into().unwrap()] };
        let tree_config = TreeConfig {
            initial_state: match board.len() {
                6 => BoardState::Flop,
                8 => BoardState::Turn,
                _ => BoardState::River,
            },
            starting_pot: (spot.pot_bb * crate::CHIPS_PER_BB) as i32,
            effective_stack: (spot.stack_bb * crate::CHIPS_PER_BB) as i32,
            rake_rate: spot.rake.rate,
            rake_cap: spot.rake.cap_bb * crate::CHIPS_PER_BB,
            flop_bet_sizes: sizes(&spot.bets.flop),
            turn_bet_sizes: sizes(&spot.bets.turn),
            river_bet_sizes: sizes(&spot.bets.river),
            turn_donk_sizes: None,
            river_donk_sizes: None,
            add_allin_threshold: 1.5,
            force_allin_threshold: 0.15,
            merging_threshold: 0.1,
        };
        let mut game = PostFlopGame::with_config(card_config, ActionTree::new(tree_config).unwrap()).unwrap();
        game.allocate_memory(false);
        solve(&mut game, 3000, 0.0005 * spot.pot_bb as f32 * crate::CHIPS_PER_BB as f32, false);
        game.cache_normalized_weights();
        [0, 1].map(|p| compute_average(&game.expected_values(p), game.normalized_weights(p)) as f64)
    }

    /// With two players this is an ordinary heads-up game, so it must come to the same values as
    /// postflop-solver: the value of a two-player zero-sum game is unique. The stacks are short so
    /// that all-ins (and the board running out) happen, and the rake has a cap.
    #[test]
    fn heads_up_matches_postflop_solver() {
        let spot = MultiSpot {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            players: vec!["OOP".into(), "IP".into()],
            ranges: vec!["AA-88,AK-AT,KQ,QJs,JTs,T9s,98s,76s,A5s".into(), "TT-55,AQ-A9,KJ+,QT+,J9s+,T8s+,97s+,86s+".into()],
            pot_bb: 10.0,
            stack_bb: 22.0,
            bets: Sizes { flop: street("50%", "3x"), turn: street("60%", "3x"), river: street("70%", "3x") },
            max_raises: 4,
            rake: Rake { rate: 0.05, cap_bb: 1.0 },
        };
        for board in ["Qs7d2c5h9c", "Qs7d2c5h"] {
            let want = reference(&spot, board);
            let mut game = Game::new(&spot, board).unwrap();
            let e = solve(&mut game, 3000, 0.02, 10, &AtomicBool::new(false), |_, _| {});
            let tol = 0.004 * game.cfg.starting_pot as f64;
            for p in 0..2 {
                assert!((e.value[p] - want[p]).abs() < tol, "{board}: player {p} {} vs postflop-solver {}", e.value[p], want[p]);
            }
        }
    }
}
