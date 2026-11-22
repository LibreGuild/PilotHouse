// The deck's page for someone who looks at it: a guest who has the shared
// link, the window the audience sees while the owner presents with notes
// (`?audience`), or the owner beside the chat of the workspace the deck was
// made from (`?preview`). The slide, the slides along the foot, and Present.
//
// It follows the deck as it changes, and follows the presenter: while the
// owner presents, the slide shown here is the one they are on. Beside a
// chat it follows the agent: the slide shown is the one written last, and
// **Open in Slides** opens the deck's own window, where it is changed.

import { Look, Show, mountSlide, fitFrame, scaleFrame, parseDeck } from "./core.js";
import { h, button, icon } from "./ui.js";

const audience = new URLSearchParams(location.search).has("audience");
const preview = Boolean(window.DECK.preview);
const look = new Look();
// `tracking`: beside a chat, the slide shown follows what is written.
const state = { deck: parseDeck(""), files: {}, at: 0, following: false, tracking: true };
const el = {};
let show = null;

const shown = () => state.deck.slides.filter((slide) => !slide.skip);
const textOf = (slide) => state.files[slide.file]?.text ?? null;
const forShow = () => shown().map((slide) => ({ html: textOf(slide), transition: slide.transition || "", file: slide.file }));

// `written`: the files an agent just wrote. Beside its chat, the slide
// shown is the last of them, unless the user went to another one since.
async function load(written = []) {
  const answer = await fetch("/_artifact/slides/deck", { cache: "no-store" }).then((r) => r.json());
  const file = shown()[state.at]?.file;
  state.files = answer.files;
  state.deck = parseDeck(answer.files["deck.json"]?.text || "");
  look.set(answer.files["deck.css"]?.text || "");
  const slides = shown();
  const again = slides.findIndex((slide) => slide.file === file);
  state.at = again >= 0 ? again : Math.min(state.at, Math.max(0, slides.length - 1));
  if (preview && state.tracking && !show) {
    const latest = slides.reduce((found, slide, index) => (written.includes(slide.file) ? index : found), -1);
    if (latest >= 0) state.at = latest;
  }
  draw();
  show?.update(forShow());
}

function draw() {
  const slides = shown();
  document.title = state.deck.title || window.DECK.title || "Slides";
  if (audience) return;
  el.empty.hidden = slides.length > 0;
  el.frame.hidden = slides.length === 0;
  el.nav.hidden = slides.length < 2;
  if (slides.length === 0) return;
  mountSlide(el.host, textOf(slides[state.at]), state.deck.size, look);
  fit();
  el.strip.replaceChildren(
    ...slides.map((slide, index) => {
      const host = h("div", { class: "host" });
      const box = h("div", { class: "thumb" }, host);
      mountSlide(host, textOf(slide), state.deck.size, look);
      scaleFrame(box, state.deck.size, 70 / state.deck.size[1]);
      const tile = h(
        "button",
        { type: "button", class: "slide-tile" + (index === state.at ? " current" : ""), "aria-label": `Slide ${index + 1}` },
        box,
        h("span", { class: "tile-number" }, String(index + 1)),
      );
      tile.addEventListener("click", () => go(index, true));
      return tile;
    }),
  );
  el.strip.querySelector(".current")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  el.count.textContent = `${state.at + 1} / ${slides.length}`;
  el.following.hidden = !state.following;
}

function fit() {
  if (el.frame.hidden) return;
  const box = el.stage.getBoundingClientRect();
  fitFrame(el.frame, state.deck.size, { width: box.width - 48, height: box.height - 48 });
}

// `own` is the viewer's own step: it stops following the presenter until
// the presenter moves again.
function go(index, own) {
  const slides = shown();
  state.at = Math.max(0, Math.min(slides.length - 1, index));
  if (own) {
    state.following = false;
    // Back on the last slide, the newest is followed again.
    state.tracking = state.at === slides.length - 1;
  }
  if (show) show.go(state.at, false);
  draw();
}

