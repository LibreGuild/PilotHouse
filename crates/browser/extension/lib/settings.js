// The extension's own choices, kept in this browser (chrome.storage.local).
// PilotHouse's choices are in the app's Settings.

export const DEFAULTS = Object.freeze({
  /** Where what you send goes: `current` (the chat on screen in PilotHouse),
   * `new` (a new chat in that workspace) or `quick` (quick chat). */
  destination: 'current',
  /** PilotHouse comes forward when you send something. */
  bringForward: true,
  /** The toolbar icon counts chats waiting for you. */
  badge: true,
  /** A Chrome notification when a chat finishes or waits for you
   * (PilotHouse already posts system notifications: off by default). */
  notifyNews: false,
  /** A Chrome notification when an agent needs you in one of its tabs (to
   * sign in, or a check that a person is there). */
  notifyHandoff: true,
  /** Buttons on these sites' pages (each asks Chrome for access first). */
  sites: Object.freeze({ github: false, jira: false, linear: false }),
});

export async function settings() {
  const stored = (await chrome.storage.local.get('settings')).settings ?? {};
  return {
    ...DEFAULTS,
    ...stored,
    sites: { ...DEFAULTS.sites, ...(stored.sites ?? {}) },
  };
}

export async function update(change) {
  const now = await settings();
  const next = { ...now, ...change, sites: { ...now.sites, ...(change.sites ?? {}) } };
  await chrome.storage.local.set({ settings: next });
  return next;
}

/** Calls `listener(settings)` when they change. */
export function onSettings(listener) {
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.settings) void settings().then(listener);
  });
}
