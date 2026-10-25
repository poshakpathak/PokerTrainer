// Card and hand-class helpers shared by the study, trainer and ranges views.

export const RANKS = "AKQJT98765432";
export const SUIT_SYMBOL = { s: "♠", h: "♥", d: "♦", c: "♣" };

const rankIndex = (r) => RANKS.indexOf(r);

/** "AsKh" -> "AKo", "KhKd" -> "KK", "Ts9s" -> "T9s" */
export function handClass(combo) {
  let [r1, s1, r2, s2] = combo;
  if (rankIndex(r1) > rankIndex(r2)) [r1, s1, r2, s2] = [r2, s2, r1, s1];
  if (r1 === r2) return r1 + r2;
  return r1 + r2 + (s1 === s2 ? "s" : "o");
}

/** The class shown in grid cell (row, col), with suited hands above the diagonal. */
export function gridClass(row, col) {
  const a = RANKS[row];
  const b = RANKS[col];
  if (row === col) return a + b;
  return row < col ? a + b + "s" : b + a + "o";
}

export function classCombos(cls) {
  return cls.length === 2 ? 6 : cls[2] === "s" ? 4 : 12;
}

/** "Ks7d2c" -> ["Ks", "7d", "2c"] */
export function splitCards(str) {
  const out = [];
  for (let i = 0; i + 1 < str.length; i += 2) out.push(str.slice(i, i + 2));
  return out;
}

export function cardHtml(card, size = "") {
  const [r, s] = card;
  return `<span class="card suit-${s} ${size}">${r === "T" ? "10" : r}<small>${SUIT_SYMBOL[s]}</small></span>`;
}

export function cardsHtml(str, size = "") {
  return splitCards(str).map((c) => cardHtml(c, size)).join("");
}

/** Every concrete combo of a class, e.g. "AKs" -> ["AsKs", "AhKh", ...]. */
export function expandClass(cls) {
  const suits = "shdc";
  const [a, b] = cls;
  const out = [];
  for (let i = 0; i < 4; i++) {
    for (let j = 0; j < 4; j++) {
      if (a === b && j <= i) continue;
      if (cls[2] === "s" && i !== j) continue;
      if (cls[2] === "o" && i === j) continue;
      out.push(a + suits[i] + b + suits[j]);
    }
  }
  return out;
}

/**
 * Parses a solver-style range string ("22+,A2s+,K9o-K7o,QJs:0.5") into
 * a Map of hand class -> weight (0..1).
 */
export function parseRange(str) {
  const weights = new Map();
  for (let part of str.split(",")) {
    part = part.trim();
    if (!part) continue;
    let w = 1;
    const colon = part.indexOf(":");
    if (colon >= 0) {
      w = parseFloat(part.slice(colon + 1));
      part = part.slice(0, colon);
    }
    for (const cls of expandRangePart(part)) weights.set(cls, w);
  }
  return weights;
}

function expandRangePart(part) {
  const pair = /^([AKQJT2-9])\1$/;
  const dash = part.split("-");
  if (dash.length === 2) {
    const [hi, lo] = dash;
    if (pair.test(hi)) {
      const out = [];
      for (let i = rankIndex(hi[0]); i <= rankIndex(lo[0]); i++) out.push(RANKS[i] + RANKS[i]);
      return out;
    }
    const out = [];
    for (let i = rankIndex(hi[1]); i <= rankIndex(lo[1]); i++) out.push(hi[0] + RANKS[i] + (hi[2] || ""));
    return out.flatMap(withSuitedness);
  }
  if (part.endsWith("+")) {
    const base = part.slice(0, -1);
    if (pair.test(base)) {
      const out = [];
      for (let i = rankIndex(base[0]); i >= 0; i--) out.push(RANKS[i] + RANKS[i]);
      return out;
    }
    const out = [];
    for (let i = rankIndex(base[1]); i > rankIndex(base[0]); i--) out.push(base[0] + RANKS[i] + (base[2] || ""));
    return out.flatMap(withSuitedness);
  }
  return withSuitedness(part);
}

// "AK" means both AKs and AKo.
function withSuitedness(cls) {
  if (cls.length === 2 && cls[0] !== cls[1]) return [cls + "s", cls + "o"];
  return [cls];
}
