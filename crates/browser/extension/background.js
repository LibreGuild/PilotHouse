// PilotHouse's Chrome extension: lets PilotHouse's agents work in tabs of the
// "PilotHouse" tab group of this browser, with the user's own sign-ins.
//
// - It talks only to the PilotHouse app, through Chrome's native messaging
//   (host `com.libreguild.pilothouse`, which Chrome lets no other extension
//   start). There is no network port and no remote code.
// - Only tabs in a group named "PilotHouse" (and popups such a tab opened) are
//   ever listed, read or controlled. The user's other tabs are never reported.
// - The toolbar button shares the tab in front (it joins the group) or stops
//   sharing it (it leaves the group, and PilotHouse lets go of it).
// - Pages are driven with chrome.debugger, attached only while an agent works
//   in the tab, and only with the DevTools methods listed in ALLOWED.
'use strict';

const HOST = 'com.libreguild.pilothouse';
const GROUP_TITLE = 'PilotHouse';
const PROTOCOL = '1.3';
/** The largest message accepted from the app, assembled from its chunks. */
const MAX_MESSAGE = 32 * 1024 * 1024;
const MAX_CHUNKS = 256;
const RETRY_MIN = 5000;
const RETRY_MAX = 60000;
/** After the app said another browser is connected. */
const BUSY_PAUSE = 5 * 60 * 1000;
const NO_GROUP = -1;

/** The DevTools methods PilotHouse uses, and the only ones passed on. */
const ALLOWED = new Set([
  'DOM.setFileInputFiles',
  'Input.dispatchKeyEvent',
  'Input.dispatchMouseEvent',
  'Input.insertText',
  'Log.enable',
  'Network.enable',
  'Network.getResponseBody',
  'Page.addScriptToEvaluateOnNewDocument',
  'Page.captureScreenshot',
  'Page.createIsolatedWorld',
  'Page.enable',
  'Page.getFrameTree',
  'Page.getNavigationHistory',
  'Page.handleJavaScriptDialog',
  'Page.navigate',
  'Page.navigateToHistoryEntry',
  'Page.reload',
  'Page.setLifecycleEventsEnabled',
  'Runtime.addBinding',
  'Runtime.enable',
  'Runtime.evaluate',
]);

let port = null;
let retryDelay = RETRY_MIN;
let lastAttempt = 0;
let pausedUntil = 0;
/** Tabs the debugger is attached to, by this extension. */
const attached = new Set();
/** Tabs of popup windows (which cannot be grouped) opened by an allowed tab. */
const popups = new Set();
/** Allowed tabs PilotHouse knows of. */
const known = new Set();
/** Chunks of large messages from the app, by message. */
const chunks = new Map();

// ---- The connection -------------------------------------------------------

function connect() {
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
    for (const tabId of attached) chrome.debugger.detach({ tabId }).catch(() => {});
    attached.clear();
    known.clear();
    chunks.clear();
    retryDelay = Math.min(retryDelay * 2, RETRY_MAX);
    // Retried later, or sooner when a tab event wakes this worker.
    setTimeout(connect, retryDelay);
    void refreshAction();
  });
  post({
    method: 'PilotHouse.hello',
    params: { browser: browserName(), extension: chrome.runtime.getManifest().version },
  });
  void refreshAction();
}

function post(message) {
  if (!port) return;
  try {
    port.postMessage(message);
  } catch (error) {
    // The port closed meanwhile; onDisconnect tidies up.
  }
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
  const chrome = /Chrome\/(\d+)/.exec(agent);
  const version = chrome ? ` ${chrome[1]}` : '';
  return navigator.brave ? `Brave${version}` : `Chrome${version}`;
}

