// PilotHouse's Chrome extension.
//
// - Agents work in tabs of the "PilotHouse" tab group of this browser, with
//   the user's own sign-ins (`lib/tabs.js`). Tabs outside it are never
//   listed, read or controlled for agents. Pages are driven with
//   chrome.debugger, attached only while an agent works in the tab, and only
//   with the DevTools methods listed in ALLOWED.
// - The user sends things to PilotHouse from any tab (`features/send.js`),
//   only when they ask: the toolbar button's popup, the page's right-click
//   menu or a shortcut.
// - The toolbar button opens the popup (`popup.html`): send, record, let
//   agents use the tab, and what waits for the user.
// - Nothing runs in a page by itself, and no site is reachable until the
//   user points at a tab: the extension asks for no site at all.
// - It talks only to the PilotHouse app, through Chrome's native messaging
//   (`lib/link.js`). There is no network port and no remote code.

import { connect, onConnection, post, send, setRequestHandler } from './lib/link.js';
import { attached, allowed, allowedTabs, frontTab, inGroup, known, openTab, popups, setShared } from './lib/tabs.js';
import * as sending from './features/send.js';
import * as statusFeature from './features/status.js';
import * as recording from './features/record.js';

const PROTOCOL = '1.3';

/** The DevTools methods PilotHouse uses, and the only ones passed on. */
const ALLOWED = new Set([
  'DOM.setFileInputFiles',
  'Emulation.clearDeviceMetricsOverride',
  'Emulation.clearGeolocationOverride',
  'Emulation.setCPUThrottlingRate',
  'Emulation.setDeviceMetricsOverride',
  'Emulation.setEmulatedMedia',
  'Emulation.setGeolocationOverride',
  'Emulation.setLocaleOverride',
  'Emulation.setTimezoneOverride',
  'Emulation.setTouchEmulationEnabled',
  'Fetch.continueRequest',
  'Fetch.disable',
  'Fetch.enable',
  'Fetch.failRequest',
  'Fetch.fulfillRequest',
  'Input.dispatchKeyEvent',
  'Input.dispatchMouseEvent',
  'Input.insertText',
  'Log.enable',
  'Network.emulateNetworkConditions',
  'Network.enable',
  'Network.getResponseBody',
  'Network.setCacheDisabled',
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
  'Performance.enable',
  'Performance.getMetrics',
  'Runtime.addBinding',
  'Runtime.enable',
  'Runtime.evaluate',
  'Runtime.getHeapUsage',
  'WebMCP.enable',
  'WebMCP.invokeTool',
]);

// ---- Requests from the app ------------------------------------------------

setRequestHandler(async ({ method, params, sessionId }) => {
  if (sessionId !== undefined) {
    if (typeof sessionId !== 'string' || !/^\d{1,12}$/.test(sessionId)) throw new Error('malformed request');
    const tabId = Number(sessionId);
    if (!attached.has(tabId)) throw new Error('PilotHouse is not attached to that tab');
    if (!ALLOWED.has(method)) throw new Error(`${method} is not allowed`);
    return chrome.debugger.sendCommand({ tabId }, method, params ?? {});
  }
  return command(method, params ?? {});
});

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
        try {
          await chrome.debugger.attach({ tabId }, PROTOCOL);
        } catch (error) {
          throw new Error(attachError(error));
        }
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

/** Why Chrome would not let the debugger attach, in words the agent can
 * pass on. A managed Chrome (Chrome 155 and later) checks the
 * organisation's policies when an extension attaches its debugger. */
function attachError(error) {
  const message = String(error?.message ?? error);
  if (/policy/i.test(message)) {
    return `Your organisation's Chrome policy does not let extensions control this tab (Chrome said: ${message}). Ask the user: their IT admin can allow it, or use PilotHouse's own browser pane (the browser_ tools) meanwhile.`;
  }
  if (/another debugger|already attached/i.test(message)) {
    return 'Chrome DevTools (or another extension) is debugging this tab. Ask the user to close DevTools on it, then try again.';
  }
  return message;
}

