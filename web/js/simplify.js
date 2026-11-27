// Simplifier: turns the solver's strategy at a decision into a few rules you can use at the
// table, and measures how much EV the simple version gives up.
//
// A simple strategy here is one bet size (or every size, or one action for the whole range)
// and one rule per hand type: top pair, flush draws, ace high and so on. A rule is an action,
// a mix rounded to quarters ("bet half the time"), or a split on something you can see at a
// glance ("bet with a backdoor flush draw, check without").
//
// EV given up is measured with the solver's EV for every action of every hand at the decision:
// what each hand gets with the simple rule, against what it gets with the solver's mix, while
// the other player keeps playing the solver's strategy. A player who adjusts to the simple
// strategy can win back more than that; in the Postflop solver you can lock the simple
// strategy and solve again to see how much.
//
// No DOM here: the Study, Postflop solver and Simplify tabs draw the results, and the tests in
// tests/ run it on the solved library.

import { splitCards } from "./cards.js";
import { actionLabel } from "./data.js";

/** Hand types, strongest first. A combo is in exactly one. */
export const CATEGORIES = [
  { id: "strong", name: "Two pair or better", short: "two pair or better",
    hint: "Sets, two pair, trips, straights, flushes and better, made with your hole cards." },
  { id: "overpair", name: "Overpair", short: "overpairs", hint: "A pocket pair above every board card." },
  { id: "tpgk", name: "Top pair, good kicker", short: "top pair with a good kicker",
    hint: "Top pair with one of the three best kickers that aren't on the board." },
  { id: "tpwk", name: "Top pair, weak kicker", short: "top pair with a weak kicker", hint: "Top pair with any other kicker." },
  { id: "second", name: "Second pair", short: "second pair",
    hint: "Pairs the second highest board card, or a pocket pair between the top two." },
  { id: "weak", name: "Weak pair", short: "weak pairs",
    hint: "Pairs a lower board card, or a pocket pair below the second highest card." },
  { id: "draw", name: "Strong draw", short: "flush draws and open-enders",
    hint: "A flush draw or an open-ended straight draw (8 or more outs), with second pair or less." },
  { id: "weakdraw", name: "Weak draw", short: "gutshots and backdoor flush draws",
    hint: "No pair: a gutshot, or on the flop two suited cards with one more of the suit on the board." },
  { id: "acehigh", name: "Ace high", short: "ace high", hint: "No pair and no draw, with an ace." },
  { id: "air", name: "Air", short: "air", hint: "No pair, no draw and no ace." },
];
export const CATEGORY = Object.fromEntries(CATEGORIES.map((c) => [c.id, c]));
const CAT_INDEX = new Map(CATEGORIES.map((c, i) => [c.id, i]));

/** The made-hand ladder, for phrases like "top pair or better". */
const LADDER = ["strong", "overpair", "tpgk", "tpwk", "second", "weak"];
const OR_BETTER = {
  strong: "two pair or better",
  overpair: "overpairs or better",
  tpgk: "top pair with a good kicker or better",
  tpwk: "top pair or better",
  second: "second pair or better",
  weak: "any pair or better",
};

/** Things you can see at a glance that a mixed hand type can be split on, best first. */
export const FEATURES = [
  { id: "bdfd", when: "with a backdoor flush draw", test: (f) => f.bdfd },
  { id: "gutshot", when: "with a gutshot", test: (f) => f.gutshot },
  { id: "kicker", when: "with a jack or better kicker", test: (f) => f.kicker >= 9, only: ["second", "weak"] },
  { id: "ace", when: "with an ace", test: (f) => f.ace },
  { id: "overcards", when: "with two overcards", test: (f) => f.overcards },
  { id: "pocket", when: "with a pocket pair", test: (f) => f.pocket },
  { id: "flushdraw", when: "with a flush draw", test: (f) => f.fd, only: ["draw"] },
  { id: "suited", when: "when suited", test: (f) => f.suited },
];

// Ranks as numbers, 2 = 0 ... A = 12.
const LOW = "23456789TJQKA";
const rankOf = (card) => LOW.indexOf(card[0]);

function hasRun(set, hi) {
  for (let r = hi; r > hi - 5; r--) if (!set.has(r < 0 ? 12 : r)) return false;
  return true;
}

/** Highs of the five-card straights in a set of ranks (A plays low too). */
function straights(set) {
  const out = [];
  for (let hi = 12; hi >= 3; hi--) if (hasRun(set, hi)) out.push(hi);
  return out;
}

/** A straight in `all` that isn't already all on `board`, i.e. it uses a hole card. */
function madeStraight(all, board) {
  return straights(all).some((hi) => !hasRun(board, hi));
}

