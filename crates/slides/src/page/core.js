// What shows a deck: reading its files, drawing a slide, drawing charts, and
// presenting. The editor builds on it, the page a guest gets is little more
// than it, and an exported web page carries it inside.
//
// A slide is drawn in a shadow root of its own, so the deck's look
// (deck.css) reaches the slides and nothing else, and the editor's own
// styles never reach a slide.

export const SIZE = [1920, 1080];

// deck.json as an object with what the page relies on always there. What a
// deck says besides is kept as it is.
export function parseDeck(text) {
  let deck;
  try {
    deck = JSON.parse(text || "{}");
  } catch {
    deck = null;
  }
  const broken = deck === null || typeof deck !== "object" || Array.isArray(deck);
  if (broken) deck = {};
  const size =
    Array.isArray(deck.size) && deck.size[0] >= 16 && deck.size[1] >= 16
      ? [Number(deck.size[0]), Number(deck.size[1])]
      : [...SIZE];
  const slides = (Array.isArray(deck.slides) ? deck.slides : [])
    .filter((slide) => slide && typeof slide.file === "string")
    .map((slide) => ({ ...slide }));
  return {
    ...deck,
    title: typeof deck.title === "string" ? deck.title : "",
    size,
    slides,
    broken: broken && Boolean(text),
  };
}

// deck.json's text for a deck object (what `parseDeck` added is left out).
export function deckText(deck) {
  const { broken: _broken, ...kept } = deck;
  const slides = kept.slides.map((slide) => {
    const out = { file: slide.file };
    for (const [key, value] of Object.entries(slide)) {
      if (key === "file") continue;
      if (value === "" || value === false || value === null || value === undefined) continue;
      out[key] = value;
    }
    return out;
  });
  return JSON.stringify({ ...kept, slides }, null, 2) + "\n";
}

// The deck's look, shared by every slide drawn in this document.
export class Look {
  constructor(doc = document) {
    this.doc = doc;
    this.modern = "adoptedStyleSheets" in doc && typeof CSSStyleSheet === "function";
    this.baseText = "";
    this.deckText = "";
    this.roots = new Set();
    if (this.modern) {
      try {
        this.base = new CSSStyleSheet();
        this.deck = new CSSStyleSheet();
      } catch {
        this.modern = false;
      }
    }
    // A font a deck brings counts only when the page itself declares it.
    this.fonts = doc.createElement("style");
    this.fonts.dataset.deckFonts = "";
    doc.head.append(this.fonts);
  }

  setBase(css) {
    this.baseText = css;
    if (this.modern) this.base.replaceSync(css);
    else this.refresh();
  }

  // deck.css. `:root` there means the slide: a deck's variables are found
  // wherever they were put.
  set(css) {
    const text = String(css || "").replace(/:root\b/g, ":host");
    if (text === this.deckText) return;
    this.deckText = text;
    if (this.modern) {
      try {
        this.deck.replaceSync(text);
      } catch {
        this.deck.replaceSync("");
      }
    } else {
      this.refresh();
    }
    const faces = text.match(/@font-face\s*{[^}]*}/g) || [];
    this.fonts.textContent = faces.join("\n");
  }

  adopt(root) {
    if (this.modern) {
      root.adoptedStyleSheets = [this.base, this.deck];
      return;
    }
    this.roots.add(new WeakRef(root));
    this.styleOf(root).textContent = this.baseText + "\n" + this.deckText;
  }

  styleOf(root) {
    let style = root.querySelector("style[data-look]");
    if (!style) {
      style = this.doc.createElement("style");
      style.dataset.look = "";
      root.prepend(style);
    }
    return style;
  }

  refresh() {
    for (const ref of [...this.roots]) {
      const root = ref.deref();
      if (!root) this.roots.delete(ref);
      else this.styleOf(root).textContent = this.baseText + "\n" + this.deckText;
    }
  }
}

const DROPPED = "script, iframe, object, embed, link, meta, base, form";

