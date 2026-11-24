// The board as the page holds it: what is on it, what is selected, how it
// is looked at; every change as a step that can be undone; and the board's
// file, which is the truth (read at the start and whenever something else
// wrote it, written a moment after each change).

const CALLS = "/_artifact/whiteboard/";
const call = (path, options) => window.artifact.kind.call(CALLS + path, options);

export const state = {
  // The board's file as an object: { version, title, elements: [] }.
  board: { version: 1, title: "", elements: [] },
  // The mark of the file this page last read or wrote.
  mark: "",
  // Whether this page changes the board: its owner's, in the board's own
  // window. A guest looks; so does the owner beside the chat of the
  // workspace the board was made from (`preview`), where the agent draws.
  owner: Boolean(window.BOARD?.owner) && !window.BOARD?.preview,
  preview: Boolean(window.BOARD?.preview),
  // Ids of what is selected.
  selection: new Set(),
  // The tool in hand, and what it makes ({ shape, color, width… }).
  tool: "select",
  // The board point at the page's top left, and how large it is drawn.
  view: { x: 0, y: 0, scale: 1 },
  // The id of the element whose words are being typed.
  editing: null,
  canUndo: false,
  canRedo: false,
  // Kept for the next thing made of each kind.
  last: { sticky: "yellow", shape: "rounded", fill: "blue", pen: "ink", penWidth: 3, line: "ink", emoji: "👍" },
};

// "board" (what is on it changed), "selection", "view", "tool", "history",
// "editing", "command" (the app asks something), "saved" / "failed".
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

// How large a new element of each kind that takes room is, and one whose
// file names no size.
export const START_SIZES = { sticky: [200, 200], shape: [180, 110], text: [240, 30], frame: [640, 420], table: [420, 132], sticker: [72, 72], image: [320, 240] };

// ---- What is on the board --------------------------------------------------

export const elements = () => state.board.elements;
// Looked up many times a frame (every line asks for both its ends): by a
// table made once for a list, while the list is the same one, as long.
let table = { list: null, size: -1, ids: new Map() };
export function byId(id) {
  const list = elements();
  if (table.list !== list || table.size !== list.length) {
    table = { list, size: list.length, ids: new Map() };
    // The first of an id, as a search of the list finds it.
    for (const element of list) if (!table.ids.has(element.id)) table.ids.set(element.id, element);
  }
  const found = table.ids.get(id);
  // An id may have been given to another element since.
  return found && found.id === id ? found : list.find((element) => element.id === id);
}
export const selected = () => elements().filter((element) => state.selection.has(element.id));

let counter = 0;
// A short id no element of the board has.
export function newId(prefix = "e") {
  const taken = new Set(elements().map((element) => element.id));
  let id;
  do {
    counter += 1;
    id = prefix + (Date.now().toString(36).slice(-4) + counter.toString(36));
  } while (taken.has(id));
  return id;
}

// ---- Steps: undo and redo ---------------------------------------------------

const MAX_STEPS = 200;
// Steps are whole boards as text: together they are kept under this many
// characters (a board of long pen strokes is large), the oldest going first.
const MAX_STEP_CHARS = 24_000_000;
const undone = [];
const done = [];
// The board as it was before the change now under way.
let before = null;
let label = "";
let labelAt = 0;
// The file was written elsewhere during a change under way: it is read
// once that change is done.
let waiting = false;

const snapshot = () => JSON.stringify(state.board.elements);

// Keeps `was` as a step back. Only where the board is changed: a page
// that looks at it keeps none.
function step(was) {
  if (!state.owner) return;
  done.push(was);
  let held = done.reduce((sum, one) => sum + one.length, 0);
  while (done.length > 1 && (done.length > MAX_STEPS || held > MAX_STEP_CHARS)) held -= done.shift().length;
}

function history() {
  state.canUndo = done.length > 0;
  state.canRedo = undone.length > 0;
}

// Call before changing elements: keeps the board as it is, so that the
// change can be taken back.
export function begin() {
  if (before === null) before = snapshot();
}

