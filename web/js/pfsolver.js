// Preflop solver view: 6-max preflop strategies for every seat, from a precomputed solution in
// data/preflop/ or from a solve with your own sizes, stacks and rake on the local pt-server.
// Step through the hand seat by seat; the 13×13 grid shows how every hand class plays, and
// every flop the hand can reach shows the ranges, ready to open in the postflop Solver.

import { renderGrid } from "./grid.js";
import {
  SEATS, CLASS_INDEX, COMBOS, walk, classFreqs, classEvs, actionFrequencies, actionColor, rangeCombos, rangeString, lineText,
  configSummary,
} from "./pfpack.js";

const state = {
  packs: [],
  source: null, // a pack id, or "server" for the last solve on pt-server
  pack: null,
  history: [],
  at: null, // walk() of history
  mode: "strategy", // or "ev"
  rangeSeat: null, // show this seat's range in the grid instead of the strategy
  focus: null,
  pinned: false,
  server: null, // the server's preflop status, or null without a server
  polling: null,
  onOpenSolver: null,
  onDrill: null,
};

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const pct = (x, d = 1) => `${(x * 100).toFixed(d)}%`;
const bb = (x) => `${Math.round(x * 100) / 100}bb`;
// Red (losing) to green (winning), for EVs in bb.
const heat = (ev) => `hsl(${Math.round(65 + 65 * Math.tanh(ev / 1.5))} 60% 46%)`;
const STORE = "pokertrainer.pfsolver.v1";

