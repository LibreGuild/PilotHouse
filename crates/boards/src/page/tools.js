// What the pointer and the keys do on the board: moving over it, picking
// things, moving and sizing them, pulling lines between them, and making
// new ones with the tool in hand.

import { toast } from "/_artifact/kit/ui.js";
import { state, elements, byId, selected, select, setTool, setView, begin, commit, cancel, change, newId, undo, redo, upload, emit } from "./model.js";
import { el, toBoard, viewSize, elementAt, edit, stopEditing, marks, extra, draw, drawSoon } from "./draw.js";
import { box, union, overlaps, fresh, ties, inFrame, ends, loosen } from "./shapes.js";

// ---- Looking at the board ---------------------------------------------------

// Scales the view by `factor`, keeping the board point under `at` (a point
// of the canvas; its middle when left out) where it is.
export function zoomBy(factor, at) {
  const size = viewSize();
  const point = at || { x: size.w / 2, y: size.h / 2 };
  const scale = Math.max(0.05, Math.min(8, state.view.scale * factor));
  const bx = point.x / state.view.scale + state.view.x;
  const by = point.y / state.view.scale + state.view.y;
  setView({ scale, x: bx - point.x / scale, y: by - point.y / scale });
}

export function zoomTo(scale) {
  zoomBy(scale / state.view.scale);
}

// Shows all of `b` (a box of the board), with room around it.
export function show(b, { most = 1.5, pad = 80 } = {}) {
  const size = viewSize();
  if (!size.w || !size.h) return;
  const scale = Math.max(0.05, Math.min(most, (size.w - pad * 2) / Math.max(b.w, 1), (size.h - pad * 2) / Math.max(b.h, 1)));
  setView({ scale, x: b.x + b.w / 2 - size.w / scale / 2, y: b.y + b.h / 2 - size.h / scale / 2 });
}

export function fit() {
  if (elements().length === 0) {
    const size = viewSize();
    setView({ scale: 1, x: -size.w / 2, y: -size.h / 2 });
  } else show(union(elements()), { most: 1 });
}

// The board point in the middle of what is seen.
export function middle() {
  const size = viewSize();
  return { x: state.view.x + size.w / state.view.scale / 2, y: state.view.y + size.h / state.view.scale / 2 };
}

// ---- Making things ------------------------------------------------------------

// Puts `made` (elements without ids, or with) on the board as one step,
// and selects them.
export function add(made, { kind = "" } = {}) {
  const list = Array.isArray(made) ? made : [made];
  begin();
  for (const element of list) {
    if (!element.id) element.id = newId(element.type?.[0] || "e");
    // Sections lie under what they hold: they go to the back of the list.
    if (element.type === "frame") {
      const first = elements().findIndex((one) => one.type !== "frame");
      elements().splice(first < 0 ? elements().length : first, 0, element);
    } else elements().push(element);
  }
  commit(kind);
  select(list.map((element) => element.id));
  return list;
}

// What the tool in hand makes at `at`, sized `size` when it was dragged out.
function make(at, size) {
  const { tool, last } = state;
  if (tool === "sticky") return fresh("sticky", at, { color: last.sticky, size });
  if (tool === "text") return fresh("text", at, { size: size && [Math.max(size[0], 80), 30] });
  if (tool === "shape") return fresh("shape", at, { shape: last.shape, fill: last.fill, size });
  if (tool === "frame") return fresh("frame", at, { size });
  if (tool === "table") return fresh("table", at, { size });
  if (tool === "sticker") return fresh("sticker", at, { emoji: last.emoji, size: size && [Math.max(...size), Math.max(...size)] });
  return null;
}

export function removeSelected() {
  const going = selected().filter((element) => !element.locked);
  if (going.length === 0) return;
  const ids = new Set(going.map((element) => element.id));
  change("", () => {
    state.board.elements = elements().filter((element) => !ids.has(element.id));
    // A line tied to what went stays where it was, loose.
    loosen(elements(), going);
  });
}