// The change begun is done: a step of undo, and the file is written. A
// change of the same `kind` within two seconds joins the step before it
// (typing, nudging with the arrows).
export function commit(kind = "") {
  if (before === null) return;
  const now = snapshot();
  const was = before;
  before = null;
  if (was !== now) {
    const joined = kind && kind === label && Date.now() - labelAt < 2000 && done.length > 0;
    if (!joined) step(was);
    undone.length = 0;
    label = kind;
    labelAt = Date.now();
    changed();
  }
  settled();
}

// The change begun is dropped: the board is as it was.
export function cancel() {
  if (before === null) return;
  state.board.elements = JSON.parse(before);
  before = null;
  emit("board");
  settled();
}

// No change is under way any more: what was written elsewhere meanwhile
// is read now.
function settled() {
  if (!waiting) return;
  waiting = false;
  refresh();
}

// One call for a whole change: `edit()` edits the elements. An edit that
// fails leaves the board as it was, and no change under way.
export function change(kind, edit) {
  begin();
  try {
    edit();
  } catch (error) {
    cancel();
    throw error;
  }
  commit(kind);
}

// What is no longer there is no longer selected.
function prune() {
  const ids = new Set(elements().map((element) => element.id));
  let dropped = false;
  for (const id of [...state.selection]) {
    if (!ids.has(id)) {
      state.selection.delete(id);
      dropped = true;
    }
  }
  return dropped;
}

function changed() {
  history();
  const dropped = prune();
  emit("board");
  emit("history");
  if (dropped) {
    emit("selection");
    tellSelection();
  }
  saveSoon();
}

export function undo() {
  if (!state.owner || before !== null || done.length === 0) return;
  undone.push(snapshot());
  state.board.elements = JSON.parse(done.pop());
  label = "";
  changed();
}

export function redo() {
  if (!state.owner || before !== null || undone.length === 0) return;
  step(snapshot());
  state.board.elements = JSON.parse(undone.pop());
  label = "";
  changed();
}

// ---- Selection, tool, view ---------------------------------------------------

export function select(ids, { add = false } = {}) {
  const next = add ? new Set(state.selection) : new Set();
  for (const id of ids) {
    // A second pick of one already picked lets go of it.
    if (add && state.selection.has(id)) next.delete(id);
    else next.add(id);
  }
  // What moves together is picked together.
  const groups = new Set(elements().filter((element) => next.has(element.id) && element.group).map((element) => element.group));
  for (const element of elements()) if (element.group && groups.has(element.group)) next.add(element.id);
  state.selection = next;
  emit("selection");
  tellSelection();
}

export function setTool(tool) {
  if (state.tool === tool) return;
  state.tool = tool;
  emit("tool");
}

export function setView(view) {
  // A number that is none (a board with a hole in it was measured) would
  // leave nothing to look at, and no way back.
  const or = (value, held) => (Number.isFinite(value) ? value : held);
  const scale = Math.max(0.05, Math.min(8, or(view.scale, state.view.scale)));
  state.view = { x: or(view.x, state.view.x), y: or(view.y, state.view.y), scale };
  emit("view");
}

// ---- The file ------------------------------------------------------------------

const text = () => JSON.stringify(state.board, null, 2) + "\n";

// The file as this page last read or wrote it: each element as text, by
// its id. What differs from it here was changed here, and not yet written.
let synced = new Map();
const sync = () => (synced = new Map(elements().map((element) => [element.id, JSON.stringify(element)])));

