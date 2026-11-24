// The board's controls: the tools down its left edge, the bar on top, the
// bar over what is selected, the menus, and the notes people pin to the
// board for each other and for the agent.

import { h, button, menu, popover, icon, toast, closeFloats, keys as keyHint } from "/_artifact/kit/ui.js";
import { state, elements, selected, select, setTool, on, emit, change, undo, redo, tell, newId, byId, sendPicture } from "./model.js";
import { el, toScreen, edit, viewSize } from "./draw.js";
import { COLOR_NAMES, soft, strong, SIZES, SHAPES, box, union } from "./shapes.js";
import { zoomBy, zoomTo, fit, show, removeSelected, duplicate, add, middle, addPictures } from "./tools.js";
import { STICKERS, openTemplates, Timer, toggleMap, walk, outlineText, picture } from "./extras.js";

const ui = {};

// ---- The tools, down the left edge -------------------------------------------

const TOOLS = [
  { id: "select", icon: "cursor", name: "Select", key: "V" },
  { id: "hand", icon: "hand", name: "Move the board", key: "H" },
  null,
  { id: "sticky", icon: "sticky", name: "Note", key: "S", more: stickyMore },
  { id: "text", icon: "text", name: "Text", key: "T" },
  { id: "shape", icon: "shapes", name: "Shape", key: "R", more: shapeMore },
  { id: "line", icon: "arrow", name: "Arrow", key: "L", more: lineMore },
  { id: "pen", icon: "pen", name: "Pen", key: "P", more: penMore },
  { id: "frame", icon: "frame", name: "Section", key: "F" },
  { id: "table", icon: "table", name: "Table" },
  { id: "sticker", icon: "sticker", name: "Sticker", key: "K", more: stickerMore },
  { id: "image", icon: "image", name: "Picture", act: choosePicture },
];

const HINTS = {
  sticky: "Click where the note goes, or drag out its size.",
  text: "Click where the words go.",
  shape: "Click for a shape, or drag out its size.",
  line: "Drag from one thing to another; ends dropped on a thing stay tied to it.",
  pen: "Draw. Press V (or Esc) when done.",
  marker: "Highlight. Press V (or Esc) when done.",
  eraser: "Rub over the pen's lines to remove them.",
  frame: "Drag out a section around things that belong together.",
  table: "Click for a table, or drag out its size.",
  sticker: "Click where the sticker goes.",
  comment: "Click the thing, or the spot, the note is about.",
  hand: "Drag to move over the board. (Holding Space does the same with any tool.)",
};

function buildDock() {
  ui.dock = h("div", { class: "dock", role: "toolbar", "aria-label": "Tools", "aria-orientation": "vertical" });
  ui.toolButtons = new Map();
  for (const tool of TOOLS) {
    if (!tool) {
      ui.dock.append(h("span", { class: "dock-rule" }));
      continue;
    }
    const one = button({
      icon: tool.icon,
      title: tool.key ? `${tool.name} (${tool.key})` : tool.name,
      pressed: false,
      size: 20,
      onClick: (_, anchor) => {
        if (tool.act) return tool.act();
        // A second press on the tool in hand shows what it can make.
        if (state.tool === tool.id && tool.more) return tool.more(anchor);
        setTool(tool.id);
        if (tool.more && tool.id !== "sticky" && tool.id !== "line") tool.more(anchor);
      },
    });
    if (tool.more) one.classList.add("has-more");
    ui.toolButtons.set(tool.id, one);
    ui.dock.append(one);
  }
  return ui.dock;
}

function drawTool() {
  // The pen's three ways share its place in the dock.
  const shown = ["marker", "eraser"].includes(state.tool) ? "pen" : state.tool;
  for (const [id, one] of ui.toolButtons) one.setAttribute("aria-pressed", String(id === shown));
  const pen = ui.toolButtons.get("pen");
  pen.replaceChildren(icon(state.tool === "marker" ? "marker" : state.tool === "eraser" ? "eraser" : "pen", 20));
  el.canvas.dataset.tool = state.tool;
  ui.hint.textContent = HINTS[state.tool] || "";
  ui.hint.hidden = !HINTS[state.tool];
  if (state.tool === "select") closeFloats();
}

const swatches = (names, picked, onPick, { tone = soft, none = false } = {}) =>
  h(
    "div",
    { class: "chips" },
    names.map((name) =>
      h("button", {
        type: "button",
        class: "chip-pick" + (name === picked ? " picked" : ""),
        title: name[0].toUpperCase() + name.slice(1),
        "aria-label": name,
        style: { background: name === "ink" ? "var(--ui-fg)" : tone(name) },
        onclick: () => onPick(name),
      }),
    ),
    none ? h("button", { type: "button", class: "chip-pick none" + (picked === "none" ? " picked" : ""), title: "None", "aria-label": "None", onclick: () => onPick("none") }, icon("none", 14)) : null,
  );