// A slide's file as an element. What a slide has no use for and could do
// harm (scripts, frames, handlers) is left out: a deck is shown to guests,
// and may have started from a file of someone else's.
export function parseSlide(html, doc = document) {
  const slide = doc.createElement("section");
  if (html === null || html === undefined) {
    slide.className = "slide slide-missing";
    slide.textContent = "This slide's file is missing.";
    return slide;
  }
  const template = doc.createElement("template");
  template.innerHTML = html;
  const found = template.content.querySelector("section.slide, section");
  const made = found || slide;
  if (!found) {
    slide.className = "slide";
    slide.append(template.content);
  } else if (!found.classList.contains("slide")) {
    found.classList.add("slide");
  }
  for (const element of made.querySelectorAll(DROPPED)) element.remove();
  for (const element of [made, ...made.querySelectorAll("*")]) {
    for (const attribute of [...element.attributes]) {
      const name = attribute.name.toLowerCase();
      const value = attribute.value.trim().toLowerCase();
      if (name.startsWith("on")) element.removeAttribute(attribute.name);
      else if ((name === "href" || name === "src" || name === "xlink:href") && value.startsWith("javascript:")) {
        element.removeAttribute(attribute.name);
      }
    }
  }
  return made;
}

// Draws slide `html` in `host` (an element kept for it). The element drawn
// is returned; it is the slide as the file has it, for the editor to change.
export function mountSlide(host, html, size, look) {
  host.style.setProperty("--deck-w", size[0] + "px");
  host.style.setProperty("--deck-h", size[1] + "px");
  const root = host.shadowRoot || host.attachShadow({ mode: "open" });
  const slide = parseSlide(html, host.ownerDocument);
  const kept = look.modern ? [] : [look.styleOf(root)];
  root.replaceChildren(...kept, slide);
  look.adopt(root);
  dress(slide);
  return slide;
}

// What the page adds to a slide as drawn: charts from their numbers, and
// text made to fit where it was asked to.
export function dress(slide) {
  for (const chart of slide.querySelectorAll("[data-chart]")) drawChart(chart);
  fitText(slide);
}

// Text in a box that says `data-fit="shrink"` is set smaller until it fits.
export function fitText(slide) {
  for (const element of slide.querySelectorAll('[data-fit="shrink"]')) {
    element.style.removeProperty("font-size");
    if (!element.isConnected || element.clientHeight === 0) continue;
    const fits = () =>
      element.scrollHeight <= element.clientHeight + 1 && element.scrollWidth <= element.clientWidth + 1;
    if (fits()) continue;
    const full = parseFloat(getComputedStyle(element).fontSize) || 34;
    let low = 0.3;
    let high = 1;
    for (let step = 0; step < 7; step += 1) {
      const middle = (low + high) / 2;
      element.style.fontSize = full * middle + "px";
      if (fits()) low = middle;
      else high = middle;
    }
    element.style.fontSize = Math.floor(full * low) + "px";
  }
}

// ---- Charts ---------------------------------------------------------------

const CHART_COLOURS = [
  "var(--chart-1, var(--accent, #4f7cff))",
  "var(--chart-2, color-mix(in srgb, var(--accent, #4f7cff) 55%, var(--fg, #111)))",
  "var(--chart-3, color-mix(in srgb, var(--accent, #4f7cff) 45%, var(--bg, #fff)))",
  "var(--chart-4, var(--muted, #777))",
  "var(--chart-5, color-mix(in srgb, var(--fg, #111) 70%, var(--bg, #fff)))",
  "var(--chart-6, color-mix(in srgb, var(--accent, #4f7cff) 25%, var(--bg, #fff)))",
];

