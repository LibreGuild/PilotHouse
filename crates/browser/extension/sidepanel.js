// The PilotHouse side panel: what waits for the user and what agents do in
// Chrome, the tab in front (share it; send from it), and the extension's
// choices. Built with DOM calls only (no innerHTML).

import { settings, update } from './lib/settings.js';
import { reachable } from './lib/tabs.js';

const $ = (id) => document.getElementById(id);
const PAGE_ACTIONS = new Set(['page', 'selection', 'area', 'element']);
const RECORDINGS = { bug: 'Recording a bug report', workflow: 'Recording a workflow' };

let windowId = null;
let tab = null;
let status = { connected: false, waiting: [], working: [], agents: [] };
let recording = null;
let recordingTimer = null;

function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === 'class') node.className = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (key in node) node[key] = value;
    else node.setAttribute(key, value);
  }
  for (const child of children) if (child !== null && child !== undefined) node.append(child);
  return node;
}

async function request(message) {
  const answer = await chrome.runtime.sendMessage(message);
  if (!answer?.ok) throw new Error(answer?.error ?? 'failed');
  return answer.result;
}

function say(text, kind = '') {
  const result = $('result');
  result.textContent = text;
  result.className = `result ${kind}`;
}

// ---- Status ---------------------------------------------------------------

function renderStatus() {
  const on = status.connected;
  $('connection').textContent = on ? 'Connected' : 'Not connected';
  $('connection').classList.toggle('on', on);
  $('offline').hidden = on;

  const waiting = on ? status.waiting : [];
  $('waiting-section').hidden = waiting.length === 0;
  $('waiting').replaceChildren(...waiting.map((chat) => el('li', {
    class: 'item waiting',
    tabIndex: 0,
    role: 'button',
    title: 'Open this chat in PilotHouse',
    onclick: () => openChat(chat.tag),
    onkeydown: (event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        openChat(chat.tag);
      }
    },
  }, el('div', { class: 'grow' },
    el('div', { class: 'title' }, chat.title || 'A chat'),
    el('div', { class: 'sub' }, [chat.workspace, chat.line].filter(Boolean).join(' · '))))));

  const agents = on ? status.agents : [];
  $('no-agents').hidden = agents.length > 0;
  $('agents').replaceChildren(...agents.map((agent) => el('li', { class: 'item' },
    el('div', { class: 'grow' },
      el('div', { class: 'row' },
        el('span', { class: 'title' }, agent.label),
        el('span', { class: `state ${agent.paused ? 'paused' : ''}` }, agent.paused ? 'Stopped' : 'Can act')),
      el('ul', { class: 'tabs' }, ...(agent.tabs ?? []).map((agentTab) => el('li', {},
        el('button', {
          title: agentTab.url,
          onclick: () => void focusTab(agentTab.tabId),
        }, agentTab.title || agentTab.url || 'A tab'))))),
    el('button', {
      class: agent.paused ? 'primary' : '',
      'aria-label': agent.paused ? `Let ${agent.label} continue` : `Stop ${agent.label}`,
      onclick: () => void request({ type: 'agent', owner: agent.owner, paused: !agent.paused }).catch((error) => say(error.message, 'error')),
    }, agent.paused ? 'Continue' : 'Stop'))));
}

function renderRecording() {
  const box = $('recording');
  clearInterval(recordingTimer);
  box.hidden = !recording;
  for (const button of document.querySelectorAll('[data-action=bug], [data-action=workflow]')) {
    button.hidden = !!recording;
  }
  if (!recording) return;
  $('recording-title').textContent = RECORDINGS[recording.kind] ?? 'Recording';
  $('recording-stop').textContent = recording.kind === 'workflow' ? 'Stop and save' : 'Stop and send';
  const tick = () => {
    const seconds = Math.max(0, Math.floor((Date.now() - recording.startedAt) / 1000));
    const steps = recording.steps === 1 ? '1 step' : `${recording.steps} steps`;
    $('recording-sub').textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')} · ${steps}`;
  };
  tick();
  recordingTimer = setInterval(tick, 1000);
}

async function finishRecording(keep) {
  if (!recording) return;
  say(keep ? (recording.kind === 'workflow' ? 'Saving…' : 'Sending…') : '');
  try {
    const result = await request({ type: 'act', action: keep ? 'stop' : 'cancel', tabId: recording.tabId });
    if (keep) say(result?.where ? `Sent to ${result.where}.` : 'Done.', 'ok');
    else say('Recording cancelled.');
  } catch (error) {
    say(error.message, 'error');
  }
}

function openChat(tag) {
  void request({ type: 'openChat', tag }).catch((error) => say(error.message, 'error'));
}

async function focusTab(tabId) {
  if (!Number.isSafeInteger(tabId)) return;
  const target = await chrome.tabs.get(tabId).catch(() => null);
  if (!target) return;
  await chrome.tabs.update(tabId, { active: true });
  await chrome.windows.update(target.windowId, { focused: true });
}

// ---- The tab in front -----------------------------------------------------

async function refreshTab() {
  [tab] = await chrome.tabs.query({ active: true, windowId });
  const name = $('tab-name');
  const share = $('share');
  if (!tab) {
    name.textContent = '';
    share.disabled = true;
    return;
  }
  name.textContent = tab.title || tab.url || '';
  name.title = tab.url ?? '';
  const group = tab.groupId !== undefined && tab.groupId !== -1
    ? await chrome.tabGroups.get(tab.groupId).catch(() => null)
    : null;
  share.checked = group?.title === 'PilotHouse';
  share.disabled = !reachable(tab.url);
  const readable = reachable(tab.url);
  for (const button of document.querySelectorAll('.actions button')) button.disabled = !readable;
  if (!readable) say('Chrome keeps this page out of reach of extensions.');
  else if ($('result').className === 'result') say('');
}

