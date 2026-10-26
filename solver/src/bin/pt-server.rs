//! pt-server: a local solver server for the PokerTrainer web app.
//!
//! It serves the web app and a small JSON API, so you can solve any spot on demand and
//! browse the whole game tree (flop, turn and river) in the browser.
//!
//! Usage:
//!   pt-server [--web ../web] [--port 8080] [--max-memory-gb 8] [--threads N]
//!
//! API (all JSON):
//!   POST /api/solve   {spot, board, accuracy, max_iters}  start a solve (replaces any current one)
//!   GET  /api/status                                       progress of the current solve
//!   POST /api/stop                                         stop early and keep the current strategy
//!   POST /api/node    {history: [..]}                      describe the node after `history`
//!
//! `history` lists action indexes for decision nodes and card ids (0-51) for chance nodes,
//! the same as postflop-solver's `apply_history`.

use postflop_solver::*;
use pt_solver::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;
use tiny_http::{Header, Method, Request, Response, Server};

/// How often (in iterations) the solver measures exploitability; it costs about one iteration.
const CHECK_EVERY: u32 = 10;

#[derive(Deserialize)]
struct SolveRequest {
    spot: Spot,
    board: String,
    #[serde(default = "default_accuracy")]
    accuracy: f32,
    #[serde(default = "default_iters")]
    max_iters: u32,
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
}

#[derive(Clone, Default)]
struct Status {
    state: &'static str,
    iteration: u32,
    max_iters: u32,
    exploitability_pct: Option<f64>,
    target_pct: f32,
    elapsed_s: f64,
    memory_gb: f64,
    error: Option<String>,
    board: String,
    spot: Option<Value>,
}

impl Status {
    fn to_json(&self) -> Value {
        json!({
            "state": self.state,
            "iteration": self.iteration,
            "max_iters": self.max_iters,
            "exploitability_pct": self.exploitability_pct.map(|e| round(e, 3)),
            "target_pct": self.target_pct,
            "elapsed_s": round(self.elapsed_s, 1),
            "memory_gb": round(self.memory_gb, 2),
            "error": self.error,
            "board": self.board,
            "spot": self.spot,
        })
    }
}

struct Solved {
    game: PostFlopGame,
    starting_pot: i32,
    /// The board as the user typed it; dealt cards are appended in order.
    board: String,
}

struct App {
    web_root: PathBuf,
    max_memory_gb: f64,
    status: Arc<Mutex<Status>>,
    game: Arc<Mutex<Option<Solved>>>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

type ApiResult = Result<Value, (u16, String)>;

fn bad(msg: impl Into<String>) -> (u16, String) {
    (400, msg.into())
}

impl App {
    fn stop_worker(&mut self) {
        if let Some(handle) = self.worker.take() {
            self.cancel.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
        self.cancel.store(false, Ordering::SeqCst);
    }

    fn start_solve(&mut self, body: &str) -> ApiResult {
        let raw: Value = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let req: SolveRequest = serde_json::from_value(raw.clone()).map_err(|e| bad(e.to_string()))?;
        self.stop_worker();
        *self.game.lock().unwrap() = None;

        let (mut game, starting_pot) = build_game(&req.spot, &req.board).map_err(bad)?;
        let (_, mem_compressed) = game.memory_usage();
        let memory_gb = mem_compressed as f64 / 1e9;
        if memory_gb > self.max_memory_gb {
            return Err(bad(format!(
                "This tree needs {memory_gb:.1} GB of memory, over the {:.1} GB limit. Use fewer bet sizes, \
                 narrower ranges, or start pt-server with a higher --max-memory-gb.",
                self.max_memory_gb
            )));
        }

        *self.status.lock().unwrap() = Status {
            state: "solving",
            max_iters: req.max_iters,
            target_pct: req.accuracy,
            memory_gb,
            board: req.board.clone(),
            spot: Some(raw["spot"].clone()),
            ..Default::default()
        };

        let board = req.board.clone();
        let status = self.status.clone();
        let slot = self.game.clone();
        let cancel = self.cancel.clone();
        self.worker = Some(std::thread::spawn(move || {
            let t0 = Instant::now();
            game.allocate_memory(true);
            let target = starting_pot as f32 * req.accuracy / 100.0;
            let mut iteration = 0;
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
                    s.exploitability_pct = Some(e as f64 / starting_pot as f64 * 100.0);
                    if e <= target {
                        break;
                    }
                }
            }
            if iteration % CHECK_EVERY != 0 {
                let e = compute_exploitability(&game);
                status.lock().unwrap().exploitability_pct = Some(e as f64 / starting_pot as f64 * 100.0);
            }
            finalize(&mut game);
            *slot.lock().unwrap() = Some(Solved { game, starting_pot, board });
            let mut s = status.lock().unwrap();
            s.state = "done";
            s.elapsed_s = t0.elapsed().as_secs_f64();
        }));
        Ok(json!({ "ok": true, "memory_gb": round(memory_gb, 2) }))
    }