// Copies of `list` moved by (dx, dy), with ids of their own; lines between
// copies are tied to the copies.
export function copies(list, dx, dy) {
  const taken = new Set(elements().map((element) => element.id));
  const ids = new Map();
  for (const element of list) {
    // Several at once, in the same moment: each an id no other has.
    let id;
    do id = newId(element.type?.[0] || "e");
    while (taken.has(id));
    taken.add(id);
    ids.set(element.id, id);
  }
  const groups = new Map();
  return list.map((element) => {
    const copy = JSON.parse(JSON.stringify(element));
    copy.id = ids.get(element.id);
    if (copy.group) {
      if (!groups.has(copy.group)) groups.set(copy.group, newId("g"));
      copy.group = groups.get(copy.group);
    }
    for (const key of ["from", "to"]) {
      const spec = copy[key];
      if (!spec?.id) continue;
      if (ids.has(spec.id)) spec.id = ids.get(spec.id);
      // Tied to what is not copied with it: the copy's end is loose,
      // where the first's is (it then moves with the copy; on another
      // board there is nothing of that id to tie to).
      else copy[key] = { x: Math.round(spec.x ?? 0), y: Math.round(spec.y ?? 0) };
    }
    shift(copy, dx, dy);
    return copy;
  });
}

// `list` as it is copied from: a line's ends tied to what is not in the
// list are noted as points, where they are now.
function withEnds(list) {
  const ids = new Set(list.map((element) => element.id));
  return list.map((element) => {
    if (element.type !== "line") return element;
    const [a, b] = ends(element);
    const copy = { ...element };
    for (const [key, point] of [["from", a], ["to", b]]) {
      if (copy[key]?.id && !ids.has(copy[key].id)) copy[key] = { ...copy[key], x: Math.round(point.x), y: Math.round(point.y) };
    }
    return copy;
  });
}

export function duplicate() {
  const list = selected();
  if (list.length) add(copies(withEnds(list), 24, 24));
}

// Moves an element by (dx, dy). A line moves its loose ends; a tied end
// follows what it is tied to.
function shift(element, dx, dy) {
  if (element.type === "line") {
    for (const key of ["from", "to"]) {
      if (element[key] && !element[key].id) {
        element[key].x = Math.round(element[key].x + dx);
        element[key].y = Math.round(element[key].y + dy);
      }
    }
  } else if (element.type === "draw") {
    element.points = (element.points || []).map(([x, y]) => [Math.round((x + dx) * 10) / 10, Math.round((y + dy) * 10) / 10]);
  } else {
    element.x = Math.round(element.x + dx);
    element.y = Math.round(element.y + dy);
  }
}

// What moves when `list` is moved: the list, and what lies in a section
// of it.
function moving(list) {
  const all = new Set(list);
  for (const frame of list.filter((element) => element.type === "frame")) {
    for (const inside of inFrame(frame, elements())) if (!inside.locked) all.add(inside);
  }
  return [...all].filter((element) => !element.locked);
}

// ---- Snapping -------------------------------------------------------------------

// How far (dx, dy) may be changed so an edge or the middle of the moving
// box meets one of something else in sight; and the lines that shows.
function snap(moved, others) {
  const reach = 6 / state.view.scale;
  const stops = (b) => ({ x: [b.x, b.x + b.w / 2, b.x + b.w], y: [b.y, b.y + b.h / 2, b.y + b.h] });
  const mine = stops(moved);
  const best = { x: null, y: null };
  for (const other of others) {
    const theirs = stops(other);
    for (const axis of ["x", "y"]) {
      for (const a of mine[axis]) {
        for (const b of theirs[axis]) {
          const by = b - a;
          if (Math.abs(by) <= reach && (best[axis] === null || Math.abs(by) < Math.abs(best[axis].by))) best[axis] = { by, at: b };
        }
      }
    }
  }
  return {
    dx: best.x?.by || 0,
    dy: best.y?.by || 0,
    guides: [best.x ? { x: best.x.at } : null, best.y ? { y: best.y.at } : null].filter(Boolean),
  };
}

// The boxes in sight that what moves may line up with: not `moved`
// itself (ids), and not more than can be gone through at every move.
function inSight(moved) {
  const size = viewSize();
  const seen = { x: state.view.x, y: state.view.y, w: size.w / state.view.scale, h: size.h / state.view.scale };
  return elements()
    .filter((element) => !moved.has(element.id) && element.type !== "line" && element.type !== "draw" && element.type !== "comment")
    .map(box)
    .filter((b) => overlaps(b, seen))
    .slice(0, 200);
}

