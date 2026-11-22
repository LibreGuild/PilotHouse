// The few controls the deck's page is made of: buttons, menus, popovers,
// dialogs, a note at the foot. Drawn in the app's colours (the `--ui-*`
// variables the app puts into the page), so the page reads as part of the
// window it sits in.

// A shortcut as this system writes it: keys("⇧⌘Z") is "Ctrl+Shift+Z"
// off a Mac.
const MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export function keys(hint) {
  if (MAC) return hint;
  return hint.replace("⇧⌘", "Ctrl+Shift+").replace("⌘", "Ctrl+").replace("↩", "Enter");
}

// h("div", { class: "x", onclick: fn, dataset: { a: 1 } }, child, "text")
export function h(tag, props, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") element.className = value;
    else if (key === "style" && typeof value === "object") Object.assign(element.style, value);
    else if (key === "dataset") Object.assign(element.dataset, value);
    else if (key.startsWith("on") && typeof value === "function") element.addEventListener(key.slice(2), value);
    else if (key in element && key !== "list" && typeof value !== "string") element[key] = value;
    else element.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    element.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return element;
}

const ICONS = {
  text: '<path d="M5 6h14M12 6v13M9 19h6"/>',
  image:
    '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><circle cx="9" cy="10" r="1.6"/><path d="M4 17l5-4.5 4 3.5 3-2.5 4.5 4"/>',
  video: '<rect x="3" y="6" width="12.5" height="12" rx="2.5"/><path d="M15.5 10.5l5-3v9l-5-3z"/>',
  table: '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M3.5 10h17M3.5 14.5h17M9.5 4.5v15"/>',
  shapes: '<rect x="3.5" y="10.5" width="9" height="9" rx="1.5"/><circle cx="15" cy="9" r="5.5"/>',
  chart: '<path d="M4.5 4v15.5H20"/><path d="M8.5 16v-5M12.5 16V8M16.5 16v-7"/>',
  download: '<path d="M12 4v11M7.5 11l4.5 4.5 4.5-4.5M5 19.5h14"/>',
  play: '<path d="M8 5.5v13l10.5-6.5z"/>',
  open: '<path d="M14 5h5v5M19 5l-8 8M10 6.500H6.500A1.500 1.500 0 0 0 5 8v9.500A1.500 1.500 0 0 0 6.500 19H16a1.500 1.500 0 0 0 1.500-1.500V14"/>',
  sliders:
    '<path d="M4 7h8M18 7h2M4 17h2M12 17h8"/><circle cx="15" cy="7" r="2.4"/><circle cx="9" cy="17" r="2.4"/>',
  comment: '<path d="M20 12a8 8 0 0 1-11.6 7.1L4 20l1-4.2A8 8 0 1 1 20 12z"/>',
  share: '<path d="M12 15V4M8 7.5l4-4 4 4M5 12v6.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V12"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  minus: '<path d="M5 12h14"/>',
  notes: '<rect x="4" y="4" width="16" height="16" rx="2.5"/><path d="M8 9h8M8 12.5h8M8 16h4.5"/>',
  single: '<rect x="4" y="6" width="16" height="12" rx="2"/>',
  grid:
    '<rect x="4" y="4" width="7" height="7" rx="1.5"/><rect x="13" y="4" width="7" height="7" rx="1.5"/><rect x="4" y="13" width="7" height="7" rx="1.5"/><rect x="13" y="13" width="7" height="7" rx="1.5"/>',
  down: '<path d="M6.5 9.5l5.5 5.5 5.5-5.5"/>',
  up: '<path d="M6.5 14.5L12 9l5.5 5.5"/>',
  right: '<path d="M9.5 6.5L15 12l-5.5 5.5"/>',
  left: '<path d="M14.5 6.5L9 12l5.5 5.5"/>',
  close: '<path d="M6 6l12 12M18 6L6 18"/>',
  undo: '<path d="M8.5 5.5l-4 4 4 4"/><path d="M5 9.5h9.5a5 5 0 0 1 0 10H11"/>',
  redo: '<path d="M15.5 5.5l4 4-4 4"/><path d="M19 9.5H9.5a5 5 0 0 0 0 10H13"/>',
  bold: '<path d="M7 5h6a3.5 3.5 0 0 1 0 7H7zM7 12h7a3.5 3.5 0 0 1 0 7H7z"/>',
  italic: '<path d="M10 5h8M6 19h8M14 5l-4 14"/>',
  underline: '<path d="M7 4.5v6a5 5 0 0 0 10 0v-6M5.5 20h13"/>',
  link:
    '<path d="M10.5 13.5a4.2 4.2 0 0 0 6 0l3-3a4.2 4.2 0 0 0-6-6L12.3 5.7"/><path d="M13.5 10.5a4.2 4.2 0 0 0-6 0l-3 3a4.2 4.2 0 0 0 6 6l1.2-1.2"/>',
  list:
    '<path d="M9 6.5h11M9 12h11M9 17.5h11"/><path d="M4.5 6.5h.01M4.5 12h.01M4.5 17.5h.01" stroke-width="2.6"/>',
  numbered: '<path d="M10 6.5h10M10 12h10M10 17.5h10M4 5l1.6-.8v4.3M4 15.3c.3-1.5 3-1.3 3 .2 0 1.3-3 1.8-3 3.5h3.2"/>',
  alignLeft: '<path d="M4 6h16M4 10.5h10M4 15h16M4 19.5h10"/>',
  alignCenter: '<path d="M4 6h16M7 10.5h10M4 15h16M7 19.5h10"/>',
  alignRight: '<path d="M4 6h16M10 10.5h10M4 15h16M10 19.5h10"/>',
  alignJustify: '<path d="M4 6h16M4 10.5h16M4 15h16M4 19.5h16"/>',
  more: '<path d="M5.5 12h.01M12 12h.01M18.5 12h.01" stroke-width="2.8"/>',
  trash: '<path d="M4.5 7h15M9.5 7V4.5h5V7M6.5 7l.8 12a1.5 1.5 0 0 0 1.5 1.4h6.4a1.5 1.5 0 0 0 1.5-1.4l.8-12M10 11v5.5M14 11v5.5"/>',
  copy: '<rect x="8.5" y="8.5" width="11" height="11" rx="2"/><path d="M15.5 8.5v-2a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  crop: '<path d="M7 3v14h14M3 7h14v14"/>',
  corner: '<path d="M5 19v-6a8 8 0 0 1 8-8h6"/>',
  front: '<rect x="4" y="4" width="11" height="11" rx="2" fill="currentColor" fill-opacity=".25"/><rect x="9" y="9" width="11" height="11" rx="2"/>',
  back: '<rect x="9" y="9" width="11" height="11" rx="2" fill="currentColor" fill-opacity=".25"/><rect x="4" y="4" width="11" height="11" rx="2"/>',
  replace: '<path d="M4.5 9a8 8 0 0 1 14-2.5L20 8M20 4v4h-4M19.5 15a8 8 0 0 1-14 2.5L4 16M4 20v-4h4"/>',
  hidden: '<path d="M4 4l16 16M10 6.2A9.6 9.6 0 0 1 12 6c5 0 8.5 6 8.5 6a14 14 0 0 1-2.6 3.2M14 14a2.8 2.8 0 0 1-4-4M6.5 8.3A14 14 0 0 0 3.5 12s3.500 6 8.500 6a9 9 0 0 0 2.700-.4"/>',
  send: '<path d="M4 12l15.500-7-5 15.500-3-6.500z"/>',
  warning: '<path d="M12 4.5l8.500 14.500h-17zM12 10v4.500M12 17.300v.2"/>',
  fit: '<path d="M4 9V5.500A1.500 1.500 0 0 1 5.500 4H9M15 4h3.500A1.500 1.500 0 0 1 20 5.500V9M20 15v3.500a1.500 1.500 0 0 1-1.500 1.500H15M9 20H5.500A1.500 1.500 0 0 1 4 18.500V15"/>',
  rect: '<rect x="4" y="6" width="16" height="12"/>',
  rounded: '<rect x="4" y="6" width="16" height="12" rx="4"/>',
  ellipse: '<ellipse cx="12" cy="12" rx="8.500" ry="6.500"/>',
  diamond: '<path d="M12 3.500l8.500 8.500-8.500 8.500L3.500 12z"/>',
  arrowRight: '<path d="M4 9.500h9V6l7 6-7 6v-3.500H4z"/>',
  arrowLeft: '<path d="M20 9.500h-9V6l-7 6 7 6v-3.500h9z"/>',
  arrowUp: '<path d="M9.500 20v-9H6l6-7 6 7h-3.500v9z"/>',
  arrowDown: '<path d="M9.500 4v9H6l6 7 6-7h-3.500V4z"/>',
  line: '<path d="M5 19L19 5"/>',
  connector: '<path d="M5 19L19 5M12 5h7v7"/>',
  palette: '<path d="M12 4a8 8 0 1 0 0 16c1.300 0 1.800-1 1.300-2-.6-1.300.3-2.500 1.700-2.500H17a3 3 0 0 0 3-3A8 8 0 0 0 12 4z"/><path d="M8 11h.01M11 7.800h.01M15 8.500h.01" stroke-width="2.600"/>',
  file: '<path d="M7 3.500h7l4.500 4.500v11A1.500 1.500 0 0 1 17 20.500H7A1.500 1.500 0 0 1 5.500 19V5A1.500 1.500 0 0 1 7 3.500z"/><path d="M14 3.500V8h4.500"/>',
  people: '<circle cx="9" cy="9" r="3.200"/><path d="M3.500 19a5.500 5.500 0 0 1 11 0M16 6.200a3.200 3.200 0 0 1 0 5.600M17.500 14a5.500 5.500 0 0 1 3 5"/>',
};