const featureCache = new Map();

/**
 * What a combo has on a board: its hand type (`cat`) and the features rules can split on.
 * `combo` is like "AsKd" and `board` like "Ks7d2c" (3 to 5 cards).
 */
export function handFeatures(combo, board) {
  const key = combo + board;
  let f = featureCache.get(key);
  if (f) return f;
  if (featureCache.size > 50000) featureCache.clear();
  const hole = [combo.slice(0, 2), combo.slice(2, 4)];
  const cards = splitCards(board);
  const onFlop = cards.length === 3;
  const draws = cards.length < 5;
  const [hi, lo] = hole.map(rankOf).sort((a, b) => b - a);
  const count = new Map();
  for (const c of cards) count.set(rankOf(c), (count.get(rankOf(c)) || 0) + 1);
  const distinct = [...count.keys()].sort((a, b) => b - a);
  const pocket = hi === lo;

  // Flushes and flush draws, using at least one hole card.
  let flush = false;
  let fd = false;
  let bdfd = false;
  let suitedBackdoor = false;
  for (const s of "shdc") {
    const h = hole.filter((c) => c[1] === s).length;
    if (!h) continue;
    const n = h + cards.filter((c) => c[1] === s).length;
    if (n >= 5) flush = true;
    else if (n === 4 && draws) fd = true;
    else if (n === 3 && onFlop) {
      bdfd = true;
      if (h === 2) suitedBackdoor = true;
    }
  }

  // Straights and straight draws, using at least one hole card.
  const boardSet = new Set(cards.map(rankOf));
  const all = new Set([...boardSet, hi, lo]);
  const straight = madeStraight(all, boardSet);
  let outs = 0;
  if (!straight && draws) {
    for (let r = 0; r < 13; r++) {
      if (all.has(r)) continue;
      const withR = new Set([...all, r]);
      const boardR = new Set([...boardSet, r]);
      if (madeStraight(withR, boardR)) outs++;
    }
  }
  const oesd = outs >= 2;
  const gutshot = outs === 1;

  // Pairs, counting only what the hole cards add to the board.
  let cat;
  let kicker = -1;
  const pairedRank = [hi, lo].find((r) => count.has(r));
  if (flush || straight) cat = "strong";
  else if (pocket && count.has(hi)) cat = "strong"; // set or better
  else if (!pocket && (count.get(hi) >= 2 || count.get(lo) >= 2)) cat = "strong"; // trips or better
  else if (!pocket && count.has(hi) && count.has(lo)) cat = "strong"; // two pair
  else if (pocket) {
    if (hi > distinct[0]) cat = "overpair";
    else if (distinct.length > 1 && hi > distinct[1]) cat = "second";
    else cat = "weak";
  } else if (pairedRank !== undefined) {
    kicker = pairedRank === hi ? lo : hi;
    if (pairedRank === distinct[0]) {
      // Good kicker: one of the three best ranks that aren't on the board.
      const free = [];
      for (let r = 12; r >= 0 && free.length < 3; r--) if (!count.has(r)) free.push(r);
      cat = free.includes(kicker) ? "tpgk" : "tpwk";
    } else if (pairedRank === distinct[1]) cat = "second";
    else cat = "weak";
  } else cat = "none";

  if ((fd || oesd) && (cat === "second" || cat === "weak" || cat === "none")) cat = "draw";
  else if (cat === "none") {
    if (gutshot || suitedBackdoor) cat = "weakdraw";
    else if (hi === 12) cat = "acehigh";
    else cat = "air";
  }

  f = {
    cat,
    fd,
    oesd,
    gutshot,
    bdfd,
    pocket,
    kicker,
    ace: hi === 12,
    suited: combo[1] === combo[3],
    overcards: lo > distinct[0],
  };
  featureCache.set(key, f);
  return f;
}

// ---- One decision ------------------------------------------------------------------------

const AGGRESSIVE = new Set(["bet", "raise", "allin"]);

/** Sum of `w * x` over rows into `acc`. */
function addScaled(acc, x, w) {
  for (let a = 0; a < acc.length; a++) acc[a] += x[a] * w;
}

/**
 * The hands at a decision with their hand types and solver EVs, ready to simplify.
 * `node` is a decision from the library or pt-server: {actions, hands, combos, pot_bb, player}.
 */
