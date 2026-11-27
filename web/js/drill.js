// Drill: deals you decisions from the solver's solutions, scores every answer by the EV it loses
// against the best play, and keeps track of where you lose the most.
//
// Sources:
//   Preflop     every decision in a Preflop solver chart: opens, facing an open, squeeze spots,
//               facing 3-bets and 4-bets, from any seat.
//   Flops       flop decisions from the solved library in data/.
//   Your solve  whole hands in the spot solved on pt-server, flop to river. The other player
//               holds a real hand and plays the solver's strategy for it, or a player type
//               (calling station, nit, ...) while you're scored against the best exploit; turn
//               and river cards are dealt at random.
//
// Stats, recent decisions, a log for the leak report and the mistakes to review are kept in
// localStorage.

import { cardsHtml, handClass, expandClass, splitCards } from "./cards.js";
import { renderGrid } from "./grid.js";
import { loadIndex, loadSolution, nodeHands, actionText, actionColor, actionLabel, classSummary, rangeFrequencies } from "./data.js";
import { renderNodeView, modeTabsHtml, wireModeTabs } from "./nodeview.js";
import * as pf from "./pfpack.js";
import { showdown } from "./handeval.js";
import * as types from "./profiles.js";

const STORE = "pokertrainer.drill.v1";
const SETTINGS = "pokertrainer.drill.settings.v1";
const PTYPE_PREFS = "pokertrainer.ptype.v1"; // shared with the Postflop solver tab
const LOG_MAX = 3000;
const RECENT_MAX = 30;
const MISTAKES_PER_SOURCE = 200;

const SOURCES = [["preflop", "Preflop"], ["flops", "Flops"], ["live", "Your solve"]];
const PF_CATS = [
  ["open", "Opens"],
  ["vsopen", "Facing an open"],
  ["squeeze", "Squeeze spots"],
  ["vs3b", "Facing a 3-bet"],
  ["vs4b", "Facing a 4-bet or more"],
];
const PF_CAT = Object.fromEntries(PF_CATS);
const POST_TYPES = { first: "first to act", checked: "checked to you", bet: "facing a bet", raise: "facing a raise" };
const STREETS = ["flop", "turn", "river"];
const CARD_RANKS = "23456789TJQKA";
const CARD_SUITS = "cdhs"; // pt-server's card ids are 4 * rank + suit, clubs first
const ALL_CARDS = [...CARD_RANKS].flatMap((r) => [...CARD_SUITS].map((s) => r + s));

const settings = {
  source: "preflop",
  mode: "new", // or "review"
  preflop: { pack: null, cats: PF_CATS.map(([id]) => id), seat: "any", hands: "close", line: null },
  flops: { spots: null, seat: "both", depth: "first" },
  // opponent: "gto", "exploit" (the solve with locks, whatever it locks) or a player type's id.
  live: { seat: "both", streets: [...STREETS], opponent: "gto", only: null },
};

const ui = {
  root: null,
  hooks: {},
  index: { spots: [] },
  packs: [],
  pfServer: null, // the server's preflop status, or null without a server
  side: "recent",
  ready: false,
  pending: null,
  dealing: 0, // bumped by every deal, so a slow one that was overtaken is dropped
  busy: 0, // the deal still in flight, for the "Dealing…" hint
  rangeOpen: true,
  rangeView: { mode: "strategy", focus: null, pinned: true },
  watching: false, // polling a solve started from here
};
let data = loadStore();
let session = newSession();
let current = null; // the question or hand summary on the table

// ---- Small helpers -----------------------------------------------------------------------

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const num = (x) => String(Math.round(x * 100) / 100);
const bb = (x) => `${num(x)}bb`;
const pct = (x, d = 0) => `${(x * 100).toFixed(d)}%`;
const cap = (s) => s[0].toUpperCase() + s.slice(1);
const pick = (arr) => arr[Math.floor(Math.random() * arr.length)];
const sameList = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);
const cardId = (c) => 4 * CARD_RANKS.indexOf(c[0]) + CARD_SUITS.indexOf(c[1]);
const overlaps = (a, b) => splitCards(a).some((c) => b.includes(c));
const streetOf = (board) => STREETS[Math.min(2, splitCards(board).length - 3)];

function pickWeighted(items, weight) {
  const total = items.reduce((s, x) => s + weight(x), 0);
  let r = Math.random() * total;
  for (const x of items) {
    r -= weight(x);
    if (r <= 0) return x;
  }
  return items[items.length - 1];
}

/** An action index drawn from a mix of frequencies. */
function sampleIndex(freq) {
  const total = freq.reduce((a, b) => a + b, 0);
  let r = Math.random() * total;
  for (let i = 0; i < freq.length; i++) {
    r -= freq[i];
    if (r <= 0 && freq[i] > 0) return i;
  }
  return freq.findLastIndex((f) => f > 0);
}

async function api(path, body) {
  const res = await fetch(`api/${path}`, body === undefined ? {} : {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  let out;
  try {
    out = await res.json();
  } catch {
    throw new Error(res.status === 404 ? "no-server" : `HTTP ${res.status}`);
  }
  if (!res.ok) throw new Error(out.error || `HTTP ${res.status}`);
  return out;
}

/**
 * Grades a decision by the EV it gives up, as a share of the pot. Playing an action the solver
 * takes at least a fifth of the time counts as correct: mixed actions are worth the same.
 */
export function grade(lossBb, potBb, freq) {
  const lossPct = potBb > 0 ? (lossBb / potBb) * 100 : 0;
  if (freq >= 0.2 || lossBb <= Math.max(0.02, potBb * 0.005)) return { label: "Correct", cls: "good" };
  if (lossPct <= 3) return { label: "Small mistake", cls: "ok" };
  if (lossPct <= 10) return { label: "Mistake", cls: "bad" };
  return { label: "Blunder", cls: "bad" };
}

// ---- Stored stats ------------------------------------------------------------------------

function emptyStore() {
  return { totals: {}, log: [], recent: [], mistakes: [] };
}

function loadStore() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORE) || "null");
    if (saved) return { ...emptyStore(), ...saved };
  } catch {}
  return migrate();
}

/** The old Postflop trainer's stats and mistakes carry over to the Flops source. */
function migrate() {
  const out = emptyStore();
  try {
    const old = JSON.parse(localStorage.getItem("pokertrainer.stats.v1") || "null");
    if (old?.hands) out.totals.flops = { n: old.hands, correct: old.correct || 0, loss: old.evLossBb || 0, lossPct: old.evLossPotPct || 0 };
    for (const m of old?.mistakes || []) {
      out.mistakes.push({ id: `flops|${m.spot}|${m.flop}|${m.path.join("/")}|${m.combo}`, src: "flops", spot: m.spot, flop: m.flop, path: m.path, combo: m.combo });
    }
  } catch {}
  return out;
}

function saveStore() {
  try {
    localStorage.setItem(STORE, JSON.stringify(data));
  } catch {
    // Private mode or blocked storage: stats just last for this visit.
  }
}

function newSession() {
  return { n: 0, correct: 0, loss: 0, lossPct: 0, counts: {}, decisions: [] };
}

function loadSettings() {
  try {
    const s = JSON.parse(localStorage.getItem(SETTINGS) || "null");
    if (!s) return;
    if (SOURCES.some(([id]) => id === s.source)) settings.source = s.source;
    Object.assign(settings.preflop, s.preflop || {}, { line: null });
    Object.assign(settings.flops, s.flops || {});
    const { solution, ...live } = s.live || {};
    Object.assign(settings.live, live, { only: null });
    // Saved before player types: "exploit" meant playing against your locks.
    if (!live.opponent && solution === "exploit") settings.live.opponent = "exploit";
  } catch {}
}

function saveSettings() {
  try {
    const { line, ...preflop } = settings.preflop;
    const { only, ...live } = settings.live;
    localStorage.setItem(SETTINGS, JSON.stringify({ source: settings.source, preflop, flops: settings.flops, live }));
  } catch {}
}

function record(q, choice, lossBb, g) {
  const lossPct = q.pot > 0 ? (lossBb / q.pot) * 100 : 0;
  data.totals[q.src] ??= { n: 0, correct: 0, loss: 0, lossPct: 0 };
  for (const t of [data.totals[q.src], session]) {
    t.n += 1;
    if (g.cls === "good") t.correct += 1;
    t.loss += lossBb;
    t.lossPct += lossPct;
  }
  session.counts[g.label] = (session.counts[g.label] || 0) + 1;
  const entry = {
    src: q.src, title: q.title, combo: q.combo, board: q.board || "", chose: q.actions[choice].text,
    best: q.actions[q.bestIndex].text, loss: +lossBb.toFixed(3), lossPct: +lossPct.toFixed(2), grade: g.label, cls: g.cls,
  };
  session.decisions.push({ ...entry, leak: q.leak });
  data.recent.unshift(entry);
  data.recent = data.recent.slice(0, RECENT_MAX);
  data.log.push({ t: Date.now(), src: q.src, ...q.leak, loss: entry.loss, ok: g.cls === "good" ? 1 : 0 });
  if (data.log.length > LOG_MAX) data.log.splice(0, data.log.length - LOG_MAX);
  if (q.key) {
    if (q.review) {
      if (g.cls === "good") forget(q.key.id);
    } else if (g.cls !== "good") {
      forget(q.key.id);
      data.mistakes.unshift(q.key);
      const mine = data.mistakes.filter((m) => m.src === q.src);
      if (mine.length > MISTAKES_PER_SOURCE) {
        const drop = new Set(mine.slice(MISTAKES_PER_SOURCE).map((m) => m.id));
        data.mistakes = data.mistakes.filter((m) => !drop.has(m.id));
      }
    }
  }
  saveStore();
}

function forget(id) {
  data.mistakes = data.mistakes.filter((m) => m.id !== id);
}

// ---- Entry points ------------------------------------------------------------------------

