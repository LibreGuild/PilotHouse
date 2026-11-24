// The few controls the app's own pages are made of (a deck's, a board's):
// icons, buttons, menus, popovers, dialogs, a note at the foot. Drawn in the
// app's colours (the `--ui-*` variables the app puts into the page), so a
// page reads as part of the window it sits in. Served to every artifact of a
// kind at `/_artifact/kit/ui.js`, with `kit.css`.

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

// The icons are the app's own drawings, in one hand: a 24-point grid, round
// line ends, and a wash of the line's colour (`T`) in the part of each that
// is its body, so a row of them reads as shapes, not as hairlines.
const T = 'fill="currentColor" fill-opacity=".2"';
const INK = 'fill="currentColor" stroke="none"';
const ICONS = {
  // Things put on a slide or a board.
  text: `<path d="M5 7.500V5h14v2.500M12 5v14M9 19h6"/>`,
  image: `<rect x="3.500" y="4.500" width="17" height="15" rx="3" ${T}/><circle cx="8.800" cy="9.600" r="1.700" ${INK}/><path d="M3.800 16.300l4.400-4 3.500 3.100 3.100-2.600 5.400 4.700"/>`,
  video: `<rect x="3.500" y="5" width="17" height="14" rx="3" ${T}/><path d="M10 8.800v6.400l5.400-3.200z" ${INK}/>`,
  table: `<path d="M4 9.500V7a2.500 2.500 0 0 1 2.500-2.500h11A2.500 2.500 0 0 1 20 7v2.500z" ${T}/><rect x="4" y="4.500" width="16" height="15" rx="2.500"/><path d="M4 9.500h16M4 14.500h16M10 9.500v10"/>`,
  chart: `<path d="M4 19.500h16"/><rect x="5.500" y="11" width="3.400" height="6" rx="1" ${T}/><rect x="10.300" y="5.500" width="3.400" height="11.500" rx="1" ${T}/><rect x="15.100" y="8.500" width="3.400" height="8.500" rx="1" ${T}/>`,
  shapes: `<path d="M8 3.800l4.600 7.700H3.400z" ${T}/><rect x="12.500" y="12.500" width="8" height="8" rx="2" ${T}/><circle cx="7.500" cy="16.700" r="3.800"/>`,
  sticky: `<path d="M5 6.500A1.500 1.500 0 0 1 6.500 5h11A1.500 1.500 0 0 1 19 6.500V14l-5 5H6.500A1.500 1.500 0 0 1 5 17.500z" ${T}/><path d="M19 14h-3.500a1.500 1.500 0 0 0-1.500 1.500V19"/>`,
  frame: `<path d="M8 3.500v17M16 3.500v17M3.500 8h17M3.500 16h17"/><rect x="8" y="8" width="8" height="8" ${T} stroke="none"/>`,
  pen: `<path d="M14.500 5.500l4 4L9 19l-5 1 1-5z" ${T}/><path d="M12.500 7.500l4 4"/>`,
  marker: `<path d="M13 4.500l6.500 6.500-7 7H7.500l-2-2 .5-4.500z" ${T}/><path d="M4 20h5"/>`,
  eraser: `<path d="M9.500 19.500L4 14l9-9 6.500 6.500-8 8z" ${T}/><path d="M8.500 9.500l6.500 6.500M9.500 19.500H20"/>`,
  arrow: `<path d="M5 19L18.500 5.500M10.500 5.500h8v8"/>`,
  sticker: `<path d="M12 3.500a8.500 8.500 0 1 0 8.500 8.500c0-.4 0-.8-.1-1.200a5.300 5.300 0 0 1-7.200-7.200c-.4-.1-.8-.1-1.200-.1z" ${T}/><path d="M8.700 11h.01M13.500 11.200h.01" stroke-width="2.400"/><path d="M8.500 14.500c1.800 2 5 2 6.800 0"/>`,
  cursor: `<path d="M5.500 4.500l13 5.800-5.500 1.900-1.900 5.800z" ${T}/>`,
  hand: `<path d="M8.500 12.500v-6a1.500 1.500 0 0 1 3 0V11M11.500 10.500v-5a1.500 1.500 0 0 1 3 0v5.500M14.500 11V7a1.500 1.500 0 0 1 3 0v8a5.500 5.500 0 0 1-5.500 5.500h-.7a5 5 0 0 1-3.800-1.800l-3.300-4a1.500 1.500 0 0 1 2.300-2l2 1.800" ${T}/>`,
  // What is done with the whole.
  download: `<path d="M12 4v10.500M7.800 10.500l4.200 4.200 4.200-4.200"/><path d="M4.500 15v2.500A2.500 2.500 0 0 0 7 20h10a2.500 2.500 0 0 0 2.500-2.500V15" />`,
  present: `<rect x="3" y="4.500" width="18" height="12.500" rx="2.500" ${T}/><path d="M10.300 8.200v5.100l4.300-2.550z" ${INK}/><path d="M8.500 20.500h7M12 17v3.500"/>`,
  play: `<path d="M8 5.500v13l10.500-6.500z" ${T}/>`,
  open: `<path d="M14 5h5v5M19 5l-8 8M10 6.500H6.500A1.500 1.500 0 0 0 5 8v9.500A1.500 1.500 0 0 0 6.500 19H16a1.500 1.500 0 0 0 1.500-1.500V14"/>`,
  sliders: `<rect x="4" y="4" width="13" height="6" rx="1.800" ${T}/><path d="M17 7h2.500v5.200H12V15"/><rect x="10.500" y="15" width="3" height="5.500" rx="1"/>`,
  comment: `<path d="M5.500 5h13A2.500 2.500 0 0 1 21 7.500v7a2.500 2.500 0 0 1-2.500 2.500H11l-4.500 3.500V17h-1A2.500 2.500 0 0 1 3 14.500v-7A2.500 2.500 0 0 1 5.500 5z" ${T}/><path d="M8 11h.01M12 11h.01M16 11h.01" stroke-width="2.400"/>`,
  share: `<circle cx="6.500" cy="12" r="2.700" ${T}/><circle cx="17" cy="6" r="2.700" ${T}/><circle cx="17" cy="18" r="2.700" ${T}/><path d="M9 10.700l5.600-3.300M9 13.300l5.600 3.300"/>`,
  plus: `<path d="M12 5v14M5 12h14"/>`,
  minus: `<path d="M5 12h14"/>`,
  notes: `<path d="M6.500 4h11A2.500 2.500 0 0 1 20 6.500V14l-5 6H6.500A2.500 2.500 0 0 1 4 17.500v-11A2.500 2.500 0 0 1 6.500 4z" ${T}/><path d="M20 14h-3.500a1.500 1.500 0 0 0-1.500 1.500V20M8 9h8M8 12.500h5"/>`,
  single: `<rect x="4" y="6" width="16" height="12" rx="2.500" ${T}/>`,
  grid: `<rect x="4" y="4" width="7" height="7" rx="2" ${T}/><rect x="13" y="4" width="7" height="7" rx="2"/><rect x="4" y="13" width="7" height="7" rx="2"/><rect x="13" y="13" width="7" height="7" rx="2"/>`,
  rail: `<path d="M9 4.500H6.500a3 3 0 0 0-3 3v9a3 3 0 0 0 3 3H9z" ${T} stroke="none"/><rect x="3.500" y="4.500" width="17" height="15" rx="3"/><path d="M9 4.500v15"/>`,
  down: `<path d="M6.500 9.500l5.500 5.500 5.500-5.500"/>`,
  up: `<path d="M6.500 14.500L12 9l5.500 5.500"/>`,
  right: `<path d="M9.500 6.500L15 12l-5.500 5.500"/>`,
  left: `<path d="M14.500 6.500L9 12l5.500 5.500"/>`,
  close: `<path d="M6 6l12 12M18 6L6 18"/>`,
  undo: `<path d="M9 14l-4.500-4.500L9 5"/><path d="M4.500 9.500H14a5.500 5.500 0 0 1 0 11h-3"/>`,
  redo: `<path d="M15 14l4.500-4.500L15 5"/><path d="M19.500 9.500H10a5.500 5.500 0 0 0 0 11h3"/>`,
  // Text.
  bold: `<path d="M7 5h6a3.500 3.500 0 0 1 0 7H7zM7 12h7a3.500 3.500 0 0 1 0 7H7z"/>`,
  italic: `<path d="M10 5h8M6 19h8M14 5l-4 14"/>`,
  underline: `<path d="M7 4.500v6a5 5 0 0 0 10 0v-6M5.500 20h13"/>`,
  link: `<path d="M10.500 13.500a4.200 4.200 0 0 0 6 0l3-3a4.200 4.200 0 0 0-6-6L12.300 5.700"/><path d="M13.500 10.500a4.200 4.200 0 0 0-6 0l-3 3a4.200 4.200 0 0 0 6 6l1.200-1.200"/>`,
  list: `<path d="M9.500 6.500H20M9.500 12H20M9.500 17.500H20"/><circle cx="5" cy="6.500" r="1.400" ${INK}/><circle cx="5" cy="12" r="1.400" ${INK}/><circle cx="5" cy="17.500" r="1.400" ${INK}/>`,
  numbered: `<path d="M10 6.500h10M10 12h10M10 17.500h10M4 5l1.600-.8v4.300M4 15.300c.3-1.500 3-1.300 3 .2 0 1.300-3 1.800-3 3.500h3.200"/>`,
  alignLeft: `<path d="M4 6h16M4 10.500h10M4 15h16M4 19.500h10"/>`,
  alignCenter: `<path d="M4 6h16M7 10.500h10M4 15h16M7 19.500h10"/>`,
  alignRight: `<path d="M4 6h16M10 10.500h10M4 15h16M10 19.500h10"/>`,
  alignJustify: `<path d="M4 6h16M4 10.500h16M4 15h16M4 19.500h16"/>`,
  textSize: `<path d="M3.500 18l4.200-11h1.100L13 18M5 14.500h6.500M14 18l2.800-7h.9l2.800 7M15 16h4.500"/>`,
  // What is done with what is selected.
  more: `<circle cx="5.500" cy="12" r="1.600" ${INK}/><circle cx="12" cy="12" r="1.600" ${INK}/><circle cx="18.500" cy="12" r="1.600" ${INK}/>`,
  trash: `<path d="M6.500 7.500l.8 11a1.500 1.500 0 0 0 1.500 1.500h6.400a1.500 1.500 0 0 0 1.500-1.500l.8-11z" ${T}/><path d="M4.500 7.500h15M9.500 7.500V5h5v2.500M10 11.500v5M14 11.500v5"/>`,
  copy: `<rect x="8.500" y="8.500" width="11" height="11" rx="2.500" ${T}/><path d="M15.500 8.500v-2A2 2 0 0 0 13.500 4.500h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>`,
  check: `<path d="M5 12.500l4.500 4.500L19 7.500"/>`,
  crop: `<path d="M7 3v14h14M3 7h14v14"/>`,
  corner: `<path d="M5 19v-6a8 8 0 0 1 8-8h6"/>`,
  front: `<rect x="9" y="9" width="11" height="11" rx="2"/><rect x="4" y="4" width="11" height="11" rx="2" ${INK} fill-opacity=".85"/>`,
  back: `<rect x="4" y="4" width="11" height="11" rx="2"/><rect x="9" y="9" width="11" height="11" rx="2" ${INK} fill-opacity=".85"/>`,
  replace: `<path d="M4.500 9a8 8 0 0 1 14-2.500L20 8M20 4v4h-4M19.500 15a8 8 0 0 1-14 2.500L4 16M4 20v-4h4"/>`,
  hidden: `<path d="M4 4l16 16M10 6.200A9.600 9.600 0 0 1 12 6c5 0 8.500 6 8.500 6a14 14 0 0 1-2.600 3.200M14 14a2.800 2.800 0 0 1-4-4M6.500 8.300A14 14 0 0 0 3.500 12s3.500 6 8.500 6a9 9 0 0 0 2.700-.4"/>`,
  send: `<path d="M4 12l15.500-7-5 15.500-3-6.500z" ${T}/>`,
  warning: `<path d="M12 4.500l8.500 14.500h-17z" ${T}/><path d="M12 10v4.500M12 17.300v.2"/>`,
  fit: `<path d="M4 9V5.500A1.500 1.500 0 0 1 5.500 4H9M15 4h3.500A1.500 1.500 0 0 1 20 5.500V9M20 15v3.500a1.500 1.500 0 0 1-1.500 1.500H15M9 20H5.500A1.500 1.500 0 0 1 4 18.500V15"/><rect x="8.500" y="8.500" width="7" height="7" rx="1.500" ${T} stroke="none"/>`,
  lock: `<rect x="5" y="10.500" width="14" height="9.500" rx="2.500" ${T}/><path d="M8 10.500V8a4 4 0 0 1 8 0v2.500M12 14.500v2"/>`,
  unlock: `<rect x="5" y="10.500" width="14" height="9.500" rx="2.500" ${T}/><path d="M8 10.500V8a4 4 0 0 1 7.700-1.500M12 14.500v2"/>`,
  group: `<rect x="7" y="7" width="10" height="10" rx="1.500" ${T}/><path d="M4 7V5.500A1.500 1.500 0 0 1 5.500 4H7M17 4h1.500A1.500 1.500 0 0 1 20 5.500V7M20 17v1.500a1.500 1.500 0 0 1-1.500 1.500H17M7 20H5.500A1.500 1.500 0 0 1 4 18.500V17"/>`,
  alignL: `<path d="M4 4v16"/><rect x="7" y="6" width="12" height="4.500" rx="1.200" ${T}/><rect x="7" y="13.500" width="7.500" height="4.500" rx="1.200" ${T}/>`,
  alignC: `<path d="M12 4v16"/><rect x="5" y="6" width="14" height="4.500" rx="1.200" ${T}/><rect x="7.500" y="13.500" width="9" height="4.500" rx="1.200" ${T}/>`,
  alignR: `<path d="M20 4v16"/><rect x="5" y="6" width="12" height="4.500" rx="1.200" ${T}/><rect x="9.500" y="13.500" width="7.500" height="4.500" rx="1.200" ${T}/>`,
  alignT: `<path d="M4 4h16"/><rect x="6" y="7" width="4.500" height="12" rx="1.200" ${T}/><rect x="13.500" y="7" width="4.500" height="7.500" rx="1.200" ${T}/>`,
  alignM: `<path d="M4 12h16"/><rect x="6" y="5" width="4.500" height="14" rx="1.200" ${T}/><rect x="13.500" y="7.500" width="4.500" height="9" rx="1.200" ${T}/>`,
  alignB: `<path d="M4 20h16"/><rect x="6" y="5" width="4.500" height="12" rx="1.200" ${T}/><rect x="13.500" y="9.500" width="4.500" height="7.500" rx="1.200" ${T}/>`,
  spreadH: `<path d="M4 4v16M20 4v16"/><rect x="9.500" y="7" width="5" height="10" rx="1.200" ${T}/>`,
  spreadV: `<path d="M4 4h16M4 20h16"/><rect x="7" y="9.500" width="10" height="5" rx="1.200" ${T}/>`,
  tidy: `<rect x="4" y="4" width="6.500" height="6.500" rx="1.500" ${T}/><rect x="13.500" y="4" width="6.500" height="6.500" rx="1.500" ${T}/><rect x="4" y="13.500" width="6.500" height="6.500" rx="1.500" ${T}/><path d="M16.700 13.500v6.500M13.500 16.700H20"/>`,
  timer: `<circle cx="12" cy="13.500" r="7" ${T}/><path d="M12 13.500V10M9.500 3.500h5M18 7.500l1.500-1.500"/>`,
  map: `<rect x="3.500" y="5" width="17" height="14" rx="2.500"/><rect x="11.500" y="11" width="6.500" height="5.500" rx="1.200" ${T}/>`,
  template: `<rect x="4" y="4" width="16" height="6" rx="1.800" ${T}/><rect x="4" y="13" width="7" height="7" rx="1.800"/><rect x="14" y="13" width="6" height="7" rx="1.800"/>`,
  vote: `<circle cx="12" cy="12" r="8" ${T}/><circle cx="12" cy="12" r="3" ${INK}/>`,
  focus: `<circle cx="12" cy="12" r="3.200" ${T}/><path d="M12 3.500v3M12 17.500v3M3.500 12h3M17.500 12h3"/>`,
  solid: `<path d="M4 12h16"/>`,
  dashed: `<path d="M4 12h3.500M10.200 12h3.600M16.500 12H20"/>`,
  none: `<circle cx="12" cy="12" r="7.500"/><path d="M6.700 17.300L17.300 6.700"/>`,
  capNone: `<path d="M4 12h16"/>`,
  capArrow: `<path d="M4 12h15M14 7l5.500 5-5.500 5"/>`,
  capDot: `<path d="M4 12h11"/><circle cx="17.500" cy="12" r="2.600" ${T}/>`,
  routeStraight: `<path d="M5 18L19 6"/>`,
  routeElbow: `<path d="M5 18h7V6h7"/>`,
  routeCurve: `<path d="M5 18c7 0 7-12 14-12"/>`,
  // Shapes, as they are listed.
  rect: `<rect x="4" y="6" width="16" height="12" ${T}/>`,
  rounded: `<rect x="4" y="6" width="16" height="12" rx="4" ${T}/>`,
  ellipse: `<ellipse cx="12" cy="12" rx="8.500" ry="6.500" ${T}/>`,
  pill: `<rect x="3.500" y="7" width="17" height="10" rx="5" ${T}/>`,
  diamond: `<path d="M12 3.500l8.500 8.500-8.500 8.500L3.500 12z" ${T}/>`,
  triangle: `<path d="M12 4.500l8.500 14.500h-17z" ${T}/>`,
  hexagon: `<path d="M8 5h8l4.500 7-4.500 7H8l-4.500-7z" ${T}/>`,
  slant: `<path d="M8 6h12.500L16 18H3.500z" ${T}/>`,
  cylinder: `<path d="M5 7v10c0 1.700 3.100 3 7 3s7-1.300 7-3V7" ${T}/><ellipse cx="12" cy="7" rx="7" ry="3"/>`,
  document: `<path d="M4.500 5h15v12.500c-3.500-2.500-5.500 2.500-9 1s-3.500-1.500-6 0z" ${T}/>`,
  callout: `<path d="M5 5h14a1.500 1.500 0 0 1 1.500 1.500v8A1.500 1.500 0 0 1 19 16h-7l-4 4v-4H5a1.500 1.500 0 0 1-1.500-1.500v-8A1.500 1.500 0 0 1 5 5z" ${T}/>`,
  cloud: `<path d="M7.500 18.500a4 4 0 0 1-.6-8 5.500 5.500 0 0 1 10.500-1.300 4.700 4.700 0 0 1-.4 9.300z" ${T}/>`,
  person: `<circle cx="12" cy="8" r="3.500" ${T}/><path d="M5 20a7 7 0 0 1 14 0z" ${T}/>`,
  star: `<path d="M12 3.800l2.500 5.200 5.700.8-4.100 4 1 5.700-5.100-2.700-5.100 2.700 1-5.700-4.100-4 5.700-.8z" ${T}/>`,
  arrowRight: `<path d="M4 9.500h9V6l7 6-7 6v-3.500H4z" ${T}/>`,
  arrowLeft: `<path d="M20 9.500h-9V6l-7 6 7 6v-3.500h9z" ${T}/>`,
  arrowUp: `<path d="M9.500 20v-9H6l6-7 6 7h-3.500v9z" ${T}/>`,
  arrowDown: `<path d="M9.500 4v9H6l6 7 6-7h-3.500V4z" ${T}/>`,
  line: `<path d="M5 19L19 5"/>`,
  connector: `<path d="M5 19L19 5M12 5h7v7"/>`,
  palette: `<path d="M12 4a8 8 0 1 0 0 16c1.300 0 1.800-1 1.300-2-.6-1.300.3-2.500 1.700-2.500H17a3 3 0 0 0 3-3A8 8 0 0 0 12 4z" ${T}/><path d="M8 11h.01M11 7.800h.01M15 8.500h.01" stroke-width="2.600"/>`,
  file: `<path d="M7 3.500h7l4.500 4.500v11A1.500 1.500 0 0 1 17 20.500H7A1.500 1.500 0 0 1 5.500 19V5A1.500 1.500 0 0 1 7 3.500z" ${T}/><path d="M14 3.500V8h4.500"/>`,
  people: `<circle cx="9" cy="9" r="3.200" ${T}/><path d="M3.500 19a5.500 5.500 0 0 1 11 0M16 6.200a3.200 3.200 0 0 1 0 5.600M17.500 14a5.500 5.500 0 0 1 3 5"/>`,
  search: `<circle cx="11" cy="11" r="6" ${T}/><path d="M15.500 15.500L20 20"/>`,
};

export function icon(name, size = 18) {
  const span = document.createElement("span");
  span.className = "icon";
  span.innerHTML = `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name] || ""}</svg>`;
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
