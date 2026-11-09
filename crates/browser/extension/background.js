// PilotHouse's Chrome extension.
//
// - Agents work in tabs of the "PilotHouse" tab group of this browser, with
//   the user's own sign-ins (`lib/tabs.js`). Tabs outside it are never
//   listed, read or controlled for agents. Pages are driven with
//   chrome.debugger, attached only while an agent works in the tab, and only
//   with the DevTools methods listed in ALLOWED.
// - The user sends things to PilotHouse from any tab (`features/send.js`),
//   only when they ask: the right-click menu, a shortcut or the side panel.
// - The toolbar button opens the side panel: what agents do, what waits for
//   the user, and the extension's choices.
// - It talks only to the PilotHouse app, through Chrome's native messaging
//   (`lib/link.js`). There is no network port and no remote code.

import { connect, onConnection, post, send, setRequestHandler } from './lib/link.js';
import { attached, allowed, allowedTabs, frontTab, inGroup, known, openTab, popups, setShared } from './lib/tabs.js';
import { onSettings, settings, update } from './lib/settings.js';
import { inPage } from './lib/page.js';
import * as sending from './features/send.js';
import * as statusFeature from './features/status.js';
import * as recording from './features/record.js';
import * as sites from './features/sites.js';

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
    void refreshMenus(tab);
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

chrome.tabs.onActivated.addListener(async ({ tabId }) => {
  connect();
  const tab = await chrome.tabs.get(tabId).catch(() => null);
  if (tab) void refreshMenus(tab);
});

// With several windows, the tab in front changes with no tab activated.
chrome.windows.onFocusChanged.addListener(async (windowId) => {
  if (windowId === chrome.windows.WINDOW_ID_NONE) return;
  const [tab] = await chrome.tabs.query({ active: true, windowId }).catch(() => []);
  if (tab) void refreshMenus(tab);
});

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

/** Runs one of the user's actions on `tab`, telling them how it went. */
async function act(action, tab, info = {}, fromPanel = false) {
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
        // What the checkbox now says (the menu's can be stale for this tab).
        await setShared(tab, typeof info.checked === 'boolean' ? info.checked : !(await inGroup(tab)));
        await refreshMenus(await chrome.tabs.get(tab.id));
        return {};
      default:
        throw new Error(`unknown action ${action}`);
    }
  } catch (error) {
    // The panel shows the error itself; elsewhere a notification says it.
    if (fromPanel) void sending.flash(tab.id, false, String(error?.message ?? error));
    else await sending.failed(tab, error);
    throw error;
  }
}

const MENU = {
  selection: 'Send the selection',
  page: 'Send this page',
  link: 'Send this link',
  image: 'Send this image',
  area: 'Screenshot an area…',
  element: 'Pick an element…',
  bug: 'Record a bug report…',
  workflow: 'Record a workflow…',
  stop: 'Stop recording and send',
  share: 'Let agents use this tab',
  panel: 'Open the PilotHouse panel',
};

async function createMenus() {
  await chrome.contextMenus.removeAll();
  const add = (id, title, contexts, extra = {}) =>
    chrome.contextMenus.create({ id, title, contexts, ...extra }, () => void chrome.runtime.lastError);
  add('selection', MENU.selection, ['selection']);
  add('link', MENU.link, ['link']);
  add('image', MENU.image, ['image']);
  add('page', MENU.page, ['page', 'frame']);
  add('area', MENU.area, ['page', 'frame', 'image', 'link', 'selection']);
  add('element', MENU.element, ['page', 'frame', 'image', 'link']);
  add('sep-1', '', ['all'], { type: 'separator' });
  add('bug', MENU.bug, ['page', 'frame']);
  add('workflow', MENU.workflow, ['page', 'frame']);
  add('stop', MENU.stop, ['all'], { visible: false });
  add('sep-4', '', ['all'], { type: 'separator' });
  add('share', MENU.share, ['page', 'frame'], { type: 'checkbox', checked: false });
  add('sep-2', '', ['all'], { type: 'separator' });
  const chosen = await settings();
  for (const [id, where] of Object.entries(sending.DESTINATIONS)) {
    add(`to-${id}`, `Send to ${where}`, ['all'], { type: 'radio', checked: chosen.destination === id });
  }
  add('sep-3', '', ['all'], { type: 'separator' });
  add('panel', MENU.panel, ['all']);
}

