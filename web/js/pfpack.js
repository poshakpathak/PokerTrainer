// Reading a preflop solution: a pack from web/data/preflop/ or the local server's last solve.
// Walks the action tree and keeps every seat's range (the chance each hand class plays to the
// current point) along the way.
//
// Pack format (written by solver/src/preflop/report.rs, pack_json):
//   {format: "pt-preflop-1", config, iterations, exploitability_mbb, ev[6], nodes: [...]}
//   decision: {s: seat, a: [[kind, to], ...], c: [child ids], f: [[169 per-mille freqs] per action],
//              v: [[169 EVs in hundredths of a bb, compared with folding] per action]}
//   fold:     {t: "fold", s: winner}
//   flop:     {t: "flop", p: [seats in postflop order], pot, rake, spr}
//   cut:      {cut: true}  (a line too rare to keep in a precomputed pack)

import { gridClass, classCombos } from "./cards.js";

export const SEATS = ["UTG", "HJ", "CO", "BTN", "SB", "BB"];
/** The 169 classes in the solver's order: row * 13 + col of the grid. */
export const CLASSES = Array.from({ length: 169 }, (_, i) => gridClass(Math.floor(i / 13), i % 13));
export const COMBOS = CLASSES.map(classCombos);
export const CLASS_INDEX = new Map(CLASSES.map((c, i) => [c, i]));

const num = (x) => String(Math.round(x * 100) / 100);

export function actionLabel(kind, to, level) {
  switch (kind) {
    case "fold": return "Fold";
    case "check": return "Check";
    case "call": return level === 0 ? "Limp" : "Call";
    case "raise": return `Raise ${num(to)}`;
    case "allin": return `All-in ${num(to)}`;
    default: return kind;
  }
}

/** Fold blue, check and call green, raises from orange to red by size, all-in dark red. */
export function actionColor(actions, i) {
  const kind = actions[i][0];
  if (kind === "fold") return "var(--act-fold)";
  if (kind === "check" || kind === "call") return "var(--act-passive)";
  if (kind === "allin") return "var(--act-allin)";
  const raises = actions.map((a, j) => (a[0] === "raise" ? j : -1)).filter((j) => j >= 0);
  return ["var(--act-agg-1)", "var(--act-agg-2)", "var(--act-agg-3)"][Math.min(raises.indexOf(i), 2)];
}

/** A hand class's action frequencies at a decision (all zero if it never gets there). */
export function classFreqs(node, c) {
  let sum = 0;
  for (const f of node.f) sum += f[c];
  return node.f.map((f) => (sum > 0 ? f[c] / sum : 0));
}

/** A hand class's EV for each action, in bb compared with folding. */
export function classEvs(node, c) {
  return node.v.map((v) => v[c] / 100);
}

/** How often the whole range takes each action, weighted by combos and reach. */
export function actionFrequencies(node, reach) {
  const out = node.a.map(() => 0);
  let total = 0;
  for (let c = 0; c < 169; c++) {
    const w = COMBOS[c] * reach[c];
    if (!w) continue;
    const fr = classFreqs(node, c);
    for (let i = 0; i < out.length; i++) out[i] += w * fr[i];
    total += w;
  }
  return out.map((x) => x / (total || 1));
}

/** Combos in a range. */
export function rangeCombos(reach) {
  let n = 0;
  for (let c = 0; c < 169; c++) n += COMBOS[c] * reach[c];
  return n;
}

/** A range in solver syntax: "AA,AKs,AQs:0.62". */
export function rangeString(reach) {
  const parts = [];
  for (let c = 0; c < 169; c++) {
    const w = reach[c];
    if (w < 0.0005) continue;
    parts.push(w >= 0.9995 ? CLASSES[c] : `${CLASSES[c]}:${+w.toFixed(3)}`);
  }
  return parts.join(",");
}

export function firstSeat(pack) {
  const i = SEATS.indexOf(String(pack.config.start_seat || "UTG").toUpperCase());
  return i < 0 ? 0 : i;
}

/**
 * The state after following `history` (action indexes) from the root:
 * {id, node, reach[6], invested[6], folded: Set, line: [{seat, label, kind, to, id, step}], level, bet}.
 * Returns null if the line isn't in the pack.
 */
export function walk(pack, history) {
  const reach = Array.from({ length: 6 }, () => new Float32Array(169).fill(1));
  const invested = [0, 0, 0, 0, 0.5, 1];
  const folded = new Set();
  for (let s = 0; s < firstSeat(pack); s++) folded.add(s);
  let id = 0;
  let level = 0;
  let bet = 1;
  const line = [];
  for (const step of history) {
    const node = pack.nodes[id];
    if (!node || node.t || node.cut || step >= node.a.length) return null;
    const [kind, to] = node.a[step];
    line.push({ seat: node.s, kind, to, label: actionLabel(kind, to, level), id, step });
    const r = reach[node.s];
    for (let c = 0; c < 169; c++) if (r[c]) r[c] *= classFreqs(node, c)[step];
    if (kind === "fold") folded.add(node.s);
    else if (kind !== "check") invested[node.s] = to;
    if (kind === "raise" || kind === "allin") {
      level++;
      bet = to;
    }
    id = node.c[step];
  }
  return { id, node: pack.nodes[id], reach, invested, folded, line, level, bet };
}

/** The line as text without the folds: "UTG Raise 2.5, BB Call". */
export function lineText(line) {
  const steps = line.filter((s) => s.kind !== "fold").map((s) => `${SEATS[s.seat]} ${s.label}`);
  return steps.length ? steps.join(", ") : "Folded to";
}

/** Short config description: "100bb, 2.5bb opens, no rake". */
export function configSummary(cfg) {
  const rake = cfg.rake?.rate ? `${+(cfg.rake.rate * 100).toFixed(2)}% rake${cfg.rake.cap_bb ? ` capped at ${cfg.rake.cap_bb}bb` : ""}` : "no rake";
  const start = cfg.start_seat && cfg.start_seat.toUpperCase() !== "UTG" ? `, folded to the ${cfg.start_seat.toUpperCase()}` : "";
  return `${cfg.stack_bb}bb, ${cfg.open_bb}bb opens, SB ${cfg.sb_limp ? "limps or raises" : "raises or folds"}, ${rake}${start}`;
}
