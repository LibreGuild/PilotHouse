// Runs in the extension's isolated world of a page, only while the user
// records (a bug report, or a workflow): notes their clicks, what they type
// (never passwords or card numbers), choices in lists and key presses, and
// shows a bar saying so, with Stop and Cancel. Each step goes to the
// extension as it happens, so a new page carries on the same recording.
(() => {
  const ns = (globalThis.__pilothouse ??= {});
  if (ns.recorder) return;

  const CSS = `
    :host { all: initial; }
    .bar { position: fixed; left: 16px; bottom: 16px; display: flex; align-items: center; gap: 10px;
      padding: 8px 10px 8px 12px; border-radius: 10px; background: rgba(20, 18, 32, 0.94); color: white;
      font: 13px/18px system-ui, -apple-system, sans-serif; box-shadow: 0 6px 24px rgba(0,0,0,0.35); }
    .dot { width: 10px; height: 10px; border-radius: 50%; background: #ef4444; animation: pulse 1.4s infinite; }
    @keyframes pulse { 50% { opacity: 0.35; } }
    .time { font-variant-numeric: tabular-nums; color: #c4c1d4; }
    button { font: 600 12px/16px system-ui, sans-serif; border-radius: 6px; padding: 5px 10px; cursor: pointer;
      border: 1px solid rgba(255,255,255,0.25); background: transparent; color: white; }
    button.stop { background: rgb(139, 92, 246); border-color: rgb(139, 92, 246); }
    button:focus-visible { outline: 2px solid white; outline-offset: 1px; }
  `;

  const esc = (s) => (globalThis.CSS && CSS.escape ? CSS.escape(s) : s);

  const selectorOf = (el) => {
    for (const attr of ['data-testid', 'data-test', 'data-cy']) {
      const value = el.getAttribute && el.getAttribute(attr);
      if (value) return `[${attr}="${value}"]`;
    }
    if (el.id && !/\d{3,}/.test(el.id)) return `#${esc(el.id)}`;
    const name = el.getAttribute && el.getAttribute('name');
    if (name && ['input', 'select', 'textarea', 'button'].includes(el.localName)) {
      return `${el.localName}[name="${name}"]`;
    }
    const parts = [];
    for (let node = el; node && node.nodeType === 1 && parts.length < 5; node = node.parentElement) {
      if (node.id && !/\d{3,}/.test(node.id)) {
        parts.unshift(`#${esc(node.id)}`);
        break;
      }
      let part = node.localName;
      const parent = node.parentElement;
      if (parent) {
        const same = [...parent.children].filter((child) => child.localName === node.localName);
        if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
      }
      parts.unshift(part);
    }
    return parts.join(' > ');
  };

  const clean = (s) => (s || '').trim().replace(/\s+/g, ' ').slice(0, 80);

  /** What a person would call the element: its label, words or role. */
  const labelOf = (el) => {
    const aria = el.getAttribute('aria-label');
    if (aria) return clean(aria);
    if (el.labels && el.labels[0]) return clean(el.labels[0].innerText);
    const placeholder = el.getAttribute('placeholder');
    if (placeholder) return clean(placeholder);
    return clean(el.innerText || el.value || el.title || el.alt || el.getAttribute('name') || '');
  };

  /** Fields whose value is never kept. */
  const secret = (el) => {
    const type = (el.getAttribute('type') || '').toLowerCase();
    const auto = (el.getAttribute('autocomplete') || '').toLowerCase();
    return type === 'password' || /cc-|one-time-code|current-password|new-password/.test(auto);
  };

  /** The element a click means: the button or link around what was hit. */
  const clickable = (el) => el.closest('a[href], button, [role=button], [role=link], [role=tab], [role=menuitem], [role=option], [role=checkbox], [role=switch], input, select, label, summary') || el;

  let state = null;

  /** To the extension: through the debugger's binding (pages after the
   * first, and frames), else as an extension message. */
  const post = (message) => {
    try {
      const binding = globalThis.__pilothouseRecorder;
      if (typeof binding === 'function') binding(JSON.stringify(message));
      else void chrome.runtime.sendMessage(message);
    } catch (error) {
      // The extension was reloaded: this page's recorder is stale.
    }
  };

  const send = (step) => post({ type: 'recorder-step', step: { ...step, at: Date.now() } });

  const fromBar = (event) => state && event.composedPath().includes(state.host);

  const onClick = (event) => {
    if (!state || fromBar(event) || !event.isTrusted) return;
    const target = event.target instanceof Element ? clickable(event.target) : null;
    if (!target) return;
    const tag = target.localName;
    // Typing into a field is noted when it changes, not when it is clicked.
    if ((tag === 'input' && !['checkbox', 'radio', 'submit', 'button', 'reset', 'file'].includes(target.type))
        || tag === 'textarea' || tag === 'select' || target.isContentEditable) return;
    send({ type: 'click', selector: selectorOf(target), text: labelOf(target), tag });
  };

  const onChange = (event) => {
    if (!state || fromBar(event) || !event.isTrusted) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (target.localName === 'select') {
      const chosen = [...target.selectedOptions].map((option) => clean(option.text)).join(', ');
      send({ type: 'select', selector: selectorOf(target), text: labelOf(target), value: chosen });
    } else if (target.localName === 'textarea' || (target.localName === 'input'
        && !['checkbox', 'radio', 'submit', 'button', 'reset', 'file'].includes(target.type))) {
      send({
        type: 'fill',
        selector: selectorOf(target),
        text: labelOf(target),
        value: secret(target) ? null : String(target.value).slice(0, 2000),
        secret: secret(target),
      });
    } else if (target.localName === 'input' && target.type === 'file') {
      send({ type: 'upload', selector: selectorOf(target), text: labelOf(target), files: [...target.files].map((f) => f.name) });
    }
  };

  const onKey = (event) => {
    if (!state || fromBar(event) || !event.isTrusted) return;
    if (!['Enter', 'Escape'].includes(event.key)) return;
    const target = event.target instanceof Element ? event.target : null;
    // Enter in a field commits what was typed first.
    if (event.key === 'Enter' && target && target.localName === 'input') onChange({ target, isTrusted: true, composedPath: () => [] });
    send({ type: 'press', key: event.key, selector: target ? selectorOf(target) : null });
  };

  const element = (root, tag, cls, text) => {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text) node.textContent = text;
    root.appendChild(node);
    return node;
  };

  ns.recorder = {
    /** Starts (or, on a new page of the same recording, resumes). */
    start({ label, startedAt, stopLabel }) {
      if (state) return;
      const listen = () => {
        addEventListener('click', onClick, true);
        addEventListener('change', onChange, true);
        addEventListener('keydown', onKey, true);
      };
      // Frames record too; the bar is the top page's.
      if (window !== window.top) {
        state = { host: document.createElement('span'), timer: null };
        listen();
        return;
      }
      // A new page's recorder starts before the page has its elements: it
      // listens at once, and the bar comes when the page is ready.
      state = { host: null, timer: null };
      listen();
      const build = () => {
        if (!state || state.host) return;
        state.host = bar({ label, startedAt, stopLabel });
      };
      if (document.documentElement && document.body) build();
      else addEventListener('DOMContentLoaded', build, { once: true });
    },
    stop() {
      if (!state) return;
      if (state.timer) clearInterval(state.timer);
      state.host?.remove();
      state = null;
      removeEventListener('click', onClick, true);
      removeEventListener('change', onChange, true);
      removeEventListener('keydown', onKey, true);
    },
  };

  /** The bar saying the page is recorded, with Stop and Cancel. */
  function bar({ label, startedAt, stopLabel }) {
      const host = document.createElement('pilothouse-recorder');
      host.style.setProperty('position', 'fixed', 'important');
      host.style.setProperty('z-index', '2147483647', 'important');
      host.style.setProperty('left', '0', 'important');
      host.style.setProperty('bottom', '0', 'important');
      const root = host.attachShadow({ mode: 'closed' });
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(CSS);
      root.adoptedStyleSheets = [sheet];
      const bar = element(root, 'div', 'bar');
      bar.setAttribute('role', 'status');
      element(bar, 'span', 'dot');
      element(bar, 'span', '', label);
      const time = element(bar, 'span', 'time', '0:00');
      const stop = element(bar, 'button', 'stop', stopLabel);
      const cancel = element(bar, 'button', '', 'Cancel');
      stop.addEventListener('click', () => post({ type: 'recorder-stop' }));
      cancel.addEventListener('click', () => post({ type: 'recorder-cancel' }));
      document.documentElement.appendChild(host);
      const tick = () => {
        const seconds = Math.max(0, Math.floor((Date.now() - startedAt) / 1000));
        time.textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
      };
      tick();
      state.timer = setInterval(tick, 1000);
      return host;
  }
})();