    fn node(&mut self, body: &str) -> ApiResult {
        let req: NodeRequest = serde_json::from_str(body).map_err(|e| bad(e.to_string()))?;
        let mut slot = self.game.lock().unwrap();
        let solved = slot.as_mut().ok_or((409, "No finished solve yet.".to_string()))?;
        let game = &mut solved.game;

        // Walk the history step by step so a bad entry gives an error instead of a panic.
        game.back_to_root();
        let mut board = solved.board.clone();
        for (i, &step) in req.history.iter().enumerate() {
            if game.is_terminal_node() {
                return Err(bad(format!("history[{i}]: the hand is already over")));
            }
            if game.is_chance_node() {
                if step >= 52 || game.possible_cards() & (1u64 << step) == 0 {
                    return Err(bad(format!("history[{i}]: card {step} can't be dealt here")));
                }
                board.push_str(&card_to_string(step as u8).unwrap_or_default());
            } else if step >= game.available_actions().len() {
                return Err(bad(format!("history[{i}]: no action {step} here")));
            }
            game.play(step);
        }

        let street_bets = game.total_bet_amount();
        let pot = solved.starting_pot + street_bets[0] + street_bets[1];

        if game.is_terminal_node() {
            return Ok(json!({ "type": "terminal", "board": board, "pot_bb": bb(pot) }));
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
                    let street = game.total_bet_amount();
                    let p = solved.starting_pot + street[0] + street[1];
                    let mut eq = [0.0; 2];
                    for pl in 0..2 {
                        eq[pl] = round(compute_average(&game.equity(pl), game.normalized_weights(pl)) as f64, 4);
                    }
                    entry["player"] = json!(if player == 0 { "oop" } else { "ip" });
                    entry["actions"] = json!(game
                        .available_actions()
                        .iter()
                        .map(|a| action_json(a, p, street, player))
                        .collect::<Vec<_>>());
                    entry["freq"] = json!(range_frequencies(game));
                    entry["equity"] = json!(eq);
                }
                cards.push(entry);
                game.apply_history(&req.history);
            }
            return Ok(json!({ "type": "chance", "board": board, "pot_bb": bb(pot), "cards": cards }));
        }

        let player = game.current_player();
        let combos = holes_to_strings(game.private_cards(player)).map_err(bad)?;
        let mut node = decision_node_json(game, solved.starting_pot);
        node["type"] = json!("decision");
        node["board"] = json!(board);
        node["combos"] = json!(combos);
        Ok(node)
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
        let result = match (&method, url.as_str()) {
            (Method::Get, "/api/status") => Ok(self.status.lock().unwrap().to_json()),
            (Method::Post, "/api/solve") => self.start_solve(&body),
            (Method::Post, "/api/stop") => {
                self.cancel.store(true, Ordering::SeqCst);
                Ok(json!({ "ok": true }))
            }
            (Method::Post, "/api/node") => {
                if self.status.lock().unwrap().state != "done" {
                    Err((409, "The solve hasn't finished yet.".to_string()))
                } else {
                    self.node(&body)
                }
            }
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
        game: Arc::new(Mutex::new(None)),
        cancel: Arc::new(AtomicBool::new(false)),
        worker: None,
    };
    for req in server.incoming_requests() {
        app.handle(req);
    }
}
