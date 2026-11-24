// Draws the board: every element as a piece of the page (a note is a box
// of words, a shape a drawing with words on it, a line a small drawing of
// its own), laid on a sheet that is moved and scaled as the board is looked
// at; and over it, in the page's own measure, what marks the selection.

import { h, icon } from "/_artifact/kit/ui.js";
import { state, elements, byId, selected, on, begin, commit, emit } from "./model.js";
import { soft, strong, ON_SOFT, SIZES, shapeOf, box, union, route, stroke, ties, sidePoint, loosen, pictureSrc } from "./shapes.js";

const SVG = "http://www.w3.org/2000/svg";
const svg = (tag, attrs = {}, ...children) => {
  const element = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attrs)) if (value !== null && value !== undefined) element.setAttribute(key, value);
  element.append(...children);
  return element;
};

export const el = {};
// id -> { node, signature }
const drawn = new Map();

export function build(root) {
  el.world = h("div", { class: "world" });
  el.overlay = h("div", { class: "overlay" });
  el.canvas = h("div", { class: "canvas", tabindex: "0" }, el.world, el.overlay);
  root.append(el.canvas);
  on("board", draw);
  on("view", () => {
    place();
    marks();
  });
  on("selection", marks);
  on("editing", draw);
  new ResizeObserver(() => marks()).observe(el.canvas);
  place();
}

// The sheet where the view says it is.
function place() {
  const { x, y, scale } = state.view;
  el.world.style.transform = `translate(${-x * scale}px, ${-y * scale}px) scale(${scale})`;
  el.world.style.setProperty("--inv", String(1 / scale));
  // The dots of the ground move and grow with the board, and thin out
  // when it is looked at from far.
  const step = 20 * scale * (scale < 0.4 ? 4 : scale < 0.8 ? 2 : 1);
  el.canvas.style.backgroundSize = `${step}px ${step}px`;
  el.canvas.style.backgroundPosition = `${-x * scale}px ${-y * scale}px`;
}

// Board point <-> point of the page (within the canvas).
export const toScreen = (point) => ({ x: (point.x - state.view.x) * state.view.scale, y: (point.y - state.view.y) * state.view.scale });
export function toBoard(event) {
  const rect = el.canvas.getBoundingClientRect();
  return { x: (event.clientX - rect.left) / state.view.scale + state.view.x, y: (event.clientY - rect.top) / state.view.scale + state.view.y };
}
export const viewSize = () => ({ w: el.canvas.clientWidth, h: el.canvas.clientHeight });

// ---- Elements ---------------------------------------------------------------

// Words as lines; with `list`, each line a point of a list.
function words(text, list) {
  const lines = String(text ?? "").split("\n");
  return lines.map((line) => h("div", { class: list ? "ln li" : "ln" }, line === "" ? h("br") : line));
}

function textStyle(element, fallback = "m") {
  const [points] = SIZES[element.size] || SIZES[fallback];
  return {
    fontSize: points + "px",
    fontWeight: element.bold ? "650" : "",
    fontStyle: element.italic ? "italic" : "",
    textAlign: element.align || "",
  };
}

// The box of words of an element; typed in while it is the one edited.
function wordsBox(element, { fallback, placeholder } = {}) {
  const editing = state.editing === element.id;
  const node = h("div", { class: "txt", style: textStyle(element, fallback), "data-placeholder": placeholder || "" }, words(element.text, element.list));
  if (editing) editable(node, element, "text");
  return node;
}

// Makes `node` the place where `element[field]` is typed.
function editable(node, element, field) {
  node.contentEditable = "true";
  node.spellcheck = true;
  node.classList.add("editing");
  node.addEventListener("paste", (event) => {
    // Words only: what a page copied carries its own looks.
    event.preventDefault();
    document.execCommand("insertText", false, event.clipboardData.getData("text/plain"));
  });
  node.addEventListener("keydown", (event) => {
    event.stopPropagation();
    if (event.key === "Escape" || (event.key === "Enter" && (event.metaKey || event.ctrlKey))) {
      event.preventDefault();
      node.blur();
    }
  });
  node.addEventListener("blur", () => finishEditing(node, element.id, field), { once: true });
}

