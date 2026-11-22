// Looks at every slide as it is drawn and says what a reader would stumble
// over: text that runs off the slide or is cut off in its box, text too
// small or too faint to read, a picture that did not load.
//
// The page says it to the app (for the agent, which then fixes the slide
// before the user has to ask) and marks the slide's tile.

import { state, emit, tell } from "./store.js";
import { slideTitle } from "./core.js";

// At most this many things said about one slide: the first few are what
// needs doing; the rest follow from them.
const MOST = 5;

const luminance = ([r, g, b]) => {
  const part = (value) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * part(r) + 0.7152 * part(g) + 0.0722 * part(b);
};

let probe;
// [r, g, b, a] of any CSS colour, as this browser reads it.
function rgba(colour) {
  probe ||= document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  probe.clearRect(0, 0, 1, 1);
  probe.fillStyle = "rgba(0,0,0,0)";
  probe.fillStyle = colour;
  probe.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = probe.getImageData(0, 0, 1, 1).data;
  return [r, g, b, a / 255];
}

// What a text's ground is: the nearest fill behind it. `null` when a
// picture or a gradient is in between, where no one colour can be named.
function groundOf(element, slide) {
  for (let node = element; node; node = node.parentElement) {
    const styles = getComputedStyle(node);
    if (styles.backgroundImage !== "none") return null;
    const colour = rgba(styles.backgroundColor);
    if (colour[3] > 0.9) return colour;
    if (colour[3] > 0.05) return null;
    if (node === slide) break;
  }
  return [255, 255, 255, 1];
}

const named = (element) => {
  const text = element.textContent.replace(/\s+/g, " ").trim();
  const quoted = text ? ` “${text.length > 48 ? text.slice(0, 47) + "…" : text}”` : "";
  return `<${element.tagName.toLowerCase()}>${quoted}`;
};

// What does not fit on one slide, as it is drawn in `slide`.
export function inspect(slide, size) {
  const issues = [];
  const say = (kind, text) => {
    if (issues.length < MOST && !issues.some((issue) => issue.text === text)) issues.push({ kind, text });
  };
  if (slide.classList.contains("slide-missing")) {
    say("missing", "The slide's file is missing or is not listed under slides/.");
    return issues;
  }
  const frame = slide.getBoundingClientRect();
  if (frame.width === 0) return issues;
  const scale = frame.width / size[0];
  const [width, height] = size;
  for (const element of slide.querySelectorAll("*")) {
    if (issues.length >= MOST) break;
    const tag = element.tagName;
    if (tag === "STYLE" || tag === "BR" || element.closest("svg") !== (tag.toLowerCase() === "svg" ? element : null)) continue;
    const words = [...element.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim());
    const thing = words || ["IMG", "VIDEO", "TABLE"].includes(tag) || tag.toLowerCase() === "svg" || element.hasAttribute("data-chart");
    if (!thing) continue;
    const box = element.getBoundingClientRect();
    if (box.width === 0 && box.height === 0) continue;
    const left = (box.left - frame.left) / scale;
    const top = (box.top - frame.top) / scale;
    const right = left + box.width / scale;
    const bottom = top + box.height / scale;
    const over = Math.max(right - width, bottom - height, -left, -top);
    if (over > 3) {
      const side = over === right - width ? "right" : over === bottom - height ? "bottom" : over === -left ? "left" : "top";
      say("overflow", `${named(element)} runs ${Math.round(over)}px off the ${side} of the slide.`);
      continue;
    }
    if (tag === "IMG" && element.complete && element.naturalWidth === 0) {
      say("picture", `The picture ${element.getAttribute("src") || ""} did not load.`);
      continue;
    }
    if (!words) continue;
    const styles = getComputedStyle(element);
    if (styles.overflowY !== "visible" && element.scrollHeight > element.clientHeight + 3) {
      say("clipped", `${named(element)} is cut off: its text is ${Math.round(element.scrollHeight - element.clientHeight)}px taller than its box.`);
    }
    const fontSize = parseFloat(styles.fontSize);
    if (fontSize < height * 0.0165) {
      say("small", `${named(element)} is set in ${Math.round(fontSize)}px, too small to read from a seat (use ${Math.round(height * 0.02)}px or more).`);
    }
    const ink = rgba(styles.color);
    const ground = groundOf(element, slide);
    if (ground && ink[3] > 0.5) {
      const a = luminance(ink);
      const b = luminance(ground);
      const ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      const large = fontSize >= 44 || (fontSize >= 34 && parseInt(styles.fontWeight, 10) >= 600);
      if (ratio < (large ? 2.4 : 3.2)) {
        say("contrast", `${named(element)} is hard to read on its ground (contrast ${ratio.toFixed(1)} to 1).`);
      }
    }
  }
  return issues;
}

export class Checks {
  // `tiles`: file -> { host, box } of the strip, where every slide is drawn.
  constructor({ tiles }) {
    this.tiles = tiles;
    this.timer = 0;
  }

  // After the next quiet moment: a slide being typed in is not judged at
  // every letter.
  soon() {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.now(), 700);
  }

  async now() {
    clearTimeout(this.timer);
    // Text is measured in its own font, and pictures at their own size.
    await document.fonts?.ready;
    const slides = [];
    const issues = new Map();
    state.deck.slides.forEach((slide, index) => {
      const entry = this.tiles.get(slide.file);
      const drawn = entry?.host.shadowRoot?.querySelector(".slide");
      if (!drawn) return;
      // A tile far off in the strip is not laid out until it is looked at.
      entry.box.style.contentVisibility = "visible";
      const found = inspect(drawn, state.deck.size);
      entry.box.style.contentVisibility = "";
      if (found.length) issues.set(slide.file, found);
      slides.push({
        slide: index + 1,
        file: slide.file,
        title: slideTitle(drawn),
        skipped: Boolean(slide.skip),
        notes: Boolean(slide.notes),
        issues: found.map((issue) => issue.text),
      });
    });
    state.issues = issues;
    emit("issues");
    tell({ report: { version: state.version, size: state.deck.size, slides } });
  }
}