function stickyMore(anchor) {
  const float = popover(anchor, swatches(COLOR_NAMES, state.last.sticky, (name) => ((state.last.sticky = name), float.close(), setTool("sticky"))), { place: "right", className: "popover-chips" });
}

function shapeMore(anchor) {
  const float = popover(
    anchor,
    h(
      "div",
      { class: "shape-picks" },
      SHAPES.map((shape) =>
        button({
          icon: shape.id,
          title: shape.name,
          pressed: state.last.shape === shape.id,
          size: 20,
          onClick: () => {
            state.last.shape = shape.id;
            float.close();
            setTool("shape");
          },
        }),
      ),
    ),
    { place: "right", className: "popover-chips" },
  );
}

function lineMore(anchor) {
  const routes = [["curve", "routeCurve", "Curved"], ["straight", "routeStraight", "Straight"], ["elbow", "routeElbow", "Right angles"]];
  const float = popover(
    anchor,
    h(
      "div",
      { class: "pick-rows" },
      h("div", { class: "pick-row" }, routes.map(([id, name, title]) => button({ icon: name, title, pressed: (state.last.route || "curve") === id, onClick: () => ((state.last.route = id), float.close(), setTool("line")) }))),
      swatches(["ink", ...COLOR_NAMES], state.last.line, (name) => ((state.last.line = name), float.close(), setTool("line")), { tone: strong }),
    ),
    { place: "right", className: "popover-chips" },
  );
}

function penMore(anchor) {
  const way = (id, name, title) => button({ icon: name, title, pressed: state.tool === id, size: 20, onClick: () => (float.close(), setTool(id)) });
  const float = popover(
    anchor,
    h(
      "div",
      { class: "pick-rows" },
      h("div", { class: "pick-row" }, way("pen", "pen", "Pen (P)"), way("marker", "marker", "Highlighter (M)"), way("eraser", "eraser", "Eraser (E)")),
      h(
        "div",
        { class: "pick-row" },
        [2, 3, 6, 10].map((width) =>
          h("button", { type: "button", class: "width-pick" + (state.last.penWidth === width ? " picked" : ""), title: `${width} points`, onclick: () => ((state.last.penWidth = width), float.close(), setTool("pen")) }, h("i", { style: { height: width + "px" } })),
        ),
      ),
      swatches(["ink", ...COLOR_NAMES], state.last.pen, (name) => ((state.last.pen = name), (state.last.marker = name === "ink" ? "yellow" : name), float.close(), setTool(state.tool === "marker" ? "marker" : "pen")), { tone: strong }),
    ),
    { place: "right", className: "popover-chips" },
  );
}

function stickerMore(anchor, onPick) {
  const pick = (emoji) => {
    float.close();
    if (onPick) return onPick(emoji);
    state.last.emoji = emoji;
    setTool("sticker");
  };
  const any = h("input", { type: "text", class: "input", placeholder: "Find one, or type any emoji", "aria-label": "Find a sticker", style: { width: "100%" } });
  const grid = h("div", { class: "sticker-grid" });
  const fill = () => {
    const query = any.value.trim().toLowerCase();
    const found = STICKERS.filter(([, words]) => !query || words.includes(query));
    grid.replaceChildren(...found.map(([emoji, words]) => h("button", { type: "button", class: "sticker-pick", title: words.split(" ")[0], onclick: () => pick(emoji) }, emoji)));
    // What is typed is itself a sticker, when it is no word.
    if (query && found.length === 0 && /\p{Extended_Pictographic}/u.test(any.value)) grid.append(h("button", { type: "button", class: "sticker-pick", onclick: () => pick(any.value.trim()) }, any.value.trim()));
  };
  any.addEventListener("input", fill);
  any.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Enter") grid.firstChild?.click();
    if (event.key === "Escape") float.close();
  });
  fill();
  const float = popover(anchor, h("div", { class: "sticker-box" }, any, grid), { place: "right" });
  any.focus();
}

function choosePicture() {
  const input = h("input", { type: "file", accept: "image/*", multiple: true });
  input.addEventListener("change", () => input.files.length && addPictures([...input.files], middle()));
  input.click();
}

// ---- The bar on top --------------------------------------------------------------

let timer;

