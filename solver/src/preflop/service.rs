//! Runs preflop solves in the background for pt-server: one at a time, with progress, a way to
//! stop early, and the finished solution kept as a pack (the format of `web/data/preflop/`).

use super::cfr::Solver;
use super::equity::EquityTables;
use super::report;
use super::tree::{Config, Tree};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

/// Exploitability is measured this often (it costs about 20 iterations).
const CHECK_EVERY: u32 = 100;

#[derive(Deserialize)]
pub struct SolveRequest {
    #[serde(default)]
    pub config: Config,
    #[serde(default = "default_iters")]
    pub max_iters: u32,
    /// Stop once the average gain from best-responding is below this, in mbb per hand.
    #[serde(default = "default_target")]
    pub target_mbb: f64,
}

fn default_iters() -> u32 {
    1000
}

fn default_target() -> f64 {
    0.25
}

#[derive(Serialize, Clone, Default)]
pub struct Status {
    /// "idle", "tables" (building the equity tables, first run only), "solving", "done",
    /// "stopped" or "failed".
    pub state: &'static str,
    pub config: Option<Config>,
    pub iteration: u32,
    pub max_iters: u32,
    pub target_mbb: f64,
    pub exploitability_mbb: Option<f64>,
    /// (iteration, exploitability) at each measurement.
    pub history: Vec<(u32, f64)>,
    pub tables_progress: f64,
    pub decisions: usize,
    pub seconds: f64,
    pub error: Option<String>,
}

pub struct Service {
    cache: PathBuf,
    tables: Arc<Mutex<Option<Arc<EquityTables>>>>,
    status: Arc<Mutex<Status>>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// The last finished solve, as pack JSON.
    solution: Arc<Mutex<Option<Arc<String>>>>,
}

type ApiResult = Result<Value, (u16, String)>;

impl Service {
    pub fn new(cache: PathBuf) -> Service {
        Service {
            cache,
            tables: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(Status { state: "idle", ..Default::default() })),
            cancel: Arc::new(AtomicBool::new(false)),
            worker: None,
            solution: Arc::new(Mutex::new(None)),
        }
    }

    fn stop_worker(&mut self) {
        if let Some(handle) = self.worker.take() {
            self.cancel.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
        self.cancel.store(false, Ordering::SeqCst);
    }

    pub fn start(&mut self, body: &str) -> ApiResult {
        let req: SolveRequest = serde_json::from_str(body).map_err(|e| (400, e.to_string()))?;
        if !(1..=20_000).contains(&req.max_iters) {
            return Err((400, "max_iters must be 1 to 20000.".into()));
        }
        let tree = Tree::build(&req.config).map_err(|e| (400, e))?;
        self.stop_worker();
        *self.status.lock().unwrap() = Status {
            state: "tables",
            config: Some(req.config.clone()),
            max_iters: req.max_iters,
            target_mbb: req.target_mbb,
            decisions: tree.decisions(),
            ..Default::default()
        };
        let (status, cancel, tables, solution, cache) =
            (self.status.clone(), self.cancel.clone(), self.tables.clone(), self.solution.clone(), self.cache.clone());
        self.worker = Some(std::thread::spawn(move || {
            let t0 = Instant::now();
            let loaded = tables.lock().unwrap().clone();
            let tables = match loaded {
                Some(t) => t,
                None => {
                    let st = status.clone();
                    let t = Arc::new(EquityTables::load_or_compute(Some(&cache), &move |p| st.lock().unwrap().tables_progress = p));
                    *tables.lock().unwrap() = Some(t.clone());
                    t
                }
            };
            status.lock().unwrap().state = "solving";
            let mut solver = Solver::new(tree, tables);
            let e = solver.run(req.max_iters, req.target_mbb, CHECK_EVERY, &cancel, &mut |it, e| {
                let mut s = status.lock().unwrap();
                s.iteration = it;
                s.seconds = t0.elapsed().as_secs_f64();
                if let Some(e) = e {
                    s.exploitability_mbb = Some(e.mbb());
                    s.history.push((it, e.mbb()));
                }
            });
            let pack = report::pack_json(&solver, &e, 0.0);
            let text = serde_json::to_string(&pack).unwrap_or_default();
            *solution.lock().unwrap() = Some(Arc::new(text));
            let mut s = status.lock().unwrap();
            s.state = if cancel.load(Ordering::SeqCst) { "stopped" } else { "done" };
            s.iteration = solver.iteration;
            s.exploitability_mbb = Some(e.mbb());
            s.seconds = t0.elapsed().as_secs_f64();
        }));
        Ok(json!({ "ok": true }))
    }

    pub fn status(&self) -> Value {
        let mut v = serde_json::to_value(self.status.lock().unwrap().clone()).unwrap_or_default();
        v["has_solution"] = json!(self.solution.lock().unwrap().is_some());
        v
    }

    pub fn stop(&self) -> ApiResult {
        self.cancel.store(true, Ordering::SeqCst);
        Ok(json!({ "ok": true }))
    }

    /// The last finished solve, as pack JSON text.
    pub fn solution(&self) -> Result<Arc<String>, (u16, String)> {
        self.solution.lock().unwrap().clone().ok_or((404, "No preflop solve has finished yet.".into()))
    }
}
