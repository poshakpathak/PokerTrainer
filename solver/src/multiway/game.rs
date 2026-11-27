//! A spot on a board, ready to solve: every player's hands, the hand order at showdown on every
//! board the hand can reach, and the game tree.

use super::eval::{self, Entry, Players, NONE};
use super::spot::{Config, MultiSpot, MAX_PLAYERS};
use super::tree::{Builder, Counts, Kind, Node};
use crate::preflop::eval::{card_bit, eval as strength};
use postflop_solver::{card_from_str, Range};
use rayon::prelude::*;

/// The board as the hand goes: the cards dealt so far, and the ones dealt after the start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    pub mask: u64,
    pub count: u8,
    pub extra: [u8; 2],
    pub extras: u8,
}

impl Board {
    pub fn with(self, card: u8) -> Board {
        let mut b = self;
        b.mask |= 1u64 << card;
        b.count += 1;
        b.extra[b.extras as usize] = card;
        b.extras += 1;
        b
    }

    /// Which showdown table belongs to this board: see [`runout_key`].
    pub fn key(&self) -> usize {
        runout_key(&self.extra[..self.extras as usize])
    }
}

/// Index of the showdown table for the cards dealt after the start.
pub fn runout_key(extra: &[u8]) -> usize {
    match extra {
        [] => 0,
        [a] => 1 + *a as usize,
        [a, b] => 53 + (*a.min(b) as usize) * 52 + *a.max(b) as usize,
        _ => unreachable!(),
    }
}

const NUM_KEYS: usize = 53 + 52 * 52;

pub struct Game {
    pub spot: MultiSpot,
    pub cfg: Config,
    pub n: usize,
    /// The board the solve starts on, e.g. "Ks7d2c".
    pub board_str: String,
    pub board: Vec<u8>,
    pub hands: Vec<Vec<(u8, u8)>>,
    pub masks: Vec<Vec<u64>>,
    /// Each hand's weight in the range.
    pub weights: Vec<Vec<f32>>,
    /// For each player and card, the hands that hold it.
    pub with_card: Vec<Vec<Vec<u16>>>,
    /// Hands at showdown, by [`runout_key`], sorted by strength.
    pub showdowns: Vec<Option<Box<[Entry]>>>,
    pub root: Node,
    /// The total weight of the deals the ranges allow (no shared cards).
    pub mass: f64,
    pub counts: Counts,
    pub iteration: u32,
    pub finalized: bool,
}

/// Parses "Ks7d2c" or "Ks7d2cQh9s" into card ids.
pub fn parse_board(board: &str) -> Result<Vec<u8>, String> {
    let b = board.trim();
    if b.len() < 6 || b.len() > 10 || b.len() % 2 != 0 {
        return Err(format!("The board must be 3 to 5 cards, like Ks7d2c, not {board:?}."));
    }
    let mut cards = Vec::new();
    for i in (0..b.len()).step_by(2) {
        let c = card_from_str(&b[i..i + 2])?;
        if cards.contains(&c) {
            return Err("The board has the same card twice.".into());
        }
        cards.push(c);
    }
    Ok(cards)
}

/// Everything about a spot and board except the tree's memory, which [`Game::allocate`] adds.
pub struct Setup {
    pub spot: MultiSpot,
    pub cfg: Config,
    pub board_str: String,
    pub board: Vec<u8>,
    pub hands: Vec<Vec<(u8, u8)>>,
    pub masks: Vec<Vec<u64>>,
    pub weights: Vec<Vec<f32>>,
    pub counts: Counts,
    /// The total weight of the deals the ranges allow (no shared cards).
    pub mass: f64,
}

impl Setup {
    pub fn new(spot: &MultiSpot, board_str: &str) -> Result<Setup, String> {
        let cfg = spot.config()?;
        let board = parse_board(board_str)?;
        let dead = board.iter().fold(0u64, |m, &c| m | (1u64 << c));
        let mut hands = Vec::new();
        let mut weights = Vec::new();
        for (p, r) in spot.ranges.iter().enumerate() {
            let range: Range = r.parse().map_err(|e| format!("{}'s range: {e}", spot.players[p]))?;
            let (h, w) = range.get_hands_weights(dead);
            if h.is_empty() {
                return Err(format!("{}'s range has no hands on this board.", spot.players[p]));
            }
            hands.push(h);
            weights.push(w);
        }
        let counts = Builder::new(&cfg).count(board.len());
        let masks: Vec<Vec<u64>> = hands.iter().map(|h: &Vec<(u8, u8)>| h.iter().map(|&(a, b)| (1u64 << a) | (1u64 << b)).collect()).collect();
        let pl = Players { n: cfg.players, hands: &hands, masks: &masks };
        let reach: Vec<&[f32]> = weights.iter().map(|w: &Vec<f32>| w.as_slice()).collect();
        let mut compat = vec![0.0; hands[0].len()];
        eval::compatible(&pl, 0, &reach, &mut compat);
        let mass: f64 = compat.iter().zip(&weights[0]).map(|(c, &w)| c * w as f64).sum();
        if mass <= 0.0 {
            return Err("No deal is possible: the ranges always share a card.".into());
        }
        Ok(Setup { spot: spot.clone(), cfg, board_str: board_str.trim().to_string(), board, hands, masks, weights, counts, mass })
    }

    pub fn hand_counts(&self) -> Vec<usize> {
        self.hands.iter().map(Vec::len).collect()
    }

    /// Showdown tables: one per runout from the starting board.
    fn runouts(&self) -> usize {
        match self.board.len() {
            3 => 49 * 48 / 2,
            4 => 48,
            _ => 1,
        }
    }

