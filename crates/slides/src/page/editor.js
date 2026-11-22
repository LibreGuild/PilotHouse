// The deck's page for its owner: the slide on a stage, the slides along the
// foot, and the few bars and panels that change them. What changes a deck
// is in edit.js (what is on a slide), panel.js (how it looks) and store.js
// (the slides themselves); this file lays the page out and draws it.

import { Look, mountSlide, fitFrame, scaleFrame, slideTitle, dress } from "./core.js";
import { h, button, menu, dialog, toast, icon, closeFloats, keys as keyHint } from "./ui.js";
import * as store from "./store.js";
import { state, on, emit } from "./store.js";
import { Editing, SHAPES } from "./edit.js";
import { Panel } from "./panel.js";
import { Comments } from "./comments.js";
import { present } from "./present.js";
import { Checks } from "./check.js";
import { measureDeck } from "./measure.js";

const TILE_HEIGHT = 70;
const remembered = (key, fallback) => {
  try {
    return localStorage.getItem("deck." + key) ?? fallback;
  } catch {
    return fallback;
  }
};
const remember = (key, value) => {
  try {
    localStorage.setItem("deck." + key, value);
  } catch {
    // Not kept: the page works the same.
  }
};

const look = new Look();
const view = {
  mode: "single", // or "grid"
  notes: remembered("notes", "off") === "on",
  panel: remembered("panel", "off") === "on",
  tray: remembered("tray", "open") !== "closed",
  zoom: null, // null fits the slide to the stage; a number is a scale
  scale: 1,
  commenting: false,
};
// The slide on the stage: its file, the text it was drawn from, its element.
const stage = { file: null, text: null, slide: null };
// file -> { tile, host, text } for the strip.
const tiles = new Map();

// ---- The page ------------------------------------------------------------

const el = {};
function build() {
  el.host = h("div", { class: "host" });
  el.frame = h("div", { class: "frame" }, el.host);
  el.notes = h("div", {
    class: "notes",
    contenteditable: "plaintext-only",
    role: "textbox",
    "aria-label": "Speaker notes",
    "aria-multiline": "true",
    spellcheck: "true",
  });
  el.empty = h("div", { class: "empty" });
  el.inner = h("div", { class: "stage-inner" }, el.frame, el.notes);
  el.stage = h("div", { class: "stage" }, el.inner, el.empty);
  el.grid = h("div", { class: "grid", hidden: true });
  el.marks = h("div", { class: "marks" });
  el.toolsLeft = h("div", { class: "tools tools-left" });
  el.toolsRight = h("div", { class: "tools tools-right" });
  el.main = h("div", { class: "main" }, el.stage, el.grid, el.marks, el.toolsLeft, el.toolsRight);
  el.panel = h("div", { class: "panel", hidden: true });
  el.trayBar = h("div", { class: "tray-bar" });
  el.strip = h("div", { class: "strip", role: "listbox", "aria-label": "Slides" });
  el.tray = h("div", { class: "tray" }, el.trayBar, el.strip);
  document.getElementById("app").replaceChildren(el.main, el.panel, el.tray);
}

let editing;
let panel;
let comments;
let checks;

// ---- Drawing -------------------------------------------------------------

// What a slide's element is saved as: itself, without what the page put
// on it to show or edit it.
export function serialize(slide) {
  const copy = slide.cloneNode(true);
  for (const drawn of copy.querySelectorAll("svg.chart-drawn")) drawn.remove();
  for (const element of [copy, ...copy.querySelectorAll("*")]) {
    element.removeAttribute("contenteditable");
    element.removeAttribute("spellcheck");
    // A size the page worked out to make text fit is worked out again
    // wherever the slide is drawn.
    if (element.getAttribute("data-fit") === "shrink") element.style.removeProperty("font-size");
    for (const name of ["class", "style"]) {
      if (element.getAttribute(name) === "") element.removeAttribute(name);
    }
  }
  return copy.outerHTML + "\n";
}

// Saves the slide on the stage as it is now. `label` says what the user did.
function commit(label) {
  if (!stage.slide || !stage.file) return;
  const text = serialize(stage.slide);
  if (text === stage.text) return;
  stage.text = text;
  store.write([{ path: stage.file, text }], label);
}