function buildTop() {
  ui.undo = button({ icon: "undo", title: `Undo (${keyHint("⌘Z")})`, disabled: true, onClick: undo });
  ui.redo = button({ icon: "redo", title: `Redo (${keyHint("⇧⌘Z")})`, disabled: true, onClick: redo });
  ui.comment = button({ icon: "comment", title: "Pin a note for someone, or for the agent (C)", pressed: false, onClick: () => setTool(state.tool === "comment" ? "select" : "comment") });
  const notes = button({ icon: "down", size: 12, title: "The notes on this board", onClick: (_, anchor) => listComments(anchor) });
  notes.classList.add("btn-caret");
  timer = new Timer();
  ui.top = h(
    "div",
    { class: "topbar" },
    h("div", { class: "top-group" }, ui.undo, ui.redo, h("span", { class: "top-rule" }), button({ icon: "template", label: "Templates", title: "Start from a layout: a flow, a retrospective, a plan…", onClick: () => openTemplates() })),
    h("span", { class: "top-space" }),
    h(
      "div",
      { class: "top-group" },
      timer.button,
      button({ icon: "present", title: "Walk through the sections, one at a time", onClick: () => walk() }),
      h("span", { class: "split" }, ui.comment, notes),
      button({ icon: "share", title: "Share on your network", onClick: () => tell({ share: true }) }),
      button({ icon: "download", title: "Export", caret: true, onClick: (_, anchor) => exportMenu(anchor) }),
      // Beside a chat the board is looked at; its own window is where it
      // is changed by hand.
      state.preview ? button({ icon: "open", label: "Open in Whiteboard", kind: "primary", title: "Open the board in its own window: change it by hand, pin notes, share", onClick: () => tell({ open: true }) }) : null,
    ),
  );
  if (!state.owner) ui.top.querySelector(".top-group").hidden = true;
  return ui.top;
}

function exportMenu(anchor) {
  menu(
    anchor,
    [
      {
        label: "Picture of the whole board (.png)",
        // The app keeps its owner's picture with the board; a guest's
        // browser takes its own.
        onSelect: () =>
          picture(2400, 1500)
            .then((blob) => (window.BOARD?.owner ? sendPicture("export", blob) : download(blob)))
            .catch((error) => toast(error.message, { kind: "error" })),
      },
      {
        label: "Copy as an outline (text)",
        onSelect: () => navigator.clipboard?.writeText(outlineText()).then(() => toast("Copied: the sections with their notes, as a list.")),
      },
    ],
    { place: "end" },
  );
}

function download(blob) {
  const link = h("a", { href: URL.createObjectURL(blob), download: (state.board.title || window.BOARD?.title || "Board") + ".png" });
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 30000);
}

// ---- The corner: how the board is looked at ------------------------------------------

function buildCorner() {
  ui.zoom = h("button", { type: "button", class: "btn zoom-value", title: "Zoom" }, "100%");
  ui.zoom.addEventListener("click", () =>
    menu(
      ui.zoom,
      [
        { label: "Fit the board", hint: "⇧1", onSelect: fit },
        { label: "Zoom to what is selected", hint: "⇧2", disabled: state.selection.size === 0, onSelect: () => show(union(selected())) },
        { separator: true },
        ...[0.25, 0.5, 1, 1.5, 2].map((scale) => ({ label: scale * 100 + "%", onSelect: () => zoomTo(scale), checked: Math.abs(state.view.scale - scale) < 0.005 })),
      ],
      { place: "above" },
    ),
  );
  ui.corner = h(
    "div",
    { class: "corner bar" },
    button({ icon: "map", title: "A small map of the board", onClick: (_, anchor) => anchor.setAttribute("aria-pressed", String(toggleMap())), pressed: false }),
    h("span", { class: "bar-rule" }),
    button({ icon: "minus", title: "Zoom out (−)", onClick: () => zoomBy(0.8) }),
    ui.zoom,
    button({ icon: "plus", title: "Zoom in (+)", onClick: () => zoomBy(1.25) }),
    button({ icon: "fit", title: "Fit the board (⇧1)", onClick: fit }),
  );
  return ui.corner;
}

// ---- The bar over what is selected -------------------------------------------------------

const apply = (kind, edit) => change(kind, () => selected().filter((element) => !element.locked).forEach(edit));
const all = (list, test) => list.length > 0 && list.every(test);
const WORDS = (element) => ["sticky", "shape", "text"].includes(element.type);

function chipButton(colour, title, onClick) {
  const one = h("button", { type: "button", class: "btn btn-icon", title, "aria-label": title }, h("i", { class: "chip-now" + (colour === "none" ? " none" : ""), style: { background: colour === "none" ? "" : colour } }));
  one.addEventListener("click", () => onClick(one));
  return one;
}