// A board as read from the file, made safe to draw: agents write it by
// hand, and one thing written wrong must not take the board down. What is
// no element (a number, a null) is left out; every element has an id of
// its own, as text, and, where its kind takes room, a place and a size.
function tidy(board) {
  const list = board.elements.filter((element) => element && typeof element === "object" && !Array.isArray(element));
  const ids = new Set();
  const words = (value) => (value === null || value === undefined ? "" : String(value));
  for (const element of list) {
    if (typeof element.id === "number") element.id = String(element.id);
    if (typeof element.id !== "string" || !element.id || ids.has(element.id)) element.id = "e" + Math.random().toString(36).slice(2, 8);
    ids.add(element.id);
    for (const key of ["text", "title", "emoji", "src"]) if (key in element && typeof element[key] !== "string") element[key] = words(element[key]);
    if (element.type === "line") {
      for (const key of ["from", "to"]) {
        const end = element[key] && typeof element[key] === "object" ? element[key] : {};
        if (typeof end.id === "number") end.id = String(end.id);
        if (typeof end.id !== "string" || !end.id) {
          delete end.id;
          for (const axis of ["x", "y"]) if (!Number.isFinite(end[axis])) end[axis] = 0;
        }
        element[key] = end;
      }
      continue;
    }
    if (element.type === "draw") {
      element.points = (Array.isArray(element.points) ? element.points : []).filter((point) => Array.isArray(point) && Number.isFinite(point[0]) && Number.isFinite(point[1]));
      continue;
    }
    const size = START_SIZES[element.type];
    if (!size && element.type !== "comment") continue;
    for (const key of ["x", "y"]) if (!Number.isFinite(element[key])) element[key] = 0;
    if (element.type === "comment") {
      if (!Array.isArray(element.replies)) delete element.replies;
      continue;
    }
    if (!(element.w > 0)) element.w = size[0];
    if (!(element.h > 0)) element.h = size[1];
    if (element.type === "table") {
      const rows = (Array.isArray(element.rows) ? element.rows : []).filter(Array.isArray).map((row) => row.map(words));
      element.rows = rows.length ? rows : [[""]];
    }
    if ("votes" in element) element.votes = Math.max(0, Math.floor(Number(element.votes) || 0));
  }
  board.elements = list;
  return board;
}

// The board in `answer` ({ text, mark }), or null when it cannot be read.
function parse(answer) {
  let board;
  try {
    board = JSON.parse(answer.text);
  } catch {
    board = null;
  }
  if (!board || typeof board !== "object" || !Array.isArray(board.elements)) {
    emit("failed", new Error("The board's file cannot be read (it is not valid JSON). Ask the agent to write it again."));
    return null;
  }
  return tidy(board);
}

export async function load() {
  const response = await call("board");
  if (!response.ok) throw new Error("The board could not be read.");
  const answer = await response.json();
  const board = parse(answer);
  if (board) {
    state.board = board;
    state.mark = answer.mark;
    sync();
  }
  emit("board");
}

// The file was written elsewhere (an agent, another window): `theirs` is
// what it lists now. What was changed here since the file was last read
// and not written yet is kept where the other side left that thing alone;
// where both changed one thing, the file's wins. Answers with the list,
// how much of this page's is kept in it (to be written), and how much was
// given up.
function merge(theirs) {
  const mine = new Map(elements().map((element) => [element.id, element]));
  const named = new Set(theirs.map((element) => element.id));
  const list = [];
  let kept = 0;
  let lost = 0;
  for (const element of theirs) {
    const base = synced.get(element.id);
    const here = mine.get(element.id);
    const hereText = here ? JSON.stringify(here) : null;
    const untouched = base !== undefined && JSON.stringify(element) === base;
    if (base !== undefined && hereText !== base) {
      // Changed here (or taken away here) since.
      if (untouched) {
        kept += 1;
        if (here) list.push(here);
        continue;
      }
      if (hereText !== JSON.stringify(element)) lost += 1;
    }
    list.push(element);
  }
  for (const [id, element] of mine) {
    if (synced.has(id) || named.has(id)) {
      // Taken away there, changed here: gone with the file.
      if (synced.has(id) && !named.has(id) && JSON.stringify(element) !== synced.get(id)) lost += 1;
      continue;
    }
    // Made here since: sections under what they hold, the rest on top.
    kept += 1;
    if (element.type === "frame") {
      const first = list.findIndex((one) => one.type !== "frame");
      list.splice(first < 0 ? list.length : first, 0, element);
    } else list.push(element);
  }
  return { list, kept, lost };
}

