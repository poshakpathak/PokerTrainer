//! pt-server: a local solver server for the PokerTrainer web app.
//!
//! It serves the web app and a small JSON API, so you can solve any spot on demand, browse the
//! whole game tree (flop, turn and river) in the browser, and lock how one player plays at any
//! decision, or at every decision as a player type, to find the best way to exploit them.
//!
//! Usage:
//!   pt-server [--web ../web] [--port 8080] [--max-memory-gb 8] [--threads N]
//!
//! API (all JSON):
//!   POST /api/solve         {spot, board, accuracy, max_iters, locks}  start a solve; `locks: true`
//!                           applies the saved locks
//!   GET  /api/status        progress of the current solve, the finished solutions and the saved locks
//!   POST /api/stop          stop early and keep the current strategy
//!   POST /api/node          {history, solution}  describe the node after `history`
//!   POST /api/lock/preview  {history, solution, targets, hands, order}  the strategy a lock would set
//!   POST /api/locks/add     {history, solution, targets, hands, order, wild, keep_path, label, crumbs}
//!   POST /api/locks/remove  {id}
//!   POST /api/locks/clear   removes the locks and the player type
//!   POST /api/profile       {player, profile}  play one player as a player type everywhere
//!   POST /api/profile/clear
//!   POST /api/preflop/solve     {config, max_iters, target_mbb}  start a 6-max preflop solve
//!   GET  /api/preflop/status    its progress
//!   POST /api/preflop/stop      stop early and keep the current strategy
//!   GET  /api/preflop/solution  the finished solve, in the format of web/data/preflop/*.json
//!
//! `history` lists action indexes for decision nodes and card ids (0-51) for chance nodes,
//! the same as postflop-solver's `apply_history`. `solution` is "gto" (the last solve without
//! locks) or "exploit" (the last solve with them). The two are kept side by side while they
//! fit under --max-memory-gb, so every node can be compared.

use postflop_solver::*;
use pt_solver::locks::{self, LockedNode, Recipe};
use pt_solver::preflop::{equity::default_cache_path, service::Service as PreflopService};
use pt_solver::profile::{self, Profile};
use pt_solver::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;
use tiny_http::{Header, Method, Request, Response, Server};

/// How often (in iterations) the solver measures exploitability; it costs about one iteration.
const CHECK_EVERY: u32 = 10;

/// The most decisions the saved locks can cover together. A river lock on every turn and river
/// card of a flop solve covers about 2,400.
const MAX_LOCKED_NODES: usize = 10_000;

/// Decisions a player type locks between letting other requests use the GTO solution.
const PROFILE_CHUNK: usize = 400;

/// The memory a player type's locks take, as a share of the game's. They hold one player's
/// strategy at every decision as 32-bit floats; the game stores both players' strategies and
/// regrets in 16 bits. Measured at 0.33 to 0.52 on the bundled spots.
const PROFILE_MEMORY: f64 = 0.55;

#[derive(Deserialize)]
struct SolveRequest {
    spot: Spot,
    board: String,
    #[serde(default = "default_accuracy")]
    accuracy: f32,
    #[serde(default = "default_iters")]
    max_iters: u32,
    /// Apply the saved locks.
    #[serde(default)]
    locks: bool,
}

fn default_accuracy() -> f32 {
    0.5
}

fn default_iters() -> u32 {
    1000
}

#[derive(Deserialize)]
struct NodeRequest {
    #[serde(default)]
    history: Vec<usize>,
    #[serde(default)]
    solution: Option<String>,
}

#[derive(Deserialize)]
struct PreviewRequest {
    #[serde(default)]
    history: Vec<usize>,
    #[serde(default)]
    solution: Option<String>,
    #[serde(flatten)]
    recipe: Recipe,
}

#[derive(Deserialize)]
struct AddLockRequest {
    #[serde(default)]
    history: Vec<usize>,
    #[serde(default)]
    solution: Option<String>,
    #[serde(flatten)]
    recipe: Recipe,
    /// Indexes of `history` entries that are cards and should mean every card.
    #[serde(default)]
    wild: Vec<usize>,
    /// Also lock this player's earlier decisions on the line as they are now.
    #[serde(default)]
    keep_path: bool,
    #[serde(default)]
    label: String,
    #[serde(default)]
    crumbs: Vec<String>,
}

#[derive(Deserialize)]
struct RemoveLockRequest {
    id: u64,
}

#[derive(Deserialize)]
struct ProfileRequest {
    /// "oop" or "ip".
    player: String,
    profile: Profile,
}

fn player_name(p: usize) -> &'static str {
    if p == 0 {
        "oop"
    } else {
        "ip"
    }
}

