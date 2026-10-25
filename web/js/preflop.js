// Preflop trainer: open or fold from each seat, checked against the opening charts.

import { gridClass, classCombos, expandClass, parseRange, cardsHtml } from "./cards.js";
import { renderGrid } from "./grid.js";

const STORE_KEY = "pokertrainer.preflop.v1";
const state = { chart: null, seat: "any", current: null, stats: loadStats() };

function loadStats() {
  try {
    return { hands: 0, correct: 0, ...JSON.parse(localStorage.getItem(STORE_KEY) || "{}") };
  } catch {
    return { hands: 0, correct: 0 };
  }
}

function saveStats() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(state.stats));
  } catch {}
}

export async function initPreflop(root) {
  const res = await fetch("data/preflop.json");
  if (!res.ok) throw new Error("data/preflop.json not found.");
  const chart = await res.json();
  state.chart = chart.positions.map((p) => ({ ...p, weights: parseRange(p.range) }));

  root.innerHTML = `
    <div class="trainer preflop">
      <aside class="panel controls">
        <h3>Preflop opens</h3>
        <label>Seat
          <select id="pf-seat">
            <option value="any">Any seat</option>
            ${state.chart.map((p) => `<option>${p.id}</option>`).join("")}
          </select>
        </label>
        <div id="pf-stats" class="stats"></div>
        <button id="pf-reset" class="link">Reset stats</button>
        <p class="hint">${chart.note}</p>
      </aside>
      <section class="panel table" id="pf-table"></section>
      <aside class="panel"><h3 id="pf-chart-title"></h3><div id="pf-chart"></div></aside>
    </div>`;
  root.querySelector("#pf-seat").onchange = (e) => {
    state.seat = e.target.value;
    deal(root);
  };
  root.querySelector("#pf-reset").onclick = () => {
    state.stats = { hands: 0, correct: 0 };
    saveStats();
    renderStats(root);
  };
  document.addEventListener("keydown", (e) => {
    if (!root.isConnected || root.hidden || !state.current || e.target.tagName === "SELECT") return;
    if (state.current.answered && (e.key === " " || e.key === "Enter")) {
      e.preventDefault();
      deal(root);
    } else if (!state.current.answered && (e.key === "1" || e.key === "2")) {
      answer(root, e.key === "1" ? "fold" : "raise");
    }
  });
  renderStats(root);
  deal(root);
}

/** Hands in the range plus their grid neighbours: the decisions worth drilling. */
function candidates(pos) {
  const out = [];
  for (let r = 0; r < 13; r++) {
    for (let c = 0; c < 13; c++) {
      let near = false;
      for (let dr = -1; dr <= 1 && !near; dr++) {
        for (let dc = -1; dc <= 1 && !near; dc++) {
          const rr = r + dr;
          const cc = c + dc;
          if (rr >= 0 && rr < 13 && cc >= 0 && cc < 13 && pos.weights.has(gridClass(rr, cc))) near = true;
        }
      }
      if (near) out.push(gridClass(r, c));
    }
  }
  return out;
}

function deal(root) {
  const seats = state.seat === "any" ? state.chart : state.chart.filter((p) => p.id === state.seat);
  const pos = seats[Math.floor(Math.random() * seats.length)];
  const pool = candidates(pos);
  // Weight by combos so offsuit hands come up as often as they do at the table.
  const total = pool.reduce((s, cls) => s + classCombos(cls), 0);
  let x = Math.random() * total;
  let cls = pool[pool.length - 1];
  for (const c of pool) {
    x -= classCombos(c);
    if (x <= 0) {
      cls = c;
      break;
    }
  }
  const combos = expandClass(cls);
  const combo = combos[Math.floor(Math.random() * combos.length)];
  state.current = { pos, cls, combo, answered: false };

  const folded = state.chart.slice(0, state.chart.indexOf(pos)).map((p) => p.id);
  root.querySelector("#pf-table").innerHTML = `
    <p class="spotname">${folded.length ? `${folded.join(", ")} fold${folded.length === 1 ? "s" : ""} to you.` : "You're first to act."}</p>
    <div class="hero"><span>You are <b>${pos.id}</b> with</span><div class="hole">${cardsHtml(combo, "big")}</div></div>
    <div class="answers">
      <button data-a="fold"><kbd>1</kbd>Fold</button>
      <button data-a="raise"><kbd>2</kbd>Raise</button>
    </div>
    <div id="pf-result"></div>`;
  root.querySelector("#pf-table .answers").onclick = (e) => {
    const a = e.target.closest("button")?.dataset.a;
    if (a && !state.current.answered) answer(root, a);
  };
  drawChart(root, pos, null);
}

function answer(root, choice) {
  const { pos, cls } = state.current;
  state.current.answered = true;
  const freq = pos.weights.get(cls) || 0;
  const chosenFreq = choice === "raise" ? freq : 1 - freq;
  // A mixed hand counts as correct either way unless one action is rare.
  const correct = chosenFreq >= 0.25;
  state.stats.hands += 1;
  if (correct) state.stats.correct += 1;
  saveStats();

  root.querySelectorAll("#pf-table .answers button").forEach((b) => {
    b.disabled = true;
    if (b.dataset.a === choice) b.classList.add("chosen");
  });
  const verdict = freq === 1 ? "always an open" : freq === 0 ? "a fold" : `an open ${Math.round(freq * 100)}% of the time`;
  root.querySelector("#pf-result").innerHTML = `
    <div class="verdict ${correct ? "good" : "bad"}"><b>${correct ? "Correct" : "Wrong"}</b> · ${cls} from ${pos.id} is ${verdict}.</div>
    <div class="next"><button id="pf-next" class="primary">Next hand <kbd>Space</kbd></button></div>`;
  root.querySelector("#pf-next").onclick = () => deal(root);
  drawChart(root, pos, cls);
  renderStats(root);
}

function drawChart(root, pos, highlight) {
  const pct = [...pos.weights].reduce((s, [cls, w]) => s + w * classCombos(cls), 0) / 1326 * 100;
  const title = root.querySelector("#pf-chart-title");
  const chart = root.querySelector("#pf-chart");
  if (!highlight) {
    // Hide the answer until the question is answered.
    title.textContent = `${pos.id} opening range`;
    chart.innerHTML = `<p class="hint">The chart appears after you answer.</p>`;
    return;
  }
  title.textContent = `${pos.id} opening range · ${pct.toFixed(1)}% of hands`;
  renderGrid(chart, {
    selected: highlight,
    cell: (cls) => {
      const w = pos.weights.get(cls);
      return w ? { fill: 1, segments: [{ color: "var(--act-agg-2)", frac: w }] } : null;
    },
  });
}

function renderStats(root) {
  const { hands, correct } = state.stats;
  root.querySelector("#pf-stats").innerHTML = `
    <div><b>${hands}</b><span>hands</span></div>
    <div><b>${hands ? Math.round((correct / hands) * 100) : 0}%</b><span>correct</span></div>`;
}