export async function initDrill(root, hooks = {}) {
  ui.root = root;
  ui.hooks = hooks;
  const [index, pfIndex, pfServer] = await Promise.all([
    loadIndex().catch(() => ({ spots: [] })),
    fetch("data/preflop/index.json").then((r) => (r.ok ? r.json() : { packs: [] })).catch(() => ({ packs: [] })),
    api("preflop/status").catch(() => null),
  ]);
  ui.index = index;
  ui.packs = pfIndex.packs || [];
  ui.pfServer = pfServer;
  loadSettings();
  if (!packOptions().some((p) => p.id === settings.preflop.pack)) settings.preflop.pack = packOptions()[0]?.id || null;

  root.innerHTML = `
    <div class="trainer drill">
      <aside class="panel controls">
        <div class="tabs" id="dr-source">${SOURCES.map(([id, name]) => `<button data-src="${id}">${name}</button>`).join("")}</div>
        <div class="tabs" id="dr-mode">
          <button data-mode="new">New hands</button>
          <button data-mode="review">Review mistakes <span id="dr-mcount"></span></button>
        </div>
        <div id="dr-settings"></div>
        <div id="dr-stats"></div>
      </aside>
      <section class="panel table" id="dr-table"><p class="hint">Dealing…</p></section>
      <aside class="panel detail" id="dr-side">
        <div class="tabs" id="dr-sidetabs"><button data-side="recent">Recent</button><button data-side="leaks">Leaks</button></div>
        <div id="dr-sidebody"></div>
      </aside>
    </div>`;

  root.querySelector("#dr-source").onclick = (e) => {
    const src = e.target.closest("button")?.dataset.src;
    if (!src || src === settings.source) return;
    settings.source = src;
    saveSettings();
    refresh();
  };
  root.querySelector("#dr-mode").onclick = (e) => {
    const mode = e.target.closest("button")?.dataset.mode;
    if (!mode) return;
    settings.mode = mode;
    refresh();
  };
  root.querySelector("#dr-sidetabs").onclick = (e) => {
    const side = e.target.closest("button")?.dataset.side;
    if (!side) return;
    ui.side = side;
    renderSide();
  };
  document.addEventListener("keydown", onKey);

  ui.ready = true;
  if (ui.pending) {
    applySpec(ui.pending);
    ui.pending = null;
  }
  await refresh();
}

/**
 * When the tab is shown again: the Solver tab may have changed the solve or the player type. A hand
 * from a solve that's gone is dealt again; otherwise only the settings are brought up to date.
 */
export async function refreshDrill() {
  if (!ui.ready || settings.source !== "live") return;
  const status = await api("status").catch(() => null);
  const plan = status?.solutions && (status.solutions.gto || status.solutions.exploit) ? opponentPlan(status) : null;
  const key = plan?.ready ? solveKey(status, plan.solution) : null;
  if (key && key === live.key && current) return renderSettings();
  refresh();
}

/**
 * Drills one spot from another tab:
 *   {preflop: {pack, history}}       one decision of a preflop chart
 *   {live: {only, solution}}         the solve on pt-server: whole hands, or `only` one decision
 *                                    ({history, labels})
 */
export function openDrill(spec) {
  if (!ui.ready) {
    ui.pending = spec;
    return;
  }
  applySpec(spec);
  refresh();
}

function applySpec(spec) {
  settings.mode = "new";
  if (spec.preflop) {
    settings.source = "preflop";
    settings.preflop.pack = spec.preflop.pack;
    settings.preflop.line = spec.preflop.history;
  } else if (spec.live) {
    settings.source = "live";
    settings.live.only = spec.live.only || null;
    if (spec.live.solution) settings.live.opponent = spec.live.solution === "exploit" ? "exploit" : "gto";
  }
  saveSettings();
}

async function refresh() {
  ++ui.dealing;
  current = null;
  waiting(true);
  if (settings.source === "preflop") ui.pfServer = await api("preflop/status").catch(() => null);
  renderTabs();
  await renderSettings();
  renderStats();
  renderSide();
  await deal();
}

function renderTabs() {
  const root = ui.root;
  root.querySelectorAll("#dr-source button").forEach((b) => b.classList.toggle("active", b.dataset.src === settings.source));
  root.querySelectorAll("#dr-mode button").forEach((b) => b.classList.toggle("active", b.dataset.mode === settings.mode));
  root.querySelectorAll("#dr-sidetabs button").forEach((b) => b.classList.toggle("active", b.dataset.side === ui.side));
}

function onKey(e) {
  const root = ui.root;
  if (!root?.isConnected || root.hidden || !current) return;
  if (["INPUT", "SELECT", "TEXTAREA"].includes(e.target.tagName) || e.ctrlKey || e.metaKey || e.altKey) return;
  if (current.answered || current.summary) {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      root.querySelector("#dr-next")?.click();
    }
  } else if (/^[1-9]$/.test(e.key)) {
    const i = +e.key - 1;
    if (i < current.actions.length) answer(i);
  }
}

// ---- Settings panel ----------------------------------------------------------------------

function packOptions() {
  const out = ui.packs.map((p) => ({ id: p.id, name: p.name, description: p.description }));
  if (ui.pfServer?.has_solution) out.push({ id: "server", name: "Your last preflop solve", description: "The last solve on pt-server." });
  return out;
}

async function renderSettings() {
  const el = ui.root.querySelector("#dr-settings");
  if (settings.source === "preflop") return renderPreflopSettings(el);
  if (settings.source === "flops") return renderFlopSettings(el);
  return renderLiveSettings(el);
}

function renderPreflopSettings(el) {
  const s = settings.preflop;
  const packs = packOptions();
  if (!packs.length) {
    el.innerHTML = `<p class="hint">No preflop charts found in <code>data/preflop/</code>.</p>`;
    return;
  }
  const meta = packs.find((p) => p.id === s.pack);
  el.innerHTML = `
    <label>Chart <select id="dr-pack">${packs.map((p) => `<option value="${esc(p.id)}">${esc(p.name)}</option>`).join("")}</select></label>
    <p class="meta">${esc(meta?.description || "")}</p>
    ${s.line ? `<div class="onlyspot"><span>Only this spot: <b id="dr-linename">…</b></span><button class="x" id="dr-unline" title="Drill every spot">×</button></div>` : `
      <fieldset id="dr-cats"><legend>Situations</legend>
        ${PF_CATS.map(([id, name]) => `<label class="check"><input type="checkbox" value="${id}" ${s.cats.includes(id) ? "checked" : ""}> ${name}</label>`).join("")}
      </fieldset>
      <label>Your seat <select id="dr-seat"><option value="any">Any seat</option>${pf.SEATS.map((x) => `<option>${x}</option>`).join("")}</select></label>`}
    <label>Hands <select id="dr-hands">
      <option value="close">Close decisions</option>
      <option value="all">Every hand, as often as it's dealt</option>
    </select></label>
    <p class="hint">${s.hands === "close"
      ? "Close decisions are hands at the edges of the range and hands the solver mixes with. Clear folds and clear raises are skipped."
      : "Every hand that gets here, so most of them are easy."}</p>`;
  const $ = (id) => el.querySelector(id);
  $("#dr-pack").value = s.pack;
  $("#dr-pack").onchange = (e) => {
    s.pack = e.target.value;
    s.line = null;
    saveSettings();
    refresh();
  };
  $("#dr-hands").value = s.hands;
  $("#dr-hands").onchange = (e) => {
    s.hands = e.target.value;
    saveSettings();
    renderPreflopSettings(el);
    deal();
  };
  if (s.line) {
    $("#dr-unline").onclick = () => {
      s.line = null;
      renderPreflopSettings(el);
      deal();
    };
    loadPack(s.pack).then(({ spots }) => {
      const spot = spots.find((x) => sameList(x.history, s.line));
      const name = el.querySelector("#dr-linename");
      if (name) name.textContent = spot ? spotName(spot) : "not in this chart";
    }).catch(() => {});
    return;
  }
  $("#dr-cats").onchange = (e) => {
    s.cats = [...el.querySelectorAll("#dr-cats input:checked")].map((x) => x.value);
    saveSettings();
    deal();
  };
  $("#dr-seat").value = s.seat;
  $("#dr-seat").onchange = (e) => {
    s.seat = e.target.value;
    saveSettings();
    deal();
  };
}

function renderFlopSettings(el) {
  const s = settings.flops;
  const spots = ui.index.spots.filter((x) => x.flops.length);
  if (!spots.length) {
    el.innerHTML = `<p class="hint">No solved flops yet. Run <code>python3 scripts/build_library.py</code>.</p>`;
    return;
  }
  const chosen = new Set(s.spots || spots.map((x) => x.id));
  el.innerHTML = `
    <fieldset id="dr-spots"><legend>Spots</legend>
      ${spots.map((x) => `<label class="check"><input type="checkbox" value="${x.id}" ${chosen.has(x.id) ? "checked" : ""}> ${esc(x.name)}</label>`).join("")}
    </fieldset>
    <label>Your seat <select id="dr-fseat">
      <option value="both">Either player</option><option value="oop">Out of position</option><option value="ip">In position</option>
    </select></label>
    <label>Decisions <select id="dr-depth">
      <option value="first">First decision on the flop</option>
      <option value="facing">Facing a bet or raise</option>
      <option value="all">Any flop decision</option>
    </select></label>
    <p class="hint">${spots.reduce((n, x) => n + x.flops.length, 0)} solved flops. Turn and river decisions are in Your solve.</p>`;
  el.querySelector("#dr-spots").onchange = () => {
    s.spots = [...el.querySelectorAll("#dr-spots input:checked")].map((x) => x.value);
    saveSettings();
    deal();
  };
  for (const [id, key] of [["#dr-fseat", "seat"], ["#dr-depth", "depth"]]) {
    const sel = el.querySelector(id);
    sel.value = s[key];
    sel.onchange = () => {
      s[key] = sel.value;
      saveSettings();
      deal();
    };
  }
}

const other = (p) => (p === "oop" ? "ip" : "oop");

function loadTypePrefs() {
  try {
    return { player: "oop", id: null, strength: 1, ...JSON.parse(localStorage.getItem(PTYPE_PREFS) || "{}") };
  } catch {
    return { player: "oop", id: null, strength: 1 };
  }
}

function saveTypePrefs(change) {
  try {
    localStorage.setItem(PTYPE_PREFS, JSON.stringify({ ...loadTypePrefs(), ...change }));
  } catch {}
}

/**
 * What playing against the chosen opponent takes: the solution to deal from, the player type
 * the other player plays in it, and whether that solve is there yet. For a player type you play
 * one seat (`hero`) and the type the other (`villain`).
 */
