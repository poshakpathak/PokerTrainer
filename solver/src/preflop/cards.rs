//! The 169 preflop hand classes, laid out like the 13x13 grid in the web app: row and column
//! follow "AKQJT98765432", pairs on the diagonal, suited hands above it and offsuit hands below.
//!
//! Cards use postflop-solver's ids: `4 * rank + suit`, with rank 0 = deuce .. 12 = ace and suits
//! clubs, diamonds, hearts, spades = 0 to 3.

use std::sync::OnceLock;

pub const NUM_CLASSES: usize = 169;
pub const GRID_RANKS: &[u8; 13] = b"AKQJT98765432";
const SUIT_CHARS: &[u8; 4] = b"cdhs";

/// Rank (0 = deuce .. 12 = ace) of grid row or column `i`.
#[inline]
pub fn grid_rank(i: usize) -> u8 {
    12 - i as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Pair,
    Suited,
    Offsuit,
}

#[derive(Clone, Debug)]
pub struct HandClass {
    pub name: String,
    pub shape: Shape,
    /// Higher and lower rank (equal for pairs), 0 = deuce .. 12 = ace.
    pub hi: u8,
    pub lo: u8,
    /// Every combo as a pair of card ids.
    pub combos: Vec<(u8, u8)>,
}

impl HandClass {
    pub fn num_combos(&self) -> usize {
        self.combos.len()
    }
}

fn rank_char(rank: u8) -> char {
    GRID_RANKS[12 - rank as usize] as char
}

pub fn card_name(card: u8) -> String {
    format!("{}{}", rank_char(card >> 2), SUIT_CHARS[(card & 3) as usize] as char)
}

fn build_classes() -> Vec<HandClass> {
    let mut out = Vec::with_capacity(NUM_CLASSES);
    for row in 0..13 {
        for col in 0..13 {
            let (shape, hi, lo) = if row == col {
                (Shape::Pair, grid_rank(row), grid_rank(row))
            } else if row < col {
                (Shape::Suited, grid_rank(row), grid_rank(col))
            } else {
                (Shape::Offsuit, grid_rank(col), grid_rank(row))
            };
            let mut combos = Vec::new();
            for s1 in 0..4u8 {
                for s2 in 0..4u8 {
                    let ok = match shape {
                        Shape::Pair => s1 < s2,
                        Shape::Suited => s1 == s2,
                        Shape::Offsuit => s1 != s2,
                    };
                    if ok {
                        combos.push((4 * hi + s1, 4 * lo + s2));
                    }
                }
            }
            let mut name = format!("{}{}", rank_char(hi), rank_char(lo));
            match shape {
                Shape::Suited => name.push('s'),
                Shape::Offsuit => name.push('o'),
                Shape::Pair => {}
            }
            out.push(HandClass { name, shape, hi, lo, combos });
        }
    }
    out
}

/// All 169 classes in grid order.
pub fn classes() -> &'static [HandClass] {
    static CLASSES: OnceLock<Vec<HandClass>> = OnceLock::new();
    CLASSES.get_or_init(build_classes)
}

/// Number of combos of each class (6, 4 or 12).
pub fn combo_counts() -> &'static [f32; NUM_CLASSES] {
    static COUNTS: OnceLock<[f32; NUM_CLASSES]> = OnceLock::new();
    COUNTS.get_or_init(|| {
        let mut w = [0.0; NUM_CLASSES];
        for (i, c) in classes().iter().enumerate() {
            w[i] = c.num_combos() as f32;
        }
        w
    })
}

/// The class of a combo given as two card ids.
pub fn class_of(c1: u8, c2: u8) -> usize {
    let (r1, r2) = (c1 >> 2, c2 >> 2);
    let (hi, lo) = if r1 >= r2 { (r1, r2) } else { (r2, r1) };
    let (row_hi, row_lo) = (12 - hi as usize, 12 - lo as usize);
    if hi == lo {
        row_hi * 13 + row_hi
    } else if c1 & 3 == c2 & 3 {
        row_hi * 13 + row_lo
    } else {
        row_lo * 13 + row_hi
    }
}

pub fn class_index(name: &str) -> Option<usize> {
    classes().iter().position(|c| c.name == name)
}

/// Bit mask of a combo's two cards.
#[inline]
pub fn combo_mask(c: (u8, u8)) -> u64 {
    (1u64 << c.0) | (1u64 << c.1)
}

/// A range as a postflop-solver range string, e.g. "AA,AKs:0.75,KQo:0.125". Classes below
/// `min_weight` are left out.
pub fn range_string(weights: &[f32], min_weight: f32) -> String {
    let mut parts = Vec::new();
    for (i, c) in classes().iter().enumerate() {
        let w = weights[i].clamp(0.0, 1.0);
        if w < min_weight {
            continue;
        }
        if w >= 0.9995 {
            parts.push(c.name.clone());
        } else {
            let mut s = format!("{w:.3}");
            while s.ends_with('0') {
                s.pop();
            }
            parts.push(format!("{}:{}", c.name, s));
        }
    }
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_layout() {
        let cl = classes();
        assert_eq!(cl.len(), 169);
        assert_eq!(cl[0].name, "AA");
        assert_eq!(cl[1].name, "AKs");
        assert_eq!(cl[13].name, "AKo");
        assert_eq!(cl[168].name, "22");
        assert_eq!(cl[12].name, "A2s");
        let total: usize = cl.iter().map(|c| c.num_combos()).sum();
        assert_eq!(total, 1326);
        for (i, c) in cl.iter().enumerate() {
            for &(a, b) in &c.combos {
                assert_eq!(class_of(a, b), i, "{}", c.name);
                assert_eq!(class_of(b, a), i, "{}", c.name);
            }
        }
        assert_eq!(card_name(51), "As");
        assert_eq!(card_name(0), "2c");
    }

    #[test]
    fn range_strings() {
        let mut w = [0.0f32; NUM_CLASSES];
        w[0] = 1.0;
        w[1] = 0.75;
        w[14] = 0.1234;
        w[168] = 0.0001;
        assert_eq!(range_string(&w, 0.001), "AA,AKs:0.75,KK:0.123");
    }
}