    /// Memory the solve needs, in bytes.
    pub fn memory_bytes(&self) -> u64 {
        let mut union = 0u64;
        let mut seen = vec![false; 52 * 52];
        for h in self.hands.iter().flatten() {
            let k = h.0 as usize * 52 + h.1 as usize;
            if !seen[k] {
                seen[k] = true;
                union += 1;
            }
        }
        self.counts.bytes(&self.hand_counts()) + self.runouts() as u64 * union * std::mem::size_of::<Entry>() as u64
    }

    /// Builds the tree and its memory, and the showdown tables.
    pub fn allocate(self) -> Game {
        let n = self.cfg.players;
        let mut index = vec![vec![NONE; 52 * 52]; MAX_PLAYERS];
        for (p, h) in self.hands.iter().enumerate() {
            for (i, &(a, b)) in h.iter().enumerate() {
                index[p][a as usize * 52 + b as usize] = i as u16;
                index[p][b as usize * 52 + a as usize] = i as u16;
            }
        }
        let with_card: Vec<Vec<Vec<u16>>> = self
            .hands
            .iter()
            .map(|h| {
                let mut by = vec![Vec::new(); 52];
                for (i, &(a, b)) in h.iter().enumerate() {
                    by[a as usize].push(i as u16);
                    by[b as usize].push(i as u16);
                }
                by
            })
            .collect();

        // Every combo in someone's range, with its index in each player's list.
        let mut union: Vec<(u8, u8)> = self.hands.iter().flatten().copied().collect();
        union.sort_unstable();
        union.dedup();
        let dead = self.board.iter().fold(0u64, |m, &c| m | (1u64 << c));
        let mut runouts: Vec<Vec<u8>> = Vec::new();
        match self.board.len() {
            3 => {
                for a in 0..52u8 {
                    for b in a + 1..52 {
                        if dead & (1u64 << a) == 0 && dead & (1u64 << b) == 0 {
                            runouts.push(vec![a, b]);
                        }
                    }
                }
            }
            4 => runouts.extend((0..52u8).filter(|&c| dead & (1u64 << c) == 0).map(|c| vec![c])),
            _ => runouts.push(vec![]),
        }
        let board_bits = self.board.iter().fold(0u64, |m, &c| m | card_bit(c));
        let tables: Vec<(usize, Box<[Entry]>)> = runouts
            .par_iter()
            .map(|extra| {
                let mask = extra.iter().fold(0u64, |m, &c| m | (1u64 << c));
                let bits = extra.iter().fold(board_bits, |m, &c| m | card_bit(c));
                let mut entries: Vec<Entry> = union
                    .iter()
                    .filter(|&&(a, b)| mask & ((1u64 << a) | (1u64 << b)) == 0)
                    .map(|&(a, b)| {
                        let k = a as usize * 52 + b as usize;
                        let mut idx = [NONE; MAX_PLAYERS];
                        for p in 0..n {
                            idx[p] = index[p][k];
                        }
                        Entry { strength: strength(bits | card_bit(a) | card_bit(b)), c1: a, c2: b, idx }
                    })
                    .collect();
                entries.sort_by_key(|e| e.strength);
                (runout_key(extra), entries.into_boxed_slice())
            })
            .collect();
        let mut showdowns: Vec<Option<Box<[Entry]>>> = (0..NUM_KEYS).map(|_| None).collect();
        for (k, t) in tables {
            showdowns[k] = Some(t);
        }

        let mut root = Builder::new(&self.cfg).build(self.board.len(), dead);
        allocate_node(&mut root, &self.hands);

        Game {
            n,
            spot: self.spot,
            cfg: self.cfg,
            board_str: self.board_str,
            board: self.board,
            hands: self.hands,
            masks: self.masks,
            weights: self.weights,
            with_card,
            showdowns,
            root,
            mass: self.mass,
            counts: self.counts,
            iteration: 0,
            finalized: false,
        }
    }
}

fn allocate_node(node: &mut Node, hands: &[Vec<(u8, u8)>]) {
    if node.kind == Kind::Decision {
        let size = node.actions.len() * hands[node.player as usize].len();
        node.regret = vec![0.0; size];
        node.strat = vec![0.0; size];
    }
    node.children.par_iter_mut().for_each(|c| allocate_node(c, hands));
}

impl Game {
    pub fn new(spot: &MultiSpot, board: &str) -> Result<Game, String> {
        Ok(Setup::new(spot, board)?.allocate())
    }

    pub fn players(&self) -> Players<'_> {
        Players { n: self.n, hands: &self.hands, masks: &self.masks }
    }

    pub fn start_board(&self) -> Board {
        Board { mask: self.board.iter().fold(0u64, |m, &c| m | (1u64 << c)), count: self.board.len() as u8, extra: [0; 2], extras: 0 }
    }

    pub fn showdown(&self, board: &Board) -> Option<&[Entry]> {
        self.showdowns.get(board.key()).and_then(|t| t.as_deref())
    }

    /// Everyone's reach after `card` is dealt: hands holding it drop out.
    pub fn deal(&self, reach: &[&[f32]], card: u8) -> Vec<Vec<f32>> {
        let mut out: Vec<Vec<f32>> = reach.iter().map(|r| r.to_vec()).collect();
        for (p, r) in out.iter_mut().enumerate() {
            for &i in &self.with_card[p][card as usize] {
                r[i as usize] = 0.0;
            }
        }
        out
    }

    /// Cards that can come at a chance node, for each set of hands: 52 less the board and
    /// everyone's hole cards.
    pub fn chance_factor(&self, board_count: u8) -> f64 {
        (52 - board_count as usize - 2 * self.n) as f64
    }
}
