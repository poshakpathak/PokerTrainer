// Trainer: deals a random hand at a random solved node, asks for an action,
// then scores it by EV lost against the solver's best action.

import { cardsHtml } from "./cards.js";
import { loadIndex, loadSolution, nodeHands, actionText, actionColor } from "./data.js";

const STORE_KEY = "pokertrainer.stats.v1";
const DEFAULT_STATS = { hands: 0, correct: 0, evLossBb: 0, evLossPotPct: 0, history: [] };

const settings = { spots: null, seat: "both", depth: "first" };
let stats = loadStats();
let current = null;

function loadStats() {
  try {
    return { ...DEFAULT_STATS, ...JSON.parse(localStorage.getItem(STORE_KEY) || "{}") };
  } catch {
    return { ...DEFAULT_STATS };
  }
}

function saveStats() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(stats));
  } catch {
    // Private mode or blocked storage: stats just last for this visit.
  }
}

export async function initTrainer(root, { onOpenStudy }) {
  const index = await loadIndex();
  const spots = index.spots.filter((s) => s.flops.length);
  settings.spots ??= new Set(spots.map((s) => s.id));

  root.innerHTML = `
    <div class="trainer">
      <aside class="panel controls">
        <h3>Drill settings</h3>
        <fieldset id="tr-spots"><legend>Spots</legend>
          ${spots.map((s) => `<label class="check"><input type="checkbox" value="${s.id}" ${settings.spots.has(s.id) ? "checked" : ""}> ${s.name}</label>`).join("")}
        </fieldset>
        <label>Your seat
          <select id="tr-seat">
            <option value="both">Either player</option>
            <option value="oop">Out of position</option>
            <option value="ip">In position</option>
          </select>
        </label>
        <label>Decisions
          <select id="tr-depth">
            <option value="first">First decision on the flop</option>
            <option value="facing">Facing a bet or raise</option>
            <option value="all">Any flop decision</option>
          </select>
        </label>
        <div id="tr-stats" class="stats"></div>
        <button id="tr-reset" class="link">Reset stats</button>
      </aside>
      <section class="panel table" id="tr-table"></section>
      <aside class="panel detail" id="tr-history"></aside>
    </div>`;

  root.querySelector("#tr-seat").value = settings.seat;
  root.querySelector("#tr-depth").value = settings.depth;
  root.querySelector("#tr-spots").onchange = (e) => {
    e.target.checked ? settings.spots.add(e.target.value) : settings.spots.delete(e.target.value);
    deal(root, index, onOpenStudy);
  };
  root.querySelector("#tr-seat").onchange = (e) => { settings.seat = e.target.value; deal(root, index, onOpenStudy); };
  root.querySelector("#tr-depth").onchange = (e) => { settings.depth = e.target.value; deal(root, index, onOpenStudy); };
  root.querySelector("#tr-reset").onclick = () => {
    stats = { ...DEFAULT_STATS, history: [] };
    saveStats();
    renderStats(root);
  };

  document.addEventListener("keydown", (e) => {
    if (!root.isConnected || root.hidden || !current || e.target.tagName === "SELECT") return;
    if (current.answered && (e.key === " " || e.key === "Enter")) {
      e.preventDefault();
      deal(root, index, onOpenStudy);
    } else if (!current.answered && /^[1-9]$/.test(e.key)) {
      const i = +e.key - 1;
      if (i < current.node.actions.length) answer(root, index, onOpenStudy, i);
    }
  });

  renderStats(root);
  await deal(root, index, onOpenStudy);
}

const pick = (arr) => arr[Math.floor(Math.random() * arr.length)];

function pickWeighted(items, weight) {
  const total = items.reduce((s, x) => s + weight(x), 0);
  let r = Math.random() * total;
  for (const x of items) {
    r -= weight(x);
    if (r <= 0) return x;
  }
  return items[items.length - 1];
}

function nodeMatches(node) {
  if (settings.seat !== "both" && node.player !== settings.seat) return false;
  if (settings.depth === "first") return node.path.length === 0 || (node.path.length === 1 && node.path[0] === "Check");
  if (settings.depth === "facing") return node.to_call_bb > 0;
  return true;
}

async function deal(root, index, onOpenStudy) {
  const table = root.querySelector("#tr-table");
  const spots = index.spots.filter((s) => s.flops.length && settings.spots.has(s.id));
  if (!spots.length) {
    table.innerHTML = `<p class="hint">Pick at least one spot.</p>`;
    current = null;
    return;
  }
  // A few attempts in case a flop has no node matching the filters.
  for (let attempt = 0; attempt < 20; attempt++) {
    const spot = pick(spots);
    const flop = pick(spot.flops).flop;
    const sol = await loadSolution(spot.id, flop);
    // Weight nodes by how much of the range gets there, so common spots come up more.
    const nodes = sol.nodes
      .filter(nodeMatches)
      .map((n) => ({ n, hands: nodeHands(n) }))
      .filter((x) => x.hands.length);
    if (!nodes.length) continue;
    const { n: node, hands } = pickWeighted(nodes, (x) => x.hands.reduce((s, h) => s + h.reach, 0));
    const hand = pickWeighted(hands, (h) => h.reach);
    current = { spot, flop, sol, node, hand, answered: false };
    renderQuestion(root, index, onOpenStudy);
    return;
  }
  table.innerHTML = `<p class="hint">No solved decisions match these settings yet.</p>`;
  current = null;
}

function lineText(spot, sol, node) {
  if (!node.path.length) return "First to act on the flop.";
  return node.path
    .map((label, i) => {
      const actor = sol.byPath.get(node.path.slice(0, i).join(" / "));
      return `${actor.player === "oop" ? spot.oop : spot.ip} ${label.toLowerCase()}`;
    })
    .join(", ") + ".";
}

