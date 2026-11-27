//! What each hand gets at the end of a hand, with exact card removal: the board and every
//! player's hand never share a card, and that includes the two opponents' hands with each other.
//!
//! For one of our hands `h`, sums over the two opponents' hands reduce to
//!
//! ```text
//! sum of x(a) y(b) over hands a, b that share no card with h or with each other
//!   = X'(h) Y'(h) - sum over cards c not in h of X'_c(h) Y'_c(h) + sum over a missing h of x(a) y(a)
//! ```
//!
//! where `X'(h)` is the total of `x` over hands that miss `h` and `X'_c(h)` the part of it that
//! holds card `c` (inclusion-exclusion: a pair of hands sharing one card is counted once for that
//! card, the same hand twice). That is 52 steps per hand. Win shares come from sweeping the hands
//! in order of strength and adding the opponents' hands as they're passed, weaker ones in full and
//! tied ones by half: that gives each pair `share_a * share_b`. A three-way tie is worth 1/3, not
//! 1/4, and a second sum over just the tied hands makes up the difference.

use super::spot::MAX_PLAYERS;
use std::cell::RefCell;

/// No hand: a combo that isn't in a player's range.
pub const NONE: u16 = u16::MAX;

/// A hand at showdown on one board: its strength, its cards, and its index in each player's
/// list of hands (or [`NONE`]).
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub strength: u32,
    pub c1: u8,
    pub c2: u8,
    pub idx: [u16; MAX_PLAYERS],
}

/// Values per card in the vector loops: 52 cards padded with zeros so the loops split evenly
/// into vector registers.
const LANES: usize = 64;

/// Weights of one player's hands, summed for card removal.
pub struct Side {
    total: f64,
    /// The weight of the hands holding each card.
    card: [f32; LANES],
    /// The weight of hand {a, b} at both `a * LANES + b` and `b * LANES + a`.
    m: Vec<f32>,
    touched: Vec<u16>,
}

impl Default for Side {
    fn default() -> Self {
        Side { total: 0.0, card: [0.0; LANES], m: vec![0.0; 52 * LANES], touched: Vec::with_capacity(1326) }
    }
}

impl Side {
    #[inline(always)]
    pub fn add(&mut self, c1: u8, c2: u8, w: f32) {
        let (a, b) = (c1 as usize, c2 as usize);
        self.total += w as f64;
        self.card[a] += w;
        self.card[b] += w;
        let k = a * LANES + b;
        if self.m[k] == 0.0 {
            self.touched.push(k as u16);
        }
        self.m[k] += w;
        self.m[b * LANES + a] += w;
    }

    #[inline(always)]
    pub fn get(&self, c1: u8, c2: u8) -> f32 {
        self.m[c1 as usize * LANES + c2 as usize]
    }

    #[inline(always)]
    fn row(&self, a: usize) -> &[f32] {
        &self.m[a * LANES..(a + 1) * LANES]
    }

    /// The total over hands that share no card with {h1, h2}.
    #[inline(always)]
    pub fn avoiding(&self, h1: u8, h2: u8) -> f64 {
        let (a, b) = (h1 as usize, h2 as usize);
        self.total - self.card[a] as f64 - self.card[b] as f64 + self.m[a * LANES + b] as f64
    }

    pub fn clear(&mut self) {
        for &k in &self.touched {
            let (a, b) = (k as usize / LANES, k as usize % LANES);
            self.m[a * LANES + b] = 0.0;
            self.m[b * LANES + a] = 0.0;
        }
        self.touched.clear();
        self.card = [0.0; LANES];
        self.total = 0.0;
    }
}

/// Two opponents' weights, for sums over pairs of hands that don't share a card.
pub struct Pair {
    x: Side,
    y: Side,
    /// Sum of x(a) y(a) over the same hand a, in total and by card.
    joint: f64,
    joint_card: [f64; 52],
}

impl Default for Pair {
    fn default() -> Self {
        Pair { x: Side::default(), y: Side::default(), joint: 0.0, joint_card: [0.0; 52] }
    }
}

