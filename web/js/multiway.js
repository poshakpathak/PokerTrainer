// Multiway solver: set up a three-way pot, solve it on the local pt-server, and browse every line
// of the tree (flop, turn and river) with each player's strategy, range, EV and equity.

import { cardsHtml, splitCards, RANKS } from "./cards.js";
import { nodeHands, rangeFrequencies, actionText, actionColor, actionLabel } from "./data.js";
import { renderNodeView, modeTabsHtml, wireModeTabs } from "./nodeview.js";

const STREETS = ["flop", "turn", "river"];
const SEATS = [0, 1, 2];
const state = {
  presets: [],
  spot: null, // the spot of the solve on the server
  board: "",
  path: [], // [{step, label}] from the start of the solve to the node shown
  node: null,
  status: null,
  resume: null, // path to reopen when the running solve finishes
  polling: null,
  goSeq: 0, // counts navigations, so a slow answer can't replace a later one
  sizeSeq: 0,
  sizeTimer: null,
};
const view = { mode: "strategy", focus: null, pinned: false };
const chanceSort = { key: "card", dir: 1 };

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const pct = (x, digits = 1) => `${(x * 100).toFixed(digits)}%`;
const cap = (s) => s[0].toUpperCase() + s.slice(1);
const historyOf = (path) => path.map((p) => p.step);
const names = () => state.spot?.players || ["P1", "P2", "P3"];

