// Solver view: set up any spot, solve it on the local pt-server, and browse the full tree
// (flop, turn and river) including any turn or river card. You can lock how a player plays at
// any decision and solve again to see the best way to exploit that, next to GTO.

import { cardsHtml, splitCards, RANKS } from "./cards.js";
import { loadIndex, nodeHands, rangeFrequencies, classSummary, actionText, actionColor, actionLabel } from "./data.js";
import { renderNodeView, modeTabsHtml, wireModeTabs } from "./nodeview.js";

const state = {
  spot: null,
  board: "",
  path: [], // [{step, label}] from the start of the hand to the node shown
  node: null,
  status: null,
  solution: "gto", // the solution shown: "gto" (no locks) or "exploit" (with locks)
  editor: null, // the lock being set up at the node shown
  resume: null, // path to reopen when the running solve finishes
  polling: null,
};
const view = { mode: "strategy", focus: null, pinned: false };
const STREETS = ["flop", "turn", "river"];
const ORDERS = [
  ["ev", "The ones that lose the least EV"],
  ["weak", "The weakest hands"],
  ["strong", "The strongest hands"],
];
const LOCK_ICON = `<svg class="lock-icon" viewBox="0 0 16 16" aria-hidden="true"><path d="M5 7V5a3 3 0 0 1 6 0v2" fill="none" stroke="currentColor" stroke-width="1.7"/><rect x="3" y="7" width="10" height="7.5" rx="1.5" fill="currentColor"/></svg>`;

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const pct = (x, digits = 1) => `${(x * 100).toFixed(digits)}%`;
const solutionName = (s) => (s === "gto" ? "GTO" : "Exploit");
const historyOf = (path) => path.map((p) => p.step);

/** Short action names for lists: "Bet 75%", "Raise to 12". */
function shortAction(a) {
  switch (a.type) {
    case "bet": return `Bet ${a.pct}%`;
    case "raise": return `Raise to ${a.to_bb}`;
    case "allin": return "All-in";
    default: return a.type[0].toUpperCase() + a.type.slice(1);
  }
}

/** "turn" or "river" for the card dealt at step `i`, given who acted at each step. */
function streetOf(actors, i) {
  const dealtBefore = actors.slice(0, i).filter((a) => a === null).length;
  return splitCards(state.board).length + dealtBefore === 3 ? "turn" : "river";
}