// The numbers a chart's element holds: { type, labels, series: [{ name,
// values }], unit }. Read forgivingly; `null` when there is nothing to draw.
export function chartSpec(element) {
  let spec;
  try {
    spec = JSON.parse(element.getAttribute("data-chart") || "");
  } catch {
    return null;
  }
  if (!spec || typeof spec !== "object") return null;
  const labels = (Array.isArray(spec.labels) ? spec.labels : []).map((label) => String(label));
  let series = Array.isArray(spec.series) ? spec.series : [];
  if (Array.isArray(spec.values)) series = [{ name: "", values: spec.values }];
  series = series
    .filter((one) => one && Array.isArray(one.values))
    .map((one) => ({
      name: String(one.name ?? ""),
      values: labels.map((_, index) => {
        const value = Number(one.values[index]);
        return Number.isFinite(value) ? value : 0;
      }),
    }));
  if (labels.length === 0 || series.length === 0) return null;
  const type = ["bar", "hbar", "stacked", "line", "area", "pie", "donut"].includes(spec.type) ? spec.type : "bar";
  return { type, labels, series, unit: typeof spec.unit === "string" ? spec.unit : "" };
}

const NS = "http://www.w3.org/2000/svg";
const short = (value) => {
  const size = Math.abs(value);
  if (size >= 1e9) return +(value / 1e9).toFixed(1) + "B";
  if (size >= 1e6) return +(value / 1e6).toFixed(1) + "M";
  if (size >= 1e4) return +(value / 1e3).toFixed(1) + "k";
  return String(+value.toFixed(size < 10 ? 1 : 0));
};

// A round number at or above `value`, for the top of an axis.
function ceiling(value) {
  if (value <= 0) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  for (const step of [1, 2, 2.5, 5, 10]) if (step * power >= value) return step * power;
  return 10 * power;
}

function drawChart(element) {
  if (!element._chartSized && typeof ResizeObserver === "function") {
    // Drawn again when its place changes size (the user resized it, or the
    // slide was first laid out).
    element._chartSized = new ResizeObserver(() => paintChart(element));
    element._chartSized.observe(element);
  }
  paintChart(element);
}

function paintChart(element) {
  const spec = chartSpec(element);
  const width = element.clientWidth;
  const height = element.clientHeight;
  const key = element.getAttribute("data-chart") + "|" + width + "x" + height;
  if (element._chartKey === key) return;
  element._chartKey = key;
  for (const old of element.querySelectorAll(":scope > svg.chart-drawn")) old.remove();
  if (!spec || width < 40 || height < 40) return;
  const svg = element.ownerDocument.createElementNS(NS, "svg");
  svg.setAttribute("class", "chart-drawn");
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", spec.series.map((one) => one.name).join(", ") || "Chart");
  svg.innerHTML = chartMarkup(spec, width, height);
  element.append(svg);
}

const esc = (text) => String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

