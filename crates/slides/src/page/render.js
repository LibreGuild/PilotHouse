// The deck drawn for the app to take a picture or a PDF of: no controls,
// one slide at the top left of the page, filling its width. The app shows this page in a web view of its own, out of sight,
// so pictures and exports never disturb the page the user works in.
//
//   deckRender.show(file, token)   one slide, filling the view's width
//   deckRender.pages(token)        every shown slide, a printed page each
//   deckRender.measure(token)      where everything on every slide is
//   deckRender.check(token)        what does not fit or cannot be read
//
// Each tells the app when it is drawn (`rendered: token`), with where. A
// deck can have two of these pages (its window's, and one for a deck whose
// window is not open): each says which it is (`key`, from its address).

import { Look, mountSlide, parseDeck, scaleFrame, slideTitle } from "./core.js";
import { measureDeck } from "./measure.js";
import { inspect } from "./check.js";

const look = new Look();
const key = new URLSearchParams(location.search).get("render") || "";
const tell = (what) =>
  window.artifact.kind.call("/_artifact/slides/tell", { method: "POST", body: JSON.stringify({ ...what, key }) }).catch(() => {});

async function deck() {
  const answer = await fetch("/_artifact/slides/deck", { cache: "no-store" }).then((r) => r.json());
  look.set(answer.files["deck.css"]?.text || "");
  return { deck: parseDeck(answer.files["deck.json"]?.text || ""), files: answer.files, version: answer.version };
}

// Its fonts and pictures are there, and it was laid out and painted.
async function settled(root) {
  await document.fonts?.ready;
  const pictures = [...root.querySelectorAll("img")].map((image) =>
    image.complete ? null : new Promise((done) => ((image.onload = done), (image.onerror = done))),
  );
  await Promise.all(pictures);
  await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
}

function frame(html, size, scale) {
  const host = document.createElement("div");
  const box = document.createElement("div");
  box.style.cssText = "position:relative;overflow:hidden;flex:none";
  host.style.cssText = "position:absolute;left:0;top:0";
  box.append(host);
  document.body.append(box);
  const slide = mountSlide(host, html, size, look);
  scaleFrame(box, size, scale);
  return slide;
}

async function show(file, token) {
  try {
    const { deck: read, files } = await deck();
    document.body.replaceChildren();
    const scale = innerWidth / read.size[0];
    const slide = frame(files[file]?.text ?? null, read.size, scale);
    await settled(slide);
    tell({ rendered: token, rects: [[0, 0, read.size[0] * scale, read.size[1] * scale]] });
  } catch (error) {
    tell({ rendered: token, error: String(error.message || error) });
  }
}

// Every slide that is shown, each on a page of its own, for a system that
// makes a PDF by printing the page whole (Windows).
async function pages(token) {
  try {
    const { deck: read, files } = await deck();
    document.body.replaceChildren();
    const scale = innerWidth / read.size[0];
    const [width, height] = [read.size[0] * scale, read.size[1] * scale];
    const style = document.createElement("style");
    style.textContent = `@page { size: ${width}px ${height}px; margin: 0 } html, body { background: none !important } body > div { break-after: page; break-inside: avoid }`;
    document.body.append(style);
    const shown = read.slides.filter((slide) => !slide.skip);
    const drawn = shown.map((slide) => frame(files[slide.file]?.text ?? null, read.size, scale));
    await Promise.all(drawn.map(settled));
    tell({ rendered: token, rects: [[0, 0, width, height]], pages: shown.length });
  } catch (error) {
    tell({ rendered: token, error: String(error.message || error) });
  }
}

// Where everything on every slide is, for an export to PowerPoint.
async function measure(token) {
  try {
    const { deck: read, files } = await deck();
    document.body.replaceChildren();
    const measured = await measureDeck({ look, deck: read, textOf: (file) => files[file]?.text ?? null, title: window.DECK.title });
    tell({ rendered: token, measured });
  } catch (error) {
    tell({ rendered: token, error: String(error.message || error) });
  }
}

// What does not fit or cannot be read on each slide: the report the
// editor makes of the deck, made here for a deck no editor has open.
async function check(token) {
  try {
    const { deck: read, files, version } = await deck();
    document.body.replaceChildren();
    const scale = innerWidth / read.size[0];
    const slides = [];
    for (const [index, slide] of read.slides.entries()) {
      // One at a time: each is measured where a picture of it is taken.
      document.body.replaceChildren();
      const drawn = frame(files[slide.file]?.text ?? null, read.size, scale);
      await settled(drawn);
      slides.push({
        slide: index + 1,
        file: slide.file,
        title: slideTitle(drawn),
        skipped: Boolean(slide.skip),
        notes: Boolean(slide.notes),
        issues: inspect(drawn, read.size).map((issue) => issue.text),
      });
    }
    document.body.replaceChildren();
    tell({ rendered: token, report: { version, size: read.size, slides } });
  } catch (error) {
    tell({ rendered: token, error: String(error.message || error) });
  }
}

async function start() {
  document.documentElement.style.cssText = "background:#fff";
  document.body.style.cssText = "margin:0;display:flex;flex-direction:column;align-items:flex-start;background:#fff";
  document.getElementById("app")?.remove();
  look.setBase(await fetch("/_artifact/slides/app/slide.css").then((r) => r.text()));
  window.deckRender = { show, pages, measure, check };
  tell({ render: "ready" });
}

start().catch((error) => tell({ render: "failed", error: String(error.message || error) }));