function opponentPlan(status) {
  const s = settings.live;
  const sols = status.solutions || {};
  const typed = sols.exploit?.profile || null;
  const opp = s.opponent || "gto";
  if (opp === "gto" || opp === "exploit") {
    // Falls back to whichever solve there is.
    const solution = sols[opp] ? opp : sols.gto ? "gto" : "exploit";
    if (solution === "exploit" && typed) {
      // The solve with locks is against a player type: the same as picking that type.
      const want = types.profileById(typed.id) || { id: typed.id, name: typed.name, rules: types.rulesFromList(typed.rules) };
      return { ready: true, solution, typed, want, hero: other(typed.player), villain: typed.player, strength: typed.strength };
    }
    return { ready: true, solution, typed: null };
  }
  const want = types.profileById(opp)
    || (typed?.id === opp ? { id: typed.id, name: typed.name, rules: types.rulesFromList(typed.rules) } : null);
  if (!want) {
    s.opponent = "gto";
    return opponentPlan(status);
  }
  const hero = s.seat === "oop" || s.seat === "ip" ? s.seat : typed?.id === opp ? other(typed.player) : "ip";
  const strength = s.strength ?? 1;
  const ready = !!sols.exploit && typed?.id === opp && typed.player === other(hero) && Math.abs(typed.strength - strength) < 0.01;
  return { ready, solution: "exploit", typed: ready ? typed : null, want, hero, villain: other(hero), strength };
}

/** "Setting up BB as a calling station: 40%" or "Solving against a calling station: iteration 12, …". */
function solveProgressText(status) {
  const p = status.profile;
  const who = p ? `${esc(status.spot?.[p.player] || p.player)} as ${esc(types.typeNoun(p))}` : "the locks";
  if (status.phase === "profile") return `Setting up ${who}: ${pct(status.phase_progress || 0)}`;
  const acc = status.exploitability_pct == null ? "" : `, accuracy ${status.exploitability_pct.toFixed(2)}% of the pot (target ${status.target_pct}%)`;
  const against = status.kind === "exploit" ? (p ? ` against ${esc(types.typeNoun(p))}` : " with your locks") : "";
  return `Solving${against}: iteration ${status.iteration}${acc}`;
}

async function renderLiveSettings(el) {
  const s = settings.live;
  el.innerHTML = `<p class="hint">Checking the solver server…</p>`;
  const status = await api("status").catch(() => null);
  if (settings.source !== "live") return;
  if (!status) {
    el.innerHTML = `<p class="hint">Drilling your own solves needs the local server. From the repo:</p>
      <pre><code>cd solver
cargo run --release --bin pt-server</code></pre>
      <p class="hint">Then open <a href="http://127.0.0.1:8080">http://127.0.0.1:8080</a>, solve a spot in the Postflop solver tab and come back.</p>`;
    return;
  }
  const sols = status.solutions || {};
  const solving = status.state === "solving";
  if (!sols.gto && !sols.exploit) {
    el.innerHTML = `<p class="hint">${solving ? "A solve is running. Come back when it's done." : "Solve a spot in the Postflop solver tab first, then drill it here."}</p>`;
    if (solving) watchSolve();
    return;
  }
  const spot = status.spot;
  const plan = opponentPlan(status);
  const typed = sols.exploit?.profile || null;
  if (plan.want && s.opponent === "exploit") {
    // Came from the solve against a player type: pick that type, so seat and strength changes work.
    Object.assign(s, { opponent: plan.want.id, strength: plan.strength, seat: plan.hero });
    saveSettings();
  }
  const lockedBy = [...new Set((status.locks || []).map((l) => l.player))];
  const known = new Set(types.PROFILES.map((p) => p.id));
  const typeMode = !!plan.want;
  const locks = status.locks?.length || 0;
  let typeBlock = "";
  if (typeMode) {
    const noun = esc(types.typeNoun(plan.want));
    const villain = esc(spot[plan.villain]);
    let state;
    if (plan.ready) {
      state = `<p class="hint">${villain} plays like ${noun} at every decision, and you're scored against the best way to exploit them.</p>`;
    } else if (solving) {
      state = `<p class="meta" id="dr-solving">${solveProgressText(status)}</p>`;
      watchSolve();
    } else {
      state = `<div class="typesolve">
        <p class="hint">First the solver works out how ${villain} plays as ${noun} and the best way to exploit that. It's usually quicker than the GTO solve${
          sols.exploit ? " and replaces the solve with locks you have now" : ""}${locks ? `, and keeps your ${locks === 1 ? "saved lock" : `${locks} saved locks`}` : ""}.</p>
        <button class="primary" id="dr-typesolve">Solve against ${noun}</button>
        <p class="error" id="dr-typeerr"></p>
      </div>`;
    }
    typeBlock = `
      <label>Strength <select id="dr-strength">${types.STRENGTHS.map(([v, t]) => `<option value="${v}">${t}</option>`).join("")}</select></label>
      ${plan.want.blurb ? `<p class="meta">${esc(plan.want.blurb)}</p>` : ""}
      ${state}`;
  } else if (plan.solution === "exploit") {
    typeBlock = `<p class="hint">${lockedBy.length === 1 ? `${esc(spot[lockedBy[0]])} plays your locks` : "The locked player plays your locks"}, and you're scored against the best exploit.</p>`;
  }
  el.innerHTML = `
    <p class="meta"><b>${esc(spot.oop)}</b> (out of position) vs <b>${esc(spot.ip)}</b> on ${cardsHtml(status.board, "tiny")}<br>
      Pot ${bb(spot.pot_bb)}, ${bb(spot.stack_bb)} behind</p>
    <label>Play against <select id="dr-opp">
      <option value="gto">GTO</option>
      ${sols.exploit && !typed ? `<option value="exploit">Your locks (exploit solve)</option>` : ""}
      <optgroup label="Player types">${types.PROFILES.map((p) => `<option value="${p.id}">${esc(p.name)}</option>`).join("")}${
        typed && !known.has(typed.id) ? `<option value="${esc(typed.id)}">${esc(typed.name)}</option>` : ""}</optgroup>
    </select></label>
    ${typeBlock}
    ${s.only ? `<div class="onlyspot"><span>Only this decision: <b>${esc(s.only.labels.length ? s.only.labels.join(" › ") : "the first one")}</b></span>
      <button class="x" id="dr-unonly" title="Play whole hands">×</button></div>` : `
      <label>${typeMode ? "You play" : "Your seat"} <select id="dr-lseat">
        ${typeMode ? "" : `<option value="both">Either player</option>`}<option value="oop">${esc(spot.oop)} (out of position)</option><option value="ip">${esc(spot.ip)} (in position)</option>
      </select></label>
      <fieldset id="dr-streets"><legend>You decide on</legend>
        ${STREETS.filter((st) => STREETS.indexOf(st) >= STREETS.indexOf(streetOf(status.board)))
          .map((st) => `<label class="check"><input type="checkbox" value="${st}" ${s.streets.includes(st) ? "checked" : ""}> The ${st}</label>`).join("")}
      </fieldset>
      <p class="hint">The other player gets a real hand and plays ${typeMode ? "the player type" : "the solver's strategy"} for it. On streets you leave out, the solver plays your hand for you.</p>`}`;

  const opp = el.querySelector("#dr-opp");
  opp.value = typed && (s.opponent === "exploit" || s.opponent === typed.id) && plan.solution === "exploit" ? typed.id : plan.want ? plan.want.id : plan.solution;
  opp.onchange = () => {
    s.opponent = opp.value;
    if (typed && opp.value === typed.id) {
      s.strength = typed.strength;
      s.seat = other(typed.player);
    } else if (types.profileById(opp.value)) {
      if (s.seat === "both") s.seat = "ip";
      s.strength = loadTypePrefs().id === opp.value ? loadTypePrefs().strength : 1;
    } else if (opp.value === "exploit" && lockedBy.length === 1) {
      s.seat = other(lockedBy[0]);
    }
    saveSettings();
    renderLiveSettings(el);
    deal();
  };
  const strength = el.querySelector("#dr-strength");
  if (strength) {
    strength.value = String(plan.strength);
    strength.onchange = () => {
      s.strength = +strength.value;
      saveSettings();
      renderLiveSettings(el);
      deal();
    };
  }
  el.querySelector("#dr-typesolve")?.addEventListener("click", (e) => solveAgainstType(e.target, plan, status));
  if (s.only) {
    el.querySelector("#dr-unonly").onclick = () => {
      s.only = null;
      renderLiveSettings(el);
      deal();
    };
    return;
  }
  const seat = el.querySelector("#dr-lseat");
  seat.value = typeMode ? plan.hero : s.seat;
  seat.onchange = () => {
    s.seat = seat.value;
    saveSettings();
    if (typeMode) renderLiveSettings(el);
    deal();
  };
  el.querySelector("#dr-streets").onchange = () => {
    s.streets = [...el.querySelectorAll("#dr-streets input:checked")].map((x) => x.value);
    saveSettings();
    deal();
  };
}

/** Sets the player type on the other seat and solves for the exploit, then deals. */
async function solveAgainstType(btn, plan, status) {
  btn.disabled = true;
  const err = ui.root.querySelector("#dr-typeerr");
  try {
    await api("profile", types.profileRequest(plan.want, plan.villain, plan.strength));
    await api("solve", { spot: status.spot, board: status.board, accuracy: status.target_pct || 0.5, max_iters: status.max_iters || 1000, locks: true });
  } catch (e) {
    btn.disabled = false;
    if (err) err.textContent = e.message;
    return;
  }
  saveTypePrefs({ player: plan.villain, id: plan.want.id, strength: plan.strength });
  refresh();
}

/** While a solve runs, shows its progress and deals once it's done. */
function watchSolve() {
  if (ui.watching) return;
  ui.watching = true;
  const tick = async () => {
    const st = settings.source === "live" ? await api("status").catch(() => null) : null;
    if (!st || st.state !== "solving") {
      ui.watching = false;
      if (st && settings.source === "live") refresh();
      return;
    }
    const text = solveProgressText(st);
    const line = ui.root.querySelector("#dr-solving");
    if (line) line.innerHTML = text;
    if (!current) ui.root.querySelector("#dr-table").innerHTML = `<p class="hint">${text}</p>`;
    setTimeout(tick, 1000);
  };
  setTimeout(tick, 1000);
}

// ---- Dealing -----------------------------------------------------------------------------

/** Dims the table while the next hand is dealt, so the old one can't be answered. */
function waiting(on) {
  ui.root.querySelector("#dr-table").classList.toggle("waiting", on);
}

/** Shows "Dealing…" if a deal takes a moment, e.g. while the server answers. */
function dealingHint(ticket) {
  ui.busy = ticket;
  waiting(true);
  setTimeout(() => {
    if (ticket !== ui.dealing || ui.busy !== ticket) return;
    waiting(false);
    ui.root.querySelector("#dr-table").innerHTML = `<p class="hint">Dealing…</p>`;
  }, 250);
}

