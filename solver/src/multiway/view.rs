//! Looking at a solved multiway game: the node after a history of actions and cards, with every
//! player's reach, and the JSON the web app shows for it.
//!
//! EVs follow the heads-up solver: what a hand expects to collect from here on minus what it
//! will still put in (so folding is worth 0). Equity is a hand's share of the pot if everyone
//! still in checked it down from here.

use super::eval::{self, Entry};
use super::game::{Board, Game};
use super::tree::{Act, Kind, Node, Pot};
use crate::{bb, round, CHIPS_PER_BB};
use postflop_solver::{card_to_string, holes_to_strings};
use rayon::prelude::*;
use serde_json::{json, Value};

/// A node of a solved game, with how everyone got there.
pub struct Position<'a> {
    pub node: &'a Node,
    pub board: Board,
    /// The board as text, with the cards dealt so far.
    pub board_str: String,
    /// Each player's weight for each hand: their range times how often they made the plays so far
    /// (0 for hands that hold a board card).
    pub reach: Vec<Vec<f32>>,
    /// Who acted at each step: a player, or `None` where a card came.
    pub actors: Vec<Option<usize>>,
}

fn slices(reach: &[Vec<f32>]) -> Vec<&[f32]> {
    reach.iter().map(|r| r.as_slice()).collect()
}

/// Follows `history` from the root: action indexes at decisions, card ids (0-51) at chance
/// nodes, as in the heads-up solver.
pub fn navigate<'a>(game: &'a Game, history: &[usize]) -> Result<Position<'a>, String> {
    if !game.finalized {
        return Err("The solve hasn't finished yet.".into());
    }
    let mut node = &game.root;
    let mut board = game.start_board();
    let mut board_str = game.board_str.clone();
    let mut reach = game.weights.clone();
    let mut actors = Vec::with_capacity(history.len());
    for (i, &step) in history.iter().enumerate() {
        let Some(c) = node.child_index(step) else {
            return Err(format!("Step {} of the history ({step}) isn't possible there.", i + 1));
        };
        match node.kind {
            Kind::Decision => {
                let q = node.player as usize;
                let nq = game.hands[q].len();
                for (r, s) in reach[q].iter_mut().zip(&node.strat[step * nq..(step + 1) * nq]) {
                    *r *= s;
                }
                actors.push(Some(q));
            }
            Kind::Chance => {
                reach = game.deal(&slices(&reach), step as u8);
                board = board.with(step as u8);
                board_str.push_str(&card_to_string(step as u8)?);
                actors.push(None);
            }
            Kind::Terminal => unreachable!("a terminal node has no children"),
        }
        node = &node.children[c];
    }
    Ok(Position { node, board, board_str, reach, actors })
}

fn alive_mask(pot: &Pot, n: usize) -> u8 {
    (0..n).filter(|&p| pot.is_in(p)).fold(0, |m, p| m | (1 << p))
}

/// An action as JSON, with sizes in bb and as a share of the pot after calling, like the
/// heads-up solver's.
pub fn act_json(act: Act, pot: &Pot, starting_pot: i32, p: usize) -> Value {
    let total = pot.total(starting_pot);
    let to_call = pot.to_call(p);
    let mine = pot.street_bet(p);
    let sized = |kind: &str, to: i32| {
        let added = to - mine;
        let pct = ((added - to_call) as f64 / (total + to_call) as f64 * 100.0).round() as i64;
        json!({ "type": kind, "to_bb": bb(to), "add_bb": bb(added), "pct": pct })
    };
    match act {
        Act::Fold => json!({ "type": "fold" }),
        Act::Check => json!({ "type": "check" }),
        Act::Call => json!({ "type": "call", "add_bb": bb(to_call) }),
        Act::Bet(to) => sized("bet", to),
        Act::Raise(to) => sized("raise", to),
        Act::AllIn(to) => sized("allin", to),
    }
}

/// Every way the rest of the board can come from `board`, one entry per set of cards.
fn runouts(board: &Board) -> Vec<Vec<u8>> {
    let free: Vec<u8> = (0..52u8).filter(|&c| board.mask & (1u64 << c) == 0).collect();
    match 5 - board.count {
        0 => vec![vec![]],
        1 => free.iter().map(|&c| vec![c]).collect(),
        _ => {
            let mut out = Vec::new();
            for (i, &a) in free.iter().enumerate() {
                for &b in &free[i + 1..] {
                    out.push(vec![a, b]);
                }
            }
            out
        }
    }
}

