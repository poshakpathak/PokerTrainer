// The strategy grid, hand detail and range summary for one decision node.
// Shared by Study (precomputed library) and Solver (live solves).

import { cardsHtml } from "./cards.js";
import { nodeHands, classSummary, actionText, actionColor } from "./data.js";
import { renderGrid, liveCombos } from "./grid.js";

// Red (low) to green (high) for the EV and equity heatmaps.
const heat = (x) => `hsl(${Math.round(Math.max(0, Math.min(1, x)) * 130)} 60% 48%)`;

/**
 * @param els   {grid, detail, range} elements
 * @param node  decision node: {player, actions, hands, combos, pot_bb, equity?, ev_bb?}
 * @param view  persistent view state: {mode, focus, pinned}
 * @param opts  {board, name(player)}
 */
export function renderNodeView(els, node, view, opts) {
  const hands = nodeHands(node);
  const summary = classSummary(hands, node.actions.length);

  // Bar height is each class's share of its live combos that reach this node, scaled so the
  // most-reached class fills its cell. Deep in the tree absolute reach gets tiny.
  const share = new Map();
  for (const [cls, c] of summary) share.set(cls, c.reach / liveCombos(cls, opts.board));
  const maxShare = Math.max(1e-9, ...share.values());

  const draw = () =>
    renderGrid(els.grid, {
      selected: view.pinned ? view.focus : null,
      cell: (cls) => {
        const c = summary.get(cls);
        if (!c) return null;
        const fill = share.get(cls) / maxShare;
        if (view.mode === "ev") {
          return { fill, segments: [{ color: heat(c.value / node.pot_bb), frac: 1 }], label: c.value.toFixed(1) };
        }
        if (view.mode === "equity") {
          if (!c.hasEq) return { fill, segments: [], label: "n/a" };
          return { fill, segments: [{ color: heat(c.eq), frac: 1 }], label: `${Math.round(c.eq * 100)}` };
        }
        return {
          fill,
          segments: node.actions.map((a, i) => ({ color: actionColor(a, node.actions), frac: c.freq[i] })),
        };
      },
      onHover: (cls) => {
        if (!view.pinned) showDetail(els.detail, node, summary.get(cls), cls, view);
      },
      onClick: (cls) => {
        view.pinned = !(view.pinned && view.focus === cls);
        view.focus = cls;
        showDetail(els.detail, node, summary.get(cls), cls, view);
        draw();
      },
    });
  draw();
  showDetail(els.detail, node, view.focus ? summary.get(view.focus) : null, view.focus, view);

  if (node.equity) {
    const side = (p) => {
      const eq = node.equity[p === "oop" ? 0 : 1];
      const ev = node.ev_bb[p === "oop" ? 0 : 1];
      const eqr = eq > 0 ? ev / (eq * node.pot_bb) : 0;
      return `<b>${opts.name(p)}</b> equity ${(eq * 100).toFixed(1)}%, EV ${ev.toFixed(2)}bb, EQR ${(eqr * 100).toFixed(0)}%`;
    };
    els.range.innerHTML = `${side("oop")} · ${side("ip")}. EQR (equity realization) is EV divided by the share of the pot your equity would win.`;
  } else {
    els.range.textContent = view.mode === "strategy" ? "" : "This flop was solved before equity was exported. Re-solve it with --force to see equity.";
  }
}

function showDetail(el, node, c, cls, view) {
  view.focus = cls;
  if (!c) {
    el.innerHTML = cls
      ? `<h3>${cls}</h3><p class="hint">Not in this range at this point.</p>`
      : `<h3>Hand detail</h3><p class="hint">Hover a cell to see each combo's strategy and EV. Click to pin it.</p>`;
    return;
  }
  const head = node.actions.map((a) => `<th><i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}</th>`).join("");
  const row = (label, strat, ev, reach, eq) => {
    const best = Math.max(...ev);
    return `<tr><td>${label}${reach !== undefined ? `<small>${(reach * 100).toFixed(0)}%</small>` : ""}</td>${strat
      .map((s, i) => `<td class="${ev[i] === best ? "best" : ""}">${(s * 100).toFixed(0)}%<small>${ev[i].toFixed(2)}</small></td>`)
      .join("")}<td>${eq == null ? "–" : `${(eq * 100).toFixed(0)}%`}</td></tr>`;
  };
  el.innerHTML = `
    <h3>${cls} <small>${c.reach.toFixed(2)} combos</small></h3>
    <table class="combos">
      <thead><tr><th></th>${head}<th>Equity</th></tr></thead>
      <tbody>
        ${row("All", c.freq, c.ev, undefined, c.hasEq ? c.eq : null)}
        ${c.hands.map((h) => row(cardsHtml(h.combo, "tiny"), h.strat, h.ev, h.reach, h.eq)).join("")}
      </tbody>
    </table>
    <p class="hint">Each cell shows frequency and EV in bb. The small % next to a combo is how often it gets to this spot.</p>`;
}

/** Strategy / EV / Equity tab markup and wiring. */
export function modeTabsHtml(id) {
  return `<div class="tabs" id="${id}">
    <button data-mode="strategy" class="active">Strategy</button>
    <button data-mode="ev">EV</button>
    <button data-mode="equity">Equity</button>
  </div>`;
}

export function wireModeTabs(el, view, onChange) {
  el.onclick = (e) => {
    const mode = e.target.dataset.mode;
    if (!mode) return;
    view.mode = mode;
    el.querySelectorAll("button").forEach((b) => b.classList.toggle("active", b.dataset.mode === mode));
    onChange();
  };
}
