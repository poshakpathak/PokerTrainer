// The 13x13 hand grid. Each cell shows a stacked bar of action frequencies whose
// height is the share of that hand class's combos that reach the node.

import { RANKS, gridClass, expandClass, splitCards } from "./cards.js";

/**
 * @param {HTMLElement} el
 * @param {object} opts
 *   cell(cls) -> { fill: 0..1, segments: [{color, frac}], label? } | null
 *   onHover(cls), onClick(cls), selected
 */
export function renderGrid(el, opts) {
  el.classList.add("grid");
  el.innerHTML = "";
  for (let r = 0; r < 13; r++) {
    for (let c = 0; c < 13; c++) {
      const cls = gridClass(r, c);
      const info = opts.cell(cls);
      const cell = document.createElement("div");
      cell.className = "cell" + (info ? "" : " empty") + (opts.selected === cls ? " selected" : "");
      cell.dataset.cls = cls;
      if (info) {
        const bar = document.createElement("div");
        bar.className = "bar";
        bar.style.height = `${Math.max(0, Math.min(1, info.fill)) * 100}%`;
        for (const seg of info.segments) {
          if (seg.frac <= 0.0005) continue;
          const s = document.createElement("span");
          s.style.background = seg.color;
          s.style.width = `${seg.frac * 100}%`;
          bar.appendChild(s);
        }
        cell.appendChild(bar);
      }
      const label = document.createElement("b");
      label.textContent = cls;
      cell.appendChild(label);
      if (info?.label) {
        const sub = document.createElement("i");
        sub.textContent = info.label;
        cell.appendChild(sub);
      }
      el.appendChild(cell);
    }
  }
  el.onmouseover = (e) => {
    const cls = e.target.closest(".cell")?.dataset.cls;
    if (cls && opts.onHover) opts.onHover(cls);
  };
  el.onclick = (e) => {
    const cls = e.target.closest(".cell")?.dataset.cls;
    if (cls && opts.onClick) opts.onClick(cls);
  };
}

/** Number of combos of a class that don't use a board card. */
export function liveCombos(cls, board) {
  const dead = new Set(splitCards(board));
  return expandClass(cls).filter((c) => !dead.has(c.slice(0, 2)) && !dead.has(c.slice(2))).length;
}

export { RANKS };
