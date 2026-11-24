// What things on a board look like and where they are: the colours by name,
// the outlines of shapes, how large an element is, where a line meets an
// element and which way it runs.

import { byId, START_SIZES } from "./model.js";

// Colours by name, the same in a light and a dark window: `soft` fills a
// note or a shape (dark words read on it), `strong` draws a line, an
// outline or coloured words.
export const COLORS = {
  yellow: { soft: "#ffe27a", strong: "#d99a00" },
  orange: { soft: "#ffc48f", strong: "#e8730c" },
  red: { soft: "#ffb0ab", strong: "#dc3b32" },
  pink: { soft: "#ffb5d6", strong: "#d23a78" },
  purple: { soft: "#d3c4ff", strong: "#7550e0" },
  blue: { soft: "#b1d2ff", strong: "#2476d6" },
  teal: { soft: "#a3e6dc", strong: "#0d9488" },
  green: { soft: "#b4e4ac", strong: "#2f9a48" },
  gray: { soft: "#dcdfe5", strong: "#7c8591" },
};
export const COLOR_NAMES = Object.keys(COLORS);
// The words on a note or a filled shape.
export const ON_SOFT = "#1d1e22";

export const soft = (name) => COLORS[name]?.soft || COLORS.gray.soft;
// "ink" is the window's own text colour: it follows the theme. A colour
// that is no name of ours is taken as written only when it is a plain
// `#rrggbb`: what the file says is put into the page's styles.
const HEX = /^#[0-9a-f]{3,8}$/i;
export const strong = (name) => COLORS[name]?.strong || (typeof name === "string" && HEX.test(name) ? name : "var(--ui-fg)");