function drawStage() {
  const slide = store.currentSlide();
  const none = state.deck.slides.length === 0;
  el.inner.hidden = none || view.mode !== "single";
  el.empty.hidden = !none || view.mode !== "single";
  el.stage.hidden = view.mode !== "single";
  el.grid.hidden = view.mode !== "grid";
  if (none) {
    stage.file = stage.text = stage.slide = null;
    editing.slideChanged();
    drawEmpty();
    return;
  }
  const text = store.textOf(slide.file);
  if (stage.file !== slide.file || stage.text !== text) {
    stage.file = slide.file;
    stage.text = text;
    stage.slide = mountSlide(el.host, text, state.deck.size, look);
    editing.slideChanged();
  }
  drawNotes();
  fitStage();
}

function drawNotes() {
  el.notes.hidden = !view.notes;
  const notes = store.currentSlide()?.notes || "";
  // Not while it is being typed in: the caret would jump.
  if (document.activeElement !== el.notes && el.notes.textContent !== notes) el.notes.textContent = notes;
}

function fitStage() {
  if (!stage.slide || el.stage.hidden) return;
  const box = el.stage.getBoundingClientRect();
  const styles = getComputedStyle(el.stage);
  const within = {
    width: box.width - parseFloat(styles.paddingLeft) - parseFloat(styles.paddingRight),
    height:
      box.height -
      parseFloat(styles.paddingTop) -
      parseFloat(styles.paddingBottom) -
      (view.notes ? Math.min(el.notes.offsetHeight + 14, box.height * 0.3) : 0),
  };
  if (view.zoom === null) view.scale = fitFrame(el.frame, state.deck.size, within);
  else {
    view.scale = view.zoom;
    scaleFrame(el.frame, state.deck.size, view.zoom);
  }
  el.notes.style.width = el.frame.style.width;
  el.zoomLabel.textContent = Math.round(view.scale * 100) + "%";
  editing.refresh();
  comments.refresh();
}

// A small picture of a slide: its text drawn at `height`.
function thumb(text, height) {
  const host = h("div", { class: "host" });
  const box = h("div", { class: "thumb" }, host);
  mountSlide(host, text, state.deck.size, look);
  scaleFrame(box, state.deck.size, height / state.deck.size[1]);
  return { box, host };
}

function drawStrip() {
  const wanted = state.deck.slides;
  const seen = new Set();
  wanted.forEach((slide, index) => {
    seen.add(slide.file);
    let entry = tiles.get(slide.file);
    const text = store.textOf(slide.file);
    if (!entry) {
      const { box, host } = thumb(text, TILE_HEIGHT);
      const tile = h(
        "button",
        { type: "button", class: "slide-tile", role: "option", draggable: "true" },
        box,
        h("span", { class: "tile-number" }),
      );
      entry = { tile, host, box, text };
      tiles.set(slide.file, entry);
      wireTile(tile, slide.file);
    } else if (entry.text !== text) {
      entry.text = text;
      mountSlide(entry.host, text, state.deck.size, look);
    }
    scaleFrame(entry.box, state.deck.size, TILE_HEIGHT / state.deck.size[1]);
    const tile = entry.tile;
    if (el.strip.children[index] !== tile) el.strip.insertBefore(tile, el.strip.children[index] || null);
    tile.classList.toggle("current", index === state.current);
    tile.classList.toggle("skipped", Boolean(slide.skip));
    tile.setAttribute("aria-selected", String(index === state.current));
    const title = slideTitle(entry.host.shadowRoot) || "Slide " + (index + 1);
    tile.setAttribute("aria-label", `Slide ${index + 1}: ${title}`);
    const number = tile.querySelector(".tile-number");
    number.replaceChildren(String(index + 1), slide.notes ? icon("notes", 11) : "", slide.skip ? icon("hidden", 11) : "");
    decorateTile(tile, slide.file);
  });
  for (const [file, entry] of [...tiles]) {
    if (!seen.has(file)) {
      entry.tile.remove();
      tiles.delete(file);
    }
  }
  el.count.textContent = wanted.length ? `${state.current + 1} / ${wanted.length}` : "";
  const current = tiles.get(store.currentSlide()?.file)?.tile;
  current?.scrollIntoView({ block: "nearest", inline: "nearest" });
  if (view.mode === "grid") drawGrid();
}

