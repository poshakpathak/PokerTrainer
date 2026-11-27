//! Discounted CFR over the preflop tree, with one strategy per hand class for each player.
//!
//! Every player's range is a vector of 169 reach probabilities. A hand's counterfactual value
//! treats the other players' hands as independent given its own, with exact card removal
//! between it and each of them, so blockers count (for instance, the hands that fold in front
//! of the button hold fewer aces than average). Three-way pots also use exact removal between
//! the two opponents.

use super::cards::{classes, combo_counts, Shape, NUM_CLASSES};
use super::equity::{idx2, EquityTables};
use super::model;
use super::tree::{ActKind, Kind, Node, Tree};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const H: usize = NUM_CLASSES;
pub type Vec169 = [f32; H];
const ZERO: Vec169 = [0.0; H];
const ONE: Vec169 = [1.0; H];
/// Nodes this close to the root solve their children in parallel.
const PAR_DEPTH: usize = 6;
/// In three-way pots, the equity leaves out opponent hands with less than this share of the
/// opponent's highest reach (they barely move it, and skipping them saves most of the work).
const THREE_WAY_SKIP: f32 = 1e-3;
/// Three-way pots scale the heads-up position edge and playability by this: each player's base
/// share is a third of the pot instead of a half.
const THREE_WAY_SCALE: f64 = 2.0 / 3.0;

/// Card removal between two players, in O(169): for a range `r`, the chance that one
/// opponent holds a hand of `r`, given each of our classes.
pub struct Removal {
    weight: Vec169,
    hi: [usize; H],
    lo: [usize; H],
    /// For each rank, the classes holding it and how many of their combos contain one given
    /// card of that rank.
    by_rank: Vec<Vec<(usize, f32)>>,
}

impl Removal {
    pub fn new() -> Removal {
        let mut r = Removal { weight: *combo_counts(), hi: [0; H], lo: [0; H], by_rank: vec![Vec::new(); 13] };
        for (i, c) in classes().iter().enumerate() {
            r.hi[i] = c.hi as usize;
            r.lo[i] = c.lo as usize;
            match c.shape {
                Shape::Pair => r.by_rank[c.hi as usize].push((i, 3.0)),
                Shape::Suited => {
                    r.by_rank[c.hi as usize].push((i, 1.0));
                    r.by_rank[c.lo as usize].push((i, 1.0));
                }
                Shape::Offsuit => {
                    r.by_rank[c.hi as usize].push((i, 3.0));
                    r.by_rank[c.lo as usize].push((i, 3.0));
                }
            }
        }
        r
    }

    /// `out[a] = sum over b of n2[a][b] * r[b]`: all combos, minus those that share a card with
    /// ours, plus the one combo (our own) subtracted twice.
    pub fn mass(&self, r: &Vec169, out: &mut Vec169) {
        let mut total = 0.0f32;
        for b in 0..H {
            total += self.weight[b] * r[b];
        }
        let mut u = [0.0f32; 13];
        for (rank, list) in self.by_rank.iter().enumerate() {
            u[rank] = list.iter().map(|&(b, c)| c * r[b]).sum();
        }
        for a in 0..H {
            out[a] = (total - u[self.hi[a]] - u[self.lo[a]] + r[a]).max(0.0) * (1.0 / 1225.0);
        }
    }
}

impl Default for Removal {
    fn default() -> Self {
        Self::new()
    }
}

/// A pot that reaches the flop (or an all-in), ready to evaluate.
struct Pot {
    /// Players in postflop order.
    players: Vec<usize>,
    /// The pot after rake.
    pot_net: f32,
    invested: [f32; 6],
    alpha: f32,
    cubic: f32,
    /// Each player's fixed transfer (position, initiative), as a share of the pot.
    edge: [f32; 6],
    /// Playability per class.
    play: Vec169,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Update regrets and the average strategy.
    Train,
    /// The traverser best-responds to the others' average strategies.
    BestResponse,
    /// Everyone plays the average strategy.
    Evaluate,
}

#[derive(Clone, Copy)]
struct Discount {
    positive: f32,
    negative: f32,
    strategy: f32,
}

const NO_DISCOUNT: Discount = Discount { positive: 1.0, negative: 1.0, strategy: 1.0 };

