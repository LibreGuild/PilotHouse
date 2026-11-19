// PilotHouse's popup, under its toolbar button: send from the tab in front,
// record in it, let agents use it; what waits for the user and what agents
// do in this browser, when there is any. Built with DOM calls only (no
// innerHTML).
//
// It is a popup and not a side panel: opening it is the user pointing at the
// tab (Chrome's `activeTab`), so the page is read, and its video recorded,
// without asking for the site first; it closes by itself; and a toolbar
// popup is the same in every browser these extensions run in (Chrome, Edge,
// Brave, Chromium).

import { settings, update } from './lib/settings.js';
import { reachable } from './lib/tabs.js';
import { notInvoked } from './lib/page.js';

const $ = (id) => document.getElementById(id);
const RECORDINGS = { bug: 'Recording a bug report', workflow: 'Recording a workflow' };
/** What goes on in the page after the popup closed (the user drags, or
 * clicks an element). */
const IN_PAGE = new Set(['area', 'element']);

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

// ---- What PilotHouse says ---------------------------------------------------

function renderStatus() {
  const on = status.connected;
  $('connection').textContent = on ? 'Connected' : 'Not connected';
  $('connection').classList.toggle('on', on);
  $('offline').hidden = on;
  renderCommands();

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
  $('agents-section').hidden = agents.length === 0;
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
  clearInterval(recordingTimer);
  $('recording').hidden = !recording;
  // One recording at a time: while it runs, its Stop stands for Record.
  $('record-section').hidden = !!recording;
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
  void request({ type: 'openChat', tag }).then(() => window.close(), (error) => say(error.message, 'error'));
}

async function focusTab(tabId) {
  if (!Number.isSafeInteger(tabId)) return;
  const target = await chrome.tabs.get(tabId).catch(() => null);
  if (!target) return;
  await chrome.tabs.update(tabId, { active: true });
  await chrome.windows.update(target.windowId, { focused: true });
  window.close();
}

// ---- The tab in front -------------------------------------------------------

/** The commands are for a page Chrome lets extensions read, with PilotHouse
 * there to take what is sent. */
function renderCommands() {
  const readable = !!tab && reachable(tab.url);
  for (const button of document.querySelectorAll('.commands button')) {
    button.disabled = !readable || !status.connected;
  }
  $('share').disabled = !readable || !status.connected;
}

async function readTab() {
  [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  renderCommands();
  if (!tab) return;
  if (!reachable(tab.url)) {
    say('Chrome keeps this page out of reach of extensions.');
    return;
  }
  const group = tab.groupId !== undefined && tab.groupId !== -1
    ? await chrome.tabGroups.get(tab.groupId).catch(() => null)
    : null;
  $('share').checked = group?.title === 'PilotHouse';
  // "The selection" shows only when there is one.
  const [selected] = await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: () => String(getSelection() ?? '').trim().length > 0,
  }).catch(() => []);
  $('selection').hidden = !selected?.result;
}

async function act(action) {
  if (!tab) return;
  say('');
  try {
    if (IN_PAGE.has(action)) {
      // The page is where it goes on: the popup gets out of its way, and
      // the toolbar icon says how it went.
      await request({ type: 'start', action, tabId: tab.id });
      window.close();
      return;
    }
    if (action === 'bug' || action === 'workflow') {
      let streamId;
      let noVideo = false;
      if (action === 'bug') {
        // Asked in the click, as Chrome wants for recording a tab.
        streamId = await chrome.tabCapture.getMediaStreamId({ targetTabId: tab.id }).catch((error) => {
          noVideo = notInvoked(error);
          return undefined;
        });
      }
      await request({ type: 'act', action, tabId: tab.id, streamId });
      // The bar in the page has Stop and Cancel from here on.
      if (noVideo) say('Recording, without a video: Chrome did not let PilotHouse record this tab’s video.');
      else window.close();
      return;
    }
    say('Sending…');
    const result = await request({ type: 'act', action, tabId: tab.id });
    say(result?.where ? `Sent to ${result.where}.` : 'Sent to PilotHouse.', 'ok');
    setTimeout(() => window.close(), 900);
  } catch (error) {
    say(error.message, 'error');
  }
}

// ---- The two choices --------------------------------------------------------

async function renderSettings() {
  const chosen = await settings();
  for (const radio of document.querySelectorAll('input[name=destination]')) {
    radio.checked = radio.value === chosen.destination;
  }
  $('bringForward').checked = !!chosen.bringForward;
}

/** The keys that do the same, beside each command (they are Chrome's to
 * change, at chrome://extensions/shortcuts). */
async function renderShortcuts() {
  const commands = await chrome.commands.getAll().catch(() => []);
  for (const key of document.querySelectorAll('kbd[data-command]')) {
    key.textContent = commands.find((command) => command.name === key.dataset.command)?.shortcut ?? '';
  }
}

// ---- Start ------------------------------------------------------------------

/** The worker's news (status, recording); it sends both first on each
 * connection. The worker restarts now and then, closing the port: the
 * popup connects again. */
function listen() {
  let port;
  try {
    port = chrome.runtime.connect({ name: 'popup' });
  } catch (error) {
    // The extension was reloaded: this popup is closing.
    return;
  }
  port.onMessage.addListener((message) => {
    if (message?.type === 'status') {
      status = message.status;
      renderStatus();
    } else if (message?.type === 'recording') {
      recording = message.recording;
      renderRecording();
    }
  });
  port.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    setTimeout(listen, 1000);
  });
}

async function start() {
  listen();
  $('recording-stop').addEventListener('click', () => void finishRecording(true));
  $('recording-cancel').addEventListener('click', () => void finishRecording(false));
  for (const button of document.querySelectorAll('.commands button')) {
    button.addEventListener('click', () => void act(button.dataset.action));
  }
  $('share').addEventListener('change', async () => {
    try {
      await request({ type: 'act', action: 'share', tabId: tab.id, checked: $('share').checked });
    } catch (error) {
      $('share').checked = !$('share').checked;
      say(error.message, 'error');
    }
  });
  for (const radio of document.querySelectorAll('input[name=destination]')) {
    radio.addEventListener('change', () => void update({ destination: radio.value }));
  }
  $('bringForward').addEventListener('change', () => void update({ bringForward: $('bringForward').checked }));

  renderStatus();
  await Promise.all([readTab(), renderSettings(), renderShortcuts()]);
}

void start();
