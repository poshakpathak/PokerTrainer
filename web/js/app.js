import { initStudy, openInStudy } from "./study.js";
import { initDrill, openDrill } from "./drill.js";
import { initRanges } from "./ranges.js";
import { initMultiway, prefillMultiway } from "./multiway.js";
import { initReports } from "./reports.js";
import { initSolver, prefillSolver, openSolverAt } from "./solver.js";
import { initPreflopSolver, openPreflopAt } from "./pfsolver.js";
import { initSimplify } from "./simplifyview.js";

const views = {
  drill: document.getElementById("view-drill"),
  study: document.getElementById("view-study"),
  reports: document.getElementById("view-reports"),
  simplify: document.getElementById("view-simplify"),
  pfsolver: document.getElementById("view-pfsolver"),
  solver: document.getElementById("view-solver"),
  multiway: document.getElementById("view-multiway"),
  ranges: document.getElementById("view-ranges"),
};
const started = {};

function show(name) {
  for (const [k, el] of Object.entries(views)) el.hidden = k !== name;
  document.querySelectorAll("header nav button").forEach((b) => b.classList.toggle("active", b.dataset.view === name));
  try {
    localStorage.setItem("pokertrainer.view", name);
  } catch {}
  if (!started[name]) {
    started[name] = start(name).catch((e) => {
      views[name].innerHTML = `<p class="panel error">${e.message}</p>`;
    });
  }
  return started[name];
}

async function openStudy(spot, flop, path, cls, mode) {
  await show("study");
  await openInStudy(views.study, spot, flop, path, cls, mode);
}

/** Opens a pot from the preflop solver in the postflop Solver, ready for a board. */
async function openSolver(spot) {
  await show("solver");
  prefillSolver(views.solver, spot);
}

/** Opens a three-way pot from the preflop solver in the Multiway solver, ready for a board. */
async function openMultiway(spot) {
  await show("multiway");
  prefillMultiway(views.multiway, spot);
}

/** Opens a decision of the solve on pt-server in the postflop Solver, from the Drill. */
async function openSolverNode({ path, solution, cls }) {
  await show("solver");
  openSolverAt(views.solver, path, solution, cls);
}

/** Opens a decision of a preflop chart in the Preflop solver, from the Drill. */
async function openPreflop({ pack, history, cls }) {
  await show("pfsolver");
  await openPreflopAt(views.pfsolver, pack, history, cls);
}

/** Drills a spot picked in one of the solver tabs. */
async function drill(spec) {
  const shown = show("drill");
  openDrill(spec);
  await shown;
}

function start(name) {
  if (name === "study") return initStudy(views.study);
  if (name === "ranges") return initRanges(views.ranges);
  if (name === "solver") return initSolver(views.solver, { onDrill: drill });
  if (name === "multiway") return initMultiway(views.multiway);
  if (name === "pfsolver") return initPreflopSolver(views.pfsolver, { onOpenSolver: openSolver, onOpenMultiway: openMultiway, onDrill: drill });
  if (name === "reports") return initReports(views.reports, { onOpenStudy: openStudy });
  if (name === "simplify") return initSimplify(views.simplify, { onOpenStudy: openStudy });
  return initDrill(views.drill, { onOpenStudy: openStudy, onOpenSolver: openSolverNode, onOpenPreflop: openPreflop });
}

document.querySelectorAll("header nav button").forEach((b) => (b.onclick = () => show(b.dataset.view)));

let initial = "drill";
try {
  initial = localStorage.getItem("pokertrainer.view") || initial;
} catch {}
// The Postflop trainer and Preflop drill tabs are now part of the Drill.
if (initial === "trainer" || initial === "preflop") initial = "drill";
show(views[initial] ? initial : "drill");