async function api(path, body) {
  const res = await fetch(`api/preflop/${path}`, body === undefined ? {} : {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  let data;
  try {
    data = await res.json();
  } catch {
    throw new Error(res.status === 404 ? "no-server" : `HTTP ${res.status}`);
  }
  if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
  return data;
}

export async function initPreflopSolver(root, { onOpenSolver, onDrill }) {
  state.onOpenSolver = onOpenSolver;
  state.onDrill = onDrill;
  const index = await fetch("data/preflop/index.json").then((r) => (r.ok ? r.json() : { packs: [] })).catch(() => ({ packs: [] }));
  state.packs = index.packs || [];
  state.server = await api("status").catch(() => null);

  root.innerHTML = `
    <div class="study pfsolver">
      <aside class="panel controls">
        <label>Solution <select id="pf-source"></select></label>
        <p id="pf-about" class="meta"></p>
        <label>Jump to <select id="pf-jump"></select></label>
        <div id="pf-ev"></div>
        ${state.server ? `<details id="pf-setup"><summary>New solve</summary>${formHtml()}</details>` : `
          <p class="hint">To solve your own sizes, stacks and rake, run the local server
          (<code>cargo run --release --bin pt-server</code> in <code>solver/</code>) and open the app from it.</p>`}
        <div id="pf-progress"></div>
      </aside>
      <section class="panel" id="pf-main"><p class="hint">Loading…</p></section>
      <aside class="panel detail" id="pf-detail"></aside>
    </div>`;

  root.querySelector("#pf-source").onchange = (e) => loadSource(root, e.target.value);
  root.querySelector("#pf-jump").onchange = (e) => {
    if (e.target.value) go(root, JSON.parse(e.target.value));
    e.target.value = "";
  };
  if (state.server) {
    root.querySelector("#pf-solve").onclick = () => startSolve(root);
    root.querySelector("#pf-limp").onchange = () => syncForm(root);
  }

  let saved = null;
  try {
    saved = JSON.parse(localStorage.getItem(STORE) || "null");
  } catch {}
  const hasServerSolution = !!state.server?.has_solution;
  const first = saved?.source === "server" && hasServerSolution ? "server"
    : state.packs.find((p) => p.id === saved?.source)?.id || (hasServerSolution ? "server" : state.packs[0]?.id);
  renderSources(root, first);
  if (first) await loadSource(root, first, saved?.source === first ? saved.history : []);
  else root.querySelector("#pf-main").innerHTML = `<p class="hint">No preflop solutions found in <code>data/preflop/</code>.${state.server ? " Set one up under New solve." : ""}</p>`;
  if (state.server && ["tables", "solving"].includes(state.server.state)) handleStatus(root, state.server);
}

/** Shows one decision of a chart (`source` is a pack id or "server"), with hand class `cls` pinned. */
export async function openPreflopAt(root, source, history, cls) {
  if (!root.querySelector("#pf-source")) return;
  if (source === "server") state.server = await api("status").catch(() => state.server);
  if (source !== "server" && !state.packs.some((p) => p.id === source)) return;
  if (source === "server" && !state.server?.has_solution) return;
  if (cls) {
    state.focus = cls;
    state.pinned = true;
  }
  renderSources(root, source);
  await loadSource(root, source, history);
}

function renderSources(root, selected) {
  const opts = state.packs.map((p) => `<option value="${esc(p.id)}">${esc(p.name)}</option>`);
  if (state.server?.has_solution) opts.push(`<option value="server">Your last solve</option>`);
  const sel = root.querySelector("#pf-source");
  sel.innerHTML = opts.join("");
  if (selected) sel.value = selected;
}

async function loadSource(root, id, history = []) {
  const main = root.querySelector("#pf-main");
  main.innerHTML = `<p class="hint">Loading…</p>`;
  try {
    if (id === "server") {
      state.pack = await fetch("api/preflop/solution").then((r) => (r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`))));
    } else {
      const meta = state.packs.find((p) => p.id === id);
      state.pack = await fetch(`data/preflop/${meta.file}`).then((r) => (r.ok ? r.json() : Promise.reject(new Error(`${meta.file}: HTTP ${r.status}`))));
    }
  } catch (e) {
    main.innerHTML = `<p class="error">Couldn't load the solution: ${esc(e.message)}</p>`;
    return;
  }
  state.source = id;
  const pack = state.pack;
  const meta = state.packs.find((p) => p.id === id);
  root.querySelector("#pf-about").innerHTML = `${esc(meta?.description || configSummary(pack.config))}<br>
    ${pack.iterations} iterations, exploitability ${pack.exploitability_mbb} mbb/hand.`;
  renderEv(root);
  renderJumps(root);
  if (state.server) fillForm(root, pack.config);
  go(root, walk(pack, history) ? history : []);
}

function renderEv(root) {
  const pack = state.pack;
  const rows = SEATS.map((s, i) => `<tr><td>${s}</td><td>${pack.ev[i] >= 0 ? "+" : ""}${pack.ev[i].toFixed(3)}</td></tr>`).join("");
  root.querySelector("#pf-ev").innerHTML = `<table class="combos evtable"><thead><tr><th>Seat</th><th>EV (bb/hand)</th></tr></thead><tbody>${rows}</tbody></table>`;
}

/** Follows (seat, kind) steps, folding every seat in between. */
function findLine(pack, steps) {
  const history = [];
  let id = 0;
  for (const [seat, kind] of steps) {
    for (;;) {
      const n = pack.nodes[id];
      if (!n || n.t || n.cut) return null;
      const i = n.s === seat ? n.a.findIndex((a) => a[0] === kind) : n.a.findIndex((a) => a[0] === "fold");
      if (i < 0) return null;
      history.push(i);
      id = n.c[i];
      if (n.s === seat) break;
    }
  }
  return history;
}

/** The decision where `seat` first acts after `steps` (folding everyone in between). */
function lineTo(pack, steps, seat) {
  const h = findLine(pack, [...steps, [seat, "fold"]]);
  return h ? h.slice(0, -1) : null;
}

function renderJumps(root) {
  const pack = state.pack;
  const groups = [];
  const add = (group, label, history) => {
    if (!history) return;
    let g = groups.find((x) => x.label === group);
    if (!g) groups.push((g = { label: group, items: [] }));
    g.items.push(`<option value="${JSON.stringify(history)}">${label}</option>`);
  };
  for (let s = 0; s <= 4; s++) add("Open", `${SEATS[s]} first in`, lineTo(pack, [], s));
  for (let o = 0; o <= 4; o++) {
    for (let r = o + 1; r <= 5; r++) add(`Against a ${SEATS[o]} open`, `${SEATS[r]} vs ${SEATS[o]} open`, lineTo(pack, [[o, "raise"]], r));
  }
  for (let o = 0; o <= 4; o++) {
    for (let r = o + 1; r <= 5; r++) add("Against a 3-bet", `${SEATS[o]} open, ${SEATS[r]} 3-bets`, lineTo(pack, [[o, "raise"], [r, "raise"]], o));
  }
  root.querySelector("#pf-jump").innerHTML = `<option value="">Pick a spot…</option>` +
    groups.map((g) => `<optgroup label="${g.label}">${g.items.join("")}</optgroup>`).join("");
}

function go(root, history) {
  const at = walk(state.pack, history);
  if (!at) return;
  state.history = history;
  state.at = at;
  state.rangeSeat = null;
  if (!state.pinned) state.focus = null;
  try {
    localStorage.setItem(STORE, JSON.stringify({ source: state.source, history }));
  } catch {}
  render(root);
}

function render(root) {
  const { node } = state.at;
  const main = root.querySelector("#pf-main");
  main.innerHTML = `
    <div class="seats" id="pf-seats"></div>
    <nav class="path" id="pf-path"></nav>
    <div id="pf-here"></div>
    <div id="pf-body"></div>`;
  renderSeats(root);
  renderPath(root);
  if (node.cut) {
    root.querySelector("#pf-here").innerHTML = `<p class="hint">This line is too rare to be kept in the precomputed solution.${
      state.server ? " Solve these settings under New solve to see it." : " Run pt-server and solve these settings to see it."}</p>`;
    root.querySelector("#pf-detail").innerHTML = "";
    return;
  }
  if (node.t === "fold") return renderFold(root);
  if (node.t === "flop") return renderFlop(root);
  renderDecision(root);
}

function seatStatus(s) {
  const { node, folded, invested } = state.at;
  const allin = invested[s] >= state.pack.config.stack_bb - 1e-9;
  if (folded.has(s)) return ["folded", "Folded"];
  if (node.t === "fold") return ["won", "Wins"];
  if (!node.t && node.s === s) return ["acting", "To act"];
  if (allin) return ["in", "All-in"];
  return ["in", invested[s] ? `In ${bb(invested[s])}` : ""];
}

function renderSeats(root) {
  const { invested } = state.at;
  const el = root.querySelector("#pf-seats");
  el.innerHTML = SEATS.map((name, s) => {
    const [cls, text] = seatStatus(s);
    const shown = state.rangeSeat === s ? " shown" : "";
    return `<button class="seat ${cls}${shown}" data-seat="${s}" ${cls === "folded" ? "disabled" : ""} title="Show ${name}'s range">
      <b>${name}</b><span>${text}</span><small>${bb(state.pack.config.stack_bb - invested[s])} behind</small></button>`;
  }).join("");
  el.onclick = (e) => {
    const s = e.target.closest(".seat")?.dataset.seat;
    if (s === undefined || state.at.node.t) return;
    state.rangeSeat = state.rangeSeat === +s || +s === state.at.node.s ? null : +s;
    renderSeats(root);
    drawDecisionGrid(root);
  };
}

function renderPath(root) {
  const crumbs = [`<a data-depth="0">Start</a>`, ...state.at.line.map((st, i) => `<a data-depth="${i + 1}">${SEATS[st.seat]} ${esc(st.label)}</a>`)];
  const el = root.querySelector("#pf-path");
  el.innerHTML = crumbs.join(" › ");
  el.onclick = (e) => {
    const d = e.target.dataset.depth;
    if (d !== undefined) go(root, state.history.slice(0, +d));
  };
}

const pot = () => state.at.invested.reduce((a, b) => a + b, 0);

function renderDecision(root) {
  const { node, reach, bet, invested } = state.at;
  const freq = actionFrequencies(node, reach[node.s]);
  const toCall = bet - invested[node.s];
  root.querySelector("#pf-here").innerHTML = `
    <p class="who"><b>${SEATS[node.s]}</b> to act · pot ${bb(pot())}${toCall > 0 ? ` · ${bb(toCall)} to call` : ""} ·
      plays ${pct(rangeCombos(reach[node.s]) / 1326)} of hands here</p>
    <div class="freqbar">${node.a.map((a, i) => `<span style="width:${freq[i] * 100}%;background:${actionColor(node.a, i)}"></span>`).join("")}</div>
    <div class="actions">${node.a.map((a, i) => `<button data-i="${i}"><i style="background:${actionColor(node.a, i)}"></i>${
      esc(stepLabel(i))}<em>${pct(freq[i])}</em></button>`).join("")}</div>
    ${state.onDrill ? `<div class="drillbar"><button class="secondary" id="pf-drill">Drill this spot</button>
      <span class="meta">Deals you hands here and scores your answers.</span></div>` : ""}`;
  root.querySelector("#pf-here .actions").onclick = (e) => {
    const i = e.target.closest("button")?.dataset.i;
    if (i !== undefined) go(root, [...state.history, +i]);
  };
  const drillBtn = root.querySelector("#pf-drill");
  if (drillBtn) drillBtn.onclick = () => state.onDrill({ preflop: { pack: state.source, history: state.history.slice() } });
  root.querySelector("#pf-body").innerHTML = `
    <div class="tabs" id="pf-mode">
      <button data-mode="strategy">Strategy</button><button data-mode="ev">EV</button>
    </div>
    <div id="pf-grid"></div>
    <p id="pf-range" class="meta"></p>`;
  const tabs = root.querySelector("#pf-mode");
  tabs.onclick = (e) => {
    if (!e.target.dataset.mode) return;
    state.mode = e.target.dataset.mode;
    drawDecisionGrid(root);
  };
  drawDecisionGrid(root);
}

/** The label of action `i` at the current decision, as it reads in the line. */
function stepLabel(i) {
  const [kind, to] = state.at.node.a[i];
  const level = state.at.level;
  return { fold: "Fold", check: "Check", call: level === 0 ? "Limp" : "Call", raise: `Raise to ${bb(to)}`, allin: `All-in ${bb(to)}` }[kind];
}

function drawDecisionGrid(root) {
  const { node, reach } = state.at;
  root.querySelectorAll("#pf-mode button").forEach((b) => b.classList.toggle("active", !state.rangeSeat && b.dataset.mode === state.mode));
  const gridEl = root.querySelector("#pf-grid");
  const note = root.querySelector("#pf-range");
  if (state.rangeSeat !== null) {
    const r = reach[state.rangeSeat];
    renderRangeGrid(gridEl, r);
    note.innerHTML = `${SEATS[state.rangeSeat]}'s range at this point: ${pct(rangeCombos(r) / 1326)} of hands. Click the seat again to go back to ${SEATS[node.s]}'s strategy.`;
    renderDetail(root);
    return;
  }
  const r = reach[node.s];
  renderGrid(gridEl, {
    selected: state.pinned ? state.focus : null,
    cell: (cls) => {
      const c = CLASS_INDEX.get(cls);
      if (r[c] < 1e-4) return null;
      const fr = classFreqs(node, c);
      if (state.mode === "ev") {
        const ev = classEvs(node, c).reduce((s, v, i) => s + v * fr[i], 0);
        return { fill: r[c], segments: [{ color: heat(ev), frac: 1 }], label: ev.toFixed(1) };
      }
      return { fill: r[c], segments: fr.map((f, i) => ({ color: actionColor(node.a, i), frac: f })) };
    },
    onHover: (cls) => {
      if (!state.pinned) {
        state.focus = cls;
        renderDetail(root);
      }
    },
    onClick: (cls) => {
      state.pinned = !(state.pinned && state.focus === cls);
      state.focus = cls;
      drawDecisionGrid(root);
    },
  });
  note.textContent = state.mode === "ev"
    ? "EV in bb of each hand's strategy, compared with folding now. Bar height is how often the hand gets here."
    : "Bar height is how often each hand gets here; the colors are how it plays.";
  renderDetail(root);
}

/** A range as a grid: each cell filled by the share of the class's combos in it. */
function renderRangeGrid(el, r, color = "var(--act-range)") {
  renderGrid(el, {
    cell: (cls) => {
      const c = CLASS_INDEX.get(cls);
      return r[c] >= 0.0005 ? { fill: 1, segments: [{ color, frac: r[c] }] } : null;
    },
  });
}

function renderDetail(root) {
  const el = root.querySelector("#pf-detail");
  const { node, reach } = state.at;
  const cls = state.focus;
  if (!cls) {
    el.innerHTML = `<h3>Hand detail</h3><p class="hint">Hover a cell to see how often each action is taken and what it's worth. Click to pin it.</p>`;
    return;
  }
  const c = CLASS_INDEX.get(cls);
  const r = reach[node.s][c];
  if (r < 1e-4) {
    el.innerHTML = `<h3>${cls}</h3><p class="hint">${SEATS[node.s]} never gets here with ${cls}.</p>`;
    return;
  }
  const fr = classFreqs(node, c);
  const ev = classEvs(node, c);
  const best = Math.max(...ev);
  el.innerHTML = `
    <h3>${cls} <small>${(COMBOS[c] * r).toFixed(1)} of ${COMBOS[c]} combos get here</small></h3>
    <table class="combos">
      <thead><tr><th>Action</th><th>Frequency</th><th>EV</th></tr></thead>
      <tbody>${node.a.map((a, i) => `<tr><td><i style="background:${actionColor(node.a, i)}"></i>${esc(stepLabel(i))}</td>
        <td>${pct(fr[i], 0)}</td><td class="${ev[i] === best ? "best" : ""}">${ev[i] >= 0 ? "+" : ""}${ev[i].toFixed(2)}bb</td></tr>`).join("")}</tbody>
    </table>
    <p class="hint">EV is in bb, compared with folding now (what's already in the pot is gone either way).</p>`;
}

function renderFold(root) {
  const { node } = state.at;
  root.querySelector("#pf-here").innerHTML = `<p class="who"><b>${SEATS[node.s]}</b> wins the pot of ${bb(pot())}. Everyone else folded.</p>`;
  root.querySelector("#pf-detail").innerHTML = "";
}

function renderFlop(root) {
  const { node, reach, invested, line } = state.at;
  const players = node.p;
  const stack = state.pack.config.stack_bb - invested[players[0]];
  const allin = node.spr === 0;
  const kind = line.filter((s) => s.kind === "raise" || s.kind === "allin").length;
  const potName = kind <= 1 ? (kind === 0 ? "limped pot" : "single-raised pot") : `${kind + 1}-bet pot`;
  root.querySelector("#pf-here").innerHTML = `
    <p class="who">${allin ? "All-in" : "Flop"}: <b>${players.map((s) => SEATS[s]).join(" vs ")}</b>, ${players.length === 3 ? "three-way " : ""}${potName} ·
      pot ${bb(node.pot)}${allin ? "" : ` · ${bb(stack)} behind (SPR ${node.spr})`}${node.rake ? ` · expected rake ${bb(node.rake)}` : ""}</p>
    <p class="meta">${esc(lineText(line))}. ${players.length === 3 || allin ? "" : `${SEATS[players[0]]} is out of position.`}</p>`;
  root.querySelector("#pf-body").innerHTML = `<div class="flopranges">${players.map((s) => `
    <div><h3>${SEATS[s]} <small>${pct(rangeCombos(reach[s]) / 1326)} of hands · ${rangeCombos(reach[s]).toFixed(0)} combos</small></h3>
    <div class="pf-flopgrid" data-seat="${s}"></div></div>`).join("")}</div>`;
  root.querySelectorAll(".pf-flopgrid").forEach((el) => renderRangeGrid(el, reach[+el.dataset.seat]));

  const canSolve = players.length === 2 && !allin;
  const detail = root.querySelector("#pf-detail");
  detail.innerHTML = `
    <h3>Play the flop</h3>
    ${canSolve ? `<p class="hint">Open this pot in the postflop Solver with these ranges, the pot, the stacks and the rake. Pick a board there and solve.</p>
      <button class="primary" id="pf-open">Open in postflop Solver</button>`
      : `<p class="hint">${allin ? "Everyone is all-in, so there's nothing left to play: equity decides." : "The postflop Solver plays heads-up pots only."}</p>`}
    ${players.map((s) => `<label>${SEATS[s]}'s range <textarea rows="4" readonly>${rangeString(reach[s])}</textarea></label>`).join("")}`;
  if (canSolve) {
    detail.querySelector("#pf-open").onclick = () => {
      const [oop, ip] = players;
      state.onOpenSolver({
        oop: SEATS[oop],
        ip: SEATS[ip],
        ranges: { oop: rangeString(reach[oop]), ip: rangeString(reach[ip]) },
        pot_bb: Math.round(node.pot * 100) / 100,
        stack_bb: Math.round(stack * 100) / 100,
        rake: state.pack.config.rake,
        pot_type: kind <= 1 ? "srp" : "3bp",
        description: lineText(line),
      });
    };
  }
}

// ---- New solve (needs pt-server) ----

const FIELDS = [
  ["stack_bb", "Stacks (bb)", 0.5],
  ["open_bb", "Open size (bb)", 0.25],
  ["sb_open_bb", "SB open size (bb)", 0.25],
  ["iso_bb", "BB raise over a limp (bb)", 0.25],
  ["threebet_ip", "3-bet in position (× open)", 0.1],
  ["threebet_oop", "3-bet out of position (× open)", 0.1],
  ["squeeze_per_caller", "Squeeze: extra opens per caller", 0.1],
  ["fourbet_ip", "4-bet in position (× 3-bet)", 0.1],
  ["fourbet_oop", "4-bet out of position (× 3-bet)", 0.1],
  ["fivebet", "5-bet (× 4-bet)", 0.1],
];

function formHtml() {
  const num = ([key, label, step]) => `<label>${label} <input id="pf-${key}" type="number" step="${step}" min="0"></label>`;
  return `
    <div class="row2">${FIELDS.slice(0, 2).map(num).join("")}</div>
    <div class="row2">${num(FIELDS[2])}${num(FIELDS[3])}</div>
    <label class="check"><input type="checkbox" id="pf-limp"> SB can limp</label>
    <div class="row2">${num(FIELDS[4])}${num(FIELDS[5])}</div>
    ${num(FIELDS[6])}
    <div class="row2">${num(FIELDS[7])}${num(FIELDS[8])}</div>
    ${num(FIELDS[9])}
    <label>Going all-in is an option from <select id="pf-allin_from">
      <option value="1">the open</option><option value="2">the 3-bet</option><option value="3">the 4-bet</option><option value="4">the 5-bet</option>
      <option value="5">never (only when a raise is most of the stack)</option></select></label>
    <label class="check"><input type="checkbox" id="pf-cold"> Players can cold-call 3-bets</label>
    <label>Players who can see a flop <select id="pf-maxp"><option value="2">2</option><option value="3">3</option></select></label>
    <div class="row2">
      <label>Rake (%) <input id="pf-rake" type="number" step="0.5" min="0"></label>
      <label>Rake cap (bb) <input id="pf-cap" type="number" step="0.5" min="0"></label>
    </div>
    <label>Postflop <select id="pf-postflop">
      <option value="calibrated">Calibrated to postflop solves</option><option value="equity">Raw equity (no position or playability)</option></select></label>
    <label>First to act <select id="pf-start">${SEATS.slice(0, 5).map((s) => `<option>${s}</option>`).join("")}</select></label>
    <div class="row2">
      <label>Max iterations <input id="pf-iters" type="number" step="100" min="10" value="1000"></label>
      <label>Target (mbb/hand) <input id="pf-target" type="number" step="0.05" min="0" value="0.25"></label>
    </div>
    <p class="hint">A 100bb solve takes about a minute on 4 cores. The first solve also builds the equity tables (about 30 seconds, once).</p>
    <button id="pf-solve" class="primary">Solve</button>`;
}

function fillForm(root, cfg) {
  const $ = (id) => root.querySelector(id);
  for (const [key] of FIELDS) $(`#pf-${key}`).value = cfg[key];
  $("#pf-limp").checked = !!cfg.sb_limp;
  $("#pf-allin_from").value = String(cfg.allin_from);
  $("#pf-cold").checked = !!cfg.cold_call_3bet;
  $("#pf-maxp").value = String(cfg.max_flop_players);
  $("#pf-rake").value = +((cfg.rake?.rate || 0) * 100).toFixed(2);
  $("#pf-cap").value = cfg.rake?.cap_bb || 0;
  $("#pf-postflop").value = cfg.postflop;
  $("#pf-start").value = String(cfg.start_seat || "UTG").toUpperCase();
  syncForm(root);
}

function syncForm(root) {
  root.querySelector("#pf-iso_bb").disabled = !root.querySelector("#pf-limp").checked;
}

function readForm(root) {
  const $ = (id) => root.querySelector(id);
  const config = { ...state.pack?.config };
  for (const [key, label] of FIELDS) {
    const v = parseFloat($(`#pf-${key}`).value);
    if (!Number.isFinite(v)) throw new Error(`${label}: enter a number.`);
    config[key] = v;
  }
  config.sb_limp = $("#pf-limp").checked;
  config.allin_from = +$("#pf-allin_from").value;
  config.cold_call_3bet = $("#pf-cold").checked;
  config.max_flop_players = +$("#pf-maxp").value;
  config.rake = { rate: (parseFloat($("#pf-rake").value) || 0) / 100, cap_bb: parseFloat($("#pf-cap").value) || 0 };
  config.postflop = $("#pf-postflop").value;
  config.start_seat = $("#pf-start").value;
  return {
    config,
    max_iters: parseInt($("#pf-iters").value, 10) || 1000,
    target_mbb: Math.max(0, parseFloat($("#pf-target").value) || 0),
  };
}

async function startSolve(root) {
  const progress = root.querySelector("#pf-progress");
  try {
    await api("solve", readForm(root));
  } catch (e) {
    progress.innerHTML = `<p class="error">${esc(e.message)}</p>`;
    return;
  }
  handleStatus(root, await api("status"));
}

function handleStatus(root, s) {
  state.server = s;
  const progress = root.querySelector("#pf-progress");
  clearTimeout(state.polling);
  if (s.state === "tables" || s.state === "solving") {
    const frac = s.state === "tables" ? s.tables_progress : Math.max(s.iteration / Math.max(1, s.max_iters),
      s.exploitability_mbb ? Math.min(1, s.target_mbb / Math.max(s.exploitability_mbb, 1e-9)) : 0);
    progress.innerHTML = `
      <div class="progress"><span style="width:${frac * 100}%"></span></div>
      <p class="meta">${s.state === "tables" ? `Building the equity tables (once): ${pct(s.tables_progress, 0)}` :
        `Solving · iteration ${s.iteration} of ${s.max_iters} · ${s.exploitability_mbb == null ? "measuring exploitability…" :
          `exploitability ${s.exploitability_mbb.toFixed(2)} mbb/hand (target ${s.target_mbb})`} · ${Math.round(s.seconds)}s`}</p>
      <button id="pf-stop" class="link">Stop and use the current strategy</button>`;
    progress.querySelector("#pf-stop").onclick = () => api("stop", {});
    state.polling = setTimeout(async () => {
      try {
        handleStatus(root, await api("status"));
      } catch (e) {
        progress.innerHTML = `<p class="error">Lost the server: ${esc(e.message)}</p>`;
      }
    }, 1000);
    return;
  }
  if (s.state === "done" || s.state === "stopped") {
    progress.innerHTML = `<p class="meta">${s.state === "done" ? "Solved" : "Stopped"} after ${s.iteration} iterations in ${Math.round(s.seconds)}s,
      exploitability ${s.exploitability_mbb?.toFixed(2)} mbb/hand.</p>`;
    if (state.source !== "server" || state.loadedServerRun !== s.seconds) {
      state.loadedServerRun = s.seconds;
      renderSources(root, "server");
      const setup = root.querySelector("#pf-setup");
      if (setup) setup.open = false;
      loadSource(root, "server", state.source === "server" ? state.history : []);
    }
    return;
  }
  if (s.state === "failed") progress.innerHTML = `<p class="error">${esc(s.error || "The solve failed.")}</p>`;
}
