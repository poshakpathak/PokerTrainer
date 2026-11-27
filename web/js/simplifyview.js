// Simple strategies on screen: the Simple tab of the Study and Postflop solver views (one
// decision), and the Simplify tab (one decision across the library's flops, by board texture).
// The work is done in simplify.js.

import { cardsHtml, handClass } from "./cards.js";
import { loadIndex, loadSolution, nodeHands, rangeFrequencies, actionColor } from "./data.js";
import { renderGrid, liveCombos } from "./grid.js";
import * as S from "./simplify.js";

const SETTINGS_KEY = "pokertrainer.simplify.v1";
const DETAILS = [
  ["simple", "One action per hand type"],
  ["balanced", "Balanced"],
  ["close", "Close to the solver"],
];
const AIMS = [0.005, 0.01, 0.02];

/** Shared by every view: how detailed the rules are and how much EV they may give up. */
const settings = { detail: "balanced", tolerance: 0.01 };
try {
  const saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) || "null");
  if (DETAILS.some(([id]) => id === saved?.detail)) settings.detail = saved.detail;
  if (AIMS.includes(saved?.tolerance)) settings.tolerance = saved.tolerance;
} catch {}

function saveSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {}
}

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const pct = (x, d = 0) => `${(x * 100).toFixed(d)}%`;
const potPct = (x) => `${(Math.max(0, x) * 100).toFixed(x < 0.001 ? 2 : 1)}%`;
const bbs = (x) => `${Math.max(0, x).toFixed(2)}bb`;

function addScaled(acc, x, w) {
  for (let a = 0; a < acc.length; a++) acc[a] += x[a] * w;
}

// ---- Pieces shared by both views -----------------------------------------------------------

function freqBar(mix, actions, cls = "") {
  return `<div class="freqbar ${cls}">${actions.map((a, i) => `<span style="width:${mix[i] * 100}%;background:${actionColor(a, actions)}"></span>`).join("")}</div>`;
}

/** A rule's mix for a whole hand type, splits weighted by how many combos fall on each side. */
function ruleMix(rule, k, action) {
  if (!rule) return Array.from({ length: k }, (_, i) => (i === action ? 1 : 0));
  if (rule.kind !== "split") return rule.mix;
  const [a, b] = rule.weights;
  return rule.mix[0].map((x, i) => (x * a + rule.mix[1][i] * b) / (a + b));
}

function ruleText(rule, actions, action) {
  if (!rule) return S.phraseAction(actions[action]);
  if (rule.kind === "pure") return S.phraseAction(actions[rule.action]);
  if (rule.kind === "mix") return cap(S.phraseMix(rule.mix, actions));
  const feat = S.FEATURES.find((f) => f.id === rule.feature);
  return `${S.phraseAction(actions[rule.yes])} ${feat.when}, otherwise ${S.phraseAction(actions[rule.no]).toLowerCase()}`;
}

const cap = (s) => s[0].toUpperCase() + s.slice(1);

function lineMarker(line, level, actions) {
  if (line.kind === "pure") return `<i style="background:${actionColor(actions[line.action], actions)}"></i>`;
  const mix = ruleMix(level.rules[line.cats[0]], actions.length);
  return `<span class="mini">${actions.map((a, i) => `<span style="width:${mix[i] * 100}%;background:${actionColor(a, actions)}"></span>`).join("")}</span>`;
}

function rulesHtml(level, actions) {
  return `<ul class="rulelines">${level.lines.map((line) => `<li>${lineMarker(line, level, actions)}<b>${esc(line.head)}</b>
    <span>${esc(line.text)}</span></li>`).join("")}</ul>`;
}

/** The level picker: every simple strategy with what it gives up. */
function levelsHtml(res, level) {
  return `<div class="levels">${res.levels.map((l) => `<button data-level="${esc(l.id)}" class="${l === level ? "active" : ""}" title="${esc(l.cautions.join(" "))}">
      <b>${esc(l.name)}</b>
      <small>${l === res.recommended ? "Recommended · " : ""}gives up ${potPct(l.lossPct)}${l.cautions.length ? " · see note" : ""}</small>
    </button>`).join("")}</div>`;
}

