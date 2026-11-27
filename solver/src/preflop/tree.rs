//! The 6-max preflop game tree: who can fold, limp, call, raise or go all-in at each point, and
//! the pots that reach the flop.

use crate::Rake;
use serde::{Deserialize, Serialize};

pub const SEATS: [&str; 6] = ["UTG", "HJ", "CO", "BTN", "SB", "BB"];
pub const SB: usize = 4;
pub const BB: usize = 5;

/// Order of play after the flop: the SB acts first and the BTN last.
#[inline]
pub fn postflop_order(seat: usize) -> usize {
    if seat >= SB { seat - SB } else { seat + 2 }
}

/// Raise levels: 1 = open (or the BB raising a SB limp), 2 = 3-bet, 3 = 4-bet, 4 = 5-bet.
/// Past the 5-bet only all-in is left.
const MAX_SIZED_LEVEL: u8 = 4;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Everyone's stack at the start of the hand, in bb.
    pub stack_bb: f64,
    /// Open size from UTG, HJ, CO and BTN, in bb.
    pub open_bb: f64,
    /// Open size from the SB.
    pub sb_open_bb: f64,
    /// Whether the SB may limp (complete) when it's folded to it.
    pub sb_limp: bool,
    /// The BB's raise over a SB limp, in bb.
    pub iso_bb: f64,
    /// 3-bet size as a multiple of the open, when the 3-bettor will be in position after the
    /// flop (e.g. BTN against CO) and out of position (the blinds against a steal).
    pub threebet_ip: f64,
    pub threebet_oop: f64,
    /// Each player who called the open adds this many opens to a squeeze.
    pub squeeze_per_caller: f64,
    /// 4-bet size as a multiple of the 3-bet, in and out of position.
    pub fourbet_ip: f64,
    pub fourbet_oop: f64,
    /// 5-bet size as a multiple of the 4-bet (usually all-in at 100bb).
    pub fivebet: f64,
    /// The first raise level where going all-in is always an option besides the sized raise:
    /// 1 = open shoves, 2 = 3-bet shoves, 3 = 4-bet shoves, 4 = 5-bet shoves.
    pub allin_from: u8,
    /// A raise that would leave less than this stack-to-pot ratio behind becomes all-in.
    pub allin_spr: f64,
    /// Whether a player who hasn't put money in voluntarily may call a 3-bet (cold call).
    pub cold_call_3bet: bool,
    /// At most this many players see the flop (2 or 3). Three players can always get all-in.
    pub max_flop_players: u8,
    /// Rake taken from pots that see a flop ("no flop, no drop").
    pub rake: Rake,
    /// "calibrated" (pot share from equity, position and playability, fit to postflop solves)
    /// or "equity" (every pot is shared by raw all-in equity).
    pub postflop: String,
    /// The first seat to act ("UTG" to "SB"); the seats before it have folded. "SB" solves
    /// blind against blind.
    pub start_seat: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            stack_bb: 100.0,
            open_bb: 2.5,
            sb_open_bb: 3.0,
            sb_limp: false,
            iso_bb: 3.5,
            threebet_ip: 3.0,
            threebet_oop: 4.0,
            squeeze_per_caller: 1.0,
            fourbet_ip: 2.2,
            fourbet_oop: 2.5,
            fivebet: 2.2,
            allin_from: 3,
            allin_spr: 1.0,
            cold_call_3bet: false,
            max_flop_players: 3,
            rake: Rake::default(),
            postflop: "calibrated".into(),
            start_seat: "UTG".into(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let check = |ok: bool, msg: &str| if ok { Ok(()) } else { Err(msg.to_string()) };
        check(self.stack_bb.is_finite() && (5.0..=400.0).contains(&self.stack_bb), "Stacks must be 5 to 400bb.")?;
        check(self.open_bb >= 2.0 && self.open_bb <= self.stack_bb, "The open size must be at least 2bb and at most the stack.")?;
        check(self.sb_open_bb >= 2.0 && self.sb_open_bb <= self.stack_bb, "The SB open size must be at least 2bb and at most the stack.")?;
        check(self.iso_bb >= 2.0 && self.iso_bb <= self.stack_bb, "The BB raise over a limp must be at least 2bb.")?;
        for (v, name) in [
            (self.threebet_ip, "3-bet"),
            (self.threebet_oop, "3-bet"),
            (self.fourbet_ip, "4-bet"),
            (self.fourbet_oop, "4-bet"),
            (self.fivebet, "5-bet"),
        ] {
            check(v.is_finite() && (1.5..=10.0).contains(&v), &format!("The {name} size must be 1.5x to 10x."))?;
        }
        check((0.0..=5.0).contains(&self.squeeze_per_caller), "The squeeze extra must be 0 to 5 opens per caller.")?;
        check((1..=5).contains(&self.allin_from), "allin_from must be 1 to 5.")?;
        check((0.0..=3.0).contains(&self.allin_spr), "allin_spr must be 0 to 3.")?;
        check((2..=3).contains(&self.max_flop_players), "At most 2 or 3 players can see the flop.")?;
        check((0.0..=0.2).contains(&self.rake.rate) && self.rake.cap_bb >= 0.0, "Rake must be 0 to 20% with a cap of 0 or more.")?;
        check(self.postflop == "calibrated" || self.postflop == "equity", "postflop must be \"calibrated\" or \"equity\".")?;
        check(self.first_seat().is_some(), "start_seat must be UTG, HJ, CO, BTN or SB.")?;
        Ok(())
    }

    /// The index of `start_seat`.
    pub fn first_seat(&self) -> Option<usize> {
        SEATS[..=SB].iter().position(|&s| s.eq_ignore_ascii_case(&self.start_seat))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActKind {
    Fold,
    Check,
    Call,
    Raise,
    #[serde(rename = "allin")]
    AllIn,
}

#[derive(Clone, Copy, Debug)]
pub struct Action {
    pub kind: ActKind,
    /// The player's total investment in the hand after the action, in bb.
    pub to: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Decision,
    /// Everyone but one player folded.
    Fold,
    /// Two or three players see the flop, or are all-in.
    Showdown,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Kind,
    /// The player to act (decisions) or the winner (fold terminals).
    pub seat: usize,
    pub actions: Vec<Action>,
    pub children: Vec<u32>,
    pub parent: u32,
    pub invested: [f64; 6],
    pub folded: u8,
    pub level: u8,
    pub aggressor: Option<usize>,
    /// Decisions: offset of this node's strategy in the solver's arrays (actions * 169 floats).
    pub slot: usize,
    /// Showdowns: index into `Tree::showdowns`.
    pub showdown: usize,
}

impl Node {
    pub fn pot(&self) -> f64 {
        self.invested.iter().sum()
    }

    pub fn is_folded(&self, seat: usize) -> bool {
        self.folded & (1 << seat) != 0
    }

    pub fn active(&self) -> Vec<usize> {
        (0..6).filter(|&s| !self.is_folded(s)).collect()
    }
}

#[derive(Clone, Debug)]
pub struct Showdown {
    pub node: u32,
    /// The players left, in postflop order (first to act first).
    pub players: Vec<usize>,
    pub pot: f64,
    /// Expected rake, in bb.
    pub rake: f64,
    /// Stack behind divided by the pot; 0 when someone is all-in.
    pub spr: f64,
    /// The last preflop raiser, if any.
    pub aggressor: Option<usize>,
}

pub struct Tree {
    pub config: Config,
    pub nodes: Vec<Node>,
    pub showdowns: Vec<Showdown>,
    /// Total strategy floats (sum of actions * 169 over decisions).
    pub slots: usize,
}

#[derive(Clone)]
struct State {
    invested: [f64; 6],
    folded: [bool; 6],
    allin: [bool; 6],
    pending: [bool; 6],
    voluntary: [bool; 6],
    bet: f64,
    last_raise: f64,
    level: u8,
    aggressor: Option<usize>,
    callers: u8,
    limped: bool,
}

impl State {
    fn folded_mask(&self) -> u8 {
        (0..6).filter(|&s| self.folded[s]).fold(0, |m, s| m | (1 << s))
    }

    fn pot(&self) -> f64 {
        self.invested.iter().sum()
    }

    fn next_actor(&self, after: usize) -> Option<usize> {
        (1..=6).map(|k| (after + k) % 6).find(|&s| !self.folded[s] && !self.allin[s] && self.pending[s])
    }
}

/// Expected final pot as a multiple of the pot at the flop, from the stack-to-pot ratio: about
/// twice the flop pot with normal stacks, the pot itself when everyone is all-in.
pub fn pot_growth(spr: f64) -> f64 {
    1.0 + 1.2 * spr / (spr + 0.6)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

struct Builder<'a> {
    cfg: &'a Config,
    nodes: Vec<Node>,
    showdowns: Vec<Showdown>,
    slots: usize,
}

impl Builder<'_> {
    fn stack(&self) -> f64 {
        self.cfg.stack_bb
    }

    /// The sized raise for `seat`, or None past the 5-bet. The bool says it became all-in.
    fn raise_to(&self, st: &State, seat: usize) -> Option<(f64, bool)> {
        let cfg = self.cfg;
        let next = st.level + 1;
        let ip_vs_aggressor = || st.aggressor.is_some_and(|a| postflop_order(seat) > postflop_order(a));
        let base = match next {
            1 if st.limped && seat == BB => cfg.iso_bb,
            1 if seat == SB => cfg.sb_open_bb,
            1 => cfg.open_bb,
            2 => {
                let m = if ip_vs_aggressor() { cfg.threebet_ip } else { cfg.threebet_oop };
                st.bet * (m + cfg.squeeze_per_caller * st.callers as f64)
            }
            3 => st.bet * if ip_vs_aggressor() { cfg.fourbet_ip } else { cfg.fourbet_oop },
            n if n <= MAX_SIZED_LEVEL => st.bet * cfg.fivebet,
            _ => return None,
        };
        let to = round2(base.max(st.bet + st.last_raise));
        if to >= self.stack() - 1e-9 {
            return Some((self.stack(), true));
        }
        let pot_if_called = st.pot() + (to - st.invested[seat]) + (to - st.bet);
        if self.stack() - to < cfg.allin_spr * pot_if_called {
            return Some((self.stack(), true));
        }
        Some((to, false))
    }

    fn actions(&self, st: &State, seat: usize) -> Vec<Action> {
        let cfg = self.cfg;
        let stack = self.stack();
        let to_call = st.bet - st.invested[seat];
        let mut out = Vec::new();
        if to_call > 1e-9 {
            out.push(Action { kind: ActKind::Fold, to: st.invested[seat] });
            let matched = (0..6).filter(|&p| !st.folded[p] && (st.invested[p] - st.bet).abs() < 1e-9).count();
            let can_call = if st.level == 0 {
                seat == SB && cfg.sb_limp
            } else if st.bet >= stack - 1e-9 {
                // Against an all-in calling is the only way to go on, so anyone may, up to a
                // three-way pot.
                matched < 3
            } else {
                (st.level < 2 || st.voluntary[seat] || cfg.cold_call_3bet) && matched < cfg.max_flop_players as usize
            };
            if can_call {
                out.push(Action { kind: ActKind::Call, to: st.bet.min(stack) });
            }
        } else {
            out.push(Action { kind: ActKind::Check, to: st.invested[seat] });
        }
        if st.bet < stack - 1e-9 {
            let next = st.level + 1;
            let mut allin = next >= cfg.allin_from || next > MAX_SIZED_LEVEL;
            match self.raise_to(st, seat) {
                Some((_, true)) => allin = true,
                Some((to, false)) => out.push(Action { kind: ActKind::Raise, to }),
                None => {}
            }
            if allin {
                out.push(Action { kind: ActKind::AllIn, to: stack });
            }
        }
        out
    }

    fn apply(&self, st: &State, seat: usize, a: Action) -> State {
        let mut s = st.clone();
        s.pending[seat] = false;
        match a.kind {
            ActKind::Fold => s.folded[seat] = true,
            ActKind::Check => {}
            ActKind::Call => {
                s.invested[seat] = a.to;
                s.voluntary[seat] = true;
                s.callers += 1;
                if a.to >= self.stack() - 1e-9 {
                    s.allin[seat] = true;
                }
                if st.level == 0 {
                    s.limped = true;
                }
            }
            ActKind::Raise | ActKind::AllIn => {
                s.last_raise = (a.to - st.bet).max(st.last_raise);
                s.bet = a.to;
                s.invested[seat] = a.to;
                s.level += 1;
                s.aggressor = Some(seat);
                s.callers = 0;
                s.voluntary[seat] = true;
                if a.kind == ActKind::AllIn {
                    s.allin[seat] = true;
                }
                for p in 0..6 {
                    if p != seat && !s.folded[p] && !s.allin[p] {
                        s.pending[p] = true;
                    }
                }
            }
        }
        s
    }

    fn push(&mut self, node: Node) -> u32 {
        self.nodes.push(node);
        (self.nodes.len() - 1) as u32
    }

    fn build(&mut self, st: State, last: usize, parent: u32) -> u32 {
        let base = Node {
            kind: Kind::Decision,
            seat: 0,
            actions: Vec::new(),
            children: Vec::new(),
            parent,
            invested: st.invested,
            folded: st.folded_mask(),
            level: st.level,
            aggressor: st.aggressor,
            slot: 0,
            showdown: 0,
        };
        let alive: Vec<usize> = (0..6).filter(|&s| !st.folded[s]).collect();
        if alive.len() == 1 {
            return self.push(Node { kind: Kind::Fold, seat: alive[0], ..base });
        }
        let Some(seat) = st.next_actor(last) else {
            let id = self.push(Node { kind: Kind::Showdown, showdown: self.showdowns.len(), ..base });
            let pot = st.pot();
            let behind = (self.stack() - st.bet).max(0.0);
            let spr = if behind < 1e-9 { 0.0 } else { behind / pot };
            let rake = (self.cfg.rake.rate * pot * pot_growth(spr)).min(if self.cfg.rake.cap_bb > 0.0 { self.cfg.rake.cap_bb } else { f64::INFINITY });
            let mut players = alive;
            players.sort_by_key(|&s| postflop_order(s));
            self.showdowns.push(Showdown { node: id, players, pot, rake, spr, aggressor: st.aggressor });
            return id;
        };
        // A player who can only fold (when three are already all-in) still gets a decision
        // node with that one action, so every fold is seen where it happens.
        let actions = self.actions(&st, seat);
        let slot = self.slots;
        self.slots += actions.len() * super::cards::NUM_CLASSES;
        let id = self.push(Node { kind: Kind::Decision, seat, actions: actions.clone(), slot, ..base });
        let mut children = Vec::with_capacity(actions.len());
        for a in actions {
            let next = self.apply(&st, seat, a);
            children.push(self.build(next, seat, id));
        }
        self.nodes[id as usize].children = children;
        id
    }
}