function present() {
  const slides = forShow();
  if (show || slides.length === 0) return;
  const stage = h("div", { class: "show-stage" });
  document.body.append(stage);
  show = new Show(stage, {
    size: state.deck.size,
    look,
    slides,
    at: state.at,
    transition: state.deck.transition || "none",
    onChange: (at) => {
      state.at = at;
      state.following = false;
      draw();
    },
    onExit: () => {
      if (audience) return;
      show.close();
      show = null;
      stage.remove();
      if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    },
  });
  if (!audience) document.documentElement.requestFullscreen?.().catch(() => {});
}

function openInSlides() {
  window.artifact.kind.call("/_artifact/slides/tell", { method: "POST", body: JSON.stringify({ open: true }) }).catch(console.error);
}

function build() {
  const app = document.getElementById("app");
  app.classList.add("viewer");
  if (audience) return;
  el.host = h("div", { class: "host" });
  el.frame = h("div", { class: "frame" }, el.host);
  el.empty = h(
    "div",
    { class: "empty" },
    h("h1", {}, "This deck has no slides yet"),
    preview ? h("p", {}, "They show here as the agent writes them.") : null,
  );
  el.stage = h("div", { class: "stage", style: { padding: "24px" } }, el.frame, el.empty);
  el.count = h("span", { class: "tray-count" });
  // Back, where it is, next: nothing to show of them in a deck of one.
  el.nav = h(
    "span",
    { class: "tray-nav" },
    button({ icon: "left", title: "Back", onClick: () => go(state.at - 1, true) }),
    el.count,
    button({ icon: "right", title: "Next", onClick: () => go(state.at + 1, true) }),
  );
  el.following = h("span", { class: "following", hidden: true }, icon("people", 14), "Following the presenter");
  el.strip = h("div", { class: "strip" });
  app.replaceChildren(
    h("div", { class: "main" }, el.stage),
    h(
      "div",
      { class: "tray" },
      h(
        "div",
        { class: "tray-bar" },
        el.nav,
        el.following,
        h("span", { class: "tray-space" }),
        // Beside a chat the deck is looked at; its own window is where it
        // is changed, presented and exported.
        preview
          ? button({ icon: "open", label: "Open in Slides", kind: "primary", title: "Open the deck in its own window: change it by hand, comment, present, export", onClick: openInSlides })
          : button({ icon: "play", label: "Present", title: "Show the slides full screen", onClick: present }),
      ),
      el.strip,
    ),
  );
  new ResizeObserver(fit).observe(el.stage);
  document.addEventListener("keydown", (event) => {
    if (show || event.metaKey || event.ctrlKey || event.altKey) return;
    if (["ArrowRight", "ArrowDown", "PageDown", " "].includes(event.key)) go(state.at + 1, true);
    else if (["ArrowLeft", "ArrowUp", "PageUp"].includes(event.key)) go(state.at - 1, true);
    else return;
    event.preventDefault();
  });
}

async function start() {
  build();
  look.setBase(await fetch("/_artifact/slides/app/slide.css").then((r) => r.text()));
  await load();
  const kind = window.artifact.kind;
  kind.on("deck", (told) => load(told?.by === "outside" ? told.files || [] : []).catch(console.error));
  kind.on("chrome", ({ css }) => {
    document.getElementById("chrome").textContent = `:root{${css}}`;
  });
  // The presenter (the deck's owner) says which slide they are on.
  window.artifact.room.on("message", ({ data }) => {
    const told = data?.deck;
    const host = window.artifact.room.people().some((person) => person.host);
    if (!told || !host) return;
    if (told.presenting === false) {
      state.following = false;
      draw();
      return;
    }
    const at = shown().findIndex((slide) => slide.file === told.at);
    if (at < 0) return;
    state.following = true;
    go(at, false);
  });
  if (audience) present();
}

start().catch((error) => {
  console.error(error);
  document.getElementById("app").textContent = "The deck could not be opened: " + error.message;
});
