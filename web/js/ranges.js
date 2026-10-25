// Ranges view: the preflop ranges each spot was solved with.

import { parseRange } from "./cards.js";
import { loadIndex } from "./data.js";
import { renderGrid } from "./grid.js";

export async function initRanges(root) {
  const index = await loadIndex();
  root.innerHTML = `
    <div class="ranges">
      <aside class="panel controls">
        <label>Spot <select id="rg-spot">${index.spots.map((s) => `<option value="${s.id}">${s.name}</option>`).join("")}</select></label>
        <p id="rg-desc" class="meta"></p>
        <p class="hint">These are the starting ranges fed to the solver. Edit them in <code>spots/*.json</code> and re-run the build script to solve with your own.</p>
      </aside>
      <section class="panel"><h3 id="rg-oop-title"></h3><div id="rg-oop"></div></section>
      <section class="panel"><h3 id="rg-ip-title"></h3><div id="rg-ip"></div></section>
    </div>`;
  const sel = root.querySelector("#rg-spot");
  const show = () => {
    const spot = index.spots.find((s) => s.id === sel.value);
    root.querySelector("#rg-desc").textContent = spot.description;
    for (const side of ["oop", "ip"]) {
      const weights = parseRange(spot.ranges[side]);
      const pct = [...weights].reduce((s, [cls, w]) => s + w * (cls.length === 2 ? 6 : cls[2] === "s" ? 4 : 12), 0) / 1326 * 100;
      root.querySelector(`#rg-${side}-title`).textContent = `${spot[side]} (${side === "oop" ? "out of position" : "in position"}) · ${pct.toFixed(1)}% of hands`;
      renderGrid(root.querySelector(`#rg-${side}`), {
        cell: (cls) => {
          const w = weights.get(cls);
          return w ? { fill: 1, segments: [{ color: "var(--act-range)", frac: w }] } : null;
        },
      });
    }
  };
  sel.onchange = show;
  show();
}
