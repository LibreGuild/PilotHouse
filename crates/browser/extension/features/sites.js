// PilotHouse's buttons on GitHub, Jira and Linear (`page/buttons.js`): they
// run only on the sites the user turned on in the panel, and only while
// Chrome has granted the extension those sites (the panel asks when the
// user turns one on; removing the site's access turns it off).

import { ask } from '../lib/link.js';
import { onSettings, settings, update } from '../lib/settings.js';

export const SITES = Object.freeze({
  github: { name: 'GitHub', origins: ['https://github.com/*'] },
  jira: { name: 'Jira', origins: ['https://*.atlassian.net/*'] },
  linear: { name: 'Linear', origins: ['https://linear.app/*'] },
});

const SCRIPT = 'pilothouse-site-buttons';

/** Registers the buttons for the sites that are on and granted. */
async function sync() {
  const chosen = await settings();
  const matches = [];
  for (const [key, site] of Object.entries(SITES)) {
    if (!chosen.sites[key]) continue;
    const granted = await chrome.permissions.contains({ origins: site.origins }).catch(() => false);
    if (granted) matches.push(...site.origins);
    // Access taken back in Chrome: the site is off.
    else await update({ sites: { [key]: false } });
  }
  const known = await chrome.scripting.getRegisteredContentScripts({ ids: [SCRIPT] }).catch(() => []);
  if (known.length) await chrome.scripting.unregisterContentScripts({ ids: [SCRIPT] }).catch(() => {});
  if (!matches.length) return;
  await chrome.scripting.registerContentScripts([{
    id: SCRIPT,
    matches,
    js: ['page/clip.js', 'page/buttons.js'],
    runAt: 'document_idle',
    persistAcrossSessions: true,
  }]).catch(() => {});
}

/** A button pressed on one of those sites: PilotHouse does it. */
export async function act(action, sender) {
  const url = sender.tab?.url ?? '';
  const chosen = await settings();
  const allowed = Object.entries(SITES)
    .some(([key, site]) => chosen.sites[key] && site.origins.some((origin) => matches(origin, url)));
  if (!allowed) throw new Error('PilotHouse’s buttons are not turned on for this site.');
  return ask('siteAction', action);
}

function matches(origin, url) {
  const pattern = new RegExp(`^${origin.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`);
  return pattern.test(url);
}

export function install() {
  onSettings(() => void sync());
  chrome.permissions.onRemoved.addListener(() => void sync());
  chrome.permissions.onAdded.addListener(() => void sync());
  chrome.runtime.onInstalled.addListener(() => void sync());
  chrome.runtime.onStartup.addListener(() => void sync());
}