async function deal() {
  const ticket = ++ui.dealing;
  const table = ui.root.querySelector("#dr-table");
  current = null;
  dealingHint(ticket);
  let q;
  try {
    q = settings.mode === "review" ? await dealReview() : await dealNew();
  } catch (e) {
    q = { message: `Couldn't deal a hand: ${e.message === "no-server" ? "the solver server isn't running." : e.message}` };
  }
  if (ticket !== ui.dealing) return; // the settings changed while this was dealing
  ui.busy = 0;
  waiting(false);
  renderMistakeCount();
  if (!q || q.message) {
    current = null;
    table.innerHTML = `<p class="hint">${q?.message || "Nothing to deal."}</p>`;
    return;
  }
  show(q);
}

function dealNew() {
  if (settings.source === "preflop") return dealPreflop();
  if (settings.source === "flops") return dealFlop();
  return dealLive();
}

function show(q) {
  current = q;
  if (q.summary) renderHandSummary(q);
  else renderQuestion(q);
}

// ---- Preflop -------------------------------------------------------------------------------

const packCache = new Map();

/** A chart and every decision in it, with how often each one comes up. */
function loadPack(id) {
  const key = id === "server" ? `server:${ui.pfServer?.seconds}:${ui.pfServer?.iteration}` : id;
  if (!packCache.has(key)) {
    const meta = ui.packs.find((p) => p.id === id);
    const url = id === "server" ? "api/preflop/solution" : `data/preflop/${meta?.file}`;
    packCache.set(key, fetch(url)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`${url}: HTTP ${r.status}`))))
      .then((pack) => ({ pack, spots: preflopSpots(pack) })));
  }
  return packCache.get(key);
}

/**
 * Every decision of a preflop solution: the line to it, the seat to act and its range there,
 * the money in, and `prob`, how often a hand plays out this way.
 */
function preflopSpots(pack) {
  const spots = [];
  const first = pf.firstSeat(pack);
  const visit = (id, st) => {
    const node = pack.nodes[id];
    if (!node || node.t || node.cut) return;
    const s = node.s;
    const freq = pf.actionFrequencies(node, st.reach[s]);
    const cat = st.level === 0 ? (st.limpers ? "vsopen" : "open") : st.level === 1 ? (st.callers ? "squeeze" : "vsopen") : st.level === 2 ? "vs3b" : "vs4b";
    spots.push({ id, node, seat: s, cat, history: st.history, steps: st.steps, reach: st.reach[s], invested: st.invested, folded: st.folded, level: st.level, bet: st.bet, prob: st.prob });
    node.a.forEach(([kind, to], i) => {
      if (freq[i] < 1e-6) return;
      const reach = st.reach.slice();
      const r = new Float32Array(169);
      for (let c = 0; c < 169; c++) if (st.reach[s][c]) r[c] = st.reach[s][c] * pf.classFreqs(node, c)[i];
      reach[s] = r;
      const invested = st.invested.slice();
      const folded = st.folded.slice();
      let { level, bet, callers, limpers } = st;
      const step = { seat: s, kind, to, level, limpers };
      if (kind === "fold") folded[s] = true;
      else if (kind !== "check") invested[s] = to;
      if (kind === "raise" || kind === "allin") {
        level++;
        bet = to;
        callers = 0;
      } else if (kind === "call") {
        if (level === 0) limpers++;
        else callers++;
      }
      visit(node.c[i], { history: [...st.history, i], steps: [...st.steps, step], reach, invested, folded, level, bet, callers, limpers, prob: st.prob * freq[i] });
    });
  };
  visit(0, {
    history: [], steps: [],
    reach: Array.from({ length: 6 }, () => new Float32Array(169).fill(1)),
    invested: [0, 0, 0, 0, 0.5, 1],
    folded: pf.SEATS.map((_, s) => s < first),
    level: 0, bet: 1, callers: 0, limpers: 0, prob: 1,
  });
  return spots;
}

/**
 * Hands worth drilling at a decision: ones the solver mixes with, and ones next to a hand in the
 * grid that plays differently, i.e. the edges between folding, calling and raising.
 */
function closeClasses(spot) {
  if (spot.close) return spot.close;
  const { node, reach } = spot;
  const main = new Int8Array(169).fill(-1);
  const mixed = new Uint8Array(169);
  for (let c = 0; c < 169; c++) {
    if (reach[c] < 0.01) continue;
    const fr = pf.classFreqs(node, c);
    let best = 0;
    for (let i = 1; i < fr.length; i++) if (fr[i] > fr[best]) best = i;
    main[c] = best;
    mixed[c] = fr[best] < 0.9 ? 1 : 0;
  }
  spot.close = [];
  for (let c = 0; c < 169; c++) {
    if (main[c] < 0) continue;
    let close = !!mixed[c];
    const row = Math.floor(c / 13);
    const col = c % 13;
    for (let dr = -1; dr <= 1 && !close; dr++) {
      for (let dc = -1; dc <= 1 && !close; dc++) {
        const rr = row + dr;
        const cc = col + dc;
        if ((dr || dc) && rr >= 0 && rr < 13 && cc >= 0 && cc < 13 && main[rr * 13 + cc] >= 0 && main[rr * 13 + cc] !== main[c]) close = true;
      }
    }
    if (close) spot.close.push(c);
  }
  return spot.close;
}

async function dealPreflop() {
  const s = settings.preflop;
  if (!s.pack) return { message: "No preflop charts found in data/preflop/." };
  const { pack, spots } = await loadPack(s.pack);
  let pool = spots.filter((x) => x.prob > 1e-6);
  if (s.line) {
    pool = spots.filter((x) => sameList(x.history, s.line));
    if (!pool.length) return { message: "That spot isn't in this chart. Clear it to drill every spot." };
  } else {
    pool = pool.filter((x) => s.cats.includes(x.cat) && (s.seat === "any" || pf.SEATS[x.seat] === s.seat));
    if (!pool.length) return { message: s.cats.length ? "No spot in this chart matches these settings." : "Pick at least one situation." };
  }
  // Every situation comes up equally often; within one, spots come up as often as at the table.
  const cat = pick([...new Set(pool.map((x) => x.cat))]);
  const spot = pickWeighted(pool.filter((x) => x.cat === cat), (x) => x.prob);
  const all = [];
  for (let c = 0; c < 169; c++) if (spot.reach[c] >= 0.001) all.push(c);
  let classes = s.hands === "close" ? closeClasses(spot) : all;
  if (!classes.length) classes = all;
  // Weighted by combos, so offsuit hands come up as often as they're dealt.
  const c = pickWeighted(classes, (x) => pf.COMBOS[x] * spot.reach[x]);
  const combo = pick(expandClass(pf.CLASSES[c]));
  return preflopQuestion(s.pack, pack, spot, combo, false);
}

function preflopQuestion(packId, pack, spot, combo, review) {
  const c = pf.CLASS_INDEX.get(handClass(combo));
  const { node, seat } = spot;
  const toCall = spot.bet - spot.invested[seat];
  const meta = packOptions().find((p) => p.id === packId);
  const freq = pf.classFreqs(node, c);
  const ev = pf.classEvs(node, c);
  return {
    src: "preflop", review, packId, pack, spot, c, combo,
    title: `${spotName(spot)} · ${meta?.name || "preflop"}`,
    leak: { cat: spot.cat, seat },
    actions: node.a.map((a, i) => ({ text: pfActionText(spot, i), color: pf.actionColor(node.a, i) })),
    freq, ev, evNote: "EV is in bb compared with folding now: what's already in the pot is gone either way.",
    pot: spot.invested.reduce((a, b) => a + b, 0),
    toCall,
    key: { id: `preflop|${packId}|${spot.history.join(",")}|${combo}`, src: "preflop", pack: packId, history: spot.history, combo },
  };
}

function pfActionText(spot, i) {
  const [kind, to] = spot.node.a[i];
  const toCall = spot.bet - spot.invested[spot.seat];
  switch (kind) {
    case "fold": return "Fold";
    case "check": return "Check";
    case "call": return `${spot.level === 0 ? "Limp" : "Call"} ${num(toCall)}`;
    case "raise": return `Raise to ${num(to)}`;
    case "allin": return `All-in ${num(to)}`;
    default: return kind;
  }
}

/** "opens to 2.5bb", "3-bets to 10bb", or in the second person for your own actions. */
function pfVerb(st, you) {
  const s = (third, second) => (you ? second : third);
  switch (st.kind) {
    case "check": return s("checks", "check");
    case "call": return st.level === 0 ? s("limps", "limp") : s("calls", "call");
    case "allin": return s(`goes all-in for ${bb(st.to)}`, `go all-in for ${bb(st.to)}`);
    case "raise": {
      if (st.level === 0) return st.limpers ? s(`raises to ${bb(st.to)}`, `raise to ${bb(st.to)}`) : s(`opens to ${bb(st.to)}`, `open to ${bb(st.to)}`);
      const n = st.level + 2;
      return s(`${n}-bets to ${bb(st.to)}`, `${n}-bet to ${bb(st.to)}`);
    }
    default: return st.kind;
  }
}

function pfLine(spot) {
  const parts = spot.steps.filter((st) => st.kind !== "fold").map((st) => {
    const you = st.seat === spot.seat;
    return `${you ? "You" : pf.SEATS[st.seat]} ${pfVerb(st, you)}`;
  });
  return parts.length ? `${parts.join(", ")}.` : "Folded to you.";
}

/** "open", "3-bet", "call": what one step of a preflop line was. */
function pfNoun(st) {
  if (st.kind === "raise") return st.level === 0 ? (st.limpers ? "raise" : "open") : `${st.level + 2}-bet`;
  if (st.kind === "call") return st.level === 0 ? "limp" : "call";
  return st.kind === "allin" ? "all-in" : st.kind;
}

/** A short name for a preflop decision: "CO first in", "BB vs BTN open", "BTN open, BB 3-bet, BTN to act". */
function spotName(spot) {
  const seat = pf.SEATS[spot.seat];
  const acts = spot.steps.filter((st) => st.kind !== "fold");
  if (!acts.length) return `${seat} first in`;
  const list = acts.map((st) => `${pf.SEATS[st.seat]} ${pfNoun(st)}`).join(", ");
  return acts.some((st) => st.seat === spot.seat) ? `${list}, ${seat} to act` : `${seat} vs ${list}`;
}

