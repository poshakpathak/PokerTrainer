// Tests for the simplifier (web/js/simplify.js): hand types, rules and their wording, the EV
// given up, and rules by board texture on the solved library in web/data.
//
// Run from the repo root with: node --test tests/

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const WEB = new URL("../web/", import.meta.url);

// The app loads the library with fetch("data/..."); read it from disk instead.
globalThis.fetch = async (url) => {
  try {
    const text = await readFile(new URL(url, WEB), "utf8");
    return { ok: true, status: 200, json: async () => JSON.parse(text) };
  } catch {
    return { ok: false, status: 404, json: async () => ({}) };
  }
};

const S = await import(new URL("js/simplify.js", WEB));
const D = await import(new URL("js/data.js", WEB));

const cat = (combo, board) => S.handFeatures(combo, board).cat;
const close = (a, b, eps = 1e-9) => Math.abs(a - b) <= eps;
const sum = (xs) => xs.reduce((a, b) => a + b, 0);

/** Every solved flop of a spot, with the decision at `path` on each. */
async function decisions(spotId, path) {
  const index = await D.loadIndex();
  const spot = index.spots.find((s) => s.id === spotId);
  const out = [];
  for (const { flop } of spot.flops) {
    const sol = await D.loadSolution(spotId, flop);
    const node = sol.byPath.get(path.join(" / "));
    if (node) out.push({ flop, node });
  }
  return out;
}

// ---- Hand types ----------------------------------------------------------------------------

test("made hands on a dry king-high flop", () => {
  const b = "Ks7d2c";
  assert.equal(cat("7h7c", b), "strong"); // set
  assert.equal(cat("Kh7h", b), "strong"); // two pair
  assert.equal(cat("AsAd", b), "overpair");
  assert.equal(cat("AhKd", b), "tpgk"); // ace is the best kicker not on the board
  assert.equal(cat("KhQd", b), "tpgk");
  assert.equal(cat("KhJd", b), "tpgk"); // A, Q, J are the three best free kickers
  assert.equal(cat("KhTd", b), "tpwk");
  assert.equal(cat("8h8c", b), "second"); // pocket pair between the top two cards
  assert.equal(cat("7h6h", b), "second");
  assert.equal(cat("5h5c", b), "weak");
  assert.equal(cat("Ah2d", b), "weak");
});

test("draws and unpaired hands", () => {
  const b = "Ks7d2c";
  assert.equal(cat("QdJd", b), "weakdraw"); // two diamonds and the 7d: a backdoor flush draw
  assert.equal(cat("QhJh", b), "air"); // no heart on the board
  assert.equal(cat("AhQh", b), "acehigh");
  assert.equal(cat("AdQd", b), "weakdraw"); // the draw counts before the ace

  const t = "Ts9d4c";
  assert.equal(cat("QhJd", t), "draw"); // open-ender
  assert.equal(cat("8h7d", t), "draw");
  assert.equal(cat("Qh8d", t), "weakdraw"); // gutshot to the jack
  assert.equal(cat("KhQd", t), "weakdraw");
  assert.equal(cat("JhJd", t), "overpair");
  assert.equal(cat("AhTd", t), "tpgk");
  assert.equal(cat("Th8d", t), "tpwk"); // a pair beats the straight draw

  const f = S.handFeatures("QhJd", t);
  assert.ok(f.oesd && !f.gutshot && !f.fd);
  assert.ok(S.handFeatures("Qh8d", t).gutshot);
  assert.ok(S.handFeatures("KhQd", t).overcards);
});

test("monotone, paired and five-card boards", () => {
  const m = "Ah8h3h";
  assert.equal(cat("KhQd", m), "draw"); // one heart: a flush draw
  assert.equal(cat("KhQh", m), "strong"); // flush
  assert.equal(cat("KdQd", m), "air"); // no backdoor flush draws on a monotone flop
  assert.equal(S.handFeatures("KdQd", m).bdfd, false);

  const p = "7h7c2d";
  assert.equal(cat("Ah7d", p), "strong"); // trips
  assert.equal(cat("2s2h", p), "strong"); // full house
  assert.equal(cat("8h8d", p), "overpair");
  assert.equal(cat("5h5d", p), "second");
  assert.equal(cat("As2s", p), "second"); // pairs the board's second rank

  // On the river there are no draws left.
  const r = "Ts9d4c2h3s";
  assert.equal(cat("QhJd", r), "air");
  assert.equal(cat("Ah5d", r), "strong"); // wheel
});

