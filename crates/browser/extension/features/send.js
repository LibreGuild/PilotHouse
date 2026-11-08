// Send to PilotHouse: the selection, the page (as Markdown), a link, an
// image, a screenshot of an area, or an element, from the right-click menu,
// a keyboard shortcut or the side panel. What is sent goes into the
// composer of a chat in PilotHouse (see the panel's "Send to" choice) and
// waits there: nothing reaches an agent until the user sends the message.

import { ask } from '../lib/link.js';
import { base64, crop, dataOf, inMainWorld, inPage, viewportWidth, visible } from '../lib/page.js';
import { settings } from '../lib/settings.js';

export const DESTINATIONS = Object.freeze({
  current: 'the chat on screen',
  new: 'a new chat',
  quick: 'quick chat',
});

/** Sends `items` to PilotHouse; resolves with where they went. */
export async function deliver(tab, items, title) {
  const chosen = await settings();
  const answer = await ask('send', {
    to: chosen.destination,
    bringForward: chosen.bringForward,
    title: title ?? tab?.title ?? '',
    source: { url: tab?.url ?? '', title: tab?.title ?? '' },
    items,
  });
  if (tab?.id !== undefined) void flash(tab.id, true, `Sent to ${answer.where ?? 'PilotHouse'}`);
  return answer;
}

/** A moment of feedback on the toolbar icon of `tabId`. */
export async function flash(tabId, ok, title) {
  const action = chrome.action;
  await action.setBadgeBackgroundColor({ tabId, color: ok ? '#16a34a' : '#dc2626' }).catch(() => {});
  await action.setBadgeText({ tabId, text: ok ? '✓' : '!' }).catch(() => {});
  if (title) await action.setTitle({ tabId, title }).catch(() => {});
  setTimeout(() => {
    // Back to the window-wide badge (chats waiting), set without a tab.
    void action.setBadgeText({ tabId, text: null }).catch(() => {});
    void action.setBadgeBackgroundColor({ tabId, color: null }).catch(() => {});
    void action.setTitle({ tabId, title: null }).catch(() => {});
  }, 2500);
}

/** Tells the user why sending did not work. */
export async function failed(tab, error) {
  const message = String(error?.message ?? error);
  if (tab?.id !== undefined) void flash(tab.id, false, message);
  await chrome.notifications.create(`pilothouse-error-${Date.now()}`, {
    type: 'basic',
    iconUrl: 'icons/icon128.png',
    title: 'PilotHouse',
    message,
    priority: 1,
  }).catch(() => {});
}

function heading(tab) {
  const title = tab.title?.trim();
  return title ? `${title}\n${tab.url}` : tab.url;
}

export async function sendPage(tab) {
  const clip = await inPage(tab, 'clip.js', () => globalThis.__pilothouse.clip({}));
  if (!clip?.markdown) throw new Error('This page has no text to send.');
  const note = clip.truncated ? '\n\n(The page is long: this is its first 200,000 characters.)' : '';
  const description = clip.description ? `\n\n> ${clip.description}` : '';
  return deliver(tab, [{
    kind: 'text',
    label: `Page: ${short(clip.title || tab.title || tab.url)}`,
    text: `${heading(tab)}${description}\n\n${clip.markdown}${note}`,
  }]);
}

/** The selection as Markdown; `fallback` is Chrome's plain text of it
 * (for pages scripts cannot read, such as PDFs). */
export async function sendSelection(tab, fallback) {
  let markdown = '';
  try {
    const clip = await inPage(tab, 'clip.js', () => globalThis.__pilothouse.clip({ selectionOnly: true }));
    markdown = clip?.markdown ?? '';
  } catch (error) {
    if (!fallback) throw error;
  }
  markdown ||= fallback ?? '';
  if (!markdown.trim()) throw new Error('Select some text first.');
  return deliver(tab, [{
    kind: 'text',
    label: `Selection: ${short(tab.title || tab.url)}`,
    text: `From ${heading(tab)}\n\n${markdown}`,
  }]);
}

export async function sendLink(tab, url, text) {
  const words = text?.trim();
  return deliver(tab, [{
    kind: 'text',
    label: `Link: ${short(words || url)}`,
    text: `${words ? `${words}\n` : ''}${url}\n\n(On ${heading(tab)})`,
  }]);
}

/** An image of the page: its file when Chrome lets the extension fetch it,
 * else the image as it is on screen. */