/// A raw pointer into one of the solver's arrays, shared by the threads of a traversal. Each
/// decision's slots are written by one call at a time: the traverser's own decisions are each
/// visited once per traversal, and nobody else writes during it.
#[derive(Clone, Copy)]
struct Ptr(*mut f32);
unsafe impl Send for Ptr {}
unsafe impl Sync for Ptr {}

impl Ptr {
    /// Safety: no other live reference may write `offset..offset + len` while this one is used.
    #[inline]
    unsafe fn read(&self, offset: usize, len: usize) -> &[f32] {
        std::slice::from_raw_parts(self.0.add(offset), len)
    }

    /// Safety: as `read`, and nothing else may read it either.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    unsafe fn write(&self, offset: usize, len: usize) -> &mut [f32] {
        std::slice::from_raw_parts_mut(self.0.add(offset), len)
    }
}

struct Walk {
    p: usize,
    mode: Mode,
    d: Discount,
    regret: Ptr,
    strat: Ptr,
    /// Evaluate mode: where to store the traverser's per-action values.
    ev_out: Option<Ptr>,
    /// Play the current strategy (regret matching) instead of the average.
    current: bool,
}

/// How far the average strategy is from an equilibrium: what each player could gain per hand
/// by best-responding while the others keep their strategies.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Exploitability {
    /// Gain per player, in bb per hand.
    pub per_player: [f64; 6],
    /// Each player's expected result, in bb per hand.
    pub ev: [f64; 6],
    /// Players dealt in.
    pub players: usize,
}

impl Exploitability {
    /// The average gain over the players, in mbb per hand.
    pub fn mbb(&self) -> f64 {
        self.per_player.iter().sum::<f64>() / self.players.max(1) as f64 * 1000.0
    }
}

pub struct Solver {
    pub tree: Tree,
    tables: Arc<EquityTables>,
    removal: Removal,
    players: Vec<usize>,
    /// `n2 * eq2`.
    ne2: Vec<f32>,
    /// `n2 * (eq2 - 1/2)^3`, for the model's cubic term.
    nx3: Vec<f32>,
    pots: Vec<Pot>,
    regret: Vec<f32>,
    strat_sum: Vec<f32>,
    /// Each action's value at every decision (bb, compared with folding there), for the player
    /// to act, from `evaluate`.
    action_ev: Vec<f32>,
    pub iteration: u32,
}

impl Solver {
    pub fn new(tree: Tree, tables: Arc<EquityTables>) -> Solver {
        let ne2: Vec<f32> = tables.n2.iter().zip(&tables.eq2).map(|(n, e)| n * e).collect();
        let nx3: Vec<f32> = tables.n2.iter().zip(&tables.eq2).map(|(n, e)| n * (e - 0.5).powi(3)).collect();
        let calibrated = tree.config.postflop == "calibrated";
        let pots = tree
            .showdowns
            .iter()
            .map(|sd| {
                let node = &tree.nodes[sd.node as usize];
                let p = model::params(if calibrated { sd.spr } else { 0.0 });
                let n = sd.players.len();
                let mut edge = [0.0f64; 6];
                // Position: the last to act gains what the first loses (in a three-way pot the
                // middle player breaks even).
                let position = if n == 2 { p.position } else { p.position * THREE_WAY_SCALE };
                edge[sd.players[0]] -= position;
                edge[sd.players[n - 1]] += position;
                if let Some(agg) = sd.aggressor.filter(|a| sd.players.contains(a)) {
                    for &s in &sd.players {
                        edge[s] += if s == agg { p.initiative } else { -p.initiative / (n - 1) as f64 };
                    }
                }
                Pot {
                    players: sd.players.clone(),
                    pot_net: (sd.pot - sd.rake) as f32,
                    invested: node.invested.map(|x| x as f32),
                    alpha: if n == 2 { p.alpha as f32 } else { 1.0 },
                    cubic: if n == 2 { p.cubic as f32 } else { 0.0 },
                    edge: edge.map(|x| x as f32),
                    play: model::playability(&p),
                }
            })
            .collect();
        let slots = tree.slots;
        let players = tree.players();
        Solver {
            tree,
            tables,
            removal: Removal::new(),
            players,
            ne2,
            nx3,
            pots,
            regret: vec![0.0; slots],
            strat_sum: vec![0.0; slots],
            action_ev: vec![0.0; slots],
            iteration: 0,
        }
    }