export function icon(name, size = 18) {
  const span = document.createElement("span");
  span.className = "icon";
  span.innerHTML = `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] || ""}</svg>`;
  return span;
}

// A button: an icon, a label, or both. `title` is what it does, said to a
// pointer resting on it and to a screen reader when it has no label.
export function button({ icon: name, label, title, onClick, pressed, kind, disabled, caret, id, size }) {
  const element = h(
    "button",
    {
      type: "button",
      class: "btn" + (kind ? " btn-" + kind : "") + (label ? "" : " btn-icon"),
      title,
      "aria-label": label ? null : title,
      "aria-pressed": pressed === undefined ? null : String(Boolean(pressed)),
      disabled,
      id,
    },
    name ? icon(name, size) : null,
    label ? h("span", { class: "btn-label" }, label) : null,
    caret ? icon("down", 12) : null,
  );
  if (onClick) element.addEventListener("click", (event) => onClick(event, element));
  return element;
}

let layer;
// Where menus, popovers and dialogs go: above everything of the page.
function overlay() {
  if (!layer) {
    layer = h("div", { class: "layer" });
    document.body.append(layer);
  }
  return layer;
}

const openFloats = [];

// Closes every menu and popover that is open.
export function closeFloats() {
  while (openFloats.length) openFloats.pop().close();
}