// What was typed is the element's words now.
function finishEditing(node, id, field) {
  if (state.editing !== id) return;
  const element = byId(id);
  state.editing = null;
  if (element) {
    const text = node.innerText.replace(/ /g, " ").replace(/\n$/, "");
    // Nothing typed: nothing changed (no step, nothing to write).
    if (text !== (element[field] ?? "")) {
      begin();
      element[field] = text;
      grow(element, node);
      // A text with no words left is nothing: it goes, and a line tied to
      // it stays where it was.
      if (element.type === "text" && text.trim() === "") {
        state.board.elements = elements().filter((one) => one !== element);
        loosen(elements(), [element]);
      }
      commit();
    }
  }
  emit("editing");
}

// A note's room around its words: its padding (14 above, 8 below) and its
// foot (16).
const NOTE_AROUND = 38;

// A note grows with its words; a text is as high as they are.
function grow(element, node) {
  // The words' own height, in board points: the box they are in fills its
  // note, so its height says nothing.
  const range = document.createRange();
  range.selectNodeContents(node);
  const high = Math.ceil(range.getBoundingClientRect().height / state.view.scale);
  if (!Number.isFinite(high)) return;
  if (element.type === "sticky") element.h = Math.max(element.h || 0, high + NOTE_AROUND);
  if (element.type === "text") element.h = Math.max(24, high);
}

// Starts typing in element `id` (its words, a section's title, a line's
// label).
export function edit(id) {
  const element = byId(id);
  if (!element || !state.owner || element.locked) return;
  if (!["sticky", "shape", "text", "frame", "line", "table"].includes(element.type)) return;
  state.editing = id;
  emit("editing");
  // A table is typed in cell by cell: its first.
  const node = drawn.get(id)?.node.querySelector(".editing, .cell");
  if (!node) return;
  node.focus();
  // The caret after the last word.
  const range = document.createRange();
  range.selectNodeContents(node);
  range.collapse(false);
  const selection = getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
}

const placed = (element, extra = {}) => {
  const b = box(element);
  return { left: b.x + "px", top: b.y + "px", width: b.w + "px", height: b.h + "px", ...extra };
};