async function api(path, body) {
  const res = await fetch(`api/${path}`, body === undefined ? {} : {
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

export async function initSolver(root) {
  let status;
  try {
    status = await api("status");
  } catch (e) {
    root.innerHTML = `
      <div class="panel narrow">
        <h3>Start the solver server</h3>
        <p>The Solver tab runs solves on your own computer through a small local server. From the repo:</p>
        <pre><code>cd solver
cargo run --release --bin pt-server</code></pre>
        <p>Then open <a href="http://127.0.0.1:8080">http://127.0.0.1:8080</a>. Everything else in the app works from there too.</p>
      </div>`;
    return;
  }

  const index = await loadIndex().catch(() => ({ spots: [] }));
  state.index = index;
  root.innerHTML = `
    <div class="study solver">
      <aside class="panel controls">
        <details id="sv-setup" open>
          <summary>Spot setup</summary>
          <label>Start from <select id="sv-preset">
            ${index.spots.map((s) => `<option value="${s.id}">${s.name}</option>`).join("")}
          </select></label>
          <div class="row2">
            <label>Out of position <input id="sv-oop-name"></label>
            <label>In position <input id="sv-ip-name"></label>
          </div>
          <label>OOP range <textarea id="sv-oop-range" rows="3"></textarea></label>
          <label>IP range <textarea id="sv-ip-range" rows="3"></textarea></label>
          <label>Board (3 to 5 cards) <input id="sv-board" placeholder="Ks7d2c"></label>
          <div class="row2">
            <label>Pot (bb) <input id="sv-pot" type="number" step="0.5" min="0.5"></label>
            <label>Eff. stack (bb) <input id="sv-stack" type="number" step="0.5" min="0.5"></label>
          </div>
          <div class="row2">
            <label>Rake (%) <input id="sv-rake" type="number" step="0.5" min="0" value="0"></label>
            <label>Rake cap (bb) <input id="sv-rake-cap" type="number" step="0.5" min="0" value="0"></label>
          </div>
          <table class="sizes">
            <thead><tr><th></th><th>OOP bets</th><th>IP bets</th><th>Raises</th></tr></thead>
            <tbody>${STREETS.map((st) => `<tr><th>${st[0].toUpperCase() + st.slice(1)}</th>
              <td><input id="sv-${st}-oop"></td><td><input id="sv-${st}-ip"></td><td><input id="sv-${st}-raise"></td></tr>`).join("")}</tbody>
          </table>
          <p class="hint">Sizes are % of pot (<code>33%, 75%</code>), <code>e</code> for geometric, <code>a</code> for all-in; raises like <code>3x</code>. Leave a box empty for none. Every extra size makes the solve slower.</p>
          <div class="row2">
            <label>Target accuracy (% pot) <input id="sv-acc" type="number" step="0.1" min="0.05" value="0.5"></label>
            <label>Max iterations <input id="sv-iters" type="number" step="50" min="10" value="1000"></label>
          </div>
          <button id="sv-solve" class="primary">Solve</button>
        </details>
        <div id="sv-progress"></div>
        <div id="sv-locks"></div>
        <div id="sv-nav"></div>
      </aside>
      <section class="panel" id="sv-main"><p class="hint">Set up a spot and press Solve.</p></section>
      <aside class="panel detail" id="sv-detail"></aside>
    </div>`;

  const presetSel = root.querySelector("#sv-preset");
  presetSel.onchange = () => fillForm(root, index.spots.find((s) => s.id === presetSel.value));
  root.querySelector("#sv-solve").onclick = () => startSolve(root);

  if (status.spot) {
    fillForm(root, status.spot, status.board);
  } else if (index.spots.length) {
    fillForm(root, index.spots[0], index.spots[0].flops[0]?.flop || "Ks7d2c");
  }
  handleStatus(root, status);
}

/**
 * Fills the setup with a pot from the preflop solver: its ranges, pot, stacks and rake, and the
 * bet sizes of a library spot of the same kind (single-raised or 3-bet pot). The board is left
 * for you to pick.
 */
export function prefillSolver(root, spot) {
  const sel = root.querySelector("#sv-preset");
  if (!sel) return; // no server: the tab shows how to start it
  const lib = state.index?.spots || [];
  const like = lib.find((s) => s.id.endsWith(`_${spot.pot_type}`)) || lib[0];
  const bets = like?.bets || { flop: { oop: "33%, 75%", ip: "33%, 75%", raise: "3x" }, turn: { oop: "66%", ip: "66%", raise: "" }, river: { oop: "75%", ip: "75%", raise: "" } };
  let opt = sel.querySelector('option[value="__preflop"]');
  if (!opt) {
    opt = document.createElement("option");
    opt.value = "__preflop";
    sel.prepend(opt);
  }
  opt.textContent = `Preflop solver: ${spot.description}`;
  sel.value = "__preflop";
  fillForm(root, { ...spot, bets });
  root.querySelector("#sv-setup").open = true;
  root.querySelector("#sv-progress").innerHTML = `<p class="meta">Ranges, pot and stacks from the preflop solver (${esc(spot.description)}). Bet sizes from ${
    esc(like?.name || "the defaults")}. Pick a board and press Solve.</p>`;
  const board = root.querySelector("#sv-board");
  board.focus();
  board.select();
}

function fillForm(root, spot, board) {
  if (!spot) return;
  const $ = (id) => root.querySelector(id);
  $("#sv-oop-name").value = spot.oop || "OOP";
  $("#sv-ip-name").value = spot.ip || "IP";
  $("#sv-oop-range").value = spot.ranges.oop;
  $("#sv-ip-range").value = spot.ranges.ip;
  $("#sv-pot").value = spot.pot_bb;
  $("#sv-stack").value = spot.stack_bb;
  for (const st of STREETS) {
    $(`#sv-${st}-oop`).value = spot.bets[st].oop;
    $(`#sv-${st}-ip`).value = spot.bets[st].ip;
    $(`#sv-${st}-raise`).value = spot.bets[st].raise;
  }
  $("#sv-rake").value = ((spot.rake?.rate || 0) * 100).toString();
  $("#sv-rake-cap").value = (spot.rake?.cap_bb || 0).toString();
  if (board) $("#sv-board").value = board;
}

function readForm(root) {
  const $ = (id) => root.querySelector(id).value.trim();
  const board = $("#sv-board").replace(/\s+/g, "");
  const norm = board.length ? board.match(/../g).map((c) => c[0].toUpperCase() + c[1].toLowerCase()).join("") : "";
  if (!/^([2-9TJQKA][shdc]){3,5}$/.test(norm)) throw new Error("Board must be 3 to 5 cards like Ks7d2c.");
  if (new Set(splitCards(norm)).size !== norm.length / 2) throw new Error("The board has the same card twice.");
  const bets = {};
  for (const st of STREETS) bets[st] = { oop: $(`#sv-${st}-oop`), ip: $(`#sv-${st}-ip`), raise: $(`#sv-${st}-raise`) };
  return {
    board: norm,
    spot: {
      oop: $("#sv-oop-name") || "OOP",
      ip: $("#sv-ip-name") || "IP",
      ranges: { oop: $("#sv-oop-range"), ip: $("#sv-ip-range") },
      pot_bb: parseFloat($("#sv-pot")),
      stack_bb: parseFloat($("#sv-stack")),
      bets,
      rake: { rate: parseFloat($("#sv-rake")) / 100 || 0, cap_bb: parseFloat($("#sv-rake-cap")) || 0 },
    },
    ...readLimits(root),
  };
}

function readLimits(root) {
  return {
    accuracy: parseFloat(root.querySelector("#sv-acc").value) || 0.5,
    max_iters: parseInt(root.querySelector("#sv-iters").value, 10) || 1000,
  };
}

/** Whether two solve requests build the same tree, so the same lines exist in both. */
function sameTree(a, b) {
  const key = (r) => JSON.stringify([r.spot.ranges, r.spot.pot_bb, r.spot.stack_bb, r.spot.bets, r.spot.rake || {}, r.board]);
  return key(a) === key(b);
}

async function startSolve(root) {
  let req;
  try {
    req = readForm(root);
  } catch (e) {
    root.querySelector("#sv-progress").innerHTML = `<p class="error">${esc(e.message)}</p>`;
    return;
  }
  const same = state.spot && sameTree(req, { spot: state.spot, board: state.board });
  await runSolve(root, req, same ? state.path : []);
}

async function runSolve(root, req, resume) {
  try {
    await api("solve", req);
  } catch (e) {
    root.querySelector("#sv-progress").innerHTML = `<p class="error">${esc(e.message)}</p>`;
    return;
  }
  state.node = null;
  state.editor = null;
  state.resume = resume;
  root.querySelector("#sv-nav").innerHTML = "";
  root.querySelector("#sv-main").innerHTML = `<p class="hint">Solving…</p>`;
  root.querySelector("#sv-detail").innerHTML = "";
  handleStatus(root, await api("status"));
}

function poll(root) {
  clearTimeout(state.polling);
  state.polling = setTimeout(async () => {
    try {
      handleStatus(root, await api("status"));
    } catch (e) {
      root.querySelector("#sv-progress").innerHTML = `<p class="error">Lost the solver server: ${esc(e.message)}</p>`;
    }
  }, 1000);
}

function handleStatus(root, s) {
  state.status = s;
  // The spot of the latest solve; any finished solution is of the same tree.
  if (s.spot) {
    state.spot = s.spot;
    state.board = s.board;
  }
  const progress = root.querySelector("#sv-progress");
  renderLocks(root);
  if (s.state === "idle") {
    progress.innerHTML = "";
    return;
  }
  if (s.state === "failed") {
    progress.innerHTML = `<p class="error">${esc(s.error || "The solve failed.")}</p>`;
    // The other solution of the same spot may still be there to browse.
    const left = ["gto", "exploit"].find((k) => s.solutions?.[k]);
    if (!state.node && left) {
      const resume = state.resume || [];
      state.resume = null;
      goTo(root, resume, left, { fallback: true });
    } else if (!state.node) {
      root.querySelector("#sv-main").innerHTML = `<p class="hint">Nothing to show.</p>`;
    }
    return;
  }
  const expl = s.exploitability_pct == null ? "measuring…" : `${s.exploitability_pct.toFixed(2)}% of pot`;
  const frac = s.exploitability_pct == null
    ? s.iteration / Math.max(1, s.max_iters)
    : Math.max(s.iteration / Math.max(1, s.max_iters), Math.min(1, s.target_pct / Math.max(s.exploitability_pct, 1e-6)));
  const what = `${s.state === "done" ? "Solved" : "Solving"}${s.kind === "exploit" ? " with locks" : ""}`;
  progress.innerHTML = `
    <div class="progress"><span style="width:${(s.state === "done" ? 1 : frac) * 100}%"></span></div>
    <p class="meta">${what} ${cardsHtml(s.board, "tiny")} · iteration ${s.iteration} ·
      accuracy ${expl} (target ${s.target_pct}%) · ${s.elapsed_s}s · ${s.memory_gb} GB</p>
    ${s.note ? `<p class="meta">${esc(s.note)}</p>` : ""}
    ${s.state === "solving" ? `<button id="sv-stop" class="link">Stop and use the current strategy</button>` : ""}`;
  if (s.state === "solving") {
    root.querySelector("#sv-stop").onclick = () => api("stop", {});
    poll(root);
    return;
  }
  if (s.state === "done" && !state.node) {
    root.querySelector("#sv-setup").open = false;
    const resume = state.resume || [];
    state.resume = null;
    goTo(root, resume, s.kind, { fallback: true });
  }
}

async function goTo(root, path, solution = state.solution, { keepFocus = false, fallback = false } = {}) {
  const sols = state.status?.solutions || {};
  const ask = sols[solution] ? solution : undefined;
  let node;
  try {
    node = await api("node", { history: historyOf(path), solution: ask });
  } catch (e) {
    if (fallback && path.length) return goTo(root, [], solution);
    root.querySelector("#sv-main").innerHTML = `<p class="error">${esc(e.message)}</p>`;
    return;
  }
  state.path = path;
  state.node = node;
  state.solution = node.solution;
  state.editor = null;
  if (!keepFocus) {
    view.focus = null;
    view.pinned = false;
  }
  render(root);
}

async function refreshStatus(root) {
  state.status = await api("status");
  renderLocks(root);
}

const name = (p) => (p === "oop" ? state.spot?.oop || "OOP" : state.spot?.ip || "IP");
const opponent = (p) => name(p === "oop" ? "ip" : "oop");

function render(root) {
  const node = state.node;
  renderLocks(root);
  const nav = root.querySelector("#sv-nav");
  const main = root.querySelector("#sv-main");
  const detail = root.querySelector("#sv-detail");

  const crumbs = [`<a data-depth="0">Start</a>`, ...state.path.map((p, i) => `<a data-depth="${i + 1}">${esc(p.label)}</a>`)];
  nav.innerHTML = `
    <div class="board">${cardsHtml(node.board, "big")}</div>
    <nav class="path">${crumbs.join(" › ")}</nav>
    <div id="sv-here"></div>`;
  nav.querySelector(".path").onclick = (e) => {
    const d = e.target.dataset.depth;
    if (d !== undefined) goTo(root, state.path.slice(0, +d));
  };
  const here = nav.querySelector("#sv-here");

  if (node.type === "terminal") {
    here.innerHTML = `<p class="who">The hand is over. Pot ${node.pot_bb}bb.</p>`;
    main.innerHTML = `<p class="hint">Nothing to decide here. Go back with the path above.</p>`;
    detail.innerHTML = "";
    return;
  }

  if (node.type === "chance") {
    const street = splitCards(node.board).length === 3 ? "turn" : "river";
    here.innerHTML = `<p class="who">Deal the ${street} · pot ${node.pot_bb}bb</p>`;
    renderChance(root, node, street);
    detail.innerHTML = `<h3>${street[0].toUpperCase() + street.slice(1)} report</h3>
      <p class="hint">Each card shows how the next player acts on it, and ${esc(state.spot.oop)}'s equity. Click a card to deal it, or a column header to sort.</p>`;
    return;
  }

  // Decision node.
  node.combos = node.combos || [];
  const k = node.actions.length;
  const { freq } = rangeFrequencies(nodeHands(node), k);
  const cmp = node.compare && rangeFrequencies(nodeHands({ ...node, hands: node.compare.hands }), k).freq;
  const lock = matchingLock();
  here.innerHTML = `
    <p class="who"><b>${esc(name(node.player))}</b> to act · pot ${node.pot_bb}bb${node.to_call_bb ? ` · facing ${node.to_call_bb}bb` : ""}</p>
    ${lockBadge(node, lock)}
    <div class="freqbar">${node.actions.map((a, i) => `<span style="width:${freq[i] * 100}%;background:${actionColor(a, node.actions)}"></span>`).join("")}</div>
    <div class="actions">${node.actions
      .map((a, i) => `<button data-i="${i}"><i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}<em>${pct(freq[i])}${
        cmp ? `<small>${solutionName(node.compare.solution)} ${pct(cmp[i])}</small>` : ""
      }</em></button>`)
      .join("")}</div>
    ${k > 1 ? `<div class="lockbar">
      <button class="secondary" id="sv-lock">${LOCK_ICON}${lock ? "Edit lock" : `Lock ${esc(name(node.player))}'s play here`}</button>
      ${lock ? `<button class="link" id="sv-unlock">Remove lock</button>` : ""}
    </div>` : ""}`;
  here.querySelector(".actions").onclick = (e) => {
    const i = e.target.closest("button")?.dataset.i;
    if (i === undefined) return;
    goTo(root, [...state.path, { step: +i, label: `${name(node.player)} ${actionLabel(node.actions[i])}` }]);
  };
  const lockBtn = here.querySelector("#sv-lock");
  if (lockBtn) lockBtn.onclick = () => (state.editor ? closeEditor(root) : openEditor(root, lock));
  const unlockBtn = here.querySelector("#sv-unlock");
  if (unlockBtn) unlockBtn.onclick = () => removeLock(root, lock.id);
  renderDecision(root);
}

/** The saved lock that covers the node shown, the most specific one if several do. */
function matchingLock() {
  const h = historyOf(state.path);
  return (state.status?.locks || [])
    .filter((l) => l.history.length === h.length && l.history.every((s, i) => s === h[i] || l.wild.includes(i)))
    .sort((a, b) => a.wild.length - b.wild.length)[0] || null;
}

function locksCurrent() {
  const s = state.status;
  return !!s?.solutions?.exploit && s.solutions.exploit.locks_version === s.locks_version;
}

function lockBadge(node, lock) {
  let text = "";
  if (node.locked) {
    text = locksCurrent()
      ? `Locked. ${esc(opponent(node.player))} adjusts to exploit it.`
      : state.status?.locks?.length
        ? "Locked in this solve. The locks have changed since, so solve with locks again to update it."
        : "Locked in this solve. You've removed the locks since.";
  } else if (lock) {
    text = locksCurrent() && state.solution === "gto"
      ? "Locked in the exploit solve. Switch to Exploit to see it."
      : "Lock saved. Solve with locks to apply it.";
  }
  return text ? `<p class="lockbadge">${LOCK_ICON}<span>${text}</span></p>` : "";
}

function renderDecision(root) {
  const main = root.querySelector("#sv-main");
  const detail = root.querySelector("#sv-detail");
  main.innerHTML = `${modeTabsHtml("sv-mode")}<div id="sv-grid"></div><p id="sv-range" class="meta"></p><p id="sv-cmp" class="meta"></p>`;
  if (state.editor) {
    detail.innerHTML = `<div id="sv-editor"></div><div id="sv-hand"></div>`;
    renderEditor(root);
  }
  main.querySelectorAll("#sv-mode button").forEach((b) => b.classList.toggle("active", b.dataset.mode === view.mode));
  wireModeTabs(main.querySelector("#sv-mode"), view, () => drawGrid(root));
  drawGrid(root);

  const node = state.node;
  const cmpEl = main.querySelector("#sv-cmp");
  if (node.compare && !state.editor) {
    const other = solutionName(node.compare.solution);
    const side = (p, i) => {
      const d = node.ev_bb[i] - node.compare.ev_bb[i];
      return `${esc(name(p))} ${node.compare.ev_bb[i].toFixed(2)} → ${node.ev_bb[i].toFixed(2)}bb (${d >= 0 ? "+" : "−"}${Math.abs(d).toFixed(2)})`;
    };
    cmpEl.innerHTML = `EV here, ${other} → ${solutionName(node.solution)}: ${side("oop", 0)} · ${side("ip", 1)}.`;
  }
}

function previewNode(node, strategy) {
  const k = node.actions.length;
  return { ...node, hands: node.hands.map((row, i) => row && [row[0], ...strategy[i], ...row.slice(1 + k)]) };
}

function drawGrid(root) {
  const node = state.node;
  const ed = state.editor;
  const main = root.querySelector("#sv-main");
  const shown = ed?.preview ? previewNode(node, ed.preview.strategy) : node;
  let compare = null;
  if (ed) {
    compare = { label: "Now", node, note: "The Now row is the whole class as it plays without this lock." };
  } else if (node.compare) {
    const other = solutionName(node.compare.solution);
    compare = { label: other, node: { ...node, hands: node.compare.hands }, note: `The ${other} row is the whole class in the ${other} solution.` };
  }
  renderNodeView(
    {
      grid: main.querySelector("#sv-grid"),
      detail: root.querySelector(ed ? "#sv-hand" : "#sv-detail"),
      range: main.querySelector("#sv-range"),
    },
    shown,
    view,
    {
      board: node.board,
      name,
      compare,
      marked: ed ? new Set(ed.hands.keys()) : null,
      onCellClick: ed ? (cls) => paint(root, cls) : null,
    },
  );
}

// ---- Lock editor -------------------------------------------------------------------------

function openEditor(root, lock) {
  const node = state.node;
  const { freq } = rangeFrequencies(nodeHands(node), node.actions.length);
  const keep = node.actors.some((a) => a === node.player);
  state.editor = {
    base: freq,
    targets: lock?.targets ? lock.targets.slice() : null,
    hands: new Map(Object.entries(lock?.hands || {})),
    order: lock?.order || "ev",
    wild: new Set(lock?.wild || []),
    keepPath: lock ? lock.keep_path : keep,
    brush: null,
    preview: null,
    seq: 0,
    timer: null,
  };
  view.pinned = false;
  render(root);
  schedulePreview(root, 0);
}

function closeEditor(root) {
  state.editor = null;
  render(root);
}

function renderEditor(root) {
  const node = state.node;
  const ed = state.editor;
  const el = root.querySelector("#sv-editor");
  const k = node.actions.length;
  const actor = name(node.player);
  const targets = ed.targets || ed.base;
  const cards = node.actors.map((a, i) => (a === null ? i : -1)).filter((i) => i >= 0);
  const earlier = node.actors.some((a) => a === node.player);
  el.innerHTML = `
    <div class="lockedit">
      <h3>${LOCK_ICON}Lock ${esc(actor)}'s play here</h3>
      <p class="hint">Set how ${esc(actor)} plays this spot, starting from the ${solutionName(state.solution)} strategy shown.
        When you solve with locks, the solver finds the best way for ${esc(opponent(node.player))} to exploit it.</p>
      <table class="lockfreq">
        <thead><tr><th>Action</th><th>Now</th><th>Lock to</th></tr></thead>
        <tbody>${node.actions.map((a, i) => `<tr>
          <td><i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}</td>
          <td>${pct(ed.base[i])}</td>
          <td><input type="number" min="0" max="100" step="1" data-a="${i}" value="${+(targets[i] * 100).toFixed(1)}" aria-label="${esc(shortAction(a))} frequency">%</td>
        </tr>`).join("")}</tbody>
      </table>
      <p class="meta" id="lk-result"></p>
      <label>When a frequency changes, switch first <select id="lk-order">${ORDERS.map(
        ([v, t]) => `<option value="${v}" ${ed.order === v ? "selected" : ""}>${t}</option>`,
      ).join("")}</select></label>
      <div class="brushes" id="lk-brushes">
        <span>Paint hands</span>
        ${node.actions.map((a, i) => `<button data-b="${i}"><i style="background:${actionColor(a, node.actions)}"></i>${shortAction(a)}</button>`).join("")}
        <button data-b="erase">Erase</button>
      </div>
      <p class="hint" id="lk-brush-hint"></p>
      <div class="chips" id="lk-painted"></div>
      ${cards.map((i) => `<label class="check"><input type="checkbox" data-wild="${i}" ${ed.wild.has(i) ? "checked" : ""}> On every ${streetOf(node.actors, i)} card, not just ${esc(cardOf(i))}</label>`).join("")}
      ${earlier ? `<label class="check"><input type="checkbox" id="lk-keep" ${ed.keepPath ? "checked" : ""}> Keep ${esc(actor)}'s earlier decisions on this line as they are now</label>
        <p class="hint">A real player doesn't change their earlier play to cover a leak. With this off the solver does, so other hands get here and the frequencies can drift from the ones above.</p>` : ""}
      <p class="error" id="lk-error"></p>
      <div class="lockbtns">
        <button class="primary" id="lk-save">Save lock</button>
        <button class="link" id="lk-reset">Start over</button>
        <button class="link" id="lk-cancel">Cancel</button>
      </div>
    </div>`;

  const inputs = [...el.querySelectorAll("input[data-a]")];
  for (const input of inputs) {
    input.oninput = () => {
      const vals = inputs.map((x) => Math.max(0, parseFloat(x.value) || 0));
      if (k === 2) {
        const i = +input.dataset.a;
        vals[1 - i] = Math.max(0, 100 - Math.min(100, vals[i]));
        inputs[1 - i].value = +vals[1 - i].toFixed(1);
      }
      ed.targets = vals.map((v) => v / 100);
      schedulePreview(root);
    };
  }
  el.querySelector("#lk-order").onchange = (e) => {
    ed.order = e.target.value;
    schedulePreview(root);
  };
  el.querySelector("#lk-brushes").onclick = (e) => {
    const b = e.target.closest("button")?.dataset.b;
    if (b === undefined) return;
    const brush = b === "erase" ? "erase" : +b;
    ed.brush = ed.brush === brush ? null : brush;
    renderBrushes(root);
  };
  el.querySelectorAll("input[data-wild]").forEach((box) => {
    box.onchange = () => (box.checked ? ed.wild.add(+box.dataset.wild) : ed.wild.delete(+box.dataset.wild));
  });
  const keep = el.querySelector("#lk-keep");
  if (keep) keep.onchange = () => (ed.keepPath = keep.checked);
  el.querySelector("#lk-save").onclick = () => saveLock(root);
  el.querySelector("#lk-cancel").onclick = () => closeEditor(root);
  el.querySelector("#lk-reset").onclick = () => {
    ed.targets = null;
    ed.hands.clear();
    ed.order = "ev";
    ed.brush = null;
    ed.preview = null;
    renderEditor(root);
    drawGrid(root);
    schedulePreview(root, 0);
  };
  el.querySelector("#lk-painted").onclick = (e) => {
    const cls = e.target.closest(".chip")?.dataset.cls;
    if (!cls) return;
    ed.hands.delete(cls);
    renderPainted(root);
    drawGrid(root);
    schedulePreview(root);
  };
  renderBrushes(root);
  renderPainted(root);
  renderResult(root);
}

/** The card dealt at step `i` of the path, like "Qh". */
function cardOf(i) {
  const label = state.path[i]?.label || "";
  return label.split(" ").pop();
}

function renderBrushes(root) {
  const ed = state.editor;
  root.querySelectorAll("#lk-brushes button").forEach((b) => {
    const brush = b.dataset.b === "erase" ? "erase" : +b.dataset.b;
    b.classList.toggle("active", ed.brush === brush);
  });
  root.querySelector("#lk-brush-hint").textContent = ed.brush === null
    ? "Pick an action, then click hands in the grid to fix them to it. The frequencies above are then met with the other hands."
    : ed.brush === "erase"
      ? "Click painted hands to let them move again."
      : `Click hands in the grid to always ${shortAction(state.node.actions[ed.brush]).toLowerCase()} with them. Click again to undo.`;
}

function renderPainted(root) {
  const node = state.node;
  const ed = state.editor;
  const el = root.querySelector("#lk-painted");
  el.innerHTML = [...ed.hands.entries()]
    .map(([cls, mix]) => {
      const a = mix.indexOf(Math.max(...mix));
      return `<span class="chip" data-cls="${esc(cls)}" title="Remove"><i style="background:${actionColor(node.actions[a], node.actions)}"></i>${esc(cls)} ×</span>`;
    })
    .join("");
}

function renderResult(root) {
  const node = state.node;
  const ed = state.editor;
  const el = root.querySelector("#lk-result");
  if (!el) return;
  const parts = [];
  if (ed.targets) {
    const total = ed.targets.reduce((a, b) => a + b, 0);
    if (Math.abs(total - 1) > 0.001) parts.push(`These add up to ${pct(total)}; they'll be scaled to 100%.`);
  }
  if (ed.preview) {
    parts.push(`With this lock: ${node.actions.map((a, i) => `${shortAction(a)} ${pct(ed.preview.after[i])}`).join(" · ")}.`);
    parts.push(...ed.preview.warnings.map(esc));
  }
  el.innerHTML = parts.join(" ");
  root.querySelector("#lk-error").textContent = ed.error || "";
}