function seatStrip(spot) {
  const last = new Map();
  for (const st of spot.steps) last.set(st.seat, st);
  return `<div class="seats drillseats">${pf.SEATS.map((name, s) => {
    const st = last.get(s);
    let cls = "";
    let text = "";
    if (spot.folded[s]) [cls, text] = ["folded", "Folded"];
    else if (s === spot.seat) [cls, text] = ["acting", "You"];
    else if (st) text = st.kind === "call" ? (st.level === 0 ? "Limped" : "Called") : st.kind === "check" ? "Checked" : st.kind === "allin" ? "All-in" : `Raised ${num(st.to)}`;
    else if (spot.invested[s]) text = `In ${num(spot.invested[s])}`;
    return `<div class="seat ${cls}"><b>${name}</b><span>${text}</span></div>`;
  }).join("")}</div>`;
}

// ---- Flops (the solved library) ------------------------------------------------------------

function flopNodeMatches(node, s) {
  if (s.seat !== "both" && node.player !== s.seat) return false;
  if (s.depth === "first") return node.path.length === 0 || (node.path.length === 1 && node.path[0] === "Check");
  if (s.depth === "facing") return node.to_call_bb > 0;
  return true;
}

async function dealFlop() {
  const s = settings.flops;
  const chosen = s.spots ? new Set(s.spots) : null;
  const spots = ui.index.spots.filter((x) => x.flops.length && (!chosen || chosen.has(x.id)));
  if (!spots.length) return { message: ui.index.spots.length ? "Pick at least one spot." : "No solved flops yet. Run scripts/build_library.py." };
  // A few attempts in case a flop has no node matching the filters.
  for (let attempt = 0; attempt < 20; attempt++) {
    const spot = pick(spots);
    const flop = pick(spot.flops).flop;
    const sol = await loadSolution(spot.id, flop);
    // Weight nodes by how much of the range gets there, so common spots come up more.
    const nodes = sol.nodes
      .filter((n) => flopNodeMatches(n, s))
      .map((n) => ({ n, hands: nodeHands(n) }))
      .filter((x) => x.hands.length);
    if (!nodes.length) continue;
    const { n: node, hands } = pickWeighted(nodes, (x) => x.hands.reduce((sum, h) => sum + h.reach, 0));
    const hand = pickWeighted(hands, (h) => h.reach);
    return flopQuestion(spot, flop, sol, node, hand, false);
  }
  return { message: "No solved decisions match these settings yet." };
}

/** The actions on the way to a library node, as steps for the hand log. */
function flopSteps(sol, node) {
  return node.path.map((label, i) => {
    const actor = sol.byPath.get(node.path.slice(0, i).join(" / "));
    return { type: "act", player: actor.player, action: actor.actions[actor.labels.indexOf(label)] };
  });
}

function flopQuestion(spot, flop, sol, node, hand, review) {
  const steps = flopSteps(sol, node);
  return {
    src: "flops", review, spot, flop, sol, node, hand, steps,
    names: { oop: spot.oop, ip: spot.ip },
    hero: node.player,
    title: spot.name,
    leak: { street: "flop", type: postflopType(node, steps) },
    actions: node.actions.map((a) => ({ text: actionText(a), color: actionColor(a, node.actions) })),
    freq: hand.strat, ev: hand.ev,
    evNote: "EV is what the hand expects to win from here in bb, counting the chips already in the pot.",
    pot: node.pot_bb, toCall: node.to_call_bb, combo: hand.combo, board: flop,
    key: { id: `flops|${spot.id}|${flop}|${node.path.join("/")}|${hand.combo}`, src: "flops", spot: spot.id, flop, path: node.path, combo: hand.combo },
  };
}

/** first / checked / bet / raise: what the player to act faces on this street. */
function postflopType(node, steps) {
  let aggressive = 0;
  for (const st of steps) {
    if (st.type === "card") aggressive = 0;
    else if (["bet", "raise", "allin"].includes(st.action.type)) aggressive++;
  }
  if (node.to_call_bb > 0) return aggressive >= 2 ? "raise" : "bet";
  return node.player === "oop" ? "first" : "checked";
}

// ---- Your solve (pt-server) ----------------------------------------------------------------

const live = { key: null, ranges: null };

/** Identifies a finished solve, so hands and saved mistakes from another one aren't mixed in. */
function solveKey(status, solution) {
  const sol = status.solutions?.[solution];
  return JSON.stringify([status.spot, status.board, solution, sol?.iterations, sol?.exploitability_pct, sol?.locks_version]);
}

async function liveContext() {
  const status = await api("status");
  const sols = status.solutions || {};
  if (!sols.gto && !sols.exploit) {
    return { message: status.state === "solving" ? "A solve is running. Come back when it's done." : "Solve a spot in the Postflop solver tab first, then drill it here." };
  }
  const plan = opponentPlan(status);
  if (!plan.ready) {
    const noun = types.typeNoun(plan.want);
    return { message: status.state === "solving" ? solveProgressText(status) : `Solve against ${noun} first (on the left), then the hands start.` };
  }
  const { solution, typed } = plan;
  const key = solveKey(status, solution);
  if (live.key !== key) {
    live.key = key;
    live.ranges = null;
  }
  return { status, solution, typed, hero: typed ? other(typed.player) : null, key, names: { oop: status.spot.oop || "OOP", ip: status.spot.ip || "IP" } };
}

/** Each player's hands and their weights at the start, from their first decision. */
async function liveRanges(ctx) {
  if (!live.ranges) {
    const hands = (n) => n.combos.map((combo, i) => n.hands[i] && { combo, w: n.hands[i][0] }).filter(Boolean);
    const first = await api("node", { history: [], solution: ctx.solution });
    // The other player's range at their first decision, after an action the first player takes
    // with some hand: a lock can take one out entirely (e.g. "always bets").
    let second = null;
    for (let a = 0; first.type === "decision" && a < first.actions.length && !second; a++) {
      const n = await api("node", { history: [a], solution: ctx.solution });
      if (n.type === "decision" && hands(n).length) second = n;
    }
    if (first.type !== "decision" || !second) throw new Error("The solve doesn't start with a decision for each player.");
    live.ranges = { [first.player]: hands(first), [second.player]: hands(second) };
  }
  return live.ranges;
}

async function dealLive() {
  const ctx = await liveContext();
  if (ctx.message) return ctx;
  const s = settings.live;
  if (s.only) return dealLiveDecision(ctx, s.only.history, s.only.labels, null, false);
  if (!s.streets.length) return { message: "Pick at least one street to decide on." };
  const ranges = await liveRanges(ctx);
  const start = STREETS.indexOf(streetOf(ctx.status.board));
  const streets = new Set(s.streets.filter((st) => STREETS.indexOf(st) >= start));
  if (!streets.size) return { message: "Pick a street this solve plays." };
  // Hands that end before you get a decision on the chosen streets are dealt again.
  for (let attempt = 0; attempt < 30; attempt++) {
    const hero = ctx.hero || (s.seat === "both" ? pick(["oop", "ip"]) : s.seat);
    const hand = newLiveHand(ctx, ranges, hero, streets);
    const q = await advanceLive(hand);
    if (q) return q;
  }
  return { message: `No hand got to a decision for you on the ${[...streets].join(" or ")} in 30 tries. Try more streets or the other seat.` };
}

function newLiveHand(ctx, ranges, hero, streets) {
  let combos;
  for (let i = 0; i < 2000 && !combos; i++) {
    const oop = pickWeighted(ranges.oop, (x) => x.w).combo;
    const ip = pickWeighted(ranges.ip, (x) => x.w).combo;
    if (!overlaps(oop, splitCards(ip))) combos = { oop, ip };
  }
  if (!combos) throw new Error("Couldn't deal two hands that don't share a card.");
  return { ...ctx, hero, villain: hero === "oop" ? "ip" : "oop", streets, combos, history: [], steps: [], labels: [], decisions: [], board: ctx.status.board, over: false };
}

/**
 * Plays the hand on until it's your turn on a street you decide on, or the hand is over.
 * Returns the question, or the hand summary.
 */
async function advanceLive(hand) {
  for (let guard = 0; guard < 80; guard++) {
    const node = await api("node", { history: hand.history, solution: hand.solution });
    hand.board = node.board;
    if (node.type === "terminal") {
      hand.over = true;
      hand.pot = node.pot_bb;
      return hand.decisions.length ? { summary: true, hand } : null;
    }
    if (node.type === "chance") {
      const dead = new Set([...splitCards(node.board), ...splitCards(hand.combos.oop), ...splitCards(hand.combos.ip)]);
      const card = pick(ALL_CARDS.filter((c) => !dead.has(c)));
      const street = splitCards(node.board).length === 3 ? "turn" : "river";
      hand.history.push(cardId(card));
      hand.steps.push({ type: "card", card, street });
      hand.labels.push(`${cap(street)} ${card}`);
      continue;
    }
    const combo = hand.combos[node.player];
    const i = node.combos.indexOf(combo);
    const row = i >= 0 ? node.hands[i] : null;
    if (node.player === hand.hero && hand.streets.has(streetOf(node.board))) {
      if (!row) {
        // Can't happen while you follow the solver's lines; end the hand rather than guess.
        hand.over = true;
        hand.lostTrack = true;
        return hand.decisions.length ? { summary: true, hand } : null;
      }
      return liveQuestion(hand, node, row, false);
    }
    playStep(hand, node, sampleIndex(strategyOf(node, row, combo)), node.player === hand.hero ? "auto" : "solver");
  }
  throw new Error("The hand didn't end.");
}

/** A hand's strategy at a node; one that no longer reaches it (very rare) plays like its class. */
function strategyOf(node, row, combo) {
  const k = node.actions.length;
  if (row) return row.slice(1, 1 + k);
  const hands = nodeHands(node);
  return classSummary(hands, k).get(handClass(combo))?.freq || rangeFrequencies(hands, k).freq;
}

function playStep(hand, node, i, who, extra = {}) {
  const a = node.actions[i];
  hand.history.push(i);
  hand.steps.push({ type: "act", player: node.player, action: a, who, ...extra });
  hand.labels.push(`${hand.names[node.player]} ${actionLabel(a)}`);
}

/** "vs Calling station", "against your locks" or "GTO": who the hand is played against. */
function againstText(hand) {
  if (hand.typed) return `vs ${hand.typed.name}`;
  return hand.solution === "exploit" ? "against your locks" : "GTO";
}

