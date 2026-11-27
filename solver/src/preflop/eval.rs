//! A fast 7-card hand evaluator for the preflop equity tables. A higher score is a better hand.
//!
//! Cards are packed into a u64 with 16 bits per suit (bit `16 * suit + rank`), so each suit's
//! ranks can be read as a 13-bit mask.

/// The evaluator's bit for a postflop-solver card id (`4 * rank + suit`).
#[inline(always)]
pub fn card_bit(card: u8) -> u64 {
    1u64 << (((card & 3) as u32) * 16 + (card >> 2) as u32)
}

#[inline(always)]
fn top_bit(x: u32) -> u32 {
    1 << (31 - x.leading_zeros())
}

/// Keeps the `n` highest set bits.
#[inline(always)]
fn keep_top(mut x: u32, n: u32) -> u32 {
    while x.count_ones() > n {
        x &= x - 1;
    }
    x
}

/// The bit of the top card of the best straight in a rank mask, or 0.
#[inline(always)]
fn straight(ranks: u32) -> u32 {
    // Shift up one so the ace can also play low as bit 0.
    let m = (ranks << 1) | ((ranks >> 12) & 1);
    let s = m & (m >> 1) & (m >> 2) & (m >> 3) & (m >> 4);
    if s == 0 {
        return 0;
    }
    // Bit i of `s` is a straight from m-bit i to i + 4, i.e. up to rank i + 3.
    1 << (31 - s.leading_zeros() + 3)
}