// The marks on a tile: how many comments are open on it, and whether
// something on it does not fit.
function decorateTile(tile, file) {
  const open = state.comments.filter((comment) => comment.file === file && !comment.resolved).length;
  let badge = tile.querySelector(".tile-badge");
  if (open && !badge) tile.append((badge = h("span", { class: "tile-badge" })));
  if (badge) {
    badge.textContent = open;
    badge.hidden = !open;
  }
  const issues = state.issues.get(file) || [];
  let mark = tile.querySelector(".tile-issue");
  if (issues.length && !mark) tile.append((mark = h("span", { class: "tile-issue" }, icon("warning", 14))));
  if (mark) {
    mark.hidden = issues.length === 0;
    mark.title = issues.map((issue) => issue.text).join("\n");
  }
}

function wireTile(tile, file) {
  const index = () => state.deck.slides.findIndex((slide) => slide.file === file);
  tile.addEventListener("click", () => {
    store.go(index());
    if (view.mode === "grid") setMode("single");
  });
  tile.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    slideMenu({ x: event.clientX, y: event.clientY, width: 0, height: 0 }, index());
  });
  tile.addEventListener("dragstart", (event) => {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/x-deck-slide", file);
    tile.classList.add("dragging");
  });
  tile.addEventListener("dragend", () => {
    tile.classList.remove("dragging");
    clearDrop();
  });
  tile.addEventListener("dragover", (event) => {
    if (!event.dataTransfer.types.includes("text/x-deck-slide")) return;
    event.preventDefault();
    const box = tile.getBoundingClientRect();
    const before = event.clientX < box.x + box.width / 2;
    clearDrop();
    tile.classList.add(before ? "drop-before" : "drop-after");
  });
  tile.addEventListener("drop", (event) => {
    const moved = event.dataTransfer.getData("text/x-deck-slide");
    if (!moved) return;
    event.preventDefault();
    const before = tile.classList.contains("drop-before");
    clearDrop();
    const from = state.deck.slides.findIndex((slide) => slide.file === moved);
    let to = index() + (before ? 0 : 1);
    if (from < to) to -= 1;
    store.moveSlide(from, to);
  });
}

function clearDrop() {
  for (const tile of document.querySelectorAll(".drop-before, .drop-after")) {
    tile.classList.remove("drop-before", "drop-after");
  }
}

// Every slide laid out, under the names of the deck's parts.
function drawGrid() {
  const parts = [];
  let part = null;
  state.deck.slides.forEach((slide, index) => {
    if (slide.section || !part) {
      part = { name: slide.section || "", slides: [] };
      parts.push(part);
    }
    part.slides.push({ slide, index });
  });
  el.grid.replaceChildren(
    ...parts.flatMap((one) => [
      one.name ? h("div", { class: "grid-section" }, one.name) : null,
      h(
        "div",
        { class: "grid-tiles" },
        one.slides.map(({ slide, index }) => {
          const { box } = thumb(store.textOf(slide.file), 150);
          const tile = h(
            "button",
            { type: "button", class: "slide-tile" + (index === state.current ? " current" : "") + (slide.skip ? " skipped" : ""), draggable: "true", "aria-label": `Slide ${index + 1}` },
            box,
            h("span", { class: "tile-number" }, String(index + 1)),
          );
          wireTile(tile, slide.file);
          decorateTile(tile, slide.file);
          // The picture fills the tile's column.
          requestAnimationFrame(() => {
            const width = tile.parentElement?.clientWidth;
            if (width) scaleFrame(box, state.deck.size, width / state.deck.size[0]);
          });
          return h("div", { class: "grid-tile" }, tile, slide.notes ? h("div", { class: "grid-note" }, slide.notes) : null);
        }),
      ),
    ]).filter(Boolean),
  );
}