// ---- The pointer -------------------------------------------------------------------

let space = false;
// What the pointer is doing between a press and a release.
let doing = null;

const DRAWS = new Set(["sticky", "text", "shape", "frame", "table", "sticker"]);

export function wire() {
  const canvas = el.canvas;
  canvas.addEventListener("wheel", wheel, { passive: false });
  canvas.addEventListener("pointerdown", down);
  canvas.addEventListener("pointermove", move);
  canvas.addEventListener("pointerup", up);
  canvas.addEventListener("pointercancel", up);
  canvas.addEventListener("dblclick", twice);
  canvas.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    const hit = elementAt(event.target);
    if (hit && !state.selection.has(hit.id)) select([hit.id]);
    if (!hit) select([]);
    emit("menu", { x: event.clientX, y: event.clientY, hit });
  });
  // A pinch on a trackpad, where the browser says it so (WebKit: the
  // app's own window on a Mac). Elsewhere it comes as the wheel with Ctrl.
  let pinched = 1;
  canvas.addEventListener("gesturestart", (event) => {
    event.preventDefault();
    pinched = event.scale || 1;
  });
  canvas.addEventListener("gesturechange", (event) => {
    event.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const scale = event.scale || 1;
    zoomBy(scale / pinched, { x: event.clientX - rect.left, y: event.clientY - rect.top });
    pinched = scale;
  });
  canvas.addEventListener("gestureend", (event) => event.preventDefault());
  document.addEventListener("keydown", keyDown);
  const letGo = () => {
    space = false;
    canvas.classList.remove("panning");
  };
  document.addEventListener("keyup", (event) => event.code === "Space" && letGo());
  // Space let go of while another window had the keys is never heard of.
  window.addEventListener("blur", letGo);
  document.addEventListener("paste", paste);
  document.addEventListener("copy", (event) => copied(event, false));
  document.addEventListener("cut", (event) => copied(event, true));
  canvas.addEventListener("dragover", (event) => event.preventDefault());
  canvas.addEventListener("drop", (event) => {
    event.preventDefault();
    if (!state.owner) return;
    const files = [...(event.dataTransfer?.files || [])].filter((file) => file.type.startsWith("image/"));
    if (files.length) addPictures(files, toBoard(event));
  });
}

function wheel(event) {
  event.preventDefault();
  const rect = el.canvas.getBoundingClientRect();
  if (event.ctrlKey || event.metaKey) {
    // A pinch, or the wheel with Ctrl or ⌘: nearer and farther, about the
    // pointer.
    // A wheel's notch is a hundred or more of these, a pinch's a few: no
    // one event zooms by more than a quarter.
    const by = Math.max(-22, Math.min(22, event.deltaY * (event.deltaMode === 1 ? 5 : 1)));
    zoomBy(Math.exp(-by * 0.01), { x: event.clientX - rect.left, y: event.clientY - rect.top });
    return;
  }
  const lines = event.deltaMode === 1 ? 32 : 1;
  const [dx, dy] = event.shiftKey && !event.deltaX ? [event.deltaY, 0] : [event.deltaX, event.deltaY];
  setView({ x: state.view.x + (dx * lines) / state.view.scale, y: state.view.y + (dy * lines) / state.view.scale });
}

