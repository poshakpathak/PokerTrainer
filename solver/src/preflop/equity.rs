//! All-in equity between preflop hand classes, heads-up and three-way, with exact card removal
//! between the players involved. Equities are estimated by Monte Carlo over every suit
//! combination of the classes (stratified) and random boards, then cached on disk because the
//! three-way table takes a few seconds to build.

use super::cards::{classes, combo_mask, NUM_CLASSES};
use super::eval::{card_bit, eval};
use rayon::prelude::*;
use std::io::{Read, Write};
use std::path::Path;

const N: usize = NUM_CLASSES;
/// Boards per heads-up class pair.
pub const SAMPLES_2: u32 = 40_000;
/// Boards per three-way class triple.
pub const SAMPLES_3: u32 = 1_200;
const MAGIC: &[u8; 4] = b"PTEQ";
const VERSION: u32 = 1;

pub struct EquityTables {
    /// `eq2[a * 169 + b]`: equity of class `a` against class `b`, ties counting half.
    pub eq2: Vec<f32>,
    /// `n2[a * 169 + b]`: chance that one opponent holds class `b`, given a combo of class `a`
    /// (exact card removal).
    pub n2: Vec<f32>,
    /// `n3[(a * 169 + b) * 169 + c]`: chance that two opponents hold `b` and `c` (in that order),
    /// given a combo of class `a`.
    pub n3: Vec<f32>,
    /// `n3` times the equity of `a` against `b` and `c`.
    pub ne3: Vec<f32>,
}

#[inline]
pub fn idx2(a: usize, b: usize) -> usize {
    a * N + b
}

#[inline]
pub fn idx3(a: usize, b: usize, c: usize) -> usize {
    (a * N + b) * N + c
}

/// A small, fast PRNG (wyrand).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491_4f6c_dd1d)
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0xa076_1d64_78bd_642f);
        let t = (self.0 as u128).wrapping_mul((self.0 ^ 0xe703_7ed1_a0b4_28db) as u128);
        ((t >> 64) as u64) ^ (t as u64)
    }

    /// A number in 0..n.
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * n as u64) >> 32) as u32
    }
}

/// Five random board cards avoiding `used` (a mask of card ids), packed for the evaluator.
#[inline]
fn deal_board(rng: &mut Rng, mut used: u64) -> u64 {
    let mut board = 0u64;
    let mut n = 0;
    while n < 5 {
        let c = rng.below(52) as u8;
        if used & (1u64 << c) == 0 {
            used |= 1u64 << c;
            board |= card_bit(c);
            n += 1;
        }
    }
    board
}

#[inline]
fn packed(c: (u8, u8)) -> u64 {
    card_bit(c.0) | card_bit(c.1)
}

/// Every disjoint pair of combos of classes `a` and `b`.
fn pairs(a: usize, b: usize) -> Vec<((u8, u8), (u8, u8))> {
    let cl = classes();
    let mut out = Vec::new();
    for &x in &cl[a].combos {
        for &y in &cl[b].combos {
            if combo_mask(x) & combo_mask(y) == 0 {
                out.push((x, y));
            }
        }
    }
    out
}

fn triples(a: usize, b: usize, c: usize) -> Vec<[(u8, u8); 3]> {
    let cl = classes();
    let mut out = Vec::new();
    for &x in &cl[a].combos {
        let mx = combo_mask(x);
        for &y in &cl[b].combos {
            let my = combo_mask(y);
            if mx & my != 0 {
                continue;
            }
            for &z in &cl[c].combos {
                if (mx | my) & combo_mask(z) == 0 {
                    out.push([x, y, z]);
                }
            }
        }
    }
    out
}