export function prepare(node, board) {
  const k = node.actions.length;
  const hands = [];
  let total = 0;
  node.hands.forEach((row, i) => {
    if (!row || !(row[0] > 0)) return;
    const strat = row.slice(1, 1 + k);
    const ev = row.slice(1 + k, 1 + 2 * k);
    const sum = strat.reduce((a, b) => a + b, 0) || 1;
    const sigma = strat.map((s) => s / sum);
    const combo = node.combos[i];
    const f = handFeatures(combo, board);
    hands.push({ i, combo, r: row[0], sigma, ev, v: sigma.reduce((s, x, a) => s + x * ev[a], 0), f, cat: f.cat });
    total += row[0];
  });
  const freq = new Array(k).fill(0);
  // The solver's own mix per hand type: {cat: {w, f}} with f summed over the combos.
  const types = {};
  // How much better each hand could do by playing its best action instead of the solver's mix:
  // zero for a perfect solve. It's how precise the EV numbers here are.
  let gap = 0;
  for (const h of hands) {
    addScaled(freq, h.sigma, h.r / total);
    gap += h.r * (Math.max(...h.ev) - h.v);
    const t = (types[h.cat] ??= { w: 0, f: new Array(k).fill(0) });
    t.w += h.r;
    addScaled(t.f, h.sigma, h.r);
  }
  const labels = node.actions.map(actionLabel);
  return {
    node, board, k, hands, total, freq, labels, types,
    pot: node.pot_bb,
    gap: total ? gap / total : 0,
    aggressive: node.actions.map((a, i) => (AGGRESSIVE.has(a.type) ? i : -1)).filter((i) => i >= 0),
  };
}

/** The candidate simple strategies at a decision: by hand type with some sizes, or one play for all. */
export function levelSpecs(prep) {
  const { node, aggressive, labels, k } = prep;
  const specs = [];
  const all = [...Array(k).keys()];
  if (aggressive.length > 1) {
    for (const a of aggressive) {
      specs.push({ id: `size:${labels[a]}`, kind: "types", keep: all.filter((x) => !aggressive.includes(x) || x === a), size: a });
    }
  }
  specs.push({ id: "types", kind: "types", keep: all, size: aggressive.length === 1 ? aggressive[0] : null });
  for (let a = 0; a < k; a++) specs.push({ id: `one:${labels[a]}`, kind: "one", action: a });
  return specs;
}

/**
 * Each hand's solver strategy squeezed onto the kept actions: what it put on a dropped size goes
 * to the kept action it likes best (by the solver's EV), e.g. 75% bets become 33% bets, or checks
 * if betting small is worse than checking for that hand.
 */
export function squeeze(prep, keep) {
  const keepSet = new Set(keep);
  return prep.hands.map((h) => {
    const out = new Array(prep.k).fill(0);
    let dropped = 0;
    for (let a = 0; a < prep.k; a++) {
      if (keepSet.has(a)) out[a] = h.sigma[a];
      else dropped += h.sigma[a];
    }
    if (dropped > 0) {
      let best = keep[0];
      for (const a of keep) if (h.ev[a] > h.ev[best]) best = a;
      out[best] += dropped;
    }
    return out;
  });
}

/**
 * Per hand type, the combos and the mix after squeezing, plus the same for each side of every
 * feature split: {cat: {w, f[], splits: {feature: [{w, f}, {w, f}]}}}.
 */
export function typeStats(prep, squeezed) {
  const stats = {};
  prep.hands.forEach((h, j) => {
    const s = (stats[h.cat] ??= { w: 0, f: new Array(prep.k).fill(0), splits: {} });
    s.w += h.r;
    addScaled(s.f, squeezed[j], h.r);
    for (const feat of FEATURES) {
      if (feat.only && !feat.only.includes(h.cat)) continue;
      const sides = (s.splits[feat.id] ??= [{ w: 0, f: new Array(prep.k).fill(0) }, { w: 0, f: new Array(prep.k).fill(0) }]);
      const side = sides[feat.test(h.f) ? 0 : 1];
      side.w += h.r;
      addScaled(side.f, squeezed[j], h.r);
    }
  });
  return stats;
}

/** Adds one decision's type stats into another (for rules shared by several flops). */
export function mergeStats(into, stats) {
  for (const [cat, s] of Object.entries(stats)) {
    const t = (into[cat] ??= { w: 0, f: new Array(s.f.length).fill(0), splits: {} });
    t.w += s.w;
    addScaled(t.f, s.f, 1);
    for (const [id, sides] of Object.entries(s.splits)) {
      const ts = (t.splits[id] ??= sides.map((x) => ({ w: 0, f: new Array(x.f.length).fill(0) })));
      sides.forEach((x, i) => {
        ts[i].w += x.w;
        addScaled(ts[i].f, x.f, 1);
      });
    }
  }
  return into;
}