// Puts `content` beside `anchor` (an element, or { x, y, width, height } in
// the page) and keeps it inside the window. It closes on a click elsewhere
// or Escape. Answers with { element, close }.
export function popover(anchor, content, { place = "below", className = "", keep = false, onClose } = {}) {
  if (!keep) closeFloats();
  const element = h("div", { class: "popover " + className, role: "dialog" }, content);
  overlay().append(element);
  const position = () => {
    const box = anchor instanceof Element ? anchor.getBoundingClientRect() : anchor;
    const mine = element.getBoundingClientRect();
    const gap = 6;
    let left = box.x;
    let top = box.y + box.height + gap;
    if (place === "above") top = box.y - mine.height - gap;
    if (place === "right") {
      left = box.x + box.width + gap;
      top = box.y;
    }
    if (place === "end") left = box.x + box.width - mine.width;
    if (top + mine.height > innerHeight - 8) top = Math.max(8, box.y - mine.height - gap);
    if (top < 8) top = 8;
    left = Math.max(8, Math.min(left, innerWidth - mine.width - 8));
    element.style.left = left + "px";
    element.style.top = top + "px";
  };
  position();
  const float = {
    element,
    position,
    close() {
      if (!element.isConnected) return;
      element.remove();
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
      const at = openFloats.indexOf(float);
      if (at >= 0) openFloats.splice(at, 1);
      onClose?.();
    },
  };
  const outside = (event) => {
    if (element.contains(event.target)) return;
    // A popover opened from this one (a colour picker in a menu) keeps it.
    const later = openFloats.slice(openFloats.indexOf(float) + 1);
    if (later.some((other) => other.element.contains(event.target))) return;
    float.close();
  };
  const escape = (event) => {
    if (event.key === "Escape" && openFloats[openFloats.length - 1] === float) {
      event.preventDefault();
      event.stopPropagation();
      float.close();
    }
  };
  // After this click, which may be the one that opened it.
  setTimeout(() => {
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape, true);
  });
  openFloats.push(float);
  return float;
}

