// Measures every slide as it is drawn here, for an export to PowerPoint:
// where each thing is, how large, in which colour and font. PowerPoint
// lays text out itself, so what it is given is each piece of text with its
// box and its styles, each picture with its place, and each fill as a
// shape; a table and a chart go as a table and a chart.

import { mountSlide, chartSpec } from "./core.js";

// `deck` is deck.json as read; `textOf(file)` a slide's file.
export async function measureDeck({ look, deck, textOf, title = "" }) {
  const [width, height] = deck.size;
  // Drawn at its own size, out of sight, so nothing on the page moves.
  const host = document.createElement("div");
  const holder = document.createElement("div");
  holder.style.cssText = `position:fixed;left:-100000px;top:0;width:${width}px;height:${height}px;overflow:hidden;pointer-events:none`;
  holder.append(host);
  document.body.append(holder);
  const slides = [];
  try {
    await document.fonts?.ready;
    for (const slide of deck.slides) {
      if (slide.skip) continue;
      const drawn = mountSlide(host, textOf(slide.file), deck.size, look);
      await settled(drawn);
      slides.push({
        file: slide.file,
        notes: slide.notes || "",
        transition: slide.transition || deck.transition || "",
        ...(await measureSlide(drawn)),
      });
    }
  } finally {
    holder.remove();
  }
  return { title: deck.title || title, size: deck.size, slides };
}

// Its pictures are loaded, and it is laid out.
async function settled(slide) {
  await Promise.all(
    [...slide.querySelectorAll("img")].map((image) =>
      image.complete ? null : new Promise((done) => ((image.onload = done), (image.onerror = done))),
    ),
  );
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
}

let probe;
// "#rrggbb" and how see-through (0 to 1) of any CSS colour.
function colour(value) {
  probe ||= document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  probe.clearRect(0, 0, 1, 1);
  probe.fillStyle = "rgba(0,0,0,0)";
  probe.fillStyle = value;
  probe.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = probe.getImageData(0, 0, 1, 1).data;
  if (a === 0) return { hex: "", alpha: 0 };
  // The canvas keeps colours multiplied by how solid they are.
  const un = (part) => Math.min(255, Math.round((part * 255) / a));
  const two = (part) => un(part).toString(16).padStart(2, "0");
  return { hex: "#" + two(r) + two(g) + two(b), alpha: +(a / 255).toFixed(3) };
}

// The first colour of a gradient, to stand in for it.
function firstStop(image) {
  const found = /(rgba?\([^)]*\)|#[0-9a-f]{3,8}|color\([^)]*\))/i.exec(image);
  return found ? colour(found[1]) : { hex: "", alpha: 0 };
}

const INLINE = new Set(["SPAN", "STRONG", "EM", "B", "I", "U", "A", "CODE", "SMALL", "MARK", "SUB", "SUP", "S", "BR"]);
const round = (value) => Math.round(value * 100) / 100;

