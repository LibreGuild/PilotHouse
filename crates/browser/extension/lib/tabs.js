// The PilotHouse tab group: the only tabs agents may list, read or drive.
// A tab is in reach when it is in a group named "PilotHouse", or is a popup
// window one of those tabs opened (popups cannot be grouped).

export const GROUP_TITLE = 'PilotHouse';
const NO_GROUP = -1;

/** Tabs the debugger is attached to for agents, by this extension. */
export const attached = new Set();
/** Tabs of popup windows (which cannot be grouped) opened by an allowed tab. */
export const popups = new Set();
/** Allowed tabs PilotHouse knows of. */
export const known = new Set();

/** The tab, when PilotHouse may use it: in a PilotHouse group, or a popup one opened. */
export async function allowed(tabId) {
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

export async function inGroup(tab) {
  if (tab.groupId === undefined || tab.groupId === NO_GROUP) return false;
  try {
    const group = await chrome.tabGroups.get(tab.groupId);
    return group.title === GROUP_TITLE;
  } catch (error) {
    return false;
  }
}

export async function groups() {
  return chrome.tabGroups.query({ title: GROUP_TITLE });
}

export async function allowedTabs() {
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
export async function openTab(url) {
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

/** Shares `tab` with agents (it joins the group) or stops sharing it. */
export async function setShared(tab, share) {
  if ((await inGroup(tab)) === share) return;
  if (!share) {
    await chrome.tabs.ungroup(tab.id);
    return;
  }
  const all = await groups();
  const here = all.find((group) => group.windowId === tab.windowId);
  if (here) {
    await chrome.tabs.group({ groupId: here.id, tabIds: [tab.id] });
  } else {
    await newGroup(tab.id);
  }
}

/** The tab in front of the window the user was last in. */
export async function frontTab() {
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  return tab ?? null;
}

/** Whether the extension can read or change `url` at all (Chrome's own
 * pages, the Web Store and other extensions' pages are out of reach). */
export function reachable(url) {
  if (!url) return false;
  try {
    const { protocol, hostname } = new URL(url);
    if (protocol === 'file:') return true;
    if (protocol !== 'http:' && protocol !== 'https:') return false;
    return hostname !== 'chromewebstore.google.com' && hostname !== 'chrome.google.com';
  } catch (error) {
    return false;
  }
}
