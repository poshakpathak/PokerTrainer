//! pt-preflop: solves 6-max preflop for one set of sizes, stacks and rake, and writes the
//! solution for the web app.
//!
//! Usage:
//!   pt-preflop [--config preflop.json] [--set key=value ...] [--iters 1000] [--target 0.5]
//!              [--check-every 50] [--out web/data/preflop/100bb.json] [--min-line 0.00005]
//!              [--threads N] [--summary]
//!
//! `--set` changes one config field, e.g. `--set stack_bb=50 --set rake.rate=0.05`.

use pt_solver::preflop::cfr::Solver;
use pt_solver::preflop::equity::{default_cache_path, EquityTables};
use pt_solver::preflop::report;
use pt_solver::preflop::tree::{Config, Tree};
use serde_json::Value;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

struct Args {
    config: Value,
    iters: u32,
    target: f64,
    check_every: u32,
    out: Option<String>,
    min_line: f64,
    threads: Option<usize>,
    summary: bool,
}

fn set(config: &mut Value, key: &str, raw: &str) -> Result<(), String> {
    let value: Value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
    let mut at = config;
    let parts: Vec<&str> = key.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        let obj = at.as_object_mut().ok_or(format!("--set {key}: not an object"))?;
        if i + 1 == parts.len() {
            obj.insert(part.to_string(), value);
            return Ok(());
        }
        at = obj.entry(part.to_string()).or_insert_with(|| Value::Object(Default::default()));
    }
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        config: serde_json::to_value(Config::default()).unwrap(),
        iters: 1000,
        target: 0.5,
        check_every: 50,
        out: None,
        min_line: 0.00005,
        threads: None,
        summary: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or(format!("missing value for {a}"));
        match a.as_str() {
            "--config" => {
                let path = val()?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                let given: Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
                for (k, v) in given.as_object().ok_or(format!("{path}: not an object"))? {
                    args.config[k] = v.clone();
                }
            }
            "--set" => {
                let kv = val()?;
                let (k, v) = kv.split_once('=').ok_or(format!("--set {kv}: expected key=value"))?;
                set(&mut args.config, k, v)?;
            }
            "--iters" => args.iters = val()?.parse().map_err(|e| format!("--iters: {e}"))?,
            "--target" => args.target = val()?.parse().map_err(|e| format!("--target: {e}"))?,
            "--check-every" => args.check_every = val()?.parse().map_err(|e| format!("--check-every: {e}"))?,
            "--out" => args.out = Some(val()?),
            "--min-line" => args.min_line = val()?.parse().map_err(|e| format!("--min-line: {e}"))?,
            "--threads" => args.threads = Some(val()?.parse().map_err(|e| format!("--threads: {e}"))?),
            "--summary" => args.summary = true,
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    Ok(args)
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    if let Some(t) = args.threads {
        rayon::ThreadPoolBuilder::new().num_threads(t).build_global().map_err(|e| e.to_string())?;
    }
    let config: Config = serde_json::from_value(args.config.clone()).map_err(|e| format!("config: {e}"))?;
    let tree = Tree::build(&config)?;
    eprintln!("tree: {} decisions, {} flops and all-ins", tree.decisions(), tree.showdowns.len());

    let t0 = Instant::now();
    let tables = EquityTables::load_or_compute(Some(&default_cache_path()), &|done| eprint!("\requity tables {:.0}%", done * 100.0));
    eprintln!("\requity tables ready in {:.1}s", t0.elapsed().as_secs_f64());

    let mut solver = Solver::new(tree, Arc::new(tables));
    let t0 = Instant::now();
    let e = solver.run(args.iters, args.target, args.check_every, &AtomicBool::new(false), &mut |it, e| {
        if let Some(e) = e {
            eprintln!("iteration {it}: exploitability {:.2} mbb/hand ({:.0}s)", e.mbb(), t0.elapsed().as_secs_f64());
        }
    });
    eprintln!(
        "solved: {} iterations in {:.0}s, exploitability {:.2} mbb/hand, EV by seat {:?}",
        solver.iteration,
        t0.elapsed().as_secs_f64(),
        e.mbb(),
        e.ev.map(|x| (x * 1000.0).round() / 1000.0)
    );
    if args.summary {
        print!("{}", report::summary(&solver));
    }
    if let Some(out) = &args.out {
        let pack = report::pack_json(&solver, &e, args.min_line);
        if let Some(dir) = std::path::Path::new(out).parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string(&pack).map_err(|e| e.to_string())?;
        std::fs::write(out, &text).map_err(|e| format!("{out}: {e}"))?;
        eprintln!("wrote {out} ({:.1} MB)", text.len() as f64 / 1e6);
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