function fromApp(message) {
  if (!isObject(message)) return;
  // The app answered: retry quickly next time. Not when the host only says
  // the app is not running (or busy), or retries would never back off.
  if (message.method !== 'PilotHouse.unavailable' && message.method !== 'PilotHouse.busy') {
    retryDelay = RETRY_MIN;
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

function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

// ---- Requests from the app ------------------------------------------------

async function handle(message) {
  const { id, method, params, sessionId } = message;
  // Notes from the app, with no answer expected.
  if (id === undefined) {
    if (method === 'PilotHouse.busy') {
      pausedUntil = Date.now() + BUSY_PAUSE;
      port?.disconnect();
      port = null;
    }
    return;
  }
  if (!Number.isSafeInteger(id) || id < 1) return;
  if (typeof method !== 'string' || method.length > 100 || !/^[A-Za-z]+\.[A-Za-z]+$/.test(method)) {
    return fail(id, 'malformed request');
  }
  if (params !== undefined && !isObject(params)) return fail(id, 'malformed request');
  try {
    if (sessionId !== undefined) {
      if (typeof sessionId !== 'string' || !/^\d{1,12}$/.test(sessionId)) {
        return fail(id, 'malformed request');
      }
      const tabId = Number(sessionId);
      if (!attached.has(tabId)) return fail(id, 'PilotHouse is not attached to that tab');
      if (!ALLOWED.has(method)) return fail(id, `${method} is not allowed`);
      reply(id, await chrome.debugger.sendCommand({ tabId }, method, params ?? {}));
      return;
    }
    reply(id, await command(method, params ?? {}));
  } catch (error) {
    fail(id, error?.message ?? error);
  }
}

async function command(method, params) {
  switch (method) {
    case 'PilotHouse.listTabs':
      return { tabs: await allowedTabs() };
    case 'PilotHouse.openTab':
      return { tabId: await openTab(params.url) };
    case 'PilotHouse.closeTab':
      await allowed(params.tabId);
      await chrome.tabs.remove(params.tabId);
      return {};
    case 'PilotHouse.attach': {
      const tabId = await allowed(params.tabId);
      if (!attached.has(tabId)) {
        await chrome.debugger.attach({ tabId }, PROTOCOL);
        attached.add(tabId);
      }
      return {};
    }
    case 'PilotHouse.detach': {
      const tabId = params.tabId;
      if (attached.delete(tabId)) await chrome.debugger.detach({ tabId }).catch(() => {});
      return {};
    }
    case 'PilotHouse.activate': {
      const tabId = await allowed(params.tabId);
      await chrome.tabs.update(tabId, { active: true });
      return {};
    }
    default:
      throw new Error(`unknown method ${method}`);
  }
}

/** The tab, when PilotHouse may use it: in a PilotHouse group, or a popup one opened. */
async function allowed(tabId) {
  if (!Number.isSafeInteger(tabId)) throw new Error('give a tabId');
  if (popups.has(tabId)) return tabId;
  let tab;
  try {
    tab = await chrome.tabs.get(tabId);
  } catch (error) {
    throw new Error('that tab is closed');
  }
  if (!(await inGroup(tab))) {
    throw new Error('that tab is not in the PilotHouse group: the user has not shared it');
  }
  return tabId;
}

async function inGroup(tab) {
  if (tab.groupId === undefined || tab.groupId === NO_GROUP) return false;
  try {
    const group = await chrome.tabGroups.get(tab.groupId);
    return group.title === GROUP_TITLE;
  } catch (error) {
    return false;
  }
}

async function groups() {
  return chrome.tabGroups.query({ title: GROUP_TITLE });
}

async function allowedTabs() {
  const focused = await chrome.windows.getLastFocused({ windowTypes: ['normal', 'popup'] }).catch(() => null);
  const tabs = [];
  for (const group of await groups()) {
    tabs.push(...(await chrome.tabs.query({ groupId: group.id })));
  }
  for (const tabId of popups) {
    const tab = await chrome.tabs.get(tabId).catch(() => null);
    if (tab) tabs.push(tab);
  }
  return tabs.map((tab) => {
    known.add(tab.id);
    return {
      tabId: tab.id,
      url: tab.url ?? tab.pendingUrl ?? '',
      title: tab.title ?? '',
      active: tab.active,
      focused: focused !== null && focused.id === tab.windowId && focused.focused,
    };
  });
}

/** A new tab for an agent, in the PilotHouse group (made when there is none). */
async function openTab(url) {
  if (url !== undefined && url !== 'about:blank') {
    throw new Error('new tabs start blank; navigate them after');
  }
  const window = await chrome.windows.getLastFocused({ windowTypes: ['normal'] }).catch(() => null);
  const all = await groups();
  const group = all.find((g) => window && g.windowId === window.id) ?? all[0];
  let tab;
  if (group) {
    tab = await chrome.tabs.create({ windowId: group.windowId, url: 'about:blank', active: true });
    await chrome.tabs.group({ groupId: group.id, tabIds: [tab.id] });
  } else if (window) {
    tab = await chrome.tabs.create({ windowId: window.id, url: 'about:blank', active: true });
    await newGroup(tab.id);
  } else {
    const created = await chrome.windows.create({ url: 'about:blank', focused: false });
    tab = created.tabs[0];
    await newGroup(tab.id);
  }
  known.add(tab.id);
  return tab.id;
}

async function newGroup(tabId) {
  const groupId = await chrome.tabs.group({ tabIds: [tabId] });
  await chrome.tabGroups.update(groupId, { title: GROUP_TITLE, color: 'purple', collapsed: false });
  return groupId;
}

// ---- Events to the app ----------------------------------------------------

chrome.debugger.onEvent.addListener((source, method, params) => {
  // Only the page itself: not its frames' or workers' sessions.
  if (source.tabId === undefined || source.sessionId || !attached.has(source.tabId)) return;
  post({ method, params: params ?? {}, sessionId: String(source.tabId) });
});

chrome.debugger.onDetach.addListener((source, reason) => {
  if (source.tabId === undefined || !attached.delete(source.tabId)) return;
  post({ method: 'PilotHouse.detached', params: { tabId: source.tabId, reason } });
});

// A tab opened from a group tab. It is PilotHouse's only when Chrome put it
// in the group (a link from a group tab), or it is a popup window that a page
// an agent is driving opened. The extension never moves a tab into the group
// itself: a tab the user opens (Ctrl+T, even with a group tab in front) stays
// theirs.
chrome.tabs.onCreated.addListener(async (tab) => {
  connect();
  const opener = tab.openerTabId;
  if (!port || opener === undefined || !known.has(opener) || known.has(tab.id)) return;
  if (!(await inGroup(tab))) {
    if (!attached.has(opener)) return;
    const window = await chrome.windows.get(tab.windowId).catch(() => null);
    if (window?.type !== 'popup') return;
    popups.add(tab.id);
  }
  opened(tab, opener);
});

/** Tells the app of an allowed tab opened from `opener`: the agent driving
 * the opener takes it; otherwise it is a tab of the group like any other. */
function opened(tab, opener) {
  known.add(tab.id);
  const params = { tabId: tab.id, url: tab.pendingUrl ?? tab.url ?? '', title: tab.title ?? '' };
  if (attached.has(opener)) {
    post({ method: 'PilotHouse.tabCreated', params: { ...params, openerTabId: opener } });
  } else {
    post({ method: 'PilotHouse.tabJoined', params });
  }
}

chrome.tabs.onUpdated.addListener(async (tabId, change, tab) => {
  connect();
  if ('groupId' in change) {
    void refreshAction(tab);
    if (!port || popups.has(tabId)) return;
    const inside = await inGroup(tab);
    if (inside && !known.has(tabId)) {
      // Chrome may group a tab opened from a group tab only after creating it.
      if (tab.openerTabId !== undefined && known.has(tab.openerTabId)) {
        opened(tab, tab.openerTabId);
      } else {
        known.add(tabId);
        post({ method: 'PilotHouse.tabJoined', params: { tabId, url: tab.url ?? '', title: tab.title ?? '' } });
      }
    } else if (!inside && known.has(tabId)) {
      leave(tabId);
    }
    return;
  }
  if (!port || !known.has(tabId)) return;
  if (change.url !== undefined || change.title !== undefined) {
    post({ method: 'PilotHouse.tabUpdated', params: { tabId, url: tab.url ?? '', title: tab.title ?? '' } });
  }
});

chrome.tabs.onRemoved.addListener((tabId) => {
  popups.delete(tabId);
  attached.delete(tabId);
  if (known.delete(tabId)) post({ method: 'PilotHouse.tabRemoved', params: { tabId } });
});

chrome.tabs.onActivated.addListener(async ({ tabId }) => {
  connect();
  const tab = await chrome.tabs.get(tabId).catch(() => null);
  if (tab) void refreshAction(tab);
});

// A group renamed away from "PilotHouse" takes its tabs out of reach.
chrome.tabGroups.onUpdated.addListener(async (group) => {
  if (group.title === GROUP_TITLE) return;
  const tabs = await chrome.tabs.query({ groupId: group.id }).catch(() => []);
  for (const tab of tabs) if (known.has(tab.id)) leave(tab.id);
});

/** A tab left the group: PilotHouse lets go of it. */
function leave(tabId) {
  known.delete(tabId);
  if (attached.delete(tabId)) chrome.debugger.detach({ tabId }).catch(() => {});
  post({ method: 'PilotHouse.tabLeft', params: { tabId } });
}

// ---- The toolbar button ---------------------------------------------------

chrome.action.onClicked.addListener(async (tab) => {
  connect();
  if (tab.id === undefined) return;
  try {
    if (await inGroup(tab)) {
      await chrome.tabs.ungroup(tab.id);
    } else {
      const all = await groups();
      const here = all.find((group) => group.windowId === tab.windowId);
      if (here) {
        await chrome.tabs.group({ groupId: here.id, tabIds: [tab.id] });
      } else {
        await newGroup(tab.id);
      }
    }
  } catch (error) {
    // A tab of a popup window or an app cannot join a group.
  }
  const now = await chrome.tabs.get(tab.id).catch(() => null);
  if (now) void refreshAction(now);
});

async function refreshAction(tab) {
  const state = port ? '' : ' (PilotHouse is not connected)';
  if (!tab) {
    await chrome.action.setTitle({ title: `Share this tab with PilotHouse${state}` }).catch(() => {});
    return;
  }
  const shared = await inGroup(tab);
  const title = shared ? 'Stop sharing this tab with PilotHouse' : 'Share this tab with PilotHouse';
  await chrome.action.setTitle({ tabId: tab.id, title: title + state }).catch(() => {});
}

chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