function selectionBar() {
  const list = selected();
  if (list.length === 0 || !state.owner || state.editing) return null;
  const one = list.length === 1 ? list[0] : null;
  const locked = all(list, (element) => element.locked);
  const parts = [];
  const rule = () => parts.length && parts.push(h("span", { class: "bar-rule" }));
  const toggle = (name, key, title) =>
    button({ icon: name, title, pressed: all(list.filter(WORDS), (element) => element[key]), onClick: () => {
      const on = !all(list.filter(WORDS), (element) => element[key]);
      apply("", (element) => WORDS(element) && (on ? (element[key] = true) : delete element[key]));
    } });

  if (!locked) {
    if (one?.type === "shape") {
      parts.push(
        button({
          icon: one.shape || "rounded",
          title: "Shape",
          caret: true,
          onClick: (_, anchor) => {
            const float = popover(anchor, h("div", { class: "shape-picks" }, SHAPES.map((shape) => button({ icon: shape.id, title: shape.name, size: 20, pressed: one.shape === shape.id, onClick: () => (float.close(), apply("", (element) => (element.shape = shape.id))) }))), { className: "popover-chips" });
          },
        }),
      );
    }
    // Colour: a note's, a shape's fill, a section's, a line's, the pen's.
    const fills = list.filter((element) => ["sticky", "shape", "frame"].includes(element.type));
    if (fills.length) {
      const now = fills[0].type === "shape" ? fills[0].fill : fills[0].color;
      parts.push(
        chipButton(now === "none" ? "none" : soft(now), "Colour", (anchor) => {
          const float = popover(
            anchor,
            swatches(COLOR_NAMES, now, (name) => (float.close(), (state.last.fill = name === "none" ? state.last.fill : name), apply("", (element) => (element.type === "shape" ? (element.fill = name) : ["sticky", "frame"].includes(element.type) && name !== "none" && (element.color = name)))), { none: fills.some((element) => element.type === "shape") }),
            { className: "popover-chips" },
          );
        }),
      );
    }
    const strokes = list.filter((element) => ["line", "draw", "text"].includes(element.type));
    if (strokes.length && fills.length === 0) {
      const now = strokes[0].color || "ink";
      parts.push(
        chipButton(strong(now), "Colour", (anchor) => {
          const float = popover(anchor, swatches(["ink", ...COLOR_NAMES], now, (name) => (float.close(), apply("", (element) => ["line", "draw", "text"].includes(element.type) && (element.color = name))), { tone: strong }), { className: "popover-chips" });
        }),
      );
    }
    if (one?.type === "shape") {
      const now = one.stroke || "none";
      const outline = button({ icon: one.dash ? "dashed" : "solid", title: "Outline", caret: true, onClick: (_, anchor) => {
        const float = popover(
          anchor,
          h(
            "div",
            { class: "pick-rows" },
            h(
              "div",
              { class: "pick-row" },
              button({ icon: "solid", title: "Solid", pressed: !one.dash, onClick: () => (float.close(), apply("", (element) => delete element.dash)) }),
              button({ icon: "dashed", title: "Dashed", pressed: Boolean(one.dash), onClick: () => (float.close(), apply("", (element) => ((element.dash = true), element.stroke === "none" || !element.stroke ? (element.stroke = "ink") : 0))) }),
            ),
            swatches(["ink", ...COLOR_NAMES], now, (name) => (float.close(), apply("", (element) => (element.stroke = name))), { tone: strong, none: true }),
          ),
          { className: "popover-chips" },
        );
      } });
      parts.push(outline);
    }
    if (list.some(WORDS)) {
      rule();
      const size = list.find(WORDS).size || "m";
      parts.push(
        button({
          icon: "textSize",
          label: SIZES[size]?.[1] || "Medium",
          title: "Text size",
          caret: true,
          onClick: (_, anchor) => menu(anchor, Object.entries(SIZES).map(([id, [points, name]]) => ({ label: name, hint: points + " pt", checked: id === size, onSelect: () => apply("", (element) => WORDS(element) && (element.size = id)) }))),
        }),
        toggle("bold", "bold", "Bold"),
        toggle("italic", "italic", "Italic"),
        toggle("list", "list", "A list: each line a point"),
        button({
          icon: { center: "alignCenter", right: "alignRight" }[list.find(WORDS).align] || "alignLeft",
          title: "Align the words",
          onClick: (_, anchor) => {
            const float = popover(anchor, h("div", { class: "pick-row" }, [["left", "alignLeft"], ["center", "alignCenter"], ["right", "alignRight"]].map(([id, name]) => button({ icon: name, title: id, onClick: () => (float.close(), apply("", (element) => WORDS(element) && (element.align = id))) }))), { className: "popover-chips" });
          },
        }),
      );
    }
    if (one?.type === "line") {
      rule();
      const caps = [["none", "capNone", "Plain"], ["arrow", "capArrow", "Arrow"], ["dot", "capDot", "Dot"]];
      const cap = (key, fallback, title, flip) =>
        button({
          // (A value the file made up shows as the usual one.)
          icon: (caps.find(([id]) => id === (one[key] ?? fallback)) || caps.find(([id]) => id === fallback))[1],
          title,
          onClick: (_, anchor) => {
            const float = popover(anchor, h("div", { class: "pick-row" }, caps.map(([id, name, label]) => button({ icon: name, title: label, pressed: (one[key] ?? fallback) === id, onClick: () => (float.close(), apply("", (element) => (element[key] = id))) }))), { className: "popover-chips" + (flip ? " flip" : "") });
          },
        });
      const start = cap("start", "none", "How it starts", true);
      start.classList.add("flip");
      const routes = [["curve", "routeCurve", "Curved"], ["straight", "routeStraight", "Straight"], ["elbow", "routeElbow", "Right angles"]];
      parts.push(
        start,
        button({
          icon: (routes.find(([id]) => id === one.route) || routes[1])[1],
          title: "How it runs",
          onClick: (_, anchor) => {
            const float = popover(anchor, h("div", { class: "pick-row" }, routes.map(([id, name, label]) => button({ icon: name, title: label, pressed: (one.route || "straight") === id, onClick: () => (float.close(), (state.last.route = id), apply("", (element) => (element.route = id))) }))), { className: "popover-chips" });
          },
        }),
        cap("end", "arrow", "How it ends"),
        button({ icon: one.dash ? "dashed" : "solid", title: one.dash ? "Dashed: make it solid" : "Solid: make it dashed", onClick: () => apply("", (element) => (element.dash ? delete element.dash : (element.dash = true))) }),
        button({ icon: "text", title: "Write on it", onClick: () => edit(one.id) }),
      );
    }
    if (one?.type === "table") {
      rule();
      const rows = one.rows || [[""]];
      const columns = Math.max(...rows.map((row) => row.length));
      const grow = (label, title, by) => h("button", { type: "button", class: "btn", title, onclick: () => apply("", by) }, label);
      parts.push(
        grow("+ Row", "Add a row", (element) => (element.rows.push(Array(columns).fill("")), (element.h += 44))),
        grow("+ Column", "Add a column", (element) => (element.rows.forEach((row) => row.push("")), (element.w += 140))),
        grow("−", "Remove the last row", (element) => element.rows.length > 1 && (element.rows.pop(), (element.h = Math.max(44, element.h - 44)))),
        grow("⊟", "Remove the last column", (element) => columns > 1 && (element.rows.forEach((row) => (row.length = columns - 1)), (element.w = Math.max(140, element.w - 140)))),
      );
    }
    if (one?.type === "sticker") parts.push(button({ icon: "sticker", title: "Another sticker", onClick: (_, anchor) => stickerMore(anchor, (emoji) => apply("", (element) => (element.emoji = emoji))) }));
    if (all(list, (element) => element.type === "sticky")) {
      rule();
      // A dot each: how a group chooses between notes.
      parts.push(button({ icon: "vote", title: "Vote for it (a dot more; Shift-click takes one off)", onClick: (event) => apply("vote", (element) => (element.votes = Math.max(0, (element.votes || 0) + (event.shiftKey ? -1 : 1)) || undefined)) }));
    }
    if (list.length > 1) {
      rule();
      parts.push(
        button({ icon: "alignL", title: "Line them up", caret: true, onClick: (_, anchor) => alignMenu(anchor) }),
        button({ icon: "tidy", title: "Tidy up: in rows, evenly apart", onClick: () => tidy() }),
      );
    }
  }
  rule();
  parts.push(button({ icon: locked ? "lock" : "unlock", title: locked ? "Locked in place: unlock" : `Lock in place (${keyHint("⇧⌘L")})`, pressed: locked, onClick: () => act("lock") }), button({ icon: "more", title: "More", onClick: (_, anchor) => moreMenu(anchor) }));
  return h("div", { class: "bar selection-bar", role: "toolbar", "aria-label": "What is selected" }, parts);
}