// A deck with no slides yet: its look to choose, and how to begin.
function drawEmpty() {
  const chosen = currentLook();
  el.empty.replaceChildren(
    h("h1", {}, "This deck has no slides yet"),
    h(
      "p",
      {},
      "Say in the chat what the deck is about, or give it a file to start from. Pick a look first, or leave it to the agent; either can change later.",
    ),
    h(
      "div",
      { class: "looks" },
      state.kit.looks.map((one) => {
        const sheet = new Look();
        sheet.setBase(look.baseText);
        sheet.set(one.css);
        const host = h("div", { class: "host" });
        const box = h("div", { class: "thumb" }, host);
        const title = state.kit.layouts.find((layout) => layout.id === "title");
        mountSlide(host, sampleTitle(title?.html || ""), state.deck.size, sheet);
        const tile = h(
          "button",
          { type: "button", class: "look", "aria-pressed": String(one.id === chosen) },
          box,
          h("span", { class: "look-name" }, one.name),
          h("span", { class: "look-hint" }, one.hint),
        );
        tile.addEventListener("click", () => store.write([{ path: "deck.css", text: one.css }], `Chose the look ${one.name}`));
        requestAnimationFrame(() => {
          if (tile.clientWidth) scaleFrame(box, state.deck.size, (tile.clientWidth - 14) / state.deck.size[0]);
        });
        return tile;
      }),
    ),
    h(
      "div",
      { class: "empty-actions" },
      button({ label: "Add a slide", icon: "plus", kind: "outline", onClick: () => newSlide(-1) }),
      button({ label: "Start from a PowerPoint file", icon: "file", kind: "outline", onClick: choosePowerPoint }),
    ),
  );
}

const POWERPOINT = /\.(pptx|potx)$/i;

// Asks for a PowerPoint file and makes slides from it.
function choosePowerPoint() {
  const input = h("input", { type: "file", accept: ".pptx,.potx" });
  input.addEventListener("change", () => {
    if (input.files[0]) startFrom(input.files[0]);
  });
  input.click();
}

// Makes slides from a PowerPoint file: they follow the ones the deck has.
async function startFrom(file) {
  const before = state.deck.slides.length;
  // Stays until the slides are made.
  const working = toast("Making slides from " + file.name + "…", { stay: 600000 });
  try {
    await store.startFrom(file);
    working.remove();
    const made = state.deck.slides.length - before;
    if (made > 0) {
      store.go(before);
      toast(made === 1 ? "Made 1 slide from the file." : `Made ${made} slides from the file.`);
    }
  } catch (error) {
    working.remove();
    toast(error.message, { kind: "error" });
  }
}

// A PowerPoint file dropped anywhere on the page makes slides. Any other
// file is the slide's to take (a picture, a video); dropped beside the
// slide it is let go, not opened in the deck's place.
function wireDrops() {
  document.addEventListener("dragover", (event) => {
    if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
  });
  document.addEventListener(
    "drop",
    (event) => {
      const decks = [...(event.dataTransfer?.files || [])].filter((file) => POWERPOINT.test(file.name));
      if (decks.length === 0) return;
      event.preventDefault();
      event.stopPropagation();
      decks.reduce((done, file) => done.then(() => startFrom(file)), Promise.resolve());
    },
    true,
  );
  document.addEventListener("drop", (event) => {
    if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
  });
}

// The title layout with the deck's own name on it, to show a look by.
function sampleTitle(html) {
  const name = (state.deck.title || window.DECK.title || "Presentation title").replace(/[<&]/g, "");
  return html.replace("Presentation title", name);
}

// Which built-in look deck.css is, by the name it carries on top.
function currentLook() {
  const css = store.textOf("deck.css") || "";
  return state.kit.looks.find((one) => css.includes(`look (${one.name})`))?.id || "";
}

// ---- Bars ----------------------------------------------------------------