/// Which solution: the last solve without locks, or the last one with them.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Kind {
    #[default]
    Gto,
    Exploit,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Gto => "gto",
            Kind::Exploit => "exploit",
        }
    }

    fn other(self) -> Kind {
        match self {
            Kind::Gto => Kind::Exploit,
            Kind::Exploit => Kind::Gto,
        }
    }

    fn parse(s: &str) -> Result<Kind, String> {
        match s {
            "gto" => Ok(Kind::Gto),
            "exploit" => Ok(Kind::Exploit),
            _ => Err(format!("solution must be \"gto\" or \"exploit\", not {s:?}")),
        }
    }
}

#[derive(Clone, Default)]
struct Status {
    state: &'static str,
    kind: Kind,
    iteration: u32,
    max_iters: u32,
    exploitability_pct: Option<f64>,
    target_pct: f32,
    elapsed_s: f64,
    memory_gb: f64,
    error: Option<String>,
    note: Option<String>,
    board: String,
    spot: Option<Value>,
    /// "profile" while a player type's locks are being set up, before the solve starts.
    phase: Option<&'static str>,
    phase_progress: f64,
}

impl Status {
    fn to_json(&self) -> Value {
        json!({
            "state": self.state,
            "kind": self.kind.name(),
            "iteration": self.iteration,
            "max_iters": self.max_iters,
            "exploitability_pct": self.exploitability_pct.map(|e| round(e, 3)),
            "target_pct": round(self.target_pct as f64, 3),
            "elapsed_s": round(self.elapsed_s, 1),
            "memory_gb": round(self.memory_gb, 2),
            "error": self.error,
            "note": self.note,
            "board": self.board,
            "spot": self.spot,
            "phase": self.phase,
            "phase_progress": round(self.phase_progress, 3),
        })
    }
}

struct Solved {
    game: PostFlopGame,
    starting_pot: i32,
    /// The board as the user typed it; dealt cards are appended in order.
    board: String,
    /// The tree it was solved on (see `tree_key`).
    key: String,
    memory_gb: f64,
    /// Each player's EV at the start of the hand, in bb.
    root_ev: [f64; 2],
    exploitability_pct: f64,
    iterations: u32,
    /// For an exploit solve, the version of the saved locks it used.
    locks_version: u64,
    /// For an exploit solve with a player type: who played it, which one, and how many
    /// decisions it locked.
    profile: Value,
}

impl Solved {
    fn summary(&self) -> Value {
        json!({
            "root_ev_bb": self.root_ev,
            "exploitability_pct": round(self.exploitability_pct, 3),
            "iterations": self.iterations,
            "memory_gb": round(self.memory_gb, 2),
            "locks_version": self.locks_version,
            "profile": self.profile,
        })
    }
}

#[derive(Default)]
struct Solutions {
    gto: Option<Solved>,
    exploit: Option<Solved>,
}

impl Solutions {
    fn slot(&mut self, kind: Kind) -> &mut Option<Solved> {
        match kind {
            Kind::Gto => &mut self.gto,
            Kind::Exploit => &mut self.exploit,
        }
    }

    fn get(&self, kind: Kind) -> Option<&Solved> {
        match kind {
            Kind::Gto => self.gto.as_ref(),
            Kind::Exploit => self.exploit.as_ref(),
        }
    }

    /// The solution of `kind`, and the other one if it was solved on the same tree.
    fn with_other(&mut self, kind: Kind) -> (Option<&mut Solved>, Option<&mut Solved>) {
        let (this, other) = match kind {
            Kind::Gto => (&mut self.gto, &mut self.exploit),
            Kind::Exploit => (&mut self.exploit, &mut self.gto),
        };
        let same_tree = matches!((&*this, &*other), (Some(a), Some(b)) if a.key == b.key);
        (this.as_mut(), if same_tree { other.as_mut() } else { None })
    }

    /// The requested solution, or by default the one solved last (`latest`) if it's there.
    fn pick(&self, requested: Option<&str>, latest: Kind) -> Result<Kind, (u16, String)> {
        match requested {
            Some(s) => {
                let kind = Kind::parse(s).map_err(bad)?;
                if self.get(kind).is_none() {
                    let what = if kind == Kind::Gto { "solve without locks" } else { "solve with locks" };
                    return Err((409, format!("There's no finished {what}.")));
                }
                Ok(kind)
            }
            None => [latest, latest.other()]
                .into_iter()
                .find(|&k| self.get(k).is_some())
                .ok_or((409, "No finished solve yet.".to_string())),
        }
    }
}