// The chart as SVG. Sizes are the slide's pixels; colours are the deck's.
export function chartMarkup(spec, width, height) {
  const text = Math.max(18, Math.min(26, Math.round(height / 22)));
  const label = `font-size:${text}px;fill:var(--muted,#777);font-family:var(--font-body,sans-serif)`;
  const value = `font-size:${text}px;font-weight:650;fill:var(--fg,#111);font-family:var(--font-body,sans-serif)`;
  const colour = (index) => CHART_COLOURS[index % CHART_COLOURS.length];
  const many = spec.series.length > 1;
  let out = "";
  let legendHeight = 0;
  if (many || spec.type === "pie" || spec.type === "donut") {
    const names = many ? spec.series.map((one) => one.name) : spec.labels;
    if (spec.type !== "pie" && spec.type !== "donut") {
      legendHeight = text * 2;
      let x = 0;
      names.forEach((name, index) => {
        out += `<rect x="${x}" y="${text * 0.25}" width="${text * 0.8}" height="${text * 0.8}" rx="4" style="fill:${colour(index)}"/>`;
        out += `<text x="${x + text * 1.2}" y="${text}" style="${label}">${esc(name)}</text>`;
        x += text * 1.2 + name.length * text * 0.58 + text * 1.6;
      });
    }
  }
  if (spec.type === "pie" || spec.type === "donut") {
    const values = spec.series[0].values.map((v) => Math.max(0, v));
    const total = values.reduce((a, b) => a + b, 0) || 1;
    const radius = Math.min(height, width * 0.55) / 2 - 4;
    const cx = radius + 4;
    const cy = height / 2;
    let angle = -Math.PI / 2;
    values.forEach((v, index) => {
      const sweep = (v / total) * Math.PI * 2;
      const end = angle + sweep;
      const large = sweep > Math.PI ? 1 : 0;
      const point = (a, r) => `${(cx + Math.cos(a) * r).toFixed(1)} ${(cy + Math.sin(a) * r).toFixed(1)}`;
      if (sweep >= Math.PI * 2 - 1e-6) {
        out += `<circle cx="${cx}" cy="${cy}" r="${radius}" style="fill:${colour(index)}"/>`;
      } else if (sweep > 0) {
        out += `<path d="M ${cx} ${cy} L ${point(angle, radius)} A ${radius} ${radius} 0 ${large} 1 ${point(end, radius)} Z" style="fill:${colour(index)}"/>`;
      }
      angle = end;
    });
    if (spec.type === "donut") out += `<circle cx="${cx}" cy="${cy}" r="${radius * 0.58}" style="fill:var(--bg,#fff)"/>`;
    const x = cx + radius + text * 2;
    const row = text * 1.9;
    let y = cy - (values.length * row) / 2 + text;
    values.forEach((v, index) => {
      out += `<rect x="${x}" y="${y - text * 0.8}" width="${text * 0.8}" height="${text * 0.8}" rx="4" style="fill:${colour(index)}"/>`;
      out += `<text x="${x + text * 1.3}" y="${y}" style="${label}">${esc(spec.labels[index])}</text>`;
      out += `<text x="${width}" y="${y}" text-anchor="end" style="${value}">${Math.round((v / total) * 100)}%</text>`;
      y += row;
    });
    return out;
  }
  const stacked = spec.type === "stacked";
  const count = spec.labels.length;
  const totals = spec.labels.map((_, i) => spec.series.reduce((sum, one) => sum + Math.max(0, one.values[i]), 0));
  const all = spec.series.flatMap((one) => one.values);
  const top = ceiling(Math.max(...(stacked ? totals : all), 0));
  const bottom = stacked ? 0 : Math.min(0, ...all);
  const span = top - bottom || 1;
  if (spec.type === "hbar") {
    const longest = Math.max(...spec.labels.map((name) => name.length));
    const left = Math.min(width * 0.36, longest * text * 0.56 + text);
    const right = text * 3.5;
    const plot = width - left - right;
    const band = (height - legendHeight) / count;
    const thick = Math.min(band * 0.62, 64) / (many ? spec.series.length : 1);
    spec.labels.forEach((name, i) => {
      const middle = legendHeight + band * i + band / 2;
      out += `<text x="${left - text * 0.7}" y="${middle + text * 0.35}" text-anchor="end" style="${label}">${esc(name)}</text>`;
      spec.series.forEach((one, s) => {
        const long = (Math.max(0, one.values[i]) / span) * plot;
        const y = middle - (thick * spec.series.length) / 2 + thick * s;
        out += `<rect x="${left}" y="${y}" width="${Math.max(long, 2)}" height="${thick - 4}" rx="6" style="fill:${colour(s)}"/>`;
        out += `<text x="${left + long + text * 0.4}" y="${y + thick / 2 + text * 0.3}" style="${value}">${short(one.values[i])}${esc(spec.unit)}</text>`;
      });
    });
    return out;
  }
  const left = text * 3.2;
  const base = height - text * 2;
  const plotTop = legendHeight + text;
  const plotHeight = base - plotTop;
  const plot = width - left;
  const y = (v) => base - ((v - bottom) / span) * plotHeight;
  for (let line = 0; line <= 4; line += 1) {
    const at = bottom + (span * line) / 4;
    out += `<line x1="${left}" x2="${width}" y1="${y(at)}" y2="${y(at)}" style="stroke:var(--line,#ddd);stroke-width:${line === 0 ? 2 : 1}"/>`;
    out += `<text x="${left - text * 0.6}" y="${y(at) + text * 0.35}" text-anchor="end" style="${label}">${short(at)}${esc(spec.unit)}</text>`;
  }
  const band = plot / count;
  spec.labels.forEach((name, i) => {
    out += `<text x="${left + band * i + band / 2}" y="${height - text * 0.4}" text-anchor="middle" style="${label}">${esc(name)}</text>`;
  });
  if (spec.type === "line" || spec.type === "area") {
    spec.series.forEach((one, s) => {
      const points = one.values.map((v, i) => `${(left + band * i + band / 2).toFixed(1)},${y(v).toFixed(1)}`);
      if (spec.type === "area") {
        out += `<polygon points="${left + band / 2},${base} ${points.join(" ")} ${left + band * (count - 1) + band / 2},${base}" style="fill:${colour(s)};opacity:0.22"/>`;
      }
      out += `<polyline points="${points.join(" ")}" style="fill:none;stroke:${colour(s)};stroke-width:5;stroke-linejoin:round;stroke-linecap:round"/>`;
      one.values.forEach((v, i) => {
        out += `<circle cx="${left + band * i + band / 2}" cy="${y(v)}" r="7" style="fill:${colour(s)}"/>`;
      });
    });
    return out;
  }
  const groups = stacked ? 1 : spec.series.length;
  const thick = Math.min((band * 0.7) / groups, 140);
  spec.labels.forEach((_, i) => {
    let height0 = 0;
    spec.series.forEach((one, s) => {
      const v = one.values[i];
      const x = left + band * i + band / 2 - (thick * groups) / 2 + (stacked ? 0 : thick * s);
      const from = stacked ? height0 : 0;
      const to = stacked ? height0 + Math.max(0, v) : v;
      const upper = Math.min(y(from), y(to));
      const tall = Math.max(Math.abs(y(from) - y(to)), 2);
      out += `<rect x="${x + 3}" y="${upper}" width="${thick - 6}" height="${tall}" rx="${stacked ? 0 : 6}" style="fill:${colour(s)}"/>`;
      if (!stacked && thick > text * 2.2) {
        out += `<text x="${x + thick / 2}" y="${upper - text * 0.4}" text-anchor="middle" style="${value}">${short(v)}${esc(spec.unit)}</text>`;
      }
      height0 = to;
    });
  });
  return out;
}

