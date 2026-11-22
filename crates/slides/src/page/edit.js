// Changing what is on a slide by hand: selecting a thing, moving and
// resizing it, typing in it, and adding new things. Every change is made
// on the slide's own element and saved at once (`env.commit`), so the file
// is what the stage shows.
//
// Things that were laid out by the deck's look (a heading in the flow of a
// slide) are moved by an offset of their own (`translate`), so nothing else
// on the slide shifts; things added by hand sit at a place of their own
// (`position: absolute`), as on a slide in any presentation program.

import { chartSpec, dress, parseSlide } from "./core.js";
import { h, button, menu, popover, dialog, toast, colourPicker, hex, closeFloats, icon, keys as keyHint } from "./ui.js";
import * as store from "./store.js";
import { state, emit } from "./store.js";

// Before the HTML of something copied from a slide, on the clipboard.
const CLIP_MARK = "<!--deck-element-->";

export const SHAPES = [
  { id: "rect", name: "Rectangle", icon: "rect", key: "R", code: "R" },
  { id: "rounded", name: "Rounded rectangle", icon: "rounded", key: "⇧R", code: "⇧R" },
  { id: "ellipse", name: "Ellipse", icon: "ellipse", key: "O", code: "O" },
  { id: "diamond", name: "Diamond", icon: "diamond", key: "D", code: "D" },
  { id: "arrow-right", name: "Arrow right", icon: "arrowRight" },
  { id: "arrow-left", name: "Arrow left", icon: "arrowLeft" },
  { id: "arrow-up", name: "Arrow up", icon: "arrowUp" },
  { id: "arrow-down", name: "Arrow down", icon: "arrowDown" },
  { id: "line", name: "Line", icon: "line", key: "L", code: "L" },
  { id: "connector", name: "Connector", icon: "connector", key: "⇧L", code: "⇧L" },
];

const INLINE = new Set(["SPAN", "STRONG", "EM", "B", "I", "U", "A", "CODE", "BR", "SMALL", "MARK", "SUB", "SUP", "S", "TSPAN"]);
const TEXT_TAGS = new Set(["H1", "H2", "H3", "H4", "H5", "H6", "P", "UL", "OL", "LI", "BLOCKQUOTE", "PRE", "FIGCAPTION", "CITE", "TD", "TH"]);

export const TEXT_STYLES = [
  { id: "h1", name: "Heading 1", tag: "h1" },
  { id: "h2", name: "Heading 2", tag: "h2" },
  { id: "h3", name: "Heading 3", tag: "h3" },
  { id: "body", name: "Body", tag: "p" },
  { id: "lead", name: "Lead", tag: "p", class: "lead" },
  { id: "small", name: "Caption", tag: "p", class: "small" },
  { id: "eyebrow", name: "Eyebrow", tag: "p", class: "eyebrow" },
  { id: "quote", name: "Quote", tag: "blockquote" },
];
const STYLE_CLASSES = ["lead", "small", "eyebrow", "caption"];

// What kind of thing an element of a slide is, for what can be done to it.
export function kindOf(element) {
  if (!element) return null;
  if (element.hasAttribute("data-shape")) return "shape";
  if (element.hasAttribute("data-chart")) return "chart";
  const tag = element.tagName;
  if (tag === "IMG") return "image";
  if (tag === "VIDEO" || tag === "AUDIO") return "video";
  if (tag === "TABLE") return "table";
  if (tag === "svg" || tag === "SVG") return "drawing";
  if (TEXT_TAGS.has(tag)) return "text";
  const ownText = [...element.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim());
  return ownText ? "text" : "box";
}

// Which text style an element has.
export function textStyleOf(element) {
  const tag = element.tagName.toLowerCase();
  const found = TEXT_STYLES.find(
    (style) => style.tag === tag && (style.class ? element.classList.contains(style.class) : !STYLE_CLASSES.some((name) => element.classList.contains(name))),
  );
  return found || TEXT_STYLES.find((style) => style.tag === tag) || null;
}

// The deck's own colours, as the slide has them: [{ name, variable, value }].
export function palette(slide) {
  if (!slide) return [];
  const styles = getComputedStyle(slide);
  const names = [
    ["Text", "--fg"],
    ["Background", "--bg"],
    ["Accent", "--accent"],
    ["On accent", "--accent-fg"],
    ["Muted", "--muted"],
    ["Surface", "--surface"],
    ["Line", "--line"],
  ];
  return names
    .map(([name, variable]) => ({ name, variable, value: styles.getPropertyValue(variable).trim() }))
    .filter((entry) => entry.value);
}

const px = (value) => Math.round(value * 10) / 10 + "px";

export class Editing {
  constructor(env) {
    this.env = env;
    this.selected = null;
    this.typing = null;
    this.hovered = null;
    this.drag = null;
    this.clip = null;
    this.cropping = false;
    const frame = env.el.frame;
    frame.addEventListener("pointerdown", (event) => this.pointerDown(event));
    frame.addEventListener("pointermove", (event) => this.pointerMove(event));
    frame.addEventListener("pointerup", (event) => this.pointerUp(event));
    frame.addEventListener("pointercancel", (event) => this.pointerUp(event));
    frame.addEventListener("pointerleave", () => this.hover(null));
    frame.addEventListener("dblclick", (event) => this.doubleClick(event));
    // A link on a slide is followed when presenting; here a click on it
    // selects its text.
    frame.addEventListener(
      "click",
      (event) => {
        if (event.composedPath().some((node) => node instanceof Element && node.tagName === "A")) event.preventDefault();
      },
      true,
    );
    frame.addEventListener("contextmenu", (event) => this.contextMenu(event));
    frame.addEventListener("dragover", (event) => {
      if (event.dataTransfer.types.includes("Files")) {
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
      }
    });
    frame.addEventListener("drop", (event) => this.dropped(event));
    // A click on the stage beside the slide lets go of what is selected.
    env.el.stage.addEventListener("pointerdown", (event) => {
      if (event.target === env.el.stage || event.target === env.el.inner) this.select(null);
    });
    document.addEventListener("paste", (event) => this.pasted(event));
    document.addEventListener("copy", (event) => this.copied(event, false));
    document.addEventListener("cut", (event) => this.copied(event, true));
    // WebKit sends a copy, cut or paste where nothing is typed only to a
    // page that says beforehand that it takes it.
    for (const type of ["beforecopy", "beforecut", "beforepaste"]) {
      document.addEventListener(type, (event) => {
        if (this.takesClipboard(event)) event.preventDefault();
      });
    }
    this.tellSoon = debounce(() => store.tell({ selection: this.describe() }), 160);
    this.commitTyping = debounce(() => this.env.commit(`Edited text on slide ${state.current + 1}`), 450);
  }

  get slide() {
    return this.env.stage.slide;
  }

  get scale() {
    return this.env.view.scale;
  }

  isTyping() {
    return Boolean(this.typing);
  }

  // ---- What is under the pointer -----------------------------------------

  // The thing of the slide an event is on: the text block around a word,
  // the table around a cell, the list around an item.
  blockAt(event) {
    const slide = this.slide;
    if (!slide) return null;
    const hit = event.composedPath().find((node) => node instanceof Element && slide.contains(node) && node !== slide);
    return this.blockOf(hit);
  }