/// A saved lock: how one player plays at a decision (or at that decision on every card), built
/// from a solution and applied the next time the spot is solved with locks.
struct Lock {
    id: u64,
    label: String,
    crumbs: Vec<String>,
    player: usize,
    history: Vec<usize>,
    /// Who acted at each step of `history` (`None` for a card).
    actors: Vec<Option<usize>>,
    wild: Vec<usize>,
    keep_path: bool,
    recipe: Recipe,
    actions: Vec<Value>,
    before: Vec<f64>,
    after: Vec<f64>,
    warnings: Vec<String>,
    from: Kind,
    nodes: Vec<LockedNode>,
    /// The player's earlier decisions on the line, locked as they were.
    path: Vec<LockedNode>,
}

impl Lock {
    /// Whether two locks cover the same decisions.
    fn same_target(&self, other: &Lock) -> bool {
        self.wild == other.wild
            && self.history.len() == other.history.len()
            && self.history.iter().zip(&other.history).enumerate().all(|(i, (a, b))| a == b || self.wild.contains(&i))
    }

    fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "crumbs": self.crumbs,
            "player": if self.player == 0 { "oop" } else { "ip" },
            "history": self.history,
            "actors": actors_json(&self.actors),
            "wild": self.wild,
            "keep_path": self.keep_path,
            "targets": self.recipe.targets,
            "hands": self.recipe.hands,
            "order": self.recipe.order,
            "actions": self.actions,
            "before": locks::rounded(&self.before),
            "after": locks::rounded(&self.after),
            "warnings": self.warnings,
            "from": self.from.name(),
            "nodes": self.nodes.len(),
            "path_nodes": self.path.len(),
        })
    }
}

/// A player type one player plays at every decision.
#[derive(Clone)]
struct ProfileLock {
    player: usize,
    profile: Profile,
}

impl ProfileLock {
    fn json(&self) -> Value {
        json!({
            "player": player_name(self.player),
            "id": self.profile.id,
            "name": self.profile.name,
            "strength": self.profile.strength,
            "rules": self.profile.rules,
        })
    }
}

/// The saved locks and player type. They belong to one tree (`key`); solving a different spot
/// clears them.
#[derive(Default)]
struct LockSet {
    key: String,
    /// Goes up on every change, so a solve can tell whether it used the current locks.
    version: u64,
    next_id: u64,
    locks: Vec<Lock>,
    profile: Option<ProfileLock>,
}

impl LockSet {
    fn summaries(&self) -> Vec<Value> {
        self.locks.iter().map(Lock::summary).collect()
    }

    fn is_empty(&self) -> bool {
        self.locks.is_empty() && self.profile.is_none()
    }

    fn clear(&mut self) {
        if !self.is_empty() {
            self.locks.clear();
            self.profile = None;
            self.version += 1;
        }
    }

    /// Every locked decision in the order to apply them: the kept earlier decisions first, then
    /// the locks from the most general (most "every card" steps) to the most specific, so the
    /// more specific lock wins where two overlap. A player with a player type keeps playing it
    /// on the way to their locks, so their kept earlier decisions are left out.
    fn ordered_nodes(&self) -> Vec<LockedNode> {
        let typed = self.profile.as_ref().map(|p| p.player);
        let mut by_generality: Vec<&Lock> = self.locks.iter().collect();
        by_generality.sort_by_key(|l| std::cmp::Reverse(l.wild.len()));
        self.locks
            .iter()
            .filter(|l| Some(l.player) != typed)
            .flat_map(|l| l.path.iter().cloned())
            .chain(by_generality.iter().flat_map(|l| l.nodes.iter().cloned()))
            .collect()
    }

    fn json(&self) -> Value {
        json!({
            "locks": self.summaries(),
            "locks_version": self.version,
            "profile": self.profile.as_ref().map(ProfileLock::json),
        })
    }
}

struct App {
    web_root: PathBuf,
    max_memory_gb: f64,
    status: Arc<Mutex<Status>>,
    solutions: Arc<Mutex<Solutions>>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// The kind of solve the worker is (or was last) filling.
    worker_kind: Kind,
    locks: LockSet,
    preflop: PreflopService,
}

type ApiResult = Result<Value, (u16, String)>;

fn bad(msg: impl Into<String>) -> (u16, String) {
    (400, msg.into())
}

/// Each player's EV at the root, in bb.
fn root_ev(game: &mut PostFlopGame) -> [f64; 2] {
    game.back_to_root();
    game.cache_normalized_weights();
    let mut ev = [0.0; 2];
    for (p, e) in ev.iter_mut().enumerate() {
        *e = round(compute_average(&game.expected_values(p), game.normalized_weights(p)) as f64 / CHIPS_PER_BB, 3);
    }
    ev
}

/// Who acted at each step of a history: "oop", "ip", or null where a card was dealt.
fn actors_json(actors: &[Option<usize>]) -> Value {
    json!(actors
        .iter()
        .map(|a| a.map(|p| if p == 0 { "oop" } else { "ip" }))
        .collect::<Vec<_>>())
}