/** The site pattern the panel needs access to for `url`. */
function siteOf(url) {
  try {
    const { protocol, host } = new URL(url);
    if (protocol === 'file:') return 'file:///*';
    return `${protocol}//${host}/*`;
  } catch (error) {
    return null;
  }
}

async function act(action) {
  if (!tab) return;
  say('');
  if (PAGE_ACTIONS.has(action)) {
    const origin = siteOf(tab.url);
    // Asked here, in the click: Chrome shows its prompt for this site once.
    const granted = origin && await chrome.permissions.request({ origins: [origin] }).catch(() => false);
    if (!granted) {
      say('PilotHouse needs your permission to read this site. You can also right-click the page and use the PilotHouse menu.', 'error');
      return;
    }
    void renderSites();
  }
  if (action === 'bug' || action === 'workflow') {
    let streamId;
    if (action === 'bug') {
      // Asked in the click, as Chrome wants for recording a tab.
      streamId = await chrome.tabCapture.getMediaStreamId({ targetTabId: tab.id }).catch(() => undefined);
    }
    try {
      await request({ type: 'act', action, tabId: tab.id, streamId });
      say(action === 'bug'
        ? 'Recording. Do what goes wrong, then press Stop and send.'
        : 'Recording. Do the steps, then press Stop and save.');
    } catch (error) {
      say(error.message, 'error');
    }
    return;
  }
  if (action === 'area' || action === 'element') {
    say(action === 'area' ? 'Drag over the page (Esc cancels)…' : 'Click an element on the page (Esc cancels)…');
  } else {
    say('Sending…');
  }
  try {
    const result = await request({ type: 'act', action, tabId: tab.id });
    if (result === null || result === undefined) say('Cancelled.');
    else say(result.where ? `Sent to ${result.where}.` : 'Sent to PilotHouse.', 'ok');
  } catch (error) {
    say(error.message, 'error');
  }
}

// ---- Options --------------------------------------------------------------

async function renderSettings() {
  const chosen = await settings();
  for (const radio of document.querySelectorAll('input[name=destination]')) {
    radio.checked = radio.value === chosen.destination;
  }
  for (const key of ['bringForward', 'badge', 'notifyNews', 'notifyHandoff']) $(key).checked = !!chosen[key];
}

async function renderSites() {
  const { origins = [] } = await chrome.permissions.getAll();
  const sites = origins.filter((origin) => origin !== '<all_urls>');
  $('no-sites').hidden = sites.length > 0;
  $('sites').replaceChildren(...sites.map((origin) => el('li', { class: 'row' },
    el('span', { class: 'sub' }, origin.replace(/\/\*$/, '')),
    el('button', {
      class: 'link',
      'aria-label': `Stop letting the panel read ${origin}`,
      onclick: async () => {
        await chrome.permissions.remove({ origins: [origin] }).catch(() => false);
        void renderSites();
      },
    }, 'Remove'))));
}

async function renderShortcuts() {
  const commands = await chrome.commands.getAll().catch(() => []);
  const names = {
    _execute_action: 'Open this panel',
    'send-to-pilothouse': 'Send the selection, or the page',
    'screenshot-area': 'Screenshot an area',
    'pick-element': 'Pick an element',
  };
  $('shortcuts').replaceChildren(...commands.filter((command) => names[command.name]).map((command) =>
    el('li', { class: 'row' },
      el('span', {}, names[command.name]),
      command.shortcut ? el('kbd', {}, command.shortcut) : el('span', { class: 'sub' }, 'Not set'))));
}

// ---- Start ----------------------------------------------------------------

async function start() {
  windowId = (await chrome.windows.getCurrent()).id;
  const port = chrome.runtime.connect({ name: 'panel' });
  port.onMessage.addListener((message) => {
    if (message?.type === 'status') {
      status = message.status;
      renderStatus();
    } else if (message?.type === 'recording') {
      recording = message.recording;
      renderRecording();
    }
  });
  $('recording-stop').addEventListener('click', () => void finishRecording(true));
  $('recording-cancel').addEventListener('click', () => void finishRecording(false));

  for (const button of document.querySelectorAll('.actions button')) {
    button.addEventListener('click', () => void act(button.dataset.action));
  }
  $('share').addEventListener('change', async () => {
    try {
      await request({ type: 'act', action: 'share', tabId: tab.id });
    } catch (error) {
      say(error.message, 'error');
    }
    void refreshTab();
  });
  for (const radio of document.querySelectorAll('input[name=destination]')) {
    radio.addEventListener('change', () => void update({ destination: radio.value }));
  }
  for (const key of ['bringForward', 'badge', 'notifyNews', 'notifyHandoff']) {
    $(key).addEventListener('change', () => void update({ [key]: $(key).checked }));
  }
  $('change-shortcuts').addEventListener('click', () => void chrome.tabs.create({ url: 'chrome://extensions/shortcuts' }));

  chrome.tabs.onActivated.addListener((info) => {
    if (info.windowId === windowId) void refreshTab();
  });
  chrome.tabs.onUpdated.addListener((tabId, change) => {
    if (tabId === tab?.id && (change.url || change.title || 'groupId' in change)) void refreshTab();
  });
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.settings) void renderSettings();
  });
  chrome.permissions.onAdded.addListener(() => void renderSites());
  chrome.permissions.onRemoved.addListener(() => void renderSites());

  renderStatus();
  await Promise.all([refreshTab(), renderSettings(), renderSites(), renderShortcuts()]);
}

void start();