function down(event) {
  if (event.target.closest(".editing, .table.editing")) return;
  // A second finger, or another button, while one is at work: not a
  // second thing to do.
  if (doing) return;
  stopEditing();
  el.canvas.focus({ preventScroll: true });
  const at = toBoard(event);
  const start = { x: event.clientX, y: event.clientY };
  // Moving over the board: the hand, Space held, or the wheel's button.
  if (state.tool === "hand" || space || event.button === 1) {
    doing = { kind: "pan", start, view: { ...state.view } };
    el.canvas.classList.add("grabbing");
  } else if (event.button !== 0) {
    return;
  } else if (!state.owner) {
    doing = { kind: "pan", start, view: { ...state.view } };
  } else if (event.target.classList.contains("handle")) {
    const element = selected()[0];
    begin();
    doing = { kind: "size", at: event.target.dataset.at, element, was: { ...box(element) }, from: at };
  } else if (event.target.classList.contains("tie")) {
    const element = selected()[0];
    doing = { kind: "tie", from: { id: element.id, side: event.target.dataset.side }, start, line: null };
  } else if (event.target.classList.contains("end-handle")) {
    const line = selected()[0];
    begin();
    doing = { kind: "end", line, end: event.target.dataset.end };
  } else if (state.tool === "pen" || state.tool === "marker") {
    begin();
    const marker = state.tool === "marker";
    const stroke = { id: newId("d"), type: "draw", points: [[round(at.x), round(at.y)]], color: marker ? state.last.marker || "yellow" : state.last.pen, width: marker ? 18 : state.last.penWidth };
    if (marker) stroke.marker = true;
    elements().push(stroke);
    doing = { kind: "draw", stroke };
  } else if (state.tool === "eraser") {
    begin();
    doing = { kind: "erase" };
    erase(event);
  } else if (state.tool === "line") {
    const hit = elementAt(event.target);
    doing = { kind: "tie", from: ties(hit) ? { id: hit.id } : { x: round(at.x), y: round(at.y) }, start, line: null };
  } else if (state.tool === "comment") {
    const hit = elementAt(event.target);
    // The tool is put down first: putting it down closes what is open,
    // and the note's box opens after.
    setTool("select");
    emit("comment", { at, on: hit && hit.type !== "comment" ? hit : null, x: event.clientX, y: event.clientY });
  } else if (DRAWS.has(state.tool)) {
    doing = { kind: "make", from: at, start };
  } else {
    const hit = elementAt(event.target);
    if (!hit) {
      doing = { kind: "marquee", from: at, add: event.shiftKey, had: new Set(state.selection) };
    } else {
      if (event.shiftKey) select([hit.id], { add: true });
      else if (!state.selection.has(hit.id)) select([hit.id]);
      doing = { kind: "move", from: at, start, moved: false, copy: event.altKey, hit };
    }
  }
  if (doing) el.canvas.setPointerCapture(event.pointerId);
}

const round = (value) => Math.round(value * 10) / 10;
const far = (a, b) => Math.hypot(a.x - b.x, a.y - b.y) > 4;