impl Tree {
    pub fn build(config: &Config) -> Result<Tree, String> {
        config.validate()?;
        let mut b = Builder { cfg: config, nodes: Vec::new(), showdowns: Vec::new(), slots: 0 };
        let mut invested = [0.0; 6];
        invested[SB] = 0.5;
        invested[BB] = 1.0;
        let first = config.first_seat().unwrap_or(0);
        let st = State {
            invested,
            folded: std::array::from_fn(|s| s < first),
            allin: [false; 6],
            pending: std::array::from_fn(|s| s >= first),
            voluntary: [false; 6],
            bet: 1.0,
            last_raise: 1.0,
            level: 0,
            aggressor: None,
            callers: 0,
            limped: false,
        };
        b.build(st, (first + 5) % 6, u32::MAX);
        Ok(Tree { config: config.clone(), nodes: b.nodes, showdowns: b.showdowns, slots: b.slots })
    }

    /// The seats dealt into the hand (all six unless `start_seat` skips some).
    pub fn players(&self) -> Vec<usize> {
        self.nodes[0].active()
    }

    pub fn decisions(&self) -> usize {
        self.nodes.iter().filter(|n| n.kind == Kind::Decision).count()
    }

    /// The node reached by following action indexes from the root.
    pub fn walk(&self, history: &[usize]) -> Result<usize, String> {
        let mut id = 0usize;
        for (i, &a) in history.iter().enumerate() {
            let n = &self.nodes[id];
            if n.kind != Kind::Decision || a >= n.children.len() {
                return Err(format!("step {i} of the line ({a}) doesn't exist"));
            }
            id = n.children[a] as usize;
        }
        Ok(id)
    }

