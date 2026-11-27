//! The game tree of a pot with two or three players: who acts, what they can do and what
//! everyone has put in, from the first street solved to the river.
//!
//! Every player still in acts in postflop order. A bet or raise reopens the action for
//! everyone else still in. Everyone has the same stack, so once an all-in is called the players
//! left are all all-in and the board runs out.

use super::spot::{Config, MAX_PLAYERS};
use postflop_solver::BetSize;

/// An action. Bets, raises and all-ins carry the player's total on this street after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Act {
    Fold,
    Check,
    Call,
    Bet(i32),
    Raise(i32),
    AllIn(i32),
}

impl Act {
    pub fn amount(self) -> Option<i32> {
        match self {
            Act::Bet(a) | Act::Raise(a) | Act::AllIn(a) => Some(a),
            _ => None,
        }
    }
}

/// What everyone has put in at a node, in chips. The starting pot isn't counted as anyone's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pot {
    /// 0 = flop, 1 = turn, 2 = river.
    pub street: u8,
    /// Put in since the start of the solve, by player.
    pub invested: [i32; MAX_PLAYERS],
    /// What every player still in had put in when this street began.
    pub street_start: i32,
    /// The street total to match.
    pub bet: i32,
    /// The size of the last bet or raise, for the minimum raise.
    pub last_raise: i32,
    /// Bets and raises so far on this street.
    pub num_bets: u8,
    /// One bit per player who has folded.
    pub folded: u8,
    /// Someone is all-in, so nobody bets again.
    pub allin: bool,
}

impl Pot {
    pub fn total(&self, starting_pot: i32) -> i32 {
        starting_pot + self.invested.iter().sum::<i32>()
    }

    pub fn is_in(&self, p: usize) -> bool {
        self.folded & (1 << p) == 0
    }

    pub fn players_in(&self, n: usize) -> usize {
        (0..n).filter(|&p| self.is_in(p)).count()
    }

    /// What player `p` has put in on this street.
    pub fn street_bet(&self, p: usize) -> i32 {
        (self.invested[p] - self.street_start).max(0)
    }