function move(event) {
  if (!doing) return;
  const at = toBoard(event);
  const here = { x: event.clientX, y: event.clientY };
  if (doing.kind === "pan") {
    setView({ x: doing.view.x - (here.x - doing.start.x) / state.view.scale, y: doing.view.y - (here.y - doing.start.y) / state.view.scale });
  } else if (doing.kind === "marquee") {
    const b = { x: Math.min(doing.from.x, at.x), y: Math.min(doing.from.y, at.y), w: Math.abs(at.x - doing.from.x), h: Math.abs(at.y - doing.from.y) };
    extra.marquee = b;
    // Where things are does not change while a box is dragged over them.
    doing.boxes ||= elements().map((element) => [element, box(element)]);
    const inside = doing.boxes.filter(([element, its]) => overlaps(its, b) && !(element.type === "frame" && !overlapsEdge(its, b))).map(([element]) => element.id);
    state.selection = new Set([...(doing.add ? doing.had : []), ...inside]);
    // The marks follow the pointer; the bar over the selection is drawn
    // when the button is let go.
    marksSoon();
  } else if (doing.kind === "move") {
    if (!doing.moved) {
      if (!far(doing.start, here)) return;
      let list = selected();
      if (list.every((element) => element.locked)) {
        doing = null;
        return;
      }
      begin();
      if (doing.copy) {
        // Alt held: what is dragged away is a copy; the first stays.
        const made = copies(list, 0, 0);
        elements().push(...made);
        state.selection = new Set(made.map((element) => element.id));
        list = made;
      }
      doing.moved = true;
      doing.list = moving(list);
      // Where each was when the move began: every move is from there.
      doing.was = new Map(doing.list.map((element) => [element.id, placeOf(element)]));
      doing.box = union(doing.list.filter((element) => element.type !== "line"));
      doing.others = inSight(new Set(doing.list.map((element) => element.id)));
    }
    let dx = at.x - doing.from.x;
    let dy = at.y - doing.from.y;
    // Shift held: along one line only.
    if (event.shiftKey) Math.abs(dx) > Math.abs(dy) ? (dy = 0) : (dx = 0);
    const snapped = event.metaKey || event.ctrlKey ? { dx: 0, dy: 0, guides: [] } : snap({ ...doing.box, x: doing.box.x + dx, y: doing.box.y + dy }, doing.others);
    extra.guides = snapped.guides;
    for (const element of doing.list) {
      Object.assign(element, placeOf(doing.was.get(element.id)));
      shift(element, dx + snapped.dx, dy + snapped.dy);
    }
    drawSoon();
  } else if (doing.kind === "size") {
    resize(doing, at, event.shiftKey);
    drawSoon();
  } else if (doing.kind === "tie") {
    if (!doing.line) {
      if (!far(doing.start, here)) return;
      begin();
      doing.line = { id: newId("l"), type: "line", from: doing.from, to: { x: round(at.x), y: round(at.y) }, route: state.last.route || "curve", end: "arrow", color: state.last.line };
      elements().push(doing.line);
    }
    aim(doing.line, "to", event, at);
    drawSoon();
  } else if (doing.kind === "end") {
    aim(doing.line, doing.end, event, at);
    drawSoon();
  } else if (doing.kind === "draw") {
    const points = doing.stroke.points;
    const [lx, ly] = points[points.length - 1];
    if (Math.hypot(at.x - lx, at.y - ly) * state.view.scale >= 2) {
      points.push([round(at.x), round(at.y)]);
      drawSoon();
    }
  } else if (doing.kind === "erase") {
    erase(event);
  } else if (doing.kind === "make") {
    extra.marquee = { x: Math.min(doing.from.x, at.x), y: Math.min(doing.from.y, at.y), w: Math.abs(at.x - doing.from.x), h: Math.abs(at.y - doing.from.y) };
    marks();
  }
}

// Where an element is, as far as moving it changes: copied, so that it
// can be put back and moved from there again.
function placeOf(element) {
  if (element.type === "line") return { from: { ...element.from }, to: { ...element.to } };
  if (element.type === "draw") return { points: element.points };
  return { x: element.x, y: element.y };
}

let marking = 0;
function marksSoon() {
  marking ||= requestAnimationFrame(() => {
    marking = 0;
    marks();
  });
}

// A section is picked by the frame's edge, not by dragging over what is
// in it.
function overlapsEdge(frame, b) {
  return !(b.x > frame.x && b.y > frame.y && b.x + b.w < frame.x + frame.w && b.y + b.h < frame.y + frame.h);
}

// Points one end of a line at what is under the pointer: an element it
// can be tied to, else the point itself.
function aim(line, key, event, at) {
  const under = document.elementsFromPoint(event.clientX, event.clientY).map((node) => elementAt(node)).find((hit) => hit && hit !== line && ties(hit) && hit.type !== "frame");
  const otherKey = key === "to" ? "from" : "to";
  if (under && line[otherKey]?.id !== under.id) {
    line[key] = { id: under.id };
    extra.target = under;
  } else {
    line[key] = { x: round(at.x), y: round(at.y) };
    extra.target = null;
  }
}

function resize(doing, at, keep) {
  const { element, was } = doing;
  const least = element.type === "text" ? 40 : 24;
  let { x, y, w, h } = was;
  const dx = at.x - doing.from.x;
  const dy = at.y - doing.from.y;
  if (doing.at.includes("e")) w = Math.max(least, was.w + dx);
  if (doing.at.includes("s")) h = Math.max(least, was.h + dy);
  if (doing.at.includes("w")) {
    w = Math.max(least, was.w - dx);
    x = was.x + was.w - w;
  }
  if (doing.at.includes("n")) {
    h = Math.max(least, was.h - dy);
    y = was.y + was.h - h;
  }
  // Shift, or a picture or a sticker: its shape is kept.
  if ((keep || element.type === "image" || element.type === "sticker") && doing.at.length === 2) {
    const ratio = was.w / was.h;
    if (w / h > ratio) w = h * ratio;
    else h = w / ratio;
    if (doing.at.includes("w")) x = was.x + was.w - w;
    if (doing.at.includes("n")) y = was.y + was.h - h;
  }
  Object.assign(element, { x: Math.round(x), y: Math.round(y), w: Math.round(w) });
  if (element.type !== "text") element.h = Math.round(h);
}