function alignMenu(anchor) {
  const float = popover(
    anchor,
    h(
      "div",
      { class: "pick-rows" },
      h("div", { class: "pick-row" }, [["alignL", "Left edges", "l"], ["alignC", "Middles, across", "c"], ["alignR", "Right edges", "r"], ["alignT", "Top edges", "t"], ["alignM", "Middles, down", "m"], ["alignB", "Bottom edges", "b"]].map(([name, title, way]) => button({ icon: name, title, onClick: () => (float.close(), align(way)) }))),
      h("div", { class: "pick-row" }, button({ icon: "spreadH", label: "Evenly across", onClick: () => (float.close(), spread("x")) }), button({ icon: "spreadV", label: "Evenly down", onClick: () => (float.close(), spread("y")) })),
    ),
    { className: "popover-chips" },
  );
}

const solid = (element) => element.type !== "line" && element.type !== "draw" && !element.locked;

function align(way) {
  const list = selected().filter(solid);
  const u = union(list);
  change("", () => {
    for (const element of list) {
      const b = box(element);
      if (way === "l") element.x = u.x;
      if (way === "c") element.x = Math.round(u.x + (u.w - b.w) / 2);
      if (way === "r") element.x = u.x + u.w - b.w;
      if (way === "t") element.y = u.y;
      if (way === "m") element.y = Math.round(u.y + (u.h - b.h) / 2);
      if (way === "b") element.y = u.y + u.h - b.h;
    }
  });
}