export async function sendImage(tab, src) {
  let png = null;
  try {
    const response = await fetch(src);
    if (response.ok && /^image\//.test(response.headers.get('content-type') ?? '')) {
      png = await toPng(await response.blob());
    }
  } catch (error) {
    // Not fetchable from here (no access to its site): the screen has it.
  }
  if (!png) {
    const rect = await inMainWorld(tab, (wanted) => {
      const img = [...document.images].find((image) => image.currentSrc === wanted || image.src === wanted);
      if (!img) return null;
      img.scrollIntoView({ block: 'nearest', inline: 'nearest' });
      const r = img.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height, viewport: innerWidth };
    }, [src]);
    if (!rect) throw new Error('That image could not be found on the page.');
    png = await crop(await visible(tab), rect, rect.viewport);
  }
  return deliver(tab, [
    { kind: 'image', label: `Image from ${short(tab.title || tab.url)}`, png },
    { kind: 'text', label: 'About the image', text: `Image ${src}\non ${heading(tab)}` },
  ]);
}

/** Converts an image file to PNG (any format Chrome decodes). */
async function toPng(blob) {
  const bitmap = await createImageBitmap(blob);
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  canvas.getContext('2d').drawImage(bitmap, 0, 0);
  bitmap.close();
  return base64(await canvas.convertToBlob({ type: 'image/png' }));
}

/** A screenshot of an area the user drags over (a click: the visible page). */
export async function sendArea(tab) {
  const rect = await inPage(tab, 'area.js', () => globalThis.__pilothouse.area());
  if (!rect) return null;
  const shot = await visible(tab);
  const png = rect.whole ? dataOf(shot) : await crop(shot, rect, await viewportWidth(tab));
  return deliver(tab, [{
    kind: 'image',
    label: rect.whole ? `Screenshot of ${short(tab.title || tab.url)}` : `Area of ${short(tab.title || tab.url)}`,
    png,
  }, {
    kind: 'text',
    label: 'About the screenshot',
    text: `Screenshot of ${heading(tab)}`,
  }]);
}

/** An element the user clicks: its details, and how it looks. */
export async function sendElement(tab) {
  const element = await inPage(tab, 'picker.js', () => globalThis.__pilothouse.pick());
  if (!element) return null;
  const component = await inMainWorld(tab, componentSource, [element.selector]);
  const source = component?.source ?? element.source;
  const items = [];
  try {
    const shot = await visible(tab);
    items.push({
      kind: 'image',
      label: `<${element.tag}> element`,
      png: await crop(shot, element.rect, await viewportWidth(tab), 8),
    });
  } catch (error) {
    // The details alone still help.
  }
  let text = `Element on ${element.url}\nSelector: ${element.selector}\nText: ${element.text}\nStyle: ${element.styles}`;
  if (element.css) text += `\nCSS: ${element.css}`;
  if (component?.component) text += `\nComponent: ${component.component}`;
  if (source) text += `\nSource: ${source}`;
  text += `\n\n\`\`\`html\n${element.html}\n\`\`\``;
  items.push({ kind: 'text', label: `<${element.tag}> element`, text });
  return deliver(tab, items);
}

/** Runs in the page's own world: the component a dev build says the
 * element comes from (React, Vue, Svelte). Reads only. */
function componentSource(selector) {
  let el;
  try {
    el = document.querySelector(selector);
  } catch (error) {
    return null;
  }
  let component = null;
  for (let node = el; node && node.nodeType === 1; node = node.parentElement) {
    try {
      const svelte = node.__svelte_meta;
      if (svelte && svelte.loc) return { source: `${svelte.loc.file}:${svelte.loc.line}`, component };
      const key = Object.keys(node).find((k) => k.startsWith('__reactFiber$'));
      if (key) {
        for (let fiber = node[key]; fiber; fiber = fiber.return) {
          const type = fiber.type;
          if (!component && type && typeof type !== 'string') component = type.displayName || type.name || null;
          const debug = fiber._debugSource;
          if (debug) return { source: `${debug.fileName}:${debug.lineNumber}`, component };
        }
        return { source: null, component };
      }
      const vue = node.__vueParentComponent;
      if (vue && vue.type) {
        return { source: vue.type.__file ?? null, component: vue.type.name ?? vue.type.__name ?? null };
      }
    } catch (error) {
      return { source: null, component };
    }
  }
  return { source: null, component };
}

function short(text) {
  const clean = String(text ?? '').replace(/\s+/g, ' ').trim();
  return clean.length > 60 ? `${clean.slice(0, 57)}…` : clean;
}