  blockOf(hit) {
    const slide = this.slide;
    let node = hit;
    while (node && node !== slide) {
      if (!INLINE.has(node.tagName) || getComputedStyle(node).display !== "inline") break;
      node = node.parentElement;
    }
    if (!node || node === slide || !slide.contains(node)) return null;
    for (const around of ["table", "[data-chart]", "svg", "[data-shape]"]) {
      const found = node.closest(around);
      if (found && slide.contains(found) && found !== slide) return found;
    }
    if (node.tagName === "LI") {
      const list = node.closest("ul, ol");
      if (list && slide.contains(list)) return list;
    }
    return node;
  }

  // ---- Selecting ----------------------------------------------------------

  select(element) {
    if (this.selected === element) {
      this.refresh();
      return;
    }
    this.stopTyping();
    this.cropping = false;
    closeFloats();
    this.selected = element && this.slide?.contains(element) ? element : null;
    this.buildBar();
    this.refresh();
    emit("selection", this.selected);
    this.tellSoon();
  }

  // The slide on the stage was drawn again (another slide, or the file
  // changed): what was selected is found again, or let go.
  slideChanged() {
    const was = this.selected;
    const id = was?.getAttribute("data-id");
    this.typing = null;
    this.selected = null;
    this.hovered = null;
    const again = id && this.slide ? this.slide.querySelector(`[data-id="${CSS.escape(id)}"]`) : null;
    if (again) this.selected = again;
    this.buildBar();
    this.refresh();
    if (was !== this.selected) emit("selection", this.selected);
    this.tellSoon();
  }

  // What is selected, for the agent: which slide, and which thing on it.
  describe() {
    const slide = store.currentSlide();
    if (!slide) return null;
    const about = { slide: state.current + 1, file: slide.file };
    const element = this.selected;
    if (!element) return about;
    const text = element.textContent.replace(/\s+/g, " ").trim();
    return {
      ...about,
      element: element.getAttribute("data-id") || "",
      tag: element.tagName.toLowerCase(),
      kind: kindOf(element),
      text: text.length > 160 ? text.slice(0, 159) + "…" : text,
      path: pathOf(element, this.slide),
    };
  }

  // A short name of a thing, as a comment on it reads ("Heading 1 “Our plan”").
  label(element) {
    const kind = kindOf(element);
    const text = element.textContent.replace(/\s+/g, " ").trim();
    const quoted = text ? ` “${text.length > 40 ? text.slice(0, 39) + "…" : text}”` : "";
    if (kind === "text") return (textStyleOf(element)?.name || "Text") + quoted;
    const names = { image: "Picture", video: "Video", table: "Table", chart: "Chart", shape: "Shape", drawing: "Drawing", box: "Box" };
    return (names[kind] || "Element") + (kind === "box" || kind === "shape" ? quoted : "");
  }

  // Gives the thing an id of its own in its slide, if it has none: what a
  // comment or a later change finds it by.
  idOf(element) {
    let id = element.getAttribute("data-id");
    if (!id) {
      id = newId(this.slide);
      element.setAttribute("data-id", id);
    }
    return id;
  }

  // ---- Marks: the outline, its handles, the bar ---------------------------

  boxOf(element) {
    const within = this.env.el.main.getBoundingClientRect();
    const box = element.getBoundingClientRect();
    return { x: box.x - within.x, y: box.y - within.y, width: box.width, height: box.height };
  }

  refresh() {
    const marks = this.env.el.marks;
    for (const old of marks.querySelectorAll(".outline, .handle, .guide")) old.remove();
    if (this.env.view.mode !== "single") {
      this.bar?.remove();
      return;
    }
    if (this.hovered && this.hovered !== this.selected && this.hovered.isConnected) {
      const box = this.boxOf(this.hovered);
      marks.append(h("div", { class: "outline hover", style: place(box) }));
    }
    const element = this.selected;
    if (!element || !element.isConnected) {
      this.bar?.remove();
      return;
    }
    const box = this.boxOf(element);
    marks.append(h("div", { class: "outline", style: place(box) }));
    if (!this.typing) {
      const kind = kindOf(element);
      const line = ["line", "connector"].includes(element.getAttribute("data-shape"));
      const sides = this.cropping ? ["n", "e", "s", "w"] : kind === "text" || kind === "table" || line ? ["w", "e"] : ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
      for (const at of sides) {
        const x = at.includes("w") ? box.x : at.includes("e") ? box.x + box.width : box.x + box.width / 2;
        const y = at.includes("n") ? box.y : at.includes("s") ? box.y + box.height : box.y + box.height / 2;
        const handle = h("div", { class: "handle", dataset: { at }, style: { left: x + "px", top: y + "px" } });
        handle.addEventListener("pointerdown", (event) => this.resizeStart(event, at));
        marks.append(handle);
      }
    }
    if (this.bar) {
      if (!this.bar.isConnected) this.env.el.main.append(this.bar);
      const mine = this.bar.getBoundingClientRect();
      const within = this.env.el.main.getBoundingClientRect();
      let top = box.y - mine.height - 10;
      if (top < 56) top = Math.min(within.height - mine.height - 8, box.y + box.height + 10);
      const left = Math.max(8, Math.min(within.width - mine.width - 8, box.x));
      this.bar.style.left = left + "px";
      this.bar.style.top = Math.max(8, top) + "px";
    }
  }

  hover(element) {
    if (this.hovered === element) return;
    this.hovered = element;
    this.refresh();
  }

  // ---- The pointer ---------------------------------------------------------

  pointerDown(event) {
    if (event.button !== 0 || this.env.view.commenting) return;
    const block = this.blockAt(event);
    // In the text being typed in: the click places the caret.
    if (this.typing && block && (this.typing === block || this.typing.contains(event.composedPath()[0]))) return;
    if (!block) {
      this.select(null);
      return;
    }
    this.select(block);
    this.drag = {
      id: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      from: placeOf(block),
      box: block.getBoundingClientRect(),
      moved: false,
    };
    try {
      this.env.el.frame.setPointerCapture(event.pointerId);
    } catch {
      // A pointer that is gone already: the drag ends with its next event.
    }
    // No text is swept and no picture is carried off by the drag.
    event.preventDefault();
  }

  pointerMove(event) {
    const drag = this.drag;
    if (!drag) {
      if (!this.env.view.commenting && !this.typing) this.hover(this.blockAt(event));
      return;
    }
    if (drag.id !== event.pointerId) return;
    let dx = event.clientX - drag.x;
    let dy = event.clientY - drag.y;
    if (!drag.moved && Math.hypot(dx, dy) < 4) return;
    drag.moved = true;
    this.bar?.remove();
    if (event.shiftKey) {
      if (Math.abs(dx) > Math.abs(dy)) dy = 0;
      else dx = 0;
    }
    const snapped = event.metaKey || event.ctrlKey ? { dx, dy, guides: [] } : this.snap(drag.box, dx, dy);
    moveTo(this.selected, drag.from, snapped.dx / this.scale, snapped.dy / this.scale);
    this.refresh();
    const within = this.env.el.main.getBoundingClientRect();
    for (const guide of snapped.guides) {
      const style = guide.vertical
        ? { left: guide.at - within.x + "px", top: guide.from - within.y + "px", width: "1px", height: guide.to - guide.from + "px" }
        : { top: guide.at - within.y + "px", left: guide.from - within.x + "px", height: "1px", width: guide.to - guide.from + "px" };
      this.env.el.marks.append(h("div", { class: "guide", style }));
    }
  }

