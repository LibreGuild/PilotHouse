// The deck as the editor holds it, and its talk with the app: reading the
// deck's files, writing changes as steps that can be undone, hearing what
// changed elsewhere (an agent wrote a slide), and telling the app what is
// selected.
//
// The files are the deck. What is here is a copy to draw from; every change
// is sent at once, and what the app says the files are is taken over this.

import { parseDeck, deckText, slideTitle } from "./core.js";

const CALLS = "/_artifact/slides/";
const call = (path, options) => window.artifact.kind.call(CALLS + path, options);

export const state = {
  // path -> { text }: the deck's files as this page has them.
  files: new Map(),
  version: 0,
  deck: parseDeck(""),
  // The slide on the stage, by its place in the deck.
  current: 0,
  canUndo: false,
  canRedo: false,
  kit: { looks: [], layouts: [] },
  comments: [],
  // file -> [{ kind, text }]: what does not fit, by slide.
  issues: new Map(),
};

// ---- A small bus ---------------------------------------------------------
// "deck" (files changed: { files, by }), "current", "comments", "issues",
// "selection", "saving" (true while a change is on its way).
const listeners = new Map();
export function on(event, listener) {
  if (!listeners.has(event)) listeners.set(event, new Set());
  listeners.get(event).add(listener);
  return () => listeners.get(event).delete(listener);
}
export function emit(event, data) {
  for (const listener of [...(listeners.get(event) || [])]) {
    try {
      listener(data);
    } catch (error) {
      console.error(error);
    }
  }
}

// ---- Reading -------------------------------------------------------------

// Whether deck.json was read at least once.
let loaded = false;
// path -> the app's mark of the file as it last told it: what a change is
// built on. Kept apart from the files, so a file removed here still has
// the mark the app knows it by until the app has removed it too.
const marks = new Map();

export const slideAt = (index) => state.deck.slides[index] || null;
export const currentSlide = () => slideAt(state.current);
export const textOf = (path) => state.files.get(path)?.text ?? null;

// Takes what the app says the deck is; answers with the paths that read
// differently than before.
function take(answer) {
  const changed = [];
  const seen = new Set();
  for (const [path, file] of Object.entries(answer.files)) {
    seen.add(path);
    const had = state.files.get(path);
    // Changed here and not sent yet: what is here stays, and only what the
    // app holds for it is noted (the change is built on that).
    marks.set(path, file.mark);
    if (pending.has(path)) continue;
    if (!had || had.text !== file.text) changed.push(path);
    state.files.set(path, { text: file.text });
  }
  for (const path of [...marks.keys()]) if (!seen.has(path)) marks.delete(path);
  for (const path of [...state.files.keys()]) {
    if (!seen.has(path) && !pending.has(path)) {
      state.files.delete(path);
      changed.push(path);
    }
  }
  state.version = answer.version;
  if ("canUndo" in answer) {
    state.canUndo = answer.canUndo;
    state.canRedo = answer.canRedo;
  }
  if (changed.includes("deck.json") || !loaded) {
    // The slide on the stage stays the one it was, wherever it moved to.
    const file = currentSlide()?.file;
    state.deck = parseDeck(textOf("deck.json") || "");
    loaded = true;
    const at = state.deck.slides.findIndex((slide) => slide.file === file);
    state.current = at >= 0 ? at : Math.min(state.current, Math.max(0, state.deck.slides.length - 1));
  }
  return changed;
}

export async function load() {
  const [deck, kit] = await Promise.all([call("deck").then((r) => r.json()), call("kit").then((r) => r.json())]);
  state.kit = kit;
  take(deck);
  if (window.DECK.owner) refreshComments();
  emit("deck", { files: [...state.files.keys()], by: "load" });
}

// Everything that talks to the app about the deck's files goes one after
// another: a change is never sent while the answer to one before it, or a
// fresh reading, is on its way.
let chain = Promise.resolve();
const queued = (work) => {
  const next = chain.then(work, work);
  chain = next.catch(() => {});
  return next;
};

// Reads the deck again (something else changed it) and says what differs.
export function refresh(by = "outside") {
  return queued(async () => {
    const answer = await call("deck").then((r) => r.json());
    const changed = take(answer);
    if (changed.length) emit("deck", { files: changed, by });
  });
}