// A menu at `anchor`. Items: { label, hint, icon, onSelect, danger,
// disabled, checked } or { separator: true } or { heading }.
export function menu(anchor, items, options = {}) {
  const list = h("div", { class: "menu", role: "menu" });
  const float = popover(anchor, list, { className: "popover-menu", ...options });
  for (const item of items) {
    if (!item) continue;
    if (item.separator) {
      list.append(h("div", { class: "menu-separator" }));
      continue;
    }
    if (item.heading) {
      list.append(h("div", { class: "menu-heading" }, item.heading));
      continue;
    }
    const row = h(
      "button",
      {
        type: "button",
        class: "menu-item" + (item.danger ? " danger" : ""),
        role: "menuitem",
        disabled: item.disabled,
      },
      item.icon ? icon(item.icon, 16) : item.checked !== undefined ? h("span", { class: "menu-check" }, item.checked ? icon("check", 14) : null) : null,
      h("span", { class: "menu-label" }, item.label),
      item.hint ? h("span", { class: "menu-hint" }, item.hint) : null,
    );
    row.addEventListener("click", () => {
      float.close();
      item.onSelect?.();
    });
    list.append(row);
  }
  float.position();
  // Arrow keys walk the menu.
  list.addEventListener("keydown", (event) => {
    const rows = [...list.querySelectorAll(".menu-item:not(:disabled)")];
    const at = rows.indexOf(document.activeElement);
    if (event.key === "ArrowDown") rows[(at + 1) % rows.length]?.focus();
    else if (event.key === "ArrowUp") rows[(at - 1 + rows.length) % rows.length]?.focus();
    else return;
    event.preventDefault();
  });
  return float;
}