// Where a picture of the board may come from: the board's own `media/`
// (or the page itself). An address elsewhere is not fetched: the file is
// written by agents, and a board shared with people would call out from
// each of their browsers.
export const pictureSrc = (src) => (typeof src === "string" && (/^media\/[^?#]+$/.test(src) || src.startsWith("data:image/") || src.startsWith("blob:")) && !src.includes("..") ? src : "");

// Text sizes by name: [points, what it is called].
export const SIZES = { s: [13, "Small"], m: [16, "Medium"], l: [24, "Large"], xl: [40, "Extra large"], xxl: [72, "Huge"] };

// The outlines of shapes, for a box `w` wide and `h` high.
export const SHAPES = [
  { id: "rounded", name: "Rounded rectangle", path: (w, h) => roundRect(w, h, Math.min(14, w / 4, h / 4)) },
  { id: "rect", name: "Rectangle", path: (w, h) => `M0 0H${w}V${h}H0Z` },
  { id: "ellipse", name: "Ellipse", path: (w, h) => `M0 ${h / 2}A${w / 2} ${h / 2} 0 1 1 ${w} ${h / 2}A${w / 2} ${h / 2} 0 1 1 0 ${h / 2}Z`, inset: 0.15 },
  { id: "pill", name: "Pill", path: (w, h) => roundRect(w, h, Math.min(w, h) / 2) },
  { id: "diamond", name: "Diamond", path: (w, h) => `M${w / 2} 0L${w} ${h / 2}L${w / 2} ${h}L0 ${h / 2}Z`, inset: 0.24 },
  { id: "triangle", name: "Triangle", path: (w, h) => `M${w / 2} 0L${w} ${h}H0Z`, inset: 0.22, low: true },
  { id: "hexagon", name: "Hexagon", path: (w, h) => `M${w * 0.22} 0H${w * 0.78}L${w} ${h / 2}L${w * 0.78} ${h}H${w * 0.22}L0 ${h / 2}Z`, inset: 0.14 },
  { id: "slant", name: "Parallelogram", path: (w, h) => `M${w * 0.18} 0H${w}L${w * 0.82} ${h}H0Z`, inset: 0.14 },
  {
    id: "cylinder",
    name: "Cylinder",
    path: (w, h) => {
      const r = Math.min(h * 0.16, 22);
      return `M0 ${r}A${w / 2} ${r} 0 0 1 ${w} ${r}V${h - r}A${w / 2} ${r} 0 0 1 0 ${h - r}ZM0 ${r}A${w / 2} ${r} 0 0 0 ${w} ${r}`;
    },
    low: true,
  },
  {
    id: "document",
    name: "Document",
    path: (w, h) => `M0 0H${w}V${h * 0.86}C${w * 0.75} ${h * 0.72} ${w * 0.62} ${h} ${w * 0.38} ${h * 0.92}C${w * 0.2} ${h * 0.86} ${w * 0.1} ${h * 0.9} 0 ${h}Z`,
  },
  {
    id: "callout",
    name: "Callout",
    path: (w, h) => {
      const b = h * 0.8;
      const r = Math.min(12, w / 4, b / 4);
      return `M${r} 0H${w - r}Q${w} 0 ${w} ${r}V${b - r}Q${w} ${b} ${w - r} ${b}H${w * 0.34}L${w * 0.18} ${h}V${b}H${r}Q0 ${b} 0 ${b - r}V${r}Q0 0 ${r} 0Z`;
    },
    high: true,
  },
  {
    id: "cloud",
    name: "Cloud",
    path: (w, h) =>
      `M${w * 0.25} ${h * 0.9}C${w * 0.02} ${h * 0.9} ${-w * 0.02} ${h * 0.5} ${w * 0.18} ${h * 0.44}C${w * 0.14} ${h * 0.12} ${w * 0.44} ${h * 0.02} ${w * 0.56} ${h * 0.24}C${w * 0.7} ${h * 0.04} ${w * 0.96} ${h * 0.2} ${w * 0.86} ${h * 0.46}C${w * 1.04} ${h * 0.52} ${w * 1.0} ${h * 0.9} ${w * 0.78} ${h * 0.9}Z`,
    inset: 0.16,
  },
  {
    id: "person",
    name: "Person",
    path: (w, h) => {
      const r = Math.min(w * 0.22, h * 0.2);
      const top = r * 2 + h * 0.04;
      return `M${w / 2 - r} ${r}A${r} ${r} 0 1 1 ${w / 2 + r} ${r}A${r} ${r} 0 1 1 ${w / 2 - r} ${r}ZM0 ${h}C0 ${top + (h - top) * 0.1} ${w * 0.2} ${top} ${w / 2} ${top}C${w * 0.8} ${top} ${w} ${top + (h - top) * 0.1} ${w} ${h}Z`;
    },
    low: true,
  },
  {
    id: "star",
    name: "Star",
    path: (w, h) => {
      const points = [];
      for (let i = 0; i < 10; i += 1) {
        const radius = i % 2 === 0 ? 0.5 : 0.21;
        const angle = (Math.PI / 5) * i - Math.PI / 2;
        points.push(`${(w * (0.5 + radius * Math.cos(angle))).toFixed(1)} ${(h * (0.52 + radius * 1.05 * Math.sin(angle))).toFixed(1)}`);
      }
      return `M${points.join("L")}Z`;
    },
    inset: 0.3,
  },
];
export const shapeOf = (id) => SHAPES.find((shape) => shape.id === id) || SHAPES[0];

function roundRect(w, h, r) {
  return `M${r} 0H${w - r}Q${w} 0 ${w} ${r}V${h - r}Q${w} ${h} ${w - r} ${h}H${r}Q0 ${h} 0 ${h - r}V${r}Q0 0 ${r} 0Z`;
}

// What a new element of a kind starts as, with its middle at `at`.
export function fresh(type, at, more = {}) {
  const [w, h] = (more.size || START_SIZES[type] || [160, 100]).map(Math.round);
  const base = { type, x: Math.round(at.x - w / 2), y: Math.round(at.y - h / 2), w, h };
  if (type === "sticky") return { ...base, text: "", color: more.color || "yellow" };
  if (type === "shape") return { ...base, shape: more.shape || "rounded", text: "", fill: more.fill || "blue", stroke: "none" };
  if (type === "text") return { ...base, text: "", size: "m" };
  if (type === "frame") return { ...base, title: "", color: "gray" };
  if (type === "table")
    return { ...base, rows: [["", "", ""], ["", "", ""], ["", "", ""]] };
  if (type === "sticker") return { ...base, emoji: more.emoji || "👍" };
  if (type === "image") return { ...base, src: more.src || "" };
  return base;
}

// ---- Where things are -----------------------------------------------------

// Whether a line can be tied to an element of this kind.
export const ties = (element) => ["sticky", "shape", "text", "frame", "table", "sticker", "image"].includes(element?.type);

// The box an element takes: { x, y, w, h }. A line's and a drawing's are
// worked out from their points.
export function box(element) {
  if (!element) return { x: 0, y: 0, w: 0, h: 0 };
  if (element.type === "line") {
    const [a, b] = ends(element);
    return around([a, b, ...mids(element, a, b)]);
  }
  if (element.type === "draw") return around((element.points || []).map(([x, y]) => ({ x, y })));
  if (element.type === "comment") return { x: (element.x || 0) - 12, y: (element.y || 0) - 12, w: 24, h: 24 };
  return { x: element.x || 0, y: element.y || 0, w: element.w || 0, h: element.h || 0 };
}

export function around(points) {
  if (points.length === 0) return { x: 0, y: 0, w: 0, h: 0 };
  const xs = points.map((point) => point.x);
  const ys = points.map((point) => point.y);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
}

// The box that holds all of `list`.
export function union(list) {
  const boxes = list.map(box);
  if (boxes.length === 0) return { x: 0, y: 0, w: 0, h: 0 };
  const x = Math.min(...boxes.map((b) => b.x));
  const y = Math.min(...boxes.map((b) => b.y));
  return { x, y, w: Math.max(...boxes.map((b) => b.x + b.w)) - x, h: Math.max(...boxes.map((b) => b.y + b.h)) - y };
}

export const overlaps = (a, b) => a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
export const holds = (outer, inner) => inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h;

const SIDES = ["top", "right", "bottom", "left"];
// The middle of a side of a box.
export function sidePoint(b, side) {
  if (side === "top") return { x: b.x + b.w / 2, y: b.y, side };
  if (side === "bottom") return { x: b.x + b.w / 2, y: b.y + b.h, side };
  if (side === "left") return { x: b.x, y: b.y + b.h / 2, side };
  return { x: b.x + b.w, y: b.y + b.h / 2, side: "right" };
}

// Which side of box `b` faces the point `to`.
function facing(b, to) {
  const dx = to.x - (b.x + b.w / 2);
  const dy = to.y - (b.y + b.h / 2);
  // Measured against the box's own shape, so a wide box is left by its
  // top or bottom only when the other is clearly above or below.
  if (Math.abs(dx) * b.h >= Math.abs(dy) * b.w) return dx >= 0 ? "right" : "left";
  return dy >= 0 ? "bottom" : "top";
}

const centre = (b) => ({ x: b.x + b.w / 2, y: b.y + b.h / 2 });

// Where one end of a line is: a point, with the side it leaves an element
// by when it is tied to one. `other` is the line's other end, as given.
function end(spec, other) {
  if (!spec) return { x: 0, y: 0 };
  const tied = spec.id ? byId(spec.id) : null;
  if (!tied || !ties(tied)) {
    if (Number.isFinite(spec.x) && Number.isFinite(spec.y)) return { x: spec.x, y: spec.y };
    // Tied to what is not there (an agent took it away): it ends where it
    // starts, rather than run to the board's corner.
    const there = other?.id ? byId(other.id) : null;
    if (there && ties(there)) return centre(box(there));
    return { x: Number.isFinite(other?.x) ? other.x : 0, y: Number.isFinite(other?.y) ? other.y : 0 };
  }
  const b = box(tied);
  if (SIDES.includes(spec.side)) return sidePoint(b, spec.side);
  const far = other?.id && byId(other.id) ? centre(box(byId(other.id))) : { x: other?.x ?? b.x, y: other?.y ?? b.y };
  return sidePoint(b, facing(b, far));
}

// Both ends of a line, as points (with their sides).
export function ends(line) {
  return [end(line.from, line.to), end(line.to, line.from)];
}

const NORMAL = { top: [0, -1], bottom: [0, 1], left: [-1, 0], right: [1, 0] };

// The points a line bends at between its ends (an elbow's corners, a
// curve's handles).
function mids(line, a, b) {
  if (line.route === "elbow") return elbow(a, b);
  if (line.route === "curve") return handles(a, b);
  return [];
}

function elbow(a, b) {
  const across = (side) => side === "left" || side === "right";
  const start = a.side ? across(a.side) : Math.abs(b.x - a.x) >= Math.abs(b.y - a.y);
  const finish = b.side ? across(b.side) : start;
  if (start && finish) {
    const x = (a.x + b.x) / 2;
    return [{ x, y: a.y }, { x, y: b.y }];
  }
  if (!start && !finish) {
    const y = (a.y + b.y) / 2;
    return [{ x: a.x, y }, { x: b.x, y }];
  }
  return start ? [{ x: b.x, y: a.y }] : [{ x: a.x, y: b.y }];
}

function handles(a, b) {
  const reach = Math.max(40, Math.hypot(b.x - a.x, b.y - a.y) / 2.4);
  const out = (point, toward) => {
    const [nx, ny] = point.side ? NORMAL[point.side] : unit(toward.x - point.x, toward.y - point.y);
    return { x: point.x + nx * reach, y: point.y + ny * reach };
  };
  return [out(a, b), out(b, a)];
}

function unit(x, y) {
  const length = Math.hypot(x, y) || 1;
  return [x / length, y / length];
}

// A line as a path, with where its heads point and where its label sits.
export function route(line) {
  const [a, b] = ends(line);
  const between = mids(line, a, b);
  let d;
  let label;
  if (line.route === "curve") {
    const [c1, c2] = between;
    d = `M${a.x} ${a.y}C${c1.x} ${c1.y} ${c2.x} ${c2.y} ${b.x} ${b.y}`;
    label = { x: (a.x + 3 * c1.x + 3 * c2.x + b.x) / 8, y: (a.y + 3 * c1.y + 3 * c2.y + b.y) / 8 };
  } else {
    const points = [a, ...between, b];
    d = "M" + points.map((point) => `${point.x} ${point.y}`).join("L");
    // The middle of its longest stretch.
    let best = 0;
    for (let i = 1; i < points.length - 1; i += 1) {
      const length = (j) => Math.hypot(points[j + 1].x - points[j].x, points[j + 1].y - points[j].y);
      if (length(i) > length(best)) best = i;
    }
    label = { x: (points[best].x + points[best + 1].x) / 2, y: (points[best].y + points[best + 1].y) / 2 };
  }
  const before = between[between.length - 1] || a;
  const after = between[0] || b;
  return {
    a,
    b,
    d,
    label,
    // The way the line runs into each end.
    intoB: Math.atan2(b.y - before.y, b.x - before.x),
    intoA: Math.atan2(a.y - after.y, a.x - after.x),
  };
}

// A freehand line through `points`, smoothed.
export function stroke(points) {
  if (points.length === 0) return "";
  if (points.length < 3) return "M" + points.map(([x, y]) => `${x} ${y}`).join("L") + (points.length === 1 ? "l0.01 0" : "");
  let d = `M${points[0][0]} ${points[0][1]}`;
  for (let i = 1; i < points.length - 1; i += 1) {
    const [x, y] = points[i];
    const [nx, ny] = points[i + 1];
    d += `Q${x} ${y} ${(x + nx) / 2} ${(y + ny) / 2}`;
  }
  const [lx, ly] = points[points.length - 1];
  return d + `L${lx} ${ly}`;
}

// What lies in a section (a frame): the elements whose middle it holds,
// and the lines drawn in it that are tied to nothing (a tied end follows
// what it is tied to).
export function inFrame(frame, list) {
  const b = box(frame);
  const within = (point) => point.x >= b.x && point.x <= b.x + b.w && point.y >= b.y && point.y <= b.y + b.h;
  return list.filter((element) => {
    if (element === frame || element.type === "frame") return false;
    if (element.type === "line") {
      const loose = [element.from, element.to].filter((spec) => spec && !spec.id);
      return loose.length > 0 && loose.every(within);
    }
    return within(centre(box(element)));
  });
}

// Lines tied to `gone` (elements leaving the board) stay where they were,
// loose.
export function loosen(lines, gone) {
  const leaving = new Map(gone.map((element) => [element.id, element]));
  for (const line of lines) {
    if (line.type !== "line") continue;
    for (const key of ["from", "to"]) {
      const tied = leaving.get(line[key]?.id);
      if (!tied) continue;
      const c = centre(box(tied));
      line[key] = { x: Math.round(c.x), y: Math.round(c.y) };
    }
  }
}