impl Pair {
    #[inline(always)]
    pub fn add_x(&mut self, c1: u8, c2: u8, w: f32) {
        let y = self.y.get(c1, c2);
        if y != 0.0 {
            let d = w as f64 * y as f64;
            self.joint += d;
            self.joint_card[c1 as usize] += d;
            self.joint_card[c2 as usize] += d;
        }
        self.x.add(c1, c2, w);
    }

    #[inline(always)]
    pub fn add_y(&mut self, c1: u8, c2: u8, w: f32) {
        let x = self.x.get(c1, c2);
        if x != 0.0 {
            let d = w as f64 * x as f64;
            self.joint += d;
            self.joint_card[c1 as usize] += d;
            self.joint_card[c2 as usize] += d;
        }
        self.y.add(c1, c2, w);
    }

    pub fn clear(&mut self) {
        self.x.clear();
        self.y.clear();
        self.joint = 0.0;
        self.joint_card = [0.0; 52];
    }

    fn is_empty(&self) -> bool {
        self.x.total == 0.0 || self.y.total == 0.0
    }

    /// The sum of x(a) y(b) over hands a and b that share no card with {h1, h2} or each other.
    #[inline(always)]
    pub fn eval(&self, h1: u8, h2: u8) -> f64 {
        let (a, b) = (h1 as usize, h2 as usize);
        let xh = self.x.avoiding(h1, h2);
        let yh = self.y.avoiding(h1, h2);
        let k = a * LANES + b;
        let same = self.joint - self.joint_card[a] - self.joint_card[b] + self.x.m[k] as f64 * self.y.m[k] as f64;
        xh * yh - self.cross(a, b) + same
    }

    /// Sum over cards c not in {a, b} of X'_c Y'_c.
    #[inline(always)]
    fn cross(&self, a: usize, b: usize) -> f64 {
        let (x, y) = (&self.x, &self.y);
        let (xa, xb, ya, yb) = (x.row(a), x.row(b), y.row(a), y.row(b));
        let mut acc = [0.0f32; 16];
        for i in 0..LANES / 16 {
            for l in 0..16 {
                let c = i * 16 + l;
                acc[l] += (x.card[c] - xa[c] - xb[c]) * (y.card[c] - ya[c] - yb[c]);
            }
        }
        let lane = |c: usize| ((x.card[c] - xa[c] - xb[c]) * (y.card[c] - ya[c] - yb[c])) as f64;
        acc.iter().map(|&v| v as f64).sum::<f64>() - lane(a) - lane(b)
    }
}

/// A slot for a card that isn't among [`Few`]'s cards.
const NO: u8 = u8::MAX;
/// The most cards [`Few`] holds.
const FEW: usize = 16;

/// The opponents' hands tied at one strength, when they hold at most 16 different cards (one or
/// two ranks, the usual case): the same sums as [`Pair`] over just those cards, which is much
/// quicker than running the full 52-card sums for every tied hand.
struct Few {
    /// Each card's slot, or [`NO`].
    slot: [u8; 52],
    k: usize,
    cards: [u8; FEW],
    xt: f64,
    yt: f64,
    x: [f32; FEW],
    y: [f32; FEW],
    xm: [[f32; FEW]; FEW],
    ym: [[f32; FEW]; FEW],
    joint: f64,
    joint_card: [f64; FEW],
}

impl Default for Few {
    fn default() -> Self {
        Few {
            slot: [NO; 52],
            k: 0,
            cards: [0; FEW],
            xt: 0.0,
            yt: 0.0,
            x: [0.0; FEW],
            y: [0.0; FEW],
            xm: [[0.0; FEW]; FEW],
            ym: [[0.0; FEW]; FEW],
            joint: 0.0,
            joint_card: [0.0; FEW],
        }
    }
}

impl Few {
    #[inline(always)]
    fn place(&mut self, c: u8) -> bool {
        if self.slot[c as usize] != NO {
            return true;
        }
        if self.k == FEW {
            return false;
        }
        self.slot[c as usize] = self.k as u8;
        self.cards[self.k] = c;
        self.k += 1;
        true
    }