function schedulePreview(root, delay = 150) {
  const ed = state.editor;
  clearTimeout(ed.timer);
  ed.timer = setTimeout(async () => {
    const seq = ++ed.seq;
    let res;
    try {
      res = await api("lock/preview", {
        history: historyOf(state.path),
        solution: state.solution,
        targets: ed.targets,
        hands: Object.fromEntries(ed.hands),
        order: ed.order,
      });
      ed.error = null;
    } catch (e) {
      if (state.editor === ed && seq === ed.seq) {
        ed.error = e.message;
        renderResult(root);
      }
      return;
    }
    if (state.editor !== ed || seq !== ed.seq) return;
    ed.preview = res;
    drawGrid(root);
    renderResult(root);
  }, delay);
}

function paint(root, cls) {
  const ed = state.editor;
  if (!ed || ed.brush === null) return false;
  const k = state.node.actions.length;
  if (ed.brush === "erase") {
    ed.hands.delete(cls);
  } else if (classSummary(nodeHands(state.node), k).has(cls)) {
    const cur = ed.hands.get(cls);
    if (cur && cur[ed.brush] === 1) ed.hands.delete(cls);
    else ed.hands.set(cls, Array.from({ length: k }, (_, i) => (i === ed.brush ? 1 : 0)));
  }
  renderPainted(root);
  drawGrid(root);
  schedulePreview(root);
  return true;
}