impl App {
    /// Stops the worker. Returns whether it was still solving, i.e. its result is incomplete.
    fn stop_worker(&mut self) -> bool {
        let mut was_solving = false;
        if let Some(handle) = self.worker.take() {
            was_solving = !handle.is_finished();
            self.cancel.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
        self.cancel.store(false, Ordering::SeqCst);
        was_solving
    }

    fn start_solve(&mut self, body: &str) -> ApiResult {
        let raw: Value = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let req: SolveRequest = serde_json::from_value(raw.clone()).map_err(|e| bad(e.to_string()))?;
        let key = tree_key(&req.spot, &req.board);
        if req.locks {
            if self.locks.is_empty() {
                return Err(bad("There are no saved locks to solve with."));
            }
            if self.locks.key != key {
                return Err(bad("The saved locks were made for a different spot or board."));
            }
        }
        // A player type is built from the GTO solution, so that has to stay in memory too.
        let profile_job = if req.locks { self.locks.profile.clone() } else { None };

        let (mut game, starting_pot) = build_game(&req.spot, &req.board).map_err(bad)?;
        let (_, mem_compressed) = game.memory_usage();
        let memory_gb = mem_compressed as f64 / 1e9;
        let lock_gb = if profile_job.is_some() { memory_gb * PROFILE_MEMORY } else { 0.0 };
        if memory_gb + lock_gb > self.max_memory_gb {
            return Err(bad(format!(
                "This tree needs {:.1} GB of memory, over the {:.1} GB limit. Use fewer bet sizes, \
                 narrower ranges, or start pt-server with a higher --max-memory-gb.",
                memory_gb + lock_gb,
                self.max_memory_gb
            )));
        }
        if profile_job.is_some() {
            let sols = self.solutions.lock().unwrap();
            let gto_gb = match &sols.gto {
                Some(g) if g.key == key => g.memory_gb,
                _ => return Err(bad("Solve the spot without locks first: a player type starts from the GTO strategy.")),
            };
            let need = gto_gb + memory_gb + lock_gb;
            if need > self.max_memory_gb {
                return Err(bad(format!(
                    "A player type needs the GTO solution, the exploit solution and its own locks in memory at once: \
                     about {need:.1} GB here, over the {:.1} GB limit. Start pt-server with a higher --max-memory-gb, \
                     or use fewer bet sizes or narrower ranges.",
                    self.max_memory_gb
                )));
            }
        }

        // A solve replaces the last one of its kind. The other kind stays for comparison if it's
        // the same tree and both fit in memory. A solve that was cut off by this one is dropped.
        let kind = if req.locks { Kind::Exploit } else { Kind::Gto };
        if self.stop_worker() {
            *self.solutions.lock().unwrap().slot(self.worker_kind) = None;
        }
        let mut note = None;
        {
            let mut sols = self.solutions.lock().unwrap();
            *sols.slot(kind) = None;
            let other = sols.slot(kind.other());
            if let Some(o) = other.as_ref() {
                if o.key != key {
                    *other = None;
                } else if o.memory_gb + memory_gb + lock_gb > self.max_memory_gb {
                    *other = None;
                    note = Some(format!(
                        "There isn't room under --max-memory-gb for both solutions, so the {} one was dropped \
                         and there's nothing to compare against.",
                        if kind == Kind::Gto { "locked" } else { "GTO" }
                    ));
                }
            }
        }
        if self.locks.key != key {
            self.locks.clear();
            self.locks.key = key.clone();
        }
        let lock_nodes = if req.locks { self.locks.ordered_nodes() } else { Vec::new() };
        let locks_version = if req.locks { self.locks.version } else { 0 };

        *self.status.lock().unwrap() = Status {
            state: "solving",
            kind,
            max_iters: req.max_iters,
            target_pct: req.accuracy,
            memory_gb: memory_gb + lock_gb,
            note,
            board: req.board.clone(),
            spot: Some(raw["spot"].clone()),
            phase: profile_job.as_ref().map(|_| "profile"),
            ..Default::default()
        };

        let board = req.board.clone();
        let status = self.status.clone();
        let solutions = self.solutions.clone();
        let cancel = self.cancel.clone();
        self.worker_kind = kind;
        self.worker = Some(std::thread::spawn(move || {
            let t0 = Instant::now();
            let fail = |msg: String| {
                let mut s = status.lock().unwrap();
                s.state = "failed";
                s.phase = None;
                s.error = Some(msg);
            };
            game.allocate_memory(true);
            // Locks go on after memory is allocated and before solving.
            for node in &lock_nodes {
                if let Err(e) = locks::apply(&mut game, node) {
                    return fail(format!("Couldn't apply the locks: {e}"));
                }
            }
            // Then the player type, on every decision of that player not locked by hand, built
            // from the GTO solution a chunk at a time so the server can answer in between.
            let mut memory_gb = memory_gb;
            let mut profile_summary = Value::Null;
            if let Some(p) = profile_job {
                let mut builder = profile::Builder::new(p.profile.clone(), p.player);
                while !builder.done() {
                    if cancel.load(Ordering::SeqCst) {
                        return fail("Stopped while setting up the player type.".into());
                    }
                    let res = {
                        let mut sols = solutions.lock().unwrap();
                        match sols.gto.as_mut() {
                            Some(g) if g.key == key => builder.step(&mut g.game, &mut game, PROFILE_CHUNK),
                            _ => Err("The GTO solution the player type is built from is gone.".into()),
                        }
                    };
                    if let Err(e) = res {
                        return fail(format!("Couldn't set up the player type: {e}"));
                    }
                    let mut s = status.lock().unwrap();
                    s.phase_progress = builder.progress;
                    s.elapsed_s = t0.elapsed().as_secs_f64();
                }
                memory_gb += builder.bytes as f64 / 1e9;
                let mut summary = p.json();
                summary["nodes"] = json!(builder.locked);
                profile_summary = summary;
                let mut s = status.lock().unwrap();
                s.phase = None;
                s.memory_gb = memory_gb;
            }
            game.back_to_root();

            let target = starting_pot as f32 * req.accuracy / 100.0;
            let mut iteration = 0;
            let mut expl_pct = f64::NAN;
            while iteration < req.max_iters {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                solve_step(&game, iteration);
                iteration += 1;
                let mut expl = None;
                if iteration % CHECK_EVERY == 0 || iteration == req.max_iters {
                    expl = Some(compute_exploitability(&game));
                }
                let mut s = status.lock().unwrap();
                s.iteration = iteration;
                s.elapsed_s = t0.elapsed().as_secs_f64();
                if let Some(e) = expl {
                    expl_pct = e as f64 / starting_pot as f64 * 100.0;
                    s.exploitability_pct = Some(expl_pct);
                    if e <= target {
                        break;
                    }
                }
            }
            if iteration % CHECK_EVERY != 0 {
                let e = compute_exploitability(&game);
                expl_pct = e as f64 / starting_pot as f64 * 100.0;
                status.lock().unwrap().exploitability_pct = Some(expl_pct);
            }
            finalize(&mut game);
            let root_ev = root_ev(&mut game);
            *solutions.lock().unwrap().slot(kind) = Some(Solved {
                game,
                starting_pot,
                board,
                key,
                memory_gb,
                root_ev,
                exploitability_pct: expl_pct,
                iterations: iteration,
                locks_version,
                profile: profile_summary,
            });
            let mut s = status.lock().unwrap();
            s.state = "done";
            s.elapsed_s = t0.elapsed().as_secs_f64();
        }));
        Ok(json!({ "ok": true, "memory_gb": round(memory_gb + lock_gb, 2), "kind": kind.name() }))
    }

    fn status_json(&self) -> Value {
        let mut v = self.status.lock().unwrap().to_json();
        let sols = self.solutions.lock().unwrap();
        v["solutions"] = json!({
            "gto": sols.gto.as_ref().map(Solved::summary),
            "exploit": sols.exploit.as_ref().map(Solved::summary),
        });
        v["locks"] = json!(self.locks.summaries());
        v["locks_version"] = json!(self.locks.version);
        v["profile"] = json!(self.locks.profile.as_ref().map(ProfileLock::json));
        v
    }

    fn latest(&self) -> Kind {
        self.status.lock().unwrap().kind
    }

    fn node(&mut self, body: &str) -> ApiResult {
        let req: NodeRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let latest = self.latest();
        let mut sols = self.solutions.lock().unwrap();
        let kind = sols.pick(req.solution.as_deref(), latest)?;
        let (Some(solved), other) = sols.with_other(kind) else { unreachable!() };
        let game = &mut solved.game;

        // Walk the history step by step so a bad entry gives an error instead of a panic.
        let actors = locks::navigate(game, &req.history).map_err(bad)?;
        let mut board = solved.board.clone();
        for (&step, actor) in req.history.iter().zip(&actors) {
            if actor.is_none() {
                board.push_str(&card_to_string(step as u8).unwrap_or_default());
            }
        }
        let actors = actors_json(&actors);

        let total = game.total_bet_amount();
        let pot = solved.starting_pot + total[0] + total[1];

        if game.is_terminal_node() {
            return Ok(json!({ "type": "terminal", "solution": kind.name(), "board": board, "pot_bb": bb(pot), "actors": actors }));
        }

        if game.is_chance_node() {
            // Summarize what happens on every card that can come: the next player's action mix
            // and both ranges' equity, like a turn/river report.
            let mask = game.possible_cards();
            let mut cards = Vec::new();
            for card in 0..52u8 {
                if mask & (1u64 << card) == 0 {
                    continue;
                }
                game.play(card as usize);
                let mut entry = json!({ "card": card, "name": card_to_string(card).unwrap_or_default() });
                if !game.is_terminal_node() && !game.is_chance_node() {
                    game.cache_normalized_weights();
                    let player = game.current_player();
                    let total = game.total_bet_amount();
                    let p = solved.starting_pot + total[0] + total[1];
                    let mut eq = [0.0; 2];
                    for pl in 0..2 {
                        eq[pl] = round(compute_average(&game.equity(pl), game.normalized_weights(pl)) as f64, 4);
                    }
                    entry["player"] = json!(if player == 0 { "oop" } else { "ip" });
                    entry["actions"] = json!(game
                        .available_actions()
                        .iter()
                        // The card starts a new street, so nobody has bet on it yet.
                        .map(|a| action_json(a, p, [0, 0], player))
                        .collect::<Vec<_>>());
                    entry["freq"] = json!(range_frequencies(game));
                    entry["equity"] = json!(eq);
                }
                cards.push(entry);
                game.apply_history(&req.history);
            }
            return Ok(json!({
                "type": "chance",
                "solution": kind.name(),
                "board": board,
                "pot_bb": bb(pot),
                "actors": actors,
                "cards": cards,
            }));
        }

        let player = game.current_player();
        let combos = holes_to_strings(game.private_cards(player)).map_err(bad)?;
        let locked = game.current_locking_strategy().is_some();
        let mut node = decision_node_json(game, solved.starting_pot);
        node["type"] = json!("decision");
        node["solution"] = json!(kind.name());
        node["locked"] = json!(locked);
        node["board"] = json!(board);
        node["combos"] = json!(combos);
        node["actors"] = actors;

        // The same decision in the other solution, to compare against.
        if let Some(o) = other {
            let og = &mut o.game;
            if locks::navigate(og, &req.history).is_ok()
                && !og.is_terminal_node()
                && !og.is_chance_node()
                && og.available_actions().len() == node["actions"].as_array().map_or(0, |a| a.len())
            {
                let olocked = og.current_locking_strategy().is_some();
                let c = decision_node_json(og, o.starting_pot);
                node["compare"] = json!({
                    "solution": kind.other().name(),
                    "locked": olocked,
                    "hands": c["hands"],
                    "equity": c["equity"],
                    "ev_bb": c["ev_bb"],
                });
            }
        }
        Ok(node)
    }

    fn lock_preview(&mut self, body: &str) -> ApiResult {
        let req: PreviewRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let latest = self.latest();
        let mut sols = self.solutions.lock().unwrap();
        let kind = sols.pick(req.solution.as_deref(), latest)?;
        let solved = sols.slot(kind).as_mut().unwrap();
        locks::navigate(&mut solved.game, &req.history).map_err(bad)?;
        let built = locks::build(&mut solved.game, solved.starting_pot, &req.recipe).map_err(bad)?;
        let n = solved.game.num_private_hands(solved.game.current_player());
        let k = built.after.len();
        let strategy: Vec<Vec<f64>> = (0..n)
            .map(|h| (0..k).map(|a| round(built.strategy[a * n + h] as f64, 3)).collect())
            .collect();
        Ok(json!({
            "strategy": strategy,
            "before": locks::rounded(&built.before),
            "after": locks::rounded(&built.after),
            "warnings": built.warnings,
        }))
    }

    fn lock_add(&mut self, body: &str) -> ApiResult {
        let req: AddLockRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let latest = self.latest();
        let mut sols = self.solutions.lock().unwrap();
        let kind = sols.pick(req.solution.as_deref(), latest)?;
        let solved = sols.slot(kind).as_mut().unwrap();
        if !self.locks.is_empty() && self.locks.key != solved.key {
            return Err((409, "The saved locks are for a different spot. Clear them first.".into()));
        }
        let starting_pot = solved.starting_pot;
        let game = &mut solved.game;

        let actors = locks::navigate(game, &req.history).map_err(bad)?;
        if game.is_terminal_node() || game.is_chance_node() {
            return Err(bad("Only a decision can be locked."));
        }
        let player = game.current_player();
        let total = game.total_bet_amount();
        let pot = starting_pot + total[0] + total[1];
        let street_bets = street_bets(game);
        let actions: Vec<Value> = game
            .available_actions()
            .iter()
            .map(|a| action_json(a, pot, street_bets, player))
            .collect();
        // The decision the lock was made at also checks the recipe and gives the summary.
        let here = locks::build(game, starting_pot, &req.recipe).map_err(bad)?;

        let mut wild = req.wild.clone();
        wild.sort_unstable();
        wild.dedup();
        let histories = locks::expand(game, &req.history, &wild).map_err(bad)?;
        let mut nodes = Vec::with_capacity(histories.len());
        for h in &histories {
            locks::navigate(game, h).map_err(bad)?;
            if game.is_terminal_node()
                || game.is_chance_node()
                || game.current_player() != player
                || game.available_actions().len() != actions.len()
            {
                return Err(bad("The decision isn't the same on every card."));
            }
            let built = locks::build(game, starting_pot, &req.recipe).map_err(bad)?;
            nodes.push(LockedNode { history: h.clone(), strategy: built.strategy });
        }

        let mut path = Vec::new();
        if req.keep_path {
            let mut seen = HashSet::new();
            for h in &histories {
                for p in locks::decisions_on_path(game, h, player).map_err(bad)? {
                    if seen.insert(p.clone()) {
                        locks::navigate(game, &p).map_err(bad)?;
                        let built = locks::build(game, starting_pot, &Recipe::default()).map_err(bad)?;
                        path.push(LockedNode { history: p, strategy: built.strategy });
                    }
                }
            }
        }

        let mut lock = Lock {
            id: 0,
            label: req.label.chars().take(300).collect(),
            crumbs: req.crumbs.iter().take(64).map(|c| c.chars().take(80).collect()).collect(),
            player,
            history: req.history.clone(),
            actors,
            wild,
            keep_path: req.keep_path,
            recipe: req.recipe,
            actions,
            before: here.before,
            after: here.after,
            warnings: here.warnings,
            from: kind,
            nodes,
            path,
        };
        let replaces = self.locks.locks.iter().position(|l| l.same_target(&lock));
        let others: usize = self
            .locks
            .locks
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != replaces)
            .map(|(_, l)| l.nodes.len() + l.path.len())
            .sum();
        if others + lock.nodes.len() + lock.path.len() > MAX_LOCKED_NODES {
            return Err(bad(format!(
                "That would lock {} decisions, over the limit of {MAX_LOCKED_NODES}. Lock fewer cards or remove a lock.",
                others + lock.nodes.len() + lock.path.len()
            )));
        }

        if self.locks.is_empty() {
            self.locks.key = solved.key.clone();
        }
        match replaces {
            Some(i) => {
                lock.id = self.locks.locks[i].id;
                self.locks.locks[i] = lock;
            }
            None => {
                self.locks.next_id += 1;
                lock.id = self.locks.next_id;
                self.locks.locks.push(lock);
            }
        }
        self.locks.version += 1;
        Ok(self.locks.json())
    }