/// Equity of `a` against `b`, sampling boards evenly over every suit combination.
fn equity2(a: usize, b: usize, samples: u32) -> f32 {
    if a == b {
        return 0.5;
    }
    let list = pairs(a, b);
    let mut rng = Rng::new((a * N + b) as u64 + 1);
    let mut score = 0u64; // in half points
    let len = list.len() as u64;
    for s in 0..samples as u64 {
        let (x, y) = list[(s * len / samples as u64) as usize];
        let board = deal_board(&mut rng, combo_mask(x) | combo_mask(y));
        let ex = eval(board | packed(x));
        let ey = eval(board | packed(y));
        score += if ex > ey { 2 } else if ex == ey { 1 } else { 0 };
    }
    score as f32 / (2.0 * samples as f32)
}

/// Equity of each hand in a three-way all-in between `a`, `b` and `c`, and the number of
/// disjoint combo triples.
fn equity3(a: usize, b: usize, c: usize, samples: u32) -> ([f32; 3], usize) {
    let list = triples(a, b, c);
    if list.is_empty() {
        return ([0.0; 3], 0);
    }
    let mut rng = Rng::new(((a * N + b) * N + c) as u64 + 7);
    let mut won = [0.0f64; 3];
    let len = list.len() as u64;
    for s in 0..samples as u64 {
        let t = list[(s * len / samples as u64) as usize];
        let used = combo_mask(t[0]) | combo_mask(t[1]) | combo_mask(t[2]);
        let board = deal_board(&mut rng, used);
        let e = [eval(board | packed(t[0])), eval(board | packed(t[1])), eval(board | packed(t[2]))];
        let best = e[0].max(e[1]).max(e[2]);
        let winners = e.iter().filter(|&&x| x == best).count() as f64;
        for i in 0..3 {
            if e[i] == best {
                won[i] += 1.0 / winners;
            }
        }
    }
    let mut eq = [0.0f32; 3];
    for i in 0..3 {
        eq[i] = (won[i] / samples as f64) as f32;
    }
    // Hands of the same class are interchangeable, so share their equity evenly.
    if a == b && b == c {
        eq = [1.0 / 3.0; 3];
    } else if a == b {
        let m = (eq[0] + eq[1]) / 2.0;
        eq[0] = m;
        eq[1] = m;
    } else if b == c {
        let m = (eq[1] + eq[2]) / 2.0;
        eq[1] = m;
        eq[2] = m;
    }
    (eq, list.len())
}

fn unordered_triples() -> Vec<(u8, u8, u8)> {
    let mut out = Vec::with_capacity(818_805);
    for a in 0..N {
        for b in a..N {
            for c in b..N {
                out.push((a as u8, b as u8, c as u8));
            }
        }
    }
    out
}

/// The raw Monte Carlo results: heads-up equities and three-way equities per unordered triple.
struct Raw {
    eq2: Vec<f32>,
    eq3: Vec<[f32; 3]>,
}

fn compute_raw(progress: &(dyn Fn(f64) + Sync)) -> Raw {
    let mut eq2 = vec![0.0f32; N * N];
    let pair_list: Vec<(usize, usize)> = (0..N).flat_map(|a| (a..N).map(move |b| (a, b))).collect();
    let vals: Vec<f32> = pair_list.par_iter().map(|&(a, b)| equity2(a, b, SAMPLES_2)).collect();
    for (&(a, b), &v) in pair_list.iter().zip(&vals) {
        eq2[idx2(a, b)] = v;
        eq2[idx2(b, a)] = 1.0 - v;
    }
    progress(0.15);

    let list = unordered_triples();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = list.len();
    let eq3: Vec<[f32; 3]> = list
        .par_chunks(2048)
        .flat_map_iter(|chunk| {
            let out: Vec<[f32; 3]> = chunk
                .iter()
                .map(|&(a, b, c)| equity3(a as usize, b as usize, c as usize, SAMPLES_3).0)
                .collect();
            let d = done.fetch_add(chunk.len(), std::sync::atomic::Ordering::Relaxed) + chunk.len();
            progress(0.15 + 0.85 * d as f64 / total as f64);
            out
        })
        .collect();
    Raw { eq2, eq3 }
}