/** A mix rounded to quarters, with the largest remainders rounded up so it adds up to 1. */
export function quarters(f) {
  const total = f.reduce((a, b) => a + b, 0) || 1;
  const q = f.map((x) => (4 * x) / total);
  const out = q.map(Math.floor);
  let left = 4 - out.reduce((a, b) => a + b, 0);
  const order = q.map((x, i) => [x - Math.floor(x), x, i]).sort((a, b) => b[0] - a[0] || b[1] - a[1]);
  for (const [, , i] of order) {
    if (left <= 0) break;
    out[i]++;
    left--;
  }
  return out.map((x) => x / 4);
}

const argmax = (f) => f.reduce((best, x, i) => (x > f[best] ? i : best), 0);
const oneHot = (k, a) => Array.from({ length: k }, (_, i) => (i === a ? 1 : 0));

/** The rule for one hand type with one action: the one the solver takes most with it. */
export function pureRule(stat, k) {
  const mix = stat.f.map((x) => x / (stat.w || 1));
  const top = argmax(mix);
  return { kind: "pure", action: top, mix: oneHot(k, top), solver: mix };
}

/**
 * A closer rule for a hand type the solver plays more than one way: a split on a feature if both
 * sides are then clear (each takes one action at least `purity` of the time), else the mix
 * rounded to quarters. Null if neither says more than the pure rule.
 */
export function refinedRule(stat, k, { purity = 0.7, splits = true } = {}) {
  const mix = stat.f.map((x) => x / (stat.w || 1));
  if (splits) {
    let best = null;
    for (const feat of FEATURES) {
      const sides = stat.splits[feat.id];
      if (!sides || sides.some((s) => s.w < 0.15 * stat.w)) continue;
      const m = sides.map((s) => s.f.map((x) => x / s.w));
      const acts = m.map(argmax);
      if (acts[0] === acts[1] || m.some((x, i) => x[acts[i]] < purity)) continue;
      const fit = (sides[0].w * m[0][acts[0]] + sides[1].w * m[1][acts[1]]) / stat.w;
      if (!best || fit > best.fit + 1e-9) best = { feat, acts, fit, w: sides.map((s) => s.w) };
    }
    if (best) {
      return {
        kind: "split", feature: best.feat.id, yes: best.acts[0], no: best.acts[1], weights: best.w,
        mix: best.acts.map((a) => oneHot(k, a)), solver: mix,
      };
    }
  }
  const q = quarters(mix);
  if (q.filter((x) => x > 0).length < 2) return null;
  return { kind: "mix", mix: q, solver: mix };
}

/** What a rule does with a hand type's combos, summed over them (not normalized). */
function ruleTotals(rule, stat) {
  if (rule.kind === "split") {
    return rule.mix[0].map((x, a) => x * rule.weights[0] + rule.mix[1][a] * rule.weights[1]);
  }
  return rule.mix.map((x) => x * stat.w);
}

/**
 * Rules for every hand type, at a level of detail:
 *   "simple"    one action per hand type
 *   "balanced"  one action per hand type, then splits or mixes for the hand types that bring the
 *               range's frequencies closest to the solver's, until they're within `target`
 *               (5 points by default) or nothing helps more
 *   "close"     a split or mix for every hand type the solver plays less than 3 times in 4 one way
 */
export function rulesFor(stats, k, opts = {}) {
  const { detail = "balanced", target = 0.05 } = opts;
  const cats = Object.keys(stats).filter((c) => stats[c].w > 0);
  const rules = Object.fromEntries(cats.map((c) => [c, pureRule(stats[c], k)]));
  if (detail === "simple") return rules;
  const refined = Object.fromEntries(cats.map((c) => [c, refinedRule(stats[c], k, opts)]));
  if (detail === "close") {
    for (const c of cats) if (refined[c] && Math.max(...rules[c].solver) < 0.75) rules[c] = refined[c];
    return rules;
  }
  const total = cats.reduce((s, c) => s + stats[c].w, 0);
  const goal = new Array(k).fill(0);
  const cur = new Array(k).fill(0);
  for (const c of cats) {
    addScaled(goal, stats[c].f, 1);
    addScaled(cur, ruleTotals(rules[c], stats[c]), 1);
  }
  const drift = (x) => x.reduce((s, v, a) => s + Math.abs(v - goal[a]), 0) / 2 / (total || 1);
  const left = new Set(cats.filter((c) => refined[c]));
  while (left.size && drift(cur) > target) {
    let best = null;
    for (const c of left) {
      const next = cur.map((v, a) => v - ruleTotals(rules[c], stats[c])[a] + ruleTotals(refined[c], stats[c])[a]);
      const d = drift(next);
      if (!best || d < best.d - 1e-12) best = { c, d, next };
    }
    if (!best || best.d >= drift(cur) - 1e-9) break;
    rules[best.c] = refined[best.c];
    cur.splice(0, k, ...best.next);
    left.delete(best.c);
  }
  return rules;
}