    fn lock_remove(&mut self, body: &str) -> ApiResult {
        let req: RemoveLockRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let before = self.locks.locks.len();
        self.locks.locks.retain(|l| l.id != req.id);
        if self.locks.locks.len() == before {
            return Err((404, format!("No lock {}.", req.id)));
        }
        self.locks.version += 1;
        Ok(self.locks.json())
    }

    fn lock_clear(&mut self) -> ApiResult {
        self.locks.clear();
        Ok(self.locks.json())
    }

    /// Sets the player type one player plays at every decision, replacing any other. It's
    /// built from the GTO solution when the spot is solved with locks.
    fn profile_set(&mut self, body: &str) -> ApiResult {
        let req: ProfileRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let player = match req.player.as_str() {
            "oop" => 0,
            "ip" => 1,
            p => return Err(bad(format!("player must be \"oop\" or \"ip\", not {p:?}"))),
        };
        let profile = req.profile.checked().map_err(bad)?;
        let key = {
            let sols = self.solutions.lock().unwrap();
            match &sols.gto {
                Some(g) => g.key.clone(),
                None => return Err((409, "Solve the spot without locks first: a player type starts from the GTO strategy.".into())),
            }
        };
        if !self.locks.is_empty() && self.locks.key != key {
            return Err((409, "The saved locks are for a different spot. Clear them first.".into()));
        }
        self.locks.key = key;
        self.locks.profile = Some(ProfileLock { player, profile });
        self.locks.version += 1;
        Ok(self.locks.json())
    }