function compareHtml(solver, simple, actions) {
  return `<div class="cmpbars">
    <div><span>Solver</span>${freqBar(solver, actions)}</div>
    <div><span>Simple</span>${freqBar(simple, actions)}</div>
    <p class="legend">${actions.map((a, i) => `<span><i style="background:${actionColor(a, actions)}"></i>${esc(S.phraseAction(a))} ${pct(solver[i])} → <b>${pct(simple[i])}</b></span>`).join("")}</p>
  </div>`;
}

/** Hand types: how much of the range each is, how the solver plays it, the rule and its cost. */
function typesHtml(res, level, actions, selected) {
  const k = actions.length;
  const rows = S.CATEGORIES.filter((c) => res.present.has(c.id) && res.types[c.id]?.w > 0).map((c) => {
    const t = res.types[c.id];
    const solver = t.f.map((x) => x / t.w);
    const rule = level.rules?.[c.id];
    const mix = ruleMix(rule, k, level.action);
    const loss = level.byCat?.[c.id]?.loss ?? 0;
    return `<tr data-type="${c.id}" class="${selected === c.id ? "chosen" : ""}" title="${esc(c.hint)}">
      <td>${esc(c.name)}</td>
      <td>${pct(t.w)}</td>
      <td>${freqBar(solver, actions, "slim")}<small>${solver.map((x, i) => (x >= 0.005 ? `${esc(S.phraseAction(actions[i]))} ${pct(x)}` : "")).filter(Boolean).join(" · ")}</small></td>
      <td>${freqBar(mix, actions, "slim")}<small>${esc(ruleText(rule, actions, level.action))}</small></td>
      <td>${bbs(loss)}</td>
    </tr>`;
  });
  return `<table class="types">
    <thead><tr><th>Hand type</th><th>Range</th><th>Solver</th><th>Simple rule</th><th title="EV given up per combo of this type">EV lost</th></tr></thead>
    <tbody>${rows.join("")}</tbody>
  </table>`;
}

function optionsHtml(id) {
  return `<div class="simpleopts" id="${id}">
    <label>Rules <select data-opt="detail">${DETAILS.map(([v, t]) => `<option value="${v}" ${settings.detail === v ? "selected" : ""}>${t}</option>`).join("")}</select></label>
    <label>Recommend the simplest that gives up at most <select data-opt="tolerance">${AIMS.map((v) => `<option value="${v}" ${settings.tolerance === v ? "selected" : ""}>${pct(v, 1)} of the pot</option>`).join("")}</select></label>
  </div>`;
}

function wireOptions(el, onChange) {
  el.onchange = (e) => {
    const opt = e.target.dataset.opt;
    if (!opt) return;
    settings[opt] = opt === "tolerance" ? +e.target.value : e.target.value;
    saveSettings();
    onChange();
  };
}

function cautionsHtml(level) {
  return level.cautions.length ? `<ul class="cautions">${level.cautions.map((c) => `<li>${esc(c)}</li>`).join("")}</ul>` : "";
}

/** The rules as plain text, for notes. */
function rulesText(title, level, actions, lossLine) {
  const lines = level.lines.map((l) => `- ${l.head}: ${l.text}`);
  return [title, `${level.name} (${lossLine})`, ...lines].join("\n");
}

async function copy(text, button) {
  const label = button.textContent;
  try {
    await navigator.clipboard.writeText(text);
    button.textContent = "Copied";
  } catch {
    button.textContent = "Couldn't copy";
  }
  setTimeout(() => (button.textContent = label), 1500);
}

const HOW = `How it works: every hand type gets the action the solver takes most with it; types it plays
  more than one way get a split ("with a backdoor flush draw") or a mix rounded to quarters where that
  keeps your frequencies close to the solver's. EV lost uses the solver's EV for each action of each
  hand, so it's what the simple strategy loses if the other player keeps playing the solver's strategy.
  An opponent who adjusts to it can take back more.`;

// ---- One decision: the Simple tab in Study and the Postflop solver -------------------------

const cache = new WeakMap();

function simplified(node, board) {
  const key = `${board}|${settings.detail}|${settings.tolerance}`;
  const hit = cache.get(node);
  if (hit?.key === key) return hit.res;
  const res = S.simplify(node, board, settings);
  cache.set(node, { key, res });
  return res;
}