/** The simple strategy a hand gets from its type's rule. */
export function ruleStrategy(rule, f) {
  if (rule.kind !== "split") return rule.mix;
  const feat = FEATURES.find((x) => x.id === rule.feature);
  return rule.mix[feat.test(f) ? 0 : 1];
}

/**
 * Plays a simple strategy at a decision: `strategyOf(hand)` gives each hand's mix. Returns the
 * EV it gives up (bb per combo that gets here, and as a share of the pot), per hand type too,
 * the range-wide frequencies and every combo's simple strategy (aligned with node.hands).
 */
export function evaluate(prep, strategyOf) {
  const { k, total } = prep;
  const freq = new Array(k).fill(0);
  const byCat = {};
  let lost = 0;
  let moved = 0;
  const strategies = new Array(prep.node.hands.length).fill(null);
  for (const h of prep.hands) {
    const s = strategyOf(h);
    strategies[h.i] = s;
    const v = s.reduce((sum, x, a) => sum + x * h.ev[a], 0);
    const loss = h.v - v;
    lost += h.r * loss;
    let diff = 0;
    for (let a = 0; a < k; a++) diff += Math.abs(s[a] - h.sigma[a]);
    moved += (h.r * diff) / 2;
    addScaled(freq, s, h.r / total);
    const c = (byCat[h.cat] ??= { w: 0, lost: 0 });
    c.w += h.r;
    c.lost += h.r * loss;
  }
  // A simple strategy can come out ahead of the solver's own mix by up to the solve's error, so
  // a negative total counts as nothing lost.
  const raw = total ? lost / total : 0;
  const loss = Math.max(0, raw);
  for (const c of Object.values(byCat)) c.loss = c.w ? Math.max(0, c.lost / c.w) : 0;
  const drift = freq.reduce((s, x, a) => s + Math.abs(x - prep.freq[a]), 0) / 2;
  return { loss, raw, lossPct: prep.pot > 0 ? loss / prep.pot : 0, freq, drift, byCat, moved: total ? moved / total : 0, strategies };
}

/** Range frequencies as [fold, check or call, bet or raise of any size]. */
function coarse(freq, actions) {
  const out = [0, 0, 0];
  actions.forEach((a, i) => (out[a.type === "fold" ? 0 : AGGRESSIVE.has(a.type) ? 2 : 1] += freq[i]));
  return out;
}

const COARSE_NAMES = (actions) => {
  const facing = actions.some((a) => a.type === "call");
  return ["Folds", facing ? "Calls" : "Checks", facing ? "Raises" : "Bets"];
};

/**
 * Warnings for a simple strategy whose frequencies stray far from the solver's. Its EV estimate
 * holds the other player's strategy fixed, which is less safe the further the range moves: an
 * opponent who notices can take back more.
 */
export function cautions(solverFreq, freq, actions) {
  const out = [];
  actions.forEach((act, a) => {
    if (AGGRESSIVE.has(act.type) && solverFreq[a] < 0.05 && freq[a] >= 0.1) {
      out.push(`The solver hardly uses ${phraseAction(act).toLowerCase()} here (${pct(solverFreq[a])} of the range), so its EVs for it assume a much stronger range than the ${pct(freq[a])} this puts there.`);
    }
  });
  const before = coarse(solverFreq, actions);
  const after = coarse(freq, actions);
  const names = COARSE_NAMES(actions);
  for (let i = 0; i < 3; i++) {
    if (Math.abs(after[i] - before[i]) >= 0.2) {
      out.push(`${names[i]} go from ${pct(before[i])} to ${pct(after[i])} of the range. An opponent who notices can take back more than this estimate.`);
    }
  }
  return out;
}

const pct = (x) => `${Math.round(x * 100)}%`;

/** "Bet 33%", "Raise to 5.4bb", "Check": an action as a rule's verb. */
export function phraseAction(a) {
  switch (a.type) {
    case "fold": return "Fold";
    case "check": return "Check";
    case "call": return "Call";
    case "bet": return `Bet ${a.pct}%`;
    case "raise": return `Raise to ${a.to_bb}bb`;
    case "allin": return "All-in";
    default: return "?";
  }
}

const SHARE = { 0.25: "a quarter of the time", 0.5: "half the time", 0.75: "3 times in 4" };