// ---- Rules and their wording ---------------------------------------------------------------

test("quarters rounds a mix to quarters that add up to 1", () => {
  assert.deepEqual(S.quarters([0.6, 0.4]), [0.5, 0.5]);
  assert.deepEqual(S.quarters([0.7, 0.3]), [0.75, 0.25]);
  assert.deepEqual(S.quarters([0.1, 0.2, 0.7]), [0, 0.25, 0.75]);
  assert.deepEqual(S.quarters([1, 0, 0]), [1, 0, 0]);
  for (let i = 0; i < 200; i++) {
    const f = [Math.random(), Math.random(), Math.random()];
    const q = S.quarters(f);
    assert.ok(close(sum(q), 1));
    q.forEach((x) => assert.ok(close((x * 4) % 1, 0)));
  }
});

test("hand types read as a phrase", () => {
  const all = new Set(S.CATEGORIES.map((c) => c.id));
  assert.equal(S.phraseTypes(["strong", "overpair", "tpgk"], all), "top pair with a good kicker or better");
  assert.equal(S.phraseTypes(["strong", "overpair", "tpgk", "tpwk", "draw"], all), "top pair or better, flush draws and open-enders");
  assert.equal(S.phraseTypes(["tpgk", "tpwk", "acehigh"], all), "top pair, ace high");
  // The ladder skips hand types nobody holds: no overpairs on an ace-high flop.
  const noOverpair = new Set([...all].filter((c) => c !== "overpair"));
  assert.equal(S.phraseTypes(["strong", "tpgk"], noOverpair), "top pair with a good kicker or better");
  assert.equal(S.phraseTypes(["weakdraw", "air"], all, { weakdraw: "gutshots" }), "gutshots, air");
});

test("weak draws are named after what the board allows", () => {
  const hands = (board, combos) => combos.map((c) => ({ cat: cat(c, board), f: S.handFeatures(c, board) }));
  assert.deepEqual(S.typeWords(hands("Ah8h3h", ["Kd5d", "7s6s"])), {}); // neither is a weak draw
  assert.deepEqual(S.typeWords(hands("Ah8h3h", ["5d4d"])), { weakdraw: "gutshots" }); // to the wheel
  assert.deepEqual(S.typeWords(hands("KdKc4s", ["QdJd", "9s8s"])), { weakdraw: "backdoor flush draws" });
  assert.deepEqual(S.typeWords(hands("Ts9d4c", ["Qh8d", "6d5d"])), {}); // one of each
});

test("rule lines group hand types by action and share lines for the same mix", () => {
  const actions = [{ type: "check" }, { type: "bet", pct: 33 }];
  const present = new Set(["strong", "tpgk", "weak", "weakdraw", "acehigh", "air"]);
  const rules = {
    strong: { kind: "pure", action: 1, mix: [0, 1] },
    tpgk: { kind: "pure", action: 1, mix: [0, 1] },
    weak: { kind: "pure", action: 0, mix: [1, 0] },
    weakdraw: { kind: "mix", mix: [0.5, 0.5] },
    acehigh: { kind: "split", feature: "bdfd", yes: 1, no: 0, weights: [1, 1], mix: [[0, 1], [1, 0]] },
    air: { kind: "mix", mix: [0.5, 0.5] },
  };
  const lines = S.ruleLines(rules, actions, present);
  assert.deepEqual(lines.map((l) => [l.head, l.text]), [
    ["Bet 33%", "top pair with a good kicker or better"],
    ["Check", "weak pairs"],
    ["Gutshots and backdoor flush draws, air", "bet 33% half the time, otherwise check"],
    ["Ace high", "bet 33% with a backdoor flush draw, otherwise check"],
  ]);
  assert.deepEqual(lines[2].cats, ["weakdraw", "air"]);
});