async function measureSlide(slide) {
  const frame = slide.getBoundingClientRect();
  const items = [];
  const boxOf = (element) => {
    const box = element.getBoundingClientRect();
    const turned = turnOf(element);
    if (turned && element.offsetWidth) {
      // The box as it is before it is turned, about the same middle.
      const cx = box.x + box.width / 2 - frame.x;
      const cy = box.y + box.height / 2 - frame.y;
      return { x: round(cx - element.offsetWidth / 2), y: round(cy - element.offsetHeight / 2), w: element.offsetWidth, h: element.offsetHeight, rotate: turned };
    }
    return { x: round(box.x - frame.x), y: round(box.y - frame.y), w: round(box.width), h: round(box.height), rotate: 0 };
  };
  const seen = (element) => {
    const styles = getComputedStyle(element);
    if (styles.display === "none" || styles.visibility !== "visible" || parseFloat(styles.opacity) === 0) return null;
    const box = element.getBoundingClientRect();
    return box.width > 0.5 && box.height > 0.5 ? styles : null;
  };

  const slideStyles = getComputedStyle(slide);
  let background = colour(slideStyles.backgroundColor);
  if (!background.hex && slideStyles.backgroundImage !== "none") background = firstStop(slideStyles.backgroundImage);

  // The fill, outline and corners of an element (or of what it draws
  // before or after itself), as a shape. An outline on every side is the
  // shape's own; one on some sides only (a card's coloured top) is drawn
  // as shapes of its own.
  const decoration = (element, styles, box, pseudo) => {
    let fill = colour(styles.backgroundColor);
    if (!fill.hex && styles.backgroundImage.includes("gradient")) fill = firstStop(styles.backgroundImage);
    const sides = ["Top", "Right", "Bottom", "Left"].map((side) => {
      const width = parseFloat(styles[`border${side}Width`]) || 0;
      const shown = width > 0 && !["none", "hidden"].includes(styles[`border${side}Style`]);
      const ink = shown ? colour(styles[`border${side}Color`]) : { hex: "", alpha: 0 };
      return { side, width, ink, shown: shown && Boolean(ink.hex) };
    });
    const around = sides.every((side) => side.shown && side.width === sides[0].width && side.ink.hex === sides[0].ink.hex);
    const some = sides.filter((side) => side.shown);
    if (!fill.hex && some.length === 0) return;
    const radius = parseFloat(styles.borderTopLeftRadius) || 0;
    const percent = styles.borderTopLeftRadius.includes("%");
    const round50 = percent ? parseFloat(styles.borderTopLeftRadius) >= 50 : radius >= Math.min(box.w, box.h) / 2 - 0.5;
    const shape = (!pseudo && element.getAttribute("data-shape")) || (round50 ? "ellipse" : radius > 0 ? "rounded" : "rect");
    const rounded = round(percent ? (Math.min(box.w, box.h) * parseFloat(styles.borderTopLeftRadius)) / 100 : radius);
    const opacity = +parseFloat(styles.opacity).toFixed(3);
    const plain = (place, ink, more = {}) => ({ kind: "shape", shape: "rect", ...place, fill: ink.hex, fillAlpha: ink.alpha, stroke: "", strokeWidth: 0, radius: 0, opacity, rotate: box.rotate, ...more });
    let body = { ...box };
    if (!around && some.length === 1 && shape === "rounded" && fill.hex) {
      // One coloured side on a rounded box: the box in that colour, and the
      // box itself over it, moved in by the side's width.
      const { side, width, ink } = some[0];
      items.push(plain(box, ink, { shape, radius: rounded }));
      if (side === "Top") body = { ...box, y: box.y + width, h: box.h - width };
      if (side === "Bottom") body = { ...box, h: box.h - width };
      if (side === "Left") body = { ...box, x: box.x + width, w: box.w - width };
      if (side === "Right") body = { ...box, w: box.w - width };
      some.length = 0;
    }
    if (fill.hex || around) {
      items.push({
        kind: "shape",
        ...body,
        shape,
        fill: fill.hex,
        fillAlpha: fill.alpha,
        stroke: around ? sides[0].ink.hex : "",
        strokeWidth: around ? sides[0].width : 0,
        radius: rounded,
        opacity,
      });
    }
    if (around) return;
    for (const { side, width, ink } of some) {
      const place =
        side === "Top"
          ? { x: box.x, y: box.y, w: box.w, h: width }
          : side === "Bottom"
            ? { x: box.x, y: box.y + box.h - width, w: box.w, h: width }
            : side === "Left"
              ? { x: box.x, y: box.y, w: width, h: box.h }
              : { x: box.x + box.w - width, y: box.y, w: width, h: box.h };
      items.push(plain(place, ink));
    }
  };

  // What an element draws before or after itself (a look's circle on the
  // title slide), where it is set at a place of its own.
  const pseudo = (element, which, parent) => {
    const styles = getComputedStyle(element, which);
    if (!styles.content || styles.content === "none" || styles.content === "normal") return;
    if (styles.position !== "absolute" || styles.display === "none") return;
    const w = parseFloat(styles.width);
    const h = parseFloat(styles.height);
    if (!(w > 0 && h > 0)) return;
    const left = parseFloat(styles.left);
    const top = parseFloat(styles.top);
    const right = parseFloat(styles.right);
    const bottom = parseFloat(styles.bottom);
    const x = Number.isFinite(left) ? left : parent.w - right - w;
    const y = Number.isFinite(top) ? top : parent.h - bottom - h;
    decoration(element, styles, { x: round(parent.x + x), y: round(parent.y + y), w, h, rotate: 0 }, true);
  };

  const walk = async (element) => {
    const styles = seen(element);
    if (!styles) return;
    const tag = element.tagName;
    const box = boxOf(element);
    if (tag === "STYLE") return;
    if (tag === "IMG") {
      const src = element.getAttribute("src") || "";
      const picture = { kind: "picture", ...box, src, radius: parseFloat(styles.borderTopLeftRadius) || 0, crop: cropOf(element, styles) };
      // PowerPoint shows PNG, JPEG and GIF; anything else goes as a
      // picture of itself.
      if (!/\.(png|jpe?g|gif)([?#].*)?$/i.test(src) || /^(data|https?):/i.test(src)) {
        const data = pictureOf(element);
        if (!data) return;
        picture.data = data;
        picture.src = "";
      }
      items.push(picture);
      return;
    }
    if (tag === "VIDEO" || tag === "AUDIO") {
      items.push({ kind: "shape", ...box, shape: "rounded", fill: "#1c1c1e", fillAlpha: 1, stroke: "", strokeWidth: 0, radius: 16, opacity: 1 });
      items.push({ kind: "text", ...box, valign: "middle", wrap: true, paragraphs: [{ align: "center", bullet: "none", level: 0, lineHeight: 1.2, spaceAfter: 0, runs: [{ text: "Video: " + (element.getAttribute("src") || ""), font: "Arial", size: 24, bold: false, italic: false, underline: false, color: "#ffffff", alpha: 1, link: "" }] }] });
      return;
    }
    if (element.hasAttribute("data-chart")) {
      const spec = chartSpec(element);
      if (spec) {
        items.push({
          kind: "chart",
          ...box,
          spec,
          colors: chartColours(element, spec),
          text: colour(styles.getPropertyValue("--muted") || styles.color).hex || colour(styles.color).hex,
          font: styles.fontFamily.split(",")[0].trim().replace(/^["']|["']$/g, ""),
        });
      }
      return;
    }
    if (tag.toLowerCase() === "svg") {
      const data = await drawingOf(element, box);
      if (data) items.push({ kind: "picture", ...box, data, radius: 0, crop: null });
      return;
    }
    if (tag === "TABLE") {
      items.push(tableOf(element, box));
      return;
    }
    if (!element.hasAttribute("data-shape") || !["line", "connector"].includes(element.getAttribute("data-shape"))) {
      pseudo(element, "::before", box);
    }
    if (element !== slide) decoration(element, styles, box, false);
    if (element.hasAttribute("data-shape") && ["line", "connector"].includes(element.getAttribute("data-shape"))) return;
    // Its words, as one box of text; what it holds besides is gone through
    // in turn.
    const text = textOf(element, styles, frame);
    if (text) items.push(text);
    for (const child of element.children) {
      if (text && (INLINE.has(child.tagName) || text.took.has(child))) continue;
      await walk(child);
    }
    pseudo(element, "::after", box);
  };
  pseudo(slide, "::before", { x: 0, y: 0, w: frame.width, h: frame.height });
  for (const child of slide.children) await walk(child);
  pseudo(slide, "::after", { x: 0, y: 0, w: frame.width, h: frame.height });
  for (const item of items) delete item.took;
  return { background: background.hex || "#ffffff", items };
}

function turnOf(element) {
  const styles = getComputedStyle(element);
  if (styles.rotate && styles.rotate !== "none") return parseFloat(styles.rotate) || 0;
  const matrix = /matrix\(([^)]+)\)/.exec(styles.transform || "");
  if (!matrix) return 0;
  const [a, b] = matrix[1].split(",").map(Number);
  return Math.round((Math.atan2(b, a) * 180) / Math.PI);
}

// How much of a picture's file is outside its box (it fills its box by
// being cut), as parts of its width and height.
function cropOf(image, styles) {
  if (styles.objectFit !== "cover" || !image.naturalWidth) return null;
  const box = image.getBoundingClientRect();
  const shown = box.width / box.height;
  const own = image.naturalWidth / image.naturalHeight;
  if (Math.abs(shown - own) < 0.01) return null;
  if (own > shown) {
    const cut = (1 - shown / own) / 2;
    return { left: cut, right: cut, top: 0, bottom: 0 };
  }
  const cut = (1 - own / shown) / 2;
  return { left: 0, right: 0, top: cut, bottom: cut };
}

// The colours a chart's series are drawn in, as they came out.
function chartColours(element, spec) {
  // Its bars, slices and lines; not its words, nor the hole of a doughnut.
  const marks = [...element.querySelectorAll("svg.chart-drawn :is(rect, path, polyline)")];
  const count = spec.type === "pie" || spec.type === "donut" ? spec.labels.length : spec.series.length;
  const found = [];
  for (const node of marks) {
    const styles = getComputedStyle(node);
    const paint = node.tagName === "polyline" ? styles.stroke : styles.fill;
    const hex = colour(paint).hex;
    if (hex && !found.includes(hex)) found.push(hex);
    if (found.length >= count) break;
  }
  return found;
}

// A picture as a PNG of itself, at the size of its own file.
function pictureOf(image) {
  if (!image.naturalWidth) return null;
  const canvas = document.createElement("canvas");
  // An SVG has no pixels of its own: twice its box is sharp enough.
  const box = image.getBoundingClientRect();
  const vector = /\.svg([?#].*)?$/i.test(image.getAttribute("src") || "");
  canvas.width = vector ? Math.ceil(box.width * 2) : image.naturalWidth;
  canvas.height = vector ? Math.ceil(box.height * 2) : image.naturalHeight;
  canvas.getContext("2d").drawImage(image, 0, 0, canvas.width, canvas.height);
  try {
    return canvas.toDataURL("image/png");
  } catch {
    return null;
  }
}

// A drawing as a picture: PowerPoint has no CSS, so its colours are written
// into it as they are here.
async function drawingOf(svg, box) {
  const copy = svg.cloneNode(true);
  const pairs = [[svg, copy]];
  const originals = svg.querySelectorAll("*");
  const copies = copy.querySelectorAll("*");
  originals.forEach((node, index) => pairs.push([node, copies[index]]));
  for (const [node, twin] of pairs) {
    const styles = getComputedStyle(node);
    for (const name of ["fill", "stroke", "stroke-width", "opacity", "font-family", "font-size", "font-weight", "color"]) {
      twin.style.setProperty(name, styles.getPropertyValue(name));
    }
  }
  copy.setAttribute("xmlns", "http://www.w3.org/2000/svg");
  copy.setAttribute("width", box.w);
  copy.setAttribute("height", box.h);
  const text = new XMLSerializer().serializeToString(copy);
  const image = new Image();
  const loaded = new Promise((done) => ((image.onload = () => done(true)), (image.onerror = () => done(false))));
  image.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(text);
  if (!(await loaded)) return null;
  const canvas = document.createElement("canvas");
  canvas.width = Math.ceil(box.w * 2);
  canvas.height = Math.ceil(box.h * 2);
  canvas.getContext("2d").drawImage(image, 0, 0, canvas.width, canvas.height);
  try {
    return canvas.toDataURL("image/png");
  } catch {
    return null;
  }
}

// How the words inside an element are set.
function runStyle(node) {
  const styles = getComputedStyle(node);
  const ink = colour(styles.color);
  const family = styles.fontFamily.split(",")[0].trim().replace(/^["']|["']$/g, "");
  const link = node.closest("a")?.getAttribute("href") || "";
  return {
    font: family,
    size: round(parseFloat(styles.fontSize)),
    bold: parseInt(styles.fontWeight, 10) >= 600,
    italic: styles.fontStyle === "italic",
    underline: styles.textDecorationLine.includes("underline"),
    strike: styles.textDecorationLine.includes("line-through"),
    color: ink.hex || "#000000",
    alpha: ink.alpha,
    spacing: styles.letterSpacing === "normal" ? 0 : round(parseFloat(styles.letterSpacing)),
    link: /^\s*javascript:/i.test(link) ? "" : link,
    upper: styles.textTransform === "uppercase",
    mono: /mono|courier|consol|menlo/i.test(family),
  };
}

const sameRun = (a, b) => ["font", "size", "bold", "italic", "underline", "strike", "color", "alpha", "link", "spacing"].every((key) => a[key] === b[key]);

// The runs of the words directly in `element` and in what is set inline in
// it. `took` gathers the block children whose words were taken too (the
// items of a list), so they are not gone through again.
function paragraphOf(element, took) {
  const runs = [];
  const push = (text, node) => {
    const style = runStyle(node);
    const said = style.upper ? text.toUpperCase() : text;
    delete style.upper;
    const last = runs[runs.length - 1];
    if (last && sameRun(last, style)) last.text += said;
    else runs.push({ text: said, ...style });
  };
  const visit = (node) => {
    for (const child of node.childNodes) {
      if (child.nodeType === 3) {
        const text = child.textContent.replace(/\s+/g, " ");
        if (text) push(text, child.parentElement);
      } else if (child.nodeType === 1) {
        if (child.tagName === "BR") push("\n", node);
        else if (INLINE.has(child.tagName) && getComputedStyle(child).display.startsWith("inline")) visit(child);
      }
    }
  };
  visit(element);
  // Spaces at the ends of a line are not set.
  if (runs.length) {
    runs[0].text = runs[0].text.replace(/^ +/, "");
    runs[runs.length - 1].text = runs[runs.length - 1].text.replace(/ +$/, "");
  }
  return runs.filter((run) => run.text !== "");
}

function paragraphStyle(element, level, bullet) {
  const styles = getComputedStyle(element);
  const size = parseFloat(styles.fontSize) || 16;
  const aligned = styles.textAlign === "start" ? "left" : styles.textAlign === "end" ? "right" : styles.textAlign;
  return {
    align: ["left", "center", "right", "justify"].includes(aligned) ? aligned : "left",
    bullet,
    level,
    lineHeight: styles.lineHeight === "normal" ? 1.2 : round(parseFloat(styles.lineHeight) / size),
    spaceAfter: round(parseFloat(styles.marginBottom) || 0),
  };
}

// An element's words as a box of text, or nothing when it has none of its
// own. A list is one box, a paragraph an item.
function textOf(element, styles, frame) {
  const took = new Set();
  const paragraphs = [];
  const tag = element.tagName;
  const list = tag === "UL" || tag === "OL";
  // How far an item's words stand right of the list's own edge: where
  // PowerPoint puts them, with the bullet before.
  let hang = 0;
  if (list) {
    const items = (within, level) => {
      for (const item of within.children) {
        if (item.tagName !== "LI") continue;
        took.add(item);
        const runs = paragraphOf(item, took);
        if (runs.length) {
          if (!hang) {
            const itemStyles = getComputedStyle(item);
            hang = item.getBoundingClientRect().x + (parseFloat(itemStyles.paddingLeft) || 0) - element.getBoundingClientRect().x;
          }
          paragraphs.push({
            ...paragraphStyle(item, level, within.tagName === "OL" ? "number" : "bullet"),
            bulletColor: colour(getComputedStyle(item, "::marker").color).hex,
            runs,
          });
        }
        for (const inner of item.children) if (inner.tagName === "UL" || inner.tagName === "OL") items(inner, level + 1);
      }
    };
    items(element, 0);
  } else {
    const own = [...element.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim());
    if (!own) return null;
    const runs = paragraphOf(element, took);
    if (runs.length) paragraphs.push({ ...paragraphStyle(element, 0, "none"), runs });
  }
  if (paragraphs.length === 0) return null;
  const box = element.getBoundingClientRect();
  const pad = (side) => (parseFloat(styles[`padding${side}`]) || 0) + (parseFloat(styles[`border${side}Width`]) || 0);
  const first = paragraphs[0];
  const lineHeight = first.runs[0].size * first.lineHeight;
  const height = box.height - pad("Top") - pad("Bottom");
  const centred = styles.display.includes("flex") && styles.alignItems === "center";
  return {
    kind: "text",
    // A list's box starts at its own edge: its bullets stand inside it.
    x: round(box.x - frame.x + (list ? 0 : pad("Left"))),
    y: round(box.y - frame.y + pad("Top")),
    w: round(box.width - (list ? 0 : pad("Left")) - pad("Right")),
    h: round(height),
    hang: round(hang),
    rotate: turnOf(element),
    valign: centred ? "middle" : "top",
    // One line stays one line, whatever PowerPoint's own measure of it.
    wrap: paragraphs.length > 1 || height > lineHeight * 1.5,
    paragraphs,
    took,
  };
}

function tableOf(table, box) {
  const rows = [...table.querySelectorAll("tr")];
  const first = rows.find((row) => [...row.children].every((cell) => cell.colSpan === 1)) || rows[0];
  const columns = first ? [...first.children].map((cell) => round(cell.getBoundingClientRect().width)) : [];
  return {
    kind: "table",
    ...box,
    columns,
    rows: rows.map((row) => ({
      h: round(row.getBoundingClientRect().height),
      cells: [...row.children].map((cell) => {
        const styles = getComputedStyle(cell);
        const fill = colour(styles.backgroundColor).hex || colour(getComputedStyle(row).backgroundColor).hex;
        const runs = paragraphOf(cell, new Set());
        return {
          header: cell.tagName === "TH",
          colspan: cell.colSpan || 1,
          rowspan: cell.rowSpan || 1,
          fill,
          line: colour(styles.borderBottomColor).hex,
          lineWidth: parseFloat(styles.borderBottomWidth) || 0,
          pad: [parseFloat(styles.paddingTop) || 0, parseFloat(styles.paddingRight) || 0, parseFloat(styles.paddingBottom) || 0, parseFloat(styles.paddingLeft) || 0],
          paragraph: { ...paragraphStyle(cell, 0, "none"), runs },
        };
      }),
    })),
  };
}