/// For each of player `p`'s hands, its pot share summed over every runout and the opponents'
/// hands, and the weight of those hands: if everyone still in (`alive`) checked down from here.
fn shares(game: &Game, board: &Board, reach: &[Vec<f32>], p: usize, alive: u8) -> (Vec<f64>, Vec<f64>) {
    let np = game.hands[p].len();
    let pl = game.players();
    runouts(board)
        .par_iter()
        .fold(
            || (vec![0.0; np], vec![0.0; np]),
            |(mut win, mut weight), extra| {
                let mut b = *board;
                let mut r = reach.to_vec();
                for &c in extra {
                    r = game.deal(&slices(&r), c);
                    b = b.with(c);
                }
                let entries: &[Entry] = game.showdown(&b).expect("every runout has a showdown table");
                let (mut w, mut c) = (vec![0.0; np], vec![0.0; np]);
                if alive.count_ones() > 1 {
                    eval::showdown(&pl, entries, p, alive, &slices(&r), &mut w, &mut c);
                } else {
                    eval::compatible(&pl, p, &slices(&r), &mut c);
                    w.copy_from_slice(&c);
                }
                for h in 0..np {
                    // Hands that hold a runout card can't see it come.
                    if pl.masks[p][h] & b.mask == 0 {
                        win[h] += w[h];
                        weight[h] += c[h];
                    }
                }
                (win, weight)
            },
        )
        .reduce(
            || (vec![0.0; np], vec![0.0; np]),
            |(mut a, mut b), (c, d)| {
                a.iter_mut().zip(&c).for_each(|(x, y)| *x += y);
                b.iter_mut().zip(&d).for_each(|(x, y)| *x += y);
                (a, b)
            },
        )
}

/// A player's range-wide equity from [`shares`].
fn range_equity(reach: &[f32], win: &[f64], weight: &[f64]) -> Option<f64> {
    let w: f64 = reach.iter().zip(win).map(|(&r, &x)| r as f64 * x).sum();
    let t: f64 = reach.iter().zip(weight).map(|(&r, &x)| r as f64 * x).sum();
    (t > 0.0).then(|| round(w / t, 4))
}

/// The total weight of the deals that reach the node: the same from every player's side.
fn deal_weight(game: &Game, reach: &[Vec<f32>]) -> f64 {
    let mut compat = vec![0.0; game.hands[0].len()];
    eval::compatible(&game.players(), 0, &slices(reach), &mut compat);
    reach[0].iter().zip(&compat).map(|(&r, &c)| r as f64 * c).sum()
}

/// Everyone at a node: chips in, stack behind, range EV and equity.
fn players_json(game: &Game, pos: &Position, equity: &[Option<f64>]) -> Value {
    let pot = &pos.node.pot;
    let weight = deal_weight(game, &pos.reach);
    let players: Vec<Value> = (0..game.n)
        .map(|i| {
            let folded = !pot.is_in(i);
            let behind = game.cfg.stack - pot.invested[i];
            // The node's value for this player, per deal that gets here, plus what they've put in.
            let ev = (!folded && weight > 0.0).then(|| round((pos.node.vals[i] as f64 * game.mass / weight + pot.invested[i] as f64) / CHIPS_PER_BB, 2));
            json!({
                "name": game.spot.players[i],
                "invested_bb": bb(pot.invested[i]),
                "street_bb": bb(if folded { 0 } else { pot.street_bet(i) }),
                "behind_bb": bb(behind),
                "folded": folded,
                "allin": !folded && behind == 0,
                "combos": round(pos.reach[i].iter().map(|&r| r as f64).sum(), 1),
                "ev_bb": if folded { Some(0.0) } else { ev },
                "equity": equity.get(i).copied().flatten(),
            })
        })
        .collect();
    json!(players)
}

/// Everyone's range equity at a node (null for players who folded).
fn all_equity(game: &Game, board: &Board, reach: &[Vec<f32>], pot: &Pot) -> Vec<Option<f64>> {
    let alive = alive_mask(pot, game.n);
    (0..game.n)
        .map(|i| {
            if alive & (1 << i) == 0 {
                return None;
            }
            let (w, t) = shares(game, board, reach, i, alive);
            range_equity(&reach[i], &w, &t)
        })
        .collect()
}

fn street_name(board: &Board) -> &'static str {
    match board.count {
        3 => "flop",
        4 => "turn",
        _ => "river",
    }
}

fn actors_json(game: &Game, actors: &[Option<usize>]) -> Value {
    json!(actors.iter().map(|a| a.map(|p| game.spot.players[p].clone())).collect::<Vec<_>>())
}