async function saveLock(root) {
  const node = state.node;
  const ed = state.editor;
  const btn = root.querySelector("#lk-save");
  btn.disabled = true;
  btn.textContent = "Saving…";
  const wild = [...ed.wild].sort((a, b) => a - b);
  const steps = state.path.map((p, i) => (ed.wild.has(i) ? `Any ${streetOf(node.actors, i)}` : p.label));
  try {
    await api("locks/add", {
      history: historyOf(state.path),
      solution: state.solution,
      targets: ed.targets,
      hands: Object.fromEntries(ed.hands),
      order: ed.order,
      wild,
      keep_path: ed.keepPath && node.actors.some((a) => a === node.player),
      label: [...steps, name(node.player)].join(" › "),
      crumbs: state.path.map((p) => p.label),
    });
  } catch (e) {
    btn.disabled = false;
    btn.textContent = "Save lock";
    ed.error = e.message;
    renderResult(root);
    return;
  }
  state.editor = null;
  await refreshStatus(root);
  render(root);
}

async function removeLock(root, id) {
  try {
    await api("locks/remove", { id });
  } catch (e) {
    root.querySelector("#sv-main").insertAdjacentHTML("afterbegin", `<p class="error">${esc(e.message)}</p>`);
    return;
  }
  await refreshStatus(root);
  if (state.node) render(root);
}

