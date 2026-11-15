//! pt-solver: solves one flop for one spot and exports every flop decision node
//! (strategy + per-action EV for each combo) as JSON for the web viewer/trainer.
//!
//! Usage:
//!   pt-solver --spot spots/btn_vs_bb_srp.json --flop AsKd7c --out web/data/btn_vs_bb_srp/AsKd7c.json
//!             [--iters 300] [--accuracy 0.5] [--threads N]

use postflop_solver::*;
use pt_solver::*;
use serde_json::{json, Value};
use std::time::Instant;

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
    let mut node = decision_node_json(game, starting_pot);
    node["path"] = json!(path.clone());
    let actions_json = node["actions"].as_array().cloned().unwrap_or_default();
    nodes.push(node);

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

    let (mut game, starting_pot) = build_game(&spot, &args.flop)?;
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
