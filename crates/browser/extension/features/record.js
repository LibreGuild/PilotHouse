// Recording what the user does in a tab, only when they start it (the
// right-click menu or the side panel), with a bar in the page saying so:
//
// - **A bug report**: the steps, a video of the tab with a few still frames,
//   and the page's console errors and failed requests (read with Chrome's
//   debugger, whose bar shows meanwhile). Stopping sends it to a composer
//   in PilotHouse, for the user to describe and send.
// - **A workflow**: the steps alone, saved in PilotHouse, where agents can
//   run it again and it can become a Playwright test or a schedule.
//
// One recording at a time. Passwords and card numbers are never kept.

import { ask } from '../lib/link.js';
import { inPage, visible, dataOf } from '../lib/page.js';
import { attached, reachable } from '../lib/tabs.js';
import { deliver, flash } from './send.js';

const PROTOCOL = '1.3';
/** The isolated world the recorder runs in on each page, and its line to
 * the extension (a DevTools binding). */
const WORLD = 'pilothouse-recorder';
const BINDING = '__pilothouseRecorder';
const LIMITS = { bug: 3 * 60 * 1000, workflow: 10 * 60 * 1000 };
const LABELS = {
  bug: { label: 'Recording a bug report', stopLabel: 'Stop and send' },
  workflow: { label: 'Recording a workflow', stopLabel: 'Stop and save' },
};
const MAX_STEPS = 500;
const MAX_LOGS = 200;

let recording = null;
const watchers = new Set();

/** The recording under way, for the side panel: kind, tab and start. */
export function current() {
  if (!recording) return null;
  const { kind, tabId, startedAt, steps } = recording;
  return { kind, tabId, startedAt, steps: steps.length };
}

export function watch(watcher) {
  watchers.add(watcher);
  return () => watchers.delete(watcher);
}

function changed() {
  const now = current();
  for (const watcher of watchers) {
    try {
      watcher(now);
    } catch (error) {
      // A closed panel.
    }
  }
}

/** Starts recording `kind` (`bug` or `workflow`) in `tab`. */
export async function start(kind, tab, streamId) {
  if (recording) throw new Error('A recording is already under way. Stop it first.');
  if (!reachable(tab.url)) throw new Error('Chrome does not let extensions record this page.');
  recording = {
    kind,
    tabId: tab.id,
    startedAt: Date.now(),
    startUrl: tab.url,
    title: tab.title ?? '',
    steps: [],
    console: [],
    failed: [],
    requests: new Map(),
    requestCount: 0,
    video: false,
    debugger: false,
    worlds: new Set(),
    pageOnly: false,
    notes: [],
    timer: null,
  };
  try {
    if (kind === 'bug') await startVideo(tab, streamId);
    await watchPage(tab);
  } catch (error) {
    await finish(false).catch(() => {});
    throw error;
  }
  recording.timer = setTimeout(() => {
    recording?.notes.push(`The recording stopped by itself after ${LIMITS[kind] / 60000} minutes.`);
    void finish(true).catch(() => {});
  }, LIMITS[kind]);
  changed();
}

function barOptions() {
  const { label, stopLabel } = LABELS[recording.kind];
  return { label, stopLabel, startedAt: recording.startedAt };
}

/** The recorder on this page and the next ones: through Chrome's debugger
 * (its bar shows while recording), which follows the tab from page to page
 * and reads the console and requests for a bug report. While an agent's
 * debugger is on the tab: this page only, through the extension. */