    pub fn to_call(&self, p: usize) -> i32 {
        (self.bet - self.street_bet(p)).max(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Decision,
    Chance,
    Terminal,
}

/// A node of the game tree. Decision nodes keep the solver's numbers for the player to act:
/// `#actions * #hands` values, action-major.
pub struct Node {
    pub kind: Kind,
    pub pot: Pot,
    /// The player to act at a decision.
    pub player: u8,
    pub actions: Vec<Act>,
    /// At a chance node, the card each child deals.
    pub cards: Vec<u8>,
    pub children: Vec<Node>,
    /// Cumulative regrets while solving; each action's counterfactual value once finalized.
    pub regret: Vec<f32>,
    /// Cumulative strategy while solving; the average strategy once finalized.
    pub strat: Vec<f32>,
    /// Once finalized, each player's counterfactual value at this node summed over their range
    /// (weighted by how often each hand gets here).
    pub vals: [f32; MAX_PLAYERS],
    /// The iteration that last updated this decision, or `u32::MAX`.
    pub last: u32,
}

impl Node {
    /// An empty node, to hold a tree's place while it's borrowed elsewhere.
    pub fn placeholder() -> Node {
        Node::new(Kind::Terminal, Pot::default())
    }

    fn new(kind: Kind, pot: Pot) -> Node {
        Node {
            kind,
            pot,
            player: 0,
            actions: Vec::new(),
            cards: Vec::new(),
            children: Vec::new(),
            regret: Vec::new(),
            strat: Vec::new(),
            vals: [0.0; MAX_PLAYERS],
            last: u32::MAX,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.kind == Kind::Terminal
    }

    pub fn is_chance(&self) -> bool {
        self.kind == Kind::Chance
    }

    /// The child reached by `step`: an action index, or a card id at a chance node.
    pub fn child_index(&self, step: usize) -> Option<usize> {
        match self.kind {
            Kind::Terminal => None,
            Kind::Decision => (step < self.children.len()).then_some(step),
            Kind::Chance => self.cards.iter().position(|&c| c as usize == step),
        }
    }
}

/// The size of a tree, counted without building it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub nodes: u64,
    pub decisions: u64,
    pub terminals: u64,
    /// Sum of the number of actions over each player's decisions: each player's decision
    /// cells are this times their number of hands.
    pub actions: [u64; MAX_PLAYERS],
}

impl Counts {
    fn add_scaled(&mut self, other: &Counts, k: u64) {
        self.nodes += other.nodes * k;
        self.decisions += other.decisions * k;
        self.terminals += other.terminals * k;
        for p in 0..MAX_PLAYERS {
            self.actions[p] += other.actions[p] * k;
        }
    }

    /// Bytes for the solver's numbers (two f32 arrays per decision) and the nodes themselves.
    pub fn bytes(&self, hands: &[usize]) -> u64 {
        let cells: u64 = hands.iter().enumerate().map(|(p, &h)| self.actions[p] * h as u64).sum();
        cells * 8 + self.nodes * std::mem::size_of::<Node>() as u64
    }
}

/// Builder state: the pot and who still has to act on this street.
#[derive(Clone, Copy, Debug)]
struct State {
    pot: Pot,
    queue: [u8; MAX_PLAYERS],
    queued: u8,
}

impl State {
    fn pop(&mut self) {
        self.queue.copy_within(1.., 0);
        self.queued -= 1;
    }
}

pub struct Builder<'a> {
    cfg: &'a Config,
}

impl<'a> Builder<'a> {
    pub fn new(cfg: &'a Config) -> Builder<'a> {
        Builder { cfg }
    }

    fn n(&self) -> usize {
        self.cfg.players
    }

    /// The state at the start of `street`.
    fn street_start(&self, street: u8, invested: [i32; MAX_PLAYERS], folded: u8, allin: bool) -> State {
        let mut st = State {
            pot: Pot { street, invested, folded, allin, ..Default::default() },
            queue: [0; MAX_PLAYERS],
            queued: 0,
        };
        st.pot.street_start = (0..self.n()).filter(|&p| folded & (1 << p) == 0).map(|p| invested[p]).max().unwrap_or(0);
        if !allin {
            for p in 0..self.n() {
                if folded & (1 << p) == 0 {
                    st.queue[st.queued as usize] = p as u8;
                    st.queued += 1;
                }
            }
        }
        st
    }

    fn root_state(&self, street: u8) -> State {
        self.street_start(street, [0; MAX_PLAYERS], 0, false)
    }

    /// What the player to act can do.
    pub fn actions(&self, pot: &Pot, p: usize) -> Vec<Act> {
        let cfg = self.cfg;
        let street = pot.street as usize;
        let total = pot.total(cfg.starting_pot);
        let to_call = pot.to_call(p);
        let pot_after_call = total + to_call;
        // Everyone still in started the street with the same amount behind.
        let max_amount = cfg.stack - pot.street_start;
        let behind_after_call = (max_amount - pot.bet).max(0);
        let spr_after_call = behind_after_call as f64 / pot_after_call as f64;
        let streets_left = 3 - street as i32;
        let geometric = |num_streets: i32, max_ratio: f64| {
            let ratio = ((2.0 * spr_after_call + 1.0).powf(1.0 / num_streets as f64) - 1.0) / 2.0;
            (pot_after_call as f64 * ratio.min(max_ratio)).round() as i32
        };
        let threshold = |x: f64| (pot_after_call as f64 * x).round() as i32;

        let mut acts = Vec::new();
        if pot.bet == 0 {
            acts.push(Act::Check);
            if max_amount > 0 {
                for &size in &cfg.bets[street][p] {
                    acts.push(match size {
                        BetSize::PotRelative(r) => Act::Bet((pot_after_call as f64 * r).round() as i32),
                        BetSize::Geometric(k, max_ratio) => Act::Bet(geometric(if k == 0 { streets_left } else { k }, max_ratio)),
                        BetSize::AllIn => Act::AllIn(max_amount),
                        BetSize::PrevBetRelative(_) | BetSize::Additive(..) => continue,
                    });
                }
                if !cfg.bets[street][p].is_empty() && max_amount <= threshold(cfg.add_allin_threshold) {
                    acts.push(Act::AllIn(max_amount));
                }
            }
        } else {
            acts.push(Act::Fold);
            acts.push(Act::Call);
            let can_raise = !pot.allin && (pot.num_bets as u32) <= cfg.max_raises && max_amount > pot.bet;
            if can_raise && !cfg.raises[street].is_empty() {
                let bets_so_far = pot.num_bets as i32;
                for &size in &cfg.raises[street] {
                    acts.push(match size {
                        BetSize::PotRelative(r) => Act::Raise(pot.bet + (pot_after_call as f64 * r).round() as i32),
                        BetSize::PrevBetRelative(x) => Act::Raise((pot.bet as f64 * x).round() as i32),
                        BetSize::Geometric(k, max_ratio) => {
                            let k = if k == 0 { streets_left - bets_so_far + 1 } else { k - bets_so_far + 1 };
                            Act::Raise(pot.bet + geometric(k.max(1), max_ratio))
                        }
                        BetSize::AllIn => Act::AllIn(max_amount),
                        BetSize::Additive(..) => continue,
                    });
                }
                if max_amount <= pot.bet + threshold(cfg.add_allin_threshold) {
                    acts.push(Act::AllIn(max_amount));
                }
            }
        }

        // Clamp to the minimum and maximum, and go all-in when little would be left behind.
        let min_amount = if pot.bet == 0 { 1 } else { pot.bet + pot.last_raise.max(1) }.clamp(1, max_amount.max(1));
        let above_threshold = |amount: i32| {
            let new_pot = pot_after_call + 2 * (amount - pot.bet);
            max_amount <= amount + (new_pot as f64 * cfg.force_allin_threshold).round() as i32
        };
        for a in acts.iter_mut() {
            if let Act::Bet(x) | Act::Raise(x) = *a {
                let clamped = x.clamp(min_amount, max_amount);
                if above_threshold(clamped) {
                    *a = Act::AllIn(max_amount);
                } else if let Act::Bet(_) = *a {
                    *a = Act::Bet(clamped);
                } else {
                    *a = Act::Raise(clamped);
                }
            }
        }
        acts.sort_unstable();
        acts.dedup();
        merge_close_sizes(acts, pot_after_call, pot.bet, cfg.merging_threshold)
    }

    /// The state after player `p` takes `act`.
    fn after(&self, st: &State, p: usize, act: Act) -> State {
        let mut next = *st;
        next.pop();
        let pot = &mut next.pot;
        match act {
            Act::Fold => pot.folded |= 1 << p,
            Act::Check => {}
            Act::Call => pot.invested[p] = st.pot.street_start + st.pot.bet,
            Act::Bet(amount) | Act::Raise(amount) | Act::AllIn(amount) => {
                pot.invested[p] = st.pot.street_start + amount;
                pot.last_raise = amount - st.pot.bet;
                pot.bet = amount;
                pot.num_bets += 1;
                if let Act::AllIn(_) = act {
                    pot.allin = true;
                }
                // Everyone else still in acts again, in order after `p`.
                next.queued = 0;
                for i in 1..self.n() {
                    let q = (p + i) % self.n();
                    if pot.folded & (1 << q) == 0 {
                        next.queue[next.queued as usize] = q as u8;
                        next.queued += 1;
                    }
                }
            }
        }
        next
    }

    /// Builds the whole tree from the first street of a board with `board_cards` cards dealt
    /// (`dead` is their mask). Decision nodes get no memory yet.
    pub fn build(&self, board_cards: usize, dead: u64) -> Node {
        let street = (board_cards - 3) as u8;
        self.build_state(self.root_state(street), dead)
    }

    fn build_state(&self, st: State, dead: u64) -> Node {
        let n = self.n();
        if st.pot.players_in(n) == 1 {
            return Node::new(Kind::Terminal, st.pot);
        }
        if st.queued == 0 {
            if st.pot.street == 2 {
                return Node::new(Kind::Terminal, st.pot);
            }
            let mut node = Node::new(Kind::Chance, st.pot);
            let next = self.street_start(st.pot.street + 1, st.pot.invested, st.pot.folded, st.pot.allin);
            for card in 0..52u8 {
                if dead & (1u64 << card) == 0 {
                    node.cards.push(card);
                    node.children.push(self.build_state(next, dead | (1u64 << card)));
                }
            }
            return node;
        }
        let p = st.queue[0] as usize;
        let mut node = Node::new(Kind::Decision, st.pot);
        node.player = p as u8;
        node.actions = self.actions(&st.pot, p);
        node.children = node.actions.iter().map(|&a| self.build_state(self.after(&st, p, a), dead)).collect();
        node
    }

    /// Counts the tree `build` would make, without building it.
    pub fn count(&self, board_cards: usize) -> Counts {
        let street = (board_cards - 3) as u8;
        self.count_state(self.root_state(street), board_cards)
    }

    fn count_state(&self, st: State, board_cards: usize) -> Counts {
        let n = self.n();
        let mut c = Counts { nodes: 1, ..Default::default() };
        if st.pot.players_in(n) == 1 || (st.queued == 0 && st.pot.street == 2) {
            c.terminals = 1;
            return c;
        }
        if st.queued == 0 {
            // Every card leads to the same tree.
            let next = self.street_start(st.pot.street + 1, st.pot.invested, st.pot.folded, st.pot.allin);
            let child = self.count_state(next, board_cards + 1);
            c.add_scaled(&child, (52 - board_cards) as u64);
            return c;
        }
        let p = st.queue[0] as usize;
        let actions = self.actions(&st.pot, p);
        c.decisions = 1;
        c.actions[p] = actions.len() as u64;
        for a in actions {
            let child = self.count_state(self.after(&st, p, a), board_cards);
            c.add_scaled(&child, 1);
        }
        c
    }
}

/// Drops bet sizes within `param` of a bigger one (as in the heads-up solver's tree builder).
fn merge_close_sizes(acts: Vec<Act>, pot: i32, offset: i32, param: f64) -> Vec<Act> {
    const EPS: f64 = 1e-12;
    let mut cur = i32::MAX;
    let mut out = Vec::with_capacity(acts.len());
    for &a in acts.iter().rev() {
        match a.amount() {
            Some(amount) => {
                let ratio = (amount - offset) as f64 / pot as f64;
                let cur_ratio = (cur as f64 - offset as f64) / pot as f64;
                let threshold = (cur_ratio - param) / (1.0 + param);
                if ratio < threshold * (1.0 - EPS) {
                    out.push(a);
                    cur = amount;
                }
            }
            None => out.push(a),
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::super::spot::{MultiSpot, Sizes, StreetSizes};
    use super::*;
    use crate::Rake;

    pub fn spot3(flop: &str, raise: &str, turn: &str, river: &str, pot: f64, stack: f64) -> MultiSpot {
        let st = |b: &str, r: &str| StreetSizes { bet: vec![b.to_string(); 3], raise: r.into() };
        MultiSpot {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            players: vec!["A".into(), "B".into(), "C".into()],
            ranges: vec!["AA".into(); 3],
            pot_bb: pot,
            stack_bb: stack,
            bets: Sizes { flop: st(flop, raise), turn: st(turn, ""), river: st(river, "") },
            max_raises: 1,
            rake: Rake::default(),
        }
    }

    /// Walks the tree, checking that every node's numbers add up.
    fn check(node: &Node, cfg: &Config) {
        let n = cfg.players;
        for p in 0..n {
            assert!(node.pot.invested[p] <= cfg.stack, "{:?}", node.pot);
        }
        match node.kind {
            Kind::Terminal => {
                let pot = &node.pot;
                if pot.players_in(n) > 1 {
                    // A showdown: everyone still in put in the same.
                    let amounts: Vec<i32> = (0..n).filter(|&p| pot.is_in(p)).map(|p| pot.invested[p]).collect();
                    assert!(amounts.iter().all(|&a| a == amounts[0]), "{pot:?}");
                    assert_eq!(pot.street, 2);
                }
            }
            Kind::Chance => {
                for c in &node.children {
                    assert_eq!(c.pot.street, node.pot.street + 1);
                    check(c, cfg);
                }
            }
            Kind::Decision => {
                let p = node.player as usize;
                assert!(node.pot.is_in(p));
                assert_eq!(node.actions.len(), node.children.len());
                for (a, c) in node.actions.iter().zip(&node.children) {
                    match a {
                        Act::Fold => assert!(!c.pot.is_in(p)),
                        Act::Call => assert_eq!(c.pot.invested[p] - node.pot.street_start, node.pot.bet),
                        _ => {}
                    }
                    check(c, cfg);
                }
            }
        }
    }

    #[test]
    fn river_tree_shape() {
        let spot = spot3("33%", "", "50%", "50%", 10.0, 100.0);
        let cfg = spot.config().unwrap();
        let b = Builder::new(&cfg);
        let root = b.build(5, 0);
        check(&root, &cfg);
        // One size, no raises: 12 decisions and 13 terminals.
        let counts = b.count(5);
        assert_eq!((counts.decisions, counts.terminals), (12, 13));
        assert_eq!(root.actions, vec![Act::Check, Act::Bet(100)]);
        // A bets 5bb: B folds or calls; then C faces it.
        let facing = &root.children[1];
        assert_eq!(facing.player, 1);
        assert_eq!(facing.actions, vec![Act::Fold, Act::Call]);
        let after_call = &facing.children[1];
        assert_eq!(after_call.player, 2);
        assert_eq!(after_call.pot.invested, [100, 100, 0]);
        // Both call: three-way showdown with 25bb in the pot.
        let sd = &after_call.children[1];
        assert!(sd.is_terminal());
        assert_eq!(sd.pot.total(cfg.starting_pot), 500);
        // Checked to C, who bets; A and B answer in order.
        let c_bets = &root.children[0].children[0].children[1];
        assert_eq!((c_bets.player, c_bets.actions.clone()), (0, vec![Act::Fold, Act::Call]));
        assert_eq!(c_bets.children[0].player, 1);
    }

    #[test]
    fn raises_reopen_the_action() {
        let spot = spot3("50%", "3x", "50%", "50%", 10.0, 100.0);
        let cfg = spot.config().unwrap();
        let b = Builder::new(&cfg);
        let root = b.build(3, 0b111);
        // A bets 5, B raises to 15, C cold-calls or folds, then A answers. No more raises.
        let b_node = &root.children[1];
        assert_eq!(b_node.actions, vec![Act::Fold, Act::Call, Act::Raise(300)]);
        let c_node = &b_node.children[2];
        assert_eq!((c_node.player, c_node.actions.clone()), (2, vec![Act::Fold, Act::Call]));
        let a_node = &c_node.children[1];
        assert_eq!((a_node.player, a_node.actions.clone()), (0, vec![Act::Fold, Act::Call]));
        // A calls: the turn is dealt with 55bb in the pot and 49 cards to come.
        let turn = &a_node.children[1];
        assert!(turn.is_chance());
        assert_eq!(turn.cards.len(), 49);
        assert_eq!(turn.pot.total(cfg.starting_pot), 1100);
        assert_eq!(turn.children[0].pot.street_start, 300);
        check(&turn.children[0], &cfg);
        let counts = b.count(3);
        let mut nodes = 0;
        fn walk(n: &Node, c: &mut u64) {
            *c += 1;
            n.children.iter().for_each(|ch| walk(ch, c));
        }
        let full = b.build(3, 0b111);
        walk(&full, &mut nodes);
        assert_eq!(counts.nodes, nodes);
    }

    #[test]
    fn allin_runs_out() {
        // Short stacks: all-in is an option, and after it's called nobody acts again.
        let spot = spot3("50%", "", "50%", "50%", 10.0, 12.0);
        let cfg = spot.config().unwrap();
        let b = Builder::new(&cfg);
        let root = b.build(4, 0b1111);
        assert!(root.actions.contains(&Act::AllIn(240)));
        let ai = root.actions.iter().position(|a| matches!(a, Act::AllIn(_))).unwrap();
        let facing = &root.children[ai];
        assert_eq!(facing.actions, vec![Act::Fold, Act::Call]);
        let called = &facing.children[1].children[0];
        // B called, C folded: the river comes with no more decisions.
        assert!(called.is_chance());
        assert!(called.children.iter().all(|c| c.is_terminal()));
        assert_eq!(called.children[0].pot.folded, 0b100);
        check(&root, &cfg);
    }

    #[test]
    fn heads_up_matches_postflop_solver_sizes() {
        // Two players: the same actions as postflop-solver's tree for the same sizes.
        use postflop_solver::*;
        // Raises on every street: postflop-solver offers an all-in raise at a low stack-to-pot
        // ratio even with no raise sizes, where this tree offers no raise at all.
        let mut spot = spot3("33%, 75%", "3x", "66%", "75%", 5.5, 97.5);
        spot.bets.turn.raise = "3x".into();
        spot.bets.river.raise = "3x".into();
        spot.players.pop();
        spot.ranges.pop();
        for s in [&mut spot.bets.flop, &mut spot.bets.turn, &mut spot.bets.river] {
            s.bet.pop();
        }
        spot.max_raises = 4;
        let cfg = spot.config().unwrap();
        let b = Builder::new(&cfg);
        let mine = b.build(3, 0b111);

        let tree_config = TreeConfig {
            initial_state: BoardState::Flop,
            starting_pot: 110,
            effective_stack: 1950,
            rake_rate: 0.0,
            rake_cap: 0.0,
            flop_bet_sizes: [("33%, 75%", "3x").try_into().unwrap(), ("33%, 75%", "3x").try_into().unwrap()],
            turn_bet_sizes: [("66%", "3x").try_into().unwrap(), ("66%", "3x").try_into().unwrap()],
            river_bet_sizes: [("75%", "3x").try_into().unwrap(), ("75%", "3x").try_into().unwrap()],
            turn_donk_sizes: None,
            river_donk_sizes: None,
            add_allin_threshold: 1.5,
            force_allin_threshold: 0.15,
            merging_threshold: 0.1,
        };
        let mut theirs = ActionTree::new(tree_config).unwrap();
        // Their tree skips chance nodes when playing, so this follows the first card of each.
        fn same(mut mine: &Node, theirs: &mut ActionTree) {
            while mine.is_chance() {
                mine = &mine.children[0];
            }
            if theirs.is_terminal_node() {
                assert!(mine.is_terminal(), "at {:?}", theirs.history());
                return;
            }
            let acts: Vec<Action> = theirs.available_actions().to_vec();
            let conv: Vec<Act> = acts
                .iter()
                .map(|a| match *a {
                    Action::Fold => Act::Fold,
                    Action::Check => Act::Check,
                    Action::Call => Act::Call,
                    Action::Bet(x) => Act::Bet(x),
                    Action::Raise(x) => Act::Raise(x),
                    Action::AllIn(x) => Act::AllIn(x),
                    _ => unreachable!(),
                })
                .collect();
            assert_eq!(mine.actions, conv, "at {:?}", theirs.history());
            for (i, a) in acts.iter().enumerate() {
                theirs.play(*a).unwrap();
                same(&mine.children[i], theirs);
                theirs.undo().unwrap();
            }
        }
        same(&mine, &mut theirs);
    }
}
