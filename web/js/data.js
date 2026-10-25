// Loads the solved library from web/data/ and turns each flop file into a navigable tree.

import { handClass } from "./cards.js";

let indexPromise;
const solutionCache = new Map();

export function loadIndex() {
  indexPromise ??= fetch("data/index.json").then((r) => {
    if (!r.ok) throw new Error("data/index.json not found. Run scripts/build_library.py first.");
    return r.json();
  });
  return indexPromise;
}

export function loadSolution(spotId, flop) {
  const key = `${spotId}/${flop}`;
  if (!solutionCache.has(key)) {
    solutionCache.set(
      key,
      fetch(`data/${key}.json`)
        .then((r) => {
          if (!r.ok) throw new Error(`Missing solution ${key}`);
          return r.json();
        })
        .then(prepareSolution),
    );
  }
  return solutionCache.get(key);
}

/** Must match `action_label` in solver/src/main.rs, which formats floats Rust-style. */
export function actionLabel(a) {
  const num = (n) => (Number.isInteger(n) ? `${n}.0` : `${n}`);
  switch (a.type) {
    case "fold": return "Fold";
    case "check": return "Check";
    case "call": return "Call";
    case "bet": return `Bet ${a.pct}%`;
    case "raise": return `Raise ${num(a.to_bb)}bb`;
    case "allin": return "All-in";
    default: return "?";
  }
}

/** Short human label with sizing, for buttons and legends. */
export function actionText(a) {
  switch (a.type) {
    case "fold": return "Fold";
    case "check": return "Check";
    case "call": return `Call ${a.add_bb}`;
    case "bet": return `Bet ${a.add_bb} (${a.pct}%)`;
    case "raise": return `Raise to ${a.to_bb}`;
    case "allin": return `All-in ${a.add_bb}`;
    default: return "?";
  }
}

/** Colors in the usual solver convention: passive green, fold blue, aggression red by size. */
export function actionColor(a, actions) {
  if (a.type === "fold") return "var(--act-fold)";
  if (a.type === "check" || a.type === "call") return "var(--act-passive)";
  if (a.type === "allin") return "var(--act-allin)";
  const aggressive = actions.filter((x) => x.type === "bet" || x.type === "raise");
  const i = aggressive.indexOf(a);
  const shades = ["var(--act-agg-1)", "var(--act-agg-2)", "var(--act-agg-3)"];
  return shades[Math.min(i, shades.length - 1)];
}

function prepareSolution(sol) {
  const byPath = new Map();
  for (const node of sol.nodes) {
    node.key = node.path.join(" / ");
    node.combos = sol.hands[node.player];
    node.labels = node.actions.map(actionLabel);
    byPath.set(node.key, node);
  }
  for (const node of sol.nodes) {
    node.children = node.labels.map((label) => byPath.get([...node.path, label].join(" / ")) || null);
  }
  sol.root = byPath.get("");
  sol.byPath = byPath;
  return sol;
}

/**
 * Per-hand view of a node: [{combo, cls, reach, strat[], ev[]}] for hands that reach it.
 */
export function nodeHands(node) {
  const n = node.actions.length;
  const out = [];
  node.hands.forEach((row, i) => {
    if (!row) return;
    const combo = node.combos[i];
    out.push({
      combo,
      cls: handClass(combo),
      reach: row[0],
      strat: row.slice(1, 1 + n),
      ev: row.slice(1 + n, 1 + 2 * n),
    });
  });
  return out;
}

/** Range-wide action frequencies at a node, weighted by how often each hand gets there. */
export function rangeFrequencies(hands, nActions) {
  const freq = new Array(nActions).fill(0);
  let total = 0;
  for (const h of hands) {
    total += h.reach;
    h.strat.forEach((s, a) => (freq[a] += s * h.reach));
  }
  return { freq: freq.map((f) => (total ? f / total : 0)), combos: total };
}

/** Groups a node's hands by class for the 13x13 grid. */
export function classSummary(hands, nActions) {
  const map = new Map();
  for (const h of hands) {
    let c = map.get(h.cls);
    if (!c) {
      c = { cls: h.cls, reach: 0, possible: 0, freq: new Array(nActions).fill(0), ev: new Array(nActions).fill(0), hands: [] };
      map.set(h.cls, c);
    }
    c.reach += h.reach;
    c.hands.push(h);
    h.strat.forEach((s, a) => {
      c.freq[a] += s * h.reach;
      c.ev[a] += h.ev[a] * h.reach;
    });
  }
  for (const c of map.values()) {
    if (c.reach > 0) {
      c.freq = c.freq.map((f) => f / c.reach);
      c.ev = c.ev.map((e) => e / c.reach);
    }
  }
  return map;
}