// Rubs out the pen's lines under the pointer.
function erase(event) {
  const hit = elementAt(event.target) || elementAt(document.elementFromPoint(event.clientX, event.clientY));
  if (hit && hit.type === "draw" && !hit.locked) {
    state.board.elements = elements().filter((element) => element !== hit);
    draw();
  }
}

function up(event) {
  if (!doing) return;
  const was = doing;
  doing = null;
  el.canvas.classList.remove("grabbing");
  extra.marquee = null;
  extra.guides = [];
  extra.target = null;
  const at = toBoard(event);
  if (was.kind === "marquee") {
    // A press and release on the empty board lets go of everything.
    if (!far(toScreenish(was.from), toScreenish(at)) && !was.add) select([]);
    else select([...state.selection]);
  } else if (was.kind === "move") {
    if (was.moved) {
      commit();
      // Copies were dragged away: they are what is selected now.
      if (was.copy) select([...state.selection]);
    } else if (!event.shiftKey && state.selection.size > 1) select([was.hit.id]);
  } else if (was.kind === "size" || was.kind === "end" || was.kind === "erase") {
    commit();
  } else if (was.kind === "draw") {
    commit();
  } else if (was.kind === "tie") {
    if (was.line) {
      commit();
      select([was.line.id]);
      if (state.tool === "line") setTool("select");
    } else if (was.from.id && state.tool !== "line") {
      again(byId(was.from.id), was.from.side);
    }
  } else if (was.kind === "make") {
    const dragged = far(was.start, { x: event.clientX, y: event.clientY });
    const size = dragged ? [Math.max(40, Math.abs(at.x - was.from.x)), Math.max(40, Math.abs(at.y - was.from.y))] : null;
    const centre = dragged ? { x: (was.from.x + at.x) / 2, y: (was.from.y + at.y) / 2 } : at;
    const made = make(centre, size);
    if (made) {
      add(made);
      setTool("select");
      if (made.type !== "sticker") edit(made.id);
    }
  }
  marks();
}

const toScreenish = (point) => ({ x: point.x * state.view.scale, y: point.y * state.view.scale });

// A click on a side's dot: the same thing again on that side, joined by
// an arrow, ready to be typed in. (How a flow or a map of ideas grows.)
function again(element, side) {
  if (!element) return;
  const b = box(element);
  const gap = 80;
  const by = { right: [b.w + gap, 0], left: [-(b.w + gap), 0], bottom: [0, b.h + gap], top: [0, -(b.h + gap)] }[side] || [b.w + gap, 0];
  const copy = JSON.parse(JSON.stringify(element));
  delete copy.votes;
  copy.id = newId(element.type[0]);
  copy.text = "";
  copy.x += by[0];
  copy.y += by[1];
  // Not onto something that is there: further on.
  let tries = 0;
  while (tries < 8 && elements().some((other) => other !== element && other.type !== "frame" && other.type !== "line" && overlaps(box(other), box(copy)))) {
    copy.x += side === "top" || side === "bottom" ? b.w + 40 : 0;
    copy.y += side === "left" || side === "right" ? b.h + 40 : 0;
    tries += 1;
  }
  const line = { id: newId("l"), type: "line", from: { id: element.id }, to: { id: copy.id }, route: state.last.route || "curve", end: "arrow", color: state.last.line };
  add([copy, line]);
  select([copy.id]);
  if (["sticky", "shape", "text"].includes(copy.type)) edit(copy.id);
}