function liveQuestion(hand, node, row, review) {
  const k = node.actions.length;
  const street = streetOf(node.board);
  // Against locks or a player type, how GTO plays the same hand here (if it gets here).
  const gtoRow = hand.solution === "exploit" && node.compare?.solution === "gto" ? node.compare.hands[node.combos.indexOf(hand.combos[node.player])] : undefined;
  return {
    src: "live", review, hand, node, row,
    steps: hand.steps.slice(), names: hand.names, hero: node.player,
    title: `${hand.names.oop} vs ${hand.names.ip} · ${againstText(hand)}`,
    leak: { street, type: postflopType(node, hand.steps), ...(hand.typed && node.player !== hand.typed.player ? { vs: hand.typed.id } : {}) },
    actions: node.actions.map((a) => ({ text: actionText(a), color: actionColor(a, node.actions) })),
    freq: row.slice(1, 1 + k), ev: row.slice(1 + k, 1 + 2 * k),
    gtoFreq: gtoRow === undefined ? undefined : gtoRow ? gtoRow.slice(1, 1 + k) : null,
    evNote: "EV is what the hand expects to win from here in bb, counting the chips already in the pot.",
    pot: node.pot_bb, toCall: node.to_call_bb, combo: hand.combos[node.player], board: node.board,
    startBoard: hand.status.board,
    key: {
      id: `live|${hand.key}|${hand.history.join(",")}|${hand.combos[node.player]}`,
      src: "live", solve: hand.key, solution: hand.solution, history: hand.history.slice(), labels: hand.labels.slice(),
      combo: hand.combos[node.player],
    },
  };
}

/** One decision of the solve, with a hand dealt from the range that gets there (or `combo`). */
async function dealLiveDecision(ctx, history, labels, combo, review) {
  const node = await api("node", { history, solution: ctx.solution });
  if (node.type !== "decision") return { message: "There's no decision at that point of the solve." };
  const hands = nodeHands(node);
  if (!hands.length) return { message: "No hand gets to that decision." };
  const hand = combo ? hands.find((h) => h.combo === combo) : pickWeighted(hands, (h) => h.reach);
  if (!hand) return null;
  const fake = {
    ...ctx, hero: node.player, history: history.slice(), labels: labels.slice(), steps: [], decisions: [],
    combos: { [node.player]: hand.combo }, single: true,
  };
  const q = liveQuestion(fake, node, node.hands[node.combos.indexOf(hand.combo)], review);
  q.lineText = labels.length ? labels.join(" › ") : "First decision of the hand.";
  return q;
}

// ---- Review mistakes -----------------------------------------------------------------------

/** Saved mistakes that can be dealt now: from this source, a chart that's available and the solve that's loaded. */
function reviewable() {
  const packs = new Set(packOptions().map((p) => p.id));
  return data.mistakes.filter((m) => m.src === settings.source
    && (m.src !== "preflop" || packs.has(m.pack))
    && (m.src !== "live" || m.solve === live.key));
}

function renderMistakeCount() {
  const el = ui.root.querySelector("#dr-mcount");
  const n = reviewable().length;
  if (el) el.textContent = n ? `(${n})` : "";
}

async function dealReview() {
  if (settings.source === "live") {
    const ctx = await liveContext();
    if (ctx.message) return ctx;
  }
  for (;;) {
    const list = reviewable();
    if (!list.length) {
      return { message: "No mistakes to review here. Hands you get wrong in New hands are saved here until you answer them correctly." };
    }
    const m = pick(list);
    // A library flop that's gone can't be loaded; any other error is passed on, so a server
    // that's down doesn't cost you your saved mistakes.
    const q = await mistakeQuestion(m).catch((e) => {
      if (m.src !== "flops") throw e;
      return null;
    });
    if (q) return q;
    forget(m.id);
    saveStore();
  }
}

async function mistakeQuestion(m) {
  if (m.src === "preflop") {
    if (!packOptions().some((p) => p.id === m.pack)) return null;
    const { pack, spots } = await loadPack(m.pack);
    const spot = spots.find((x) => sameList(x.history, m.history));
    if (!spot || spot.reach[pf.CLASS_INDEX.get(handClass(m.combo))] < 0.001) return null;
    return preflopQuestion(m.pack, pack, spot, m.combo, true);
  }
  if (m.src === "flops") {
    const spot = ui.index.spots.find((s) => s.id === m.spot && s.flops.some((f) => f.flop === m.flop));
    if (!spot) return null;
    const sol = await loadSolution(m.spot, m.flop);
    const node = sol.byPath.get(m.path.join(" / "));
    const hand = node && nodeHands(node).find((h) => h.combo === m.combo);
    return hand ? flopQuestion(spot, m.flop, sol, node, hand, true) : null;
  }
  const ctx = await liveContext();
  if (ctx.message || ctx.key !== m.solve) return null;
  const q = await dealLiveDecision(ctx, m.history, m.labels, m.combo, true);
  return q && !q.message ? q : null;
}

// ---- The table -------------------------------------------------------------------------------

const position = (p) => (p === "oop" ? "out of position" : "in position");

function actVerb(a, you) {
  const s = (third, second) => (you ? second : third);
  switch (a.type) {
    case "fold": return s("folds", "fold");
    case "check": return s("checks", "check");
    case "call": return s(`calls ${bb(a.add_bb)}`, `call ${bb(a.add_bb)}`);
    case "bet": return s(`bets ${bb(a.add_bb)} (${a.pct}%)`, `bet ${bb(a.add_bb)} (${a.pct}%)`);
    case "raise": return s(`raises to ${bb(a.to_bb)}`, `raise to ${bb(a.to_bb)}`);
    case "allin": return s(`goes all-in for ${bb(a.add_bb)}`, `go all-in for ${bb(a.add_bb)}`);
    default: return a.type;
  }
}

const MARK = { good: "✓", ok: "~", bad: "✗" };

/** The hand so far, one line per street. */
function handLog(steps, q, startBoard) {
  const lines = [{ street: streetOf(startBoard), card: null, acts: [] }];
  for (const st of steps) {
    if (st.type === "card") {
      lines.push({ street: st.street, card: st.card, acts: [] });
      continue;
    }
    const you = st.player === q.hero;
    let text = `${you ? "You" : esc(q.names[st.player])} ${actVerb(st.chosen || st.action, you)}`;
    if (st.who === "hero" && st.grade) {
      text += ` <span class="mark ${st.grade.cls}" title="${st.grade.label}">${MARK[st.grade.cls]}</span>`;
      if (st.chosen) text += ` <span class="meta">(played on as if you'd ${actVerb(st.action, true)})</span>`;
    } else if (st.who === "auto") {
      text += ` <span class="meta">(played for you)</span>`;
    }
    lines.at(-1).acts.push(text);
  }
  return `<div class="handlog">${lines.map((l, i) => `<p><b>${cap(l.street)}</b>${l.card ? ` ${cardsHtml(l.card, "tiny")}` : ""}${
    l.acts.length ? ` ${l.acts.join(", ")}.` : i === lines.length - 1 ? "" : " (no action)"}</p>`).join("")}</div>`;
}

function questionHtml(q) {
  const review = q.review ? "Review · " : "";
  if (q.src === "preflop") {
    const { spot } = q;
    return `
      <p class="spotname">${review}${esc(q.title)}</p>
      ${seatStrip(spot)}
      <p class="line">${pfLine(spot)}</p>
      <p class="pot">Pot <b>${bb(q.pot)}</b>${q.toCall > 0 && spot.node.a.some((a) => a[0] === "call") ? ` · <b>${bb(q.toCall)}</b> to call` : ""}</p>
      <div class="hero"><span>You are <b>${pf.SEATS[spot.seat]}</b> with</span><div class="hole">${cardsHtml(q.combo, "big")}</div></div>`;
  }
  const villain = q.names[q.hero === "oop" ? "ip" : "oop"];
  const line = q.lineText
    ? `<p class="line">${esc(q.lineText)}</p>`
    : q.steps.length ? handLog(q.steps, q, q.startBoard || q.board) : `<p class="line">First to act on the ${streetOf(q.board)}.</p>`;
  return `
    <p class="spotname">${review}${esc(q.title)}</p>
    <div class="board">${cardsHtml(q.board, "big")}</div>
    ${line}
    <p class="pot">Pot <b>${bb(q.pot)}</b>${q.toCall ? ` · ${esc(villain)} ${q.leak.type === "raise" ? "raised" : "bet"}, you face <b>${bb(q.toCall)}</b>` : ""}</p>
    <div class="hero"><span>You are <b>${esc(q.names[q.hero])}</b> (${position(q.hero)}) with</span><div class="hole">${cardsHtml(q.combo, "big")}</div></div>`;
}

function renderQuestion(q) {
  const table = ui.root.querySelector("#dr-table");
  table.innerHTML = `
    ${questionHtml(q)}
    <div class="answers">${q.actions.map((a, i) => `<button data-i="${i}"><kbd>${i + 1}</kbd>${esc(a.text)}</button>`).join("")}</div>
    <div id="dr-result"></div>`;
  table.querySelector(".answers").onclick = (e) => {
    const i = e.target.closest("button")?.dataset.i;
    if (i !== undefined && !q.answered) answer(+i);
  };
}

function answer(choice) {
  const q = current;
  if (!q || q.answered || q.summary) return;
  q.answered = true;
  q.choice = choice;
  const best = Math.max(...q.ev);
  q.bestIndex = q.ev.indexOf(best);
  const lossBb = Math.max(0, best - q.ev[choice]);
  const g = grade(lossBb, q.pot, q.freq[choice]);
  q.lossBb = lossBb;
  q.grade = g;
  record(q, choice, lossBb, g);
  if (q.src === "live" && !q.hand.single) continueLive(q, choice, g, lossBb);

  ui.root.querySelectorAll("#dr-table .answers button").forEach((b, i) => {
    b.disabled = true;
    if (i === choice) b.classList.add("chosen");
  });
  renderResult(q);
  renderStats();
  renderSide();
  renderMistakeCount();
}

/** Puts your answer into the hand being played. */
function continueLive(q, choice, g, lossBb) {
  const { hand, node, row } = q;
  // After an action the solver never takes with this hand it has no strategy for it, so the
  // hand goes on with one the solver does take. A fold ends the hand, so it always stands.
  let step = choice;
  if (node.actions[choice].type !== "fold" && Math.max(row[0], 5e-4) * q.freq[choice] < 2e-4) {
    step = sampleIndex(q.freq);
    q.goesOnWith = node.actions[step];
  }
  hand.decisions.push({ street: streetOf(node.board), chose: q.actions[choice].text, best: q.actions[q.bestIndex].text, lossBb, grade: g });
  playStep(hand, node, step, "hero", { grade: g, chosen: step === choice ? null : node.actions[choice] });
}