  pointerUp(event) {
    const drag = this.drag;
    if (!drag || drag.id !== event.pointerId) return;
    this.drag = null;
    if (this.env.el.frame.hasPointerCapture(event.pointerId)) this.env.el.frame.releasePointerCapture(event.pointerId);
    if (drag.moved && this.selected) {
      this.idOf(this.selected);
      this.env.commit(`Moved ${this.label(this.selected)} on slide ${state.current + 1}`);
    }
    this.refresh();
  }

  // Where a box being moved by (dx, dy) lines up with the slide or with
  // another thing on it: the move is pulled onto that line, which is shown.
  snap(box, dx, dy) {
    const reach = 6;
    const frame = this.env.el.frame.getBoundingClientRect();
    const others = [frame];
    for (const child of this.slide.children) {
      if (child !== this.selected && !child.contains(this.selected)) others.push(child.getBoundingClientRect());
    }
    const lines = (boxes, start, size) => boxes.flatMap((b) => [b[start], b[start] + b[size] / 2, b[start] + b[size]]);
    const fit = (start, size, delta, candidates) => {
      let best = null;
      for (const mine of [box[start] + delta, box[start] + delta + box[size] / 2, box[start] + delta + box[size]]) {
        for (const line of candidates) {
          const off = line - mine;
          if (Math.abs(off) <= reach && (!best || Math.abs(off) < Math.abs(best.off))) best = { off, line };
        }
      }
      return best;
    };
    const guides = [];
    const vertical = fit("x", "width", dx, lines(others, "x", "width"));
    if (vertical) {
      dx += vertical.off;
      guides.push({ vertical: true, at: vertical.line, from: frame.y, to: frame.y + frame.height });
    }
    const horizontal = fit("y", "height", dy, lines(others, "y", "height"));
    if (horizontal) {
      dy += horizontal.off;
      guides.push({ vertical: false, at: horizontal.line, from: frame.x, to: frame.x + frame.width });
    }
    return { dx, dy, guides };
  }

