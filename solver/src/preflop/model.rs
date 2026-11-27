//! What a pot that reaches the flop is worth to each hand: the postflop model.
//!
//! A preflop solve can't play the flop, turn and river, so each hand's share of the pot comes
//! from a model. Raw all-in equity gets the important things wrong: position is worth a lot,
//! a hand that dominates another wins more than its equity (it gets paid off, the weaker hand
//! folds or pays), close matchups are closer to even than their equity, and pairs and suited
//! hands play better than offsuit junk. The calibrated model gives the out-of-position player
//! `o`, holding class `a` against class `b` in position, this share of the pot:
//!
//! ```text
//! share_o(a, b) = 1/2 + alpha * x + cubic * x^3 - position + initiative + play(a) - play(b)
//! ```
//!
//! where `x = equity(a, b) - 1/2`, and the in-position player the rest, so no money is created.
//! `position` is what being last to act is worth, `initiative` is ± for the preflop raiser, and
//! `play` scores a hand's playability from four features (suited, pair, straight potential,
//! kicker height). Everything goes back to raw equity as the stack-to-pot ratio goes to zero,
//! where the hand is simply all-in.
//!
//! The numbers are fit to postflop solves of six spots (BTN and CO opens against the BB, CO
//! against a BTN call, SB against the BB, and 3-bet pots between BB and BTN and BTN and CO, on
//! 9 to 18 flops each): every hand class's average share of the pot against what the model
//! predicts from its preflop equity against each hand in the other range. The fit explains
//! about 86% of the variance between hand classes in single-raised pots and 88% in 3-bet pots;
//! `scripts/fit_realization.py` reproduces it.
//!
//! Three-way pots use three-way all-in equity, plus two thirds of the heads-up position edge
//! (from the first player to act to the last) and of the playability difference.

use super::cards::{classes, Shape, NUM_CLASSES};

/// Model parameters at one stack-to-pot ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub alpha: f64,
    /// Weight of the cubed equity edge, `(equity - 1/2)^3`: lopsided matchups are won by more
    /// than close ones.
    pub cubic: f64,
    pub position: f64,
    pub initiative: f64,
    /// Weights of the playability features: suited, pair, straight potential, kicker height.
    pub play: [f64; 4],
}

const ALL_IN: Params = Params { alpha: 1.0, cubic: 0.0, position: 0.0, initiative: 0.0, play: [0.0; 4] };

/// Fit on 3-bet pots (SPR about 4.8).
const THREE_BET: (f64, Params) = (
    4.78,
    Params { alpha: 0.113, cubic: 23.014, position: 0.0539, initiative: 0.0474, play: [0.006, 0.158, 0.127, -0.032] },
);

/// Fit on single-raised pots (SPR about 16.7).
const SINGLE_RAISED: (f64, Params) = (
    16.66,
    Params { alpha: 0.196, cubic: 18.134, position: 0.0516, initiative: 0.0044, play: [0.060, 0.172, 0.027, 0.040] },
);

fn lerp(a: &Params, b: &Params, t: f64) -> Params {
    let m = |x: f64, y: f64| x + (y - x) * t;
    let mut play = [0.0; 4];
    for i in 0..4 {
        play[i] = m(a.play[i], b.play[i]);
    }
    Params { alpha: m(a.alpha, b.alpha), cubic: m(a.cubic, b.cubic), position: m(a.position, b.position), initiative: m(a.initiative, b.initiative), play }
}

/// The calibrated parameters at a stack-to-pot ratio: linear from all-in to the 3-bet pot fit,
/// then linear in log(SPR) up to the single-raised pot fit, and constant beyond it.
pub fn params(spr: f64) -> Params {
    let (s3, p3) = THREE_BET;
    let (s1, p1) = SINGLE_RAISED;
    if spr <= 0.0 {
        ALL_IN
    } else if spr < s3 {
        lerp(&ALL_IN, &p3, spr / s3)
    } else if spr < s1 {
        lerp(&p3, &p1, (spr.ln() - s3.ln()) / (s1.ln() - s3.ln()))
    } else {
        p1
    }
}

/// The four playability features of a class: suited, pair, straight potential (share of the
/// four most a hand can be part of), kicker height (0 for a deuce, 1 for an ace).
pub fn features(class: usize) -> [f64; 4] {
    let c = &classes()[class];
    let pair = c.shape == Shape::Pair;
    let suited = c.shape == Shape::Suited;
    let mut straights = 0;
    if !pair {
        // Ranks 1 (ace low) to 14 (ace high); count the 5-card straights containing both cards.
        let (r1, r2) = (c.hi as i32 + 2, c.lo as i32 + 2);
        let has = |lo: i32, r: i32| (lo..lo + 5).contains(&r) || (r == 14 && lo == 1);
        for lo in 1..=10 {
            if has(lo, r1) && has(lo, r2) {
                straights += 1;
            }
        }
    }
    [suited as i32 as f64, pair as i32 as f64, straights as f64 / 4.0, c.lo as f64 / 12.0]
}

/// `play(a)` for every class at a stack-to-pot ratio.
pub fn playability(p: &Params) -> [f32; NUM_CLASSES] {
    let mut out = [0.0; NUM_CLASSES];
    for (a, v) in out.iter_mut().enumerate() {
        let f = features(a);
        *v = (0..4).map(|i| p.play[i] * f[i]).sum::<f64>() as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preflop::cards::class_index;

    #[test]
    fn interpolation() {
        assert_eq!(params(0.0), ALL_IN);
        let (s3, p3) = THREE_BET;
        let (s1, p1) = SINGLE_RAISED;
        assert_eq!(params(s1), p1);
        assert_eq!(params(40.0), p1);
        assert!((params(s3).alpha - p3.alpha).abs() < 1e-9);
        let mid = params(s3 / 2.0);
        assert!((mid.alpha - (1.0 + p3.alpha) / 2.0).abs() < 1e-9);
        assert!((mid.cubic - p3.cubic / 2.0).abs() < 1e-9);
        // Halfway between the two fits in log(SPR).
        let geo = params((s1 * s3).sqrt());
        assert!((geo.position - (p1.position + p3.position) / 2.0).abs() < 1e-9);
    }

    #[test]
    fn feature_values() {
        let f = |s: &str| features(class_index(s).unwrap());
        assert_eq!(f("76s"), [1.0, 0.0, 1.0, 4.0 / 12.0]);
        assert_eq!(f("AKo"), [0.0, 0.0, 0.25, 11.0 / 12.0]);
        assert_eq!(f("A5s"), [1.0, 0.0, 0.25, 3.0 / 12.0]);
        assert_eq!(f("QQ"), [0.0, 1.0, 0.0, 10.0 / 12.0]);
        assert_eq!(f("72o"), [0.0, 0.0, 0.0, 0.0]);
    }
}