/// Scores 5 to 7 cards packed with `card_bit`.
#[inline]
pub fn eval(mask: u64) -> u32 {
    let c = (mask & 0x1fff) as u32;
    let d = ((mask >> 16) & 0x1fff) as u32;
    let h = ((mask >> 32) & 0x1fff) as u32;
    let s = ((mask >> 48) & 0x1fff) as u32;
    let ranks = c | d | h | s;

    // With 7 cards a flush rules out quads and full houses.
    for suit in [c, d, h, s] {
        if suit.count_ones() >= 5 {
            let sf = straight(suit);
            if sf != 0 {
                return (8 << 26) | sf;
            }
            return (5 << 26) | keep_top(suit, 5);
        }
    }
    let m4 = c & d & h & s;
    if m4 != 0 {
        return (7 << 26) | (m4 << 13) | top_bit(ranks & !m4);
    }
    let m3 = (c & d & h) | (c & d & s) | (c & h & s) | (d & h & s);
    let m2 = (c & d) | (c & h) | (c & s) | (d & h) | (d & s) | (h & s);
    if m3 != 0 {
        let t = top_bit(m3);
        let pair = m2 & !t;
        if pair != 0 {
            return (6 << 26) | (t << 13) | top_bit(pair);
        }
    }
    let st = straight(ranks);
    if st != 0 {
        return (4 << 26) | st;
    }
    if m3 != 0 {
        return (3 << 26) | (m3 << 13) | keep_top(ranks & !m3, 2);
    }
    if m2 != 0 {
        if m2.count_ones() >= 2 {
            let pairs = keep_top(m2, 2);
            return (2 << 26) | (pairs << 13) | top_bit(ranks & !pairs);
        }
        return (1 << 26) | (m2 << 13) | keep_top(ranks & !m2, 3);
    }
    keep_top(ranks, 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> u64 {
        let b = s.as_bytes();
        let mut mask = 0;
        for i in (0..b.len()).step_by(2) {
            let rank = b"23456789TJQKA".iter().position(|&r| r == b[i]).unwrap() as u8;
            let suit = b"cdhs".iter().position(|&x| x == b[i + 1]).unwrap() as u8;
            mask |= card_bit(4 * rank + suit);
        }
        mask
    }

    fn cat(s: &str) -> u32 {
        eval(parse(s)) >> 26
    }

    #[test]
    fn categories() {
        assert_eq!(cat("AsKsQsJsTs2d3c"), 8);
        assert_eq!(cat("As2s3s4s5s9d9c"), 8);
        assert_eq!(cat("AsAhAdAc2d3c4h"), 7);
        assert_eq!(cat("AsAhAdKcKd3c4h"), 6);
        assert_eq!(cat("AsAhAdKcKdKh4h"), 6);
        assert_eq!(cat("As9s3s4s5s9d9c"), 5);
        assert_eq!(cat("Ad2s3h4c5s9d9c"), 4);
        assert_eq!(cat("Td9s8h7c6sAdAc"), 4);
        assert_eq!(cat("AsAhAd2c7d3c4h"), 3);
        assert_eq!(cat("AsAhKdKc7d7c4h"), 2);
        assert_eq!(cat("AsAh2d3c7d9cTh"), 1);
        assert_eq!(cat("As8h2d3c7d9cTh"), 0);
    }

    #[test]
    fn ordering() {
        let better = |a: &str, b: &str| eval(parse(a)) > eval(parse(b));
        let same = |a: &str, b: &str| eval(parse(a)) == eval(parse(b));
        // Wheel loses to a six-high straight.
        assert!(better("2s3h4d5c6s9dJc", "As2h3d4c5s9dJc"));
        assert!(better("AsAhAdKcKd3c4h", "KsKhKdAcAd3c4h"));
        assert!(better("AsAhKdKc7d7c4h", "AsAhKdKc7d7c3h") == false);
        assert!(same("AsAhKdKc7d7c4h", "AsAhKdKc7d7c3h"));
        assert!(better("AsAhKdKc7d7c5h", "AsAhKdKc6d6c5h"));
        assert!(better("AsAh2d3c7d9cKh", "AsAh2d3c7d9cQh"));
        assert!(better("KsKhQd3c7d9cJh", "QsQhKd3c7d9cJh"));
        assert!(same("As8h2d3c7d9cTh", "Ad8c2s3h7c9hTd"));
        assert!(better("TsJsQsKsAs2c3c", "9sTsJsQsKs2c3c"));
        // A straight beats trips; a flush beats a straight.
        assert!(better("Ad2s3h4c5s9d9c", "9s9h9d2c3d5cKh"));
        assert!(better("As9s3s4s7s9d9c", "Td9s8h7c6sAdAc"));
    }

    /// Checks against a simple reference that tries every 5-card subset.
    #[test]
    fn matches_reference() {
        fn score5(cards: &[(u8, u8)]) -> (u8, Vec<u8>) {
            let mut ranks: Vec<u8> = cards.iter().map(|c| c.0).collect();
            ranks.sort_unstable_by(|a, b| b.cmp(a));
            let flush = cards.iter().all(|c| c.1 == cards[0].1);
            let mut uniq = ranks.clone();
            uniq.dedup();
            let straight_top = if uniq.len() == 5 && uniq[0] - uniq[4] == 4 {
                Some(uniq[0])
            } else if uniq == vec![12, 3, 2, 1, 0] {
                Some(3)
            } else {
                None
            };
            let mut counts: Vec<(u8, u8)> = uniq.iter().map(|&r| (ranks.iter().filter(|&&x| x == r).count() as u8, r)).collect();
            counts.sort_unstable_by(|a, b| b.cmp(a));
            let order: Vec<u8> = counts.iter().map(|c| c.1).collect();
            let pattern: Vec<u8> = counts.iter().map(|c| c.0).collect();
            match (straight_top, flush, pattern.as_slice()) {
                (Some(t), true, _) => (8, vec![t]),
                (_, _, [4, 1]) => (7, order),
                (_, _, [3, 2]) => (6, order),
                (_, true, _) => (5, ranks),
                (Some(t), false, _) => (4, vec![t]),
                (_, _, [3, 1, 1]) => (3, order),
                (_, _, [2, 2, 1]) => (2, order),
                (_, _, [2, 1, 1, 1]) => (1, order),
                _ => (0, ranks),
            }
        }
        fn best7(cards: &[(u8, u8)]) -> (u8, Vec<u8>) {
            let mut best = (0, vec![]);
            for skip1 in 0..7 {
                for skip2 in skip1 + 1..7 {
                    let five: Vec<(u8, u8)> = (0..7).filter(|&i| i != skip1 && i != skip2).map(|i| cards[i]).collect();
                    let s = score5(&five);
                    if s > best {
                        best = s;
                    }
                }
            }
            best
        }
        let mut seed = 12345u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut hands = Vec::new();
        for _ in 0..3000 {
            let mut used = 0u64;
            let mut cards = Vec::new();
            while cards.len() < 7 {
                let c = (next() % 52) as u8;
                if used & (1 << c) == 0 {
                    used |= 1 << c;
                    cards.push(c);
                }
            }
            let mask = cards.iter().fold(0, |m, &c| m | card_bit(c));
            let rs: Vec<(u8, u8)> = cards.iter().map(|&c| (c >> 2, c & 3)).collect();
            hands.push((eval(mask), best7(&rs)));
        }
        for i in 0..hands.len() {
            for j in (i + 1)..(i + 40).min(hands.len()) {
                let (a, ra) = (&hands[i].0, &hands[i].1);
                let (b, rb) = (&hands[j].0, &hands[j].1);
                assert_eq!(a.cmp(b), ra.cmp(rb), "{ra:?} vs {rb:?}");
            }
        }
    }
}