/** A mix in quarters as text: "bet 33% 3 times in 4, otherwise check". */
export function phraseMix(mix, actions) {
  const parts = mix
    .map((x, a) => [x, a])
    .filter(([x]) => x > 0)
    .sort((p, q) => q[0] - p[0] || q[1] - p[1]);
  const verb = (a) => phraseAction(actions[a]).toLowerCase();
  if (parts.length === 2) return `${verb(parts[0][1])} ${SHARE[parts[0][0]] || pct(parts[0][0])}, otherwise ${verb(parts[1][1])}`;
  return parts.map(([x, a]) => `${verb(a)} ${SHARE[x] || pct(x)}`).join(", ");
}

/**
 * Words for hand types that depend on the board, to use instead of their usual ones: a weak draw
 * on a monotone flop or on the turn can only be a gutshot.
 */
export function typeWords(hands) {
  let gutshots = false;
  let backdoors = false;
  for (const h of hands) {
    if (h.cat !== "weakdraw") continue;
    if (h.f.gutshot) gutshots = true;
    else backdoors = true;
  }
  if (gutshots && !backdoors) return { weakdraw: "gutshots" };
  if (backdoors && !gutshots) return { weakdraw: "backdoor flush draws" };
  return {};
}

/** "top pair or better, flush draws and open-enders": hand types as a phrase. */
export function phraseTypes(cats, present, words = {}) {
  const ids = new Set(cats);
  const out = [];
  // The made-hand ladder from the top, skipping types nobody holds here.
  const ladder = LADDER.filter((c) => present.has(c));
  let run = 0;
  while (run < ladder.length && ids.has(ladder[run])) run++;
  if (run >= 2 || (run === 1 && ladder[0] === "strong")) {
    out.push(OR_BETTER[ladder[run - 1]]);
    for (let i = 0; i < run; i++) ids.delete(ladder[i]);
  }
  if (ids.has("tpgk") && ids.has("tpwk")) {
    out.push("top pair");
    ids.delete("tpgk");
    ids.delete("tpwk");
  }
  for (const c of CATEGORIES) if (ids.has(c.id)) out.push(words[c.id] || c.short);
  return out.join(", ");
}

/**
 * The rules as lines to read at the table: one per action for the hand types that always take it
 * (strongest action first), then one per split and one per mix, hand types with the same mix
 * sharing a line. Each line is {kind, cats, head, text}, plus `action` for the one-action lines.
 */
export function ruleLines(rules, actions, present, words = {}) {
  const pure = new Map();
  const mixes = new Map();
  const other = [];
  for (const c of CATEGORIES) {
    const rule = rules[c.id];
    if (!rule || !present.has(c.id)) continue;
    if (rule.kind === "pure") {
      if (!pure.has(rule.action)) pure.set(rule.action, []);
      pure.get(rule.action).push(c.id);
    } else if (rule.kind === "mix") {
      const key = rule.mix.join();
      if (!mixes.has(key)) {
        mixes.set(key, { rule, cats: [] });
        other.push(mixes.get(key));
      }
      mixes.get(key).cats.push(c.id);
    } else other.push({ rule, cats: [c.id] });
  }
  const lines = [];
  const order = [...pure.keys()].sort((a, b) => b - a);
  for (const a of order) {
    lines.push({ kind: "pure", action: a, cats: pure.get(a), head: phraseAction(actions[a]), text: phraseTypes(pure.get(a), present, words) });
  }
  const verb = (a) => phraseAction(actions[a]).toLowerCase();
  for (const { rule, cats } of other) {
    const head = cats.length === 1 ? CATEGORY[cats[0]].name : capitalize(phraseTypes(cats, present, words));
    if (rule.kind === "split") {
      const feat = FEATURES.find((x) => x.id === rule.feature);
      lines.push({ kind: "split", cats, head, text: `${verb(rule.yes)} ${feat.when}, otherwise ${verb(rule.no)}` });
    } else {
      lines.push({ kind: "mix", cats, head, text: phraseMix(rule.mix, actions) });
    }
  }
  return lines;
}

const capitalize = (s) => s && s[0].toUpperCase() + s.slice(1);

/** How hard a simple strategy is to remember: one line per action or mixed/split hand type. */
const lineCount = (lines) => lines.length;

/**
 * Every simple strategy at a decision, with the EV each gives up, and the one to recommend: the
 * simplest that gives up at most `tolerance` of the pot (default 1%).
 *
 * opts: {tolerance, detail, target, purity, splits}
 */