    /// Takes in the hands of a tie `group` held by opponents `q` (as x) and `z` (as y). False,
    /// with nothing taken in, when they hold more than 16 cards.
    #[inline(always)]
    fn fill(&mut self, group: &[Entry], q: usize, z: usize, rq: &[f32], rz: &[f32]) -> bool {
        let w_of = |r: &[f32], i: u16| if i == NONE { 0.0 } else { r[i as usize] };
        for e in group {
            if (w_of(rq, e.idx[q]) != 0.0 || w_of(rz, e.idx[z]) != 0.0) && !(self.place(e.c1) && self.place(e.c2)) {
                self.clear();
                return false;
            }
        }
        for e in group {
            let (a, b) = (self.slot[e.c1 as usize] as usize, self.slot[e.c2 as usize] as usize);
            let wq = w_of(rq, e.idx[q]);
            if wq != 0.0 {
                self.xt += wq as f64;
                self.x[a] += wq;
                self.x[b] += wq;
                self.xm[a][b] += wq;
                self.xm[b][a] += wq;
            }
            let wz = w_of(rz, e.idx[z]);
            if wz != 0.0 {
                self.yt += wz as f64;
                self.y[a] += wz;
                self.y[b] += wz;
                self.ym[a][b] += wz;
                self.ym[b][a] += wz;
            }
            if wq != 0.0 && wz != 0.0 {
                let j = wq as f64 * wz as f64;
                self.joint += j;
                self.joint_card[a] += j;
                self.joint_card[b] += j;
            }
        }
        true
    }

    /// As [`Pair::eval`].
    #[inline(always)]
    fn eval(&self, h1: u8, h2: u8) -> f64 {
        const ZERO: [f32; FEW] = [0.0; FEW];
        let (la, lb) = (self.slot[h1 as usize], self.slot[h2 as usize]);
        let row = |m: &[[f32; FEW]; FEW], l: u8| if l == NO { ZERO } else { m[l as usize] };
        let at = |v: &[f32; FEW], l: u8| if l == NO { 0.0 } else { v[l as usize] };
        let (xa, xb, ya, yb) = (row(&self.xm, la), row(&self.xm, lb), row(&self.ym, la), row(&self.ym, lb));
        let (xab, yab) = (at(&xa, lb), at(&ya, lb));
        let (cxa, cxb, cya, cyb) = (at(&self.x, la), at(&self.x, lb), at(&self.y, la), at(&self.y, lb));
        let xh = self.xt - cxa as f64 - cxb as f64 + xab as f64;
        let yh = self.yt - cya as f64 - cyb as f64 + yab as f64;
        let mut cross = 0.0f32;
        for c in 0..FEW {
            cross += (self.x[c] - xa[c] - xb[c]) * (self.y[c] - ya[c] - yb[c]);
        }
        let mut cross = cross as f64;
        if la != NO {
            cross -= ((cxa - xab) * (cya - yab)) as f64;
        }
        if lb != NO {
            cross -= ((cxb - xab) * (cyb - yab)) as f64;
        }
        let jc = |l: u8| if l == NO { 0.0 } else { self.joint_card[l as usize] };
        let same = self.joint - jc(la) - jc(lb) + xab as f64 * yab as f64;
        xh * yh - cross + same
    }

    fn clear(&mut self) {
        for i in 0..self.k {
            self.slot[self.cards[i] as usize] = NO;
            self.xm[i] = [0.0; FEW];
            self.ym[i] = [0.0; FEW];
        }
        self.k = 0;
        self.xt = 0.0;
        self.yt = 0.0;
        self.x = [0.0; FEW];
        self.y = [0.0; FEW];
        self.joint = 0.0;
        self.joint_card = [0.0; FEW];
    }
}

/// The hands and ranges a terminal value needs.
pub struct Players<'a> {
    pub n: usize,
    pub hands: &'a [Vec<(u8, u8)>],
    pub masks: &'a [Vec<u64>],
}

