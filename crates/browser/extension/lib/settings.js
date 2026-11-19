// The extension's own choices, kept in this browser (chrome.storage.local).
// PilotHouse's choices are in the app's Settings.

export const DEFAULTS = Object.freeze({
  /** Where what you send goes: `current` (the chat on screen in PilotHouse),
   * `new` (a new chat in that workspace) or `quick` (quick chat). */
  destination: 'current',
  /** PilotHouse comes forward when you send something. */
  bringForward: true,
});

/** The choices; what an earlier version kept besides them is left out. */
export async function settings() {
  const stored = (await chrome.storage.local.get('settings')).settings ?? {};
  const chosen = { ...DEFAULTS };
  for (const key of Object.keys(DEFAULTS)) {
    if (typeof stored[key] === typeof DEFAULTS[key]) chosen[key] = stored[key];
  }
  return chosen;
}

export async function update(change) {
  const next = { ...(await settings()) };
  for (const key of Object.keys(DEFAULTS)) {
    if (typeof change[key] === typeof DEFAULTS[key]) next[key] = change[key];
  }
  await chrome.storage.local.set({ settings: next });
  return next;
}