/**
 * Draws a decision's simple strategy.
 * @param els   {grid, detail, simple} elements
 * @param node  decision node: {player, actions, hands, combos, pot_bb}
 * @param view  view state shared with the other modes: {focus, pinned, level, type}
 * @param opts  {board, name(player), title, onLock(level)?: lock it on pt-server}
 */
export function renderSimple(els, node, view, opts) {
  const res = simplified(node, opts.board);
  const actions = node.actions;
  const k = actions.length;
  if (!res.prep.hands.length) {
    els.grid.innerHTML = "";
    els.simple.innerHTML = `<p class="hint">No hands get to this decision.</p>`;
    els.detail.innerHTML = "";
    return;
  }
  const level = res.levels.find((l) => l.id === view.level) || res.recommended;
  const other = opts.name(node.player === "oop" ? "ip" : "oop");

  // Per hand class: the simple and solver mixes, for the grid and the hand detail.
  const classes = new Map();
  for (const h of res.prep.hands) {
    const cls = handClass(h.combo);
    let c = classes.get(cls);
    if (!c) classes.set(cls, (c = { reach: 0, mix: new Array(k).fill(0), hands: [] }));
    c.reach += h.r;
    addScaled(c.mix, level.strategies[h.i], h.r);
    c.hands.push(h);
  }
  const share = new Map([...classes].map(([cls, c]) => [cls, c.reach / Math.max(1, liveCombos(cls, opts.board))]));
  const maxShare = Math.max(1e-9, ...share.values());
  const marked = view.type ? new Set([...classes].filter(([, c]) => c.hands.some((h) => h.cat === view.type)).map(([cls]) => cls)) : null;

  const detail = (cls) => showHand(els.detail, cls, classes.get(cls), level, actions);
  const draw = () =>
    renderGrid(els.grid, {
      selected: view.pinned ? view.focus : null,
      cell: (cls) => {
        const c = classes.get(cls);
        if (!c) return null;
        return {
          fill: share.get(cls) / maxShare,
          segments: actions.map((a, i) => ({ color: actionColor(a, actions), frac: c.mix[i] / c.reach })),
          marked: !!marked?.has(cls),
        };
      },
      onHover: (cls) => {
        if (!view.pinned) {
          view.focus = cls;
          detail(cls);
        }
      },
      onClick: (cls) => {
        view.pinned = !(view.pinned && view.focus === cls);
        view.focus = cls;
        detail(cls);
        draw();
      },
    });
  draw();
  detail(view.focus);

  const lossLine = `gives up ${potPct(level.lossPct)} of the pot`;
  const precise = res.gapPct > 0.01
    ? `<p class="cautions warn">The solve hasn't settled at this decision: the solver's own mix here is ${potPct(res.gapPct)} of the pot short of its best play, so these numbers are rough. That happens at spots the solver rarely reaches.</p>`
    : "";
  els.simple.innerHTML = `
    <div class="simple">
      ${levelsHtml(res, level)}
      ${rulesHtml(level, actions)}
      <p class="evsum">${esc(level.name)} gives up <b>${bbs(level.loss)}</b> per hand here, <b>${potPct(level.lossPct)} of the pot</b>, if ${esc(other)} keeps playing the solver's strategy.${
        level.lossPct <= res.gapPct ? ` That's within the solve's precision here (${potPct(res.gapPct)} of the pot).` : ""}</p>
      ${precise}
      ${cautionsHtml(level)}
      ${compareHtml(res.freq, level.freq, actions)}
      ${typesHtml(res, level, actions, view.type)}
      <p class="hint">Click a hand type to see its hands in the grid. ${HOW}</p>
      ${optionsHtml("sm-opts")}
      <div class="simplebtns">
        <button class="secondary" id="sm-copy">Copy rules</button>
        ${opts.onLock ? `<button class="secondary" id="sm-lock">Lock this simple strategy</button>` : ""}
      </div>
      ${opts.onLock ? `<p class="hint" id="sm-lock-note">Lock it and solve with the lock to see what it costs once ${esc(other)} adjusts: the Locks panel shows both players' EV for the whole hand, GTO against the locked solve.</p>` : ""}
    </div>`;

  const rerender = () => renderSimple(els, node, view, opts);
  els.simple.querySelector(".levels").onclick = (e) => {
    const id = e.target.closest("button")?.dataset.level;
    if (!id) return;
    view.level = id;
    rerender();
  };
  els.simple.querySelector("table.types tbody").onclick = (e) => {
    const type = e.target.closest("tr")?.dataset.type;
    if (!type) return;
    view.type = view.type === type ? null : type;
    rerender();
  };
  wireOptions(els.simple.querySelector("#sm-opts"), () => {
    view.level = null;
    rerender();
  });
  const copyBtn = els.simple.querySelector("#sm-copy");
  copyBtn.onclick = () => copy(rulesText(opts.title || "", level, actions, lossLine), copyBtn);
  const lockBtn = els.simple.querySelector("#sm-lock");
  if (lockBtn) {
    lockBtn.onclick = async () => {
      lockBtn.disabled = true;
      lockBtn.textContent = "Locking…";
      try {
        await opts.onLock(level, res);
      } catch (e) {
        lockBtn.disabled = false;
        lockBtn.textContent = "Lock this simple strategy";
        els.simple.querySelector("#sm-lock-note").innerHTML = `<span class="error">${esc(e.message)}</span>`;
      }
    };
  }
}

/** The hand detail: every combo of a class with its hand type, the solver's mix and the rule's. */
function showHand(el, cls, c, level, actions) {
  if (!c) {
    el.innerHTML = cls
      ? `<h3>${esc(cls)}</h3><p class="hint">Not in this range at this point.</p>`
      : `<h3>Hand detail</h3><p class="hint">Hover a cell to see each combo's hand type, the solver's play and the simple rule. Click to pin it.</p>`;
    return;
  }
  const mixCell = (mix) => `${freqBar(mix, actions, "slim")}<small>${mix.map((x, i) => (x >= 0.005 ? `${esc(S.phraseAction(actions[i]))} ${pct(x)}` : "")).filter(Boolean).join(" · ")}</small>`;
  const rows = c.hands
    .slice()
    .sort((a, b) => S.CATEGORIES.findIndex((x) => x.id === a.cat) - S.CATEGORIES.findIndex((x) => x.id === b.cat))
    .map((h) => {
      const s = level.strategies[h.i];
      const v = s.reduce((sum, x, a) => sum + x * h.ev[a], 0);
      return `<tr><td>${cardsHtml(h.combo, "tiny")}<small>${esc(S.CATEGORY[h.cat].name)}</small></td>
        <td>${mixCell(h.sigma)}</td><td>${mixCell(s)}</td><td>${bbs(Math.max(0, h.v - v))}</td></tr>`;
    });
  const types = [...new Set(c.hands.map((h) => h.cat))];
  el.innerHTML = `
    <h3>${esc(cls)} <small>${c.reach.toFixed(1)} combos · ${types.map((t) => esc(S.CATEGORY[t].name)).join(", ")}</small></h3>
    <table class="combos simplecombos">
      <thead><tr><th></th><th>Solver</th><th>Simple</th><th>EV lost</th></tr></thead>
      <tbody>${rows.join("")}</tbody>
    </table>
    <p class="hint">EV lost is in bb, for this combo, against the solver's mix.</p>`;
}

