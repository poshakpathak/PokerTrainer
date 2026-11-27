//! Runs multiway solves in the background for pt-server, one at a time: progress, a way to stop
//! early, and the finished game kept in memory to browse.

use super::cfr;
use super::game::{Game, Setup};
use super::spot::MultiSpot;
use super::view;
use crate::{round, CHIPS_PER_BB};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

/// Exploitability is measured this often; it costs about one iteration.
const CHECK_EVERY: u32 = 10;

#[derive(Deserialize)]
pub struct SolveRequest {
    pub spot: MultiSpot,
    pub board: String,
    /// Stop once the average gain from best-responding is below this, in % of the starting pot.
    #[serde(default = "default_accuracy")]
    pub accuracy: f64,
    #[serde(default = "default_iters")]
    pub max_iters: u32,
}

fn default_accuracy() -> f64 {
    0.5
}

fn default_iters() -> u32 {
    300
}

#[derive(Deserialize)]
pub struct NodeRequest {
    #[serde(default)]
    pub history: Vec<usize>,
}

#[derive(Serialize, Clone, Default)]
pub struct Status {
    /// "idle", "building", "solving", "done", "stopped" or "failed".
    pub state: &'static str,
    pub spot: Option<MultiSpot>,
    pub board: String,
    pub iteration: u32,
    pub max_iters: u32,
    pub target_pct: f64,
    pub exploitability_pct: Option<f64>,
    /// (iteration, exploitability %) at each measurement.
    pub history: Vec<(u32, f64)>,
    pub seconds: f64,
    pub memory_gb: f64,
    pub hands: Vec<usize>,
    pub nodes: u64,
    /// Each player's EV at the start, in bb, once solved.
    pub root_ev_bb: Vec<f64>,
    pub error: Option<String>,
}

pub struct Service {
    status: Arc<Mutex<Status>>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// The last solve, finalized when it ends.
    game: Arc<Mutex<Option<Game>>>,
}

type ApiResult = Result<Value, (u16, String)>;

fn bad(msg: impl Into<String>) -> (u16, String) {
    (400, msg.into())
}

impl Default for Service {
    fn default() -> Self {
        Service::new()
    }
}

impl Service {
    pub fn new() -> Service {
        Service {
            status: Arc::new(Mutex::new(Status { state: "idle", ..Default::default() })),
            cancel: Arc::new(AtomicBool::new(false)),
            worker: None,
            game: Arc::new(Mutex::new(None)),
        }
    }

    fn stop_worker(&mut self) {
        if let Some(handle) = self.worker.take() {
            self.cancel.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
        self.cancel.store(false, Ordering::SeqCst);
    }

    /// Stops any solve and frees the kept one.
    pub fn clear(&mut self) {
        self.stop_worker();
        *self.game.lock().unwrap() = None;
        *self.status.lock().unwrap() = Status { state: "idle", ..Default::default() };
    }

    /// Memory the kept solve holds, in GB.
    pub fn memory_gb(&self) -> f64 {
        let s = self.status.lock().unwrap();
        if matches!(s.state, "building" | "solving" | "done" | "stopped") {
            s.memory_gb
        } else {
            0.0
        }
    }

    /// Starts a solve, replacing the last one. `memory_gb` is what it may use (the last multiway
    /// solve's memory is freed first, so it doesn't count against this).
    pub fn start(&mut self, body: &str, memory_gb: f64) -> ApiResult {
        let req: SolveRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        if (req.accuracy.is_nan() || req.accuracy <= 0.0) && req.max_iters == 0 {
            return Err(bad("Give an accuracy above 0 or a number of iterations."));
        }
        let setup = Setup::new(&req.spot, &req.board).map_err(bad)?;
        let need = setup.memory_bytes() as f64 / 1e9;
        if need > memory_gb {
            return Err(bad(format!(
                "This tree needs {need:.1} GB of memory, over the {memory_gb:.1} GB free under the limit. \
                 Use fewer bet sizes or no raises on the flop, narrower ranges, start from the turn, or \
                 start pt-server with a higher --max-memory-gb."
            )));
        }
        // The last solve's memory is freed before this one is built.
        self.stop_worker();
        *self.game.lock().unwrap() = None;
        let counts = setup.counts;
        let hands = setup.hand_counts();
        *self.status.lock().unwrap() = Status {
            state: "building",
            spot: Some(req.spot.clone()),
            board: setup.board_str.clone(),
            max_iters: req.max_iters.max(1),
            target_pct: req.accuracy,
            memory_gb: round(need, 2),
            hands: hands.clone(),
            nodes: counts.nodes,
            ..Default::default()
        };

        let status = self.status.clone();
        let cancel = self.cancel.clone();
        let slot = self.game.clone();
        let max_iters = req.max_iters.max(1);
        let target = req.accuracy;
        self.worker = Some(std::thread::spawn(move || {
            let t0 = Instant::now();
            let mut game = setup.allocate();
            status.lock().unwrap().state = "solving";
            let e = cfr::solve(&mut game, max_iters, target, CHECK_EVERY, &cancel, |it, pct| {
                let mut s = status.lock().unwrap();
                s.iteration = it;
                s.seconds = round(t0.elapsed().as_secs_f64(), 1);
                if let Some(p) = pct {
                    s.exploitability_pct = Some(round(p, 3));
                    s.history.push((it, round(p, 3)));
                }
            });
            let stopped = cancel.load(Ordering::SeqCst);
            {
                let mut s = status.lock().unwrap();
                s.state = if stopped { "stopped" } else { "done" };
                s.iteration = game.iteration;
                s.exploitability_pct = Some(round(e.pct_of_pot(game.cfg.starting_pot), 3));
                s.root_ev_bb = e.value.iter().map(|v| round(v / CHIPS_PER_BB, 3)).collect();
                s.seconds = round(t0.elapsed().as_secs_f64(), 1);
            }
            *slot.lock().unwrap() = Some(game);
        }));
        Ok(json!({ "ok": true, "memory_gb": round(need, 2), "hands": hands, "nodes": counts.nodes }))
    }

    pub fn status(&self) -> Value {
        json!(*self.status.lock().unwrap())
    }

    /// Stops early; what's been solved so far can be browsed once it has wrapped up.
    pub fn stop(&self) -> ApiResult {
        self.cancel.store(true, Ordering::SeqCst);
        Ok(json!({ "ok": true }))
    }

    /// Checks a spot without solving: its size and the memory it needs.
    pub fn preview(&self, body: &str) -> ApiResult {
        let req: SolveRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let setup = Setup::new(&req.spot, &req.board).map_err(bad)?;
        let c = setup.counts;
        Ok(json!({
            "hands": setup.hand_counts(),
            "nodes": c.nodes,
            "decisions": c.decisions,
            "memory_gb": round(setup.memory_bytes() as f64 / 1e9, 2),
        }))
    }

    /// The node after `history` in the last solve.
    pub fn node(&self, body: &str) -> ApiResult {
        let req: NodeRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let guard = self.game.lock().unwrap();
        let Some(game) = guard.as_ref() else {
            return Err((409, "No multiway solve has finished yet.".into()));
        };
        let pos = view::navigate(game, &req.history).map_err(bad)?;
        let mut v = view::node_json(game, &pos).map_err(bad)?;
        v["names"] = json!(game.spot.players);
        Ok(v)
    }
}
