// Seven-card hand evaluation, for showing who wins a drilled hand at showdown.

import { splitCards } from "./cards.js";

const RANK = "23456789TJQKA";
const NAMES = ["high card", "a pair", "two pair", "three of a kind", "a straight", "a flush", "a full house", "four of a kind", "a straight flush"];

/** [category, tiebreaks...] for five cards like ["As", "Kd", ...]; higher compares better. */
function rank5(cards) {
  const ranks = cards.map((c) => RANK.indexOf(c[0])).sort((a, b) => b - a);
  const flush = cards.every((c) => c[1] === cards[0][1]);
  const distinct = new Set(ranks).size === 5;
  let straight = -1;
  if (distinct && ranks[0] - ranks[4] === 4) straight = ranks[0];
  else if (distinct && ranks[0] === 12 && ranks[1] === 3) straight = 3; // A-2-3-4-5
  const counts = new Map();
  for (const r of ranks) counts.set(r, (counts.get(r) || 0) + 1);
  const groups = [...counts].sort((a, b) => b[1] - a[1] || b[0] - a[0]);
  const shape = groups.map((g) => g[1]).join("");
  const byGroup = groups.map((g) => g[0]);
  if (straight >= 0 && flush) return [8, straight];
  if (shape === "41") return [7, ...byGroup];
  if (shape === "32") return [6, ...byGroup];
  if (flush) return [5, ...ranks];
  if (straight >= 0) return [4, straight];
  if (shape === "311") return [3, ...byGroup];
  if (shape === "221") return [2, ...byGroup];
  if (shape === "2111") return [1, ...byGroup];
  return [0, ...ranks];
}

function compare(a, b) {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const d = (a[i] ?? -1) - (b[i] ?? -1);
    if (d) return d;
  }
  return 0;
}

/** The best five-card hand out of five to seven cards. */
export function bestHand(cards) {
  let best = null;
  const visit = (start, chosen) => {
    if (chosen.length === 5) {
      const r = rank5(chosen);
      if (!best || compare(r, best) > 0) best = r;
      return;
    }
    for (let i = start; i <= cards.length - (5 - chosen.length); i++) visit(i + 1, [...chosen, cards[i]]);
  };
  visit(0, []);
  return best;
}

/** Who wins between two hole-card pairs on a full board: 1 the first, -1 the second, 0 a split. */
export function showdown(first, second, board) {
  const b = splitCards(board);
  const r1 = bestHand([...splitCards(first), ...b]);
  const r2 = bestHand([...splitCards(second), ...b]);
  const d = compare(r1, r2);
  return { winner: Math.sign(d), names: [NAMES[r1[0]], NAMES[r2[0]]] };
}
