//! pt-solver: solves one flop for one spot and exports every flop decision node
//! (strategy + per-action EV for each combo) as JSON for the web viewer/trainer.
//!
//! Usage:
//!   pt-solver --spot spots/btn_vs_bb_srp.json --flop AsKd7c --out web/data/btn_vs_bb_srp/AsKd7c.json
//!             [--iters 300] [--accuracy 0.5] [--threads N]

use postflop_solver::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

/// Chip units per big blind. The solver works in integers, so we use 1/20 bb.
const CHIPS_PER_BB: f64 = 20.0;

#[derive(Deserialize)]
struct StreetBets {
    oop: String,
    ip: String,
    raise: String,
    #[serde(default)]
    donk: Option<String>,
}

#[derive(Deserialize)]
struct Bets {
    flop: StreetBets,
    turn: StreetBets,
    river: StreetBets,
}

#[derive(Deserialize)]
struct Ranges {
    oop: String,
    ip: String,
}

#[derive(Deserialize, Default)]
struct Rake {
    #[serde(default)]
    rate: f64,
    #[serde(default)]
    cap_bb: f64,
}

#[derive(Deserialize)]
struct Spot {
    id: String,
    pot_bb: f64,
    stack_bb: f64,
    ranges: Ranges,
    bets: Bets,
    #[serde(default)]
    rake: Rake,
}

struct Args {
    spot: String,
    flop: String,
    out: String,
    iters: u32,
    accuracy: f32,
    threads: Option<usize>,
}

fn parse_args() -> Result<Args, String> {
    let mut spot = None;
    let mut flop = None;
    let mut out = None;
    let mut iters = 300;
    let mut accuracy = 0.5;
    let mut threads = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or(format!("missing value for {a}"));
        match a.as_str() {
            "--spot" => spot = Some(val()?),
            "--flop" => flop = Some(val()?),
            "--out" => out = Some(val()?),
            "--iters" => iters = val()?.parse().map_err(|e| format!("--iters: {e}"))?,
            "--accuracy" => accuracy = val()?.parse().map_err(|e| format!("--accuracy: {e}"))?,
            "--threads" => threads = Some(val()?.parse().map_err(|e| format!("--threads: {e}"))?),
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    Ok(Args {
        spot: spot.ok_or("--spot is required")?,
        flop: flop.ok_or("--flop is required")?,
        out: out.ok_or("--out is required")?,
        iters,
        accuracy,
        threads,
    })
}

fn bet_sizes(s: &StreetBets) -> Result<[BetSizeOptions; 2], String> {
    Ok([
        BetSizeOptions::try_from((s.oop.as_str(), s.raise.as_str()))?,
        BetSizeOptions::try_from((s.ip.as_str(), s.raise.as_str()))?,
    ])
}

fn donk_sizes(s: &StreetBets) -> Result<Option<DonkSizeOptions>, String> {
    s.donk
        .as_deref()
        .map(DonkSizeOptions::try_from)
        .transpose()
}

fn bb(chips: i32) -> f64 {
    round(chips as f64 / CHIPS_PER_BB, 2)
}

fn round(x: f64, digits: i32) -> f64 {
    let m = 10f64.powi(digits);
    (x * m).round() / m
}

/// Describes an action as JSON, with sizes in bb and as a fraction of the pot before the action.
fn action_json(action: &Action, pot_chips: i32, street_bets: [i32; 2], player: usize) -> Value {
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

fn action_label(v: &Value) -> String {
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

/// Walks every decision node on the flop (stops at the turn card or a terminal node).
fn export_nodes(
    game: &mut PostFlopGame,
    path: &mut Vec<String>,
    history: &mut Vec<usize>,
    starting_pot: i32,
    nodes: &mut Vec<Value>,
) {
    if game.is_terminal_node() || game.is_chance_node() {
        return;
    }
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

    // Per hand: [reach, s_0..s_n, ev_0..ev_n]; null when the hand never reaches this node.
    let mut hands = Vec::with_capacity(n_hands);
    for h in 0..n_hands {
        if combos[h] <= 0.0 || reach[h] <= 1e-4 {
            hands.push(Value::Null);
            continue;
        }
        let mut row = Vec::with_capacity(1 + 2 * n_actions);
        row.push(json!(round(reach[h] as f64, 3)));
        for a in 0..n_actions {
            row.push(json!(round(strategy[a * n_hands + h] as f64, 3)));
        }
        for a in 0..n_actions {
            row.push(json!(round(ev[a * n_hands + h] as f64 / CHIPS_PER_BB, 2)));
        }
        hands.push(Value::Array(row));
    }

    nodes.push(json!({
        "path": path.clone(),
        "player": if player == 0 { "oop" } else { "ip" },
        "pot_bb": bb(pot),
        "to_call_bb": bb((street_bets[player ^ 1] - street_bets[player]).max(0)),
        "actions": actions_json,
        "hands": hands,
    }));

    for (i, a) in actions_json.iter().enumerate() {
        path.push(action_label(a));
        history.push(i);
        game.play(i);
        export_nodes(game, path, history, starting_pot, nodes);
        history.pop();
        path.pop();
        game.apply_history(history);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    if let Some(t) = args.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build_global()
            .map_err(|e| e.to_string())?;
    }

    let spot: Spot = serde_json::from_str(
        &std::fs::read_to_string(&args.spot).map_err(|e| format!("{}: {e}", args.spot))?,
    )
    .map_err(|e| format!("{}: {e}", args.spot))?;

    let card_config = CardConfig {
        range: [spot.ranges.oop.parse()?, spot.ranges.ip.parse()?],
        flop: flop_from_str(&args.flop)?,
        turn: NOT_DEALT,
        river: NOT_DEALT,
    };

    let starting_pot = (spot.pot_bb * CHIPS_PER_BB).round() as i32;
    let tree_config = TreeConfig {
        initial_state: BoardState::Flop,
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
    let mut game = PostFlopGame::with_config(card_config, action_tree)?;
    let (_, mem_compressed) = game.memory_usage();
    eprintln!(
        "[{} {}] memory {:.2} GB (compressed)",
        spot.id,
        args.flop,
        mem_compressed as f64 / 1e9
    );
    game.allocate_memory(true);

    let t0 = Instant::now();
    let target = starting_pot as f32 * args.accuracy / 100.0;
    let exploitability = solve(&mut game, args.iters, target, false);
    let secs = t0.elapsed().as_secs_f64();
    let expl_pct = exploitability as f64 / starting_pot as f64 * 100.0;
    eprintln!(
        "[{} {}] solved in {:.0}s, exploitability {:.2}% pot",
        spot.id, args.flop, secs, expl_pct
    );

    let hands = [
        holes_to_strings(game.private_cards(0))?,
        holes_to_strings(game.private_cards(1))?,
    ];
    let mut nodes = Vec::new();
    game.back_to_root();
    export_nodes(&mut game, &mut Vec::new(), &mut Vec::new(), starting_pot, &mut nodes);

    let out = json!({
        "spot": spot.id,
        "flop": args.flop,
        "pot_bb": spot.pot_bb,
        "stack_bb": spot.stack_bb,
        "exploitability_pct": round(expl_pct, 3),
        "solve_seconds": round(secs, 1),
        "hands": { "oop": hands[0], "ip": hands[1] },
        "nodes": nodes,
    });

    if let Some(dir) = std::path::Path::new(&args.out).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&args.out, serde_json::to_string(&out).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", args.out))?;
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