async function watchPage(tab) {
  const tabId = tab.id;
  if (!attached.has(tabId)) {
    try {
      await chrome.debugger.attach({ tabId }, PROTOCOL);
      recording.debugger = true;
      const send = (method, params = {}) => chrome.debugger.sendCommand({ tabId }, method, params);
      await send('Page.enable');
      await send('Runtime.enable');
      if (recording.kind === 'bug') {
        await send('Log.enable');
        await send('Network.enable');
      }
      await send('Runtime.addBinding', { name: BINDING, executionContextName: WORLD });
      const source = await (await fetch(chrome.runtime.getURL('page/recorder.js'))).text();
      await send('Page.addScriptToEvaluateOnNewDocument', {
        source: `${source}\n;globalThis.__pilothouse.recorder.start(${JSON.stringify(barOptions())});`,
        worldName: WORLD,
        runImmediately: true,
      });
      return;
    } catch (error) {
      if (recording.debugger) await chrome.debugger.detach({ tabId }).catch(() => {});
      recording.debugger = false;
      recording.notes.push(`Chrome’s debugger could not follow the tab (${error?.message ?? error}), so only the first page was recorded.`);
    }
  } else {
    recording.notes.push('An agent is using this tab, so only the first page was recorded, and its console and requests are in that agent’s logs.');
  }
  recording.pageOnly = true;
  await inPage(tab, 'recorder.js', (options) => globalThis.__pilothouse.recorder.start(options), [barOptions()]);
}

/** A video of the tab, when Chrome allows it here. */
async function startVideo(tab, given) {
  try {
    // From the panel, the stream comes with the click (Chrome asks for one).
    const streamId = given ?? await chrome.tabCapture.getMediaStreamId({ targetTabId: tab.id });
    await offscreen();
    const answer = await chrome.runtime.sendMessage({ target: 'offscreen', type: 'start', streamId });
    if (!answer?.ok) throw new Error(answer?.error ?? 'could not record');
    recording.video = true;
  } catch (error) {
    recording.notes.push(`No video: ${String(error?.message ?? error).replace(/\.+$/, '')}.`);
  }
}

async function offscreen() {
  const open = await chrome.runtime.getContexts({ contextTypes: ['OFFSCREEN_DOCUMENT'] });
  if (open.length) return;
  await chrome.offscreen.createDocument({
    url: 'offscreen.html',
    reasons: ['USER_MEDIA'],
    justification: 'Records the tab for a bug report the user asked for.',
  });
}

async function closeOffscreen() {
  const open = await chrome.runtime.getContexts({ contextTypes: ['OFFSCREEN_DOCUMENT'] }).catch(() => []);
  if (open.length) await chrome.offscreen.closeDocument().catch(() => {});
}

/** Stops the recording; `keep` sends (or saves) it, else it is dropped. */
export async function finish(keep) {
  const done = recording;
  if (!done) return null;
  recording = null;
  clearTimeout(done.timer);
  changed();
  const tab = await chrome.tabs.get(done.tabId).catch(() => null);
  const stopping = 'globalThis.__pilothouse?.recorder?.stop()';
  if (done.debugger) {
    for (const contextId of done.worlds) {
      await chrome.debugger.sendCommand({ tabId: done.tabId }, 'Runtime.evaluate', { expression: stopping, contextId }).catch(() => {});
    }
  } else if (tab) {
    await chrome.scripting.executeScript({
      target: { tabId: tab.id, allFrames: true },
      func: () => globalThis.__pilothouse?.recorder?.stop(),
    }).catch(() => {});
  }
  let media = { video: null, frames: [] };
  if (done.video) {
    const answer = await chrome.runtime.sendMessage({ target: 'offscreen', type: 'stop', keep }).catch(() => null);
    if (answer?.ok) media = answer.result;
  }
  await closeOffscreen();
  if (done.debugger) await chrome.debugger.detach({ tabId: done.tabId }).catch(() => {});
  if (!keep) return null;
  if (done.kind === 'workflow') return saveWorkflow(done, tab);
  return sendBug(done, tab, media);
}

async function saveWorkflow(done, tab) {
  if (!done.steps.length) throw new Error('Nothing was recorded: no clicks or typing.');
  const answer = await ask('saveWorkflow', {
    name: done.title || new URL(done.startUrl).hostname,
    startUrl: done.startUrl,
    steps: done.steps,
  });
  if (tab) void flash(tab.id, true, `Saved the workflow “${answer.name}” in PilotHouse`);
  return { where: `PilotHouse’s workflows as “${answer.name}”`, name: answer.name };
}