test("rules: simple is one action per type, balanced and close add splits or mixes", () => {
  const k = 2;
  // Check and bet; each type's combos (w) and how they play summed over them (f).
  const stats = {
    strong: { w: 1, f: [0, 1], splits: {} },
    // Ace high bets with a backdoor flush draw and checks without.
    acehigh: { w: 2, f: [1, 1], splits: { bdfd: [{ w: 1, f: [0.05, 0.95] }, { w: 1, f: [0.95, 0.05] }] } },
    // Air bets half the time, whatever it holds.
    air: { w: 3, f: [1.5, 1.5], splits: {} },
  };
  const betShare = (rules) =>
    Object.entries(rules).reduce((bet, [c, r]) => bet + (r.kind === "split"
      ? r.weights[0] * r.mix[0][1] + r.weights[1] * r.mix[1][1]
      : stats[c].w * r.mix[1]), 0) / 6;
  const solver = 3.5 / 6;

  const simple = S.rulesFor(stats, k, { detail: "simple" });
  assert.ok(Object.values(simple).every((r) => r.kind === "pure"));
  assert.equal(simple.strong.action, 1);
  assert.ok(Math.abs(betShare(simple) - solver) > 0.05);

  const close = S.rulesFor(stats, k, { detail: "close" });
  assert.equal(close.strong.kind, "pure");
  assert.equal(close.acehigh.kind, "split");
  assert.equal(close.acehigh.feature, "bdfd");
  assert.deepEqual([close.acehigh.yes, close.acehigh.no], [1, 0]);
  assert.equal(close.air.kind, "mix");
  assert.deepEqual(close.air.mix, [0.5, 0.5]);

  // Balanced adds detail until the range bets within 5 points of the solver's frequency.
  const balanced = S.rulesFor(stats, k, { detail: "balanced" });
  assert.ok(Math.abs(betShare(balanced) - solver) <= 0.05 + 1e-9);
});

test("cautions flag sizes the solver hardly uses and big shifts in the range", () => {
  const actions = [{ type: "check" }, { type: "bet", pct: 33 }, { type: "bet", pct: 75 }];
  assert.deepEqual(S.cautions([0.5, 0.3, 0.2], [0.5, 0.5, 0], actions), []);
  const unsupported = S.cautions([0.5, 0.47, 0.03], [0.5, 0.3, 0.2], actions);
  assert.equal(unsupported.length, 1);
  assert.match(unsupported[0], /hardly uses bet 75%/);
  const shift = S.cautions([0.6, 0.4, 0], [0.2, 0.8, 0], actions);
  assert.ok(shift.some((c) => /Checks go from 60% to 20%/.test(c)));
});

test("textures: every flop lands in one group", () => {
  const cases = {
    Ks7d2c: "high", Jd8c3s: "high", Ah8h3h: "monotone", "7h7c2d": "paired", TcTd5h: "paired",
    AhKc5d: "ace", Ac5d4s: "ace", KhQdTs: "broadway", QhJs4c: "broadway", Td9d6h: "connected",
    "5c4h3d": "connected", "6s4d2h": "connected", Tc5h2d: "low", "9c5h2d": "low",
  };
  for (const [flop, want] of Object.entries(cases)) assert.equal(S.textureOf(flop), want, flop);
  for (const t of Object.values(cases)) assert.ok(S.TEXTURE[t]);
});

// ---- On the solved library -----------------------------------------------------------------

test("the solver's own strategy gives up nothing, and squeezing keeps every hand's mix whole", async () => {
  const [{ flop, node }] = await decisions("btn_vs_bb_srp", ["Check"]);
  const prep = S.prepare(node, flop);
  assert.ok(prep.hands.length > 100);
  assert.ok(close(sum(prep.freq), 1, 1e-9));
  const own = S.evaluate(prep, (h) => h.sigma);
  assert.ok(Math.abs(own.raw) < 1e-9);
  assert.ok(own.drift < 1e-12);

  const bet33 = node.actions.findIndex((a) => a.type === "bet" && a.pct === 33);
  assert.ok(bet33 > 0);
  const keep = [0, bet33];
  const squeezed = S.squeeze(prep, keep);
  squeezed.forEach((s) => {
    assert.ok(close(sum(s), 1, 1e-9));
    s.forEach((x, a) => keep.includes(a) || assert.equal(x, 0));
  });
});