export function simplify(node, board, opts = {}) {
  const tolerance = opts.tolerance ?? 0.01;
  const prep = prepare(node, board);
  const present = new Set(prep.hands.map((h) => h.cat));
  const words = typeWords(prep.hands);
  const levels = [];
  for (const spec of levelSpecs(prep)) {
    let level;
    if (spec.kind === "one") {
      const s = oneHot(prep.k, spec.action);
      const res = evaluate(prep, () => s);
      level = { ...spec, ...res, rules: null, lines: oneLine(spec.action, node.actions, present) };
    } else {
      const squeezed = squeeze(prep, spec.keep);
      const stats = typeStats(prep, squeezed);
      const rules = rulesFor(stats, prep.k, opts);
      const res = evaluate(prep, (h) => ruleStrategy(rules[h.cat], h.f));
      level = { ...spec, ...res, rules, stats, lines: ruleLines(rules, node.actions, present, words) };
    }
    level.cautions = cautions(prep.freq, level.freq, node.actions);
    level.name = levelName(level, node.actions);
    levels.push(level);
  }
  const shown = bestOne(levels);
  return {
    prep, present, tolerance,
    actions: node.actions,
    freq: prep.freq,
    types: Object.fromEntries(Object.entries(prep.types).map(([cat, t]) => [cat, { w: t.w / prep.total, f: t.f.map((x) => x / prep.total) }])),
    gapPct: prep.pot > 0 ? prep.gap / prep.pot : 0,
    levels: shown,
    recommended: recommend(shown, tolerance),
  };
}

function levelName(level, actions) {
  if (level.kind === "one") return `${phraseAction(actions[level.action])} with every hand`;
  if (level.id === "types") return level.size === null && actions.some((a) => AGGRESSIVE.has(a.type)) ? "Every size, by hand type" : "By hand type";
  return `Only ${phraseAction(actions[level.size]).replace(/^(Bet|Raise to|All-in)/, (m) => m.toLowerCase())}, by hand type`;
}

/**
 * The simplest strategy within the tolerance, preferring ones whose frequencies stay close to
 * the solver's; with none within it, the one that gives up the least.
 */
export function recommend(levels, tolerance) {
  const rank = (l) => (l.kind === "one" ? 0 : l.id === "types" && levels.some((x) => x.id.startsWith("size:")) ? 2 : 1);
  const ok = levels.filter((l) => l.lossPct <= tolerance);
  // Within a kind, each extra line of rules is worth 0.05% of the pot.
  const cost = (l) => l.lossPct + 0.0005 * lineCount(l.lines);
  const pickFrom = (list) => list.slice().sort((a, b) => rank(a) - rank(b) || cost(a) - cost(b))[0];
  return pickFrom(ok.filter((l) => !l.cautions.length)) || pickFrom(ok) || levels.slice().sort((a, b) => a.loss - b.loss)[0];
}

// ---- Several flops: rules by board texture -----------------------------------------------

/** Board texture groups for flop rules, checked in order; every flop is in exactly one. */
export const TEXTURES = [
  { id: "monotone", name: "Monotone", hint: "All three cards of one suit." },
  { id: "paired", name: "Paired", hint: "Two or three cards of the same rank." },
  { id: "ace", name: "Ace high", hint: "An ace, not paired or monotone." },
  { id: "broadway", name: "Two or more broadways", hint: "King, queen or jack high with another card ten or higher." },
  { id: "connected", name: "Connected", hint: "No ace and at most one broadway, with all three cards inside five ranks (T96, 875, J98)." },
  { id: "high", name: "King to jack high, dry", hint: "King, queen or jack high, the other two cards nine or lower and not connected." },
  { id: "low", name: "Ten high or lower, dry", hint: "Ten high or lower and not connected." },
];
export const TEXTURE = Object.fromEntries(TEXTURES.map((t) => [t.id, t]));

/** The texture group of a flop like "Ks7d2c". */
export function textureOf(flop) {
  const cards = splitCards(flop).slice(0, 3);
  const ranks = cards.map(rankOf).sort((a, b) => b - a);
  if (new Set(cards.map((c) => c[1])).size === 1) return "monotone";
  if (new Set(ranks).size < 3) return "paired";
  if (ranks[0] === 12) return "ace";
  if (ranks[0] >= 9 && ranks[1] >= 8) return "broadway";
  if (connected(ranks)) return "connected";
  return ranks[0] >= 9 ? "high" : "low";
}

/** Whether three distinct ranks fit in five consecutive ranks (with the ace low too). */
function connected(ranks) {
  const span = (rs) => Math.max(...rs) - Math.min(...rs);
  return span(ranks) <= 4 || (ranks.includes(12) && span(ranks.map((r) => (r === 12 ? -1 : r))) <= 4);
}

