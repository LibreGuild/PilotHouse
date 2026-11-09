// Reading a tab the user pointed at (a menu, a shortcut, the side panel):
// the page scripts in `page/`, run in the extension's isolated world, and
// screenshots of the visible part of the tab.

import { reachable } from './tabs.js';

/** What the user is told when Chrome keeps a page out of reach. */
export const OUT_OF_REACH = 'Chrome does not let extensions read this page (its own pages, the Web Store and other extensions’ pages).';

/** Runs `page/<file>` in the tab's isolated world, then `call(...args)` there. */
export async function inPage(tab, file, call, args = []) {
  if (!reachable(tab.url)) throw new Error(OUT_OF_REACH);
  try {
    await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: [`page/${file}`] });
    const [result] = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: call,
      args,
    });
    return result?.result;
  } catch (error) {
    throw new Error(access(error));
  }
}

/** Runs `func(...args)` in the page's own world (to read what its scripts
 * keep, such as a dev build's component sources). Only reads. */
export async function inMainWorld(tab, func, args = []) {
  try {
    const [result] = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: 'MAIN',
      func,
      args,
    });
    return result?.result;
  } catch (error) {
    return undefined;
  }
}

function access(error) {
  const message = String(error?.message ?? error);
  if (/Cannot access|cannot be scripted|permission/i.test(message)) {
    return 'PilotHouse may not read this page yet. Use the PilotHouse menu (right-click) or its shortcut on the page, or allow this site in the PilotHouse panel.';
  }
  return message;
}

/** Whether Chrome refused a screenshot or a tab's video because the user did
 * not ask from the page itself (its menu or a shortcut grant that; the side
 * panel and a site's access do not). */
export function notInvoked(error) {
  return /activeTab|all_urls|not been invoked/i.test(String(error?.message ?? error));
}

/** The visible part of the tab as a PNG data URL (the tab must be in front). */
export async function visible(tab) {
  if (!reachable(tab.url)) throw new Error(OUT_OF_REACH);
  try {
    return await chrome.tabs.captureVisibleTab(tab.windowId, { format: 'png' });
  } catch (error) {
    if (notInvoked(error)) {
      throw new Error('Chrome lets PilotHouse take screenshots only when you ask from the page: press Alt+Shift+S (unless you changed the shortcut) or right-click the page and use the PilotHouse menu.');
    }
    throw new Error(access(error));
  }
}

/** `dataUrl` (a screenshot of a viewport `viewport` CSS pixels wide) cut to
 * `rect` (CSS pixels), with `pad` pixels around it; base64 PNG. */
export async function crop(dataUrl, rect, viewport, pad = 0) {
  const bitmap = await createImageBitmap(await (await fetch(dataUrl)).blob());
  const scale = viewport > 0 ? bitmap.width / viewport : 1;
  const x = Math.max(0, Math.floor((rect.x - pad) * scale));
  const y = Math.max(0, Math.floor((rect.y - pad) * scale));
  const width = Math.min(bitmap.width - x, Math.ceil((rect.width + pad * 2) * scale));
  const height = Math.min(bitmap.height - y, Math.ceil((rect.height + pad * 2) * scale));
  if (width < 1 || height < 1) throw new Error('That area is outside the visible page.');
  const canvas = new OffscreenCanvas(width, height);
  canvas.getContext('2d').drawImage(bitmap, x, y, width, height, 0, 0, width, height);
  bitmap.close();
  return base64(await canvas.convertToBlob({ type: 'image/png' }));
}

/** A data URL's base64 part. */
export function dataOf(dataUrl) {
  return dataUrl.slice(dataUrl.indexOf(',') + 1);
}

export async function base64(blob) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let text = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(text);
}

/** The viewport's width in CSS pixels. */
export async function viewportWidth(tab) {
  const [result] = await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: () => innerWidth,
  });
  return result?.result ?? 0;
}