// ---- Writing -------------------------------------------------------------

// path -> text (null: removed), waiting to be sent, and what the user did.
const pending = new Map();
let pendingLabel = "";
let flushing = false;

// Changes files: `changes` is [{ path, text }] (text null removes). Taken
// at once here, so the page shows it without waiting, and sent as one step.
// `label` says what the user did ("Moved the title on slide 3"), for the
// agent to be told; "" for what needs no telling.
export function write(changes, label = "") {
  const paths = [];
  for (const { path, text } of changes) {
    const had = state.files.get(path);
    if ((had?.text ?? null) === text) continue;
    pending.set(path, text);
    if (text === null) state.files.delete(path);
    else state.files.set(path, { text });
    paths.push(path);
  }
  if (paths.length === 0) return;
  if (label) pendingLabel = label;
  if (paths.includes("deck.json")) {
    const file = currentSlide()?.file;
    state.deck = parseDeck(textOf("deck.json") || "");
    const at = state.deck.slides.findIndex((slide) => slide.file === file);
    if (at >= 0) state.current = at;
    else state.current = Math.min(state.current, Math.max(0, state.deck.slides.length - 1));
  }
  emit("deck", { files: paths, by: "self" });
  flush();
}

function flush() {
  if (flushing || pending.size === 0) return;
  flushing = true;
  emit("saving", true);
  queued(async () => {
    try {
      while (pending.size) {
        const sent = new Map(pending);
        const label = pendingLabel;
        pending.clear();
        pendingLabel = "";
        const body = (base) =>
          JSON.stringify({
            changes: [...sent].map(([path, text]) => ({ path, text })),
            base,
            label,
          });
        // What the app holds for each file must be what this page built on.
        const base = {};
        for (const path of sent.keys()) base[path] = marks.get(path) ?? "";
        let response = await call("change", { method: "POST", body: body(base) });
        if (response.status === 409) {
          // An agent wrote one of them meanwhile. The user's hand wins: it
          // is the later of the two, and the agent reads a file again
          // before it changes it.
          const fresh = await call("deck").then((r) => r.json());
          const rebased = {};
          for (const path of sent.keys()) rebased[path] = fresh.files[path]?.mark ?? "";
          response = await call("change", { method: "POST", body: body(rebased) });
        }
        if (!response.ok) {
          const why = await response.json().catch(() => ({}));
          throw new Error(why.error || `The app answered ${response.status}.`);
        }
        const answer = await response.json();
        state.version = answer.version;
        state.canUndo = answer.canUndo;
        state.canRedo = answer.canRedo;
        for (const [path, mark] of Object.entries(answer.marks || {})) {
          if (mark) marks.set(path, mark);
          else marks.delete(path);
        }
      }
      emit("saved");
    } catch (error) {
      emit("failed", error);
      // What the files are is read again, so the page shows the truth.
      const answer = await call("deck").then((r) => r.json()).catch(() => null);
      if (answer) {
        const changed = take(answer);
        if (changed.length) emit("deck", { files: changed, by: "outside" });
      }
    } finally {
      flushing = false;
      emit("saving", false);
    }
  });
}

async function step(which) {
  await queued(async () => {
    const answer = await call(which, { method: "POST" }).then((r) => r.json());
    const changed = take(answer);
    if (changed.length) emit("deck", { files: changed, by: which });
    else emit("history");
  });
}
export const undo = () => step("undo");
export const redo = () => step("redo");

// ---- The deck's own changes ---------------------------------------------

const writeDeck = (label, more = []) => write([{ path: "deck.json", text: deckText(state.deck) }, ...more], label);

export function setDeck(change, label) {
  change(state.deck);
  writeDeck(label);
}

export function setSlide(index, change, label) {
  const slide = slideAt(index);
  if (!slide) return;
  change(slide);
  writeDeck(label);
}