function buildBars() {
  const insert = (kind, more) => () => editing.insert(kind, more);
  const shapes = button({
    icon: "shapes",
    title: "Shapes",
    caret: true,
    onClick: (_, anchor) =>
      menu(
        anchor,
        SHAPES.flatMap((shape, index) => [
          index === 4 || index === 8 ? { separator: true } : null,
          { label: shape.name, icon: shape.icon, hint: shape.key, onSelect: insert("shape", { shape: shape.id }) },
        ]),
      ),
  });
  shapes.classList.remove("btn-icon");
  el.toolsLeft.replaceChildren(
    h(
      "div",
      { class: "bar", role: "toolbar", "aria-label": "Add to the slide" },
      button({ icon: "text", title: "Text (T)", onClick: insert("text") }),
      button({ icon: "image", title: "Picture", onClick: insert("image") }),
      button({ icon: "video", title: "Video", onClick: insert("video") }),
      button({ icon: "table", title: "Table", onClick: insert("table") }),
      button({ icon: "chart", title: "Chart", onClick: insert("chart") }),
      shapes,
    ),
  );
  el.commentButton = button({
    icon: "comment",
    title: "Comment on something (C)",
    pressed: false,
    onClick: () => setCommenting(!view.commenting),
  });
  const commentMore = button({
    icon: "down",
    size: 12,
    title: "Comments",
    onClick: (_, anchor) => comments.list(anchor),
  });
  commentMore.style.width = "18px";
  el.zoomLabel = h("span", { class: "btn-label" }, "100%");
  const zoom = h("button", { type: "button", class: "btn", title: "Zoom" }, el.zoomLabel, icon("down", 12));
  zoom.addEventListener("click", () => zoomMenu(zoom));
  el.panelButton = button({
    icon: "sliders",
    title: "How it looks",
    pressed: view.panel,
    onClick: () => setPanel(!view.panel),
  });
  el.toolsRight.replaceChildren(
    h(
      "div",
      { class: "bar", role: "toolbar", "aria-label": "The deck" },
      el.commentButton,
      commentMore,
      button({ icon: "share", label: "Share", title: "Share on your network", onClick: () => store.tell({ share: true }) }),
      h("span", { class: "bar-rule" }),
      button({ icon: "download", title: "Export", onClick: (_, anchor) => exportMenu(anchor) }),
      button({ icon: "play", title: "Present", onClick: (_, anchor) => presentMenu(anchor) }),
      h("span", { class: "bar-rule" }),
      el.panelButton,
      zoom,
    ),
  );
  el.count = h("span", { class: "tray-count" });
  el.notesButton = button({ icon: "notes", title: "Speaker notes", pressed: view.notes, onClick: () => setNotes(!view.notes) });
  el.singleButton = button({ icon: "single", title: "One slide", pressed: true, onClick: () => setMode("single") });
  el.gridButton = button({ icon: "grid", title: "All slides", pressed: false, onClick: () => setMode("grid") });
  el.trayButton = button({ icon: "down", title: "Hide the slides", onClick: () => setTray(!view.tray) });
  el.trayBar.replaceChildren(
    button({ icon: "plus", title: "New slide", onClick: () => newSlide() }),
    el.notesButton,
    el.count,
    h("span", { class: "tray-space" }),
    el.singleButton,
    el.gridButton,
    el.trayButton,
  );
}

function zoomMenu(anchor) {
  const to = (scale) => () => {
    view.zoom = scale;
    fitStage();
  };
  const step = (by) => () => {
    view.zoom = Math.max(0.05, Math.min(4, +(view.scale * by).toFixed(3)));
    fitStage();
  };
  menu(
    anchor,
    [
      { label: "Zoom in", hint: "+", onSelect: step(1.25) },
      { label: "Zoom out", hint: "−", onSelect: step(0.8) },
      { label: "Fit the slide", hint: "0", onSelect: to(null), checked: view.zoom === null },
      { separator: true },
      ...[0.25, 0.5, 0.75, 1, 1.5, 2].map((scale) => ({ label: scale * 100 + "%", onSelect: to(scale) })),
      { separator: true },
      { label: "Undo", hint: keyHint("⌘Z"), disabled: !state.canUndo, onSelect: store.undo },
      { label: "Redo", hint: keyHint("⇧⌘Z"), disabled: !state.canRedo, onSelect: store.redo },
    ],
    { place: "end" },
  );
}