#[derive(Default)]
struct Scratch {
    pair: Pair,
    ties: Pair,
    few: Few,
    side: Side,
    win: Vec<f64>,
    compat: Vec<f64>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

#[inline(always)]
fn fill_side(side: &mut Side, hands: &[(u8, u8)], reach: &[f32]) {
    for (&(c1, c2), &w) in hands.iter().zip(reach) {
        if w != 0.0 {
            side.add(c1, c2, w);
        }
    }
}

fn opponents(n: usize, p: usize) -> (usize, usize) {
    match (n, p) {
        (2, _) => (p ^ 1, usize::MAX),
        (_, 0) => (1, 2),
        (_, 1) => (0, 2),
        _ => (0, 1),
    }
}

/// For each of player `p`'s hands, the weight of the opponents' hands that share no card with it
/// (or each other). `reach[i]` is player `i`'s weight for each of their hands.
#[inline(always)]
fn compatible_into(s: &mut Scratch, pl: &Players, p: usize, reach: &[&[f32]], out: &mut [f64]) {
    let (q, z) = opponents(pl.n, p);
    if pl.n == 2 {
        s.side.clear();
        fill_side(&mut s.side, &pl.hands[q], reach[q]);
        for (o, &(c1, c2)) in out.iter_mut().zip(&pl.hands[p]) {
            *o = s.side.avoiding(c1, c2);
        }
        s.side.clear();
        return;
    }
    s.pair.clear();
    for (&(c1, c2), &w) in pl.hands[q].iter().zip(reach[q]) {
        if w != 0.0 {
            s.pair.add_x(c1, c2, w);
        }
    }
    for (&(c1, c2), &w) in pl.hands[z].iter().zip(reach[z]) {
        if w != 0.0 {
            s.pair.add_y(c1, c2, w);
        }
    }
    if s.pair.is_empty() {
        out.iter_mut().for_each(|o| *o = 0.0);
    } else {
        for (o, &(c1, c2)) in out.iter_mut().zip(&pl.hands[p]) {
            *o = s.pair.eval(c1, c2);
        }
    }
    s.pair.clear();
}

/// The weight of the opponents' hands that share no card with each of player `p`'s hands.
pub fn compatible(pl: &Players, p: usize, reach: &[&[f32]], out: &mut [f64]) {
    SCRATCH.with(|s| compatible_into(&mut s.borrow_mut(), pl, p, reach, out));
}

/// At a showdown on a complete board (`entries`, sorted by strength) among the players in
/// `alive` (a bit mask that includes `p`), fills `win` with each of `p`'s hands' share of the pot
/// summed over the opponents' hands (weighted by `reach`) and `compat` with the weight of those
/// hands, as in [`compatible`].
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn showdown_into(s: &mut Scratch, pl: &Players, entries: &[Entry], p: usize, alive: u8, reach: &[&[f32]], win: &mut [f64], compat: &mut [f64]) {
    win.iter_mut().for_each(|w| *w = 0.0);
    let (q, z) = opponents(pl.n, p);
    let rq = reach[q];
    let w_of = |r: &[f32], i: u16| if i == NONE { 0.0 } else { r[i as usize] };

    if pl.n == 2 {
        let side = &mut s.side;
        side.clear();
        let mut i = 0;
        while i < entries.len() {
            let mut j = i + 1;
            while j < entries.len() && entries[j].strength == entries[i].strength {
                j += 1;
            }
            let group = &entries[i..j];
            for e in group {
                let w = w_of(rq, e.idx[q]);
                if w != 0.0 {
                    side.add(e.c1, e.c2, 0.5 * w);
                }
            }
            for e in group {
                let h = e.idx[p];
                if h != NONE {
                    win[h as usize] = side.avoiding(e.c1, e.c2);
                }
            }
            for e in group {
                let w = w_of(rq, e.idx[q]);
                if w != 0.0 {
                    side.add(e.c1, e.c2, 0.5 * w);
                }
            }
            i = j;
        }
        for (o, &(c1, c2)) in compat.iter_mut().zip(&pl.hands[p]) {
            *o = side.avoiding(c1, c2);
        }
        side.clear();
        return;
    }

    let rz = reach[z];
    let both = alive & (1 << q) != 0 && alive & (1 << z) != 0;
    // The other opponent is still in (swept with q) or folded (blocks cards, in full).
    let (sweep_q, sweep_z) = if both { (true, true) } else if alive & (1 << q) != 0 { (true, false) } else { (false, true) };
    let pair = &mut s.pair;
    pair.clear();
    if !sweep_q {
        for (&(c1, c2), &w) in pl.hands[q].iter().zip(rq) {
            if w != 0.0 {
                pair.add_x(c1, c2, w);
            }
        }
    }
    if !sweep_z {
        for (&(c1, c2), &w) in pl.hands[z].iter().zip(rz) {
            if w != 0.0 {
                pair.add_y(c1, c2, w);
            }
        }
    }
    let ties = &mut s.ties;
    let few = &mut s.few;
    let mut i = 0;
    while i < entries.len() {
        let mut j = i + 1;
        while j < entries.len() && entries[j].strength == entries[i].strength {
            j += 1;
        }
        let group = &entries[i..j];
        let (mut tie_q, mut tie_z) = (false, false);
        for e in group {
            if sweep_q {
                let w = w_of(rq, e.idx[q]);
                if w != 0.0 {
                    pair.add_x(e.c1, e.c2, 0.5 * w);
                    tie_q = true;
                }
            }
            if sweep_z {
                let w = w_of(rz, e.idx[z]);
                if w != 0.0 {
                    pair.add_y(e.c1, e.c2, 0.5 * w);
                    tie_z = true;
                }
            }
        }
        // Three-way ties: both opponents hold a hand of this strength.
        let three_way = both && tie_q && tie_z && group.len() > 1;
        let small = three_way && few.fill(group, q, z, rq, rz);
        if three_way && !small {
            for e in group {
                let wq = w_of(rq, e.idx[q]);
                if wq != 0.0 {
                    ties.add_x(e.c1, e.c2, wq);
                }
                let wz = w_of(rz, e.idx[z]);
                if wz != 0.0 {
                    ties.add_y(e.c1, e.c2, wz);
                }
            }
        }
        for e in group {
            let h = e.idx[p];
            if h != NONE {
                let mut w = pair.eval(e.c1, e.c2);
                if small {
                    w += few.eval(e.c1, e.c2) / 12.0;
                } else if three_way {
                    w += ties.eval(e.c1, e.c2) / 12.0;
                }
                win[h as usize] = w;
            }
        }
        if small {
            few.clear();
        } else if three_way {
            ties.clear();
        }
        for e in group {
            if sweep_q {
                let w = w_of(rq, e.idx[q]);
                if w != 0.0 {
                    pair.add_x(e.c1, e.c2, 0.5 * w);
                }
            }
            if sweep_z {
                let w = w_of(rz, e.idx[z]);
                if w != 0.0 {
                    pair.add_y(e.c1, e.c2, 0.5 * w);
                }
            }
        }
        i = j;
    }
    // Every opponent hand is in at full weight now.
    if pair.is_empty() {
        compat.iter_mut().for_each(|o| *o = 0.0);
    } else {
        for (o, &(c1, c2)) in compat.iter_mut().zip(&pl.hands[p]) {
            *o = pair.eval(c1, c2);
        }
    }
    pair.clear();
}

/// See [`showdown_into`].
pub fn showdown(pl: &Players, entries: &[Entry], p: usize, alive: u8, reach: &[&[f32]], win: &mut [f64], compat: &mut [f64]) {
    SCRATCH.with(|s| showdown_into(&mut s.borrow_mut(), pl, entries, p, alive, reach, win, compat));
}

/// A terminal node's payoffs, in chips, for the counterfactual values of player `p`.
pub struct Payoff {
    /// The pot after rake.
    pub net: f64,
    /// What `p` has put in.
    pub invested: f64,
    /// Players still in.
    pub alive: u8,
}

/// Counterfactual values of player `p`'s hands at a terminal node, scaled by `scale`: what each
/// hand ends up with minus what it put in, summed over the opponents' hands weighted by `reach`.
/// `entries` is the showdown order of the board, needed when more than one player is still in.
pub fn terminal(pl: &Players, entries: Option<&[Entry]>, board: u64, p: usize, pay: &Payoff, reach: &[&[f32]], scale: f64, out: &mut [f32]) {
    SCRATCH.with(|s| {
        let s = &mut *s.borrow_mut();
        #[cfg(target_arch = "x86_64")]
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: the CPU has AVX2.
            unsafe { terminal_avx2(s, pl, entries, board, p, pay, reach, scale, out) };
            return;
        }
        terminal_into(s, pl, entries, board, p, pay, reach, scale, out);
    });
}