  resizeStart(event, at) {
    const element = this.selected;
    if (!element || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const handle = event.currentTarget;
    try {
      handle.setPointerCapture(event.pointerId);
    } catch {
      // As above.
    }
    const start = {
      x: event.clientX,
      y: event.clientY,
      width: element.offsetWidth ?? element.getBoundingClientRect().width / this.scale,
      height: element.offsetHeight ?? element.getBoundingClientRect().height / this.scale,
      from: placeOf(element),
      crop: cropOf(element),
    };
    if (!start.width) {
      const box = element.getBoundingClientRect();
      start.width = box.width / this.scale;
      start.height = box.height / this.scale;
    }
    const kind = kindOf(element);
    const keeps = kind === "image" || kind === "video";
    let changed = false;
    const move = (moved) => {
      const dx = (moved.clientX - start.x) / this.scale;
      const dy = (moved.clientY - start.y) / this.scale;
      changed = true;
      if (this.cropping) {
        const crop = { ...start.crop };
        if (at === "n") crop.top = clamp(start.crop.top + (dy / start.height) * 100, 0, 95 - crop.bottom);
        if (at === "s") crop.bottom = clamp(start.crop.bottom - (dy / start.height) * 100, 0, 95 - crop.top);
        if (at === "w") crop.left = clamp(start.crop.left + (dx / start.width) * 100, 0, 95 - crop.right);
        if (at === "e") crop.right = clamp(start.crop.right - (dx / start.width) * 100, 0, 95 - crop.left);
        setCrop(element, crop);
        this.refresh();
        return;
      }
      let width = start.width + (at.includes("e") ? dx : at.includes("w") ? -dx : 0);
      let height = start.height + (at.includes("s") ? dy : at.includes("n") ? -dy : 0);
      width = Math.max(8, width);
      height = Math.max(8, height);
      // A picture keeps its proportions from a corner, unless Shift says not.
      if (keeps && at.length === 2 && !moved.shiftKey) height = (width * start.height) / start.width;
      element.style.width = px(width);
      if (getComputedStyle(element.parentElement).display.includes("flex")) element.style.flex = "none";
      if (kind !== "text" && kind !== "table") element.style.height = px(height);
      // The side opposite the handle stays where it is.
      const shiftX = at.includes("w") ? start.width - width : 0;
      const shiftY = at.includes("n") ? start.height - height : 0;
      if (shiftX || shiftY) moveTo(element, start.from, shiftX, shiftY);
      dress(this.slide);
      this.refresh();
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      if (changed) {
        this.idOf(element);
        this.env.commit(`${this.cropping ? "Cropped" : "Resized"} ${this.label(element)} on slide ${state.current + 1}`);
      }
      this.refresh();
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  }

  doubleClick(event) {
    if (this.env.view.commenting) return;
    const block = this.blockAt(event);
    if (!block) return;
    const kind = kindOf(block);
    if (kind === "text" || kind === "shape" || kind === "box") {
      if (this.typing !== block) this.startTyping(block);
    } else if (kind === "table") {
      const cell = event.composedPath().find((node) => node instanceof Element && (node.tagName === "TD" || node.tagName === "TH"));
      if (cell) this.startTyping(cell, { within: block });
    } else if (kind === "chart") this.editChart(block);
    else if (kind === "image" || kind === "video") this.replaceMedia(block);
  }

  contextMenu(event) {
    const block = this.blockAt(event);
    if (this.typing) return;
    event.preventDefault();
    if (!block) {
      this.select(null);
      if (this.clip) menu({ x: event.clientX, y: event.clientY, width: 0, height: 0 }, [{ label: "Paste", hint: keyHint("⌘V"), onSelect: () => this.paste() }]);
      return;
    }
    this.select(block);
    menu({ x: event.clientX, y: event.clientY, width: 0, height: 0 }, [
      { label: "Duplicate", hint: keyHint("⌘D"), onSelect: () => this.duplicate() },
      { label: "Copy", hint: keyHint("⌘C"), onSelect: () => this.copyNow() },
      { label: "Paste", hint: keyHint("⌘V"), disabled: !this.clip, onSelect: () => this.paste() },
      { separator: true },
      { label: "Bring to front", icon: "front", onSelect: () => this.order(true) },
      { label: "Send to back", icon: "back", onSelect: () => this.order(false) },
      { separator: true },
      { label: "Comment", icon: "comment", onSelect: () => this.env.comment(block) },
      { label: "Delete", hint: "⌫", danger: true, onSelect: () => this.remove() },
    ]);
  }

  // ---- Keys -----------------------------------------------------------------

  // Answers whether the key was this's to take.
  key(event) {
    const element = this.selected;
    if (!element || this.typing) return false;
    const command = event.metaKey || event.ctrlKey;
    const key = event.key;
    if (key === "Backspace" || key === "Delete") this.remove();
    else if (key === "Escape") {
      // Out of what is selected to what holds it; then to nothing.
      const around = this.blockOf(element.parentElement);
      this.select(around && around !== element ? around : null);
    } else if (key === "Enter" && ["text", "shape", "box"].includes(kindOf(element))) this.startTyping(element);
    else if (command && (key === "d" || key === "D")) this.duplicate();
    else if (command && (key === "b" || key === "B")) this.format("bold");
    else if (command && (key === "i" || key === "I")) this.format("italic");
    else if (command && (key === "u" || key === "U")) this.format("underline");
    else if (!command && key.startsWith("Arrow")) {
      const by = event.shiftKey ? 10 : 1;
      const dx = key === "ArrowLeft" ? -by : key === "ArrowRight" ? by : 0;
      const dy = key === "ArrowUp" ? -by : key === "ArrowDown" ? by : 0;
      moveTo(element, placeOf(element), dx, dy);
      this.refresh();
      this.env.commit(`Nudged ${this.label(element)} on slide ${state.current + 1}`);
    } else return false;
    event.preventDefault();
    return true;
  }

  // ---- Changing the selected thing ------------------------------------------

  // Makes a change to the selected element and saves it.
  change(what, make) {
    const element = this.selected;
    if (!element) return;
    make(element);
    this.idOf(element);
    dress(this.slide);
    this.env.commit(`${what} ${this.label(element)} on slide ${state.current + 1}`);
    this.refresh();
    emit("selection", this.selected);
  }

  remove() {
    const element = this.selected;
    if (!element) return;
    const label = this.label(element);
    // The spaces that set it on a line of its own in the file go with it.
    const before = element.previousSibling;
    if (before?.nodeType === 3 && !before.textContent.trim()) before.remove();
    element.remove();
    this.select(null);
    this.env.commit(`Deleted ${label} from slide ${state.current + 1}`);
  }

  duplicate() {
    const element = this.selected;
    if (!element) return;
    const copy = element.cloneNode(true);
    copy.setAttribute("data-id", newId(this.slide));
    for (const inner of copy.querySelectorAll("[data-id]")) inner.removeAttribute("data-id");
    element.after("\n  ", copy);
    moveTo(copy, placeOf(copy), 24, 24);
    dress(this.slide);
    this.select(copy);
    this.env.commit(`Duplicated ${this.label(copy)} on slide ${state.current + 1}`);
  }

  copy() {
    if (!this.selected) return;
    const copy = this.selected.cloneNode(true);
    for (const drawn of copy.querySelectorAll("svg.chart-drawn")) drawn.remove();
    this.clip = copy.outerHTML;
  }

  // Whether a copy, cut or paste is of the slide's things (not of words
  // being typed, nor in a box of the page's own).
  takesClipboard(event) {
    if (event.target instanceof Element && event.target.closest("input, textarea, [contenteditable]")) return false;
    return !this.typing && !document.querySelector(".scrim") && this.env.view.mode === "single";
  }

  // ⌘C or ⌘X with something selected: it goes to the system's clipboard,
  // as itself for a deck and as its words for anything else.
  copied(event, cut) {
    if (!this.takesClipboard(event) || !this.selected) return;
    event.preventDefault();
    this.copy();
    event.clipboardData.setData("text/plain", this.selected.textContent.trim());
    event.clipboardData.setData("text/html", CLIP_MARK + this.clip);
    if (cut) this.remove();
  }

  // Copy from a menu: the system's own copy, which comes back to `copied`.
  copyNow() {
    this.copy();
    document.execCommand("copy");
  }

  paste() {
    if (!this.clip || !this.slide) return;
    const element = this.add(this.clip);
    moveTo(element, placeOf(element), 24, 24);
    this.select(element);
    this.env.commit(`Pasted ${this.label(element)} on slide ${state.current + 1}`);
  }

  // In front of everything else on the slide, or behind it.
  order(front) {
    this.change(front ? "Brought to front" : "Sent to back", (element) => {
      const levels = [...this.slide.querySelectorAll("*")].map((other) => parseInt(getComputedStyle(other).zIndex, 10)).filter(Number.isFinite);
      const level = front ? Math.max(0, ...levels) + 1 : Math.min(0, ...levels) - 1;
      if (getComputedStyle(element).position === "static") element.style.position = "relative";
      element.style.zIndex = String(level);
    });
  }

  // Bold, italic, underline: on the words selected while typing, else on
  // the whole thing.
  format(what) {
    const element = this.selected;
    if (!element) return;
    if (this.typing && hasRange(this.typing)) {
      document.execCommand(what);
      this.commitTyping();
      return;
    }
    const styles = getComputedStyle(element);
    this.change("Restyled", (target) => {
      if (what === "bold") target.style.fontWeight = parseInt(styles.fontWeight, 10) >= 600 ? "400" : "700";
      if (what === "italic") target.style.fontStyle = styles.fontStyle === "italic" ? "normal" : "italic";
      if (what === "underline") target.style.textDecoration = styles.textDecorationLine.includes("underline") ? "none" : "underline";
    });
    this.buildBar();
    this.refresh();
  }

  // Another text style: the element becomes that kind of element.
  restyle(style) {
    const element = this.selected;
    if (!element || kindOf(element) !== "text") return;
    this.stopTyping();
    let target = element;
    if (element.tagName.toLowerCase() !== style.tag && !["UL", "OL"].includes(element.tagName)) {
      target = document.createElement(style.tag);
      for (const attribute of element.attributes) target.setAttribute(attribute.name, attribute.value);
      target.append(...element.childNodes);
      element.replaceWith(target);
    }
    target.classList.remove(...STYLE_CLASSES);
    if (style.class) target.classList.add(style.class);
    this.selected = target;
    this.idOf(target);
    this.env.commit(`Made ${this.label(target)} on slide ${state.current + 1} a ${style.name}`);
    this.buildBar();
    this.refresh();
    emit("selection", target);
  }

  // No list, bullets or numbers.
  listAs(kind) {
    const element = this.selected;
    if (!element) return;
    this.stopTyping();
    const isList = ["UL", "OL"].includes(element.tagName);
    let target = element;
    if (kind === "none" && isList) {
      target = document.createElement("p");
      [...element.children].forEach((item, index) => {
        if (index) target.append(document.createElement("br"));
        target.append(...item.childNodes);
      });
    } else if (kind !== "none") {
      target = document.createElement(kind === "numbers" ? "ol" : "ul");
      if (isList) target.append(...element.childNodes);
      else {
        // Each line of the text becomes an item.
        let item = document.createElement("li");
        for (const node of [...element.childNodes]) {
          if (node.nodeName === "BR") {
            target.append("\n    ", item);
            item = document.createElement("li");
          } else item.append(node);
        }
        target.append("\n    ", item, "\n  ");
      }
    } else return;
    for (const attribute of element.attributes) target.setAttribute(attribute.name, attribute.value);
    if (!isList) target.classList.remove(...STYLE_CLASSES);
    element.replaceWith(target);
    this.selected = target;
    this.idOf(target);
    this.env.commit(`Changed the list of ${this.label(target)} on slide ${state.current + 1}`);
    this.buildBar();
    this.refresh();
    emit("selection", target);
  }

  // ---- Typing in place ---------------------------------------------------------

  startTyping(element, { within, fresh = false } = {}) {
    this.stopTyping();
    if (within) this.selected = within;
    else if (this.selected !== element) this.select(element);
    this.typing = element;
    this.typingFresh = fresh;
    element.setAttribute("contenteditable", "true");
    element.setAttribute("spellcheck", "false");
    element.focus({ preventScroll: true });
    selectAll(element);
    const stop = (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        this.stopTyping();
      } else if ((event.metaKey || event.ctrlKey) && (event.key === "z" || event.key === "Z")) {
        // The words' own undo, asked for here: in the app's window no
        // menu sends it, as a browser's does.
        event.preventDefault();
        document.execCommand(event.shiftKey ? "redo" : "undo");
      } else if (event.key === "Tab" && within) {
        // To the next cell of the table (Shift: the one before).
        event.preventDefault();
        const cells = [...within.querySelectorAll("td, th")];
        const next = cells[cells.indexOf(element) + (event.shiftKey ? -1 : 1)];
        if (next) this.startTyping(next, { within });
        else this.stopTyping();
      } else if (event.key === "Enter" && !event.shiftKey && !element.closest("ul, ol, pre") && !["UL", "OL", "PRE"].includes(element.tagName)) {
        // A new line in the same box, not a new box beside it.
        event.preventDefault();
        if (within) this.stopTyping();
        else insertText(element, "\n", true);
      }
      event.stopPropagation();
    };
    const paste = (event) => {
      // What is pasted comes as plain words, in the style of where it lands.
      event.preventDefault();
      event.stopPropagation();
      insertText(element, event.clipboardData.getData("text/plain"), false);
      this.commitTyping();
      this.refresh();
    };
    const typed = () => {
      this.commitTyping();
      this.refresh();
    };
    const blurred = () => {
      // A press on the bar keeps it; anything else ends it.
      setTimeout(() => {
        if (this.typing === element && document.activeElement !== element && !rootActive(element)) this.stopTyping();
      }, 0);
    };
    element.addEventListener("keydown", stop);
    element.addEventListener("paste", paste);
    element.addEventListener("input", typed);
    element.addEventListener("blur", blurred);
    this.typingOff = () => {
      element.removeEventListener("keydown", stop);
      element.removeEventListener("paste", paste);
      element.removeEventListener("input", typed);
      element.removeEventListener("blur", blurred);
    };
    this.buildBar();
    this.refresh();
  }

  stopTyping() {
    const element = this.typing;
    if (!element) return;
    this.typing = null;
    this.typingOff?.();
    element.removeAttribute("contenteditable");
    element.removeAttribute("spellcheck");
    getSelectionOf(element)?.removeAllRanges();
    // A text box added and left empty goes away again.
    if (this.typingFresh && !element.textContent.trim() && element.isConnected) {
      element.remove();
      if (this.selected === element) this.selected = null;
    }
    this.typingFresh = false;
    dress(this.slide);
    this.env.commit(`Edited text on slide ${state.current + 1}`);
    this.buildBar();
    this.refresh();
  }

  // ---- Adding things -----------------------------------------------------------

  // Puts `html` (one element) on the slide and answers with the element.
  add(html) {
    const template = document.createElement("template");
    template.innerHTML = html.trim();
    const element = template.content.firstElementChild;
    element.setAttribute("data-id", newId(this.slide));
    const last = this.slide.lastChild;
    if (last?.nodeType === 3 && !last.textContent.trim()) last.remove();
    this.slide.append("\n  ", element, "\n");
    dress(this.slide);
    return element;
  }

  // A place for a new thing of `width` by `height`: the middle of the slide,
  // a little lower and to the right for each thing already put there.
  spot(width, height) {
    const [w, h] = state.deck.size;
    const taken = [...this.slide.children].filter((child) => getComputedStyle(child).position === "absolute").length;
    const shift = (taken % 8) * 28;
    return `position:absolute;left:${Math.round((w - width) / 2 + shift)}px;top:${Math.round((h - height) / 2 + shift)}px`;
  }

  async insert(kind, options = {}) {
    if (!this.slide) {
      toast("Add a slide first.");
      return;
    }
    const number = state.current + 1;
    if (kind === "text") {
      const element = this.add(`<p style="${this.spot(760, 60)};width:760px;margin:0">Text</p>`);
      this.select(element);
      this.startTyping(element, { fresh: true });
      return;
    }
    if (kind === "shape") {
      const shape = options.shape || "rect";
      const line = shape === "line" || shape === "connector";
      const size = line ? [480, 6] : shape.startsWith("arrow") ? [360, 200] : [360, 240];
      const element = this.add(`<div data-shape="${shape}" style="${this.spot(...size)};width:${size[0]}px;height:${size[1]}px"></div>`);
      this.select(element);
      this.env.commit(`Added a ${SHAPES.find((one) => one.id === shape)?.name.toLowerCase() || "shape"} to slide ${number}`);
      return;
    }
    if (kind === "table") {
      const row = (cell, values) => `<tr>${values.map((value) => `<${cell}>${value}</${cell}>`).join("")}</tr>`;
      const element = this.add(
        `<table style="${this.spot(1200, 320)};width:1200px"><thead>${row("th", ["Item", "Status", "Owner"])}</thead><tbody>${row("td", ["First", "", ""])}${row("td", ["Second", "", ""])}</tbody></table>`,
      );
      this.select(element);
      this.env.commit(`Added a table to slide ${number}`);
      return;
    }
    if (kind === "chart") {
      const spec = { type: "bar", labels: ["A", "B", "C", "D"], series: [{ name: "Series 1", values: [12, 18, 9, 22] }] };
      const element = this.add(`<figure data-chart='${JSON.stringify(spec).replace(/'/g, "&#39;")}' style="${this.spot(960, 540)};width:960px;height:540px;margin:0"></figure>`);
      this.select(element);
      this.env.commit(`Added a chart to slide ${number}`);
      this.editChart(element);
      return;
    }
    if (kind === "image" || kind === "video") {
      const file = options.file || (await pickFile(kind === "image" ? "image/*" : "video/*,audio/*"));
      if (file) await this.addMedia(file, options.at);
    }
  }

  // Keeps a picture or video with the deck and puts it on the slide.
  async addMedia(file, at) {
    const isVideo = /^(video|audio)\//.test(file.type) || /\.(mp4|webm|mov|m4v|mp3|wav|m4a)$/i.test(file.name);
    let path;
    try {
      path = await store.upload(file);
    } catch (error) {
      toast(error.message, { kind: "error" });
      return;
    }
    const [w, hgt] = state.deck.size;
    let width = Math.round(w * 0.5);
    let height = Math.round((width * 9) / 16);
    if (!isVideo) {
      const size = await imageSize(path);
      if (size) {
        const scale = Math.min(1, (w * 0.6) / size.width, (hgt * 0.6) / size.height);
        width = Math.round(size.width * scale);
        height = Math.round(size.height * scale);
      }
    }
    const where = at
      ? `position:absolute;left:${Math.round(at.x - width / 2)}px;top:${Math.round(at.y - height / 2)}px`
      : this.spot(width, height);
    const name = (file.name || "").replace(/\.[^.]*$/, "").replace(/[-_]+/g, " ").replace(/["<>&]/g, "");
    const element = isVideo
      ? this.add(`<video src="${path}" controls style="${where};width:${width}px;height:${height}px"></video>`)
      : this.add(`<img src="${path}" alt="${name}" style="${where};width:${width}px;height:${height}px">`);
    this.select(element);
    this.env.commit(`Added a ${isVideo ? "video" : "picture"} to slide ${state.current + 1}`);
  }

  async replaceMedia(element) {
    const video = kindOf(element) === "video";
    const file = await pickFile(video ? "video/*,audio/*" : "image/*");
    if (!file) return;
    try {
      const path = await store.upload(file);
      this.select(element);
      this.change("Replaced", (target) => target.setAttribute("src", path));
    } catch (error) {
      toast(error.message, { kind: "error" });
    }
  }

  dropped(event) {
    const files = [...(event.dataTransfer?.files || [])];
    if (files.length === 0 || !this.slide) return;
    event.preventDefault();
    const frame = this.env.el.frame.getBoundingClientRect();
    const at = { x: (event.clientX - frame.x) / this.scale, y: (event.clientY - frame.y) / this.scale };
    files.forEach((file, index) => this.addMedia(file, { x: at.x + index * 32, y: at.y + index * 32 }));
  }

  pasted(event) {
    if (!this.takesClipboard(event)) return;
    const files = [...(event.clipboardData?.files || [])].filter((file) => /^(image|video)\//.test(file.type));
    if (files.length) {
      event.preventDefault();
      files.forEach((file) => this.addMedia(file));
      return;
    }
    // Something copied from a slide (this deck's or another's), cleaned as
    // a slide's file is when it is read.
    const html = event.clipboardData?.getData("text/html") || "";
    const marked = html.indexOf(CLIP_MARK);
    const copied = marked < 0 ? null : parseSlide(`<section>${html.slice(marked + CLIP_MARK.length)}</section>`).firstElementChild;
    if (copied && this.slide) {
      event.preventDefault();
      this.clip = copied.outerHTML;
      this.paste();
      return;
    }
    const text = event.clipboardData?.getData("text/plain").trim();
    if (text && this.slide) {
      event.preventDefault();
      const element = this.add(`<p style="${this.spot(900, 80)};width:900px;margin:0"></p>`);
      element.textContent = text;
      this.select(element);
      this.env.commit(`Pasted text on slide ${state.current + 1}`);
    }
  }

  // ---- Tables and charts -------------------------------------------------------

  tableSize(table) {
    const rows = [...table.querySelectorAll("tr")];
    return { rows: rows.length, columns: Math.max(0, ...rows.map((row) => row.children.length)) };
  }

  tableRows(by) {
    this.change(by > 0 ? "Added a row to" : "Removed a row from", (table) => {
      const body = table.tBodies[table.tBodies.length - 1] || table;
      const rows = [...table.querySelectorAll("tr")];
      const last = rows[rows.length - 1];
      if (by > 0) {
        const row = document.createElement("tr");
        const count = last ? last.children.length : 3;
        for (let index = 0; index < count; index += 1) row.append(document.createElement("td"));
        body.append(row);
      } else if (rows.length > 1) last.remove();
    });
    this.buildBar();
    this.refresh();
  }

  tableColumns(by) {
    this.change(by > 0 ? "Added a column to" : "Removed a column from", (table) => {
      for (const row of table.querySelectorAll("tr")) {
        if (by > 0) row.append(document.createElement(row.lastElementChild?.tagName === "TH" ? "th" : "td"));
        else if (row.children.length > 1) row.lastElementChild.remove();
      }
    });
    this.buildBar();
    this.refresh();
  }

  // The chart's numbers in a small sheet: a row a label, a column a series.
  editChart(element) {
    const spec = chartSpec(element) || { type: "bar", labels: ["A"], series: [{ name: "Series 1", values: [1] }], unit: "" };
    const sheet = h("div", { class: "sheet" });
    const cell = (value, placeholder) => h("input", { type: "text", class: "input", value, placeholder, style: { width: "100%" } });
    const draw = () => {
      sheet.replaceChildren(
        h(
          "div",
          { class: "sheet-grid", style: { display: "grid", gap: "4px", gridTemplateColumns: `minmax(90px,1.4fr) repeat(${spec.series.length}, minmax(70px,1fr))` } },
          h("span", {}),
          spec.series.map((series, s) => {
            const input = cell(series.name, "Series");
            input.addEventListener("input", () => (series.name = input.value));
            return input;
          }),
          spec.labels.flatMap((label, row) => {
            const name = cell(label, "Label");
            name.addEventListener("input", () => (spec.labels[row] = name.value));
            return [
              name,
              ...spec.series.map((series) => {
                const input = cell(series.values[row] ?? 0, "0");
                input.inputMode = "decimal";
                input.addEventListener("input", () => (series.values[row] = parseFloat(input.value) || 0));
                return input;
              }),
            ];
          }),
        ),
      );
    };
    draw();
    const kinds = [
      ["bar", "Columns"],
      ["hbar", "Bars"],
      ["stacked", "Stacked columns"],
      ["line", "Line"],
      ["area", "Area"],
      ["pie", "Pie"],
      ["donut", "Doughnut"],
    ];
    const kind = h("select", { class: "input select" }, kinds.map(([id, name]) => h("option", { value: id, selected: id === spec.type }, name)));
    kind.addEventListener("change", () => (spec.type = kind.value));
    const unit = h("input", { type: "text", class: "input", value: spec.unit, placeholder: "%", style: { width: "70px" } });
    unit.addEventListener("input", () => (spec.unit = unit.value));
    const row = (by) => () => {
      if (by > 0) {
        spec.labels.push("");
        spec.series.forEach((series) => series.values.push(0));
      } else if (spec.labels.length > 1) {
        spec.labels.pop();
        spec.series.forEach((series) => series.values.pop());
      }
      draw();
    };
    const column = (by) => () => {
      if (by > 0) spec.series.push({ name: `Series ${spec.series.length + 1}`, values: spec.labels.map(() => 0) });
      else if (spec.series.length > 1) spec.series.pop();
      draw();
    };
    const box = dialog({
      title: "Chart",
      width: 560,
      content: h(
        "div",
        { style: { display: "flex", flexDirection: "column", gap: "12px" } },
        h("div", { style: { display: "flex", gap: "8px", alignItems: "center" } }, kind, h("span", { style: { color: "var(--ui-muted)" } }, "Unit"), unit),
        sheet,
        h(
          "div",
          { style: { display: "flex", gap: "6px" } },
          button({ label: "Row", icon: "plus", kind: "outline", onClick: row(1) }),
          button({ label: "Row", icon: "minus", kind: "outline", onClick: row(-1) }),
          button({ label: "Series", icon: "plus", kind: "outline", onClick: column(1) }),
          button({ label: "Series", icon: "minus", kind: "outline", onClick: column(-1) }),
        ),
      ),
      actions: [
        button({ label: "Cancel", onClick: () => box.close() }),
        button({
          label: "Done",
          kind: "primary",
          onClick: () => {
            box.close();
            this.select(element);
            this.change("Changed the numbers of", (chart) => {
              const clean = { type: spec.type, labels: spec.labels, series: spec.series };
              if (spec.unit) clean.unit = spec.unit;
              chart.setAttribute("data-chart", JSON.stringify(clean));
            });
          },
        }),
      ],
    });
  }

  // ---- The bar at the selection ---------------------------------------------------

  buildBar() {
    this.bar?.remove();
    this.bar = null;
    const element = this.selected;
    if (!element) return;
    const kind = kindOf(element);
    // A press on the bar does not take the caret out of the text.
    const keep = (control) => {
      control.addEventListener("pointerdown", (event) => event.preventDefault());
      return control;
    };
    const items = [];
    const rule = () => h("span", { class: "bar-rule" });
    const colours = () => palette(this.slide);
    const colourButton = (title, property, value) => {
      const chip = h("span", { class: "chip" }, h("i", { style: { background: value } }));
      const control = h("button", { type: "button", class: "btn btn-icon", title, "aria-label": title }, chip);
      control.addEventListener("click", () =>
        colourPicker(control, {
          value,
          palette: colours(),
          allowNone: property !== "color",
          onPick: (colour) => {
            if (property === "color" && this.typing && hasRange(this.typing)) {
              document.execCommand("styleWithCSS", false, true);
              document.execCommand("foreColor", false, hex(resolve(this.slide, colour)));
              this.commitTyping();
            } else {
              this.change("Recoloured", (target) => {
                if (colour) target.style.setProperty(property, colour);
                else target.style.setProperty(property, "transparent");
              });
            }
            chip.firstChild.style.background = colour || "transparent";
          },
        }),
      );
      return keep(control);
    };
    const styles = getComputedStyle(element);
    if (kind === "text") {
      const style = textStyleOf(element);
      const list = ["UL", "OL"].includes(element.tagName);
      const pick = keep(button({ icon: "text", label: list ? "List" : style?.name || "Text", caret: true, title: "Text style" }));
      pick.addEventListener("click", () =>
        menu(
          pick,
          TEXT_STYLES.map((one) => ({ label: one.name, checked: !list && one.id === style?.id, onSelect: () => this.restyle(one) })),
          { keep: false },
        ),
      );
      const bold = parseInt(styles.fontWeight, 10) >= 600;
      const lists = keep(button({ icon: element.tagName === "OL" ? "numbered" : "list", title: "List", pressed: list }));
      lists.addEventListener("click", () =>
        menu(lists, [
          { label: "No list", checked: !list, onSelect: () => this.listAs("none") },
          { label: "Bullets", icon: "list", onSelect: () => this.listAs("bullets") },
          { label: "Numbers", icon: "numbered", onSelect: () => this.listAs("numbers") },
        ]),
      );
      const aligned = styles.textAlign === "start" ? "left" : styles.textAlign;
      const icons = { left: "alignLeft", center: "alignCenter", right: "alignRight", justify: "alignJustify" };
      const align = keep(button({ icon: icons[aligned] || "alignLeft", title: "Align" }));
      align.addEventListener("click", () =>
        menu(
          align,
          Object.entries({ left: "Left", center: "Centre", right: "Right", justify: "Justified" }).map(([value, label]) => ({
            label,
            icon: icons[value],
            onSelect: () => {
              this.change("Aligned", (target) => (target.style.textAlign = value));
              this.buildBar();
              this.refresh();
            },
          })),
        ),
      );
      items.push(
        pick,
        colourButton("Text colour", "color", styles.color),
        keep(button({ icon: "bold", title: "Bold (" + keyHint("⌘B") + ")", pressed: bold, onClick: () => this.format("bold") })),
        keep(button({ icon: "italic", title: "Italic (" + keyHint("⌘I") + ")", pressed: styles.fontStyle === "italic", onClick: () => this.format("italic") })),
        keep(button({ icon: "underline", title: "Underline (" + keyHint("⌘U") + ")", pressed: styles.textDecorationLine.includes("underline"), onClick: () => this.format("underline") })),
        keep(button({ icon: "link", title: "Link", onClick: (_, anchor) => this.link(anchor) })),
        rule(),
        lists,
        align,
      );
    } else if (kind === "image" || kind === "video") {
      items.push(button({ icon: "replace", title: "Replace", onClick: () => this.replaceMedia(element) }));
      if (kind === "image") {
        items.push(
          button({
            icon: "crop",
            title: "Crop",
            pressed: this.cropping,
            onClick: () => {
              this.cropping = !this.cropping;
              this.buildBar();
              this.refresh();
            },
          }),
        );
        const corners = button({ icon: "corner", title: "Corners" });
        corners.addEventListener("click", () =>
          menu(
            corners,
            [
              ["Square", "0"],
              ["Slightly round", "12px"],
              ["Round", "28px"],
              ["Very round", "64px"],
              ["Circle", "50%"],
            ].map(([label, value]) => ({ label, onSelect: () => this.change("Rounded", (target) => (target.style.borderRadius = value)) })),
          ),
        );
        items.push(corners);
      }
    } else if (kind === "table") {
      const size = this.tableSize(element);
      const stepper = (name, count, change) =>
        h(
          "span",
          { class: "stepper", style: { display: "inline-flex", alignItems: "center", gap: "2px" } },
          button({ icon: "minus", title: `One ${name} fewer`, onClick: () => change(-1) }),
          h("span", { style: { minWidth: "44px", textAlign: "center", color: "var(--ui-muted)" } }, `${count} ${name}${count === 1 ? "" : "s"}`),
          button({ icon: "plus", title: `One ${name} more`, onClick: () => change(1) }),
        );
      items.push(stepper("row", size.rows, (by) => this.tableRows(by)), rule(), stepper("column", size.columns, (by) => this.tableColumns(by)));
    } else if (kind === "chart") {
      items.push(button({ icon: "chart", label: "Numbers", title: "Change the chart's numbers", onClick: () => this.editChart(element) }));
    } else {
      const line = ["line", "connector"].includes(element.getAttribute("data-shape"));
      items.push(colourButton(line ? "Colour" : "Fill", "background", styles.backgroundColor));
      if (kind === "shape" && !line) items.push(colourButton("Text colour", "color", styles.color));
    }
    if (kind !== "text") {
      items.push(
        rule(),
        button({ icon: "front", title: "Bring to front", onClick: () => this.order(true) }),
        button({ icon: "back", title: "Send to back", onClick: () => this.order(false) }),
      );
    }
    items.push(
      rule(),
      keep(button({ icon: "comment", title: "Comment on this", onClick: () => this.env.comment(element) })),
      keep(button({ icon: "more", title: "How it looks", onClick: () => this.env.setPanel(true) })),
    );
    this.bar = h("div", { class: "bar selection-bar", role: "toolbar", "aria-label": "The selected " + (kind === "text" ? "text" : kind) }, items);
  }

  // A link on the words selected while typing, or on the whole text.
  link(anchor) {
    const element = this.selected;
    const existing = element.closest("a") || element.querySelector("a");
    const input = h("input", { type: "url", class: "input", placeholder: "https://…", value: existing?.getAttribute("href") || "", style: { width: "260px" } });
    const range = this.typing && hasRange(this.typing) ? rangeOf(this.typing).cloneRange() : null;
    const apply = () => {
      const url = input.value.trim();
      float.close();
      if (/^\s*javascript:/i.test(url)) return;
      if (range) {
        this.typing.focus({ preventScroll: true });
        setRange(this.typing, range);
        document.execCommand(url ? "createLink" : "unlink", false, url);
        this.commitTyping();
        return;
      }
      this.change(url ? "Linked" : "Unlinked", (target) => {
        for (const old of target.querySelectorAll("a")) old.replaceWith(...old.childNodes);
        if (url) {
          const link = document.createElement("a");
          link.setAttribute("href", url);
          link.append(...target.childNodes);
          target.append(link);
        }
      });
    };
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") apply();
    });
    const float = popover(anchor, h("div", { style: { display: "flex", gap: "6px" } }, input, button({ label: "Apply", kind: "primary", onClick: apply })), { keep: true });
    input.focus();
  }
}

// ---- Small things ---------------------------------------------------------------

function debounce(work, wait) {
  let timer = 0;
  return () => {
    clearTimeout(timer);
    timer = setTimeout(work, wait);
  };
}

const clamp = (value, low, high) => Math.max(low, Math.min(high, value));
const place = (box) => ({ left: box.x + "px", top: box.y + "px", width: box.width + "px", height: box.height + "px" });

// An id no element of the slide has: e1, e2, …
export function newId(slide) {
  let most = 0;
  for (const element of slide.querySelectorAll("[data-id]")) {
    const number = parseInt(element.getAttribute("data-id").replace(/^e/, ""), 10);
    if (Number.isFinite(number)) most = Math.max(most, number);
  }
  return "e" + (most + 1);
}

// Where an element can be found from its slide: `div.cols > div:nth-of-type(2) > h3`.
function pathOf(element, slide) {
  const parts = [];
  for (let node = element; node && node !== slide; node = node.parentElement) {
    const tag = node.tagName.toLowerCase();
    const first = node.classList[0] ? "." + node.classList[0] : "";
    const same = [...node.parentElement.children].filter((other) => other.tagName === node.tagName);
    const nth = same.length > 1 ? `:nth-of-type(${same.indexOf(node) + 1})` : "";
    parts.unshift(tag + first + nth);
  }
  return parts.join(" > ");
}

// How an element is placed: at a spot of its own, or by an offset from
// where the slide's layout puts it.
function placeOf(element) {
  const styles = getComputedStyle(element);
  if (styles.position === "absolute" || styles.position === "fixed") {
    return { absolute: true, x: parseFloat(styles.left) || 0, y: parseFloat(styles.top) || 0 };
  }
  const [x = "0", y = "0"] = styles.translate && styles.translate !== "none" ? styles.translate.split(" ") : [];
  return { absolute: false, x: parseFloat(x) || 0, y: parseFloat(y) || 0 };
}

// Moves an element by (dx, dy), in the slide's pixels, from where `from`
// says it was.
function moveTo(element, from, dx, dy) {
  const x = Math.round(from.x + dx);
  const y = Math.round(from.y + dy);
  if (from.absolute) {
    element.style.left = x + "px";
    element.style.top = y + "px";
    element.style.right = "auto";
    element.style.bottom = "auto";
  } else if (x === 0 && y === 0) element.style.removeProperty("translate");
  else element.style.translate = `${x}px ${y}px`;
}

// How much of a picture is cut away on each side, in percent.
function cropOf(element) {
  const found = /inset\(([^)]*)\)/.exec(element.style.clipPath || "");
  const parts = found ? found[1].split(/\s+/).map((part) => parseFloat(part) || 0) : [];
  const [top = 0, right = top, bottom = top, left = right] = parts;
  return { top, right, bottom, left };
}

function setCrop(element, crop) {
  const round = (value) => +value.toFixed(2);
  if (crop.top + crop.right + crop.bottom + crop.left < 0.05) element.style.removeProperty("clip-path");
  else element.style.clipPath = `inset(${round(crop.top)}% ${round(crop.right)}% ${round(crop.bottom)}% ${round(crop.left)}%)`;
}

// The selection in the slide an element is in. A slide is drawn in a
// shadow root, whose selection browsers hand out in two ways.
// The selection, where `element` is. A slide is in a shadow tree: Chrome
// keeps that tree's selection at its root, WebKit at the document (and
// hands its ranges out only through `getComposedRanges`: see `rangeOf`).
function getSelectionOf(element) {
  const root = element.getRootNode();
  return typeof root.getSelection === "function" ? root.getSelection() : document.getSelection();
}

// What is selected in `element`'s tree, as a range; null with nothing.
function rangeOf(element) {
  const selection = getSelectionOf(element);
  if (!selection) return null;
  const root = element.getRootNode();
  if (typeof root.getSelection !== "function" && root instanceof ShadowRoot && typeof selection.getComposedRanges === "function") {
    // Asked both ways it has been spelled: with the roots in a list, and
    // one by one.
    let found = null;
    try {
      found = selection.getComposedRanges({ shadowRoots: [root] })[0];
    } catch {
      found = null;
    }
    if (!found || !root.contains(found.startContainer)) found = selection.getComposedRanges(root)[0];
    if (!found || !root.contains(found.startContainer)) return null;
    const range = document.createRange();
    range.setStart(found.startContainer, found.startOffset);
    range.setEnd(found.endContainer, found.endOffset);
    return range;
  }
  return selection.rangeCount ? selection.getRangeAt(0) : null;
}

// Selects `range`. By its ends, which reaches into a shadow tree in every
// browser; `addRange` does not in WebKit, and leaves nothing selected.
function setRange(element, range) {
  getSelectionOf(element)?.setBaseAndExtent(range.startContainer, range.startOffset, range.endContainer, range.endOffset);
}

function hasRange(element) {
  const range = rangeOf(element);
  return Boolean(range && !range.collapsed);
}

function rootActive(element) {
  const root = element.getRootNode();
  return root.activeElement === element;
}

function selectAll(element) {
  const range = document.createRange();
  range.selectNodeContents(element);
  setRange(element, range);
}

// Puts text where the caret is, in place of what is selected. A line break
// becomes one in the box (`br`), not a new paragraph. Done as the browser's
// own typing is, so that its undo takes it back.
function insertText(element, text, single) {
  if (!rangeOf(element)) return;
  const lines = single ? ["", ""] : text.replace(/\r/g, "").split("\n");
  lines.forEach((line, index) => {
    if (index) document.execCommand("insertLineBreak");
    if (line) document.execCommand("insertText", false, line);
  });
}

// What a CSS colour of the deck's (`var(--accent)`) comes to on the slide.
function resolve(slide, colour) {
  const probe = document.createElement("span");
  probe.style.color = colour;
  slide.append(probe);
  const value = getComputedStyle(probe).color;
  probe.remove();
  return value;
}

function pickFile(accept) {
  return new Promise((done) => {
    const input = h("input", { type: "file", accept, style: { display: "none" } });
    document.body.append(input);
    input.addEventListener("change", () => {
      done(input.files[0] || null);
      input.remove();
    });
    input.addEventListener("cancel", () => {
      done(null);
      input.remove();
    });
    input.click();
  });
}

function imageSize(path) {
  return new Promise((done) => {
    const image = new Image();
    image.onload = () => done({ width: image.naturalWidth, height: image.naturalHeight });
    image.onerror = () => done(null);
    image.src = path;
  });
}

export { icon };