function spread(axis) {
  const size = axis === "x" ? "w" : "h";
  const list = selected().filter(solid).sort((a, b) => a[axis] - b[axis]);
  if (list.length < 3) return toast("Pick three or more to space them evenly.");
  const u = union(list);
  const gap = (u[size] - list.reduce((sum, element) => sum + element[size], 0)) / (list.length - 1);
  change("", () => {
    let at = u[axis];
    for (const element of list) {
      element[axis] = Math.round(at);
      at += element[size] + gap;
    }
  });
}

// In rows, in reading order, evenly apart: what a pile of notes becomes.
function tidy() {
  const list = selected().filter(solid).filter((element) => element.type !== "frame");
  if (list.length < 2) return;
  const u = union(list);
  const ordered = [...list].sort((a, b) => (Math.abs(a.y - b.y) > Math.max(a.h, b.h) / 2 ? a.y - b.y : a.x - b.x));
  const across = Math.max(1, Math.round(Math.sqrt(ordered.length * Math.max(0.6, Math.min(2.4, u.w / Math.max(u.h, 1))))));
  const gap = 20;
  change("", () => {
    let y = u.y;
    for (let start = 0; start < ordered.length; start += across) {
      const row = ordered.slice(start, start + across);
      let x = u.x;
      for (const element of row) {
        element.x = Math.round(x);
        element.y = Math.round(y);
        x += element.w + gap;
      }
      y += Math.max(...row.map((element) => element.h)) + gap;
    }
  });
}

// What the keys and the menus do to what is selected.
function act(what) {
  const list = selected();
  if (list.length === 0) return;
  const ids = new Set(list.map((element) => element.id));
  if (what === "lock") {
    const lock = !all(list, (element) => element.locked);
    change("", () => list.forEach((element) => (lock ? (element.locked = true) : delete element.locked)));
  } else if (what === "front" || what === "back") {
    change("", () => {
      const rest = elements().filter((element) => !ids.has(element.id));
      // Sections stay under what they hold.
      const frames = rest.filter((element) => element.type === "frame").length;
      state.board.elements = what === "front" ? [...rest, ...list] : [...rest.slice(0, frames), ...list, ...rest.slice(frames)];
    });
  } else if (what === "group") {
    if (list.length < 2) return;
    const group = newId("g");
    change("", () => list.forEach((element) => (element.group = group)));
  } else if (what === "ungroup") {
    change("", () => list.forEach((element) => delete element.group));
  }
  emit("selection");
}

function moreMenu(anchor) {
  const list = selected();
  const locked = all(list, (element) => element.locked);
  const grouped = list.some((element) => element.group);
  menu(anchor, [
    { label: locked ? "Unlock" : "Lock in place", hint: keyHint("⇧⌘L"), onSelect: () => act("lock") },
    { separator: true },
    { label: "Duplicate", hint: keyHint("⌘D"), disabled: locked, onSelect: duplicate },
    { label: "Bring to the front", hint: "]", onSelect: () => act("front") },
    { label: "Send to the back", hint: "[", onSelect: () => act("back") },
    { label: "Group", hint: keyHint("⌘G"), disabled: list.length < 2, onSelect: () => act("group") },
    { label: "Ungroup", hint: keyHint("⇧⌘G"), disabled: !grouped, onSelect: () => act("ungroup") },
    list.some((element) => element.votes) ? { label: "Clear the votes", onSelect: () => apply("", (element) => delete element.votes) } : null,
    { separator: true },
    { label: "Pin a note here…", onSelect: () => compose({ at: { x: union(list).x + union(list).w, y: union(list).y }, on: list[0] }) },
    { label: "Zoom to it", hint: "⇧2", onSelect: () => show(union(list)) },
    { separator: true },
    { label: "Delete", hint: "⌫", danger: true, disabled: locked, onSelect: removeSelected },
  ]);
}

