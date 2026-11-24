// The panel at the side: how the selected thing looks (its text, its box,
// its fill), or, with nothing selected, the slide and the deck. Every
// field changes the element's own style and saves at once.

import { h, button, select, numberField, colourPicker, dialog } from "/_artifact/kit/ui.js";
import * as store from "./store.js";
import { state } from "./store.js";
import { kindOf, textStyleOf, TEXT_STYLES, palette } from "./edit.js";

const FONTS = [
  ["", "As the deck sets it"],
  ["var(--font-head)", "The deck's headings"],
  ["var(--font-body)", "The deck's text"],
  ["var(--font-code)", "The deck's code"],
  ["Inter, sans-serif", "Inter"],
  ['"Helvetica Neue", Arial, sans-serif', "Helvetica"],
  ["Georgia, serif", "Georgia"],
  ['"Iowan Old Style", Palatino, serif', "Palatino"],
  ['"Times New Roman", Times, serif', "Times"],
  ["Verdana, sans-serif", "Verdana"],
  ['"JetBrains Mono", monospace', "JetBrains Mono"],
  ['"Courier New", monospace', "Courier"],
];
const WEIGHTS = [
  ["300", "Light"],
  ["400", "Regular"],
  ["500", "Medium"],
  ["600", "Semibold"],
  ["700", "Bold"],
  ["800", "Extra bold"],
];
const SIZES = [
  ["1920x1080", "Widescreen 16:9"],
  ["1440x1080", "Standard 4:3"],
  ["1920x1200", "Wide 16:10"],
  ["1754x1240", "A4 landscape"],
  ["1080x1080", "Square 1:1"],
  ["1080x1920", "Tall 9:16"],
];
const TRANSITIONS = [
  ["none", "None"],
  ["fade", "Fade"],
  ["slide", "Slide"],
];
const KINDS = [
  ["", "Plain"],
  ["title", "Title"],
  ["section", "Section"],
  ["quote", "Quote"],
  ["picture", "Picture beside text"],
  ["closing", "Closing"],
];

const row = (name, control) => h("div", { class: "row" }, h("span", { class: "name" }, name), control);
const section = (title, ...rows) => h("div", { class: "panel-section" }, h("div", { class: "panel-title" }, title), rows);
const number = (value) => Math.round(parseFloat(value) * 10) / 10 || 0;

export class Panel {
  constructor(env) {
    this.env = env;
  }

  get editing() {
    return this.env.editing;
  }

  draw() {
    const element = this.editing.selected;
    const root = this.env.el.panel;
    if (this.env.view.mode !== "single" || state.deck.slides.length === 0) {
      root.replaceChildren(this.deckSection());
      return;
    }
    root.replaceChildren(...(element ? this.elementSections(element) : [this.deckSection(), this.slideSection()]));
  }

  // A colour field: a chip and its value; a click picks another.
  colour(name, value, onPick, allowNone = true) {
    const shown = !value || value === "rgba(0, 0, 0, 0)" ? "" : value;
    const chip = h("span", { class: "chip" }, h("i", { style: { background: shown || "transparent" } }));
    const text = h("span", {}, shown ? shortColour(shown) : "None");
    const field = h("button", { type: "button", class: "colour-field", "aria-label": name }, chip, text);
    field.addEventListener("click", () =>
      colourPicker(field, {
        value: shown,
        palette: palette(this.env.stage.slide),
        allowNone,
        onPick: (colour) => {
          onPick(colour);
          chip.firstChild.style.background = colour || "transparent";
          text.textContent = colour ? shortColour(colour) : "None";
        },
      }),
    );
    return field;
  }