    /// One iteration: a regret update for each player in turn (alternating updates).
    pub fn step(&mut self) {
        self.iteration += 1;
        let t = self.iteration as f64;
        // DCFR with alpha = 1.5, beta = 0, gamma = 2 (Brown and Sandholm, 2019): what was
        // accumulated through iteration t - 1 is discounted before iteration t adds to it.
        let ta = (t - 1.0).powf(1.5);
        let d = Discount { positive: (ta / (ta + 1.0)) as f32, negative: 0.5, strategy: ((t - 1.0) / t).powi(2) as f32 };
        let regret = Ptr(self.regret.as_mut_ptr());
        let strat = Ptr(self.strat_sum.as_mut_ptr());
        for &p in &self.players {
            let w = Walk { p, mode: Mode::Train, d, regret, strat, ev_out: None, current: true };
            self.walk(&w, 0, [&ONE; 6], &ONE, 0);
        }
    }

    /// Iterates until `max_iters`, the target exploitability (mbb per hand) or `cancel`,
    /// measuring every `check_every` iterations. `progress` gets the iteration and any new
    /// measurement. Fills the per-action values at the end.
    pub fn run(
        &mut self,
        max_iters: u32,
        target_mbb: f64,
        check_every: u32,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u32, Option<&Exploitability>),
    ) -> Exploitability {
        let mut last: Option<(u32, Exploitability)> = None;
        while self.iteration < max_iters && !cancel.load(Ordering::Relaxed) {
            self.step();
            let it = self.iteration;
            if it % check_every.max(1) == 0 || it == max_iters {
                let e = self.exploitability();
                progress(it, Some(&e));
                let done = e.mbb() <= target_mbb;
                last = Some((it, e));
                if done {
                    break;
                }
            } else {
                progress(it, None);
            }
        }
        let e = match last {
            Some((it, e)) if it == self.iteration => e,
            _ => self.exploitability(),
        };
        self.evaluate();
        e
    }

    fn read_walk(&self, p: usize, mode: Mode, ev_out: Option<Ptr>) -> Walk {
        // Read-only modes never write through these.
        let regret = Ptr(self.regret.as_ptr() as *mut f32);
        let strat = Ptr(self.strat_sum.as_ptr() as *mut f32);
        Walk { p, mode, d: NO_DISCOUNT, regret, strat, ev_out, current: false }
    }

    pub fn exploitability(&self) -> Exploitability {
        let prior = prior();
        let mut e = Exploitability { players: self.players.len(), ..Default::default() };
        for &p in &self.players {
            let br = self.walk(&self.read_walk(p, Mode::BestResponse, None), 0, [&ONE; 6], &ONE, 0);
            let ev = self.walk(&self.read_walk(p, Mode::Evaluate, None), 0, [&ONE; 6], &ONE, 0);
            let (mut vb, mut ve) = (0.0f64, 0.0f64);
            for a in 0..H {
                vb += prior[a] as f64 * br[a] as f64;
                ve += prior[a] as f64 * ev[a] as f64;
            }
            e.per_player[p] = (vb - ve).max(0.0);
            e.ev[p] = ve;
        }
        e
    }

    /// Computes every action's value for the player to act, from the average strategy.
    pub fn evaluate(&mut self) {
        let mut out = std::mem::take(&mut self.action_ev);
        let ptr = Ptr(out.as_mut_ptr());
        for &p in &self.players {
            self.walk(&self.read_walk(p, Mode::Evaluate, Some(ptr)), 0, [&ONE; 6], &ONE, 0);
        }
        self.action_ev = out;
    }

    /// The average strategy at a decision: one vector of 169 frequencies per action.
    pub fn strategy(&self, id: usize) -> Vec<Vec169> {
        let node = &self.tree.nodes[id];
        let mut out = vec![ZERO; node.actions.len()];
        regret_match(&self.strat_sum[node.slot..node.slot + node.actions.len() * H], &mut out);
        out
    }

    /// Each action's value at a decision for the player to act, in bb compared with folding
    /// there (from `evaluate`).
    pub fn action_values(&self, id: usize) -> Vec<Vec169> {
        let node = &self.tree.nodes[id];
        (0..node.actions.len()).map(|i| self.action_ev[node.slot + i * H..node.slot + (i + 1) * H].try_into().unwrap()).collect()
    }

    /// Every player's reach at a node under the average strategy: the chance that each hand
    /// class plays to it.
    pub fn reach(&self, id: usize) -> [Vec169; 6] {
        let mut reach = [ONE; 6];
        let mut node = 0usize;
        for step in self.tree.history(id) {
            let n = &self.tree.nodes[node];
            let sigma = self.strategy(node);
            for a in 0..H {
                reach[n.seat][a] *= sigma[step][a];
            }
            node = n.children[step] as usize;
        }
        reach
    }

    /// The equity tables in use.
    pub fn tables(&self) -> &EquityTables {
        &self.tables
    }

    fn sigma(&self, w: &Walk, node: &Node, out: &mut [Vec169]) {
        let len = node.actions.len() * H;
        let src = if w.current { w.regret } else { w.strat };
        // Safety: only the traverser's decisions are written, by the call that visits them, and
        // that call reads them here before writing.
        regret_match(unsafe { src.read(node.slot, len) }, out);
    }

    /// The chance that every other player holds a hand that plays to this node, given ours.
    fn others_mass(&self, node: &Node, p: usize, reach: &[&Vec169; 6], fold_mass: &Vec169) -> Vec169 {
        let mut m = *fold_mass;
        let mut tmp = ZERO;
        for s in 0..6 {
            if s != p && !node.is_folded(s) {
                self.removal.mass(reach[s], &mut tmp);
                for a in 0..H {
                    m[a] *= tmp[a];
                }
            }
        }
        m
    }

    /// The traverser's counterfactual value of each hand at a node. `reach` holds every
    /// player's reach; `fold_mass` the chance that the players who folded so far held the
    /// hands they folded, given ours.
    fn walk(&self, w: &Walk, id: usize, reach: [&Vec169; 6], fold_mass: &Vec169, depth: usize) -> Vec169 {
        let node = &self.tree.nodes[id];
        let p = w.p;
        match node.kind {
            // Everyone else folded (the traverser's own folds are valued where they happen).
            Kind::Fold => return scale(fold_mass, (node.pot() - node.invested[p]) as f32),
            Kind::Showdown => return self.showdown(node.showdown, p, &reach, fold_mass),
            Kind::Decision => {}
        }
        let k = node.actions.len();
        let q = node.seat;
        let mut sigma = vec![ZERO; k];
        self.sigma(w, node, &mut sigma);

        let child_value = |i: usize| -> Vec169 {
            let act = node.actions[i];
            if q == p && act.kind == ActKind::Fold {
                // Folding loses what's in, whatever happens next.
                return scale(&self.others_mass(node, p, &reach, fold_mass), -(node.invested[p] as f32));
            }
            let mut r = ZERO;
            let mut any = false;
            for a in 0..H {
                r[a] = reach[q][a] * sigma[i][a];
                any |= r[a] > 0.0;
            }
            if q != p && !any {
                // Nobody gets here, so every value is zero, but the traverser's regrets still
                // decay and its average strategy still counts what it would play.
                if w.mode == Mode::Train {
                    self.train_unreached(w, node.children[i] as usize, reach[p]);
                }
                return ZERO;
            }
            let mut next = reach;
            next[q] = &r;
            let child = node.children[i] as usize;
            if act.kind == ActKind::Fold {
                let mut m = ZERO;
                self.removal.mass(&r, &mut m);
                for a in 0..H {
                    m[a] *= fold_mass[a];
                }
                self.walk(w, child, next, &m, depth + 1)
            } else {
                self.walk(w, child, next, fold_mass, depth + 1)
            }
        };
        let values: Vec<Vec169> =
            if depth < PAR_DEPTH && k > 1 { (0..k).into_par_iter().map(child_value).collect() } else { (0..k).map(child_value).collect() };

        let mut v = ZERO;
        if q != p {
            for val in &values {
                for a in 0..H {
                    v[a] += val[a];
                }
            }
            return v;
        }
        if w.mode == Mode::BestResponse {
            for a in 0..H {
                v[a] = values.iter().map(|x| x[a]).fold(f32::NEG_INFINITY, f32::max);
            }
            return v;
        }
        for (s, val) in sigma.iter().zip(&values) {
            for a in 0..H {
                v[a] += s[a] * val[a];
            }
        }
        match w.mode {
            Mode::Train => {
                // Safety: the traverser's decisions are visited once per traversal.
                let regret = unsafe { w.regret.write(node.slot, k * H) };
                let strat = unsafe { w.strat.write(node.slot, k * H) };
                let own = reach[p];
                for i in 0..k {
                    for a in 0..H {
                        let r = &mut regret[i * H + a];
                        *r = *r * if *r > 0.0 { w.d.positive } else { w.d.negative } + values[i][a] - v[a];
                        let s = &mut strat[i * H + a];
                        *s = *s * w.d.strategy + own[a] * sigma[i][a];
                    }
                }
            }
            Mode::Evaluate => {
                if let Some(out) = w.ev_out {
                    // Per hand: divide by the chance the others get here, and count from
                    // folding now (what's already in is gone either way).
                    let m = self.others_mass(node, p, &reach, fold_mass);
                    let sunk = node.invested[p] as f32;
                    // Safety: as in Train mode, and `out` isn't read during the traversal.
                    let slot = unsafe { out.write(node.slot, k * H) };
                    for i in 0..k {
                        for a in 0..H {
                            slot[i * H + a] = if m[a] > 1e-12 { values[i][a] / m[a] + sunk } else { 0.0 };
                        }
                    }
                }
            }
            Mode::BestResponse => unreachable!(),
        }
        v
    }

    /// Train mode below a line the others never play: discounts the traverser's regrets and
    /// adds its strategy to the average, as at any other node.
    fn train_unreached(&self, w: &Walk, id: usize, own: &Vec169) {
        let node = &self.tree.nodes[id];
        if node.kind != Kind::Decision {
            return;
        }
        let k = node.actions.len();
        let mut sigma = vec![ZERO; k];
        self.sigma(w, node, &mut sigma);
        if node.seat != w.p {
            for &c in &node.children {
                self.train_unreached(w, c as usize, own);
            }
            return;
        }
        {
            // Safety: as in `walk`.
            let regret = unsafe { w.regret.write(node.slot, k * H) };
            let strat = unsafe { w.strat.write(node.slot, k * H) };
            for i in 0..k {
                for a in 0..H {
                    let r = &mut regret[i * H + a];
                    *r *= if *r > 0.0 { w.d.positive } else { w.d.negative };
                    let s = &mut strat[i * H + a];
                    *s = *s * w.d.strategy + own[a] * sigma[i][a];
                }
            }
        }
        for (i, &c) in node.children.iter().enumerate() {
            let mut next = ZERO;
            for a in 0..H {
                next[a] = own[a] * sigma[i][a];
            }
            self.train_unreached(w, c as usize, &next);
        }
    }

    /// The traverser's value at a showdown, for each of its hands.
    fn showdown(&self, idx: usize, p: usize, reach: &[&Vec169; 6], fold_mass: &Vec169) -> Vec169 {
        let pot = &self.pots[idx];
        let others: Vec<usize> = pot.players.iter().copied().filter(|&s| s != p).collect();
        let inv = pot.invested[p];
        let mut v = ZERO;
        if let [j] = others[..] {
            let r = reach[j];
            let mut m = ZERO;
            self.removal.mass(r, &mut m);
            // Equity against the range, weighted by card removal: e[a] = sum_b n2 eq2 r[b].
            let mut e = ZERO;
            for a in 0..H {
                let row = &self.ne2[idx2(a, 0)..idx2(a, 0) + H];
                e[a] = row.iter().zip(r).map(|(x, y)| x * y).sum();
            }
            // The opponent's playability: f[a] = sum_b n2 play[b] r[b].
            let mut rp = ZERO;
            for b in 0..H {
                rp[b] = r[b] * pot.play[b];
            }
            let mut f = ZERO;
            self.removal.mass(&rp, &mut f);
            // The cubic term: sum_b n2 (eq2 - 1/2)^3 r[b].
            if pot.cubic != 0.0 {
                for a in 0..H {
                    let row = &self.nx3[idx2(a, 0)..idx2(a, 0) + H];
                    e[a] = pot.alpha * e[a] + pot.cubic * row.iter().zip(r).map(|(x, y)| x * y).sum::<f32>();
                }
            } else {
                for x in e.iter_mut() {
                    *x *= pot.alpha;
                }
            }
            let base = 0.5 - pot.alpha / 2.0 + pot.edge[p];
            for a in 0..H {
                let share = (base + pot.play[a]) * m[a] + e[a] - f[a];
                v[a] = fold_mass[a] * (pot.pot_net * share - inv * m[a]);
            }
            return v;
        }
        // Three-way: equity with exact removal among the three, applied to the same
        // independent-opponents mass the rest of the tree uses so the values stay consistent.
        let (rj, rk) = (reach[others[0]], reach[others[1]]);
        let t = &self.tables;
        let live = |r: &Vec169| -> Vec<(usize, f32)> {
            let cut = r.iter().fold(0.0f32, |m, &x| m.max(x)) * THREE_WAY_SKIP;
            (0..H).filter(|&b| r[b] > cut).map(|b| (b, r[b])).collect()
        };
        let (live_j, live_k) = (live(rj), live(rk));
        let dense_k = live_k.len() * 3 > H;
        let rows: Vec<(f32, f32)> = (0..H)
            .into_par_iter()
            .map(|a| {
                let (mut s0, mut s1) = (0.0f32, 0.0f32);
                for &(b, wb) in &live_j {
                    let off = (a * H + b) * H;
                    let n3 = &t.n3[off..off + H];
                    let ne3 = &t.ne3[off..off + H];
                    let (mut x0, mut x1) = (0.0f32, 0.0f32);
                    if dense_k {
                        for c in 0..H {
                            x0 += n3[c] * rk[c];
                            x1 += ne3[c] * rk[c];
                        }
                    } else {
                        for &(c, wc) in &live_k {
                            x0 += n3[c] * wc;
                            x1 += ne3[c] * wc;
                        }
                    }
                    s0 += wb * x0;
                    s1 += wb * x1;
                }
                (s0, s1)
            })
            .collect();
        let (mut mj, mut mk, mut pj, mut pk) = (ZERO, ZERO, ZERO, ZERO);
        self.removal.mass(rj, &mut mj);
        self.removal.mass(rk, &mut mk);
        // The opponents' playability, as in heads-up pots: pj[a] / mj[a] is the average over
        // the first opponent's range, given our hand.
        self.removal.mass(&std::array::from_fn(|b| rj[b] * pot.play[b]), &mut pj);
        self.removal.mass(&std::array::from_fn(|b| rk[b] * pot.play[b]), &mut pk);
        let scale = THREE_WAY_SCALE as f32;
        for a in 0..H {
            let (s0, s1) = rows[a];
            if s0 > 0.0 && mj[a] > 0.0 && mk[a] > 0.0 {
                let others = (pj[a] / mj[a] + pk[a] / mk[a]) / 2.0;
                let share = s1 / s0 + pot.edge[p] + scale * (pot.play[a] - others);
                v[a] = fold_mass[a] * mj[a] * mk[a] * (pot.pot_net * share - inv);
            }
        }
        v
    }
}