function renderResult(q) {
  const { choice, lossBb, grade: g } = q;
  const lossPct = q.pot > 0 ? (lossBb / q.pot) * 100 : 0;
  const best = q.ev[q.bestIndex];
  const sign = q.src === "preflop";
  const evText = (x) => `${sign && x >= 0 ? "+" : ""}${x.toFixed(2)}bb`;
  const gto = q.gtoFreq !== undefined;
  const rows = q.actions.map((a, i) => `<tr class="${i === choice ? "chosen" : ""}">
      <td><i style="background:${a.color}"></i>${esc(a.text)}</td>
      <td><div class="mini"><span style="width:${q.freq[i] * 100}%;background:${a.color}"></span></div>${pct(q.freq[i])}</td>
      ${gto ? `<td class="gtocol">${q.gtoFreq ? pct(q.gtoFreq[i]) : "–"}</td>` : ""}
      <td class="${i === q.bestIndex ? "best" : ""}">${evText(q.ev[i])}</td>
      <td>${(best - q.ev[i]).toFixed(2)}</td></tr>`).join("");
  let why = "";
  if (g.cls === "good") {
    why = lossBb < 0.005 ? (q.freq[choice] > 0.98 ? "The solver always plays this." : "The best play.")
      : `The solver plays this ${pct(q.freq[choice])} of the time.`;
  } else {
    why = `The best play is ${esc(q.actions[q.bestIndex].text)}${q.freq[q.bestIndex] < 0.98 ? `, mixed with ${esc(mixText(q))}` : ""}.`;
  }
  const liveHand = q.src === "live" && !q.hand.single;
  const next = liveHand ? "Continue" : "Next hand";
  const links = [];
  if (q.src === "preflop" && ui.hooks.onOpenPreflop) links.push(`<button id="dr-open" class="link">Open in the Preflop solver</button>`);
  if (q.src === "flops" && ui.hooks.onOpenStudy) links.push(`<button id="dr-open" class="link">Open in Study</button>`);
  if (q.src === "live" && ui.hooks.onOpenSolver) links.push(`<button id="dr-open" class="link">Open in the Postflop solver</button>`);
  const who = q.src === "preflop" ? pf.SEATS[q.spot.seat] : esc(q.names[q.hero]);

  ui.root.querySelector("#dr-result").innerHTML = `
    <div class="verdict ${g.cls}"><b>${g.label}</b> · EV lost ${lossBb.toFixed(2)}bb (${lossPct.toFixed(1)}% of pot). ${why}</div>
    <table class="combos result">
      <thead><tr><th>Action</th><th>${gto ? "Exploit" : "Solver frequency"}</th>${gto ? "<th>GTO</th>" : ""}<th>EV</th><th>Loss</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
    ${gtoNote(q)}
    ${q.goesOnWith ? `<p class="meta">The solver never plays this hand that way here, so it has no strategy for you after it.
      The hand goes on as if you'd ${actVerb(q.goesOnWith, true)}.</p>` : ""}
    <p class="hint">${q.evNote} Loss is the gap to the best action.</p>
    <div class="next">
      <button id="dr-next" class="primary">${next} <kbd>Space</kbd></button>
      ${links.join("")}
    </div>
    <details class="drillrange" id="dr-range" ${ui.rangeOpen ? "open" : ""}>
      <summary>How ${who} plays every hand here</summary>
      <div id="dr-rangebody"></div>
    </details>`;
  const res = ui.root.querySelector("#dr-result");
  res.querySelector("#dr-next").onclick = () => (liveHand ? nextLiveStep(q) : deal());
  res.querySelector("#dr-open")?.addEventListener("click", () => openElsewhere(q));
  const det = res.querySelector("#dr-range");
  det.ontoggle = () => {
    ui.rangeOpen = det.open;
    if (det.open) renderRange(q);
  };
  if (ui.rangeOpen) renderRange(q);
}

/** Against locks or a player type: how the best play differs from GTO with this hand. */
function gtoNote(q) {
  if (q.gtoFreq === undefined) return "";
  if (!q.gtoFreq) return `<p class="vsgto">GTO never has this hand here: it plays it differently earlier in the hand.</p>`;
  const gap = q.freq.reduce((s, f, i) => s + Math.abs(f - q.gtoFreq[i]), 0) / 2;
  if (gap < 0.2) return `<p class="vsgto">GTO plays this hand about the same way here.</p>`;
  const mix = (f) => q.actions.map((a, i) => [a.text, f[i]]).filter(([, x]) => x >= 0.05).sort((a, b) => b[1] - a[1])
    .map(([t, x]) => `${esc(t)} ${pct(x)}`).join(", ");
  const vs = q.hand.typed ? esc(types.typeNoun(q.hand.typed)) : "your locks";
  return `<p class="vsgto">Exploiting ${vs} changes this hand: ${mix(q.freq)}, where GTO plays ${mix(q.gtoFreq)}.</p>`;
}

/** "Call 40%, Raise to 16.2 10%": the other actions the solver mixes in. */
function mixText(q) {
  return q.actions.map((a, i) => (i !== q.bestIndex && q.freq[i] >= 0.02 ? `${a.text} ${pct(q.freq[i])}` : null)).filter(Boolean).join(", ") || "nothing else";
}

async function nextLiveStep(q) {
  const ticket = ++ui.dealing;
  current = null;
  dealingHint(ticket);
  let next;
  try {
    next = await advanceLive(q.hand);
  } catch (e) {
    next = { message: `Lost the hand: ${e.message === "no-server" ? "the solver server isn't running." : e.message}` };
  }
  if (ticket !== ui.dealing) return;
  ui.busy = 0;
  waiting(false);
  if (!next) return deal();
  if (next.message) {
    current = null;
    ui.root.querySelector("#dr-table").innerHTML = `<p class="hint">${esc(next.message)}</p>`;
    return;
  }
  show(next);
}

function openElsewhere(q) {
  const cls = handClass(q.combo);
  if (q.src === "preflop") ui.hooks.onOpenPreflop({ pack: q.packId, history: q.spot.history, cls });
  else if (q.src === "flops") ui.hooks.onOpenStudy(q.spot.id, q.flop, q.node.path, cls);
  else {
    const path = q.key.history.map((step, i) => ({ step, label: q.key.labels[i] }));
    ui.hooks.onOpenSolver({ path, solution: q.hand.solution, cls });
  }
}

// ---- The whole range at the decision -------------------------------------------------------

function renderRange(q) {
  const body = ui.root.querySelector("#dr-rangebody");
  if (!body) return;
  if (q.src === "preflop") return renderPreflopRange(body, q);
  body.innerHTML = `${modeTabsHtml("dr-rmode")}<div class="rangewrap"><div id="dr-grid"></div><div id="dr-cell"></div></div><p id="dr-rnote" class="meta"></p>`;
  const view = ui.rangeView;
  view.focus = handClass(q.combo);
  view.pinned = true;
  body.querySelectorAll("#dr-rmode button").forEach((b) => b.classList.toggle("active", b.dataset.mode === view.mode));
  // Against locks or a player type, the hand detail also shows the class in the GTO solve.
  const compare = q.hand?.solution === "exploit" && q.node.compare?.solution === "gto"
    ? { label: "GTO", node: { ...q.node, hands: q.node.compare.hands }, note: "The GTO row is the whole class in the GTO solve." }
    : null;
  const draw = () => renderNodeView(
    { grid: body.querySelector("#dr-grid"), detail: body.querySelector("#dr-cell"), range: body.querySelector("#dr-rnote") },
    q.node, view, { board: q.board, name: (p) => esc(q.names[p]), compare },
  );
  wireModeTabs(body.querySelector("#dr-rmode"), view, draw);
  draw();
}

function renderPreflopRange(body, q) {
  const { spot } = q;
  const { node, reach } = spot;
  body.innerHTML = `<div class="rangewrap"><div id="dr-grid"></div><div id="dr-cell"></div></div>
    <p class="meta">Bar height is how often each hand gets here; the colors are how it plays. ${pf.SEATS[spot.seat]} plays
      ${pct(pf.rangeCombos(reach) / 1326, 1)} of hands at this point.</p>`;
  let focus = pf.CLASSES[q.c];
  const detail = (cls) => {
    const el = body.querySelector("#dr-cell");
    const c = pf.CLASS_INDEX.get(cls);
    if (reach[c] < 1e-4) {
      el.innerHTML = `<h3>${cls}</h3><p class="hint">${pf.SEATS[spot.seat]} never gets here with ${cls}.</p>`;
      return;
    }
    const fr = pf.classFreqs(node, c);
    const ev = pf.classEvs(node, c);
    const best = Math.max(...ev);
    el.innerHTML = `<h3>${cls}${cls === pf.CLASSES[q.c] ? " <small>your hand</small>" : ""}</h3>
      <table class="combos"><thead><tr><th>Action</th><th>Freq</th><th>EV</th></tr></thead>
      <tbody>${node.a.map((a, i) => `<tr><td><i style="background:${pf.actionColor(node.a, i)}"></i>${esc(pfActionText(spot, i))}</td>
        <td>${pct(fr[i])}</td><td class="${ev[i] === best ? "best" : ""}">${ev[i] >= 0 ? "+" : ""}${ev[i].toFixed(2)}</td></tr>`).join("")}</tbody></table>`;
  };
  const draw = () => renderGrid(body.querySelector("#dr-grid"), {
    selected: focus,
    cell: (cls) => {
      const c = pf.CLASS_INDEX.get(cls);
      if (reach[c] < 1e-4) return null;
      return { fill: reach[c], segments: pf.classFreqs(node, c).map((f, i) => ({ color: pf.actionColor(node.a, i), frac: f })) };
    },
    onHover: (cls) => detail(cls),
    onClick: (cls) => {
      focus = cls;
      draw();
      detail(cls);
    },
  });
  body.querySelector("#dr-grid").onmouseleave = () => detail(focus);
  draw();
  detail(focus);
}

// ---- End of a hand played on your solve ----------------------------------------------------

function renderHandSummary(q) {
  const { hand } = q;
  const table = ui.root.querySelector("#dr-table");
  const hero = hand.hero;
  const villain = hand.villain;
  const last = hand.steps.at(-1);
  let ending = "";
  if (last?.type === "act" && last.action.type === "fold") {
    if (last.player !== hero) ending = `${esc(hand.names[villain])} folds. You win the pot.`;
    else ending = last.chosen ? "The hand ends there, since the solver folds your hand." : "You fold.";
  } else if (splitCards(hand.board).length === 5) {
    const sd = showdown(hand.combos[hero], hand.combos[villain], hand.board);
    ending = `${esc(hand.names[villain])} shows ${cardsHtml(hand.combos[villain], "tiny")}. ${
      sd.winner === 0 ? `Split pot, both with ${sd.names[0]}.` : sd.winner > 0 ? `You win with ${sd.names[0]}.` : `${esc(hand.names[villain])} wins with ${sd.names[1]}.`}`;
  }
  if (hand.lostTrack) ending = "The hand stops here: the solver's lines don't reach your hand any more.";
  const total = hand.decisions.reduce((s, d) => s + d.lossBb, 0);
  const rows = hand.decisions.map((d) => `<tr><td>${cap(d.street)}</td><td>${esc(d.chose)}</td><td>${esc(d.best)}</td>
    <td><span class="mark ${d.grade.cls}">${MARK[d.grade.cls]}</span> ${d.grade.label}</td><td>${d.lossBb.toFixed(2)}</td></tr>`).join("");
  table.innerHTML = `
    <p class="spotname">${esc(hand.names.oop)} vs ${esc(hand.names.ip)} · hand over</p>
    <div class="board">${cardsHtml(hand.board, "big")}</div>
    ${handLog(hand.steps, { hero, names: hand.names }, hand.status.board)}
    <p class="line">${ending}</p>
    <div class="hero"><span>You were <b>${esc(hand.names[hero])}</b> with</span><div class="hole">${cardsHtml(hand.combos[hero], "big")}</div></div>
    <table class="combos result">
      <thead><tr><th>Street</th><th>You</th><th>Best</th><th>Grade</th><th>EV lost</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
    <p class="meta">EV lost this hand: <b>${total.toFixed(2)}bb</b>. The result of one hand is luck; the EV you lose is what costs you over time.</p>
    <div class="next"><button id="dr-next" class="primary">Next hand <kbd>Space</kbd></button></div>`;
  table.querySelector("#dr-next").onclick = () => deal();
}

// ---- Stats, recent decisions and leaks -----------------------------------------------------

function statBlock(t) {
  const n = t?.n || 0;
  return `<div class="stats">
    <div><b>${n}</b><span>decisions</span></div>
    <div><b>${n ? Math.round((t.correct / n) * 100) : 0}%</b><span>correct</span></div>
    <div><b>${n ? (t.loss / n).toFixed(2) : "0.00"}</b><span>bb lost per decision</span></div>
    <div><b>${n ? (t.lossPct / n).toFixed(1) : "0.0"}%</b><span>of the pot lost per decision</span></div>
  </div>`;
}

function renderStats() {
  const el = ui.root.querySelector("#dr-stats");
  const name = SOURCES.find(([id]) => id === settings.source)[1];
  el.innerHTML = `
    <h3 class="statshead">This session <button class="link" id="dr-summary">Summary</button></h3>
    ${statBlock(session)}
    <h3 class="statshead">${name}, all time</h3>
    ${statBlock(data.totals[settings.source])}
    <p class="statlinks"><button class="link" id="dr-newsession">New session</button><button class="link" id="dr-reset">Reset all stats</button></p>`;
  el.querySelector("#dr-newsession").onclick = () => {
    session = newSession();
    renderStats();
  };
  el.querySelector("#dr-summary").onclick = () => renderSessionSummary();
  el.querySelector("#dr-reset").onclick = () => {
    if (!el.querySelector("#dr-reset").dataset.armed) {
      const b = el.querySelector("#dr-reset");
      b.dataset.armed = "1";
      b.textContent = "Click again to erase every stat and mistake";
      return;
    }
    data = emptyStore();
    session = newSession();
    saveStore();
    renderStats();
    renderSide();
    renderMistakeCount();
  };
}

function leakKey(e) {
  if (e.src === "preflop") return { id: `pre|${e.cat}|${e.seat}`, label: `${PF_CAT[e.cat]}, ${pf.SEATS[e.seat]}` };
  // Hands against a player type get their own rows: the best play there isn't GTO's.
  const vs = e.vs ? `|${e.vs}` : "";
  const name = e.vs ? ` · vs ${types.profileById(e.vs)?.name || e.vs}` : "";
  return { id: `post|${e.street}|${e.type}${vs}`, label: `${cap(e.street)}, ${POST_TYPES[e.type]}${name}` };
}

function leakRows(entries) {
  const map = new Map();
  for (const e of entries) {
    if (e.src === "preflop" ? e.cat === undefined : !e.street) continue;
    const { id, label } = leakKey(e);
    let r = map.get(id);
    if (!r) map.set(id, (r = { id, label, n: 0, ok: 0, loss: 0 }));
    r.n += 1;
    r.ok += e.ok ?? (e.grade === "Correct" ? 1 : 0);
    r.loss += e.loss;
  }
  return [...map.values()].sort((a, b) => b.loss - a.loss);
}

function leakTable(rows) {
  return `<table class="combos leaks"><thead><tr><th>Situation</th><th>Decisions</th><th>Correct</th><th>bb lost each</th><th>bb lost</th></tr></thead>
    <tbody>${rows.map((r) => `<tr><td>${esc(r.label)}</td><td>${r.n}</td>
      <td>${Math.round((r.ok / r.n) * 100)}%</td><td>${(r.loss / r.n).toFixed(2)}</td><td>${r.loss.toFixed(1)}</td></tr>`).join("")}</tbody></table>`;
}

function renderSide() {
  renderTabs();
  const el = ui.root.querySelector("#dr-sidebody");
  if (ui.side === "leaks") {
    const rows = leakRows(data.log);
    const most = Math.max(1e-9, ...rows.map((r) => r.loss));
    el.innerHTML = rows.length
      ? `<p class="hint">Your last ${data.log.length} decisions by situation, the ones costing you the most EV first. Click one to drill it.</p>
        <ol class="leaklist">${rows.map((r) => `<li data-leak="${r.id}" title="Drill this">
          <div><b>${esc(r.label)}</b><span>${r.loss >= 0.05 ? `−${r.loss.toFixed(1)}bb` : "0bb"}</span></div>
          <div class="freqbar"><span style="width:${(r.loss / most) * 100}%;background:var(--bad)"></span></div>
          <small>${r.n} decision${r.n === 1 ? "" : "s"} · ${Math.round((r.ok / r.n) * 100)}% correct · ${(r.loss / r.n).toFixed(2)}bb lost each</small>
        </li>`).join("")}</ol>`
      : `<p class="hint">Answer a few hands and the situations where you lose the most EV show up here.</p>`;
    el.querySelector(".leaklist")?.addEventListener("click", (e) => {
      const id = e.target.closest("li")?.dataset.leak;
      if (id) drillLeak(id);
    });
    return;
  }
  el.innerHTML = data.recent.length
    ? `<ol class="history">${data.recent.map((h) => `<li class="${h.cls}">
        <span>${cardsHtml(h.combo, "tiny")}${h.board ? ` on ${cardsHtml(h.board, "tiny")}` : ""}</span>
        <span class="meta">${esc(h.title)}</span>
        <span>${esc(h.chose)} · ${h.grade}${h.loss >= 0.005 ? `, −${h.loss.toFixed(2)}bb` : ""}</span></li>`).join("")}</ol>`
    : `<p class="hint">Your answers show up here. Keys 1 to 9 pick an action, Space deals the next hand.</p>`;
}

/** Sets up the drill for one situation from the leak report. */
function drillLeak(id) {
  const [kind, a, b, vs] = id.split("|");
  if (kind === "pre") {
    settings.source = "preflop";
    Object.assign(settings.preflop, { cats: [a], seat: pf.SEATS[+b], line: null });
  } else if (vs) {
    settings.source = "live";
    Object.assign(settings.live, { streets: [a], only: null, opponent: types.profileById(vs) ? vs : "exploit" });
  } else if (a === "flop") {
    settings.source = "flops";
    const map = { first: ["oop", "first"], checked: ["ip", "first"], bet: ["both", "facing"], raise: ["both", "facing"] };
    [settings.flops.seat, settings.flops.depth] = map[b];
  } else {
    settings.source = "live";
    Object.assign(settings.live, { streets: [a], only: null });
  }
  settings.mode = "new";
  saveSettings();
  refresh();
}

function renderSessionSummary() {
  const table = ui.root.querySelector("#dr-table");
  ++ui.dealing;
  waiting(false);
  const ds = session.decisions;
  if (!ds.length) {
    current = null;
    table.innerHTML = `<p class="hint">No decisions yet this session.</p><div class="next"><button id="dr-next" class="primary">Deal a hand <kbd>Space</kbd></button></div>`;
    table.querySelector("#dr-next").onclick = () => deal();
    current = { summary: true };
    return;
  }
  const worst = ds.filter((d) => d.cls !== "good").sort((a, b) => b.loss - a.loss).slice(0, 8);
  const grades = ["Correct", "Small mistake", "Mistake", "Blunder"];
  current = { summary: true };
  table.innerHTML = `
    <p class="spotname">Session summary</p>
    ${statBlock(session)}
    <div class="gradebar">${grades.map((g) => `<span class="g-${g.replace(" ", "-").toLowerCase()}">${session.counts[g] || 0} ${g.toLowerCase()}${(session.counts[g] || 0) === 1 || g === "Correct" ? "" : "s"}</span>`).join("")}</div>
    <p class="meta">You lost <b>${session.loss.toFixed(2)}bb</b> in EV over ${ds.length} decisions${ds.length >= 20 ? `, about ${((session.loss / ds.length) * 100).toFixed(0)}bb per 100 decisions` : ""}.</p>
    ${worst.length ? `<h3>Your costliest decisions</h3>
      <table class="combos"><thead><tr><th>Hand</th><th>Spot</th><th>You</th><th>Best</th><th>EV lost</th></tr></thead>
      <tbody>${worst.map((d) => `<tr><td>${cardsHtml(d.combo, "tiny")}${d.board ? ` ${cardsHtml(d.board, "tiny")}` : ""}</td><td>${esc(d.title)}</td>
        <td>${esc(d.chose)}</td><td>${esc(d.best)}</td><td>${d.loss.toFixed(2)}</td></tr>`).join("")}</tbody></table>` : `<p>No mistakes this session.</p>`}
    <h3>By situation</h3>
    ${leakTable(leakRows(ds.map((d) => ({ src: d.src, ...d.leak, loss: d.loss, grade: d.grade }))))}
    <div class="next"><button id="dr-next" class="primary">Keep drilling <kbd>Space</kbd></button></div>`;
  table.querySelector("#dr-next").onclick = () => deal();
}