onConnection((isConnected) => {
  if (isConnected) return;
  for (const tabId of attached) chrome.debugger.detach({ tabId }).catch(() => {});
  attached.clear();
  known.clear();
});

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
  if (opener === undefined || !known.has(opener) || known.has(tab.id)) return;
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
    if (popups.has(tabId)) return;
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
  if (!known.has(tabId)) return;
  if (change.url !== undefined || change.title !== undefined) {
    post({ method: 'PilotHouse.tabUpdated', params: { tabId, url: tab.url ?? '', title: tab.title ?? '' } });
  }
});

chrome.tabs.onRemoved.addListener((tabId) => {
  popups.delete(tabId);
  attached.delete(tabId);
  if (known.delete(tabId)) post({ method: 'PilotHouse.tabRemoved', params: { tabId } });
});

// A tab coming forward is Chrome in use: the link to the app is kept up.
chrome.tabs.onActivated.addListener(() => connect());

// A group renamed away from "PilotHouse" takes its tabs out of reach.
chrome.tabGroups.onUpdated.addListener(async (group) => {
  if (group.title === 'PilotHouse') return;
  const tabs = await chrome.tabs.query({ groupId: group.id }).catch(() => []);
  for (const tab of tabs) if (known.has(tab.id)) leave(tab.id);
});

/** A tab left the group: PilotHouse lets go of it. */
function leave(tabId) {
  known.delete(tabId);
  if (attached.delete(tabId)) chrome.debugger.detach({ tabId }).catch(() => {});
  post({ method: 'PilotHouse.tabLeft', params: { tabId } });
}

// ---- What the user does ---------------------------------------------------

/** Runs one of the user's actions on `tab`, telling them how it went.
 * `shown`: the popup is open and says how it went itself. */
async function act(action, tab, info = {}, shown = false) {
  try {
    switch (action) {
      case 'page':
        return await sending.sendPage(tab);
      case 'selection':
        return await sending.sendSelection(tab, info.selectionText);
      case 'link':
        return await sending.sendLink(tab, info.linkUrl, info.selectionText ?? info.linkText);
      case 'image':
        return await sending.sendImage(tab, info.srcUrl);
      case 'area':
        return await sending.sendArea(tab);
      case 'element':
        return await sending.sendElement(tab);
      case 'bug':
      case 'workflow':
        await recording.start(action, tab, info.streamId);
        return {};
      case 'stop':
        return await recording.finish(true);
      case 'cancel':
        await recording.finish(false);
        return null;
      case 'share':
        await setShared(tab, typeof info.checked === 'boolean' ? info.checked : !(await inGroup(tab)));
        return {};
      default:
        throw new Error(`unknown action ${action}`);
    }
  } catch (error) {
    // The popup shows the error itself; elsewhere a notification says it.
    if (shown) void sending.flash(tab.id, false, String(error?.message ?? error));
    else await sending.failed(tab, error);
    throw error;
  }
}

// The page's menu holds what a right-click points at (the selection, a
// link, an image, the page) and the two that go on in the page. Recording,
// sharing the tab and where things go are in the popup, each in one place.
const MENU = [
  ['selection', 'Send the selection', ['selection']],
  ['link', 'Send this link', ['link']],
  ['image', 'Send this image', ['image']],
  ['page', 'Send this page', ['page', 'frame']],
  ['area', 'Screenshot an area…', ['page', 'frame', 'image', 'link', 'selection']],
  ['element', 'Pick an element…', ['page', 'frame', 'image', 'link']],
];

async function createMenus() {
  await chrome.contextMenus.removeAll();
  for (const [id, title, contexts] of MENU) {
    chrome.contextMenus.create({ id, title, contexts }, () => void chrome.runtime.lastError);
  }
}

// The bar's Stop and Cancel in the page.
recording.onBarFinish(async (keep) => {
  const now = recording.current();
  const tab = now ? await chrome.tabs.get(now.tabId).catch(() => null) : null;
  try {
    await recording.finish(keep);
  } catch (error) {
    await sending.failed(tab, error);
  }
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const id = String(info.menuItemId);
  if (!tab?.id || !MENU.some(([known]) => known === id)) return;
  connect();
  await act(id, tab, info).catch(() => {});
});