// ---- Many flops: the Simplify tab ----------------------------------------------------------

const tab = { root: null, hooks: {}, index: null, spot: null, sols: [], path: null, picked: new Map(), seq: 0, groups: null };

export async function initSimplify(root, hooks = {}) {
  tab.root = root;
  tab.hooks = hooks;
  tab.index = await loadIndex();
  const spots = tab.index.spots.filter((s) => s.flops.length);
  if (!spots.length) {
    root.innerHTML = `<p class="panel">No solved flops yet. Run <code>python3 scripts/build_library.py</code>.</p>`;
    return;
  }
  root.innerHTML = `
    <div class="reports simplify">
      <aside class="panel controls">
        <label>Spot <select id="sp-spot">${spots.map((s) => `<option value="${s.id}">${esc(s.name)}</option>`).join("")}</select></label>
        <label>Decision <select id="sp-node"></select></label>
        ${optionsHtml("sp-opts")}
        <p class="hint">Simple rules for one decision on every solved flop, grouped by board texture: one bet size and
          one rule per hand type, made from the solver's play on all the flops in the group. EV lost is the average over
          those flops when you play the group's rules on each of them.</p>
        <p class="hint">${HOW}</p>
        <button class="secondary" id="sp-copy">Copy every rule</button>
      </aside>
      <section id="sp-main"><p class="panel hint">Loading…</p></section>
    </div>`;
  root.querySelector("#sp-spot").onchange = (e) => selectSpot(e.target.value);
  root.querySelector("#sp-node").onchange = (e) => {
    tab.path = JSON.parse(e.target.value);
    tab.picked.clear();
    drawGroups();
  };
  wireOptions(root.querySelector("#sp-opts"), () => {
    tab.picked.clear();
    drawGroups();
  });
  root.querySelector("#sp-copy").onclick = (e) => copy(allRulesText(), e.target);
  await selectSpot(spots[0].id);
}

