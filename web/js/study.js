// Study view: browse a solved flop node by node, like a solver's strategy browser.

import { cardsHtml } from "./cards.js";
import { loadIndex, loadSolution, nodeHands, rangeFrequencies, classSummary, actionText, actionColor } from "./data.js";
import { renderGrid, liveCombos } from "./grid.js";

const state = { spot: null, flop: null, sol: null, node: null, focus: null, pinned: false };

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
      <section class="panel"><div id="st-grid"></div></section>
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
  await selectSpot(root, index, spots[0].id);
}

/** Opens a specific spot/flop/node, used by the trainer's "open in study" link. */
export async function openInStudy(root, spotId, flop, path, cls) {
  const index = await loadIndex();
  root.querySelector("#st-spot").value = spotId;
  await selectSpot(root, index, spotId, flop);
  const node = state.sol.byPath.get(path.join(" / "));
  if (node) {
    state.focus = cls || null;
    state.pinned = !!cls;
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
  state.focus = null;
  state.pinned = false;
  showNode(root, state.sol.root);
}

function showNode(root, node) {
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
      state.focus = null;
      state.pinned = false;
      showNode(root, node.children[i]);
    }
  };

  const summary = classSummary(hands, node.actions.length);
  const draw = () =>
    renderGrid(root.querySelector("#st-grid"), {
      selected: state.pinned ? state.focus : null,
      cell: (cls) => {
        const c = summary.get(cls);
        if (!c) return null;
        return {
          fill: c.reach / liveCombos(cls, state.flop),
          segments: node.actions.map((a, i) => ({ color: actionColor(a, node.actions), frac: c.freq[i] })),
        };
      },
      onHover: (cls) => {
        if (!state.pinned) showDetail(root, node, summary.get(cls), cls);
      },
      onClick: (cls) => {
        state.pinned = !(state.pinned && state.focus === cls);
        state.focus = cls;
        showDetail(root, node, summary.get(cls), cls);
        draw();
      },
    });
  draw();
  showDetail(root, node, state.focus ? summary.get(state.focus) : null, state.focus);
}

function showDetail(root, node, c, cls) {
  state.focus = cls;
  const el = root.querySelector("#st-detail");
  if (!c) {
    el.innerHTML = cls
      ? `<h3>${cls}</h3><p class="hint">Not in this range at this point.</p>`
      : `<h3>Hand detail</h3><p class="hint">Hover a cell to see each combo's strategy and EV. Click to pin it.</p>`;
    return;
  }
  const head = node.actions.map((a) => `<th><i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}</th>`).join("");
  const row = (label, strat, ev, reach) => {
    const best = Math.max(...ev);
    return `<tr><td>${label}${reach !== undefined ? `<small>${(reach * 100).toFixed(0)}%</small>` : ""}</td>${strat
      .map((s, i) => `<td class="${ev[i] === best ? "best" : ""}">${(s * 100).toFixed(0)}%<small>${ev[i].toFixed(2)}</small></td>`)
      .join("")}</tr>`;
  };
  el.innerHTML = `
    <h3>${cls} <small>${c.reach.toFixed(2)} combos</small></h3>
    <table class="combos">
      <thead><tr><th></th>${head}</tr></thead>
      <tbody>
        ${row("All", c.freq, c.ev)}
        ${c.hands.map((h) => row(cardsHtml(h.combo, "tiny"), h.strat, h.ev, h.reach)).join("")}
      </tbody>
    </table>
    <p class="hint">Each cell shows frequency and EV in bb. The small % next to a combo is how often it gets to this spot.</p>`;
}