// ---- Saved locks and the GTO / exploit comparison ------------------------------------------

function renderLocks(root) {
  const el = root.querySelector("#sv-locks");
  const s = state.status;
  if (!el || !s) return;
  const locks = s.locks || [];
  const ex = s.solutions?.exploit;
  const gto = s.solutions?.gto;
  if (!locks.length && !ex) {
    el.innerHTML = "";
    return;
  }
  const solving = s.state === "solving";
  const current = locksCurrent();
  const evLine = () => {
    const side = (p, i) => {
      const d = ex.root_ev_bb[i] - gto.root_ev_bb[i];
      return `${esc(name(p))} ${gto.root_ev_bb[i].toFixed(2)} → ${ex.root_ev_bb[i].toFixed(2)}bb <b class="${d >= 0 ? "up" : "down"}">${d >= 0 ? "+" : "−"}${Math.abs(d).toFixed(2)}</b>`;
    };
    return `<p class="meta">EV for the whole hand, GTO → exploit: ${side("oop", 0)} · ${side("ip", 1)}</p>`;
  };
  el.innerHTML = `
    <div class="locks">
      <h3>${LOCK_ICON}Locks <small>${locks.length}</small></h3>
      ${locks.length ? `<ul>${locks.map(lockItem).join("")}</ul>` : `<p class="hint">No saved locks. The exploit solve used locks you've removed since.</p>`}
      ${locks.length ? `<button class="primary" id="sv-solve-locks" ${solving ? "disabled" : ""}>${
        current ? "Solve again with locks" : locks.length === 1 ? "Solve with this lock" : `Solve with these ${locks.length} locks`
      }</button>` : ""}
      ${locks.length && ex && !current && !solving ? `<p class="meta">The locks have changed since the last solve with locks.</p>` : ""}
      <p class="error" id="sv-locks-error"></p>
      ${ex && gto && !solving ? `
        <div class="tabs" id="sv-sol">
          <button data-sol="exploit" class="${state.solution === "exploit" ? "active" : ""}">Exploit</button>
          <button data-sol="gto" class="${state.solution === "gto" ? "active" : ""}">GTO</button>
        </div>
        ${evLine()}` : ""}
    </div>`;

  el.querySelector("ul")?.addEventListener("click", (e) => {
    const remove = e.target.closest("[data-remove]")?.dataset.remove;
    if (remove) return removeLock(root, +remove);
    const go = e.target.closest("[data-go]")?.dataset.go;
    const lock = locks.find((l) => l.id === +go);
    if (lock && !solving) goTo(root, lock.history.map((step, i) => ({ step, label: lock.crumbs[i] ?? "?" })));
  });
  const solveBtn = el.querySelector("#sv-solve-locks");
  if (solveBtn) {
    solveBtn.onclick = async () => {
      const req = { spot: state.spot, board: state.board, ...readLimits(root), locks: true };
      try {
        await api("solve", req);
      } catch (e) {
        el.querySelector("#sv-locks-error").textContent = e.message;
        return;
      }
      state.solution = "exploit";
      state.node = null;
      state.editor = null;
      state.resume = state.path;
      root.querySelector("#sv-nav").innerHTML = "";
      root.querySelector("#sv-main").innerHTML = `<p class="hint">Solving with locks…</p>`;
      root.querySelector("#sv-detail").innerHTML = "";
      handleStatus(root, await api("status"));
    };
  }
  el.querySelector("#sv-sol")?.addEventListener("click", (e) => {
    const sol = e.target.dataset.sol;
    if (sol && sol !== state.solution && state.node) goTo(root, state.path, sol, { keepFocus: true });
  });
}

function lockItem(l) {
  const where = l.wild.length ? ` · every ${l.wild.map((i) => streetOf(l.actors, i)).join(" and ")} card` : "";
  return `<li>
    <div class="lockhead"><a data-go="${l.id}" title="Go to this spot">${esc(l.label)}</a>
      <button class="x" data-remove="${l.id}" title="Remove this lock" aria-label="Remove this lock">×</button></div>
    <div class="freqbar">${l.actions.map((a, i) => `<span style="width:${l.after[i] * 100}%;background:${actionColor(a, l.actions)}"></span>`).join("")}</div>
    <small>${l.actions.map((a, i) => `${shortAction(a)} ${pct(l.after[i], 0)}`).join(" · ")}${where}</small>
  </li>`;
}

// ---- Turn and river card reports -----------------------------------------------------------

const chanceSort = { key: "card", dir: 1 };

function renderChance(root, node, street) {
  const main = root.querySelector("#sv-main");
  const rows = node.cards.slice();
  const actions = rows.find((c) => c.actions)?.actions || [];
  // Cards can offer different actions (e.g. all-in thresholds), so align columns by label.
  const labels = actions.map(actionLabel);
  const freqOf = (c, i) => {
    if (!c.actions) return 0;
    const j = c.actions.findIndex((a) => actionLabel(a) === labels[i]);
    return j >= 0 ? c.freq[j] : 0;
  };
  const { key, dir } = chanceSort;
  rows.sort((a, b) => {
    let va, vb;
    if (key === "card") [va, vb] = [RANKS.indexOf(a.name[0]) * 4 + "shdc".indexOf(a.name[1]), RANKS.indexOf(b.name[0]) * 4 + "shdc".indexOf(b.name[1])];
    else if (key === "eq") [va, vb] = [a.equity?.[0] ?? 0, b.equity?.[0] ?? 0];
    else [va, vb] = [freqOf(a, +key), freqOf(b, +key)];
    return va < vb ? -dir : va > vb ? dir : 0;
  });
  const arrow = (k) => (chanceSort.key === String(k) ? (chanceSort.dir > 0 ? " ▲" : " ▼") : "");
  const actor = rows.find((c) => c.player)?.player;
  main.innerHTML = `
    <h3>${actor ? `${esc(name(actor))}'s strategy on each ${street} card` : `${street} cards`}</h3>
    <div class="scroll"><table class="report">
      <thead><tr>
        <th data-sort="card">Card${arrow("card")}</th><th>Strategy</th>
        ${actions.map((a, i) => `<th data-sort="${i}"><i style="background:${actionColor(a, actions)}"></i>${actionText(a)}${arrow(i)}</th>`).join("")}
        <th data-sort="eq">${esc(state.spot.oop)} equity${arrow("eq")}</th>
      </tr></thead>
      <tbody>${rows.map((c) => `<tr data-card="${c.card}" data-name="${c.name}">
        <td>${cardsHtml(c.name, "tiny")}</td>
        <td>${c.actions ? `<div class="freqbar">${actions.map((a, i) => `<span style="width:${freqOf(c, i) * 100}%;background:${actionColor(a, actions)}"></span>`).join("")}</div>` : "–"}</td>
        ${actions.map((_, i) => `<td>${(freqOf(c, i) * 100).toFixed(0)}%</td>`).join("")}
        <td>${c.equity ? `${(c.equity[0] * 100).toFixed(1)}%` : "–"}</td>
      </tr>`).join("")}</tbody>
    </table></div>`;
  main.querySelector("thead").onclick = (e) => {
    const k = e.target.closest("th")?.dataset.sort;
    if (k === undefined) return;
    chanceSort.dir = chanceSort.key === k ? -chanceSort.dir : k === "card" ? 1 : -1;
    chanceSort.key = k;
    renderChance(root, node, street);
  };
  main.querySelector("tbody").onclick = (e) => {
    const tr = e.target.closest("tr");
    if (!tr) return;
    const label = `${street[0].toUpperCase() + street.slice(1)} ${tr.dataset.name}`;
    goTo(root, [...state.path, { step: +tr.dataset.card, label }]);
  };
}
