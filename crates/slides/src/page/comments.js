// Comments on a slide or on one thing on it: a pin where it is about, a
// thread at the pin, and the choice to send it to the agent, which then
// changes the slide and answers in the thread.

import { h, button, popover, menu, icon, toast } from "/_artifact/kit/ui.js";
import * as store from "./store.js";
import { state } from "./store.js";

const toAgent = () => {
  try {
    return localStorage.getItem("deck.comment-agent") !== "off";
  } catch {
    return true;
  }
};
const setToAgent = (on) => {
  try {
    localStorage.setItem("deck.comment-agent", on ? "on" : "off");
  } catch {
    // Not kept.
  }
};

function when(seconds) {
  const ago = Math.max(0, Date.now() / 1000 - seconds);
  if (ago < 90) return "now";
  if (ago < 3600) return Math.round(ago / 60) + " min ago";
  if (ago < 86400) return Math.round(ago / 3600) + " h ago";
  return new Date(seconds * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

export class Comments {
  constructor(env) {
    this.env = env;
    this.mode = false;
    this.hovered = null;
    const frame = env.el.frame;
    // In comment mode a click says what the comment is about.
    frame.addEventListener(
      "pointerdown",
      (event) => {
        if (!this.mode || event.button !== 0) return;
        event.preventDefault();
        event.stopPropagation();
        this.compose(env.editing.blockAt(event));
      },
      true,
    );
    frame.addEventListener("pointermove", (event) => {
      if (!this.mode) return;
      const block = env.editing.blockAt(event);
      if (block !== this.hovered) {
        this.hovered = block;
        this.refresh();
      }
    });
  }

  setMode(on) {
    this.mode = on;
    this.hovered = null;
    this.env.el.frame.style.cursor = on ? "crosshair" : "";
    this.refresh();
  }

  onSlide() {
    const file = store.currentSlide()?.file;
    return state.comments.filter((comment) => comment.file === file);
  }

  elementOf(comment) {
    const slide = this.env.stage.slide;
    if (!slide || !comment.element) return null;
    return slide.querySelector(`[data-id="${CSS.escape(comment.element)}"]`);
  }

  // Where a comment's pin goes: the corner of what it is about.
  spotOf(comment, index) {
    const within = this.env.el.main.getBoundingClientRect();
    const about = this.elementOf(comment);
    const box = (about || this.env.el.frame).getBoundingClientRect();
    // Comments on the slide itself line up down its right edge.
    return about
      ? { x: box.x + box.width - within.x, y: box.y - within.y }
      : { x: box.x + box.width - within.x - 16, y: box.y - within.y + 18 + index * 30 };
  }

  refresh() {
    const marks = this.env.el.marks;
    for (const old of marks.querySelectorAll(".pin, .outline.comment")) old.remove();
    if (this.env.view.mode !== "single" || !this.env.stage.slide) return;
    if (this.mode && this.hovered?.isConnected) {
      const within = this.env.el.main.getBoundingClientRect();
      const box = this.hovered.getBoundingClientRect();
      marks.append(
        h("div", {
          class: "outline comment",
          style: { left: box.x - within.x + "px", top: box.y - within.y + "px", width: box.width + "px", height: box.height + "px" },
        }),
      );
    }
    let loose = 0;
    this.onSlide().forEach((comment, index) => {
      if (comment.resolved && !this.showResolved) return;
      const spot = this.spotOf(comment, this.elementOf(comment) ? 0 : loose++);
      const pin = h(
        "button",
        {
          type: "button",
          class: "pin" + (comment.resolved ? " resolved" : ""),
          style: { left: spot.x + "px", top: spot.y + "px" },
          title: comment.text,
          "aria-label": `Comment ${index + 1}: ${comment.text}`,
        },
        String(index + 1),
      );
      pin.addEventListener("pointerdown", (event) => event.stopPropagation());
      pin.addEventListener("click", () => this.open(comment.id, pin));
      marks.append(pin);
    });
  }

  // The box a comment is written in. `agent` is whether it goes to the
  // agent at once; the choice is kept for the next comment.
  writer(placeholder, onPost) {
    const text = h("textarea", { class: "input", rows: 3, placeholder, style: { width: "100%" } });
    const agent = h("input", { type: "checkbox", checked: toAgent() });
    const post = () => {
      const said = text.value.trim();
      if (!said) return;
      setToAgent(agent.checked);
      onPost(said, agent.checked);
    };
    text.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        post();
      }
      event.stopPropagation();
    });
    const element = h(
      "div",
      { style: { display: "flex", flexDirection: "column", gap: "8px" } },
      text,
      h(
        "div",
        { class: "thread-foot" },
        h("label", { class: "check" }, agent, "Send to the agent"),
        button({ label: "Post", kind: "primary", onClick: post }),
      ),
    );
    return { element, text };
  }

  // A new comment about `element` (or, with none, the slide).
  compose(element) {
    const slide = store.currentSlide();
    if (!slide) return;
    const editing = this.env.editing;
    const label = element ? editing.label(element) : "";
    const about = `Slide ${state.current + 1}${label ? " · " + label : ""}`;
    const anchor = element || this.env.el.frame;
    const { element: writer, text } = this.writer("What should change here?", async (said, agent) => {
      float.close();
      let id = "";
      if (element) {
        id = editing.idOf(element);
        // Its id is in the file from now on, so the comment finds it.
        this.env.commit("");
      }
      try {
        await store.comment({ op: "add", file: slide.file, element: id, label, text: said, agent });
      } catch (error) {
        toast(error.message, { kind: "error" });
      }
    });
    const float = popover(anchor, h("div", { class: "thread" }, h("div", { class: "thread-about" }, about), writer), {
      place: element ? "right" : "below",
    });
    text.focus();
  }

  // The thread of comment `id`, at its pin.
  open(id, anchor) {
    const comment = state.comments.find((one) => one.id === id);
    if (!comment) return;
    const item = (who, at, said) =>
      h(
        "div",
        { class: "thread-item" },
        h("div", { class: "thread-who" }, who, h("span", { class: "thread-when" }, when(at))),
        h("div", { class: "thread-text" }, said),
      );
    const act = (op) => async () => {
      float.close();
      try {
        await store.comment({ ...op, id });
      } catch (error) {
        toast(error.message, { kind: "error" });
      }
    };
    const { element: writer } = this.writer("Reply…", async (said, agent) => {
      float.close();
      try {
        await store.comment({ op: "reply", id, text: said, agent });
      } catch (error) {
        toast(error.message, { kind: "error" });
      }
    });
    const more = button({
      icon: "more",
      title: "More",
      onClick: (_, at) => menu(at, [{ label: "Delete comment", danger: true, icon: "trash", onSelect: act({ op: "delete" }) }], { keep: true }),
    });
    const content = h(
      "div",
      { class: "thread" },
      h(
        "div",
        { style: { display: "flex", alignItems: "center", gap: "4px" } },
        h("div", { class: "thread-about", style: { flex: "1" } }, comment.label || `Slide ${state.current + 1}`),
        button({
          icon: "check",
          title: comment.resolved ? "Open it again" : "Resolve",
          pressed: comment.resolved,
          onClick: act({ op: "resolve", resolved: !comment.resolved }),
        }),
        more,
      ),
      item(comment.author, comment.at, comment.text),
      (comment.replies || []).map((reply) => item(reply.author, reply.at, reply.text)),
      comment.resolved ? null : writer,
    );
    const float = popover(anchor, content, { place: "right" });
  }

  // Every comment of the deck, open ones first; a click goes to it.
  list(anchor) {
    const rows = [...state.comments].sort((a, b) => Number(a.resolved) - Number(b.resolved) || b.at - a.at);
    const content = h(
      "div",
      { style: { width: "320px" } },
      h(
        "div",
        { style: { display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: "6px" } },
        h("strong", {}, rows.length ? `${rows.filter((one) => !one.resolved).length} open` : "No comments yet"),
        h(
          "label",
          { class: "check" },
          (() => {
            const box = h("input", { type: "checkbox", checked: Boolean(this.showResolved) });
            box.addEventListener("change", () => {
              this.showResolved = box.checked;
              this.refresh();
            });
            return box;
          })(),
          "Show resolved on the slide",
        ),
      ),
      rows.length === 0
        ? h("div", { style: { color: "var(--ui-muted)" } }, "Press the comment button, then click what the comment is about.")
        : h(
            "div",
            { class: "comment-list" },
            rows.map((comment) => {
              const at = state.deck.slides.findIndex((slide) => slide.file === comment.file);
              const entry = h(
                "button",
                { type: "button", class: "comment-row" + (comment.resolved ? " resolved" : "") },
                h("span", { class: "thread-about" }, `${at >= 0 ? "Slide " + (at + 1) : "A slide that is gone"}${comment.label ? " · " + comment.label : ""}`),
                h("span", { class: "thread-text" }, comment.text),
                h(
                  "span",
                  { class: "thread-when" },
                  [comment.author, when(comment.at), comment.replies?.length ? `${comment.replies.length} repl${comment.replies.length === 1 ? "y" : "ies"}` : "", comment.resolved ? "resolved" : ""]
                    .filter(Boolean)
                    .join(" · "),
                ),
              );
              entry.addEventListener("click", () => {
                float.close();
                if (at < 0) return;
                store.go(at);
                // Once the slide is on the stage, its pin opens.
                requestAnimationFrame(() => {
                  if (comment.resolved) this.showResolved = true;
                  this.refresh();
                  const pins = [...this.env.el.marks.querySelectorAll(".pin")];
                  const pin = pins[this.onSlide().filter((one) => !one.resolved || this.showResolved).findIndex((one) => one.id === comment.id)];
                  if (pin) this.open(comment.id, pin);
                });
              });
              return entry;
            }),
          ),
    );
    const float = popover(anchor, content, { place: "end" });
  }
}

export { icon };
