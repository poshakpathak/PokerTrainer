// Study view: browse a solved flop node by node, like a solver's strategy browser.

import { cardsHtml } from "./cards.js";
import { loadIndex, loadSolution, nodeHands, rangeFrequencies, actionText, actionColor } from "./data.js";
import { renderNodeView, modeTabsHtml, wireModeTabs } from "./nodeview.js";
import { renderSimple } from "./simplifyview.js";

const state = { spot: null, flop: null, sol: null, node: null };
const view = { mode: "strategy", focus: null, pinned: false, level: null, type: null };

export async function initStudy(root) {
  root.innerHTML = `
    <div class="study">
      <aside class="panel controls">
        <label>Spot <select id="st-spot"></select></label>
        <label>Flop <select id="st-flop"></select></label>
        <div id="st-board" class="board"></div>
        <div id="st-meta" class="meta"></div>
        <nav id="st-path" class="path"></nav>
        <div id="st-who" class="who"></div>
        <div id="st-freq" class="freqbar"></div>
        <div id="st-actions" class="actions"></div>
      </aside>
      <section class="panel">
        ${modeTabsHtml("st-mode", { simple: true })}
        <div id="st-grid"></div>
        <p id="st-range" class="meta"></p>
        <div id="st-simple"></div>
      </section>
      <aside class="panel detail" id="st-detail"></aside>
    </div>`;

  const index = await loadIndex();
  const spotSel = root.querySelector("#st-spot");
  const spots = index.spots.filter((s) => s.flops.length);
  if (!spots.length) {
    root.querySelector(".study").innerHTML = `<p class="panel">No solved flops yet. Run <code>python3 scripts/build_library.py</code>.</p>`;
    return;
  }
  spotSel.innerHTML = spots.map((s) => `<option value="${s.id}">${s.name}</option>`).join("");
  spotSel.onchange = () => selectSpot(root, index, spotSel.value);
  root.querySelector("#st-flop").onchange = (e) => selectFlop(root, e.target.value);
  wireModeTabs(root.querySelector("#st-mode"), view, () => showNode(root, state.node));
  await selectSpot(root, index, spots[0].id);
}

/**
 * Opens a specific spot/flop/node, used by the Drill's and Simplify's links. `mode` picks the
 * tab above the grid, e.g. "simple".
 */
export async function openInStudy(root, spotId, flop, path, cls, mode) {
  const index = await loadIndex();
  root.querySelector("#st-spot").value = spotId;
  if (mode) {
    view.mode = mode;
    root.querySelectorAll("#st-mode button").forEach((b) => b.classList.toggle("active", b.dataset.mode === mode));
  }
  await selectSpot(root, index, spotId, flop);
  const node = state.sol.byPath.get(path.join(" / "));
  if (node) {
    view.focus = cls || null;
    view.pinned = !!cls;
    showNode(root, node);
  }
}

async function selectSpot(root, index, spotId, flop) {
  state.spot = index.spots.find((s) => s.id === spotId);
  const flopSel = root.querySelector("#st-flop");
  flopSel.innerHTML = state.spot.flops.map((f) => `<option>${f.flop}</option>`).join("");
  flopSel.value = flop || state.spot.flops[0].flop;
  await selectFlop(root, flopSel.value);
}

async function selectFlop(root, flop) {
  state.flop = flop;
  state.sol = await loadSolution(state.spot.id, flop);
  root.querySelector("#st-board").innerHTML = cardsHtml(flop, "big");
  root.querySelector("#st-meta").textContent =
    `${state.spot.description} Accuracy: ${state.sol.exploitability_pct}% of pot.`;
  view.focus = null;
  view.pinned = false;
  showNode(root, state.sol.root);
}

function showNode(root, node) {
  if (node !== state.node) view.level = null;
  state.node = node;
  const spot = state.spot;
  const name = (p) => (p === "oop" ? spot.oop : spot.ip);

  // Breadcrumb of the line taken to get here.
  const crumbs = [`<a data-depth="0">Flop</a>`];
  node.path.forEach((label, i) => {
    const actor = state.sol.byPath.get(node.path.slice(0, i).join(" / "));
    crumbs.push(`<a data-depth="${i + 1}">${name(actor.player)} ${label}</a>`);
  });
  const pathEl = root.querySelector("#st-path");
  pathEl.innerHTML = crumbs.join(" › ");
  pathEl.onclick = (e) => {
    const d = e.target.dataset.depth;
    if (d !== undefined) showNode(root, state.sol.byPath.get(node.path.slice(0, +d).join(" / ")));
  };

  root.querySelector("#st-who").innerHTML =
    `<b>${name(node.player)}</b> to act · pot ${node.pot_bb}bb` + (node.to_call_bb ? ` · facing ${node.to_call_bb}bb` : "");

  const hands = nodeHands(node);
  const { freq, combos } = rangeFrequencies(hands, node.actions.length);
  root.querySelector("#st-freq").innerHTML =
    node.actions
      .map((a, i) => `<span style="width:${freq[i] * 100}%;background:${actionColor(a, node.actions)}"></span>`)
      .join("") ;

  const actionsEl = root.querySelector("#st-actions");
  actionsEl.innerHTML = node.actions
    .map((a, i) => {
      const next = node.children[i];
      return `<button data-i="${i}" ${next ? "" : "disabled"} title="${next ? "Go to this line" : "Line ends or goes to the turn"}">
        <i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}<em>${(freq[i] * 100).toFixed(1)}%</em></button>`;
    })
    .join("") + `<p class="hint">${combos.toFixed(1)} combos weighted by reach. Buttons step into the line.</p>`;
  actionsEl.onclick = (e) => {
    const i = e.target.closest("button")?.dataset.i;
    if (i !== undefined && node.children[i]) {
      view.focus = null;
      view.pinned = false;
      showNode(root, node.children[i]);
    }
  };

  const els = { grid: root.querySelector("#st-grid"), detail: root.querySelector("#st-detail"), range: root.querySelector("#st-range") };
  const simple = root.querySelector("#st-simple");
  if (view.mode === "simple") {
    els.range.textContent = "";
    const line = node.path.map((label, i) => `${name(state.sol.byPath.get(node.path.slice(0, i).join(" / ")).player)} ${label}`);
    const title = `${spot.name} · ${state.flop} · ${[...line, `${name(node.player)} to act`].join(" › ")}`;
    renderSimple({ ...els, simple }, node, view, { board: state.flop, name, title });
    return;
  }
  simple.innerHTML = "";
  renderNodeView(els, node, view, { board: state.flop, name });
}