async function api(path, body) {
  const res = await fetch(`api/multiway/${path}`, body === undefined ? {} : {
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

export async function initMultiway(root) {
  let status;
  try {
    status = await api("status");
  } catch {
    root.innerHTML = `
      <div class="panel narrow">
        <h3>Start the solver server</h3>
        <p>The Multiway solver runs solves on your own computer through a small local server. From the repo:</p>
        <pre><code>cd solver
cargo run --release --bin pt-server</code></pre>
        <p>Then open <a href="http://127.0.0.1:8080">http://127.0.0.1:8080</a>. Everything else in the app works from there too.</p>
      </div>`;
    return;
  }
  state.presets = await fetch("data/multiway/presets.json")
    .then((r) => (r.ok ? r.json() : { spots: [] }))
    .then((d) => d.spots || [])
    .catch(() => []);

  root.innerHTML = `
    <div class="study solver multiway">
      <aside class="panel controls">
        <details id="mw-setup" open>
          <summary>Spot setup</summary>
          <label>Start from <select id="mw-preset">
            ${state.presets.map((s) => `<option value="${s.id}">${esc(s.name)}</option>`).join("")}
          </select></label>
          <p class="hint">Players in the order they act after the flop.</p>
          ${SEATS.map((i) => `
            <label>Player ${i + 1} <input id="mw-name-${i}"></label>
            <label>${i === 0 ? "Range (first to act)" : i === 2 ? "Range (last to act)" : "Range"} <textarea id="mw-range-${i}" rows="3"></textarea></label>`).join("")}
          <label>Board (3 to 5 cards) <input id="mw-board" placeholder="Ks7d2c"></label>
          <div class="row2">
            <label>Pot (bb) <input id="mw-pot" type="number" step="0.5" min="0.5"></label>
            <label>Stacks (bb) <input id="mw-stack" type="number" step="0.5" min="0.5"></label>
          </div>
          <div class="row2">
            <label>Rake (%) <input id="mw-rake" type="number" step="0.5" min="0" value="0"></label>
            <label>Rake cap (bb) <input id="mw-rake-cap" type="number" step="0.5" min="0" value="0"></label>
          </div>
          <table class="sizes">
            <thead><tr><th></th>${SEATS.map((i) => `<th id="mw-head-${i}">P${i + 1} bets</th>`).join("")}<th>Raises</th></tr></thead>
            <tbody>${STREETS.map((st) => `<tr><th>${cap(st)}</th>
              ${SEATS.map((i) => `<td><input id="mw-${st}-${i}"></td>`).join("")}<td><input id="mw-${st}-raise"></td></tr>`).join("")}</tbody>
          </table>
          <label>Raises per street <select id="mw-maxr"><option value="1">1</option><option value="2">2</option><option value="3">3</option></select></label>
          <p class="hint">Bets are % of the pot (<code>33%, 75%</code>), <code>e</code> for geometric, <code>a</code> for all-in; raises like <code>3x</code>.
            An empty bet box means that player only checks there (no leads); an empty raise box means no raises.
            Three-way trees grow fast, so each extra size or raise can double the solve.</p>
          <p id="mw-size" class="meta"></p>
          <div class="row2">
            <label>Target accuracy (% pot) <input id="mw-acc" type="number" step="0.1" min="0.1" value="1"></label>
            <label>Max iterations <input id="mw-iters" type="number" step="10" min="10" value="200"></label>
          </div>
          <button id="mw-solve" class="primary">Solve</button>
        </details>
        <div id="mw-progress"></div>
        <div id="mw-nav"></div>
      </aside>
      <section class="panel" id="mw-main"><p class="hint">Set up a three-way pot and press Solve. Turn and river spots solve in seconds; a flop takes minutes.</p></section>
      <aside class="panel detail" id="mw-detail"></aside>
    </div>`;

  const presetSel = root.querySelector("#mw-preset");
  presetSel.onchange = () => {
    const p = state.presets.find((s) => s.id === presetSel.value);
    if (p) fillForm(root, p, root.querySelector("#mw-board").value || p.board);
  };
  root.querySelector("#mw-solve").onclick = () => startSolve(root);
  root.querySelector("#mw-setup").addEventListener("input", () => checkSize(root));

  if (status.spot) {
    fillForm(root, status.spot, status.board);
  } else if (state.presets.length) {
    fillForm(root, state.presets[0], state.presets[0].board || "Ks7d2c");
  }
  handleStatus(root, status);
}

/**
 * Fills the setup with a three-way pot from the preflop solver: its players, ranges, pot, stacks
 * and rake, with the bet sizes of a preset. The board is left for you to pick.
 */
export function prefillMultiway(root, spot) {
  const sel = root.querySelector("#mw-preset");
  if (!sel) return; // no server: the tab shows how to start it
  const like = state.presets.find((s) => s.pot_type === spot.pot_type) || state.presets[0];
  // The bet sizes go to the same seats: whoever opened gets the preset opener's sizes.
  const bets = like ? betsFor(like, spot.opener) : defaultBets();
  let opt = sel.querySelector('option[value="__preflop"]');
  if (!opt) {
    opt = document.createElement("option");
    opt.value = "__preflop";
    sel.prepend(opt);
  }
  opt.textContent = `Preflop solver: ${spot.description}`;
  sel.value = "__preflop";
  fillForm(root, { ...spot, bets, max_raises: like?.max_raises || 1 });
  root.querySelector("#mw-setup").open = true;
  root.querySelector("#mw-progress").innerHTML = `<p class="meta">Players, ranges, pot and stacks from the preflop solver (${esc(spot.description)}). Bet sizes from ${
    esc(like?.name || "the defaults")}. Pick a board and press Solve.</p>`;
  const board = root.querySelector("#mw-board");
  board.focus();
  board.select();
}

function defaultBets() {
  const all = (b, r = "") => ({ bet: [b, b, b], raise: r });
  return { flop: all("33%"), turn: all("66%"), river: all("75%") };
}

/**
 * A preset's bet sizes moved to the seats of another pot: per-player sizes follow the role
 * (the preflop raiser's sizes go to whoever raised), the rest stay in order.
 */
function betsFor(preset, opener) {
  const bets = JSON.parse(JSON.stringify(preset.bets));
  const from = preset.opener;
  if (from == null || opener == null || from === opener) return bets;
  for (const st of STREETS) {
    const b = bets[st].bet;
    const pfr = b[from];
    const rest = b.filter((_, i) => i !== from);
    bets[st].bet = SEATS.map((i) => (i === opener ? pfr : rest.shift()));
  }
  return bets;
}

function fillForm(root, spot, board) {
  if (!spot) return;
  const $ = (id) => root.querySelector(id);
  SEATS.forEach((i) => {
    $(`#mw-name-${i}`).value = spot.players[i] || `P${i + 1}`;
    $(`#mw-range-${i}`).value = spot.ranges[i] || "";
  });
  $("#mw-pot").value = spot.pot_bb;
  $("#mw-stack").value = spot.stack_bb;
  for (const st of STREETS) {
    SEATS.forEach((i) => ($(`#mw-${st}-${i}`).value = spot.bets[st].bet[i] ?? ""));
    $(`#mw-${st}-raise`).value = spot.bets[st].raise || "";
  }
  $("#mw-maxr").value = String(Math.min(3, Math.max(1, spot.max_raises || 1)));
  $("#mw-rake").value = ((spot.rake?.rate || 0) * 100).toString();
  $("#mw-rake-cap").value = (spot.rake?.cap_bb || 0).toString();
  if (board) $("#mw-board").value = board;
  labelSeats(root);
  checkSize(root);
}

function labelSeats(root) {
  SEATS.forEach((i) => {
    const n = root.querySelector(`#mw-name-${i}`).value.trim() || `P${i + 1}`;
    root.querySelector(`#mw-head-${i}`).textContent = `${n} bets`;
  });
}

function readForm(root) {
  const $ = (id) => root.querySelector(id).value.trim();
  const board = $("#mw-board").replace(/\s+/g, "");
  const norm = board.length ? board.match(/../g).map((c) => c[0].toUpperCase() + c[1].toLowerCase()).join("") : "";
  if (!/^([2-9TJQKA][shdc]){3,5}$/.test(norm)) throw new Error("Board must be 3 to 5 cards like Ks7d2c.");
  if (new Set(splitCards(norm)).size !== norm.length / 2) throw new Error("The board has the same card twice.");
  const bets = {};
  for (const st of STREETS) bets[st] = { bet: SEATS.map((i) => $(`#mw-${st}-${i}`)), raise: $(`#mw-${st}-raise`) };
  const players = SEATS.map((i) => $(`#mw-name-${i}`) || `P${i + 1}`);
  if (new Set(players).size !== players.length) throw new Error("Give the players different names.");
  return {
    board: norm,
    spot: {
      players,
      ranges: SEATS.map((i) => $(`#mw-range-${i}`)),
      pot_bb: parseFloat($("#mw-pot")),
      stack_bb: parseFloat($("#mw-stack")),
      bets,
      max_raises: parseInt($("#mw-maxr"), 10) || 1,
      rake: { rate: parseFloat($("#mw-rake")) / 100 || 0, cap_bb: parseFloat($("#mw-rake-cap")) || 0 },
    },
    accuracy: parseFloat($("#mw-acc")) || 1,
    max_iters: parseInt($("#mw-iters"), 10) || 200,
  };
}

/** Shows the size of the tree the setup builds, a moment after it changes. */
function checkSize(root) {
  labelSeats(root);
  clearTimeout(state.sizeTimer);
  state.sizeTimer = setTimeout(async () => {
    const el = root.querySelector("#mw-size");
    const seq = ++state.sizeSeq;
    let req;
    try {
      req = readForm(root);
    } catch (e) {
      el.innerHTML = "";
      return;
    }
    try {
      const p = await api("preview", req);
      if (seq !== state.sizeSeq) return;
      const nodes = p.nodes >= 1e6 ? `${(p.nodes / 1e6).toFixed(1)} million` : p.nodes.toLocaleString();
      el.innerHTML = `This tree: ${nodes} nodes, ${p.memory_gb} GB of memory. Hands: ${p.hands.join(" / ")}.`;
    } catch (e) {
      if (seq === state.sizeSeq) el.innerHTML = `<span class="error">${esc(e.message)}</span>`;
    }
  }, 300);
}

/** Whether two requests build the same tree, so the same lines exist in both. */
function sameTree(a, b) {
  const key = (r) => JSON.stringify([r.spot.ranges, r.spot.pot_bb, r.spot.stack_bb, r.spot.bets, r.spot.max_raises || 1, r.spot.rake || {}, r.board]);
  return key(a) === key(b);
}

async function startSolve(root) {
  let req;
  try {
    req = readForm(root);
  } catch (e) {
    root.querySelector("#mw-progress").innerHTML = `<p class="error">${esc(e.message)}</p>`;
    return;
  }
  const same = state.spot && sameTree(req, { spot: state.spot, board: state.board });
  try {
    await api("solve", req);
  } catch (e) {
    root.querySelector("#mw-progress").innerHTML = `<p class="error">${esc(e.message)}</p>`;
    return;
  }
  state.node = null;
  state.resume = same ? state.path : [];
  root.querySelector("#mw-nav").innerHTML = "";
  root.querySelector("#mw-main").innerHTML = `<p class="hint">Solving…</p>`;
  root.querySelector("#mw-detail").innerHTML = "";
  handleStatus(root, await api("status"));
}

function poll(root) {
  clearTimeout(state.polling);
  state.polling = setTimeout(async () => {
    try {
      handleStatus(root, await api("status"));
    } catch (e) {
      root.querySelector("#mw-progress").innerHTML = `<p class="error">Lost the solver server: ${esc(e.message)}</p>`;
    }
  }, 1000);
}

function handleStatus(root, s) {
  state.status = s;
  if (s.spot) {
    state.spot = s.spot;
    state.board = s.board;
  }
  const progress = root.querySelector("#mw-progress");
  if (s.state === "idle") {
    progress.innerHTML = "";
    return;
  }
  if (s.state === "failed") {
    progress.innerHTML = `<p class="error">${esc(s.error || "The solve failed.")}</p>`;
    return;
  }
  if (s.state === "building") {
    progress.innerHTML = `<p class="meta">Building the tree (${s.memory_gb} GB)…</p>`;
    poll(root);
    return;
  }
  const expl = s.exploitability_pct == null ? "measuring…" : `${s.exploitability_pct.toFixed(2)}% of pot`;
  const frac = s.exploitability_pct == null
    ? s.iteration / Math.max(1, s.max_iters)
    : Math.max(s.iteration / Math.max(1, s.max_iters), Math.min(1, s.target_pct / Math.max(s.exploitability_pct, 1e-6)));
  const finished = s.state === "done" || s.state === "stopped";
  const what = s.state === "done" ? "Solved" : s.state === "stopped" ? "Stopped early" : "Solving";
  // Per-iteration time only matters for the slow (flop) solves.
  const perIter = s.iteration && s.seconds / s.iteration >= 0.1 ? ` (${(s.seconds / s.iteration).toFixed(1)}s per iteration)` : "";
  progress.innerHTML = `
    <div class="progress"><span style="width:${(finished ? 1 : frac) * 100}%"></span></div>
    <p class="meta">${what} ${cardsHtml(s.board, "tiny")} · iteration ${s.iteration} ·
      accuracy ${expl} (target ${s.target_pct}%) · ${s.seconds}s${perIter} · ${s.memory_gb} GB</p>
    ${s.state === "solving" ? `<button id="mw-stop" class="link">Stop and use the current strategy</button>` : ""}`;
  if (s.state === "solving") {
    root.querySelector("#mw-stop").onclick = () => api("stop", {});
    poll(root);
    return;
  }
  if (finished && !state.node) {
    root.querySelector("#mw-setup").open = false;
    const resume = state.resume || [];
    state.resume = null;
    goTo(root, resume, { fallback: true });
  }
}

async function goTo(root, path, { keepFocus = false, fallback = false } = {}) {
  const seq = ++state.goSeq;
  let node;
  try {
    node = await api("node", { history: historyOf(path) });
  } catch (e) {
    if (seq !== state.goSeq) return;
    if (fallback && path.length) return goTo(root, []);
    root.querySelector("#mw-main").innerHTML = `<p class="error">${esc(e.message)}</p>`;
    return;
  }
  if (seq !== state.goSeq) return;
  state.path = path;
  state.node = node;
  if (!keepFocus) {
    view.focus = null;
    view.pinned = false;
  }
  render(root);
}

/** Everyone at the node: stack, chips in, range EV and equity. */
function playersHtml(node) {
  const acting = node.type === "decision" ? node.player : -1;
  const rows = node.players.map((p, i) => {
    const status = p.folded ? "folded" : p.allin ? "all-in" : "";
    const ev = p.ev_bb == null ? "–" : `${p.ev_bb.toFixed(2)}bb`;
    const eq = p.equity == null ? "–" : pct(p.equity);
    return `<tr class="${i === acting ? "acting" : ""} ${p.folded ? "folded" : ""}">
      <td><b>${esc(p.name)}</b>${status ? ` <small>${status}</small>` : ""}</td>
      <td>${p.behind_bb}bb</td><td>${p.invested_bb}bb</td>
      <td>${p.folded ? "–" : p.combos.toFixed(0)}</td><td>${ev}</td><td>${eq}</td></tr>`;
  });
  return `<table class="combos players">
    <thead><tr><th>Player</th><th>Behind</th><th>Put in</th><th>Combos</th><th>EV</th><th>Equity</th></tr></thead>
    <tbody>${rows.join("")}</tbody></table>`;
}

const RANGE_NOTE = "EV is what a range expects to collect from here on, less what it still puts in. Equity is its share of the pot if everyone still in checked it down.";

function render(root) {
  const node = state.node;
  const who = node.names || names();
  const nav = root.querySelector("#mw-nav");
  const main = root.querySelector("#mw-main");
  const detail = root.querySelector("#mw-detail");

  const crumbs = [`<a data-depth="0">Start</a>`, ...state.path.map((p, i) => `<a data-depth="${i + 1}">${esc(p.label)}</a>`)];
  nav.innerHTML = `
    <div class="board">${cardsHtml(node.board, "big")}</div>
    <nav class="path">${crumbs.join(" › ")}</nav>
    <div id="mw-here"></div>`;
  nav.querySelector(".path").onclick = (e) => {
    const d = e.target.dataset.depth;
    if (d !== undefined) goTo(root, state.path.slice(0, +d));
  };
  const here = nav.querySelector("#mw-here");

  if (node.type === "terminal") {
    here.innerHTML = `<p class="who">${node.winner ? `${esc(node.winner)} wins the pot` : "Showdown"} · pot ${node.pot_bb}bb</p>`;
    main.innerHTML = `${playersHtml(node)}<p class="hint">The hand is over. Go back with the path on the left.</p>`;
    detail.innerHTML = "";
    return;
  }

  if (node.type === "chance") {
    const street = splitCards(node.board).length === 3 ? "turn" : "river";
    here.innerHTML = `<p class="who">Deal the ${street} · pot ${node.pot_bb}bb</p>`;
    renderChance(root, node, street);
    detail.innerHTML = `<h3>${cap(street)} report</h3>
      <p class="hint">Each card shows how the next player acts on it and everyone's equity. Click a card to deal it, or a column header to sort.</p>
      <p class="hint">${RANGE_NOTE}</p>`;
    return;
  }

  // Decision node.
  const k = node.actions.length;
  const { freq } = rangeFrequencies(nodeHands(node), k);
  here.innerHTML = `
    <p class="who"><b>${esc(who[node.player])}</b> to act · pot ${node.pot_bb}bb${node.to_call_bb ? ` · facing ${node.to_call_bb}bb` : ""}</p>
    <div class="freqbar">${node.actions.map((a, i) => `<span style="width:${freq[i] * 100}%;background:${actionColor(a, node.actions)}"></span>`).join("")}</div>
    <div class="actions">${node.actions
      .map((a, i) => `<button data-i="${i}"><i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}<em>${pct(freq[i])}</em></button>`)
      .join("")}</div>`;
  here.querySelector(".actions").onclick = (e) => {
    const i = e.target.closest("button")?.dataset.i;
    if (i === undefined) return;
    goTo(root, [...state.path, { step: +i, label: `${who[node.player]} ${actionLabel(node.actions[i])}` }]);
  };

  main.innerHTML = `${playersHtml(node)}${modeTabsHtml("mw-mode")}<div id="mw-grid"></div><p id="mw-range" class="meta"></p>`;
  main.querySelectorAll("#mw-mode button").forEach((b) => b.classList.toggle("active", b.dataset.mode === view.mode));
  wireModeTabs(main.querySelector("#mw-mode"), view, () => drawGrid(root));
  drawGrid(root);
}

function drawGrid(root) {
  const node = state.node;
  const who = node.names || names();
  const main = root.querySelector("#mw-main");
  renderNodeView(
    { grid: main.querySelector("#mw-grid"), detail: root.querySelector("#mw-detail"), range: main.querySelector("#mw-range") },
    node,
    view,
    {
      board: node.board,
      name: (p) => who[p],
      rangeHtml: `${esc(who[node.player])}'s hands. ${RANGE_NOTE}`,
    },
  );
}

function renderChance(root, node, street) {
  const main = root.querySelector("#mw-main");
  const who = node.names || names();
  const rows = node.cards.slice();
  const actions = rows.find((c) => c.actions)?.actions || [];
  // Cards can offer different actions (e.g. all-in thresholds), so align columns by label.
  const labels = actions.map(actionLabel);
  const freqOf = (c, i) => {
    if (!c.actions) return 0;
    const j = c.actions.findIndex((a) => actionLabel(a) === labels[i]);
    return j >= 0 ? c.freq[j] : 0;
  };
  const alive = node.players.map((p, i) => (p.folded ? -1 : i)).filter((i) => i >= 0);
  const { key, dir } = chanceSort;
  rows.sort((a, b) => {
    let va, vb;
    if (key === "card") [va, vb] = [RANKS.indexOf(a.name[0]) * 4 + "shdc".indexOf(a.name[1]), RANKS.indexOf(b.name[0]) * 4 + "shdc".indexOf(b.name[1])];
    else if (key.startsWith("eq")) [va, vb] = [a.equity?.[+key.slice(2)] ?? 0, b.equity?.[+key.slice(2)] ?? 0];
    else [va, vb] = [freqOf(a, +key), freqOf(b, +key)];
    return va < vb ? -dir : va > vb ? dir : 0;
  });
  const arrow = (k) => (chanceSort.key === String(k) ? (chanceSort.dir > 0 ? " ▲" : " ▼") : "");
  const actor = rows.find((c) => c.player != null)?.player;
  main.innerHTML = `
    ${playersHtml(node)}
    <h3>${actor != null ? `${esc(who[actor])}'s strategy on each ${street} card` : `${cap(street)} cards`}</h3>
    <div class="scroll"><table class="report">
      <thead><tr>
        <th data-sort="card">Card${arrow("card")}</th><th>Strategy</th>
        ${actions.map((a, i) => `<th data-sort="${i}"><i style="background:${actionColor(a, actions)}"></i>${actionText(a)}${arrow(i)}</th>`).join("")}
        ${alive.map((i) => `<th data-sort="eq${i}">${esc(who[i])} equity${arrow(`eq${i}`)}</th>`).join("")}
      </tr></thead>
      <tbody>${rows.map((c) => `<tr data-card="${c.card}" data-name="${c.name}">
        <td>${cardsHtml(c.name, "tiny")}</td>
        <td>${c.actions ? `<div class="freqbar">${actions.map((a, i) => `<span style="width:${freqOf(c, i) * 100}%;background:${actionColor(a, actions)}"></span>`).join("")}</div>` : "–"}</td>
        ${actions.map((_, i) => `<td>${(freqOf(c, i) * 100).toFixed(0)}%</td>`).join("")}
        ${alive.map((i) => `<td>${c.equity?.[i] != null ? pct(c.equity[i]) : "–"}</td>`).join("")}
      </tr>`).join("")}</tbody>
    </table></div>`;
  // Scoped to the report: the players table above it has its own head and body.
  const report = main.querySelector("table.report");
  report.querySelector("thead").onclick = (e) => {
    const k = e.target.closest("th")?.dataset.sort;
    if (k === undefined) return;
    chanceSort.dir = chanceSort.key === k ? -chanceSort.dir : k === "card" ? 1 : -1;
    chanceSort.key = k;
    renderChance(root, node, street);
  };
  report.querySelector("tbody").onclick = (e) => {
    const tr = e.target.closest("tr");
    if (!tr) return;
    goTo(root, [...state.path, { step: +tr.dataset.card, label: `${cap(street)} ${tr.dataset.name}` }]);
  };
}