    fn profile_clear(&mut self) -> ApiResult {
        if self.locks.profile.take().is_some() {
            self.locks.version += 1;
        }
        Ok(self.locks.json())
    }

    fn handle(&mut self, mut req: Request) {
        let url = req.url().split('?').next().unwrap_or("/").to_string();
        let method = req.method().clone();

        if !url.starts_with("/api/") {
            let resp = self.static_file(&url);
            let _ = req.respond(resp);
            return;
        }

        let mut body = String::new();
        if req.as_reader().read_to_string(&mut body).is_err() {
            let _ = req.respond(json_response(400, json!({ "error": "unreadable body" })));
            return;
        }
        if (&method, url.as_str()) == (&Method::Get, "/api/preflop/solution") {
            // Already JSON text, and a few MB: sent as it is.
            let resp = match self.preflop.solution() {
                Ok(text) => Response::from_data(text.as_bytes().to_vec())
                    .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
                    .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap()),
                Err((code, msg)) => json_response(code, json!({ "error": msg })),
            };
            let _ = req.respond(resp);
            return;
        }
        let result = match (&method, url.as_str()) {
            (Method::Get, "/api/status") => Ok(self.status_json()),
            (Method::Post, "/api/preflop/solve") => self.preflop.start(&body),
            (Method::Get, "/api/preflop/status") => Ok(self.preflop.status()),
            (Method::Post, "/api/preflop/stop") => self.preflop.stop(),
            (Method::Post, "/api/solve") => self.start_solve(&body),
            (Method::Post, "/api/stop") => {
                self.cancel.store(true, Ordering::SeqCst);
                Ok(json!({ "ok": true }))
            }
            (Method::Post, "/api/node") => self.node(&body),
            (Method::Post, "/api/lock/preview") => self.lock_preview(&body),
            (Method::Post, "/api/locks/add") => self.lock_add(&body),
            (Method::Post, "/api/locks/remove") => self.lock_remove(&body),
            (Method::Post, "/api/locks/clear") => self.lock_clear(),
            (Method::Post, "/api/profile") => self.profile_set(&body),
            (Method::Post, "/api/profile/clear") => self.profile_clear(),
            _ => Err((404, "not found".to_string())),
        };
        let resp = match result {
            Ok(v) => json_response(200, v),
            Err((code, msg)) => json_response(code, json!({ "error": msg })),
        };
        let _ = req.respond(resp);
    }

    fn static_file(&self, url: &str) -> Response<std::io::Cursor<Vec<u8>>> {
        let rel = if url == "/" { "index.html" } else { url.trim_start_matches('/') };
        // Only plain relative paths inside the web root.
        if rel.split('/').any(|part| part == ".." || part.is_empty() || part.starts_with('.')) {
            return Response::from_data(b"not found".to_vec()).with_status_code(404);
        }
        let path = self.web_root.join(rel);
        match std::fs::read(&path) {
            Ok(data) => Response::from_data(data).with_header(content_type(&path)),
            Err(_) => Response::from_data(b"not found".to_vec()).with_status_code(404),
        }
    }
}

