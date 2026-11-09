// PilotHouse's buttons on GitHub, Jira and Linear (`page/buttons.js`): they
// run only on the sites the user turned on in the panel, and only while
// Chrome has granted the extension those sites (the panel asks when the
// user turns one on; removing the site's access turns it off).

import { ask, isObject } from '../lib/link.js';
import { onSettings, settings, update } from '../lib/settings.js';

/** `origins` are what Chrome grants and runs the buttons on; `host` says
 * whether a page's host is the site, matched exactly (a pattern over the
 * whole address would take `https://evil.example/a.atlassian.net/`). */
export const SITES = Object.freeze({
  github: { name: 'GitHub', origins: ['https://github.com/*'], host: (host) => host === 'github.com' },
  jira: { name: 'Jira', origins: ['https://*.atlassian.net/*'], host: (host) => host.endsWith('.atlassian.net') },
  linear: { name: 'Linear', origins: ['https://linear.app/*'], host: (host) => host === 'linear.app' },
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

/** A button pressed on one of those sites: PilotHouse does it. The page's
 * address is the one Chrome gives for the frame that sent it, not the one
 * the message says. */
export async function act(action, sender) {
  let url = null;
  try {
    url = new URL(sender.url ?? '');
  } catch (error) {
    // No address: no site.
  }
  const chosen = await settings();
  const site = url?.protocol === 'https:'
    ? Object.keys(SITES).find((key) => chosen.sites[key] && SITES[key].host(url.hostname))
    : undefined;
  if (!site || !isObject(action)) throw new Error('PilotHouse’s buttons are not turned on for this site.');
  const given = { ...action, url: url.href, host: url.hostname };
  if (given.kind === 'review') {
    const pull = site === 'github' ? /^\/([^/]+)\/([^/]+)\/pull\/(\d+)/.exec(url.pathname) : null;
    if (!pull) throw new Error('This page is not a pull request.');
    Object.assign(given, { owner: pull[1], repo: pull[2], number: Number(pull[3]) });
  }
  return ask('siteAction', given);
}

export function install() {
  onSettings(() => void sync());
  chrome.permissions.onRemoved.addListener(() => void sync());
  chrome.permissions.onAdded.addListener(() => void sync());
  chrome.runtime.onInstalled.addListener(() => void sync());
  chrome.runtime.onStartup.addListener(() => void sync());
}
