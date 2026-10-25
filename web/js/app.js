import { initStudy, openInStudy } from "./study.js";
import { initTrainer } from "./trainer.js";
import { initRanges } from "./ranges.js";

const views = {
  study: document.getElementById("view-study"),
  trainer: document.getElementById("view-trainer"),
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

function start(name) {
  if (name === "study") return initStudy(views.study);
  if (name === "ranges") return initRanges(views.ranges);
  return initTrainer(views.trainer, {
    onOpenStudy: async (spot, flop, path, cls) => {
      await show("study");
      await openInStudy(views.study, spot, flop, path, cls);
    },
  });
}

document.querySelectorAll("header nav button").forEach((b) => (b.onclick = () => show(b.dataset.view)));

let initial = "trainer";
try {
  initial = localStorage.getItem("pokertrainer.view") || initial;
} catch {}
show(views[initial] ? initial : "trainer");