/// The JSON for the node at `pos`.
pub fn node_json(game: &Game, pos: &Position) -> Result<Value, String> {
    let node = pos.node;
    let pot = &node.pot;
    let mut out = match node.kind {
        Kind::Decision => decision_json(game, pos)?,
        Kind::Chance => chance_json(game, pos)?,
        Kind::Terminal => {
            let alive = alive_mask(pot, game.n);
            let winner = (alive.count_ones() == 1).then(|| game.spot.players[alive.trailing_zeros() as usize].clone());
            json!({ "type": "terminal", "winner": winner, "players": players_json(game, pos, &[]) })
        }
    };
    out["board"] = json!(pos.board_str);
    out["street"] = json!(street_name(&pos.board));
    out["pot_bb"] = json!(bb(pot.total(game.cfg.starting_pot)));
    out["actors"] = actors_json(game, &pos.actors);
    Ok(out)
}

fn decision_json(game: &Game, pos: &Position) -> Result<Value, String> {
    let node = pos.node;
    let pot = &node.pot;
    let q = node.player as usize;
    let nq = game.hands[q].len();
    let k = node.actions.len();
    let alive = alive_mask(pot, game.n);

    // The weight of the opponents' hands that each hand can be up against here.
    let mut compat = vec![0.0; nq];
    eval::compatible(&game.players(), q, &slices(&pos.reach), &mut compat);
    let mut equity = vec![None; game.n];
    let mut hand_eq = vec![0.0; nq];
    for i in 0..game.n {
        if alive & (1 << i) == 0 {
            continue;
        }
        let (w, t) = shares(game, &pos.board, &pos.reach, i, alive);
        equity[i] = range_equity(&pos.reach[i], &w, &t);
        if i == q {
            for h in 0..nq {
                hand_eq[h] = if t[h] > 0.0 { w[h] / t[h] } else { 0.0 };
            }
        }
    }

    let invested = pot.invested[q] as f64;
    let reach = &pos.reach[q];
    let hands: Vec<Value> = (0..nq)
        .map(|h| {
            if reach[h] <= 1e-4 || compat[h] <= 0.0 {
                return Value::Null;
            }
            let mut row = Vec::with_capacity(2 + 2 * k);
            row.push(json!(round(reach[h] as f64, 3)));
            for a in 0..k {
                row.push(json!(round(node.strat[a * nq + h] as f64, 3)));
            }
            for a in 0..k {
                let ev = if node.actions[a] == Act::Fold { 0.0 } else { node.regret[a * nq + h] as f64 * game.mass / compat[h] + invested };
                row.push(json!(round(ev / CHIPS_PER_BB, 2)));
            }
            row.push(json!(round(hand_eq[h], 3)));
            Value::Array(row)
        })
        .collect();

    Ok(json!({
        "type": "decision",
        "player": q,
        "to_call_bb": bb(pot.to_call(q)),
        "actions": node.actions.iter().map(|&a| act_json(a, pot, game.cfg.starting_pot, q)).collect::<Vec<_>>(),
        "combos": holes_to_strings(&game.hands[q])?,
        "hands": hands,
        "players": players_json(game, pos, &equity),
    }))
}

/// Range-wide action frequencies of the player to act, weighted by reach.
fn frequencies(game: &Game, node: &Node, reach: &[f32]) -> Vec<f64> {
    let nq = game.hands[node.player as usize].len();
    let total: f64 = reach.iter().map(|&r| r as f64).sum();
    (0..node.actions.len())
        .map(|a| {
            let f: f64 = (0..nq).map(|h| node.strat[a * nq + h] as f64 * reach[h] as f64).sum();
            if total > 0.0 { round(f / total, 4) } else { 0.0 }
        })
        .collect()
}