fn save_raw(path: &Path, raw: &Raw) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut buf = Vec::with_capacity(16 + 4 * (raw.eq2.len() + 3 * raw.eq3.len()));
    buf.extend_from_slice(MAGIC);
    for v in [VERSION, SAMPLES_2, SAMPLES_3] {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    for v in &raw.eq2 {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    for t in &raw.eq3 {
        for v in t {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::File::create(&tmp)?.write_all(&buf)?;
    std::fs::rename(tmp, path)
}

fn load_raw(path: &Path) -> Option<Raw> {
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut buf).ok()?;
    let n3 = 818_805;
    if buf.len() != 16 + 4 * (N * N + 3 * n3) || &buf[0..4] != MAGIC {
        return None;
    }
    let word = |i: usize| u32::from_le_bytes(buf[i..i + 4].try_into().unwrap());
    if word(4) != VERSION || word(8) != SAMPLES_2 || word(12) != SAMPLES_3 {
        return None;
    }
    let float = |i: usize| f32::from_le_bytes(buf[i..i + 4].try_into().unwrap());
    let mut off = 16;
    let mut eq2 = Vec::with_capacity(N * N);
    for _ in 0..N * N {
        eq2.push(float(off));
        off += 4;
    }
    let mut eq3 = Vec::with_capacity(n3);
    for _ in 0..n3 {
        eq3.push([float(off), float(off + 4), float(off + 8)]);
        off += 12;
    }
    Some(Raw { eq2, eq3 })
}

impl EquityTables {
    /// Loads the tables from `cache` (a file path), or computes and saves them. `progress`
    /// gets the share done while computing.
    pub fn load_or_compute(cache: Option<&Path>, progress: &(dyn Fn(f64) + Sync)) -> EquityTables {
        let raw = cache.and_then(load_raw).unwrap_or_else(|| {
            let raw = compute_raw(progress);
            if let Some(p) = cache {
                if let Err(e) = save_raw(p, &raw) {
                    eprintln!("couldn't cache the equity tables at {}: {e}", p.display());
                }
            }
            raw
        });
        Self::from_raw(raw)
    }

    fn from_raw(raw: Raw) -> EquityTables {
        let cl = classes();
        // Card removal counts per class pair, for one combo of the first class.
        let mut n2 = vec![0.0f32; N * N];
        for a in 0..N {
            let x = cl[a].combos[0];
            for b in 0..N {
                let n = cl[b].combos.iter().filter(|&&y| combo_mask(x) & combo_mask(y) == 0).count();
                n2[idx2(a, b)] = n as f32 / 1225.0;
            }
        }
        let mut n3 = vec![0.0f32; N * N * N];
        let mut ne3 = vec![0.0f32; N * N * N];
        // Count disjoint (b, c) combo pairs for one combo of `a`, in parallel over `a`.
        n3.par_chunks_mut(N * N).enumerate().for_each(|(a, slab)| {
            let x = combo_mask(cl[a].combos[0]);
            for b in 0..N {
                let ys: Vec<u64> = cl[b].combos.iter().map(|&y| combo_mask(y)).filter(|&y| y & x == 0).collect();
                for c in 0..N {
                    let mut n = 0;
                    for &y in &ys {
                        for &z in &cl[c].combos {
                            if (x | y) & combo_mask(z) == 0 {
                                n += 1;
                            }
                        }
                    }
                    slab[b * N + c] = n as f32 / (1225.0 * 1128.0);
                }
            }
        });
        let list = unordered_triples();
        for (&(a, b, c), eq) in list.iter().zip(&raw.eq3) {
            let (a, b, c) = (a as usize, b as usize, c as usize);
            let perms = [
                (a, b, c, eq[0]),
                (a, c, b, eq[0]),
                (b, a, c, eq[1]),
                (b, c, a, eq[1]),
                (c, a, b, eq[2]),
                (c, b, a, eq[2]),
            ];
            for (x, y, z, e) in perms {
                let i = idx3(x, y, z);
                ne3[i] = n3[i] * e;
            }
        }
        EquityTables { eq2: raw.eq2, n2, n3, ne3 }
    }

    /// Equity of class `a` against a weighted range (weights per class, before card removal).
    pub fn equity_vs_range(&self, a: usize, range: &[f32]) -> f32 {
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for b in 0..N {
            let w = (range[b] * self.n2[idx2(a, b)]) as f64;
            num += w * self.eq2[idx2(a, b)] as f64;
            den += w;
        }
        if den > 0.0 { (num / den) as f32 } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preflop::cards::class_index;

    /// Exact class-against-class equity, enumerating every suit combination and board.
    fn exact(a: usize, b: usize) -> f32 {
        let mut score = 0u64;
        let mut count = 0u64;
        for (x, y) in pairs(a, b) {
            let used = combo_mask(x) | combo_mask(y);
            let deck: Vec<u8> = (0..52u8).filter(|&c| used & (1u64 << c) == 0).collect();
            let n = deck.len();
            for i in 0..n {
                for j in i + 1..n {
                    for k in j + 1..n {
                        for l in k + 1..n {
                            for m in l + 1..n {
                                let board = card_bit(deck[i]) | card_bit(deck[j]) | card_bit(deck[k]) | card_bit(deck[l]) | card_bit(deck[m]);
                                let (ex, ey) = (eval(board | packed(x)), eval(board | packed(y)));
                                score += if ex > ey { 2 } else if ex == ey { 1 } else { 0 };
                                count += 2;
                            }
                        }
                    }
                }
            }
        }
        score as f32 / count as f32
    }

    #[test]
    fn heads_up_equities() {
        let idx = |s: &str| class_index(s).unwrap();
        // Well-known values: AA beats KK about 82% of the time, 22 is a small favourite over AKo.
        let aa_kk = exact(idx("AA"), idx("KK"));
        assert!((aa_kk - 0.8195).abs() < 0.003, "AA vs KK {aa_kk}");
        let deuces = exact(idx("22"), idx("AKo"));
        assert!(deuces > 0.51 && deuces < 0.54, "22 vs AKo {deuces}");
        // The Monte Carlo estimate matches exact enumeration.
        for (a, b) in [("AA", "KK"), ("AKs", "QJs"), ("T9s", "AKo")] {
            let (want, got) = (exact(idx(a), idx(b)), equity2(idx(a), idx(b), SAMPLES_2));
            assert!((got - want).abs() < 0.01, "{a} vs {b}: {got} (exact {want})");
        }
    }

    #[test]
    fn three_way_sums_to_one() {
        let (a, b, c) = (class_index("AA").unwrap(), class_index("KK").unwrap(), class_index("QQ").unwrap());
        let (e, n) = equity3(a, b, c, 50_000);
        assert!(n > 0);
        assert!((e.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        assert!(e[0] > 0.62 && e[0] < 0.70, "{e:?}");
        // Three aces aren't available to three players.
        let (_, none) = equity3(a, a, a, 10);
        assert_eq!(none, 0);
    }
}

#[cfg(test)]
mod timing {
    use super::*;

    #[test]
    #[ignore]
    fn build_tables() {
        let t = std::time::Instant::now();
        let raw = compute_raw(&|_| {});
        println!("raw tables in {:.1}s", t.elapsed().as_secs_f64());
        let t = std::time::Instant::now();
        let tables = EquityTables::from_raw(raw);
        println!("expanded in {:.1}s", t.elapsed().as_secs_f64());
        let s: f64 = tables.n3[..169 * 169].iter().map(|&x| x as f64).sum();
        println!("sum n3 for AA = {s:.6}");
        let aa = crate::preflop::cards::class_index("AA").unwrap();
        let kk = crate::preflop::cards::class_index("KK").unwrap();
        let qq = crate::preflop::cards::class_index("QQ").unwrap();
        let i = idx3(aa, kk, qq);
        println!("AA vs KK vs QQ: {:.4}", tables.ne3[i] / tables.n3[i]);
        println!("AA vs KK: {:.4}", tables.eq2[idx2(aa, kk)]);
    }
}
