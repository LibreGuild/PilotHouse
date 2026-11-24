// What a board has beyond drawing on it: layouts to start from, a timer
// for a round of work, a small map of the whole board, a walk through its
// sections one at a time, the stickers, and the board as an outline of
// words.

import { h, button, dialog, popover, toast } from "/_artifact/kit/ui.js";
import { state, elements, byId, select, setView, on, newId } from "./model.js";
import { el, viewSize } from "./draw.js";
import { box, union, soft, strong, inFrame, fresh, ON_SOFT, SIZES, shapeOf, route, stroke, pictureSrc } from "./shapes.js";
import { add, show, middle } from "./tools.js";

// ---- Stickers ---------------------------------------------------------------

// [the sticker, the words it is found by].
export const STICKERS = [
  ["👍", "yes agree thumbs up good like"],
  ["👎", "no disagree thumbs down bad"],
  ["✅", "done check yes complete"],
  ["❌", "no cross wrong remove"],
  ["⭐", "star favourite best"],
  ["❤️", "love heart like"],
  ["🔥", "fire hot urgent"],
  ["🎉", "party celebrate done shipped"],
  ["💡", "idea light bulb"],
  ["❓", "question unknown ask"],
  ["❗", "important exclamation warning"],
  ["⚠️", "warning risk careful"],
  ["🚀", "rocket launch ship fast"],
  ["🎯", "target goal aim"],
  ["🧠", "brain think idea"],
  ["👀", "eyes look review watch"],
  ["🙌", "hands praise thanks"],
  ["👏", "clap applause well done"],
  ["🤔", "thinking unsure hmm"],
  ["😀", "happy smile glad"],
  ["😕", "confused unsure sad"],
  ["😡", "angry mad blocked"],
  ["😴", "sleep slow boring later"],
  ["🐛", "bug defect error"],
  ["🔧", "fix tool wrench repair"],
  ["🔒", "lock secure private security"],
  ["🔑", "key access secret"],
  ["📌", "pin keep important"],
  ["📎", "clip attach link"],
  ["📈", "up growth chart metric"],
  ["📉", "down drop chart metric"],
  ["📅", "date calendar deadline"],
  ["⏰", "time clock deadline late"],
  ["⏳", "waiting hourglass pending"],
  ["💬", "talk comment discuss chat"],
  ["📣", "announce loud tell"],
  ["💰", "money cost budget price"],
  ["🧪", "test experiment try"],
  ["🧩", "piece part puzzle depends"],
  ["🗑️", "delete trash remove drop"],
  ["🚧", "blocked construction work in progress"],
  ["🛑", "stop blocked halt"],
  ["🟢", "green go good ok"],
  ["🟡", "yellow wait careful"],
  ["🔴", "red stop bad"],
  ["1️⃣", "one first 1"],
  ["2️⃣", "two second 2"],
  ["3️⃣", "three third 3"],
  ["➕", "plus add more"],
  ["➖", "minus less remove"],
  ["🏁", "finish end flag done"],
  ["🏆", "win trophy best"],
  ["👤", "person user someone"],
  ["👥", "people team users"],
  ["🤝", "agree deal handshake partner"],
  ["☁️", "cloud server hosted"],
  ["💾", "save disk data store"],
  ["📱", "phone mobile app"],
  ["💻", "computer laptop code"],
  ["🌍", "world web global"],
];

// ---- Layouts to start from --------------------------------------------------

const withId = (element) => ({ id: newId(element.type[0]), ...element });
const note = (x, y, text, color = "yellow") => withId({ ...fresh("sticky", { x: x + 100, y: y + 100 }, { color }), text });
const shape = (x, y, w, h, kind, text, fill = "blue") => withId({ type: "shape", x, y, w, h, shape: kind, text, fill, stroke: "none" });
const section = (x, y, w, h, title, color = "gray") => withId({ type: "frame", x, y, w, h, title, color });
const words = (x, y, w, text, more = {}) => withId({ type: "text", x, y, w, h: 30, text, size: "m", ...more });
const join = (from, to, more = {}) => withId({ type: "line", from: { id: from.id }, to: { id: to.id }, route: "curve", end: "arrow", color: "ink", ...more });