async function sendBug(done, tab, media) {
  const items = [];
  media.frames.forEach((png, index) => {
    items.push({
      kind: 'image',
      label: index === media.frames.length - 1 ? 'Bug report: the end' : `Bug report: frame ${index + 1}`,
      png,
    });
  });
  if (!media.frames.length && tab?.active) {
    const shot = await visible(tab).catch(() => null);
    if (shot) items.push({ kind: 'image', label: 'Bug report: the page', png: dataOf(shot) });
  }
  if (media.video) items.push({ kind: 'video', label: 'Bug report: video', webm: media.video });
  items.push({ kind: 'text', label: `Bug report: ${short(done.title || done.startUrl)}`, text: report(done, tab) });
  return deliver(tab ?? { url: done.startUrl, title: done.title }, items, done.title);
}

function report(done, tab) {
  const seconds = Math.round((Date.now() - done.startedAt) / 1000);
  const lines = [
    'Bug report recorded in Chrome',
    `Page: ${tab?.title || done.title}`,
    tab?.url ?? done.startUrl,
    `Recorded for ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')} on ${new Date(done.startedAt).toLocaleString()}.`,
    '',
    'What I did:',
    ...(done.steps.length ? done.steps.map((step, index) => `${index + 1}. ${describe(step)}`) : ['(no clicks or typing)']),
  ];
  if (done.debugger) {
    lines.push('', `Console errors and warnings (${done.console.length}):`);
    lines.push(...(done.console.length ? done.console.map((entry) => `- ${entry}`) : ['(none)']));
    lines.push('', `Failed requests (${done.failed.length} of ${done.requestCount}):`);
    lines.push(...(done.failed.length ? done.failed.map((entry) => `- ${entry}`) : ['(none)']));
  }
  if (done.notes.length) lines.push('', ...done.notes);
  return lines.join('\n');
}

/** A step as words: `Clicked “Save” (button#save)`. */
export function describe(step) {
  const what = step.text ? `“${step.text}”` : 'an element';
  switch (step.type) {
    case 'click':
      return `Clicked ${what} (${step.selector})`;
    case 'fill':
      return step.secret ? `Typed a password or secret into ${what}` : `Typed “${step.value}” into ${what}`;
    case 'select':
      return `Chose “${step.value}” in ${what}`;
    case 'press':
      return `Pressed ${step.key}`;
    case 'upload':
      return `Chose file(s) ${step.files.join(', ')} for ${what}`;
    case 'navigate':
      return `Went to ${step.url}`;
    default:
      return step.type;
  }
}

/** A message from the page's recorder (a step, Stop or Cancel). */
export function fromRecorder(message, sender) {
  if (!recording || sender.tab?.id !== recording.tabId) return;
  if (message?.type === 'recorder-step') step(message.step, sender);
  else if (message?.type === 'recorder-stop') void finishAndTell(true);
  else if (message?.type === 'recorder-cancel') void finishAndTell(false);
}

/** Set by the worker: what to do when the bar's buttons are used. */
let finishAndTell = (keep) => finish(keep);

export function onBarFinish(handler) {
  finishAndTell = handler;
}

/** A step from the page's recorder. */
function step(step, sender) {
  if (!recording || sender.tab?.id !== recording.tabId || typeof step?.type !== 'string') return;
  const steps = recording.steps;
  // Enter in a field commits it, and its change follows: once is enough.
  if (step.type === 'fill' && steps.slice(-3).some((before) => before.type === 'fill'
      && before.selector === step.selector && (before.value ?? null) === (step.value ?? null))) return;
  if (steps.length >= MAX_STEPS) return;
  steps.push(sanitize(step));
  changed();
}

function sanitize(step) {
  const keep = {};
  for (const key of ['type', 'selector', 'text', 'tag', 'value', 'secret', 'key', 'url', 'at']) {
    if (step[key] !== undefined && step[key] !== null) keep[key] = typeof step[key] === 'string' ? step[key].slice(0, 2000) : step[key];
  }
  if (Array.isArray(step.files)) keep.files = step.files.slice(0, 20).map((name) => String(name).slice(0, 200));
  return keep;
}