/**
 * Rules shared by several flops of one decision (e.g. every ace-high flop): the hand types' mixes
 * are pooled across the flops, one set of rules is made from them, and each flop is scored with
 * those rules. `items` is [{flop, node}]; flops whose decision has other actions than the first
 * one's are left out.
 *
 * Returns the same levels as simplify(), each with `flops` (the loss on every flop) and the
 * average and worst loss.
 */
export function simplifyGroup(items, opts = {}) {
  const tolerance = opts.tolerance ?? 0.01;
  if (!items.length) return null;
  const labels = items[0].node.actions.map(actionLabel);
  const preps = items
    .filter(({ node }) => node.actions.length === labels.length && node.actions.every((a, i) => actionLabel(a) === labels[i]))
    .map(({ flop, node }) => ({ flop, prep: prepare(node, flop) }))
    .filter(({ prep }) => prep.hands.length);
  if (!preps.length) return null;
  const base = preps[0].prep;
  const actions = base.node.actions;
  const k = base.k;
  const present = new Set(preps.flatMap((p) => p.prep.hands.map((h) => h.cat)));
  const words = typeWords(preps.flatMap((p) => p.prep.hands));
  // The solver's frequencies over the group, each flop counting the same, and its mix per hand type.
  const freq = new Array(k).fill(0);
  const types = {};
  for (const { prep } of preps) {
    addScaled(freq, prep.freq, 1 / preps.length);
    for (const [cat, t] of Object.entries(prep.types)) {
      const into = (types[cat] ??= { w: 0, f: new Array(k).fill(0) });
      into.w += t.w / prep.total / preps.length;
      addScaled(into.f, t.f, 1 / prep.total / preps.length);
    }
  }
  const gapPct = preps.reduce((sum, { prep }) => sum + prep.gap / prep.pot, 0) / preps.length;

  const levels = [];
  for (const spec of levelSpecs(base)) {
    let rules = null;
    if (spec.kind === "types") {
      const pooled = {};
      for (const { prep } of preps) mergeStats(pooled, typeStats(prep, squeeze(prep, spec.keep)));
      rules = rulesFor(pooled, k, opts);
    }
    const one = spec.kind === "one" ? oneHot(k, spec.action) : null;
    const byCat = {};
    const flops = preps.map(({ flop, prep }) => {
      const res = evaluate(prep, (h) => one || ruleStrategy(rules[h.cat], h.f));
      for (const [cat, c] of Object.entries(res.byCat)) {
        const into = (byCat[cat] ??= { w: 0, lost: 0 });
        into.w += c.w / prep.total;
        into.lost += c.lost / prep.total;
      }
      return { flop, loss: res.loss, lossPct: res.lossPct, freq: res.freq, solverFreq: prep.freq, moved: res.moved };
    });
    for (const c of Object.values(byCat)) c.loss = c.w ? Math.max(0, c.lost / c.w) : 0;
    const avg = (key) => flops.reduce((sum, x) => sum + x[key], 0) / flops.length;
    const simpleFreq = new Array(k).fill(0);
    for (const x of flops) addScaled(simpleFreq, x.freq, 1 / flops.length);
    const level = {
      ...spec, rules, flops, byCat, freq: simpleFreq,
      loss: avg("loss"), lossPct: avg("lossPct"), moved: avg("moved"),
      drift: simpleFreq.reduce((sum, x, a) => sum + Math.abs(x - freq[a]), 0) / 2,
      worst: flops.reduce((w, x) => (x.lossPct > w.lossPct ? x : w), flops[0]),
      lines: rules ? ruleLines(rules, actions, present, words) : oneLine(spec.action, actions, present),
    };
    level.cautions = cautions(freq, simpleFreq, actions);
    level.name = levelName(level, actions);
    levels.push(level);
  }
  const shown = bestOne(levels);
  return {
    actions, labels, freq, types, present, gapPct, tolerance,
    flops: preps.map((p) => p.flop),
    levels: shown,
    recommended: recommend(shown, tolerance),
  };
}

/** The rule line of a one-action strategy. */
function oneLine(action, actions, present) {
  return [{ kind: "pure", action, cats: [...present], head: phraseAction(actions[action]), text: "every hand" }];
}

/** All the levels but the one-action ones, and of those only the best: the others aren't worth showing. */
function bestOne(levels) {
  const ones = levels.filter((l) => l.kind === "one").sort((a, b) => a.loss - b.loss);
  return levels.filter((l) => l.kind !== "one" || l === ones[0]);
}