// Each makes its elements around the point (0, 0); they are moved to where
// the board is looked at.
const TEMPLATES = [
  {
    name: "Flow",
    about: "Steps, a decision, and where each answer leads",
    make() {
      const start = shape(0, 0, 160, 64, "pill", "Start", "green");
      const first = shape(240, -8, 180, 80, "rounded", "First step");
      const choice = shape(500, -28, 170, 120, "diamond", "Is it right?", "yellow");
      const yes = shape(760, -8, 180, 80, "rounded", "Carry on");
      const no = shape(495, 180, 180, 80, "rounded", "Fix it", "red");
      const end = shape(1020, 0, 160, 64, "pill", "Done", "green");
      return [start, first, choice, yes, no, end, join(start, first), join(first, choice), join(choice, yes, { text: "Yes" }), join(choice, no, { text: "No" }), join(no, first, { route: "elbow", from: { id: no.id, side: "left" }, to: { id: first.id, side: "bottom" } }), join(yes, end)];
    },
  },
  {
    name: "Retrospective",
    about: "What went well, what to improve, what to do about it",
    make() {
      const columns = [["Went well", "green"], ["To improve", "red"], ["Actions", "blue"]];
      return columns.flatMap(([title, color], index) => {
        const x = index * 500;
        return [section(x, 0, 470, 560, title, color), note(x + 24, 56, "", color), note(x + 244, 56, "", color)];
      });
    },
  },
  {
    name: "Task board",
    about: "To do, doing, in review, done",
    make() {
      return ["To do", "Doing", "In review", "Done"].flatMap((title, index) => {
        const x = index * 280;
        const made = [section(x, 0, 256, 620, title)];
        if (index === 0) made.push(note(x + 28, 56, "A task"), note(x + 28, 276, "Another"));
        return made;
      });
    },
  },
  {
    name: "Map of ideas",
    about: "One subject in the middle, its parts around it",
    make() {
      const centre = shape(-110, -50, 220, 100, "ellipse", "The subject", "purple");
      const made = [centre];
      const colours = ["blue", "teal", "green", "yellow", "orange", "pink"];
      for (let i = 0; i < 6; i += 1) {
        const angle = (Math.PI / 3) * i;
        const part = shape(Math.round(Math.cos(angle) * 380) - 85, Math.round(Math.sin(angle) * 240) - 32, 170, 64, "rounded", "A part", colours[i]);
        made.push(part, join(centre, part, { end: "none" }));
      }
      return made;
    },
  },
  {
    name: "For and against",
    about: "Two sides of a decision",
    make() {
      return [words(0, -64, 600, "The decision", { size: "l", bold: true }), section(0, 0, 470, 520, "For", "green"), note(24, 56, "", "green"), section(500, 0, 470, 520, "Against", "red"), note(524, 56, "", "red")];
    },
  },
  {
    name: "Two by two",
    about: "Sort things by two measures: effort and worth",
    make() {
      const cells = [["Quick wins", "green"], ["Big bets", "blue"], ["Fill-ins", "gray"], ["Not worth it", "red"]];
      const made = cells.map(([title, color], index) => section((index % 2) * 440, Math.floor(index / 2) * 360, 420, 340, title, color));
      made.push(words(330, -52, 220, "Effort →", { align: "center", bold: true }), words(-150, 330, 130, "↑ Worth", { bold: true }));
      return made;
    },
  },
  {
    name: "Timeline",
    about: "What happens when, in a row",
    make() {
      const made = [];
      const dots = [];
      for (let i = 0; i < 5; i += 1) {
        const dot = shape(i * 240, 0, 28, 28, "ellipse", "", "blue");
        dots.push(dot);
        made.push(dot, words(i * 240 - 76, 44, 180, i === 0 ? "The start" : `Milestone ${i}`, { align: "center", bold: true }), words(i * 240 - 76, 74, 180, "When, and what", { align: "center", size: "s", color: "gray" }));
      }
      for (let i = 0; i < 4; i += 1) made.push(join(dots[i], dots[i + 1], { route: "straight", end: "none", from: { id: dots[i].id, side: "right" }, to: { id: dots[i + 1].id, side: "left" } }));
      return made;
    },
  },
  {
    name: "System",
    about: "Who uses it, the parts, and where the data lies",
    make() {
      const user = shape(0, 20, 90, 120, "person", "User", "gray");
      const app = shape(210, 30, 190, 100, "rounded", "App");
      const service = shape(520, 30, 190, 100, "rounded", "Service", "teal");
      const data = shape(830, 10, 150, 140, "cylinder", "Database", "purple");
      const outside = shape(520, 250, 190, 120, "cloud", "Another system", "gray");
      return [user, app, service, data, outside, join(user, app), join(app, service, { text: "asks" }), join(service, data, { text: "reads, writes" }), join(service, outside, { dash: true })];
    },
  },
];

