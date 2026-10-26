// Solver view: set up any spot, solve it on the local pt-server, and browse the full tree
// (flop, turn and river) including any turn or river card.

import { cardsHtml, splitCards, RANKS } from "./cards.js";
import { loadIndex, nodeHands, rangeFrequencies, actionText, actionColor, actionLabel } from "./data.js";
import { renderNodeView, modeTabsHtml, wireModeTabs } from "./nodeview.js";

const state = { spot: null, board: "", history: [], crumbs: [], node: null, polling: null };
const view = { mode: "strategy", focus: null, pinned: false };
const STREETS = ["flop", "turn", "river"];

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
    accuracy: parseFloat($("#sv-acc")) || 0.5,
    max_iters: parseInt($("#sv-iters"), 10) || 1000,
  };
}

async function startSolve(root) {
  const progress = root.querySelector("#sv-progress");
  let req;
  try {
    req = readForm(root);
    await api("solve", req);
  } catch (e) {
    progress.innerHTML = `<p class="error">${e.message}</p>`;
    return;
  }
  state.node = null;
  root.querySelector("#sv-nav").innerHTML = "";
  root.querySelector("#sv-main").innerHTML = `<p class="hint">Solving…</p>`;
  root.querySelector("#sv-detail").innerHTML = "";
  poll(root);
}

function poll(root) {
  clearTimeout(state.polling);
  state.polling = setTimeout(async () => {
    try {
      handleStatus(root, await api("status"));
    } catch (e) {
      root.querySelector("#sv-progress").innerHTML = `<p class="error">Lost the solver server: ${e.message}</p>`;
    }
  }, 1000);
}

function handleStatus(root, s) {
  const progress = root.querySelector("#sv-progress");
  if (s.state === "idle") {
    progress.innerHTML = "";
    return;
  }
  const expl = s.exploitability_pct == null ? "measuring…" : `${s.exploitability_pct.toFixed(2)}% of pot`;
  const frac = s.exploitability_pct == null
    ? s.iteration / Math.max(1, s.max_iters)
    : Math.max(s.iteration / Math.max(1, s.max_iters), Math.min(1, s.target_pct / Math.max(s.exploitability_pct, 1e-6)));
  progress.innerHTML = `
    <div class="progress"><span style="width:${(s.state === "done" ? 1 : frac) * 100}%"></span></div>
    <p class="meta">${s.state === "done" ? "Solved" : "Solving"} ${cardsHtml(s.board, "tiny")} · iteration ${s.iteration} ·
      accuracy ${expl} (target ${s.target_pct}%) · ${s.elapsed_s}s · ${s.memory_gb} GB</p>
    ${s.state === "solving" ? `<button id="sv-stop" class="link">Stop and use the current strategy</button>` : ""}`;
  if (s.state === "solving") {
    root.querySelector("#sv-stop").onclick = () => api("stop", {});
    poll(root);
    return;
  }
  if (s.state === "done" && !state.node) {
    state.spot = s.spot;
    state.board = s.board;
    root.querySelector("#sv-setup").open = false;
    goTo(root, [], []);
  }
}

async function goTo(root, history, crumbs) {
  let node;
  try {
    node = await api("node", { history });
  } catch (e) {
    root.querySelector("#sv-main").innerHTML = `<p class="error">${e.message}</p>`;
    return;
  }
  state.history = history;
  state.crumbs = crumbs;
  state.node = node;
  view.focus = null;
  view.pinned = false;
  render(root);
}

const name = (p) => (p === "oop" ? state.spot.oop : state.spot.ip);

function render(root) {
  const node = state.node;
  const nav = root.querySelector("#sv-nav");
  const main = root.querySelector("#sv-main");
  const detail = root.querySelector("#sv-detail");

  const crumbs = [`<a data-depth="0">Start</a>`, ...state.crumbs.map((c, i) => `<a data-depth="${i + 1}">${c}</a>`)];
  nav.innerHTML = `
    <div class="board">${cardsHtml(node.board, "big")}</div>
    <nav class="path">${crumbs.join(" › ")}</nav>
    <div id="sv-here"></div>`;
  nav.querySelector(".path").onclick = (e) => {
    const d = e.target.dataset.depth;
    if (d !== undefined) goTo(root, state.history.slice(0, +d), state.crumbs.slice(0, +d));
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
      <p class="hint">Each card shows how the next player acts on it, and ${state.spot.oop}'s equity. Click a card to deal it, or a column header to sort.</p>`;
    return;
  }

  // Decision node.
  node.combos = node.combos || [];
  const hands = nodeHands(node);
  const { freq } = rangeFrequencies(hands, node.actions.length);
  here.innerHTML = `
    <p class="who"><b>${name(node.player)}</b> to act · pot ${node.pot_bb}bb${node.to_call_bb ? ` · facing ${node.to_call_bb}bb` : ""}</p>
    <div class="freqbar">${node.actions.map((a, i) => `<span style="width:${freq[i] * 100}%;background:${actionColor(a, node.actions)}"></span>`).join("")}</div>
    <div class="actions">${node.actions
      .map((a, i) => `<button data-i="${i}"><i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}<em>${(freq[i] * 100).toFixed(1)}%</em></button>`)
      .join("")}</div>`;
  here.querySelector(".actions").onclick = (e) => {
    const i = e.target.closest("button")?.dataset.i;
    if (i === undefined) return;
    goTo(root, [...state.history, +i], [...state.crumbs, `${name(node.player)} ${actionLabel(node.actions[i])}`]);
  };

  main.innerHTML = `${modeTabsHtml("sv-mode")}<div id="sv-grid"></div><p id="sv-range" class="meta"></p>`;
  const draw = () =>
    renderNodeView(
      { grid: main.querySelector("#sv-grid"), detail, range: main.querySelector("#sv-range") },
      node,
      view,
      { board: node.board, name },
    );
  main.querySelectorAll("#sv-mode button").forEach((b) => b.classList.toggle("active", b.dataset.mode === view.mode));
  wireModeTabs(main.querySelector("#sv-mode"), view, draw);
  draw();
}

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
    <h3>${actor ? `${name(actor)}'s strategy on each ${street} card` : `${street} cards`}</h3>
    <div class="scroll"><table class="report">
      <thead><tr>
        <th data-sort="card">Card${arrow("card")}</th><th>Strategy</th>
        ${actions.map((a, i) => `<th data-sort="${i}"><i style="background:${actionColor(a, actions)}"></i>${actionText(a)}${arrow(i)}</th>`).join("")}
        <th data-sort="eq">${state.spot.oop} equity${arrow("eq")}</th>
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
    goTo(root, [...state.history, +tr.dataset.card], [...state.crumbs, `${street[0].toUpperCase() + street.slice(1)} ${tr.dataset.name}`]);
  };
}