// Takes the board the file holds now (`answer`), as one step that can be
// taken back, so that an agent's change is undone like one's own.
function adopt(answer) {
  if (answer.mark === state.mark) return;
  const board = parse(answer);
  if (!board) return;
  const was = snapshot();
  const theirs = new Map(board.elements.map((element) => [element.id, JSON.stringify(element)]));
  const merged = state.owner ? merge(board.elements) : { list: board.elements, kept: 0, lost: 0 };
  board.elements = merged.list;
  state.board = board;
  state.mark = answer.mark;
  synced = theirs;
  if (snapshot() !== was) {
    step(was);
    undone.length = 0;
    label = "";
  }
  history();
  prune();
  // Typing goes on in what is still there.
  if (state.editing && !byId(state.editing)) state.editing = null;
  clearTimeout(timer);
  timer = 0;
  emit("board");
  emit("history");
  emit("selection");
  tellSelection();
  if (merged.lost) emit("elsewhere");
  // What is kept of this page's is not in the file yet.
  if (merged.kept) saveSoon();
}

// Something else wrote the file: the board is read again. While a change
// is under way here (something dragged, a line drawn) it waits for it.
export async function refresh() {
  if (before !== null) {
    waiting = true;
    return;
  }
  const response = await call("board");
  if (!response.ok) return;
  const answer = await response.json();
  if (before !== null) {
    waiting = true;
    return;
  }
  adopt(answer);
}

let saving = null;
let wanted = false;
let timer = 0;
function saveSoon() {
  if (!state.owner) return;
  clearTimeout(timer);
  timer = setTimeout(() => {
    timer = 0;
    save();
  }, 350);
}

// A request the browser finishes though the page is gone takes this much
// at most.
const KEPT_ALIVE = 60_000;

function save({ leaving = false } = {}) {
  if (saving) {
    wanted = true;
    return;
  }
  const sent = text();
  const body = JSON.stringify({ text: sent, base: state.mark });
  const written = new Map(elements().map((element) => [element.id, JSON.stringify(element)]));
  saving = call("save", { method: "POST", body, keepalive: leaving && body.length < KEPT_ALIVE })
    .then(async (response) => {
      const answer = await response.json().catch(() => ({}));
      if (response.ok) {
        state.mark = answer.mark;
        synced = written;
        emit("saved");
      } else if (response.status === 409) {
        // Written elsewhere meanwhile: that is taken, with what was
        // changed here laid over it where the two do not meet.
        saving = null;
        adopt(answer);
      } else {
        emit("failed", new Error(answer.error || "The board could not be saved."));
      }
    })
    .catch((error) => emit("failed", error))
    .finally(() => {
      saving = null;
      if (wanted) {
        wanted = false;
        save();
      }
    });
}

// Writes what waits to be written, now (the page is being left).
export function flush() {
  if (!timer) return;
  clearTimeout(timer);
  timer = 0;
  save({ leaving: true });
}

export function tell(what) {
  return call("tell", { method: "POST", body: JSON.stringify(what) }).catch(() => {});
}

// What the app was last told is selected: nothing it knows of yet, so a
// page that opens says so (the app may still hold what a page before it
// had selected).
let told = null;
let tellTimer = 0;
export function tellSelection() {
  if (!state.owner) return;
  clearTimeout(tellTimer);
  tellTimer = setTimeout(() => {
    const ids = [...state.selection];
    const now = ids.join(",");
    if (now === told) return;
    told = now;
    tell({ selection: ids.length ? { ids } : null });
  }, 250);
}

// Keeps a picture with the board; answers with its path there (`media/…`).
export async function upload(file) {
  const response = await call("media?name=" + encodeURIComponent(file.name || "picture.png"), { method: "POST", body: file });
  const answer = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(answer.error || "The picture could not be kept.");
  return answer.src;
}

// Sends a picture of the whole board to the app: `wanted` is "thumbnail"
// (the board's picture in the list) or "export" (a file).
export async function sendPicture(wanted, blob) {
  const response = await call("picture?for=" + wanted, { method: "POST", body: blob });
  if (!response.ok) throw new Error("The picture could not be sent.");
}

export function listen() {
  const kind = window.artifact.kind;
  kind.on("board", (news) => {
    if (news.mark !== state.mark) refresh();
  });
  kind.on("chrome", ({ css }) => {
    const style = document.getElementById("chrome");
    if (style) style.textContent = `:root{${css}}`;
  });
  kind.on("command", (command) => emit("command", command));
}
