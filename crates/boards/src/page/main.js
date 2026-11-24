// Starts the board's page: builds it, reads the board, and listens to the
// app (which shows the page in its window) and to the board's file.

import { h, toast } from "/_artifact/kit/ui.js";
import { state, elements, byId, on, load, listen, flush, tell, tellSelection, select, sendPicture } from "./model.js";
import { build as buildCanvas, draw, stopEditing } from "./draw.js";
import { wire, fit, show } from "./tools.js";
import { build as buildBars } from "./bars.js";
import { union } from "./shapes.js";
import { picture } from "./extras.js";

const app = document.getElementById("app");

async function start() {
  if (!state.owner) app.classList.add("viewer");
  const stage = h("div", { class: "stage" });
  app.append(stage);
  buildCanvas(stage);
  buildBars(app);
  wire();

  on("failed", (error) => toast(error.message || String(error), { kind: "error", stay: 8000 }));
  on("elsewhere", () => toast("The board was changed while you worked (by the agent, or in another window). Your last change is one Undo away."));
  on("command", (command) => {
    // The app, or an agent through it, points at part of the board.
    if (command.toast) toast(command.toast, { kind: command.error ? "error" : "" });
    if (command.fit) fit();
    const ids = Array.isArray(command.show) ? command.show.filter((id) => byId(id)) : [];
    if (ids.length) {
      select(ids);
      show(union(ids.map(byId)));
    }
  });

  // What goes wrong in the page is the app's to log: a web view has no
  // console anyone reads.
  window.addEventListener("error", (event) => tell({ error: String(event.message), at: `${event.filename}:${event.lineno}` }));
  window.addEventListener("unhandledrejection", (event) => tell({ error: String(event.reason?.message || event.reason) }));

  await load();
  draw();
  // Heard from here on: a change said while the board was first read is
  // in what was read.
  listen();
  // The app may hold what a page before this one had selected.
  tellSelection();
  // Once the page has its size.
  requestAnimationFrame(() => fit());

  // The page is left (closed, or put away): what is being typed is kept,
  // and what waits to be written is written now.
  const leave = () => {
    if (!state.owner) return;
    stopEditing();
    flush();
  };
  window.addEventListener("pagehide", leave);
  document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && leave());

  // Beside its workspace's chat the board follows the agent: what it draws
  // stays in sight, until the user looks somewhere themselves.
  if (state.preview) {
    let following = true;
    for (const moved of ["wheel", "pointerdown"]) stage.addEventListener(moved, () => (following = false), { capture: true, passive: true });
    on("board", () => following && requestAnimationFrame(() => following && fit()));
  }

  // The board's picture in the list: drawn once it has been quiet a while,
  // not at every note moved. (Also from beside a chat, where a board an
  // agent made is first seen.)
  if (window.BOARD?.owner) {
    let due = 0;
    let held = false;
    window.addEventListener("pointerdown", () => (held = true), true);
    for (const let_go of ["pointerup", "pointercancel"]) window.addEventListener(let_go, () => (held = false), true);
    const soon = (after) => {
      clearTimeout(due);
      due = setTimeout(() => {
        // Not under the user's hands: it waits while something is typed
        // or dragged, and the page is not seen.
        if (held || state.editing || document.visibilityState === "hidden") return soon(2000);
        picture(1280, 800, 1)
          .then((blob) => sendPicture("thumbnail", blob))
          .catch(() => {});
      }, after);
    };
    on("board", () => soon(4000));
    soon(800);
  }

  if (window.BOARD?.embedded) {
    // The chat beside the board takes the keys until the board is used.
    window.addEventListener("focus", () => tell({ focus: true }));
    if (document.hasFocus()) tell({ focus: true });
  }
  tell({ opened: true, elements: elements().length });
}

start().catch((error) => {
  app.replaceChildren(h("div", { class: "broken" }, "The board could not be opened: " + error.message));
});