test("every decision in the library simplifies", async () => {
  const index = await D.loadIndex();
  for (const spot of index.spots) {
    for (const { flop } of spot.flops.slice(0, 4)) {
      const sol = await D.loadSolution(spot.id, flop);
      for (const node of sol.nodes.filter((n) => n.path.length <= 2 && n.actions.length > 1)) {
        const where = `${spot.id} ${flop} ${node.path.join(" / ") || "root"}`;
        const res = S.simplify(node, flop);
        if (!res.prep.hands.length) continue; // no hand gets here: the Simple tab says so
        assert.ok(res.levels.length >= 2, where);
        assert.ok(res.levels.includes(res.recommended), where);
        assert.equal(res.levels.filter((l) => l.kind === "one").length, 1, where);
        for (const level of res.levels) {
          assert.ok(level.loss >= 0 && level.lossPct >= 0, where);
          assert.ok(level.lines.length >= 1, where);
          assert.ok(close(sum(level.freq), 1, 1e-6), where);
          node.hands.forEach((row, i) => {
            const s = level.strategies[i];
            if (row && row[0] > 0) assert.ok(s && close(sum(s), 1, 1e-9), where);
            else assert.equal(s, null, where);
          });
          if (level.kind === "types") {
            for (const c of res.present) assert.ok(level.rules[c], `${where}: no rule for ${c}`);
            if (level.id.startsWith("size:")) {
              const dropped = res.prep.aggressive.filter((a) => a !== level.size);
              dropped.forEach((a) => assert.equal(level.freq[a], 0, where));
            }
          }
        }
        // The recommendation is within the tolerance unless nothing is.
        const within = res.levels.filter((l) => l.lossPct <= res.tolerance);
        if (within.length) assert.ok(res.recommended.lossPct <= res.tolerance, where);
        else assert.equal(res.recommended.loss, Math.min(...res.levels.map((l) => l.loss)), where);
      }
    }
  }
});

test("balanced rules keep the range's frequencies at least as close as one action per type", async () => {
  for (const { flop, node } of await decisions("btn_vs_bb_srp", ["Check", "Bet 33%"])) {
    const simple = S.simplify(node, flop, { detail: "simple" }).levels.find((l) => l.id === "types");
    const balanced = S.simplify(node, flop, { detail: "balanced" }).levels.find((l) => l.id === "types");
    assert.ok(balanced.drift <= simple.drift + 1e-9, flop);
  }
});

test("rules by texture: one set of rules scored on every flop of the group", async () => {
  const items = await decisions("btn_vs_bb_srp", ["Check"]);
  assert.ok(items.length >= 10);
  const res = S.simplifyGroup(items, {});
  assert.deepEqual(res.flops, items.map((x) => x.flop));
  assert.ok(close(sum(res.freq), 1, 1e-9));
  for (const level of res.levels) {
    assert.equal(level.flops.length, items.length);
    const avg = sum(level.flops.map((f) => f.lossPct)) / items.length;
    assert.ok(close(level.lossPct, avg, 1e-12));
    assert.ok(level.worst.lossPct >= level.lossPct - 1e-12);
    assert.ok(close(sum(level.freq), 1, 1e-6));
  }
  // A group of one flop gives the same rules as that flop on its own.
  const [one] = items;
  const alone = S.simplify(one.node, one.flop);
  const group = S.simplifyGroup([one], {});
  for (const level of alone.levels) {
    const same = group.levels.find((l) => l.id === level.id);
    assert.ok(same, level.id);
    assert.ok(close(same.loss, level.loss, 1e-9), level.id);
    assert.deepEqual(same.lines.map((l) => l.text), level.lines.map((l) => l.text), level.id);
  }
  // Every texture group of the spot comes out, and each flop is in one.
  const byTexture = new Map();
  for (const it of items) byTexture.set(S.textureOf(it.flop), (byTexture.get(S.textureOf(it.flop)) || 0) + 1);
  assert.equal(sum([...byTexture.values()]), items.length);
  for (const t of byTexture.keys()) assert.ok(S.simplifyGroup(items.filter((x) => S.textureOf(x.flop) === t), {}));
});