const name = (p) => (p === "oop" ? tab.spot.oop : tab.spot.ip);

/** "BB checks, BTN bets 33%": the line to a library node. */
function lineName(sol, node) {
  if (!node.path.length) return `${name(node.player)} first to act`;
  const steps = node.path.map((label, i) => {
    const actor = sol.byPath.get(node.path.slice(0, i).join(" / "));
    const a = actor.actions[actor.labels.indexOf(label)];
    return `${name(actor.player)} ${verb(a)}`;
  });
  return `${steps.join(", ")}: ${name(node.player)} to act`;
}

function verb(a) {
  switch (a.type) {
    case "check": return "checks";
    case "call": return "calls";
    case "fold": return "folds";
    case "bet": return `bets ${a.pct}%`;
    case "raise": return `raises to ${a.to_bb}bb`;
    case "allin": return "goes all-in";
    default: return a.type;
  }
}

async function selectSpot(id) {
  const seq = ++tab.seq;
  tab.spot = tab.index.spots.find((s) => s.id === id);
  tab.picked.clear();
  tab.root.querySelector("#sp-main").innerHTML = `<p class="panel hint">Loading ${tab.spot.flops.length} flops…</p>`;
  tab.sols = (await Promise.all(tab.spot.flops.map(({ flop }) => loadSolution(tab.spot.id, flop).then((sol) => ({ flop, sol })).catch(() => null)))).filter(Boolean);
  if (seq !== tab.seq) return;
  if (!tab.sols.length) {
    tab.root.querySelector("#sp-node").innerHTML = "";
    tab.root.querySelector("#sp-main").innerHTML = `<p class="panel hint">Couldn't load the solved flops for this spot.</p>`;
    return;
  }
  // Decisions that come up at least 2% of the time on average, up to three actions deep.
  const often = new Map();
  for (const { sol } of tab.sols) {
    for (const n of sol.nodes) {
      if (n.path.length > 3 || n.actions.length < 2) continue;
      often.set(n.key, (often.get(n.key) || 0) + lineOdds(sol, n) / tab.sols.length);
    }
  }
  const first = tab.sols[0].sol;
  const nodes = first.nodes.filter((n) => (often.get(n.key) || 0) >= 0.02);
  const sel = tab.root.querySelector("#sp-node");
  sel.innerHTML = nodes.map((n) => `<option value='${esc(JSON.stringify(n.path))}'>${esc(lineName(first, n))}</option>`).join("");
  const keep = nodes.find((n) => JSON.stringify(n.path) === JSON.stringify(tab.path));
  tab.path = keep ? keep.path : nodes.find((n) => n.path.length === 1)?.path || nodes[0].path;
  sel.value = JSON.stringify(tab.path);
  drawGroups();
}

/** How often a hand goes down the line to a node: each action's share of the range that takes it. */
function lineOdds(sol, node) {
  let p = 1;
  node.path.forEach((label, i) => {
    const actor = sol.byPath.get(node.path.slice(0, i).join(" / "));
    const { freq } = rangeFrequencies(nodeHands(actor), actor.actions.length);
    p *= freq[actor.labels.indexOf(label)] || 0;
  });
  return p;
}

