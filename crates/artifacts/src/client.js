// What a page can ask of the app that keeps it. The server puts this script
// into every artifact page (server.rs), before the page's own scripts run.
//
//   artifact.storage   what this person keeps here, for themselves
//   artifact.shared    what everyone with the page open reads and writes
//   artifact.room      who is here now, and messages between them
//   artifact.me        who this is: { id, name, host }
(() => {
  "use strict";
  if (window.artifact) return;

  // A guest came through the link the owner shared; the server says so in
  // this script's own address.
  const script = document.currentScript;
  const guest = Boolean(script) && new URL(script.src).search === "?guest";

  // A browser may refuse to keep anything (a private window): then nothing
  // is remembered, and nothing fails.
  const remembered = (key) => {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  };
  const remember = (key, value) => {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Not kept.
    }
  };

  // Who this is: an id this browser keeps for this artifact. It tells
  // people apart; it proves nothing.
  // `?artifact-who=second` in the address is someone else for this once:
  // two windows of one browser are otherwise one person, and a game wants
  // trying alone.
  const named = /^[A-Za-z0-9_-]{1,40}$/;
  const asked = new URLSearchParams(location.search).get("artifact-who");
  let id = asked && named.test(asked) ? asked : remembered("artifact.who");
  if (!id || !named.test(id)) {
    const bytes = crypto.getRandomValues(new Uint8Array(10));
    id = "p-" + Array.from(bytes, (byte) => (byte % 36).toString(36)).join("");
    remember("artifact.who", id);
  }
  let name = (remembered("artifact.name") || "").slice(0, 40);

  // This page's number in the room, once its stream is up.
  let page = null;

  const checked = (key) => {
    if (typeof key !== "string" || key === "") {
      throw new TypeError("A storage key is a non-empty string.");
    }
    return key;
  };

  const request = async (path, method, key, body) => {
    const url = key === undefined ? path : `${path}?key=${encodeURIComponent(checked(key))}`;
    const headers = {};
    if (body !== undefined) headers["Content-Type"] = "application/json";
    if (page !== null) headers["X-Artifact-Page"] = String(page);
    const response = await fetch(url, { method, body, headers, cache: "no-store" });
    if (response.ok) return response;
    let message = `The app answered ${response.status}.`;
    try {
      message = (await response.json()).error || message;
    } catch {
      // Not ours to read: the status says enough.
    }
    throw new Error(message);
  };

  const asText = (value) => {
    const text = JSON.stringify(value);
    if (text === undefined) throw new TypeError("A stored value is something JSON can hold.");
    return text;
  };

  // Values the app keeps with the artifact, under `path`.
  const kept = (path) => ({
    // The value under `key`, or null when there is none.
    async get(key) {
      return (await request(path, "GET", key)).json();
    },
    // Keeps any JSON value under `key`.
    async set(key, value) {
      await request(path, "PUT", key, asText(value));
    },
    async remove(key) {
      await request(path, "DELETE", key);
    },
    // Every key, by name.
    async keys() {
      return (await (await request(path, "GET")).json()).keys.map((entry) => entry.key);
    },
  });

  // A guest's own values stay in the guest's browser: the owner's computer
  // keeps nothing for them, and gives them nothing of the owner's.
  const inBrowser = () => {
    const prefix = "artifact.storage:";
    return {
      async get(key) {
        const text = localStorage.getItem(prefix + checked(key));
        return text === null ? null : JSON.parse(text);
      },
      async set(key, value) {
        localStorage.setItem(prefix + checked(key), asText(value));
      },
      async remove(key) {
        localStorage.removeItem(prefix + checked(key));
      },
      async keys() {
        return Object.keys(localStorage)
          .filter((key) => key.startsWith(prefix))
          .map((key) => key.slice(prefix.length))
          .sort();
      },
    };
  };

  const listeners = { message: new Set(), people: new Set(), change: new Set() };
  const fire = (kind, value) => {
    for (const listener of [...listeners[kind]]) {
      try {
        listener(value);
      } catch (error) {
        console.error(error);
      }
    }
  };
  // `on(kind, listener)` for the kinds given; what it returns takes the
  // listener off again.
  const on = (...kinds) => (kind, listener) => {
    if (!kinds.includes(kind)) throw new TypeError(`There is no "${kind}" here: ${kinds.join(", ")}.`);
    listeners[kind].add(listener);
    return () => listeners[kind].delete(listener);
  };

  // What everyone reads and writes. `on("change", ({ key, value }) => …)`
  // hears what someone else changed.
  const shared = Object.freeze({ ...kept("/_artifact/shared"), on: on("change") });

  // Whether this page may send and write what is shared (a guest of an
  // artifact shared to look at only may not).
  let mayChange = !guest;
  let people = [];

  // Messages go out one batch at a time, in the order they were sent.
  const waiting = [];
  let sending = false;
  const pump = async () => {
    if (sending || page === null || waiting.length === 0) return;
    sending = true;
    const batch = waiting.splice(0, 50);
    try {
      const messages = batch.map(({ to, data }) => ({ to, data }));
      await request("/_artifact/room", "POST", undefined, asText({ page, messages }));
      batch.forEach((message) => message.done());
    } catch (error) {
      batch.forEach((message) => message.failed(error));
    }
    sending = false;
    pump();
  };

  const room = Object.freeze({
    // Sends any JSON value to everyone else here now, or with `{ to: id }`
    // to one person. Nothing is kept: who is not here does not get it.
    send(data, options) {
      return new Promise((done, failed) => {
        waiting.push({ to: options?.to ?? null, data, done, failed });
        pump();
      });
    },
    // Who is here now: [{ id, name, host }], in the order they came.
    people() {
      return people.map((person) => ({ ...person }));
    },
    // on("message", ({ from, data }) => …), on("people", (list) => …).
    on: on("message", "people"),
    // What this person is called, for everyone here.
    async setName(value) {
      name = String(value).trim().slice(0, 40);
      remember("artifact.name", name);
      if (page !== null) await request("/_artifact/room", "POST", undefined, asText({ page, name }));
    },
    get canSend() {
      return mayChange;
    },
  });

  // For the app's own pages (a deck's): what its kind tells on this page's
  // stream, and its calls, made as this page.
  const kindHeard = new Map();
  const kind = Object.freeze({
    // on("deck", (data) => …): one of the kind's events.
    on(event, listener) {
      if (!kindHeard.has(event)) {
        const set = new Set();
        kindHeard.set(event, set);
        events.addEventListener(event, (message) => {
          const data = JSON.parse(message.data);
          for (const each of [...set]) {
            try {
              each(data);
            } catch (error) {
              console.error(error);
            }
          }
        });
      }
      kindHeard.get(event).add(listener);
      return () => kindHeard.get(event).delete(listener);
    },
    // fetch(), saying which open page asks.
    call(path, options = {}) {
      const headers = { ...(options.headers || {}) };
      if (page !== null) headers["X-Artifact-Page"] = String(page);
      return fetch(path, { cache: "no-store", ...options, headers });
    },
    // This page's number in the room, once its stream is up.
    get page() {
      return page;
    },
  });

  window.artifact = Object.freeze({
    storage: Object.freeze(guest ? inBrowser() : kept("/_artifact/storage")),
    shared,
    room,
    kind,
    get me() {
      return { id, name, host: !guest };
    },
  });

  // The page's stream. It says which publish is live when the page connects
  // and again whenever that changes (also after the app was started again):
  // a page that is open shows a new publish by itself.
  let live = null;
  const events = new EventSource(`/_artifact/events?who=${id}&name=${encodeURIComponent(name)}`);
  const heard = (kind, then) =>
    events.addEventListener(kind, (event) => then(JSON.parse(event.data)));
  events.addEventListener("version", (event) => {
    if (live !== null && event.data !== live) location.reload();
    live = event.data;
  });
  heard("hello", (hello) => {
    page = hello.page;
    people = hello.people;
    mayChange = hello.change;
    fire("people", room.people());
    // Named since the stream was asked for.
    if (hello.you.name !== name) room.setName(name).catch(() => {});
    pump();
  });
  heard("people", (list) => {
    people = list;
    fire("people", room.people());
  });
  heard("message", (message) => fire("message", message));
  heard("access", (access) => {
    mayChange = access.change;
  });
  heard("shared", async ({ key }) => {
    try {
      fire("change", { key, value: await shared.get(key) });
    } catch (error) {
      console.error(error);
    }
  });
  // The stream broke; the browser connects again by itself, and what waits
  // to be sent goes out then.
  events.addEventListener("error", () => {
    page = null;
  });
})();