  elementSections(element) {
    const styles = getComputedStyle(element);
    const kind = kindOf(element);
    const change = (what, make) => this.editing.change(what, make);
    const set = (what, property) => (value) => change(what, (target) => target.style.setProperty(property, value));
    const out = [];
    if (kind === "text" || kind === "shape" || kind === "box") {
      const rows = [];
      if (kind === "text" && !["UL", "OL"].includes(element.tagName)) {
        rows.push(
          row(
            "Text style",
            select({
              label: "Text style",
              value: textStyleOf(element)?.id || "body",
              options: TEXT_STYLES.map((style) => ({ value: style.id, label: style.name })),
              onChange: (id) => this.editing.restyle(TEXT_STYLES.find((style) => style.id === id)),
            }),
          ),
        );
      }
      const family = element.style.fontFamily;
      const fonts = FONTS.some(([value]) => value === family) ? FONTS : [[family, family.split(",")[0].replace(/"/g, "")], ...FONTS];
      rows.push(
        row(
          "Font",
          select({
            label: "Font",
            value: family,
            options: fonts.map(([value, label]) => ({ value, label })),
            onChange: (value) =>
              change("Changed the font of", (target) => (value ? target.style.setProperty("font-family", value) : target.style.removeProperty("font-family"))),
          }),
        ),
        row(
          "Weight",
          select({
            label: "Weight",
            value: String(Math.round(parseInt(styles.fontWeight, 10) / 100) * 100),
            options: WEIGHTS.map(([value, label]) => ({ value, label })),
            onChange: set("Changed the weight of", "font-weight"),
          }),
        ),
        row(
          "Size",
          numberField({ label: "Size", value: number(styles.fontSize), min: 6, max: 600, unit: "px", onChange: (value) => set("Resized the text of", "font-size")(value + "px") }),
        ),
        row(
          "Spacing",
          h(
            "div",
            { class: "pair" },
            numberField({
              label: "Line height",
              value: styles.lineHeight === "normal" ? 1.2 : +(parseFloat(styles.lineHeight) / parseFloat(styles.fontSize)).toFixed(2),
              min: 0.6,
              max: 4,
              step: 0.05,
              onChange: (value) => set("Changed the line height of", "line-height")(String(value)),
            }),
            numberField({
              label: "Letter spacing",
              value: styles.letterSpacing === "normal" ? 0 : number(styles.letterSpacing),
              min: -20,
              max: 60,
              step: 0.5,
              unit: "px",
              onChange: (value) => set("Changed the letter spacing of", "letter-spacing")(value + "px"),
            }),
          ),
        ),
        row("Colour", this.colour("Text colour", styles.color, set("Recoloured", "color"), false)),
      );
      const aligned = styles.textAlign === "start" ? "left" : styles.textAlign;
      rows.push(
        row(
          "Align",
          h(
            "div",
            { class: "segmented" },
            [
              ["left", "alignLeft", "Left"],
              ["center", "alignCenter", "Centre"],
              ["right", "alignRight", "Right"],
              ["justify", "alignJustify", "Justified"],
            ].map(([value, name, title]) =>
              button({ icon: name, title, pressed: aligned === value, size: 15, onClick: () => (set("Aligned", "text-align")(value), this.draw()) }),
            ),
          ),
        ),
        row(
          "When full",
          select({
            label: "When there is too much text",
            value: element.getAttribute("data-fit") === "shrink" ? "shrink" : "keep",
            options: [
              { value: "keep", label: "Keep the size" },
              { value: "shrink", label: "Shrink text to fit" },
            ],
            onChange: (value) =>
              change("Changed the fitting of", (target) => {
                if (value === "shrink") {
                  target.setAttribute("data-fit", "shrink");
                  // Fitting needs a box to fit in: the one it has now.
                  if (!target.style.height) target.style.height = Math.round(target.offsetHeight) + "px";
                  target.style.overflow = "hidden";
                } else {
                  target.removeAttribute("data-fit");
                  target.style.removeProperty("font-size");
                  target.style.removeProperty("overflow");
                }
              }),
          }),
        ),
      );
      out.push(section("Text", rows));
    }
    out.push(
      section(
        "Layout",
        row(
          "Size",
          h(
            "div",
            { class: "pair" },
            numberField({ label: "Width", value: Math.round(element.getBoundingClientRect().width / this.env.view.scale), min: 4, max: 9999, unit: "w", onChange: (value) => set("Resized", "width")(value + "px") }),
            numberField({ label: "Height", value: Math.round(element.getBoundingClientRect().height / this.env.view.scale), min: 4, max: 9999, unit: "h", onChange: (value) => set("Resized", "height")(value + "px") }),
          ),
        ),
        row("Padding", numberField({ label: "Padding", value: number(styles.paddingTop), min: 0, max: 600, unit: "px", onChange: (value) => set("Padded", "padding")(value + "px") })),
        row("Margin", numberField({ label: "Margin", value: number(styles.marginBottom), min: -600, max: 600, unit: "px", onChange: (value) => set("Changed the margin of", "margin-bottom")(value + "px") })),
        row(
          "Turn",
          numberField({
            label: "Turned by",
            value: styles.rotate && styles.rotate !== "none" ? number(styles.rotate) : 0,
            min: -360,
            max: 360,
            unit: "°",
            onChange: (value) => change("Turned", (target) => (value ? (target.style.rotate = value + "deg") : target.style.removeProperty("rotate"))),
          }),
        ),
      ),
      section(
        "Appearance",
        row("Fill", this.colour("Fill", styles.backgroundColor, (colour) => set("Filled", "background")(colour || "transparent"))),
        row(
          "Opacity",
          numberField({ label: "Opacity", value: Math.round(parseFloat(styles.opacity) * 100), min: 0, max: 100, unit: "%", onChange: (value) => set("Changed the opacity of", "opacity")(String(value / 100)) }),
        ),
        row("Corners", numberField({ label: "Corner radius", value: number(styles.borderTopLeftRadius), min: 0, max: 999, unit: "px", onChange: (value) => set("Rounded", "border-radius")(value + "px") })),
        row(
          "Border",
          h(
            "div",
            { class: "pair" },
            numberField({
              label: "Border width",
              value: number(styles.borderTopWidth),
              min: 0,
              max: 80,
              unit: "px",
              onChange: (value) =>
                change("Changed the border of", (target) => {
                  target.style.borderWidth = value + "px";
                  target.style.borderStyle = value ? "solid" : "none";
                }),
            }),
            this.colour("Border colour", styles.borderTopColor, (colour) =>
              change("Changed the border of", (target) => {
                target.style.borderColor = colour || "transparent";
                if (!parseFloat(getComputedStyle(target).borderTopWidth)) {
                  target.style.borderWidth = "3px";
                  target.style.borderStyle = "solid";
                }
              }),
            ),
          ),
        ),
        row(
          "Shadow",
          select({
            label: "Shadow",
            value: styles.boxShadow === "none" ? "none" : element.style.boxShadow.includes("60px") ? "strong" : "soft",
            options: [
              { value: "none", label: "None" },
              { value: "soft", label: "Soft" },
              { value: "strong", label: "Strong" },
            ],
            onChange: (value) =>
              set("Changed the shadow of", "box-shadow")(
                value === "none" ? "none" : value === "soft" ? "0 8px 24px rgb(0 0 0 / 0.18)" : "0 24px 60px rgb(0 0 0 / 0.35)",
              ),
          }),
        ),
      ),
      h(
        "div",
        { class: "panel-section", style: { display: "flex", gap: "8px" } },
        button({ label: "Duplicate", icon: "copy", kind: "outline", onClick: () => this.editing.duplicate() }),
        button({ label: "Delete", icon: "trash", kind: "outline", onClick: () => this.editing.remove() }),
      ),
    );
    return out;
  }

  deckSection() {
    const css = store.textOf("deck.css") || "";
    const current = state.kit.looks.find((look) => css.includes(`look (${look.name})`))?.id || "";
    const size = state.deck.size.join("x");
    const sizes = SIZES.some(([value]) => value === size) ? SIZES : [[size, `${state.deck.size[0]} × ${state.deck.size[1]}`], ...SIZES];
    return section(
      "Presentation",
      row(
        "Size",
        select({
          label: "Slide size",
          value: size,
          options: sizes.map(([value, label]) => ({ value, label })),
          onChange: (value) => store.setDeck((deck) => (deck.size = value.split("x").map(Number)), "Changed the deck's size"),
        }),
      ),
      row(
        "Look",
        select({
          label: "Look",
          value: current,
          options: [...(current ? [] : [{ value: "", label: "The deck's own" }]), ...state.kit.looks.map((look) => ({ value: look.id, label: look.name }))],
          onChange: (id) => this.changeLook(id),
        }),
      ),
      row(
        "Transition",
        select({
          label: "Transition between slides",
          value: state.deck.transition || "none",
          options: TRANSITIONS.map(([value, label]) => ({ value, label })),
          onChange: (value) => store.setDeck((deck) => (deck.transition = value === "none" ? "" : value), "Changed the deck's transition"),
        }),
      ),
    );
  }

  // Another look replaces deck.css whole: said first when the deck's own
  // was changed since it was chosen.
  changeLook(id) {
    const look = state.kit.looks.find((one) => one.id === id);
    if (!look) return;
    const css = store.textOf("deck.css") || "";
    const known = state.kit.looks.some((one) => one.css === css);
    const apply = () => store.write([{ path: "deck.css", text: look.css }], `Changed the deck's look to ${look.name}`);
    if (known || !css.trim()) {
      apply();
      return;
    }
    const box = dialog({
      title: `Change the look to ${look.name}?`,
      width: 420,
      content: h("div", {}, "This deck's look was changed by hand or by the agent. Another look replaces all of it. Undo brings it back."),
      actions: [
        button({ label: "Cancel", onClick: () => (box.close(), this.draw()) }),
        button({ label: "Change the look", kind: "primary", onClick: () => (box.close(), apply()) }),
      ],
    });
  }

  slideSection() {
    const slide = store.currentSlide();
    const element = this.env.stage.slide;
    if (!slide || !element) return null;
    const index = state.current;
    const kind = KINDS.find(([value]) => value && element.classList.contains(value))?.[0] || "";
    const saveSlide = (what, make) => {
      make(element);
      this.env.commit(`${what} slide ${index + 1}`);
      this.env.drawStage();
    };
    return section(
      `Slide ${index + 1}`,
      row(
        "Kind",
        select({
          label: "Kind of slide",
          value: kind,
          options: KINDS.map(([value, label]) => ({ value, label })),
          onChange: (value) =>
            saveSlide("Changed the kind of", (target) => {
              target.classList.remove(...KINDS.map(([name]) => name).filter(Boolean));
              if (value) target.classList.add(value);
            }),
        }),
      ),
      row(
        "Colours",
        select({
          label: "Colours",
          value: element.classList.contains("dark") ? "dark" : "",
          options: [
            { value: "", label: "As the deck" },
            { value: "dark", label: "Turned around" },
          ],
          onChange: (value) => saveSlide("Recoloured", (target) => target.classList.toggle("dark", value === "dark")),
        }),
      ),
      row(
        "Background",
        this.colour("Background", element.style.background ? getComputedStyle(element).backgroundColor : "", (colour) =>
          saveSlide("Changed the background of", (target) => (colour ? (target.style.background = colour) : target.style.removeProperty("background"))),
        ),
      ),
      row(
        "Transition",
        select({
          label: "Transition into this slide",
          value: slide.transition || "",
          options: [{ value: "", label: "As the deck" }, ...TRANSITIONS.map(([value, label]) => ({ value, label }))],
          onChange: (value) => store.setSlide(index, (one) => (one.transition = value), `Changed the transition of slide ${index + 1}`),
        }),
      ),
      row(
        "Presenting",
        select({
          label: "When presenting",
          value: slide.skip ? "skip" : "show",
          options: [
            { value: "show", label: "Shown" },
            { value: "skip", label: "Skipped" },
          ],
          onChange: (value) => store.setSlide(index, (one) => (one.skip = value === "skip"), `${value === "skip" ? "Skipped" : "Showed"} slide ${index + 1}`),
        }),
      ),
    );
  }
}

// A colour as the panel writes it: the deck's name for it, or its hex.
function shortColour(colour) {
  const variable = /^var\((--[\w-]+)\)$/.exec(colour);
  if (variable) return variable[1].replace(/^--/, "");
  const parts = colour.match(/[\d.]+/g);
  if (colour.startsWith("rgb") && parts) {
    const two = (value) => Math.round(Number(value)).toString(16).padStart(2, "0");
    return ("#" + two(parts[0]) + two(parts[1]) + two(parts[2])).toUpperCase();
  }
  return colour;
}