function twice(event) {
  if (!state.owner || state.tool !== "select") return;
  // The marks over the selection (a dot, a handle) are no empty board.
  if (event.target instanceof Element && event.target.closest(".overlay > *")) return;
  // (Where the press was kept by the canvas, the click is the canvas's
  // too: what lies under the pointer is asked for.)
  const hit = elementAt(event.target) || elementAt(document.elementFromPoint(event.clientX, event.clientY));
  if (hit) {
    if (hit.type === "comment") emit("thread", hit);
    else edit(hit.id);
    return;
  }
  // On the empty board: a note, there, ready to be typed in.
  const made = fresh("sticky", toBoard(event), { color: state.last.sticky });
  add(made);
  edit(made.id);
}

// ---- The keys ---------------------------------------------------------------------

const TOOL_KEYS = { v: "select", h: "hand", s: "sticky", t: "text", r: "shape", l: "line", p: "pen", m: "marker", e: "eraser", f: "frame", k: "sticker", c: "comment" };

function keyDown(event) {
  const target = event.target;
  if (target instanceof Element && target.closest("input, textarea, select, [contenteditable]")) return;
  // A dialog, the walk, a menu or a pop-up has the keys while it is open.
  if (document.querySelector(".scrim, .walk, .menu, .popover")) return;
  const command = event.metaKey || event.ctrlKey;
  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
  // A button that has the keyboard is pressed with Space and Enter.
  if (target instanceof Element && target.closest("button, a, [role=button], [role=menuitem]") && (event.code === "Space" || key === "Enter")) return;
  if (event.code === "Space" && !command) {
    event.preventDefault();
    space = true;
    el.canvas.classList.add("panning");
    return;
  }
  if (command && key === "z") {
    event.preventDefault();
    event.shiftKey ? redo() : undo();
    return;
  }
  if (command && (key === "=" || key === "+")) return void (event.preventDefault(), zoomBy(1.25));
  if (command && key === "-") return void (event.preventDefault(), zoomBy(0.8));
  if (command && key === "0") return void (event.preventDefault(), fit());
  if (!state.owner) return;
  if (command && key === "a") {
    event.preventDefault();
    select(elements().map((element) => element.id));
  } else if (command && key === "d") {
    event.preventDefault();
    duplicate();
  } else if (command && key === "g") {
    event.preventDefault();
    emit("do", event.shiftKey ? "ungroup" : "group");
  } else if (command && event.shiftKey && key === "l") {
    event.preventDefault();
    emit("do", "lock");
  } else if (command) {
    // (Copy, cut and paste are heard as the browser's own events.)
    return;
  } else if (key === "Tab") {
    // From thing to thing, in the order they lie: the board by keys.
    const list = elements();
    if (list.length === 0) return;
    event.preventDefault();
    const at = list.findIndex((element) => state.selection.has(element.id));
    const next = at < 0 ? (event.shiftKey ? list.length - 1 : 0) : (at + (event.shiftKey ? -1 : 1) + list.length) % list.length;
    select([list[next].id]);
    const b = box(list[next]);
    const size = viewSize();
    const seen = { x: state.view.x, y: state.view.y, w: size.w / state.view.scale, h: size.h / state.view.scale };
    if (!overlaps(b, seen)) setView({ x: b.x + b.w / 2 - seen.w / 2, y: b.y + b.h / 2 - seen.h / 2 });
  } else if (key === "Backspace" || key === "Delete") {
    event.preventDefault();
    removeSelected();
  } else if (key === "Escape") {
    cancel();
    select([]);
    setTool("select");
  } else if (key === "Enter" && state.selection.size === 1) {
    event.preventDefault();
    edit([...state.selection][0]);
  } else if (key.startsWith("Arrow") && state.selection.size) {
    event.preventDefault();
    const by = event.shiftKey ? 10 : 1;
    const [dx, dy] = { ArrowLeft: [-by, 0], ArrowRight: [by, 0], ArrowUp: [0, -by], ArrowDown: [0, by] }[key];
    begin();
    for (const element of moving(selected())) shift(element, dx, dy);
    commit("nudge");
  } else if (key === "]" || key === "[") {
    emit("do", key === "]" ? "front" : "back");
  } else if (event.shiftKey && key === "!") {
    fit();
  } else if (event.shiftKey && key === "@") {
    if (state.selection.size) show(union(selected()));
  } else if (key === "+" || key === "=") {
    zoomBy(1.25);
  } else if (key === "-") {
    zoomBy(0.8);
  } else if (key === "0") {
    zoomTo(1);
  } else if (TOOL_KEYS[key] && !event.shiftKey && !event.altKey) {
    setTool(TOOL_KEYS[key]);
  }
}