chrome.commands.onCommand.addListener(async (command, tab) => {
  connect();
  const target = tab ?? (await frontTab());
  if (!target) return;
  const action = { 'send-to-pilothouse': 'selection-or-page', 'screenshot-area': 'area', 'pick-element': 'element' }[command];
  if (!action) return;
  if (action === 'selection-or-page') {
    // The selection when there is one, else the page.
    const [result] = await chrome.scripting.executeScript({
      target: { tabId: target.id },
      func: () => String(getSelection() ?? '').trim().length > 0,
    }).catch(() => []);
    await act(result?.result ? 'selection' : 'page', target).catch(() => {});
    return;
  }
  await act(action, target).catch(() => {});
});

// ---- The popup ------------------------------------------------------------

// Messages from the popup (what it shows, and the user's actions) and from
// the recorder in a page.
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id !== chrome.runtime.id) return false;
  // The recorder in a page (its steps, and the bar's buttons): the only
  // thing of the extension's that speaks from a page.
  if (sender.tab) {
    if (typeof message?.type === 'string' && message.type.startsWith('recorder-')) {
      recording.fromRecorder(message, sender);
    }
    return false;
  }
  if (message?.target === 'offscreen') return false;
  void popupRequest(message).then(
    (result) => respond({ ok: true, result }),
    (error) => respond({ ok: false, error: String(error?.message ?? error) }),
  );
  return true;
});

/** What goes on in the page once the popup has closed. */
const IN_PAGE = new Set(['area', 'element']);

async function popupRequest(message) {
  switch (message?.type) {
    case 'status':
      return statusFeature.status();
    case 'act': {
      const tab = await chrome.tabs.get(message.tabId);
      return act(message.action, tab, { streamId: message.streamId, checked: message.checked }, true);
    }
    // Started here, finished without the popup: it closes so that the user
    // can drag over the page or click in it, and is not there to hear how
    // it went (the toolbar icon says, and a notification if it failed).
    case 'start': {
      if (!IN_PAGE.has(message.action)) throw new Error('unknown request');
      const tab = await chrome.tabs.get(message.tabId);
      void act(message.action, tab).catch(() => {});
      return {};
    }
    case 'recording':
      return recording.current();
    case 'agent':
      send('PilotHouse.agentControl', { owner: String(message.owner), paused: !!message.paused });
      return {};
    case 'openChat':
      send('PilotHouse.openChat', { tag: String(message.tag) });
      return {};
    default:
      throw new Error('unknown request');
  }
}

chrome.runtime.onConnect.addListener((port) => {
  // Content scripts are this extension too, but run in pages: only the
  // popup (no tab) hears what agents do.
  if (port.name !== 'popup' || port.sender?.id !== chrome.runtime.id || port.sender.tab) return;
  connect();
  const stop = statusFeature.watch((status) => {
    try {
      port.postMessage({ type: 'status', status });
    } catch (error) {
      stop();
    }
  });
  const stopRecording = recording.watch((now) => {
    try {
      port.postMessage({ type: 'recording', recording: now });
    } catch (error) {
      stopRecording();
    }
  });
  port.postMessage({ type: 'status', status: statusFeature.status() });
  port.postMessage({ type: 'recording', recording: recording.current() });
  port.onDisconnect.addListener(() => {
    stop();
    stopRecording();
  });
});

// ---- Start ----------------------------------------------------------------

statusFeature.install();
recording.install();

/** What versions before 0.7 left in the browser: the buttons they put on
 * GitHub, Jira and Linear pages, registered to outlive the worker. */
async function forgetOldVersions() {
  await chrome.scripting.unregisterContentScripts({ ids: ['pilothouse-site-buttons'] }).catch(() => {});
}

chrome.runtime.onInstalled.addListener(async () => {
  await createMenus();
  await forgetOldVersions();
  connect();
});
chrome.runtime.onStartup.addListener(async () => {
  await createMenus();
  connect();
});
connect();