    /// The action indexes from the root to a node.
    pub fn history(&self, mut id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        while id != 0 {
            let p = self.nodes[id].parent as usize;
            out.push(self.nodes[p].children.iter().position(|&c| c as usize == id).unwrap());
            id = p;
        }
        out.reverse();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(tree: &Tree, steps: &[&str]) -> usize {
        // Steps are action kinds by name, e.g. "fold", "raise".
        let mut id = 0usize;
        for s in steps {
            let n = &tree.nodes[id];
            let i = n.actions.iter().position(|a| format!("{:?}", a.kind).to_lowercase() == *s).unwrap_or_else(|| panic!("no {s} at {:?}", n.actions));
            id = n.children[i] as usize;
        }
        id
    }

    #[test]
    fn default_tree() {
        let tree = Tree::build(&Config::default()).unwrap();
        let root = &tree.nodes[0];
        assert_eq!(root.seat, 0);
        assert_eq!(root.actions.len(), 2); // fold or open
        assert_eq!(root.actions[1].to, 2.5);
        // Folded to the BB: a walk.
        let walk = line(&tree, &["fold", "fold", "fold", "fold", "fold"]);
        assert_eq!(tree.nodes[walk].kind, Kind::Fold);
        assert_eq!(tree.nodes[walk].seat, BB);
        // BTN opens, SB folds, BB 3-bets out of position to 10bb (4x).
        let bb = line(&tree, &["fold", "fold", "fold", "raise", "fold"]);
        assert_eq!(tree.nodes[bb].seat, BB);
        let three = tree.nodes[bb].actions.iter().find(|a| a.kind == ActKind::Raise).unwrap();
        assert_eq!(three.to, 10.0);
        // CO opens, BTN 3-bets in position to 7.5bb.
        let btn = line(&tree, &["fold", "fold", "raise"]);
        assert_eq!(tree.nodes[btn].seat, 3);
        assert_eq!(tree.nodes[btn].actions.iter().find(|a| a.kind == ActKind::Raise).unwrap().to, 7.5);
        // BTN opens, BB calls: a heads-up flop, 5.5bb pot.
        let srp = line(&tree, &["fold", "fold", "fold", "raise", "fold", "call"]);
        let sd = &tree.showdowns[tree.nodes[srp].showdown];
        assert_eq!(sd.players, vec![BB, 3]);
        assert!((sd.pot - 5.5).abs() < 1e-9);
        assert!((sd.spr - 97.5 / 5.5).abs() < 1e-9);
        // 4-bet then 5-bet all-in.
        let fourbet = line(&tree, &["fold", "fold", "fold", "raise", "fold", "raise", "raise"]);
        assert_eq!(tree.nodes[fourbet].seat, BB);
        let acts: Vec<ActKind> = tree.nodes[fourbet].actions.iter().map(|a| a.kind).collect();
        assert_eq!(acts, vec![ActKind::Fold, ActKind::Call, ActKind::AllIn]);
        println!("decisions {} showdowns {} floats {}", tree.decisions(), tree.showdowns.len(), tree.slots);
        let three_way = tree.showdowns.iter().filter(|s| s.players.len() == 3).count();
        println!("3-way showdowns {three_way}");
        assert!(tree.showdowns.iter().all(|s| s.players.len() <= 3));
        assert_eq!(tree.history(fourbet), vec![0, 0, 0, 1, 0, 2, 2]);
        assert_eq!(tree.walk(&[0, 0, 0, 1, 0, 2, 2]).unwrap(), fourbet);
    }

    #[test]
    fn blind_vs_blind() {
        let cfg = Config { start_seat: "SB".into(), stack_bb: 10.0, sb_open_bb: 10.0, ..Config::default() };
        let tree = Tree::build(&cfg).unwrap();
        assert_eq!(tree.players(), vec![SB, BB]);
        let root = &tree.nodes[0];
        assert_eq!(root.seat, SB);
        let acts: Vec<ActKind> = root.actions.iter().map(|a| a.kind).collect();
        assert_eq!(acts, vec![ActKind::Fold, ActKind::AllIn]);
        assert_eq!(tree.decisions(), 2);
        assert!(Tree::build(&Config { start_seat: "BB".into(), ..Config::default() }).is_err());
    }
}