function drawGroups() {
  const main = tab.root.querySelector("#sp-main");
  const items = [];
  for (const { flop, sol } of tab.sols) {
    const node = sol.byPath.get(tab.path.join(" / "));
    if (node) items.push({ flop, node, texture: S.textureOf(flop) });
  }
  if (!items.length) {
    main.innerHTML = `<p class="panel hint">No solved flop has this decision.</p>`;
    return;
  }
  const groups = [{ id: "all", name: "Every flop", hint: "One set of rules for all the solved flops." }, ...S.TEXTURES]
    .map((t) => {
      const members = t.id === "all" ? items : items.filter((x) => x.texture === t.id);
      return members.length ? { ...t, members, res: S.simplifyGroup(members, settings) } : null;
    })
    .filter((g) => g?.res);
  tab.groups = groups;
  const sel = tab.root.querySelector("#sp-node");
  main.innerHTML = `<h2 class="sptitle">${esc(tab.spot.name)} · ${esc(sel.options[sel.selectedIndex]?.text || "")}</h2>` +
    groups.map((g) => `<article class="panel texcard" data-group="${g.id}"></article>`).join("");
  for (const g of groups) drawCard(g);
}

function drawCard(g) {
  const el = tab.root.querySelector(`.texcard[data-group="${g.id}"]`);
  const { res } = g;
  const level = res.levels.find((l) => l.id === tab.picked.get(g.id)) || res.recommended;
  const actions = res.actions;
  const worst = level.worst;
  el.innerHTML = `
    <header>
      <h3 title="${esc(g.hint)}">${esc(g.name)} <small>${g.members.length} flop${g.members.length > 1 ? "s" : ""}</small></h3>
      <div class="texflops">${g.members.map((m) => cardsHtml(m.flop, "tiny")).map((h) => `<span>${h}</span>`).join("")}</div>
    </header>
    ${levelsHtml(res, level)}
    ${rulesHtml(level, actions)}
    <p class="evsum">Gives up <b>${potPct(level.lossPct)} of the pot</b> on average${g.members.length > 1 ? `, ${potPct(worst.lossPct)} on ${esc(worst.flop)} at worst` : ""}, if the other player keeps playing the solver's strategy.${
      level.lossPct <= res.gapPct ? ` That's within the solves' precision here (${potPct(res.gapPct)} of the pot).` : ""}</p>
    ${res.gapPct > 0.01 ? `<p class="cautions warn">The solves haven't settled at this decision (the solver's own mix is ${potPct(res.gapPct)} of the pot short of its best play), so these numbers are rough.</p>` : ""}
    ${cautionsHtml(level)}
    ${compareHtml(res.freq, level.freq, actions)}
    <details><summary>Hand types</summary>${typesHtml(res, level, actions, null)}</details>
    <details><summary>Each flop</summary>
      <table class="report flops">
        <thead><tr><th>Flop</th><th>Solver</th><th>Simple</th><th>EV lost</th></tr></thead>
        <tbody>${level.flops.map((f) => `<tr data-flop="${f.flop}" title="Open in Study">
          <td>${cardsHtml(f.flop, "tiny")}</td><td>${freqBar(f.solverFreq, actions)}</td><td>${freqBar(f.freq, actions)}</td><td>${potPct(f.lossPct)}</td>
        </tr>`).join("")}</tbody>
      </table>
      <p class="hint">Click a flop to open it in Study, on the Simple tab.</p>
    </details>`;
  el.querySelector(".levels").onclick = (e) => {
    const id = e.target.closest("button")?.dataset.level;
    if (!id) return;
    tab.picked.set(g.id, id);
    drawCard(g);
  };
  el.querySelector("table.flops tbody").onclick = (e) => {
    const flop = e.target.closest("tr")?.dataset.flop;
    if (flop && tab.hooks.onOpenStudy) tab.hooks.onOpenStudy(tab.spot.id, flop, tab.path, null, "simple");
  };
}

function allRulesText() {
  if (!tab.groups) return "";
  const sel = tab.root.querySelector("#sp-node");
  const head = `${tab.spot.name} · ${sel.options[sel.selectedIndex]?.text || ""}`;
  const blocks = tab.groups.map((g) => {
    const level = g.res.levels.find((l) => l.id === tab.picked.get(g.id)) || g.res.recommended;
    return rulesText(`${g.name} (${g.members.map((m) => m.flop).join(", ")})`, level, g.res.actions, `gives up ${potPct(level.lossPct)} of the pot on average`);
  });
  return [head, ...blocks].join("\n\n");
}