const MAKE = {
  sticky(element) {
    return h(
      "div",
      { class: "el sticky", style: placed(element, { background: soft(element.color), color: ON_SOFT }) },
      wordsBox(element, { placeholder: "Type a note" }),
      h(
        "div",
        { class: "sticky-foot" },
        h("span", { class: "sticky-by" }, element.by || ""),
        element.votes ? h("span", { class: "votes", title: "Votes" }, "●".repeat(Math.min(element.votes, 5)), element.votes > 5 ? " " + element.votes : "") : null,
      ),
    );
  },
  shape(element) {
    const shape = shapeOf(element.shape);
    const filled = element.fill && element.fill !== "none";
    const outline = element.stroke && element.stroke !== "none" ? strong(element.stroke) : filled ? "none" : "var(--ui-fg)";
    const inset = shape.inset || 0;
    return h(
      "div",
      { class: "el shape", style: placed(element, { color: filled ? ON_SOFT : "var(--ui-fg)" }) },
      svg(
        "svg",
        { class: "shape-art", viewBox: `0 0 ${element.w} ${element.h}`, preserveAspectRatio: "none" },
        svg("path", {
          d: shape.path(element.w, element.h),
          fill: filled ? soft(element.fill) : "transparent",
          stroke: outline,
          "stroke-width": outline === "none" ? 0 : 2,
          "stroke-dasharray": element.dash ? "7 6" : null,
          "vector-effect": "non-scaling-stroke",
        }),
      ),
      h(
        "div",
        {
          class: "shape-words",
          style: {
            padding: `${Math.round(element.h * inset) + 8}px ${Math.round(element.w * inset) + 10}px`,
            alignItems: shape.low ? "flex-end" : shape.high ? "flex-start" : "center",
          },
        },
        wordsBox({ align: "center", ...element }),
      ),
    );
  },
  text(element) {
    const b = box(element);
    return h(
      "div",
      { class: "el text", style: { left: b.x + "px", top: b.y + "px", width: b.w + "px", color: strong(element.color) } },
      wordsBox(element, { placeholder: "Type something" }),
    );
  },
  frame(element) {
    const editing = state.editing === element.id;
    const title = h("div", { class: "frame-title", "data-placeholder": "Section" }, element.title || "");
    if (editing) editable(title, element, "title");
    // A section is taken hold of by its edge and its title: what lies in
    // it is reached through it.
    const node = h("div", { class: "el frame", style: placed(element) }, h("div", { class: "frame-edge" }), title);
    node.style.setProperty("--frame", element.color && element.color !== "gray" ? strong(element.color) : "var(--ui-muted)");
    return node;
  },
  table(element) {
    const rows = Array.isArray(element.rows) && element.rows.length ? element.rows : [[""]];
    const columns = Math.max(...rows.map((row) => row.length), 1);
    const editing = state.editing === element.id;
    const grid = h("div", { class: "table-grid", style: { gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`, gridTemplateRows: `repeat(${rows.length}, minmax(0, 1fr))` } });
    rows.forEach((row, r) => {
      for (let c = 0; c < columns; c += 1) {
        const cell = h("div", { class: "cell" + (r === 0 ? " head" : ""), dataset: { r, c } }, row[c] ?? "");
        if (editing) {
          cell.contentEditable = "plaintext-only";
          cell.addEventListener("keydown", (event) => {
            event.stopPropagation();
            if (event.key === "Escape") finishTable(grid, element.id);
          });
        }
        grid.append(cell);
      }
    });
    if (editing) grid.addEventListener("focusout", (event) => !grid.contains(event.relatedTarget) && finishTable(grid, element.id));
    return h("div", { class: "el table" + (editing ? " editing" : ""), style: placed(element) }, grid);
  },
  sticker(element) {
    const b = box(element);
    return h("div", { class: "el sticker", style: placed(element, { fontSize: Math.min(b.w, b.h) * 0.82 + "px" }) }, element.emoji || "⭐");
  },
  image(element) {
    // (A picture from elsewhere is not fetched: its place is shown empty.)
    const src = pictureSrc(element.src);
    return h("div", { class: "el image" + (src ? "" : " missing"), style: placed(element) }, src ? h("img", { src, alt: "", draggable: false }) : null);
  },
  comment(element) {
    const open = !element.resolved;
    return h(
      "div",
      { class: "el comment" + (open ? "" : " resolved"), style: { left: element.x + "px", top: element.y + "px" }, title: element.text || "" },
      h("span", { class: "pin" }, icon("comment", 14)),
    );
  },
  line(element) {
    const r = route(element);
    const b = box(element);
    const pad = 24;
    const colour = strong(element.color || "ink");
    const width = element.width || 2;
    const shift = (d) => d; // The path is in board points: the drawing is moved, not the path.
    const head = (kind, at, angle) => {
      if (kind === "arrow") {
        const size = 6 + width * 2.2;
        const wing = (turn) => `${at.x - size * Math.cos(angle + turn)} ${at.y - size * Math.sin(angle + turn)}`;
        return svg("path", { d: `M${wing(-0.45)}L${at.x} ${at.y}L${wing(0.45)}`, fill: "none", stroke: colour, "stroke-width": width, "stroke-linecap": "round", "stroke-linejoin": "round" });
      }
      if (kind === "dot") return svg("circle", { cx: at.x, cy: at.y, r: 3 + width, fill: colour });
      return null;
    };
    const art = svg(
      "svg",
      { class: "line-art", viewBox: `${b.x - pad} ${b.y - pad} ${b.w + pad * 2} ${b.h + pad * 2}`, width: b.w + pad * 2, height: b.h + pad * 2 },
      svg("path", { class: "line-hit", d: shift(r.d), fill: "none", stroke: "transparent", "stroke-width": 16 }),
      svg("path", { d: shift(r.d), fill: "none", stroke: colour, "stroke-width": width, "stroke-linecap": "round", "stroke-linejoin": "round", "stroke-dasharray": element.dash ? `${width * 3} ${width * 3}` : null }),
    );
    for (const part of [head(element.end ?? "arrow", r.b, r.intoB), head(element.start ?? "none", r.a, r.intoA)]) if (part) art.append(part);
    const editing = state.editing === element.id;
    const label = element.text || editing ? h("div", { class: "line-label", style: { left: r.label.x - (b.x - pad) + "px", top: r.label.y - (b.y - pad) + "px" } }, element.text || "") : null;
    if (label && editing) editable(label, element, "text");
    return h("div", { class: "el line", style: { left: b.x - pad + "px", top: b.y - pad + "px", width: b.w + pad * 2 + "px", height: b.h + pad * 2 + "px" } }, art, label);
  },
  draw(element) {
    const b = box(element);
    const width = element.width || 3;
    const pad = width + 4;
    return h(
      "div",
      { class: "el draw", style: { left: b.x - pad + "px", top: b.y - pad + "px", width: b.w + pad * 2 + "px", height: b.h + pad * 2 + "px" } },
      svg(
        "svg",
        { class: "line-art", viewBox: `${b.x - pad} ${b.y - pad} ${b.w + pad * 2} ${b.h + pad * 2}`, width: b.w + pad * 2, height: b.h + pad * 2 },
        svg("path", { class: "line-hit", d: stroke(element.points || []), fill: "none", stroke: "transparent", "stroke-width": Math.max(14, width + 8) }),
        svg("path", {
          d: stroke(element.points || []),
          fill: "none",
          stroke: strong(element.color || "ink"),
          "stroke-width": width,
          "stroke-linecap": "round",
          "stroke-linejoin": "round",
          opacity: element.marker ? 0.42 : 1,
        }),
      ),
    );
  },
};

// What is typed into a table's cells is its rows now.
function finishTable(grid, id) {
  if (state.editing !== id) return;
  const element = byId(id);
  state.editing = null;
  if (element) {
    const rows = [];
    for (const cell of grid.children) {
      const { r, c } = cell.dataset;
      (rows[r] ||= [])[c] = cell.innerText.replace(/\n$/, "");
    }
    begin();
    element.rows = rows;
    commit();
  }
  emit("editing");
}

// The kinds whose piece of the page is a box at (x, y): moved, it is put
// elsewhere, not made again.
const BOXED = new Set(["sticky", "shape", "text", "frame", "table", "sticker", "image"]);

// What makes an element look as it does: when it reads differently, its
// piece of the page is made again. Where it is is left out for a box.
function signature(element) {
  const extra = state.editing === element.id ? "|editing" : "";
  // A line follows what it is tied to.
  if (element.type === "line") return JSON.stringify(element) + extra + "|" + route(element).d;
  if (!BOXED.has(element.type)) return JSON.stringify(element) + extra;
  const { x, y, ...rest } = element;
  return JSON.stringify(rest) + extra;
}

// Draws the board at the next frame: for what changes with every move of
// the pointer, which may come several times a frame.
let due = 0;
export function drawSoon() {
  if (due) return;
  due = requestAnimationFrame(() => {
    due = 0;
    draw();
  });
}

export function draw() {
  if (due) {
    cancelAnimationFrame(due);
    due = 0;
  }
  const seen = new Set();
  const measure = [];
  let previous = null;
  for (const element of elements()) {
    const make = MAKE[element.type];
    if (!make) continue;
    seen.add(element.id);
    let entry = drawn.get(element.id);
    try {
      const mark = signature(element);
      if (!entry || entry.signature !== mark) {
        // The one typed in is left as it is while it is typed in.
        if (entry && state.editing === element.id && entry.node.querySelector(".editing")) {
          entry.signature = mark;
        } else {
          const node = make(element);
          node.dataset.id = element.id;
          if (element.locked) node.classList.add("locked");
          if (entry) entry.node.replaceWith(node);
          entry = { node, signature: mark, x: element.x, y: element.y };
          drawn.set(element.id, entry);
          if (element.type === "text" && state.editing !== element.id) measure.push([element, node]);
        }
      } else if (BOXED.has(element.type) && (entry.x !== element.x || entry.y !== element.y)) {
        entry.node.style.left = element.x + "px";
        entry.node.style.top = element.y + "px";
        entry.x = element.x;
        entry.y = element.y;
      }
    } catch (error) {
      // One thing written wrong (by an agent, by hand) is left out; the
      // rest of the board is drawn.
      console.error(error);
      if (!entry) {
        seen.delete(element.id);
        continue;
      }
    }
    // In the order of the list: the last is on top.
    const after = previous ? previous.nextSibling : el.world.firstChild;
    if (after !== entry.node) el.world.insertBefore(entry.node, after);
    previous = entry.node;
  }
  for (const [id, entry] of [...drawn]) {
    if (!seen.has(id)) {
      entry.node.remove();
      drawn.delete(id);
    }
  }
  // A text is as high as its words: known once it is drawn. (Read after
  // all is written, so the page is laid out once.)
  for (const [element, node] of measure) {
    const height = node.offsetHeight;
    if (height && Math.abs(height - (element.h || 0)) > 1) element.h = height;
  }
  marks();
}

// The element of the board a piece of the page belongs to.
export function elementAt(target) {
  const node = target instanceof Element ? target.closest(".el") : null;
  return node ? byId(node.dataset.id) : null;
}

// ---- What marks the selection -------------------------------------------------

// Lines and boxes the tools ask for (a selection being dragged out, the
// lines things snap to, a line being drawn).
export const extra = { marquee: null, guides: [], target: null };

const screenBox = (b) => {
  const a = toScreen(b);
  return { x: a.x, y: a.y, w: b.w * state.view.scale, h: b.h * state.view.scale };
};
const rectStyle = (b) => ({ left: b.x + "px", top: b.y + "px", width: b.w + "px", height: b.h + "px" });

export function marks() {
  if (!el.overlay) return;
  const nodes = [];
  const picked = selected();
  const one = picked.length === 1 ? picked[0] : null;
  for (const element of picked) {
    if (element.type === "line") {
      // A line alone shows its ends, to be taken hold of; among other
      // things it is marked as they are.
      if (one && state.owner && !element.locked) {
        const r = route(element);
        for (const [name, point] of [["from", r.a], ["to", r.b]]) {
          const at = toScreen(point);
          nodes.push(h("div", { class: "end-handle", dataset: { end: name }, style: { left: at.x + "px", top: at.y + "px" } }));
        }
      }
      if (one) continue;
    }
    nodes.push(h("div", { class: "outline" + (picked.length > 1 ? " part" : "") + (element.locked ? " locked" : ""), style: rectStyle(screenBox(box(element))) }));
  }
  if (picked.length > 1) nodes.push(h("div", { class: "outline all", style: rectStyle(screenBox(union(picked))) }));
  if (one && state.owner && !one.locked && state.editing !== one.id && one.type !== "line" && one.type !== "draw" && one.type !== "comment") {
    const b = screenBox(box(one));
    const corners = { nw: [0, 0], ne: [1, 0], sw: [0, 1], se: [1, 1] };
    // A text is made wider or narrower; its height is its words'.
    const sides = one.type === "text" ? { w: [0, 0.5], e: [1, 0.5] } : corners;
    for (const [at, [fx, fy]] of Object.entries(sides)) {
      nodes.push(h("div", { class: "handle", dataset: { at }, style: { left: b.x + b.w * fx + "px", top: b.y + b.h * fy + "px" } }));
    }
    // Where a line can be pulled out of it.
    if (ties(one)) {
      for (const side of ["top", "right", "bottom", "left"]) {
        const point = sidePoint(b, side);
        const out = { top: [0, -16], right: [16, 0], bottom: [0, 16], left: [-16, 0] }[side];
        nodes.push(h("div", { class: "tie", dataset: { side }, title: "Drag: a line from here. Click: the same again, joined by an arrow.", style: { left: point.x + out[0] + "px", top: point.y + out[1] + "px" } }));
      }
    }
  }
  if (extra.target) nodes.push(h("div", { class: "outline target", style: rectStyle(screenBox(box(extra.target))) }));
  if (extra.marquee) nodes.push(h("div", { class: "marquee", style: rectStyle(screenBox(extra.marquee)) }));
  for (const guide of extra.guides) {
    const at = toScreen({ x: guide.x ?? 0, y: guide.y ?? 0 });
    nodes.push(h("div", { class: "guide", style: guide.x !== undefined ? { left: at.x + "px", top: 0, bottom: 0, width: "1px" } : { top: at.y + "px", left: 0, right: 0, height: "1px" } }));
  }
  el.overlay.replaceChildren(...nodes);
  emit("marks");
}

// Lets go of typing, keeping what was typed.
export function stopEditing() {
  const node = el.world.querySelector(".editing");
  if (node) node.blur();
  const grid = el.world.querySelector(".table.editing .table-grid");
  if (grid && state.editing) finishTable(grid, state.editing);
}

