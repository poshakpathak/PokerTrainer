// Reports view: one spot's first-decision strategy across every solved flop,
// so you can see how c-bet frequency and sizing change with board texture.

import { RANKS, cardsHtml, splitCards } from "./cards.js";
import { loadIndex, actionText, actionColor } from "./data.js";

const NODES = {
  oop_first: (spot) => `${spot.oop} first to act`,
  ip_vs_check: (spot) => `${spot.ip} after ${spot.oop} checks`,
};

/** Texture tags for a flop like "Ks7d2c". */
export function flopTextures(flop) {
  const cards = splitCards(flop);
  const ranks = cards.map((c) => RANKS.indexOf(c[0])).sort((a, b) => a - b); // 0 = ace
  const suits = new Set(cards.map((c) => c[1])).size;
  const tags = [];
  tags.push(suits === 1 ? "Monotone" : suits === 2 ? "Two-tone" : "Rainbow");
  if (new Set(ranks).size < 3) tags.push("Paired");
  const high = ranks[0];
  tags.push(high === 0 ? "A-high" : high <= 3 ? "Broadway-high" : high <= 6 ? "Middle" : "Low");
  // Connected: all three ranks fit in a 5-rank window (straight possible), counting the wheel.
  const distinct = [...new Set(ranks)];
  const span = (rs) => Math.max(...rs) - Math.min(...rs);
  const wheel = distinct.map((r) => (r === 0 ? 13 : r));
  if (distinct.length === 3 && (span(distinct) <= 4 || span(wheel) <= 4)) tags.push("Connected");
  return tags;
}

const state = { spot: null, node: "ip_vs_check", sort: { key: "flop", dir: 1 }, filter: "All" };

export async function initReports(root, { onOpenStudy }) {
  const index = await loadIndex();
  const spots = index.spots.filter((s) => s.flops.some((f) => f.report));
  if (!spots.length) {
    root.innerHTML = `<p class="panel">No reports yet. Run <code>python3 scripts/build_library.py --index-only</code>.</p>`;
    return;
  }
  state.spot = spots[0];
  root.innerHTML = `
    <div class="reports">
      <aside class="panel controls">
        <label>Spot <select id="rp-spot">${spots.map((s) => `<option value="${s.id}">${s.name}</option>`).join("")}</select></label>
        <label>Decision <select id="rp-node"></select></label>
        <label>Texture <select id="rp-filter"></select></label>
        <div id="rp-avg"></div>
        <p class="hint">Click a column to sort, or a row to open that flop in Study.</p>
      </aside>
      <section class="panel"><div class="scroll"><table class="report" id="rp-table"></table></div></section>
    </div>`;
  const nodeSel = root.querySelector("#rp-node");
  const filterSel = root.querySelector("#rp-filter");
  const tags = ["All", "Rainbow", "Two-tone", "Monotone", "Paired", "Connected", "A-high", "Broadway-high", "Middle", "Low"];
  filterSel.innerHTML = tags.map((t) => `<option>${t}</option>`).join("");

  const fillNodes = () => {
    nodeSel.innerHTML = Object.entries(NODES).map(([k, f]) => `<option value="${k}">${f(state.spot)}</option>`).join("");
    nodeSel.value = state.node;
  };
  root.querySelector("#rp-spot").onchange = (e) => {
    state.spot = spots.find((s) => s.id === e.target.value);
    fillNodes();
    draw(root, onOpenStudy);
  };
  nodeSel.onchange = (e) => { state.node = e.target.value; draw(root, onOpenStudy); };
  filterSel.onchange = (e) => { state.filter = e.target.value; draw(root, onOpenStudy); };
  fillNodes();
  draw(root, onOpenStudy);
}

function draw(root, onOpenStudy) {
  const spot = state.spot;
  const rows = spot.flops
    .filter((f) => f.report?.[state.node])
    .map((f) => ({ flop: f.flop, tags: flopTextures(f.flop), ...f.report[state.node] }))
    .filter((r) => state.filter === "All" || r.tags.includes(state.filter));
  const table = root.querySelector("#rp-table");
  if (!rows.length) {
    table.innerHTML = `<tr><td class="hint">No flops match.</td></tr>`;
    root.querySelector("#rp-avg").innerHTML = "";
    return;
  }

  // Columns come from the first row; the bundled spots use the same sizes on every flop.
  const actions = rows[0].actions;
  const col = (r, i) => r.freq[i] ?? 0;
  const hasEq = rows.every((r) => r.equity);
  const { key, dir } = state.sort;
  rows.sort((a, b) => {
    let va, vb;
    if (key === "flop") {
      [va, vb] = [a.flop, b.flop].map((f) => splitCards(f).map((c) => RANKS.indexOf(c[0])).join(","));
    } else if (key === "eq") {
      [va, vb] = [a.equity?.[0] ?? 0, b.equity?.[0] ?? 0];
    } else {
      [va, vb] = [col(a, +key), col(b, +key)];
    }
    return va < vb ? -dir : va > vb ? dir : 0;
  });

  const arrow = (k) => (state.sort.key === String(k) ? (state.sort.dir > 0 ? " ▲" : " ▼") : "");
  table.innerHTML = `
    <thead><tr>
      <th data-sort="flop">Flop${arrow("flop")}</th>
      <th>Texture</th>
      <th>Strategy</th>
      ${actions.map((a, i) => `<th data-sort="${i}"><i style="background:${actionColor(a, actions)}"></i>${actionText(a)}${arrow(i)}</th>`).join("")}
      ${hasEq ? `<th data-sort="eq">${spot.oop} equity${arrow("eq")}</th>` : ""}
    </tr></thead>
    <tbody>${rows
      .map((r) => `<tr data-flop="${r.flop}">
        <td>${cardsHtml(r.flop, "tiny")}</td>
        <td class="tags">${r.tags.join(", ")}</td>
        <td><div class="freqbar">${actions.map((a, i) => `<span style="width:${col(r, i) * 100}%;background:${actionColor(a, actions)}"></span>`).join("")}</div></td>
        ${actions.map((_, i) => `<td>${(col(r, i) * 100).toFixed(0)}%</td>`).join("")}
        ${hasEq ? `<td>${(r.equity[0] * 100).toFixed(1)}%</td>` : ""}
      </tr>`)
      .join("")}</tbody>`;

  table.querySelector("thead").onclick = (e) => {
    const k = e.target.closest("th")?.dataset.sort;
    if (k === undefined) return;
    state.sort = { key: k, dir: state.sort.key === k ? -state.sort.dir : k === "flop" ? 1 : -1 };
    draw(root, onOpenStudy);
  };
  table.querySelector("tbody").onclick = (e) => {
    const flop = e.target.closest("tr")?.dataset.flop;
    if (flop) onOpenStudy(spot.id, flop, state.node === "ip_vs_check" ? ["Check"] : []);
  };

  const avg = actions.map((_, i) => rows.reduce((s, r) => s + col(r, i), 0) / rows.length);
  root.querySelector("#rp-avg").innerHTML = `
    <h3>Average over ${rows.length} flop${rows.length > 1 ? "s" : ""}</h3>
    <div class="freqbar">${actions.map((a, i) => `<span style="width:${avg[i] * 100}%;background:${actionColor(a, actions)}"></span>`).join("")}</div>
    ${actions.map((a, i) => `<div class="avgrow"><i style="background:${actionColor(a, actions)}"></i>${actionText(a)}<b>${(avg[i] * 100).toFixed(1)}%</b></div>`).join("")}`;
}