function exportMenu(anchor) {
  const ask = (format) => () => {
    if (state.deck.slides.length === 0) {
      toast("There is nothing to export yet.");
      return;
    }
    toast("Exporting…", { stay: 1500 });
    store.tell({ export: format, slide: store.currentSlide()?.file });
  };
  menu(
    anchor,
    [
      { label: "PDF (.pdf)", onSelect: ask("pdf") },
      { label: "Web page (.html)", onSelect: ask("html") },
      { label: "This slide as a picture (.png)", onSelect: ask("png") },
      { separator: true },
      { label: "PowerPoint, these fonts (.pptx)", onSelect: ask("pptx") },
      { label: "PowerPoint, basic fonts (.pptx)", onSelect: ask("pptx-basic") },
      { heading: "Basic fonts are on every computer" },
    ],
    { place: "end" },
  );
}

function presentMenu(anchor) {
  menu(
    anchor,
    [
      { label: "Present", icon: "play", hint: keyHint("⌘↩"), onSelect: () => startPresenting(false) },
      { label: "Present with speaker notes", icon: "notes", onSelect: () => startPresenting(true) },
    ],
    { place: "end" },
  );
}

function startPresenting(notes) {
  if (state.deck.slides.length === 0) return;
  editing.select(null);
  closeFloats();
  present({ look, notes, from: state.current });
}

function slideMenu(anchor, index) {
  const slide = store.slideAt(index);
  if (!slide) return;
  const last = state.deck.slides.length - 1;
  menu(anchor, [
    { label: "Duplicate slide", hint: keyHint("⌘D"), onSelect: () => store.duplicateSlide(index) },
    { label: "Add slide after", onSelect: () => newSlide(index) },
    { separator: true },
    {
      label: slide.skip ? "Show when presenting" : "Skip when presenting",
      onSelect: () =>
        store.setSlide(index, (one) => (one.skip = !one.skip), `${slide.skip ? "Showed" : "Skipped"} slide ${index + 1}`),
    },
    {
      label: "Start a part here…",
      onSelect: () => namePart(index),
    },
    { label: "Use as the deck's picture", onSelect: () => store.tell({ thumbnail: slide.file }) },
    { separator: true },
    { label: "Move left", disabled: index === 0, onSelect: () => store.moveSlide(index, index - 1) },
    { label: "Move right", disabled: index === last, onSelect: () => store.moveSlide(index, index + 1) },
    { separator: true },
    { label: "Delete slide", hint: "⌫", danger: true, onSelect: () => store.removeSlide(index) },
  ]);
}

// Names the part of the deck that starts at a slide (shown where all slides
// are laid out); an empty name ends it.
function namePart(index) {
  const slide = store.slideAt(index);
  const input = h("input", { type: "text", class: "input", value: slide.section || "", placeholder: "Name of this part of the deck", style: { width: "100%" } });
  const save = () => {
    const name = input.value.trim();
    store.setSlide(index, (one) => (one.section = name), name ? `Named a part "${name}"` : "Removed a part's name");
    box.close();
  };
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") save();
  });
  const box = dialog({
    title: "Part of the deck",
    width: 380,
    content: input,
    actions: [button({ label: "Cancel", onClick: () => box.close() }), button({ label: "Save", kind: "primary", onClick: save })],
  });
  input.focus();
}

// *New slide*: the layouts, drawn in this deck's look.
function newSlide(after = state.current) {
  const box = dialog({
    title: "New slide",
    width: 560,
    content: h(
      "div",
      { class: "layouts" },
      state.kit.layouts.map((layout) => {
        const { box: picture } = thumb(layout.html, 138);
        const tile = h("button", { type: "button", class: "layout" }, picture, h("span", {}, layout.name));
        tile.addEventListener("click", () => {
          box.close();
          store.addSlide(layout.html, after, layout.id === "blank" ? "slide" : layout.id);
          setMode("single");
        });
        return tile;
      }),
    ),
  });
  // The pictures fill their columns.
  requestAnimationFrame(() => {
    for (const tile of box.element.querySelectorAll(".layout")) {
      const picture = tile.querySelector(".thumb");
      scaleFrame(picture, state.deck.size, tile.clientWidth / state.deck.size[0]);
    }
  });
  box.element.querySelector(".layout")?.focus();
}