function renderQuestion(root, index, onOpenStudy) {
  const { spot, flop, sol, node, hand } = current;
  const hero = node.player === "oop" ? spot.oop : spot.ip;
  const villain = node.player === "oop" ? spot.ip : spot.oop;
  const table = root.querySelector("#tr-table");
  table.innerHTML = `
    <p class="spotname">${spot.name}</p>
    <div class="board">${cardsHtml(flop, "big")}</div>
    <p class="line">${lineText(spot, sol, node)}</p>
    <p class="pot">Pot <b>${node.pot_bb}bb</b>${node.to_call_bb ? ` · ${villain} bet, you face <b>${node.to_call_bb}bb</b>` : ""}</p>
    <div class="hero"><span>You are <b>${hero}</b> (${node.player === "oop" ? "out of position" : "in position"}) with</span>
      <div class="hole">${cardsHtml(hand.combo, "big")}</div></div>
    <div class="answers">
      ${node.actions.map((a, i) => `<button data-i="${i}"><kbd>${i + 1}</kbd>${actionText(a)}</button>`).join("")}
    </div>
    <div id="tr-result"></div>`;
  table.querySelector(".answers").onclick = (e) => {
    const i = e.target.closest("button")?.dataset.i;
    if (i !== undefined && !current.answered) answer(root, index, onOpenStudy, +i);
  };
}

function grade(lossPotPct, freq) {
  if (lossPotPct <= 0.5 || freq >= 0.2) return { label: "Correct", cls: "good" };
  if (lossPotPct <= 3) return { label: "Small mistake", cls: "ok" };
  if (lossPotPct <= 10) return { label: "Mistake", cls: "bad" };
  return { label: "Blunder", cls: "bad" };
}

function answer(root, index, onOpenStudy, choice) {
  const { spot, flop, node, hand } = current;
  current.answered = true;
  const best = Math.max(...hand.ev);
  const lossBb = Math.max(0, best - hand.ev[choice]);
  const lossPotPct = (lossBb / node.pot_bb) * 100;
  const g = grade(lossPotPct, hand.strat[choice]);

  stats.hands += 1;
  if (g.cls === "good") stats.correct += 1;
  stats.evLossBb += lossBb;
  stats.evLossPotPct += lossPotPct;
  stats.history.unshift({
    spot: spot.id, spotName: spot.name, flop, path: node.path, combo: hand.combo,
    chose: actionText(node.actions[choice]), lossBb: +lossBb.toFixed(2), grade: g.label, cls: g.cls,
  });
  stats.history = stats.history.slice(0, 30);
  saveStats();

  const buttons = root.querySelectorAll("#tr-table .answers button");
  buttons.forEach((b, i) => {
    b.disabled = true;
    if (i === choice) b.classList.add("chosen");
  });

  const rows = node.actions
    .map((a, i) => `<tr class="${i === choice ? "chosen" : ""}">
      <td><i style="background:${actionColor(a, node.actions)}"></i>${actionText(a)}</td>
      <td><div class="mini"><span style="width:${hand.strat[i] * 100}%;background:${actionColor(a, node.actions)}"></span></div>${(hand.strat[i] * 100).toFixed(0)}%</td>
      <td>${hand.ev[i].toFixed(2)}bb</td>
      <td>${(best - hand.ev[i]).toFixed(2)}</td></tr>`)
    .join("");

  root.querySelector("#tr-result").innerHTML = `
    <div class="verdict ${g.cls}"><b>${g.label}</b> · EV lost ${lossBb.toFixed(2)}bb (${lossPotPct.toFixed(1)}% of pot)</div>
    <table class="combos result">
      <thead><tr><th>Action</th><th>Solver frequency</th><th>EV</th><th>Loss</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
    <p class="hint">EV is what the hand expects to win from here in bb, counting the chips already in the pot. Loss is the gap to the best action.</p>
    <div class="next">
      <button id="tr-next" class="primary">Next hand <kbd>Space</kbd></button>
      <button id="tr-study" class="link">See the whole range in Study</button>
    </div>`;
  root.querySelector("#tr-next").onclick = () => deal(root, index, onOpenStudy);
  root.querySelector("#tr-study").onclick = () => onOpenStudy(spot.id, flop, node.path, hand.cls);
  renderStats(root);
}

function renderStats(root) {
  const avgPot = stats.hands ? stats.evLossPotPct / stats.hands : 0;
  const avgBb = stats.hands ? stats.evLossBb / stats.hands : 0;
  root.querySelector("#tr-stats").innerHTML = `
    <div><b>${stats.hands}</b><span>hands</span></div>
    <div><b>${stats.hands ? Math.round((stats.correct / stats.hands) * 100) : 0}%</b><span>correct</span></div>
    <div><b>${avgBb.toFixed(2)}</b><span>avg bb lost</span></div>
    <div><b>${avgPot.toFixed(1)}%</b><span>avg pot lost</span></div>`;
  root.querySelector("#tr-history").innerHTML = `<h3>Recent hands</h3>` + (stats.history.length
    ? `<ol class="history">${stats.history
        .map((h) => `<li class="${h.cls}"><span>${cardsHtml(h.combo, "tiny")} on ${cardsHtml(h.flop, "tiny")}</span>
          <span>${h.chose}</span><span>${h.grade}${h.lossBb ? `, −${h.lossBb}bb` : ""}</span></li>`)
        .join("")}</ol>`
    : `<p class="hint">Your answers show up here. Keys 1 to 9 pick an action, Space deals the next hand.</p>`);
}