// ---- Fitting a slide into a place ----------------------------------------

// Sizes `frame` (which holds a slide's host as its only child) to fit
// `within`, and scales the host to fill the frame. Answers with the scale.
export function fitFrame(frame, size, within, margin = 0) {
  const scale = Math.max(0.01, Math.min((within.width - margin * 2) / size[0], (within.height - margin * 2) / size[1]));
  scaleFrame(frame, size, scale);
  return scale;
}

export function scaleFrame(frame, size, scale) {
  frame.style.width = size[0] * scale + "px";
  frame.style.height = size[1] * scale + "px";
  const host = frame.firstElementChild;
  if (host) {
    host.style.transformOrigin = "0 0";
    host.style.transform = `scale(${scale})`;
  }
}

// The first words of a slide: its heading, or what it starts with.
export function slideTitle(slide) {
  const heading = slide.querySelector("h1, h2, h3, blockquote");
  const text = (heading || slide).textContent.replace(/\s+/g, " ").trim();
  return text.length > 80 ? text.slice(0, 79) + "…" : text;
}

// ---- Presenting -----------------------------------------------------------

// Shows the slides of a deck one at a time, filling `stage`. `slides` is a
// list of { html, transition }, in order, without the ones left out.
export class Show {
  constructor(stage, { size, look, slides, at = 0, transition = "fade", onChange = () => {}, onExit = () => {} }) {
    Object.assign(this, { stage, size, look, slides, at, transition, onChange, onExit });
    this.blank = false;
    stage.classList.add("show");
    this.layer = this.newLayer();
    this.draw(this.layer, at);
    this.sized = () => this.fit();
    this.resizer = new ResizeObserver(this.sized);
    this.resizer.observe(stage);
    this.fit();
    this.keys = (event) => this.key(event);
    this.clicked = (event) => {
      const path = event.composedPath().filter((node) => node instanceof Element);
      const link = path.find((node) => node.tagName === "A" && node.getAttribute("href"));
      if (link) {
        // A link leaves the show standing: it opens beside it.
        event.preventDefault();
        window.open(link.href, "_blank", "noopener");
        return;
      }
      if (path.some((node) => node.matches("button, video, audio, [data-show-control]"))) return;
      this.next();
    };
    stage.ownerDocument.addEventListener("keydown", this.keys, true);
    stage.addEventListener("click", this.clicked);
  }