/// A report on every card that can come: the next decision's action mix and everyone's equity.
fn chance_json(game: &Game, pos: &Position) -> Result<Value, String> {
    let node = pos.node;
    let alive = alive_mask(&node.pot, game.n);
    let cards: Vec<Value> = node
        .cards
        .par_iter()
        .zip(node.children.par_iter())
        .map(|(&card, child)| {
            let reach = game.deal(&slices(&pos.reach), card);
            let board = pos.board.with(card);
            let mut entry = json!({ "card": card, "name": card_to_string(card).unwrap_or_default() });
            let eq: Vec<Option<f64>> = (0..game.n)
                .map(|i| {
                    if alive & (1 << i) == 0 {
                        return None;
                    }
                    let (w, t) = shares(game, &board, &reach, i, alive);
                    range_equity(&reach[i], &w, &t)
                })
                .collect();
            entry["equity"] = json!(eq);
            if child.kind == Kind::Decision {
                let q = child.player as usize;
                entry["player"] = json!(q);
                entry["actions"] = json!(child.actions.iter().map(|&a| act_json(a, &child.pot, game.cfg.starting_pot, q)).collect::<Vec<_>>());
                entry["freq"] = json!(frequencies(game, child, &reach[q]));
            }
            entry
        })
        .collect();
    let equity = all_equity(game, &pos.board, &pos.reach, &node.pot);
    Ok(json!({ "type": "chance", "cards": cards, "players": players_json(game, pos, &equity) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiway::cfr;
    use crate::multiway::spot::{MultiSpot, Sizes, StreetSizes};
    use std::sync::atomic::AtomicBool;

    fn spot() -> MultiSpot {
        let street = |b: &str| StreetSizes { bet: vec![b.to_string(); 3], raise: String::new() };
        MultiSpot {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            players: vec!["BB".into(), "CO".into(), "BTN".into()],
            ranges: vec!["99-22,AJs-A2s,KTs+,QTs+,JTs,T9s,98s,87s,76s,65s,AJo-ATo,KJo+".into(), "22+,A2s+,K9s+,QTs+,JTs,T9s,98s,ATo+,KJo+".into(), "JJ-22,AQs-A9s,KQs-KTs,QJs,JTs,T9s,98s,87s,AQo-AJo,KQo".into()],
            pot_bb: 8.0,
            stack_bb: 40.0,
            bets: Sizes { flop: street("50%"), turn: street("60%"), river: street("75%") },
            max_raises: 1,
            rake: Default::default(),
        }
    }

    /// On the river: the EVs the view shows add up (everyone's range EV is the pot), each hand's
    /// action EVs average to its range EV, and navigating changes who acts.
    #[test]
    fn river_node_values_add_up() {
        let mut game = Game::new(&spot(), "Ks7d2c5h9c").unwrap();
        cfr::solve(&mut game, 300, 0.01, 50, &AtomicBool::new(false), |_, _| {});
        let pos = navigate(&game, &[]).unwrap();
        let v = node_json(&game, &pos).unwrap();
        assert_eq!(v["type"], "decision");
        assert_eq!(v["player"], 0);
        let total: f64 = v["players"].as_array().unwrap().iter().map(|p| p["ev_bb"].as_f64().unwrap()).sum();
        assert!((total - 8.0).abs() < 0.02, "range EVs add up to {total}, not the 8bb pot");
        let eq: f64 = v["players"].as_array().unwrap().iter().map(|p| p["equity"].as_f64().unwrap()).sum();
        assert!((eq - 1.0).abs() < 0.01, "equities add up to {eq}");

        // The acting player's hands, weighted by how often they get here, average to their range EV.
        let k = v["actions"].as_array().unwrap().len();
        let (mut num, mut den) = (0.0, 0.0);
        let mut compat = vec![0.0; game.hands[0].len()];
        eval::compatible(&game.players(), 0, &slices(&pos.reach), &mut compat);
        for (h, row) in v["hands"].as_array().unwrap().iter().enumerate() {
            if row.is_null() {
                continue;
            }
            let row = row.as_array().unwrap();
            let ev: f64 = (0..k).map(|a| row[1 + a].as_f64().unwrap() * row[1 + k + a].as_f64().unwrap()).sum();
            let w = pos.reach[0][h] as f64 * compat[h];
            num += w * ev;
            den += w;
        }
        let range_ev = v["players"][0]["ev_bb"].as_f64().unwrap();
        assert!((num / den - range_ev).abs() < 0.05, "hands average {} vs range {range_ev}", num / den);

        // After BB checks, CO acts.
        let next = node_json(&game, &navigate(&game, &[0]).unwrap()).unwrap();
        assert_eq!(next["player"], 1);
        assert_eq!(next["actors"], json!(["BB"]));
        assert!(navigate(&game, &[9]).is_err());
    }

    /// On the turn, a check-around reaches the river card report, with every card that can come.
    #[test]
    fn turn_chance_report() {
        let mut game = Game::new(&spot(), "Ks7d2c5h").unwrap();
        cfr::solve(&mut game, 20, 0.0, 20, &AtomicBool::new(false), |_, _| {});
        let pos = navigate(&game, &[0, 0, 0]).unwrap();
        let v = node_json(&game, &pos).unwrap();
        assert_eq!(v["type"], "chance");
        assert_eq!(v["cards"].as_array().unwrap().len(), 48);
        let first = &v["cards"][0];
        assert_eq!(first["player"], 0);
        let eq: f64 = first["equity"].as_array().unwrap().iter().map(|e| e.as_f64().unwrap()).sum();
        assert!((eq - 1.0).abs() < 0.01);
        let card = first["card"].as_u64().unwrap() as usize;
        let river = node_json(&game, &navigate(&game, &[0, 0, 0, card]).unwrap()).unwrap();
        assert_eq!(river["street"], "river");
        assert_eq!(river["board"].as_str().unwrap().len(), 10);
    }
}