/** The share checkbox follows the tab in front of the window in front. */
async function refreshMenus(tab) {
  if (!tab?.active) return;
  const front = await chrome.windows.getLastFocused().catch(() => null);
  if (front && front.id !== tab.windowId) return;
  const shared = await inGroup(tab);
  chrome.contextMenus.update('share', { checked: shared }, () => void chrome.runtime.lastError);
}

// The "Send to" choice made in the panel shows in the menu too.
onSettings((chosen) => {
  for (const id of Object.keys(sending.DESTINATIONS)) {
    chrome.contextMenus.update(`to-${id}`, { checked: chosen.destination === id }, () => void chrome.runtime.lastError);
  }
});

// While recording: Stop in the menu, not Record.
function recordingMenus(now) {
  const update = (id, props) => chrome.contextMenus.update(id, props, () => void chrome.runtime.lastError);
  update('stop', { visible: !!now, title: now?.kind === 'workflow' ? 'Stop recording and save' : MENU.stop });
  update('bug', { visible: !now });
  update('workflow', { visible: !now });
}
recording.watch(recordingMenus);
// Menus outlive this worker; a recording does not (see record.js).
recordingMenus(recording.current());

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
  if (id.startsWith('to-')) {
    await update({ destination: id.slice(3) });
    return;
  }
  if (id === 'panel') {
    if (tab?.windowId !== undefined) await chrome.sidePanel.open({ windowId: tab.windowId }).catch(() => {});
    return;
  }
  if (id === 'stop') {
    const now = recording.current();
    const recorded = now ? await chrome.tabs.get(now.tabId).catch(() => null) : null;
    await act('stop', recorded ?? tab).catch(() => {});
    return;
  }
  if (!tab?.id) return;
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

// ---- The side panel -------------------------------------------------------

// Messages from the side panel: what it shows, and the user's actions.
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id !== chrome.runtime.id) return false;
  // The recorder in a page (its steps, and the bar's buttons).
  if (sender.tab) {
    if (typeof message?.type === 'string' && message.type.startsWith('recorder-')) {
      recording.fromRecorder(message, sender);
      return false;
    }
    // A button of PilotHouse's on GitHub, Jira or Linear.
    if (message?.type === 'site-action') {
      void sites.act(message.action, sender).then(
        (result) => respond({ ok: true, result }),
        (error) => respond({ ok: false, error: String(error?.message ?? error) }),
      );
      return true;
    }
    return false;
  }
  if (message?.target === 'offscreen') return false;
  void panelRequest(message).then(
    (result) => respond({ ok: true, result }),
    (error) => respond({ ok: false, error: String(error?.message ?? error) }),
  );
  return true;
});

async function panelRequest(message) {
  switch (message?.type) {
    case 'status':
      return statusFeature.status();
    case 'act': {
      const tab = await chrome.tabs.get(message.tabId);
      return act(message.action, tab, { streamId: message.streamId, checked: message.checked }, true);
    }
    case 'recording':
      return recording.current();
    // The page's text, for a summary the panel makes on this computer.
    case 'clip': {
      const tab = await chrome.tabs.get(message.tabId);
      return inPage(tab, 'clip.js', () => globalThis.__pilothouse.clip({}));
    }
    case 'deliver': {
      const tab = await chrome.tabs.get(message.tabId);
      return sending.deliver(tab, message.items, message.title);
    }
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
  // panel (no tab) hears what agents do.
  if (port.name !== 'panel' || port.sender?.id !== chrome.runtime.id || port.sender.tab) return;
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
sites.install();

chrome.runtime.onInstalled.addListener(async () => {
  await createMenus();
  await chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch(() => {});
  connect();
});
chrome.runtime.onStartup.addListener(async () => {
  await createMenus();
  connect();
});
connect();