/// [`terminal_into`] compiled for CPUs with AVX2, which runs the card loops 8 values at a time.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(clippy::too_many_arguments)]
unsafe fn terminal_avx2(s: &mut Scratch, pl: &Players, entries: Option<&[Entry]>, board: u64, p: usize, pay: &Payoff, reach: &[&[f32]], scale: f64, out: &mut [f32]) {
    terminal_into(s, pl, entries, board, p, pay, reach, scale, out)
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn terminal_into(s: &mut Scratch, pl: &Players, entries: Option<&[Entry]>, board: u64, p: usize, pay: &Payoff, reach: &[&[f32]], scale: f64, out: &mut [f32]) {
    let nh = pl.hands[p].len();
    let mut win = std::mem::take(&mut s.win);
    let mut compat = std::mem::take(&mut s.compat);
    win.resize(nh, 0.0);
    compat.resize(nh, 0.0);
    let p_in = pay.alive & (1 << p) != 0;
    if !p_in || pay.alive.count_ones() == 1 {
        let payoff = if p_in { pay.net - pay.invested } else { -pay.invested };
        compatible_into(s, pl, p, reach, &mut compat);
        for ((o, c), &m) in out.iter_mut().zip(&compat).zip(&pl.masks[p]) {
            *o = if m & board != 0 { 0.0 } else { (payoff * c * scale) as f32 };
        }
    } else {
        let entries = entries.expect("a showdown needs the board's hand order");
        showdown_into(s, pl, entries, p, pay.alive, reach, &mut win, &mut compat);
        for (((o, w), c), &m) in out.iter_mut().zip(&win).zip(&compat).zip(&pl.masks[p]) {
            *o = if m & board != 0 { 0.0 } else { ((pay.net * w - pay.invested * c) * scale) as f32 };
        }
    }
    s.win = win;
    s.compat = compat;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::preflop::eval::{card_bit, eval};

    pub struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        pub fn f(&mut self) -> f32 {
            (self.next() % 1000) as f32 / 1000.0
        }
    }

    /// Random hands for each player that miss the board, with some combos shared.
    fn setup(rng: &mut Rng, n: usize, board: u64, per_player: usize) -> (Vec<Vec<(u8, u8)>>, Vec<Vec<u64>>, Vec<Vec<f32>>) {
        let mut hands = Vec::new();
        let mut masks = Vec::new();
        let mut reach = Vec::new();
        for _ in 0..n {
            let mut set = std::collections::BTreeSet::new();
            while set.len() < per_player {
                let a = (rng.next() % 52) as u8;
                let b = (rng.next() % 52) as u8;
                if a == b || board & (1 << a) != 0 || board & (1 << b) != 0 {
                    continue;
                }
                set.insert((a.min(b), a.max(b)));
            }
            let h: Vec<(u8, u8)> = set.into_iter().collect();
            masks.push(h.iter().map(|&(a, b)| (1u64 << a) | (1u64 << b)).collect());
            // Some zero weights, as when a hand never takes an action.
            reach.push(h.iter().map(|_| if rng.next() % 5 == 0 { 0.0 } else { rng.f() }).collect());
            hands.push(h);
        }
        (hands, masks, reach)
    }

    fn entries_for(hands: &[Vec<(u8, u8)>], board_cards: &[u8]) -> Vec<Entry> {
        let mut all: Vec<(u8, u8)> = hands.iter().flatten().copied().collect();
        all.sort_unstable();
        all.dedup();
        let board_mask = board_cards.iter().fold(0, |m, &c| m | card_bit(c));
        let mut out: Vec<Entry> = all
            .iter()
            .map(|&(c1, c2)| {
                let mut idx = [NONE; MAX_PLAYERS];
                for (p, h) in hands.iter().enumerate() {
                    if let Some(i) = h.iter().position(|&x| x == (c1, c2)) {
                        idx[p] = i as u16;
                    }
                }
                Entry { strength: eval(board_mask | card_bit(c1) | card_bit(c2)), c1, c2, idx }
            })
            .collect();
        out.sort_by_key(|e| e.strength);
        out
    }

    /// Brute force over every combination of opponent hands.
    fn brute(hands: &[Vec<(u8, u8)>], masks: &[Vec<u64>], reach: &[Vec<f32>], board_cards: &[u8], p: usize, alive: u8) -> (Vec<f64>, Vec<f64>) {
        let n = hands.len();
        let board_mask = board_cards.iter().fold(0, |m, &c| m | card_bit(c));
        let strength = |h: (u8, u8)| eval(board_mask | card_bit(h.0) | card_bit(h.1));
        let others: Vec<usize> = (0..n).filter(|&i| i != p).collect();
        let mut win = vec![0.0; hands[p].len()];
        let mut compat = vec![0.0; hands[p].len()];
        for (h, &hand) in hands[p].iter().enumerate() {
            let hm = masks[p][h];
            let mut visit = |opp: &[(usize, usize)]| {
                let mut w = 1.0f64;
                let mut used = hm;
                for &(i, k) in opp {
                    if used & masks[i][k] != 0 {
                        return;
                    }
                    used |= masks[i][k];
                    w *= reach[i][k] as f64;
                }
                compat[h] += w;
                // Pot share among the players still in.
                let mut best = strength(hand);
                let mut tied = 1;
                let mut lost = false;
                for &(i, k) in opp {
                    if alive & (1 << i) == 0 {
                        continue;
                    }
                    let s = strength(hands[i][k]);
                    if s > best {
                        lost = true;
                    } else if s == best {
                        tied += 1;
                    }
                    best = best.max(s);
                }
                if !lost {
                    win[h] += w / tied as f64;
                }
            };
            if others.len() == 1 {
                for k in 0..hands[others[0]].len() {
                    visit(&[(others[0], k)]);
                }
            } else {
                for k in 0..hands[others[0]].len() {
                    for l in 0..hands[others[1]].len() {
                        visit(&[(others[0], k), (others[1], l)]);
                    }
                }
            }
        }
        (win, compat)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-4 * (1.0 + a.abs().max(b.abs()))
    }

    #[test]
    fn matches_brute_force() {
        let mut rng = Rng(0x9e3779b97f4a7c15);
        for trial in 0..24 {
            let n = if trial % 4 == 0 { 2 } else { 3 };
            // Paired and low boards make lots of ties.
            let board_cards: Vec<u8> = match trial % 3 {
                0 => vec![0, 1, 2, 3, 4 * 12],      // 2c 2d 2h 2s Ac
                1 => vec![4 * 12 + 3, 4 * 11 + 3, 4 * 10 + 3, 4 * 9 + 3, 4 * 8 + 3], // royal flush board
                _ => {
                    let mut b = Vec::new();
                    while b.len() < 5 {
                        let c = (rng.next() % 52) as u8;
                        if !b.contains(&c) {
                            b.push(c);
                        }
                    }
                    b
                }
            };
            let board = board_cards.iter().fold(0u64, |m, &c| m | (1u64 << c));
            let (hands, masks, reach) = setup(&mut rng, n, board, 40 + trial * 3);
            let entries = entries_for(&hands, &board_cards);
            let pl = Players { n, hands: &hands, masks: &masks };
            let rr: Vec<&[f32]> = reach.iter().map(|r| r.as_slice()).collect();
            for p in 0..n {
                let alives: Vec<u8> = if n == 2 { vec![0b11] } else { vec![0b111, 0b011, 0b101, 0b110] };
                for alive in alives {
                    if alive & (1 << p) == 0 {
                        continue;
                    }
                    let (bw, bc) = brute(&hands, &masks, &reach, &board_cards, p, alive);
                    let mut win = vec![0.0; hands[p].len()];
                    let mut compat = vec![0.0; hands[p].len()];
                    showdown(&pl, &entries, p, alive, &rr, &mut win, &mut compat);
                    for h in 0..hands[p].len() {
                        assert!(close(win[h], bw[h]), "trial {trial} p {p} alive {alive:b} hand {h}: {} vs {}", win[h], bw[h]);
                        assert!(close(compat[h], bc[h]), "trial {trial} compat {} vs {}", compat[h], bc[h]);
                    }
                    let mut c2 = vec![0.0; hands[p].len()];
                    compatible(&pl, p, &rr, &mut c2);
                    for h in 0..hands[p].len() {
                        assert!(close(c2[h], bc[h]));
                    }
                }
            }
        }
    }
}
