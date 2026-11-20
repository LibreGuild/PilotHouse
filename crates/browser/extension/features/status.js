// What PilotHouse says about its agents, shown in Chrome: the count of chats
// waiting for you on the toolbar icon, and handing a tab to the user when an
// agent needs them in it (to sign in, or to pass a check that a person is
// there). PilotHouse itself tells of chats that finish: Chrome does not say
// it a second time.

import { onAppNote, onConnection } from '../lib/link.js';

const HANDOFF = 'pilothouse-handoff|';

/** The latest from the app; an open popup is told of each change. */
let latest = { connected: false, busy: false, waiting: [], working: [], agents: [] };
/** Fields set since this worker started, which what was kept must not undo. */
const live = new Set();
const watchers = new Set();
const BADGE_COLOR = '#d97706';

export function status() {
  return latest;
}

/** Calls `watcher(status)` on each change; returns a function to stop. */
export function watch(watcher) {
  watchers.add(watcher);
  return () => watchers.delete(watcher);
}

function changed(next) {
  for (const key of Object.keys(next)) live.add(key);
  latest = { ...latest, ...next };
  void chrome.storage.session.set({ status: latest }).catch(() => {});
  void badge();
  for (const watcher of watchers) {
    try {
      watcher(latest);
    } catch (error) {
      // A closed popup.
    }
  }
}

async function badge() {
  const waiting = latest.connected ? latest.waiting.length : 0;
  const action = chrome.action;
  // Set even with no badge: a tab's own colour is put back to this one.
  await action.setBadgeBackgroundColor({ color: BADGE_COLOR }).catch(() => {});
  if (waiting === 0) {
    await action.setBadgeText({ text: '' }).catch(() => {});
    await action.setTitle({ title: chrome.runtime.getManifest().action?.default_title ?? '' }).catch(() => {});
    return;
  }
  await action.setBadgeText({ text: waiting > 99 ? '99+' : String(waiting) }).catch(() => {});
  await action.setTitle({
    title: waiting === 1 ? 'A chat is waiting for you in PilotHouse' : `${waiting} chats are waiting for you in PilotHouse`,
  }).catch(() => {});
}

export function install() {
  // Kept across this worker's restarts, so the popup shows it at once; read
  // after the connection may already have been made, so what is known by
  // now wins (and `connected` is never taken from before the restart).
  void chrome.storage.session.get('status').then(({ status: kept }) => {
    if (!kept) return;
    const restored = Object.fromEntries(Object.entries(kept)
      .filter(([key]) => key !== 'connected' && key !== 'busy' && !live.has(key) && key in latest));
    if (Object.keys(restored).length) changed(restored);
  }).catch(() => {});
  void badge();

  onConnection((connected, busy) => changed(connected
    ? { connected, busy: false }
    : { connected, busy: !!busy, waiting: [], working: [], agents: [] }));

  onAppNote('PilotHouse.status', (params) => {
    changed({
      waiting: list(params.waiting),
      working: list(params.working),
      agents: list(params.agents),
    });
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
    if (!id.startsWith(HANDOFF)) return;
    await chrome.notifications.clear(id).catch(() => {});
    const tab = await chrome.tabs.get(Number(id.slice(HANDOFF.length))).catch(() => null);
    if (!tab) return;
    await chrome.tabs.update(tab.id, { active: true }).catch(() => {});
    await chrome.windows.update(tab.windowId, { focused: true }).catch(() => {});
  });
}

function list(value) {
  return Array.isArray(value) ? value.slice(0, 50) : [];
}

function text(value, max) {
  return typeof value === 'string' ? value.slice(0, max) : '';
}
