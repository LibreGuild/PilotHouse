// The connection to the PilotHouse app: Chrome's native messaging to the
// host `com.libreguild.pilothouse` (which Chrome lets no other extension
// start). There is no network port and no remote code.
//
// - Requests from the app carry an `id` and get one answer (see
//   `setRequestHandler`); notes from the app have none (`onAppNote`).
// - Notes to the app have no id either (`post`); `ask` sends one that the
//   app answers with a `PilotHouse.answer` note naming the same `ref`.

const HOST = 'com.libreguild.pilothouse';
/** The largest message accepted from the app, assembled from its chunks. */
const MAX_MESSAGE = 32 * 1024 * 1024;
const MAX_CHUNKS = 256;
const RETRY_MIN = 5000;
const RETRY_MAX = 60000;
/** After the app said another browser is connected. */
const BUSY_PAUSE = 5 * 60 * 1000;
/** How long `ask` waits for the app's answer. */
const ASK_TIMEOUT = 15000;

let port = null;
/** The app itself answered on `port` (not only the host saying it is not
 * running, or busy with another browser). */
let linked = false;
let retryDelay = RETRY_MIN;
let lastAttempt = 0;
let pausedUntil = 0;
/** Chunks of large messages from the app, by message. */
const chunks = new Map();
const notes = new Map();
const listeners = new Set();
const answers = new Map();
let nextRef = 1;
let requestHandler = async () => {
  throw new Error('not ready');
};

/** Handles requests from the app (messages with an id); returns the result. */
export function setRequestHandler(handler) {
  requestHandler = handler;
}

/** Calls `handler(params)` for each note `method` from the app. */
export function onAppNote(method, handler) {
  notes.set(method, handler);
}

/** Calls `listener(connected)` whenever the connection opens or closes. */
export function onConnection(listener) {
  listeners.add(listener);
}

export function connected() {
  return port !== null && linked;
}

export function connect() {
  const now = Date.now();
  if (port || now < pausedUntil || now - lastAttempt < retryDelay) return;
  lastAttempt = now;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch (error) {
    port = null;
    return;
  }
  port.onMessage.addListener(fromApp);
  port.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    port = null;
    chunks.clear();
    for (const [, waiting] of answers) waiting.reject(new Error('PilotHouse is not running'));
    answers.clear();
    retryDelay = Math.min(retryDelay * 2, RETRY_MAX);
    // Retried later, or sooner when a tab event wakes this worker.
    setTimeout(connect, retryDelay);
    if (linked) {
      linked = false;
      tell(false);
    }
  });
  // Connected once the app speaks (it sends its status for the hello): the
  // host starts even when the app is not running, only to say so and quit.
  post({
    method: 'PilotHouse.hello',
    params: { browser: browserName(), extension: chrome.runtime.getManifest().version },
  });
}

function tell(isConnected) {
  for (const listener of listeners) {
    try {
      listener(isConnected);
    } catch (error) {
      // One listener's failure does not stop the others.
    }
  }
}

export function post(message) {
  if (!port) return false;
  try {
    port.postMessage(message);
    return true;
  } catch (error) {
    // The port closed meanwhile; onDisconnect tidies up.
    return false;
  }
}

/** Sends a note to the app, connecting first when needed. */
export function send(method, params) {
  connect();
  if (!post({ method, params: params ?? {} })) {
    throw new Error('PilotHouse is not running. Open PilotHouse, then try again.');
  }
}

/** Asks the app something; resolves with its answer. */
export function ask(method, params) {
  connect();
  const ref = nextRef++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      answers.delete(ref);
      reject(new Error('PilotHouse did not answer'));
    }, ASK_TIMEOUT);
    answers.set(ref, {
      resolve: (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      },
    });
    if (!post({ method: 'PilotHouse.ask', params: { ref, method, params: params ?? {} } })) {
      answers.get(ref)?.reject(new Error('PilotHouse is not running. Open PilotHouse, then try again.'));
      answers.delete(ref);
    }
  });
}

function reply(id, result) {
  post({ id, result: result ?? {} });
}

function fail(id, message) {
  post({ id, error: { message: String(message) } });
}

function browserName() {
  const agent = navigator.userAgent;
  const edge = /Edg\/(\d+)/.exec(agent);
  if (edge) return `Microsoft Edge ${edge[1]}`;
  const version = /Chrome\/(\d+)/.exec(agent);
  const number = version ? ` ${version[1]}` : '';
  return navigator.brave ? `Brave${number}` : `Chrome${number}`;
}

function fromApp(message) {
  if (!isObject(message)) return;
  // The app answered: retry quickly next time. Not when the host only says
  // the app is not running (or busy), or retries would never back off.
  if (message.method !== 'PilotHouse.unavailable' && message.method !== 'PilotHouse.busy') {
    retryDelay = RETRY_MIN;
    if (!linked) {
      linked = true;
      tell(true);
    }
  }
  if (isObject(message.chunk)) {
    const whole = assemble(message.chunk);
    if (whole !== undefined) void handle(whole);
    return;
  }
  void handle(message);
}

/** Puts a large message back together; undefined until it is whole. */
function assemble(chunk) {
  const { id, index, count, data } = chunk;
  if (!Number.isSafeInteger(id) || !Number.isSafeInteger(count) || !Number.isSafeInteger(index)
      || count < 1 || count > MAX_CHUNKS || index < 0 || index >= count || typeof data !== 'string') {
    return undefined;
  }
  let parts = chunks.get(id);
  if (!parts) {
    parts = { count, received: 0, size: 0, data: new Array(count) };
    chunks.set(id, parts);
  }
  if (parts.count !== count || parts.data[index] !== undefined) return undefined;
  parts.data[index] = data;
  parts.received += 1;
  parts.size += data.length;
  if (parts.size > MAX_MESSAGE) {
    chunks.delete(id);
    return undefined;
  }
  if (parts.received < count) return undefined;
  chunks.delete(id);
  try {
    const whole = JSON.parse(parts.data.join(''));
    return isObject(whole) ? whole : undefined;
  } catch (error) {
    return undefined;
  }
}

export function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

async function handle(message) {
  const { id, method, params } = message;
  // Notes from the app, with no answer expected.
  if (id === undefined) {
    if (method === 'PilotHouse.busy') {
      pausedUntil = Date.now() + BUSY_PAUSE;
      port?.disconnect();
      port = null;
      linked = false;
      tell(false);
      return;
    }
    if (method === 'PilotHouse.answer' && isObject(params)) {
      const waiting = answers.get(params.ref);
      if (!waiting) return;
      answers.delete(params.ref);
      if (isObject(params.error)) waiting.reject(new Error(String(params.error.message ?? 'failed')));
      else waiting.resolve(params.result ?? {});
      return;
    }
    const handler = typeof method === 'string' ? notes.get(method) : undefined;
    if (handler && (params === undefined || isObject(params))) {
      try {
        await handler(params ?? {});
      } catch (error) {
        // A note's handler failing changes nothing for the app.
      }
    }
    return;
  }
  if (!Number.isSafeInteger(id) || id < 1) return;
  if (typeof method !== 'string' || method.length > 100 || !/^[A-Za-z]+\.[A-Za-z]+$/.test(method)) {
    return fail(id, 'malformed request');
  }
  if (params !== undefined && !isObject(params)) return fail(id, 'malformed request');
  try {
    reply(id, await requestHandler(message));
  } catch (error) {
    fail(id, error?.message ?? error);
  }
}