  newLayer() {
    const frame = this.stage.ownerDocument.createElement("div");
    frame.className = "show-frame";
    frame.append(this.stage.ownerDocument.createElement("div"));
    this.stage.append(frame);
    return frame;
  }

  draw(frame, index) {
    const slide = this.slides[index];
    mountSlide(frame.firstElementChild, slide ? slide.html : null, this.size, this.look);
  }

  fit() {
    const within = this.stage.getBoundingClientRect();
    for (const frame of this.stage.querySelectorAll(".show-frame")) fitFrame(frame, this.size, within);
  }

  // Goes to slide `index`. `told` is false when a presenter elsewhere said
  // so, and there is nobody to tell in turn.
  go(index, told = true) {
    const to = Math.max(0, Math.min(this.slides.length - 1, index));
    if (to === this.at || this.slides.length === 0) return;
    const kind = this.slides[to].transition || this.transition || "none";
    const forward = to > this.at;
    const old = this.layer;
    const next = this.newLayer();
    this.draw(next, to);
    this.layer = next;
    this.at = to;
    this.fit();
    const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (kind === "none" || calm || !next.animate) {
      old.remove();
    } else {
      const time = { duration: 320, easing: "cubic-bezier(.2,.7,.2,1)", fill: "both" };
      const from = kind === "slide" ? { transform: `translateX(${forward ? 6 : -6}%)`, opacity: 0 } : { opacity: 0 };
      const leave = kind === "slide" ? { transform: `translateX(${forward ? -6 : 6}%)`, opacity: 0 } : { opacity: 0 };
      next.animate([from, { transform: "none", opacity: 1 }], time);
      old.animate([{ opacity: 1 }, leave], time).finished.then(
        () => old.remove(),
        () => old.remove(),
      );
    }
    if (told) this.onChange(to);
  }

  next() {
    this.go(this.at + 1);
  }

  previous() {
    this.go(this.at - 1);
  }

  key(event) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const key = event.key;
    let handled = true;
    if (["ArrowRight", "ArrowDown", " ", "PageDown", "Enter", "n", "N"].includes(key)) this.next();
    else if (["ArrowLeft", "ArrowUp", "PageUp", "Backspace", "p", "P"].includes(key)) this.previous();
    else if (key === "Home") this.go(0);
    else if (key === "End") this.go(this.slides.length - 1);
    else if (key === "b" || key === "B" || key === ".") {
      this.blank = !this.blank;
      this.stage.classList.toggle("show-blank", this.blank);
    } else if (key === "Escape") this.onExit();
    else handled = false;
    if (handled) {
      event.preventDefault();
      event.stopPropagation();
    }
  }

  // The slides changed under it (someone edited the deck meanwhile).
  update(slides) {
    this.slides = slides;
    this.at = Math.max(0, Math.min(slides.length - 1, this.at));
    this.draw(this.layer, this.at);
    this.fit();
  }

  close() {
    this.resizer.disconnect();
    this.stage.ownerDocument.removeEventListener("keydown", this.keys, true);
    this.stage.removeEventListener("click", this.clicked);
    this.stage.classList.remove("show", "show-blank");
    for (const frame of this.stage.querySelectorAll(".show-frame")) frame.remove();
  }
}
