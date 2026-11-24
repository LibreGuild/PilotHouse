// Presenting from the editor: the slides filling the page, one at a time;
// or, with speaker notes, the presenter's view here (this slide, the next,
// the notes, the time) while the app shows the slides in a window of their
// own for the audience.
//
// Wherever the presenter goes, everyone with the deck open is told (through
// the artifact's room), so the audience's window and colleagues who opened
// the shared link show the same slide.

import { Show, mountSlide, fitFrame } from "./core.js";
import { h, button } from "/_artifact/kit/ui.js";
import * as store from "./store.js";
import { state, on } from "./store.js";

// The slides that are shown, with their place in the deck.
function shownSlides() {
  return state.deck.slides
    .map((slide, index) => ({ slide, index }))
    .filter(({ slide }) => !slide.skip)
    .map(({ slide, index }) => ({
      html: store.textOf(slide.file),
      transition: slide.transition || "",
      notes: slide.notes || "",
      file: slide.file,
      index,
    }));
}

const said = (data) => window.artifact.room.send({ deck: data }).catch(() => {});

export function present({ look, notes, from }) {
  let slides = shownSlides();
  if (slides.length === 0) return;
  const start = Math.max(0, slides.findIndex((slide) => slide.index >= from));
  const stage = h("div", { class: notes ? "presenter-now" : "show-stage" });
  const counter = h("span", { style: { color: "var(--ui-muted)", fontVariantNumeric: "tabular-nums" } });
  const began = Date.now();
  let timer = 0;
  let root = stage;
  let nextFrame;
  let notesBox;
  let clock;

  const controls = (show) => [
    button({ icon: "left", title: "Back", onClick: () => show.previous() }),
    counter,
    button({ icon: "right", title: "Next", onClick: () => show.next() }),
    button({ label: "End", title: "End presenting (Esc)", onClick: () => close() }),
  ];

  const drawSide = () => {
    counter.textContent = `${show.at + 1} / ${slides.length}`;
    if (!notes) return;
    notesBox.textContent = slides[show.at]?.notes || "No notes for this slide.";
    const next = slides[show.at + 1];
    const host = nextFrame.firstElementChild;
    if (next) {
      mountSlide(host, next.html, state.deck.size, look);
      fitFrame(nextFrame, state.deck.size, nextFrame.parentElement.getBoundingClientRect());
    }
    nextFrame.style.visibility = next ? "visible" : "hidden";
  };

  const show = new Show(stage, {
    size: state.deck.size,
    look,
    slides,
    at: start,
    transition: state.deck.transition || "none",
    onChange: (at) => {
      said({ at: slides[at].file });
      drawSide();
    },
    onExit: () => close(),
  });

  if (notes) {
    nextFrame = h("div", { class: "show-frame" }, h("div", {}));
    notesBox = h("div", { class: "presenter-notes" });
    clock = h("span", { class: "presenter-clock" }, "0:00");
    root = h(
      "div",
      { class: "presenter" },
      h("div", { class: "presenter-bar" }, clock, h("span", { style: { flex: "1" } }), controls(show)),
      stage,
      h("div", { class: "presenter-side" }, h("div", { class: "presenter-next" }, h("span", { class: "presenter-label" }, "Next"), nextFrame), notesBox),
    );
    timer = setInterval(() => {
      const seconds = Math.floor((Date.now() - began) / 1000);
      clock.textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
    }, 1000);
  } else {
    stage.append(h("div", { class: "bar show-hud", dataset: { showControl: "" } }, controls(show)));
  }
  document.body.append(root);
  // The window goes full screen for it (the app does that; a browser is
  // asked directly), and with notes the audience gets a window of its own.
  store.tell({ present: notes ? "notes" : "full" });
  if (!notes && !window.DECK.embedded && document.documentElement.requestFullscreen) {
    document.documentElement.requestFullscreen().catch(() => {});
  }
  said({ presenting: true, at: slides[start].file });
  if (notes) requestAnimationFrame(drawSide);
  else drawSide();

  // The deck changed while it is presented (the agent fixed a slide).
  const off = on("deck", () => {
    slides = shownSlides();
    if (slides.length === 0) {
      close();
      return;
    }
    show.update(slides);
    drawSide();
  });

  function close() {
    off();
    clearInterval(timer);
    const at = slides[show.at]?.index ?? state.current;
    show.close();
    root.remove();
    said({ presenting: false });
    store.tell({ present: false });
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    store.go(at);
  }
}
