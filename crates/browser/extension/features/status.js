// What PilotHouse says about its agents, shown in Chrome: the count of chats
// waiting for you on the toolbar icon, Chrome notifications (when chosen in
// the panel), and handing a tab to the user when an agent needs them in it
// (to sign in, or to pass a check that a person is there).

import { onAppNote, onConnection, send } from '../lib/link.js';
import { onSettings, settings } from '../lib/settings.js';

const CHAT = 'pilothouse-chat|';
const HANDOFF = 'pilothouse-handoff|';

/** The latest from the app; side panels are told of each change. */
let latest = { connected: false, waiting: [], working: [], agents: [] };
const watchers = new Set();

export function status() {
  return latest;
}

/** Calls `watcher(status)` on each change; returns a function to stop. */
export function watch(watcher) {
  watchers.add(watcher);
  return () => watchers.delete(watcher);
}

function changed(next) {
  latest = { ...latest, ...next };
  void chrome.storage.session.set({ status: latest }).catch(() => {});
  void badge();
  for (const watcher of watchers) {
    try {
      watcher(latest);
    } catch (error) {
      // A closed panel.
    }
  }
}

async function badge() {
  const chosen = await settings();
  const waiting = latest.connected ? latest.waiting.length : 0;
  const action = chrome.action;
  if (!chosen.badge || waiting === 0) {
    await action.setBadgeText({ text: '' }).catch(() => {});
    return;
  }
  await action.setBadgeBackgroundColor({ color: '#d97706' }).catch(() => {});
  await action.setBadgeText({ text: waiting > 99 ? '99+' : String(waiting) }).catch(() => {});
  await action.setTitle({
    title: waiting === 1 ? 'A chat is waiting for you in PilotHouse' : `${waiting} chats are waiting for you in PilotHouse`,
  }).catch(() => {});
}

export function install() {
  // Kept across this worker's restarts, so the panel shows it at once.
  void chrome.storage.session.get('status').then(({ status: kept }) => {
    if (kept) latest = { ...kept, connected: false };
  }).catch(() => {});

  onConnection((connected) => changed(connected ? { connected } : { connected, waiting: [], working: [], agents: [] }));
  onSettings(() => void badge());

  onAppNote('PilotHouse.status', (params) => {
    changed({
      waiting: list(params.waiting),
      working: list(params.working),
      agents: list(params.agents),
    });
  });

  onAppNote('PilotHouse.news', async (params) => {
    if (!(await settings()).notifyNews || typeof params.tag !== 'string') return;
    await chrome.notifications.create(`${CHAT}${params.tag}`, {
      type: 'basic',
      iconUrl: 'icons/icon128.png',
      title: text(params.title, 120) || 'PilotHouse',
      message: text(params.body, 300),
      priority: params.kind === 'needs' ? 2 : 0,
    }).catch(() => {});
  });

  onAppNote('PilotHouse.handoff', async (params) => {
    const tabId = params.tabId;
    if (!Number.isSafeInteger(tabId)) return;
    const tab = await chrome.tabs.get(tabId).catch(() => null);
    if (!tab) return;
    // The tab comes to the front of its window, and the window asks for
    // attention without taking the keyboard from what the user is doing.
    await chrome.tabs.update(tabId, { active: true }).catch(() => {});
    await chrome.windows.update(tab.windowId, { drawAttention: true }).catch(() => {});
    if (!(await settings()).notifyHandoff) return;
    const agent = text(params.agent, 60) || 'An agent';
    await chrome.notifications.create(`${HANDOFF}${tabId}`, {
      type: 'basic',
      iconUrl: 'icons/icon128.png',
      title: `${agent} needs you in Chrome`,
      message: text(params.reason, 300) || 'Finish this step in the tab, then tell the agent to go on.',
      priority: 2,
      requireInteraction: true,
    }).catch(() => {});
  });

  chrome.notifications.onClicked.addListener(async (id) => {
    await chrome.notifications.clear(id).catch(() => {});
    if (id.startsWith(CHAT)) {
      try {
        send('PilotHouse.openChat', { tag: id.slice(CHAT.length) });
      } catch (error) {
        // PilotHouse closed meanwhile.
      }
    } else if (id.startsWith(HANDOFF)) {
      const tab = await chrome.tabs.get(Number(id.slice(HANDOFF.length))).catch(() => null);
      if (!tab) return;
      await chrome.tabs.update(tab.id, { active: true }).catch(() => {});
      await chrome.windows.update(tab.windowId, { focused: true }).catch(() => {});
    }
  });
}

function list(value) {
  return Array.isArray(value) ? value.slice(0, 50) : [];
}

function text(value, max) {
  return typeof value === 'string' ? value.slice(0, max) : '';
}