/// Regret matching (or normalizing an average strategy): `src` holds one block of 169 per
/// action; hands with nothing positive play every action equally.
fn regret_match(src: &[f32], out: &mut [Vec169]) {
    let k = out.len();
    for a in 0..H {
        let mut sum = 0.0f32;
        for i in 0..k {
            sum += src[i * H + a].max(0.0);
        }
        for i in 0..k {
            out[i][a] = if sum > 0.0 { src[i * H + a].max(0.0) / sum } else { 1.0 / k as f32 };
        }
    }
}

fn scale(v: &Vec169, x: f32) -> Vec169 {
    v.map(|y| y * x)
}

/// The chance of being dealt each class.
pub fn prior() -> Vec169 {
    combo_counts().map(|w| w / 1326.0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::preflop::cards::class_index;
    use crate::preflop::tree::{Config, SB};
    use std::sync::OnceLock;

    /// The equity tables, cached where the command-line tools keep them.
    pub(crate) fn tables() -> Arc<EquityTables> {
        static T: OnceLock<Arc<EquityTables>> = OnceLock::new();
        T.get_or_init(|| Arc::new(EquityTables::load_or_compute(Some(&crate::preflop::equity::default_cache_path()), &|_| {})))
        .clone()
    }

    #[test]
    fn removal_matches_table() {
        let tables = tables();
        let rem = Removal::new();
        let mut r = ZERO;
        for a in 0..H {
            r[a] = ((a * 37) % 11) as f32 / 10.0;
        }
        let mut fast = ZERO;
        rem.mass(&r, &mut fast);
        for a in 0..H {
            let slow: f32 = (0..H).map(|b| tables.n2[idx2(a, b)] * r[b]).sum();
            assert!((fast[a] - slow).abs() < 1e-5, "{a}: {} vs {}", fast[a], slow);
        }
    }

    fn solve(cfg: Config, iters: u32) -> (Solver, Exploitability) {
        let tree = Tree::build(&cfg).unwrap();
        let mut s = Solver::new(tree, tables());
        let e = s.run(iters, 0.0, iters, &AtomicBool::new(false), &mut |_, _| {});
        (s, e)
    }

    /// Share of all combos that take action `i` at node `id`, weighted by reach.
    fn frequency(s: &Solver, id: usize, i: usize) -> f64 {
        let seat = s.tree.nodes[id].seat;
        let reach = s.reach(id)[seat];
        let sigma = s.strategy(id);
        let w = combo_counts();
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for a in 0..H {
            num += (w[a] * reach[a] * sigma[i][a]) as f64;
            den += (w[a] * reach[a]) as f64;
        }
        num / den
    }

    /// Heads-up push or fold at 10bb has a known equilibrium: the SB shoves about 58% of hands
    /// and the BB calls about 37%.
    #[test]
    fn push_fold_10bb() {
        let cfg = Config { start_seat: "SB".into(), stack_bb: 10.0, sb_open_bb: 10.0, postflop: "equity".into(), ..Config::default() };
        let (s, e) = solve(cfg, 400);
        let push = frequency(&s, 0, 1);
        let call_node = s.tree.nodes[0].children[1] as usize;
        let call = frequency(&s, call_node, 1);
        println!("push {push:.3} call {call:.3} exploitability {:.3} mbb", e.mbb());
        assert!((push - 0.58).abs() < 0.03, "push {push}");
        assert!((call - 0.37).abs() < 0.03, "call {call}");
        assert!(e.mbb() < 1.0, "{e:?}");
        // No rake: what one blind wins the other loses.
        assert!((e.ev[SB] + e.ev[SB + 1]).abs() < 1e-3, "{:?}", e.ev);
        // Pocket aces always shove and always call; 72o folds.
        let aa = class_index("AA").unwrap();
        let seven_two = class_index("72o").unwrap();
        assert!(s.strategy(0)[1][aa] > 0.99);
        assert!(s.strategy(call_node)[1][aa] > 0.99);
        assert!(s.strategy(call_node)[1][seven_two] < 0.01);
        // Shoving AA wins the blind most of the time and is a big favourite when called.
        let shove = s.action_values(0)[1][aa];
        println!("AA shove {shove:.3}bb more than a fold");
        assert!(shove > 3.0 && shove < 4.5, "{shove}");
    }

    /// Exploitability goes down as the solve goes on.
    #[test]
    fn converges_blind_vs_blind() {
        let cfg = Config { start_seat: "SB".into(), stack_bb: 40.0, sb_limp: true, ..Config::default() };
        let tree = Tree::build(&cfg).unwrap();
        let mut s = Solver::new(tree, tables());
        let mut seen = Vec::new();
        s.run(300, 0.0, 100, &AtomicBool::new(false), &mut |_, e| {
            if let Some(e) = e {
                seen.push(e.mbb());
            }
        });
        println!("{seen:?}");
        assert!(seen.windows(2).all(|w| w[1] < w[0] * 1.05), "{seen:?}");
        assert!(*seen.last().unwrap() < 5.0, "{seen:?}");
    }

    #[test]
    #[ignore]
    fn timing_six_max() {
        let tables = tables();
        let tree = Tree::build(&Config::default()).unwrap();
        let mut s = Solver::new(tree, tables);
        let t = std::time::Instant::now();
        for _ in 0..10 {
            s.step();
        }
        println!("10 iterations in {:.2}s", t.elapsed().as_secs_f64());
        for _ in 0..90 {
            s.step();
        }
        let t = std::time::Instant::now();
        for _ in 0..10 {
            s.step();
        }
        println!("iterations 101-110 in {:.2}s", t.elapsed().as_secs_f64());
        let t = std::time::Instant::now();
        let e = s.exploitability();
        println!("exploitability {:.1} mbb in {:.2}s: {:?}", e.mbb(), t.elapsed().as_secs_f64(), e);
    }
}
