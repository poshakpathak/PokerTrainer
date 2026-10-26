//! Shared pieces of the PokerTrainer solver tools: spot configs, building a game
//! from a spot and board, and turning solver nodes into JSON for the web app.

use postflop_solver::*;
use serde::Deserialize;
use serde_json::{json, Value};

/// Chip units per big blind. The solver works in integers, so we use 1/20 bb.
pub const CHIPS_PER_BB: f64 = 20.0;

#[derive(Deserialize, Clone)]
pub struct StreetBets {
    pub oop: String,
    pub ip: String,
    pub raise: String,
    #[serde(default)]
    pub donk: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct Bets {
    pub flop: StreetBets,
    pub turn: StreetBets,
    pub river: StreetBets,
}

#[derive(Deserialize, Clone)]
pub struct Ranges {
    pub oop: String,
    pub ip: String,
}

#[derive(Deserialize, Default, Clone)]
pub struct Rake {
    #[serde(default)]
    pub rate: f64,
    #[serde(default)]
    pub cap_bb: f64,
}

#[derive(Deserialize, Clone)]
pub struct Spot {
    #[serde(default)]
    pub id: String,
    pub pot_bb: f64,
    pub stack_bb: f64,
    pub ranges: Ranges,
    pub bets: Bets,
    #[serde(default)]
    pub rake: Rake,
}

pub fn bet_sizes(s: &StreetBets) -> Result<[BetSizeOptions; 2], String> {
    Ok([
        BetSizeOptions::try_from((s.oop.as_str(), s.raise.as_str()))?,
        BetSizeOptions::try_from((s.ip.as_str(), s.raise.as_str()))?,
    ])
}

pub fn donk_sizes(s: &StreetBets) -> Result<Option<DonkSizeOptions>, String> {
    s.donk
        .as_deref()
        .map(DonkSizeOptions::try_from)
        .transpose()
}

pub fn bb(chips: i32) -> f64 {
    round(chips as f64 / CHIPS_PER_BB, 2)
}

pub fn round(x: f64, digits: i32) -> f64 {
    let m = 10f64.powi(digits);
    (x * m).round() / m
}

/// Describes an action as JSON, with sizes in bb and as a fraction of the pot before the action.
pub fn action_json(action: &Action, pot_chips: i32, street_bets: [i32; 2], player: usize) -> Value {
    let to_call = (street_bets[player ^ 1] - street_bets[player]).max(0);
    let sized = |kind: &str, to: i32| {
        // `to` is the player's total commitment on this street; the extra chips put in
        // beyond calling, relative to the pot after calling, is the conventional "% pot".
        let added = to - street_bets[player];
        let pot_after_call = pot_chips + to_call;
        let pct = ((added - to_call) as f64 / pot_after_call as f64 * 100.0).round() as i64;
        json!({ "type": kind, "to_bb": bb(to), "add_bb": bb(added), "pct": pct })
    };
    match *action {
        Action::Fold => json!({ "type": "fold" }),
        Action::Check => json!({ "type": "check" }),
        Action::Call => json!({ "type": "call", "add_bb": bb(to_call) }),
        Action::Bet(to) => sized("bet", to),
        Action::Raise(to) => sized("raise", to),
        Action::AllIn(to) => sized("allin", to),
        _ => json!({ "type": "other" }),
    }
}

pub fn action_label(v: &Value) -> String {
    match v["type"].as_str().unwrap_or("") {
        "fold" => "Fold".into(),
        "check" => "Check".into(),
        "call" => "Call".into(),
        "bet" => format!("Bet {}%", v["pct"]),
        "raise" => format!("Raise {}bb", v["to_bb"]),
        "allin" => "All-in".into(),
        _ => "?".into(),
    }
}


/// Builds an unsolved game for a spot on a board of 3, 4 or 5 cards (e.g. "Ks7d2c" or "Ks7d2cQh").
/// Returns the game and the starting pot in chips. Memory is not allocated yet.
pub fn build_game(spot: &Spot, board: &str) -> Result<(PostFlopGame, i32), String> {
    if board.len() < 6 || board.len() > 10 || board.len() % 2 != 0 {
        return Err(format!("board must be 3 to 5 cards, got {board:?}"));
    }
    let flop = flop_from_str(&board[..6])?;
    let turn = if board.len() >= 8 { card_from_str(&board[6..8])? } else { NOT_DEALT };
    let river = if board.len() == 10 { card_from_str(&board[8..10])? } else { NOT_DEALT };
    let initial_state = match board.len() {
        6 => BoardState::Flop,
        8 => BoardState::Turn,
        _ => BoardState::River,
    };
    let card_config = CardConfig {
        range: [spot.ranges.oop.parse()?, spot.ranges.ip.parse()?],
        flop,
        turn,
        river,
    };

    let starting_pot = (spot.pot_bb * CHIPS_PER_BB).round() as i32;
    let tree_config = TreeConfig {
        initial_state,
        starting_pot,
        effective_stack: (spot.stack_bb * CHIPS_PER_BB).round() as i32,
        rake_rate: spot.rake.rate,
        rake_cap: spot.rake.cap_bb * CHIPS_PER_BB,
        flop_bet_sizes: bet_sizes(&spot.bets.flop)?,
        turn_bet_sizes: bet_sizes(&spot.bets.turn)?,
        river_bet_sizes: bet_sizes(&spot.bets.river)?,
        turn_donk_sizes: donk_sizes(&spot.bets.turn)?,
        river_donk_sizes: donk_sizes(&spot.bets.river)?,
        add_allin_threshold: 1.5,
        force_allin_threshold: 0.15,
        merging_threshold: 0.1,
    };

    let action_tree = ActionTree::new(tree_config)?;
    let game = PostFlopGame::with_config(card_config, action_tree)?;
    Ok((game, starting_pot))
}

/// Range-wide action frequencies of the player to act, weighted by reach.
pub fn range_frequencies(game: &PostFlopGame) -> Vec<f64> {
    let player = game.current_player();
    let n_hands = game.private_cards(player).len();
    let strategy = game.strategy();
    let reach = game.weights(player);
    let n_actions = strategy.len() / n_hands.max(1);
    let total: f64 = reach.iter().map(|&w| w as f64).sum();
    (0..n_actions)
        .map(|a| {
            let f: f64 = (0..n_hands)
                .map(|h| strategy[a * n_hands + h] as f64 * reach[h] as f64)
                .sum();
            if total > 0.0 { round(f / total, 4) } else { 0.0 }
        })
        .collect()
}

/// Describes the current decision node: actions, and for every hand of the player to act
/// `[reach, strategy..., ev..., equity]` (null when the hand doesn't reach this node).
/// The game must be solved and positioned at a decision node.
pub fn decision_node_json(game: &mut PostFlopGame, starting_pot: i32) -> Value {
    game.cache_normalized_weights();

    let player = game.current_player();
    let street_bets = game.total_bet_amount();
    let pot = starting_pot + street_bets[0] + street_bets[1];
    let actions = game.available_actions();
    let actions_json: Vec<Value> = actions
        .iter()
        .map(|a| action_json(a, pot, street_bets, player))
        .collect();

    let n_hands = game.private_cards(player).len();
    let n_actions = actions.len();
    let strategy = game.strategy();
    let ev = game.expected_values_detail(player);
    let reach = game.weights(player).to_vec();
    let combos = game.normalized_weights(player).to_vec();
    let equity = game.equity(player);

    // Range-wide equity and EV for both players, weighted by combos reaching this node.
    let mut range_eq = [0.0; 2];
    let mut range_ev = [0.0; 2];
    for p in 0..2 {
        let w = game.normalized_weights(p);
        range_eq[p] = round(compute_average(&game.equity(p), w) as f64, 4);
        range_ev[p] = round(compute_average(&game.expected_values(p), w) as f64 / CHIPS_PER_BB, 2);
    }

    let mut hands = Vec::with_capacity(n_hands);
    for h in 0..n_hands {
        if combos[h] <= 0.0 || reach[h] <= 1e-4 {
            hands.push(Value::Null);
            continue;
        }
        let mut row = Vec::with_capacity(2 + 2 * n_actions);
        row.push(json!(round(reach[h] as f64, 3)));
        for a in 0..n_actions {
            row.push(json!(round(strategy[a * n_hands + h] as f64, 3)));
        }
        for a in 0..n_actions {
            row.push(json!(round(ev[a * n_hands + h] as f64 / CHIPS_PER_BB, 2)));
        }
        row.push(json!(round(equity[h] as f64, 3)));
        hands.push(Value::Array(row));
    }

    json!({
        "player": if player == 0 { "oop" } else { "ip" },
        "pot_bb": bb(pot),
        "to_call_bb": bb((street_bets[player ^ 1] - street_bets[player]).max(0)),
        "actions": actions_json,
        "equity": range_eq,
        "ev_bb": range_ev,
        "hands": hands,
    })
}