// ---- Switches ------------------------------------------------------------

function setMode(mode) {
  if (view.mode === mode) return;
  view.mode = mode;
  editing.select(null);
  el.singleButton.setAttribute("aria-pressed", String(mode === "single"));
  el.gridButton.setAttribute("aria-pressed", String(mode === "grid"));
  el.toolsLeft.hidden = mode !== "single";
  drawStage();
  if (mode === "grid") drawGrid();
}

function setNotes(shown) {
  view.notes = shown;
  remember("notes", shown ? "on" : "off");
  el.notesButton.setAttribute("aria-pressed", String(shown));
  drawNotes();
  fitStage();
}

function setPanel(shown) {
  view.panel = shown;
  remember("panel", shown ? "on" : "off");
  el.panel.hidden = !shown;
  el.panelButton.setAttribute("aria-pressed", String(shown));
  if (shown) panel.draw();
  fitStage();
}

function setTray(open) {
  view.tray = open;
  remember("tray", open ? "open" : "closed");
  el.tray.classList.toggle("collapsed", !open);
  el.trayButton.replaceChildren(icon(open ? "down" : "up"));
  el.trayButton.title = open ? "Hide the slides" : "Show the slides";
  fitStage();
}

function setCommenting(commenting) {
  view.commenting = commenting;
  el.commentButton.setAttribute("aria-pressed", String(commenting));
  editing.select(null);
  comments.setMode(commenting);
}

// ---- Keys ----------------------------------------------------------------

const typing = (target) =>
  target instanceof Element && (target.closest("input, textarea, select, [contenteditable]") || editing.isTyping());

function keys(event) {
  const command = event.metaKey || event.ctrlKey;
  const key = event.key;
  if (document.querySelector(".scrim, .show-stage, .presenter")) return;
  if (command && (key === "z" || key === "Z")) {
    event.preventDefault();
    // In a box of the page's own, its own undo: asked for here, since in
    // the app's window no menu sends it, as a browser's does.
    const own = event.target instanceof Element && event.target.closest("input, textarea, .notes");
    if (own || editing.isTyping()) document.execCommand(event.shiftKey ? "redo" : "undo");
    else if (event.shiftKey) store.redo();
    else store.undo();
    return;
  }
  if (command && key === "Enter") {
    event.preventDefault();
    startPresenting(false);
    return;
  }
  if (typing(event.target)) return;
  if (editing.key(event)) return;
  if (command && (key === "d" || key === "D")) {
    event.preventDefault();
    store.duplicateSlide(state.current);
    return;
  }
  if (command || event.altKey) return;
  const shape = SHAPES.find((one) => one.code === (event.shiftKey ? "⇧" : "") + key.toUpperCase());
  if (view.mode === "single" && shape) {
    event.preventDefault();
    editing.insert("shape", { shape: shape.id });
  } else if (view.mode === "single" && (key === "t" || key === "T")) {
    event.preventDefault();
    editing.insert("text");
  } else if (key === "c" || key === "C") {
    setCommenting(!view.commenting);
  } else if (view.mode === "single" && (key === "+" || key === "=" || key === "-" || key === "0")) {
    // The slide larger, smaller, fitted. Without ⌘ or Ctrl: with one,
    // these size the whole app (or the browser's page).
    event.preventDefault();
    if (key === "0") view.zoom = null;
    else view.zoom = Math.max(0.05, Math.min(4, +(view.scale * (key === "-" ? 0.8 : 1.25)).toFixed(3)));
    fitStage();
  } else if (key === "ArrowRight" || key === "ArrowDown" || key === "PageDown") {
    event.preventDefault();
    store.go(state.current + 1);
  } else if (key === "ArrowLeft" || key === "ArrowUp" || key === "PageUp") {
    event.preventDefault();
    store.go(state.current - 1);
  } else if (key === "Home") store.go(0);
  else if (key === "End") store.go(state.deck.slides.length - 1);
  else if ((key === "Backspace" || key === "Delete") && document.activeElement?.closest?.(".slide-tile")) {
    store.removeSlide(state.current);
  } else if (key === "Escape" && view.commenting) setCommenting(false);
}

// ---- Start ---------------------------------------------------------------