// A dialog in the middle of the page, over a dimmed page. `content` is its
// body; `actions` its buttons at the foot. Answers with { element, close }.
export function dialog({ title, content, actions = [], width = 520, className = "", onClose }) {
  closeFloats();
  const close = () => {
    if (!scrim.isConnected) return;
    scrim.remove();
    document.removeEventListener("keydown", escape, true);
    onClose?.();
  };
  const box = h(
    "div",
    { class: "dialog " + className, role: "dialog", "aria-modal": "true", "aria-label": title, style: { width: width + "px" } },
    h(
      "div",
      { class: "dialog-head" },
      h("div", { class: "dialog-title" }, title),
      button({ icon: "close", title: "Close", onClick: close }),
    ),
    h("div", { class: "dialog-body" }, content),
    actions.length ? h("div", { class: "dialog-foot" }, actions) : null,
  );
  const scrim = h("div", { class: "scrim" }, box);
  scrim.addEventListener("pointerdown", (event) => {
    if (event.target === scrim) close();
  });
  const escape = (event) => {
    if (event.key === "Escape" && openFloats.length === 0) {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  };
  document.addEventListener("keydown", escape, true);
  overlay().append(scrim);
  (box.querySelector("[autofocus], input, textarea, button.btn-primary") || box).focus?.();
  return { element: box, close };
}

let toasts;
// A line at the foot of the page that goes by itself. `action` is a button
// in it: { label, onClick }.
export function toast(text, { action, kind = "", stay = 4200 } = {}) {
  if (!toasts) {
    toasts = h("div", { class: "toasts", role: "status", "aria-live": "polite" });
    overlay().append(toasts);
  }
  const note = h("div", { class: "toast " + kind }, h("span", {}, text));
  if (action) {
    note.append(
      h("button", { type: "button", class: "toast-action", onclick: () => (action.onClick(), note.remove()) }, action.label),
    );
  }
  toasts.append(note);
  setTimeout(() => {
    note.classList.add("leaving");
    setTimeout(() => note.remove(), 220);
  }, stay);
  return note;
}

// ---- Colours --------------------------------------------------------------

// "#rrggbb" (or with alpha) for any CSS colour, as this browser reads it.
export function hex(colour) {
  const probe = document.createElement("canvas").getContext("2d");
  probe.fillStyle = "#000";
  probe.fillStyle = colour;
  const read = probe.fillStyle;
  if (read.startsWith("#")) return read;
  const parts = read.match(/[\d.]+/g) || [0, 0, 0, 1];
  const two = (value) => Math.round(Number(value)).toString(16).padStart(2, "0");
  const alpha = parts[3] === undefined ? 1 : Number(parts[3]);
  return "#" + two(parts[0]) + two(parts[1]) + two(parts[2]) + (alpha < 1 ? two(alpha * 255) : "");
}

const STANDARD = [
  "#000000", "#434a54", "#8a93a0", "#cfd4dc", "#ffffff", "#e03131", "#f08c00", "#f2c200",
  "#2f9e44", "#1098ad", "#1c7ed6", "#4263eb", "#7048e8", "#c2255c", "#a8431f", "#5b4636",
];

// A colour picker at `anchor`: the deck's own colours first (`palette`:
// [{ name, value, variable }]), then standard ones, then any colour.
// `onPick(value)` gets a CSS colour: `var(--accent)` for one of the deck's,
// so it follows the deck's look when that changes.
export function colourPicker(anchor, { value, palette = [], onPick, allowNone = false }) {
  const swatch = (colour, title, picked) =>
    h("button", {
      type: "button",
      class: "swatch" + (picked ? " picked" : ""),
      title,
      "aria-label": title,
      style: { background: colour },
      onclick: () => {
        onPick(colour);
        float.close();
      },
    });
  const current = value ? hex(value) : "";
  const any = h("input", { type: "color", value: current.slice(0, 7) || "#000000", class: "colour-any", "aria-label": "Any colour" });
  any.addEventListener("input", () => onPick(any.value));
  const typed = h("input", { type: "text", class: "input colour-hex", value: current, spellcheck: false, "aria-label": "Colour as hex" });
  typed.addEventListener("change", () => {
    const text = typed.value.trim();
    if (/^#?[0-9a-f]{3,8}$/i.test(text)) onPick(text.startsWith("#") ? text : "#" + text);
  });
  const content = h(
    "div",
    { class: "colours" },
    palette.length ? h("div", { class: "colours-heading" }, "This deck") : null,
    palette.length
      ? h("div", { class: "swatches" }, palette.map((entry) => swatch(entry.variable ? `var(${entry.variable})` : entry.value, entry.name, false)))
      : null,
    h("div", { class: "colours-heading" }, "Standard"),
    h("div", { class: "swatches" }, STANDARD.map((colour) => swatch(colour, colour, colour === current))),
    h("div", { class: "colours-row" }, any, typed, allowNone ? button({ label: "None", onClick: () => (onPick(""), float.close()) }) : null),
  );
  // The swatches show the deck's colours as they are on the slide.
  for (const [index, entry] of palette.entries()) {
    content.querySelectorAll(".swatches")[0].children[index].style.background = entry.value;
  }
  const float = popover(anchor, content, { keep: true, className: "popover-colours" });
  return float;
}

// A number box that changes as it is typed in, and with the arrow keys.
export function numberField({ value, min = -Infinity, max = Infinity, step = 1, unit, onChange, label }) {
  const input = h("input", { type: "text", inputmode: "decimal", class: "input number", value: value ?? "", "aria-label": label });
  const read = () => {
    const number = parseFloat(input.value);
    return Number.isFinite(number) ? Math.max(min, Math.min(max, number)) : null;
  };
  input.addEventListener("change", () => {
    const number = read();
    if (number !== null) onChange(number);
  });
  input.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    const by = (event.shiftKey ? 10 : 1) * step * (event.key === "ArrowUp" ? 1 : -1);
    const number = Math.max(min, Math.min(max, +((read() ?? 0) + by).toFixed(3)));
    input.value = number;
    onChange(number);
  });
  return h("label", { class: "field-number" }, input, unit ? h("span", { class: "unit" }, unit) : null);
}

// A drop-down of choices: [{ value, label }].
export function select({ value, options, onChange, label }) {
  const element = h(
    "select",
    { class: "input select", "aria-label": label },
    options.map((option) => h("option", { value: option.value, selected: option.value === value }, option.label)),
  );
  element.addEventListener("change", () => onChange(element.value));
  return element;
}