export function install() {
  // A new page in the recorded tab: the bar comes back, and the move is a step.
  chrome.tabs.onUpdated.addListener((tabId, change) => {
    if (!recording || tabId !== recording.tabId) return;
    if (change.url && change.url !== recording.lastUrl) {
      recording.lastUrl = change.url;
      const steps = recording.steps;
      if (steps.length && steps.length < MAX_STEPS) steps.push({ type: 'navigate', url: change.url, at: Date.now() });
      changed();
    }
  });
  chrome.tabs.onRemoved.addListener((tabId) => {
    if (recording?.tabId === tabId) void finish(true).catch(() => {});
  });
  chrome.debugger.onEvent.addListener((source, method, params) => {
    if (!recording?.debugger || source.tabId !== recording.tabId || source.sessionId) return;
    if (method === 'Runtime.executionContextCreated' && params?.context?.name === WORLD) {
      recording.worlds.add(params.context.id);
    } else if (method === 'Runtime.executionContextDestroyed') {
      recording.worlds.delete(params?.executionContextId);
    } else if (method === 'Runtime.bindingCalled' && params?.name === BINDING) {
      let message;
      try {
        message = JSON.parse(params.payload);
      } catch (error) {
        return;
      }
      fromRecorder(message, { tab: { id: source.tabId } });
    } else {
      log(method, params ?? {});
    }
  });
  chrome.debugger.onDetach.addListener((source, reason) => {
    if (!recording?.debugger || source.tabId !== recording.tabId) return;
    recording.debugger = false;
    if (reason === 'canceled_by_user') recording.notes.push('The console and requests stopped when Chrome’s debugging bar was closed.');
  });
}

function log(method, params) {
  const add = (list, text) => {
    if (list.length < MAX_LOGS) list.push(text.slice(0, 600));
  };
  switch (method) {
    case 'Runtime.consoleAPICalled': {
      if (!['error', 'warning', 'assert'].includes(params.type)) return;
      const text = (params.args ?? []).map((arg) => arg.value ?? arg.description ?? '').join(' ');
      add(recording.console, `[${params.type}] ${text}${where(params.stackTrace?.callFrames?.[0])}`);
      return;
    }
    case 'Runtime.exceptionThrown': {
      const details = params.exceptionDetails ?? {};
      const text = details.exception?.description ?? details.text ?? 'Uncaught error';
      add(recording.console, `[uncaught] ${text.split('\n')[0]}${where(details.stackTrace?.callFrames?.[0] ?? details)}`);
      return;
    }
    case 'Log.entryAdded': {
      const entry = params.entry ?? {};
      if (entry.level !== 'error' || entry.source === 'network') return;
      add(recording.console, `[${entry.source}] ${entry.text}`);
      return;
    }
    case 'Network.requestWillBeSent':
      recording.requestCount += 1;
      if (recording.requests.size < 2000) {
        recording.requests.set(params.requestId, { method: params.request?.method ?? 'GET', url: redact(params.request?.url ?? '') });
      }
      return;
    case 'Network.responseReceived': {
      const status = params.response?.status ?? 0;
      if (status < 400) return;
      const request = recording.requests.get(params.requestId);
      add(recording.failed, `${request?.method ?? 'GET'} ${request?.url ?? redact(params.response?.url ?? '')} → ${status} ${params.response?.statusText ?? ''}`.trim());
      return;
    }
    case 'Network.loadingFailed': {
      if (params.canceled) return;
      const request = recording.requests.get(params.requestId);
      add(recording.failed, `${request?.method ?? 'GET'} ${request?.url ?? '(a request)'} → ${params.errorText ?? 'failed'}`);
      return;
    }
    default:
  }
}

function where(frame) {
  if (!frame?.url) return '';
  return ` (${frame.url.split('/').slice(-1)[0] || frame.url}:${(frame.lineNumber ?? 0) + 1}:${(frame.columnNumber ?? 0) + 1})`;
}

/** An address without the values of parameters that look like secrets. */
function redact(url) {
  try {
    const parsed = new URL(url);
    for (const key of [...parsed.searchParams.keys()]) {
      if (/token|secret|key|pass|session|auth|code|sig/i.test(key)) parsed.searchParams.set(key, '••••');
    }
    return parsed.href.slice(0, 500);
  } catch (error) {
    return String(url).slice(0, 500);
  }
}

function short(text) {
  const clean = String(text ?? '').replace(/\s+/g, ' ').trim();
  return clean.length > 50 ? `${clean.slice(0, 47)}…` : clean;
}