fn content_type(path: &Path) -> Header {
    let ct = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        _ => "application/octet-stream",
    };
    Header::from_bytes("Content-Type", ct).unwrap()
}

fn json_response(code: u16, v: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(serde_json::to_vec(&v).unwrap())
        .with_status_code(code)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
        .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
}

fn main() {
    let mut web = PathBuf::from("../web");
    let mut port = 8080u16;
    let mut max_memory_gb = 8.0;
    let mut threads = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let val = args.next().unwrap_or_else(|| {
            eprintln!("missing value for {a}");
            std::process::exit(2);
        });
        match a.as_str() {
            "--web" => web = PathBuf::from(val),
            "--port" => port = val.parse().expect("--port"),
            "--max-memory-gb" => max_memory_gb = val.parse().expect("--max-memory-gb"),
            "--threads" => threads = Some(val.parse::<usize>().expect("--threads")),
            _ => {
                eprintln!("unknown argument {a}");
                std::process::exit(2);
            }
        }
    }
    if let Some(t) = threads {
        rayon::ThreadPoolBuilder::new().num_threads(t).build_global().unwrap();
    }
    if !web.join("index.html").exists() {
        eprintln!("can't find the web app at {} (pass --web <path to web/>)", web.display());
        std::process::exit(2);
    }

    // Localhost only: the API can use a lot of memory and CPU.
    let server = Server::http(("127.0.0.1", port)).unwrap_or_else(|e| {
        eprintln!("can't listen on port {port}: {e}");
        std::process::exit(1);
    });
    println!("PokerTrainer solver running at http://127.0.0.1:{port}");

    let mut app = App {
        web_root: web,
        max_memory_gb,
        status: Arc::new(Mutex::new(Status { state: "idle", ..Default::default() })),
        solutions: Arc::new(Mutex::new(Solutions::default())),
        cancel: Arc::new(AtomicBool::new(false)),
        worker: None,
        worker_kind: Kind::Gto,
        locks: LockSet::default(),
        preflop: PreflopService::new(default_cache_path()),
    };
    for req in server.incoming_requests() {
        app.handle(req);
    }
}