// ---- Copy and paste ----------------------------------------------------------------

const MARK = "application/x-board-elements:";
let pastes = 0;

// What is selected, as the text a paste here (or on another board) takes
// for elements.
const copyText = () => MARK + JSON.stringify(withEnds(selected()));
// The last copied here: pasted where the system's clipboard is not given.
let kept = "";

// ⌘C and ⌘X, as the browser says them: what is selected goes on the
// clipboard with the event itself, which every web view allows.
function copied(event, cut) {
  const target = event.target;
  if (target instanceof Element && target.closest("input, textarea, [contenteditable]")) return;
  if (!state.owner || state.selection.size === 0) return;
  event.preventDefault();
  kept = copyText();
  pastes = 0;
  event.clipboardData?.setData("text/plain", kept);
  if (cut) removeSelected();
}

function paste(event) {
  const target = event.target;
  if (target instanceof Element && target.closest("input, textarea, [contenteditable]")) return;
  if (!state.owner) return;
  const files = [...(event.clipboardData?.files || [])].filter((file) => file.type.startsWith("image/"));
  if (files.length) {
    event.preventDefault();
    addPictures(files, middle());
    return;
  }
  const text = event.clipboardData?.getData("text/plain") || kept;
  if (!text.trim()) return;
  event.preventDefault();
  if (text.startsWith(MARK)) {
    let list;
    try {
      list = JSON.parse(text.slice(MARK.length));
    } catch {
      return;
    }
    if (!Array.isArray(list)) return;
    list = list.filter((element) => element && typeof element === "object" && typeof element.type === "string");
    if (list.length === 0) return;
    pastes += 1;
    // Where it is looked at now, when what was copied is out of sight.
    const b = union(list);
    const centre = middle();
    const size = viewSize();
    const seen = { x: state.view.x, y: state.view.y, w: size.w / state.view.scale, h: size.h / state.view.scale };
    const [dx, dy] = overlaps(b, seen) ? [24 * pastes, 24 * pastes] : [centre.x - (b.x + b.w / 2), centre.y - (b.y + b.h / 2)];
    add(copies(list, dx, dy));
    return;
  }
  // Words from elsewhere: a line each becomes a note, in rows.
  const lines = text.split(/\r?\n/).map((line) => line.replace(/^\s*(?:[-*•]|\d+[.)])\s+/, "").trim()).filter(Boolean);
  if (lines.length === 0) return;
  const centre = middle();
  if (lines.length === 1 && lines[0].length > 60) {
    const made = fresh("text", centre, { size: [360, 30] });
    made.text = lines[0];
    add(made);
    return;
  }
  const across = Math.min(lines.length, Math.ceil(Math.sqrt(lines.length * 1.6)));
  const made = lines.slice(0, 120).map((line, index) => {
    const note = fresh("sticky", { x: centre.x + ((index % across) - (across - 1) / 2) * 220, y: centre.y + Math.floor(index / across) * 220 - 110 }, { color: state.last.sticky });
    note.text = line;
    return note;
  });
  add(made);
  if (lines.length > 1) toast(`Made ${made.length} notes, one a line.`);
}

// Keeps pictures with the board and puts them on it, at `at`.
export async function addPictures(files, at) {
  if (!state.owner) return;
  const made = [];
  for (const [index, file] of files.entries()) {
    try {
      const src = await upload(file);
      const size = await new Promise((resolve) => {
        const probe = new Image();
        probe.onload = () => resolve([probe.naturalWidth || 320, probe.naturalHeight || 240]);
        probe.onerror = () => resolve([320, 240]);
        probe.src = src;
      });
      const scale = Math.min(1, 480 / size[0], 480 / size[1]);
      made.push(fresh("image", { x: at.x + index * 32, y: at.y + index * 32 }, { src, size: [Math.round(size[0] * scale), Math.round(size[1] * scale)] }));
    } catch (error) {
      toast(error.message, { kind: "error" });
    }
  }
  if (made.length) add(made);
}

export { shift, moving };