export function openTemplates() {
  if (!state.owner) return;
  const float = dialog({
    title: "Start from a layout",
    width: 620,
    content: h(
      "div",
      { class: "templates" },
      TEMPLATES.map((template) =>
        h(
          "button",
          {
            type: "button",
            class: "template",
            onclick: () => {
              float.close();
              place(template.make());
            },
          },
          h("div", { class: "template-name" }, template.name),
          h("div", { class: "template-about" }, template.about),
        ),
      ),
    ),
  });
}

// Puts a layout where the board is looked at; beside what is there, when
// something is.
function place(made) {
  const b = union(made);
  const centre = middle();
  let dx = centre.x - (b.x + b.w / 2);
  let dy = centre.y - (b.y + b.h / 2);
  if (elements().length) {
    const all = union(elements());
    dx = all.x + all.w + 160 - b.x;
    dy = all.y - b.y;
  }
  for (const element of made) {
    if (element.type === "line") continue;
    element.x = Math.round(element.x + dx);
    element.y = Math.round(element.y + dy);
  }
  add(made);
  show(union(made));
}

// ---- The timer ------------------------------------------------------------------

const clock = (seconds) => `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;

// A round of work, counted down; a chime when it is over.
export class Timer {
  constructor() {
    this.length = 300;
    this.left = 300;
    this.ends = 0;
    this.chime = true;
    this.label = h("span", { class: "btn-label timer-left", hidden: true });
    this.button = button({ icon: "timer", title: "A timer for a round of work", onClick: (_, anchor) => this.open(anchor) });
    this.button.append(this.label);
  }

  get running() {
    return this.ends > 0;
  }

  open(anchor) {
    const face = h("div", { class: "timer-face" }, clock(this.left));
    const go = button({ label: this.running ? "Pause" : "Start", kind: "primary", onClick: () => (this.running ? this.pause() : this.start(), float.close()) });
    const by = (minutes) => {
      this.length = Math.max(60, Math.min(99 * 60, (this.running ? this.left : this.length) + minutes * 60));
      this.left = this.length;
      if (this.running) this.ends = Date.now() + this.left * 1000;
      face.textContent = clock(this.left);
      this.show();
    };
    const chime = h("input", { type: "checkbox", checked: this.chime, onchange: () => (this.chime = chime.checked) });
    const float = popover(
      anchor,
      h(
        "div",
        { class: "timer-box" },
        h("div", { class: "timer-row" }, button({ icon: "minus", title: "A minute less", onClick: () => by(-1) }), face, button({ icon: "plus", title: "A minute more", onClick: () => by(1) })),
        h("div", { class: "timer-row" }, [1, 3, 5, 10, 15].map((minutes) => h("button", { type: "button", class: "btn", onclick: () => ((this.length = 0), (this.left = 0), by(minutes)) }, minutes + " min"))),
        h("div", { class: "timer-row" }, h("label", { class: "check" }, chime, "Chime at the end"), h("span", { style: { flex: "1" } }), button({ label: "Reset", onClick: () => (this.reset(), float.close()) }), go),
      ),
      { place: "end" },
    );
  }

  start() {
    // Asked for by a press: a sound first made from a timer stays silent
    // in some browsers.
    if (this.chime) sound();
    if (this.left <= 0) this.left = this.length;
    this.ends = Date.now() + this.left * 1000;
    clearInterval(this.tick);
    this.tick = setInterval(() => this.beat(), 250);
    this.show();
  }

  pause() {
    this.left = Math.max(0, Math.ceil((this.ends - Date.now()) / 1000));
    this.ends = 0;
    clearInterval(this.tick);
    this.show();
  }

  reset() {
    this.ends = 0;
    this.left = this.length;
    clearInterval(this.tick);
    this.show();
  }

  beat() {
    this.left = Math.max(0, Math.ceil((this.ends - Date.now()) / 1000));
    if (this.left === 0) {
      this.ends = 0;
      clearInterval(this.tick);
      this.left = this.length;
      if (this.chime) ring();
      toast("Time is up.", { stay: 8000 });
    }
    this.show();
  }

  show() {
    const shown = this.running || this.left !== this.length;
    this.label.hidden = !shown;
    this.label.textContent = clock(this.left);
    this.button.classList.toggle("btn-icon", !shown);
    this.button.classList.toggle("running", this.running);
  }
}

// What the chime is played with: made once, and woken at each use.
let audio = null;
function sound() {
  try {
    audio ||= new (window.AudioContext || window.webkitAudioContext)();
    audio.resume?.();
  } catch {
    audio = null;
  }
  return audio;
}

// Three soft notes, rising.
function ring() {
  try {
    const audio = sound();
    if (!audio) return;
    [523.25, 659.25, 783.99].forEach((pitch, index) => {
      const tone = audio.createOscillator();
      const loud = audio.createGain();
      const at = audio.currentTime + index * 0.22;
      tone.frequency.value = pitch;
      loud.gain.setValueAtTime(0.0001, at);
      loud.gain.exponentialRampToValueAtTime(0.22, at + 0.03);
      loud.gain.exponentialRampToValueAtTime(0.0001, at + 0.7);
      tone.connect(loud).connect(audio.destination);
      tone.start(at);
      tone.stop(at + 0.75);
    });
  } catch {
    // No sound here: the toast says it.
  }
}

// ---- The small map ------------------------------------------------------------------

let map = null;
const MAP = { w: 200, h: 132 };

// Shows or hides the map; answers whether it is shown now.
export function toggleMap() {
  if (map) {
    map.remove();
    map = null;
    return false;
  }
  map = h("canvas", { class: "minimap", width: MAP.w * 2, height: MAP.h * 2, title: "The whole board: click or drag to look elsewhere" });
  const go = (event) => {
    const rect = map.getBoundingClientRect();
    const { area, scale } = mapped();
    const size = viewSize();
    const x = area.x + (event.clientX - rect.left - (MAP.w - area.w * scale) / 2) / scale;
    const y = area.y + (event.clientY - rect.top - (MAP.h - area.h * scale) / 2) / scale;
    setView({ x: x - size.w / state.view.scale / 2, y: y - size.h / state.view.scale / 2 });
  };
  map.addEventListener("pointerdown", (event) => {
    map.setPointerCapture(event.pointerId);
    go(event);
  });
  map.addEventListener("pointermove", (event) => event.buttons && go(event));
  el.canvas.parentElement.append(map);
  drawMap();
  return true;
}

const seen = () => {
  const size = viewSize();
  return { x: state.view.x, y: state.view.y, w: size.w / state.view.scale, h: size.h / state.view.scale };
};

// The part of the board the map shows (everything on it and what is
// looked at), and how small it is drawn.
function mapped() {
  const look = seen();
  const all = elements().length ? union(elements()) : look;
  const x = Math.min(all.x, look.x) - 40;
  const y = Math.min(all.y, look.y) - 40;
  const area = { x, y, w: Math.max(all.x + all.w, look.x + look.w) + 40 - x, h: Math.max(all.y + all.h, look.y + look.h) + 40 - y };
  return { area, scale: Math.min(MAP.w / area.w, MAP.h / area.h) };
}

function drawMap() {
  if (!map) return;
  const pen = map.getContext("2d");
  const { area, scale } = mapped();
  const style = getComputedStyle(document.documentElement);
  const colour = (name) => style.getPropertyValue(name).trim() || "#888";
  pen.setTransform(2, 0, 0, 2, 0, 0);
  pen.clearRect(0, 0, MAP.w, MAP.h);
  pen.translate((MAP.w - area.w * scale) / 2, (MAP.h - area.h * scale) / 2);
  pen.scale(scale, scale);
  pen.translate(-area.x, -area.y);
  const thin = 1 / scale;
  for (const element of elements()) {
    const b = box(element);
    if (element.type === "frame") {
      pen.strokeStyle = colour("--ui-muted");
      pen.lineWidth = thin;
      pen.strokeRect(b.x, b.y, b.w, b.h);
    } else if (element.type === "line" || element.type === "draw" || element.type === "comment") {
      continue;
    } else {
      pen.fillStyle = element.type === "sticky" ? soft(element.color) : element.type === "shape" && element.fill !== "none" ? soft(element.fill) : colour("--ui-muted");
      pen.fillRect(b.x, b.y, Math.max(b.w, thin * 2), Math.max(b.h, thin * 2));
    }
  }
  const look = seen();
  pen.strokeStyle = colour("--ui-accent");
  pen.lineWidth = thin * 1.5;
  pen.strokeRect(look.x, look.y, look.w, look.h);
}

// Once a frame at most: the view changes with every move of the pointer.
let mapDue = 0;
const mapSoon = () => {
  if (map && !mapDue) {
    mapDue = requestAnimationFrame(() => {
      mapDue = 0;
      drawMap();
    });
  }
};
on("board", mapSoon);
on("view", mapSoon);

// ---- A walk through the sections ----------------------------------------------------------

// The sections in reading order: rows from the top, each from the left.
function sections() {
  return elements()
    .filter((element) => element.type === "frame")
    .sort((a, b) => (Math.abs(a.y - b.y) > Math.min(a.h, b.h) / 2 ? a.y - b.y : a.x - b.x));
}

// Shows the sections one at a time, each filling the window: a board
// talked through like slides.
export function walk() {
  const stops = sections();
  if (stops.length === 0) return toast("A walk goes from section to section: draw sections (F) around the parts of the board first.");
  if (document.querySelector(".walk")) return;
  const stage = el.canvas.parentElement;
  const before = { ...state.view };
  let at = 0;
  const where = h("span", { class: "walk-where" });
  const go = (to) => {
    at = Math.max(0, Math.min(stops.length - 1, to));
    const stop = byId(stops[at].id) || stops[at];
    show(box(stop), { most: 4, pad: 48 });
    where.textContent = `${at + 1} of ${stops.length}` + (stop.title ? ` · ${stop.title}` : "");
    back.disabled = at === 0;
    next.disabled = at === stops.length - 1;
  };
  const end = () => {
    bar.remove();
    stage.classList.remove("walking");
    document.removeEventListener("keydown", keys, true);
    setView(before);
  };
  const keys = (event) => {
    if (event.target instanceof Element && event.target.closest("input, textarea, [contenteditable]")) return;
    // A button of the walk's own that has the keyboard is pressed by them.
    if ([" ", "Enter"].includes(event.key) && event.target instanceof Element && event.target.closest("button")) return;
    if (["ArrowRight", "ArrowDown", " ", "Enter", "PageDown"].includes(event.key)) go(at + 1);
    else if (["ArrowLeft", "ArrowUp", "PageUp"].includes(event.key)) go(at - 1);
    else if (event.key === "Escape") end();
    else return;
    event.preventDefault();
    event.stopPropagation();
  };
  const back = button({ icon: "left", title: "The section before (←)", onClick: () => go(at - 1) });
  const next = button({ icon: "right", title: "The next section (→)", onClick: () => go(at + 1) });
  const bar = h("div", { class: "walk bar" }, back, where, next, h("span", { class: "bar-rule" }), button({ icon: "close", title: "End the walk (Esc)", onClick: end }));
  select([]);
  stage.classList.add("walking");
  stage.append(bar);
  document.addEventListener("keydown", keys, true);
  go(0);
}

// ---- The board as a picture ---------------------------------------------------------------

const wait = (image) => new Promise((resolve) => ((image.onload = () => resolve(image)), (image.onerror = () => resolve(null))));

// Words broken into lines no wider than `width`, as the pen measures them.
function wrap(pen, text, width) {
  const lines = [];
  for (const given of String(text || "").split("\n")) {
    let line = "";
    for (const word of given.split(" ")) {
      const longer = line ? line + " " + word : word;
      if (line && pen.measureText(longer).width > width) {
        lines.push(line);
        line = word;
      } else line = longer;
    }
    lines.push(line);
  }
  return lines;
}

function write(pen, element, b, { pad = 12, middle: centred = false, fallback = "m", colour } = {}) {
  if (!element.text) return;
  const points = (SIZES[element.size] || SIZES[fallback])[0];
  pen.font = `${element.italic ? "italic " : ""}${element.bold ? 650 : 400} ${points}px ${getComputedStyle(document.body).fontFamily}`;
  pen.fillStyle = colour;
  pen.textBaseline = "top";
  const align = element.align || (centred ? "center" : "left");
  pen.textAlign = align;
  const lines = wrap(pen, element.text, b.w - pad * 2).map((line) => (element.list && line ? "• " + line : line));
  const high = points * 1.3;
  const x = align === "center" ? b.x + b.w / 2 : align === "right" ? b.x + b.w - pad : b.x + pad;
  let y = centred ? b.y + (b.h - lines.length * high) / 2 : b.y + pad;
  for (const line of lines) {
    pen.fillText(line, x, y);
    y += high;
  }
}

// A box with rounded corners, where the browser draws those (an older
// one draws it square).
function rounded(pen, b, radii) {
  if (pen.roundRect) pen.roundRect(b.x, b.y, b.w, b.h, radii);
  else pen.rect(b.x, b.y, b.w, b.h);
}

// The whole board drawn `width` by `height` points, as a PNG: what the
// list shows a board by, and what "export" saves. Drawn here, not taken
// off the screen, so it shows the board alone, all of it, whatever part
// is looked at. `sharp`: dots a point (2 for a file to keep; 1 for the
// small picture in the list, which is drawn often).
export async function picture(width, height, sharp = 2) {
  const list = elements().filter((element) => element.type !== "comment");
  const all = list.length ? union(list) : { x: -width / 2, y: -height / 2, w: width, h: height };
  const scale = Math.min(2, (width - 80) / Math.max(all.w, 1), (height - 80) / Math.max(all.h, 1));
  const canvas = h("canvas", { width: width * sharp, height: height * sharp });
  const pen = canvas.getContext("2d");
  const style = getComputedStyle(document.documentElement);
  const colour = (name) => style.getPropertyValue(name).trim() || "#888";
  // The names of the board's colours, as the pen needs them.
  const ink = (name) => {
    const named = strong(name);
    return named.startsWith("var(") ? colour("--ui-fg") : named;
  };
  pen.setTransform(sharp, 0, 0, sharp, 0, 0);
  pen.fillStyle = colour("--ui-stage");
  pen.fillRect(0, 0, width, height);
  pen.translate(width / 2, height / 2);
  pen.scale(scale, scale);
  pen.translate(-(all.x + all.w / 2), -(all.y + all.h / 2));
  pen.lineJoin = "round";
  pen.lineCap = "round";
  const pictures = new Map();
  await Promise.all(
    list
      .filter((element) => element.type === "image" && pictureSrc(element.src))
      .map(async (element) => {
        const image = new Image();
        image.src = pictureSrc(element.src);
        pictures.set(element.id, await wait(image));
      }),
  );
  for (const element of list) {
    pen.save();
    try {
      paint(element);
    } catch (error) {
      // One thing that cannot be drawn is left out of the picture.
      console.error(error);
    }
    pen.restore();
  }
  return new Promise((resolve, reject) => canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("The picture could not be made."))), "image/png"));

  function paint(element) {
    const b = box(element);
    if (element.type === "frame") {
      pen.strokeStyle = element.color && element.color !== "gray" ? ink(element.color) : colour("--ui-muted");
      pen.lineWidth = 1.5;
      pen.beginPath();
      rounded(pen, b, 10);
      pen.stroke();
      pen.globalAlpha = 0.07;
      pen.fillStyle = pen.strokeStyle;
      pen.fill();
      pen.globalAlpha = 1;
      pen.font = `600 14px ${getComputedStyle(document.body).fontFamily}`;
      pen.textBaseline = "bottom";
      pen.fillText(element.title || "", b.x + 4, b.y - 5);
    } else if (element.type === "sticky") {
      pen.shadowColor = "rgb(0 0 0 / 0.25)";
      pen.shadowBlur = 10;
      pen.shadowOffsetY = 4;
      pen.fillStyle = soft(element.color);
      pen.beginPath();
      rounded(pen, b, [3, 3, 14, 3]);
      pen.fill();
      pen.shadowColor = "transparent";
      write(pen, element, b, { pad: 14, colour: ON_SOFT });
    } else if (element.type === "shape") {
      const filled = element.fill && element.fill !== "none";
      const outline = element.stroke && element.stroke !== "none" ? ink(element.stroke) : filled ? null : colour("--ui-fg");
      const path = new Path2D(shapeOf(element.shape).path(b.w, b.h));
      pen.translate(b.x, b.y);
      if (filled) {
        pen.fillStyle = soft(element.fill);
        pen.fill(path);
      }
      if (outline) {
        pen.strokeStyle = outline;
        pen.lineWidth = 2;
        pen.setLineDash(element.dash ? [7, 6] : []);
        pen.stroke(path);
      }
      write(pen, element, { x: 0, y: 0, w: b.w, h: b.h }, { middle: true, colour: filled ? ON_SOFT : colour("--ui-fg") });
    } else if (element.type === "text") {
      write(pen, element, b, { pad: 0, colour: ink(element.color) });
    } else if (element.type === "table") {
      const rows = element.rows?.length ? element.rows : [[""]];
      const columns = Math.max(...rows.map((row) => row.length), 1);
      const [cw, ch] = [b.w / columns, b.h / rows.length];
      pen.fillStyle = colour("--ui-surface");
      pen.fillRect(b.x, b.y, b.w, b.h);
      pen.strokeStyle = colour("--ui-border");
      pen.lineWidth = 1;
      rows.forEach((row, r) => {
        for (let c = 0; c < columns; c += 1) {
          const cell = { x: b.x + c * cw, y: b.y + r * ch, w: cw, h: ch };
          pen.strokeRect(cell.x, cell.y, cell.w, cell.h);
          write(pen, { text: row[c] || "", size: "s", bold: r === 0 }, cell, { pad: 8, colour: colour("--ui-fg") });
        }
      });
    } else if (element.type === "sticker") {
      pen.font = `${Math.min(b.w, b.h) * 0.82}px sans-serif`;
      pen.textAlign = "center";
      pen.textBaseline = "middle";
      pen.fillText(element.emoji || "⭐", b.x + b.w / 2, b.y + b.h / 2);
    } else if (element.type === "image") {
      const image = pictures.get(element.id);
      if (image) pen.drawImage(image, b.x, b.y, b.w, b.h);
    } else if (element.type === "draw") {
      pen.strokeStyle = ink(element.color);
      pen.lineWidth = element.width || 3;
      pen.globalAlpha = element.marker ? 0.42 : 1;
      pen.stroke(new Path2D(stroke(element.points || [])));
    } else if (element.type === "line") {
      const r = route(element);
      const wide = element.width || 2;
      pen.strokeStyle = pen.fillStyle = ink(element.color);
      pen.lineWidth = wide;
      pen.setLineDash(element.dash ? [wide * 3, wide * 3] : []);
      pen.stroke(new Path2D(r.d));
      pen.setLineDash([]);
      for (const [kind, at, angle] of [[element.end ?? "arrow", r.b, r.intoB], [element.start ?? "none", r.a, r.intoA]]) {
        if (kind === "arrow") {
          const size = 6 + wide * 2.2;
          pen.beginPath();
          pen.moveTo(at.x - size * Math.cos(angle - 0.45), at.y - size * Math.sin(angle - 0.45));
          pen.lineTo(at.x, at.y);
          pen.lineTo(at.x - size * Math.cos(angle + 0.45), at.y - size * Math.sin(angle + 0.45));
          pen.stroke();
        } else if (kind === "dot") {
          pen.beginPath();
          pen.arc(at.x, at.y, 3 + wide, 0, Math.PI * 2);
          pen.fill();
        }
      }
      if (element.text) {
        pen.font = `13px ${getComputedStyle(document.body).fontFamily}`;
        const wide_ = pen.measureText(element.text).width + 12;
        pen.fillStyle = colour("--ui-stage");
        pen.fillRect(r.label.x - wide_ / 2, r.label.y - 10, wide_, 20);
        pen.fillStyle = colour("--ui-fg");
        pen.textAlign = "center";
        pen.textBaseline = "middle";
        pen.fillText(element.text, r.label.x, r.label.y);
      }
    }
  }
}

// ---- The board as words ----------------------------------------------------------------------

const said = (element) => {
  if (element.type === "table") return (element.rows || []).map((row) => row.join(" | ")).join("\n  ");
  return (element.text || element.title || element.emoji || "").trim();
};

// The sections with what lies in them, then what lies loose, then what is
// joined to what: a board as a list someone can paste elsewhere.
export function outlineText() {
  const list = elements();
  const lines = [state.board.title || window.BOARD?.title || "Board", ""];
  const placed = new Set();
  const items = (inside) =>
    inside
      .filter((element) => !["line", "draw", "comment", "image"].includes(element.type) && said(element))
      .sort((a, b) => (Math.abs(a.y - b.y) > 40 ? a.y - b.y : a.x - b.x))
      .map((element) => `- ${said(element).replace(/\n/g, "\n  ")}${element.votes ? ` (${element.votes} vote${element.votes === 1 ? "" : "s"})` : ""}`);
  for (const frame of sections()) {
    const inside = inFrame(frame, list);
    inside.forEach((element) => placed.add(element.id));
    lines.push(`## ${frame.title || "Section"}`, ...items(inside), "");
  }
  const loose = items(list.filter((element) => element.type !== "frame" && !placed.has(element.id)));
  if (loose.length) lines.push(sections().length ? "## Outside the sections" : "## On the board", ...loose, "");
  const name = (spec) => (spec?.id && byId(spec.id) ? said(byId(spec.id)).split("\n")[0] || byId(spec.id).type : null);
  const links = list
    .filter((element) => element.type === "line" && name(element.from) && name(element.to))
    .map((line) => `- ${name(line.from)} → ${name(line.to)}${line.text ? ` (${line.text})` : ""}`);
  if (links.length) lines.push("## What leads to what", ...links, "");
  return lines.join("\n").trimEnd() + "\n";
}