async function start() {
  build();
  look.setBase(await fetch("/_artifact/slides/app/slide.css").then((r) => r.text()));
  const env = {
    el,
    look,
    view,
    stage,
    commit,
    setPanel,
    comment: (element) => comments.compose(element),
  };
  editing = new Editing(env);
  panel = new Panel({ ...env, editing, drawStage });
  comments = new Comments({ ...env, editing });
  checks = new Checks({ tiles, stage });
  buildBars();
  setTray(view.tray);
  el.panel.hidden = !view.panel;

  on("deck", ({ files }) => {
    if (files.includes("deck.css") || look.deckText === "") look.set(store.textOf("deck.css") || "");
    if (files.includes("deck.css")) {
      // Charts and fitted text are drawn from the look: once more.
      for (const entry of tiles.values()) dress(entry.host.shadowRoot.querySelector(".slide"));
      if (stage.slide) dress(stage.slide);
    }
    drawStage();
    drawStrip();
    if (view.panel) panel.draw();
    checks.soon();
  });
  on("current", () => {
    editing.select(null);
    drawStage();
    drawStrip();
    if (view.panel) panel.draw();
  });
  on("comments", () => {
    for (const [file, entry] of tiles) decorateTile(entry.tile, file);
    comments.refresh();
  });
  on("issues", () => {
    for (const [file, entry] of tiles) decorateTile(entry.tile, file);
  });
  on("selection", () => {
    if (view.panel) panel.draw();
  });
  on("history", () => {});
  on("failed", (error) => toast(`That change was not saved: ${error.message}`, { kind: "error", stay: 7000 }));
  on("command", (command) => run(command));

  el.notes.addEventListener("input", () => {
    const text = el.notes.textContent;
    const index = state.current;
    store.setSlide(index, (slide) => (slide.notes = text), `Edited the notes of slide ${index + 1}`);
  });
  el.notes.addEventListener("keydown", (event) => {
    if (event.key === "Escape") el.notes.blur();
  });
  new ResizeObserver(() => fitStage()).observe(el.stage);
  el.stage.addEventListener("scroll", () => {
    editing.refresh();
    comments.refresh();
  });
  document.addEventListener("keydown", keys);
  // The page never leaves itself: a file dropped beside the slide is not
  // opened in its place.
  for (const type of ["dragover", "drop"]) {
    window.addEventListener(type, (event) => {
      if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
    });
  }
  window.addEventListener("error", (event) => store.tell({ error: String(event.message), at: `${event.filename}:${event.lineno}` }));
  window.addEventListener("unhandledrejection", (event) => store.tell({ error: String(event.reason?.message || event.reason) }));

  // For the app's own tests of the page, which drive it from outside.
  window.deckEditor = { state, store, editing, view, comments, checks };

  await store.load();
  wireDrops();
  if (window.DECK.embedded) {
    // In the app's window the page says when it takes the keyboard, so
    // that the chat beside it lets go of it.
    window.addEventListener("focus", () => store.tell({ focus: true }));
    if (document.hasFocus()) store.tell({ focus: true });
  }
  store.listen();
  store.tell({ opened: true });
}

// What the app asks of the page.
async function run(command) {
  if (command.toast) toast(command.toast, { kind: command.error ? "error" : "", stay: command.error ? 7000 : 4200 });
  if (typeof command.go === "string") {
    const at = state.deck.slides.findIndex((slide) => slide.file === command.go);
    if (at >= 0) store.go(at);
  }
  if (command.measure) {
    // For an export: where everything on every slide is, as drawn here.
    const measured = await measureDeck({ look, deck: state.deck, textOf: store.textOf, title: window.DECK.title }).catch((error) => ({
      error: String(error.message || error),
    }));
    store.tell({ measured, for: command.measure });
  }
  if (command.check) {
    // What an agent just wrote is read first, then measured.
    await store.refresh();
    checks.now();
  }
}

start().catch((error) => {
  console.error(error);
  document.getElementById("app").replaceChildren(
    h("div", { class: "banner", style: { position: "static", transform: "none", margin: "40px auto" } }, "The deck could not be opened: " + error.message),
  );
});