// A name for a new slide's file that no slide has.
function freeFile(wanted) {
  let stem = String(wanted || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
  if (!stem) stem = "slide";
  const taken = (name) => state.files.has(name) || state.deck.slides.some((slide) => slide.file === name);
  let name = `slides/${stem}.html`;
  for (let n = 2; taken(name); n += 1) name = `slides/${stem}-${n}.html`;
  return name;
}

// Adds a slide with `html` after slide `after` (-1: first), and shows it.
export function addSlide(html, after = state.current, name = "") {
  const title = (() => {
    const probe = document.createElement("template");
    probe.innerHTML = html;
    return slideTitle(probe.content);
  })();
  const file = freeFile(name || title);
  const at = Math.min(state.deck.slides.length, after + 1);
  state.deck.slides.splice(at, 0, { file });
  state.current = at;
  writeDeck(`Added slide ${at + 1}`, [{ path: file, text: html }]);
  emit("current");
  return file;
}

export function duplicateSlide(index) {
  const slide = slideAt(index);
  if (!slide) return;
  const file = freeFile(slide.file.replace(/^slides\/|\.html$/g, "") + "-copy");
  const { file: _file, ...rest } = slide;
  state.deck.slides.splice(index + 1, 0, { ...rest, file });
  state.current = index + 1;
  writeDeck(`Duplicated slide ${index + 1}`, [{ path: file, text: textOf(slide.file) ?? "" }]);
  emit("current");
}

export function removeSlide(index) {
  const slide = slideAt(index);
  if (!slide) return;
  state.deck.slides.splice(index, 1);
  if (state.current >= state.deck.slides.length) state.current = Math.max(0, state.deck.slides.length - 1);
  // Its file goes with it unless another slide shows the same file.
  const shared = state.deck.slides.some((other) => other.file === slide.file);
  writeDeck(`Deleted slide ${index + 1}`, shared ? [] : [{ path: slide.file, text: null }]);
  emit("current");
}

export function moveSlide(from, to) {
  const count = state.deck.slides.length;
  if (from === to || from < 0 || from >= count || to < 0 || to >= count) return;
  const file = currentSlide()?.file;
  const [slide] = state.deck.slides.splice(from, 1);
  state.deck.slides.splice(to, 0, slide);
  state.current = Math.max(0, state.deck.slides.findIndex((one) => one.file === file));
  writeDeck(`Moved slide ${from + 1} to place ${to + 1}`);
  emit("current");
}

export function go(index) {
  const to = Math.max(0, Math.min(state.deck.slides.length - 1, index));
  if (to === state.current) return;
  state.current = to;
  emit("current");
}

// ---- The app -------------------------------------------------------------

// Tells the app something: what is selected, what does not fit, a wish.
export function tell(what) {
  return call("tell", { method: "POST", body: JSON.stringify(what) }).catch(() => {});
}

// Keeps a file (a picture, a video) with the deck; answers with its path
// there (`media/…`).
export async function upload(file) {
  const response = await call("media?name=" + encodeURIComponent(file.name || "file"), {
    method: "POST",
    body: file,
  });
  const answer = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(answer.error || "The file could not be kept.");
  return answer.path;
}

// Makes slides from a PowerPoint file, after the ones the deck has.
export async function startFrom(file) {
  const response = await call("import?name=" + encodeURIComponent(file.name || "deck.pptx"), {
    method: "POST",
    body: file,
  });
  const answer = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(answer.error || "The file could not be read.");
  await refresh();
}

export async function refreshComments() {
  try {
    state.comments = await call("comments").then((r) => r.json());
    emit("comments");
  } catch (error) {
    console.error(error);
  }
}

export async function comment(op) {
  const response = await call("comments", { method: "POST", body: JSON.stringify(op) });
  const answer = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(answer.error || "The comment could not be kept.");
  state.comments = answer;
  emit("comments");
}

// What the app says on the page's stream.
export function listen() {
  const kind = window.artifact.kind;
  kind.on("deck", (news) => {
    // This page's own changes come back as answers, not as news.
    if (news.version > state.version) refresh(news.by);
  });
  kind.on("comments", () => refreshComments());
  kind.on("chrome", ({ css }) => {
    const style = document.getElementById("chrome");
    if (style) style.textContent = `:root{${css}}`;
  });
  kind.on("command", (command) => emit("command", command));
}