function boardMenu(anchor) {
  menu(anchor, [
    { label: "Select everything", hint: keyHint("⌘A"), onSelect: () => select(elements().map((element) => element.id)) },
    { label: "Fit the board", hint: "⇧1", onSelect: fit },
    { separator: true },
    { label: "Pin a note here…", onSelect: () => compose({ at: anchor.board }) },
    { label: "Templates…", onSelect: () => openTemplates() },
  ]);
}

function placeBar() {
  if (!ui.bar) return;
  const list = selected();
  if (list.length === 0) return;
  const b = union(list);
  const top = toScreen(b);
  const size = viewSize();
  const mine = ui.bar.getBoundingClientRect();
  const width = b.w * state.view.scale;
  let left = top.x + width / 2 - mine.width / 2;
  let y = top.y - mine.height - 30;
  // No room above: under it.
  if (y < 8) y = Math.min(size.h - mine.height - 8, top.y + b.h * state.view.scale + 30);
  left = Math.max(64, Math.min(left, size.w - mine.width - 8));
  ui.bar.style.left = left + "px";
  ui.bar.style.top = Math.max(8, y) + "px";
}

function drawBar() {
  ui.bar?.remove();
  ui.bar = selectionBar();
  if (ui.bar) {
    el.canvas.parentElement.append(ui.bar);
    placeBar();
  }
}

// ---- Notes pinned to the board ----------------------------------------------------------

// Who writes a note: the name this visitor goes by on shared pages, when
// they chose one.
const me = () => window.artifact?.me?.name || "You";
const when = () => new Date().toISOString();

// A new note, about element `on` or the spot `at`.
function compose({ at, on: about, x, y }) {
  const box_ = h("textarea", { class: "input", rows: 3, placeholder: "What should be said, changed or looked at?", style: { width: "280px" } });
  const toAgent = h("input", { type: "checkbox", checked: true });
  const send = () => {
    const text = box_.value.trim();
    if (!text) return;
    const comment = { id: newId("c"), type: "comment", x: Math.round(at.x), y: Math.round(at.y), text, by: me(), at: when(), replies: [] };
    if (about) comment.on = about.id;
    change("", () => elements().push(comment));
    float.close();
    if (toAgent.checked) ask(comment);
  };
  box_.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) send();
    if (event.key === "Escape") float.close();
  });
  const screen = x === undefined ? toScreen(at) : null;
  const rect = el.canvas.getBoundingClientRect();
  const anchor = { x: x ?? rect.left + screen.x, y: y ?? rect.top + screen.y, width: 0, height: 0 };
  const float = popover(
    anchor,
    h("div", { class: "thread" }, about ? h("div", { class: "thread-about" }, "About " + label(about)) : null, box_, h("div", { class: "thread-foot" }, h("label", { class: "check" }, toAgent, "Send to the agent"), button({ label: "Pin it", kind: "primary", onClick: send }))),
  );
  // After the press that opened it, which gives the board the keys.
  setTimeout(() => box_.focus());
}

const label = (element) => {
  const said = (element.text || element.title || element.emoji || "").split("\n")[0].slice(0, 40);
  return said ? `“${said}”` : `the ${element.type}`;
};

// Sends a note to the agent: a message in the chat that says what it is
// about and how to answer it.
function ask(comment) {
  const last = comment.replies?.length ? comment.replies[comment.replies.length - 1].text : comment.text;
  const about = comment.on && byId(comment.on);
  tell({ ask: { id: comment.id, text: last, on: comment.on || null, about: about ? `${about.type} ${about.id} ${label(about)}` : `the spot (${comment.x}, ${comment.y})` } });
  toast("Sent to the agent.");
}

