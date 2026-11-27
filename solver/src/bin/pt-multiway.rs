//! pt-multiway: solves a pot with two or three players on one board from the command line, and
//! prints the tree size, the progress and each player's EV.
//!
//! Usage:
//!   pt-multiway --spot ../web/data/multiway/presets.json --preset co_btn_bb_srp --board Ks7d2c
//!               [--iters 1000] [--accuracy 0.5] [--check-every 10] [--threads N] [--count]
//!
//! `--spot` is one spot, or a file with a list of them under "spots" (the presets file), from
//! which `--preset` picks one by id (the first by default). `--count` only prints the tree size
//! and the memory the solve would need.

use pt_solver::multiway::cfr;
use pt_solver::multiway::game::Setup;
use pt_solver::multiway::spot::MultiSpot;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

struct Args {
    spot: String,
    preset: Option<String>,
    board: String,
    iters: u32,
    accuracy: f64,
    check_every: u32,
    threads: Option<usize>,
    count: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { spot: String::new(), preset: None, board: String::new(), iters: 1000, accuracy: 0.5, check_every: 10, threads: None, count: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or(format!("missing value for {arg}"));
        match arg.as_str() {
            "--spot" => a.spot = val()?,
            "--preset" => a.preset = Some(val()?),
            "--board" => a.board = val()?,
            "--iters" => a.iters = val()?.parse().map_err(|e| format!("--iters: {e}"))?,
            "--accuracy" => a.accuracy = val()?.parse().map_err(|e| format!("--accuracy: {e}"))?,
            "--check-every" => a.check_every = val()?.parse().map_err(|e| format!("--check-every: {e}"))?,
            "--threads" => a.threads = Some(val()?.parse().map_err(|e| format!("--threads: {e}"))?),
            "--count" => a.count = true,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if a.spot.is_empty() || a.board.is_empty() {
        return Err("--spot and --board are required".into());
    }
    Ok(a)
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    if let Some(t) = args.threads {
        rayon::ThreadPoolBuilder::new().num_threads(t).build_global().map_err(|e| e.to_string())?;
    }
    let text = std::fs::read_to_string(&args.spot).map_err(|e| format!("{}: {e}", args.spot))?;
    let mut value: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", args.spot))?;
    if let Some(list) = value.get_mut("spots").and_then(|v| v.as_array_mut()) {
        let i = match &args.preset {
            Some(id) => list.iter().position(|s| s["id"] == id.as_str()).ok_or(format!("no preset {id:?} in {}", args.spot))?,
            None => 0,
        };
        value = list.swap_remove(i);
    }
    let spot: MultiSpot = serde_json::from_value(value).map_err(|e| format!("{}: {e}", args.spot))?;
    let setup = Setup::new(&spot, &args.board)?;
    let c = setup.counts;
    eprintln!(
        "{} on {}: hands {:?}, {} nodes ({} decisions, {} terminals), memory {:.2} GB",
        if spot.name.is_empty() { &spot.id } else { &spot.name },
        args.board,
        setup.hand_counts(),
        c.nodes,
        c.decisions,
        c.terminals,
        setup.memory_bytes() as f64 / 1e9
    );
    if args.count {
        return Ok(());
    }
    let t0 = Instant::now();
    let mut game = setup.allocate();
    eprintln!("built in {:.1}s", t0.elapsed().as_secs_f64());
    let t1 = Instant::now();
    let cancel = AtomicBool::new(false);
    let e = cfr::solve(&mut game, args.iters, args.accuracy, args.check_every, &cancel, |it, pct| {
        if let Some(pct) = pct {
            eprintln!("iteration {it}: exploitability {pct:.3}% of the pot, {:.1}s", t1.elapsed().as_secs_f64());
        }
    });
    let secs = t1.elapsed().as_secs_f64();
    eprintln!(
        "solved in {secs:.1}s ({} iterations, {:.2}s each), exploitability {:.3}% of the pot",
        game.iteration,
        secs / game.iteration.max(1) as f64,
        e.pct_of_pot(game.cfg.starting_pot)
    );
    for (p, name) in spot.players.iter().enumerate() {
        eprintln!("  {name}: EV {:.3}bb", e.value[p] / pt_solver::CHIPS_PER_BB);
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