function openThread(comment) {
  const at = toScreen(comment);
  const rect = el.canvas.getBoundingClientRect();
  const reply = h("textarea", { class: "input", rows: 2, placeholder: "Reply", style: { width: "280px" } });
  const item = (who, time, text) => h("div", { class: "thread-item" }, h("div", { class: "thread-who" }, who || "Someone", time ? h("span", { class: "thread-when" }, new Date(time).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })) : null), h("div", { class: "thread-text" }, text));
  const edit_ = (edit) => change("", () => edit(byId(comment.id)));
  const send = (toAgent) => {
    const text = reply.value.trim();
    if (text) edit_((one) => (one.replies ||= []).push({ by: me(), text, at: when() }));
    float.close();
    if (toAgent) ask(byId(comment.id));
  };
  reply.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) send(false);
  });
  const about = comment.on && byId(comment.on);
  const float = popover(
    { x: rect.left + at.x + 14, y: rect.top + at.y - 10, width: 0, height: 0 },
    h(
      "div",
      { class: "thread" },
      about ? h("div", { class: "thread-about" }, "About " + label(about)) : null,
      item(comment.by, comment.at, comment.text),
      (comment.replies || []).map((one) => item(one.by, one.at, one.text)),
      state.owner ? reply : null,
      state.owner
        ? h(
            "div",
            { class: "thread-foot" },
            h("span", {}, button({ icon: "check", title: comment.resolved ? "Open it again" : "Done: resolve it", pressed: Boolean(comment.resolved), onClick: () => (float.close(), edit_((one) => (one.resolved ? delete one.resolved : (one.resolved = true)))) }), button({ icon: "trash", title: "Delete the note", onClick: () => (float.close(), change("", () => (state.board.elements = elements().filter((one) => one.id !== comment.id)))) })),
            h("span", {}, button({ icon: "send", label: "To the agent", title: "Send it to the agent in the chat", onClick: () => send(true) }), button({ label: "Reply", kind: "primary", onClick: () => send(false) })),
          )
        : null,
    ),
  );
}

function listComments(anchor) {
  const notes = elements().filter((element) => element.type === "comment");
  if (notes.length === 0) return void menu(anchor, [{ heading: "No notes are pinned to this board." }, { label: "Pin one…", onSelect: () => setTool("comment") }], { place: "end" });
  popover(
    anchor,
    h(
      "div",
      { class: "comment-list" },
      notes.map((note) => {
        const row = h("button", { type: "button", class: "comment-row" + (note.resolved ? " resolved" : "") }, h("div", { class: "thread-who" }, note.by || "Someone", note.replies?.length ? h("span", { class: "thread-when" }, `${note.replies.length} repl${note.replies.length === 1 ? "y" : "ies"}`) : null), h("div", { class: "thread-about" }, note.text));
        row.addEventListener("click", () => {
          closeFloats();
          show({ x: note.x - 200, y: note.y - 150, w: 400, h: 300 }, { most: 1 });
          openThread(note);
        });
        return row;
      }),
    ),
    { place: "end" },
  );
}

// ---- Putting it together -------------------------------------------------------------------

export function build(root) {
  ui.hint = h("div", { class: "hint", hidden: true });
  ui.empty = h("div", { class: "welcome", hidden: true });
  root.prepend(buildTop());
  const stage = el.canvas.parentElement;
  stage.append(buildDock(), buildCorner(), ui.hint, ui.empty);
  if (!state.owner) ui.dock.hidden = true;

  on("tool", drawTool);
  on("history", () => {
    ui.undo.disabled = !state.canUndo;
    ui.redo.disabled = !state.canRedo;
  });
  on("view", () => {
    ui.zoom.textContent = Math.round(state.view.scale * 100) + "%";
    placeBar();
  });
  on("selection", drawBar);
  on("editing", drawBar);
  on("board", () => {
    drawBar();
    drawEmpty();
  });
  on("marks", placeBar);
  on("menu", ({ x, y, hit }) => {
    if (!state.owner) return;
    const anchor = { x, y, width: 0, height: 0 };
    if (hit) return moreMenu(anchor);
    const rect = el.canvas.getBoundingClientRect();
    anchor.board = { x: (x - rect.left) / state.view.scale + state.view.x, y: (y - rect.top) / state.view.scale + state.view.y };
    boardMenu(anchor);
  });
  on("do", act);
  on("comment", compose);
  on("thread", openThread);
  // A click on a pinned note opens it.
  el.canvas.addEventListener("click", (event) => {
    const pin = event.target.closest?.(".el.comment");
    if (pin && state.tool === "select") openThread(byId(pin.dataset.id));
  });
  drawTool();
}

// Nothing on the board yet: how to begin.
function drawEmpty() {
  const none = elements().length === 0 && (state.owner || state.preview);
  ui.empty.hidden = !none;
  if (!none || ui.empty.firstChild) return;
  if (state.preview) {
    ui.empty.append(h("h1", {}, "Nothing on this board yet"), h("p", {}, "It shows here as the agent draws it."));
    return;
  }
  ui.empty.append(
    h("h1", {}, "An empty board"),
    h("p", {}, "Double-click anywhere for a note, or pick a tool at the left. Say in the chat what the board should show (a flow, a plan, the parts of a system) and the agent draws it; what either of you changes shows at once."),
    h("div", { class: "welcome-actions" }, button({ icon: "template", label: "Start from a template", kind: "primary", onClick: () => openTemplates() })),
  );
}

