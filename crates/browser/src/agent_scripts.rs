//! JavaScript for agents' Chrome ([`crate::AgentChrome`]), run in an
//! isolated world of each page: it shares the page's DOM but not its
//! scripts, so a site can neither see nor break it.
//!
//! - [`OVERLAY`]: the glow around the page, the pointer and the bar with
//!   Stop, shown while an agent acts. Built with DOM calls and a
//!   constructed style sheet (no `innerHTML`, no inline `<style>`), so pages
//!   with a strict Content Security Policy or Trusted Types show it too.
//! - [`AGENT`]: reading the page for agents (numbered elements) and finding
//!   what an agent names: a number from the last read, a CSS selector, or
//!   the text on it.

/// The isolated world's name; the overlay's Stop button calls the binding
/// of the same name.
pub const WORLD: &str = "pilothouse";
/// The binding the bar's buttons call with `stop` or `continue`.
pub const BINDING: &str = "__pilothouseControl";

/// In the page's own world, before its scripts: Chrome sets
/// `navigator.webdriver` for any browser driven over DevTools, which bot
/// checks look at first.
pub const NOT_AUTOMATED: &str = r#"(() => {
  try {
    Object.defineProperty(Navigator.prototype, 'webdriver', { get: () => false, configurable: true });
  } catch (_) {}
})()"#;

pub const OVERLAY: &str = r#"(() => {
  if (window.__phOverlay) return;
  const VIOLET = '139, 92, 246';
  const css = `
    :host { all: initial; }
    .frame { position: fixed; inset: 0; pointer-events: none; opacity: 0;
      transition: opacity .8s ease;
      box-shadow: inset 0 0 0 2px rgba(${VIOLET}, .85), inset 0 0 22px 5px rgba(${VIOLET}, .38); }
    .on .frame { opacity: 1; animation: breathe 2.4s ease-in-out infinite; }
    @keyframes breathe {
      0%, 100% { box-shadow: inset 0 0 0 2px rgba(${VIOLET}, .8), inset 0 0 18px 4px rgba(${VIOLET}, .3); }
      50% { box-shadow: inset 0 0 0 2px rgba(${VIOLET}, 1), inset 0 0 34px 9px rgba(${VIOLET}, .5); }
    }
    .cursor { position: fixed; left: -40px; top: -40px; width: 24px; height: 24px; pointer-events: none;
      opacity: 0; transition: left .22s ease, top .22s ease, opacity .25s ease;
      filter: drop-shadow(0 1px 2px rgba(0, 0, 0, .45)); }
    .on .cursor.placed { opacity: 1; }
    .ripple { position: fixed; width: 30px; height: 30px; margin: -15px 0 0 -15px; border-radius: 50%;
      border: 2px solid rgba(${VIOLET}, .95); pointer-events: none; animation: ripple .5s ease-out forwards; }
    @keyframes ripple { from { transform: scale(.3); opacity: 1; } to { transform: scale(1.4); opacity: 0; } }
    .bar { position: fixed; left: 50%; bottom: 16px; transform: translateX(-50%); display: flex; gap: 10px;
      align-items: center; padding: 5px 5px 5px 12px; border-radius: 999px; background: rgba(22, 18, 36, .94);
      color: #fff; font: 500 12px/1.2 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
      box-shadow: 0 6px 22px rgba(0, 0, 0, .35), 0 0 0 1px rgba(${VIOLET}, .55); pointer-events: none;
      opacity: 0; transition: opacity .25s ease; white-space: nowrap; }
    .on .bar, .paused .bar { opacity: 1; pointer-events: auto; }
    .through .bar { pointer-events: none !important; }
    .dot { width: 8px; height: 8px; border-radius: 50%; background: rgb(${VIOLET}); animation: blink 1.2s ease-in-out infinite; }
    .paused .dot { animation: none; background: #f5b74e; }
    @keyframes blink { 50% { opacity: .35; } }
    button { all: unset; cursor: pointer; padding: 5px 11px; border-radius: 999px; background: #fff; color: #17131f;
      font: 600 12px/1.2 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif; }
    button:hover { background: #ece8f7; }
  `;
  const state = { host: null, root: null, wrap: null, cursor: null, text: null, button: null,
    label: 'An agent', on: false, paused: false };
  const el = (tag, cls) => { const node = document.createElement(tag); if (cls) node.className = cls; return node; };
  const build = () => {
    if (state.host && state.host.isConnected) return;
    if (!state.host) {
      const host = document.createElement('pilothouse-overlay');
      for (const [k, v] of [['position', 'fixed'], ['inset', '0'], ['z-index', '2147483647'],
        ['pointer-events', 'none'], ['display', 'block'], ['margin', '0'], ['padding', '0']]) {
        host.style.setProperty(k, v, 'important');
      }
      const root = host.attachShadow({ mode: 'closed' });
      try {
        const sheet = new CSSStyleSheet();
        sheet.replaceSync(css);
        root.adoptedStyleSheets = [sheet];
      } catch (_) {}
      const wrap = el('div');
      wrap.appendChild(el('div', 'frame'));
      const cursor = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      cursor.setAttribute('viewBox', '0 0 24 24');
      cursor.setAttribute('class', 'cursor');
      const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
      path.setAttribute('d', 'M3 2 L3 19 L7.6 14.8 L10.4 21.2 L13.4 19.9 L10.6 13.6 L17 13.6 Z');
      path.setAttribute('fill', `rgb(${VIOLET})`);
      path.setAttribute('stroke', '#fff');
      path.setAttribute('stroke-width', '1.5');
      path.setAttribute('stroke-linejoin', 'round');
      cursor.appendChild(path);
      wrap.appendChild(cursor);
      const bar = el('div', 'bar');
      bar.appendChild(el('div', 'dot'));
      const text = el('span');
      bar.appendChild(text);
      const button = el('button');
      button.addEventListener('click', (event) => {
        event.preventDefault();
        event.stopPropagation();
        const call = window['__pilothouseControl'];
        if (typeof call === 'function') call(state.paused ? 'continue' : 'stop');
      }, true);
      bar.appendChild(button);
      wrap.appendChild(bar);
      root.appendChild(wrap);
      Object.assign(state, { host, root, wrap, cursor, text, button });
    }
    (document.documentElement || document).appendChild(state.host);
  };
  const render = () => {
    if (!state.wrap) return;
    state.wrap.className = (state.paused ? 'paused' : (state.on ? 'on' : '')) + (state.through ? ' through' : '');
    state.text.textContent = state.paused
      ? `${state.label} is paused`
      : `${state.label} is using this tab`;
    state.button.textContent = state.paused ? 'Let it continue' : 'Stop';
  };
  window.__phOverlay = {
    show(label, paused) {
      state.label = label || state.label;
      state.on = !paused;
      state.paused = !!paused;
      build();
      render();
    },
    hide() {
      state.on = false;
      state.through = false;
      render();
    },
    move(x, y) {
      build();
      state.cursor.style.setProperty('left', `${x - 3}px`);
      state.cursor.style.setProperty('top', `${y - 2}px`);
      state.cursor.classList.add('placed');
    },
    pulse(x, y) {
      if (!state.root) return;
      const ring = el('div', 'ripple');
      ring.style.setProperty('left', `${x}px`);
      ring.style.setProperty('top', `${y}px`);
      state.wrap.appendChild(ring);
      setTimeout(() => ring.remove(), 600);
    },
    // While the agent clicks, the bar lets its clicks through to the page
    // under it (a cookie banner at the bottom); the user's clicks still
    // reach Stop.
    through(on) {
      state.through = !!on;
      render();
    },
    // Out of the way of a screenshot, and back.
    conceal(hidden) {
      if (state.host) state.host.style.setProperty('display', hidden ? 'none' : 'block', 'important');
    },
  };
})()"#;

pub const AGENT: &str = r#"(() => {
  if (window.__phAgent) return;
  const refs = new Map();
  let nextRef = 1;
  const clean = (s) => (s || '').trim().replace(/\s+/g, ' ');
  const labelOf = (el) => {
    const aria = el.getAttribute('aria-label') || el.getAttribute('aria-labelledby') && [...el.getAttribute('aria-labelledby').split(/\s+/)].map((id) => document.getElementById(id)?.innerText || '').join(' ');
    let label = aria || '';
    if (!label && el.labels && el.labels.length) label = el.labels[0].innerText;
    // A list's own text is all its options: its name says more.
    const text = el.localName === 'select' ? '' : el.innerText;
    if (!label) label = text || (el.localName === 'select' ? '' : el.value) || el.placeholder || el.title || el.alt || el.getAttribute('name') || el.id || '';
    return clean(label).slice(0, 80);
  };
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
  };
  const INTERACTIVE = 'a[href], button, input:not([type=hidden]), select, textarea, summary, [role=button], [role=link], [role=tab], [role=menuitem], [role=checkbox], [role=radio], [role=switch], [role=option], [role=combobox], [role=textbox], [contenteditable=""], [contenteditable=true], [onclick], [tabindex]:not([tabindex="-1"])';
  const kindOf = (el) => {
    const role = el.getAttribute('role');
    if (role) return role;
    if (el.localName === 'a') return 'link';
    if (el.localName === 'input') return `input[${el.type || 'text'}]`;
    if (el.isContentEditable) return 'editable';
    return el.localName;
  };
  const describe = (el, ref) => {
    let line = `[${ref}] ${kindOf(el)} "${labelOf(el)}"`;
    if (el.localName === 'input' || el.localName === 'textarea') {
      if (el.type === 'checkbox' || el.type === 'radio') line += el.checked ? ' (checked)' : ' (not checked)';
      else if (el.type === 'password') line += el.value ? ' = (password, filled)' : '';
      else if (el.value) line += ` = "${clean(el.value).slice(0, 80)}"`;
    }
    if (el.localName === 'select') {
      const chosen = el.selectedOptions && el.selectedOptions[0];
      line += ` = "${chosen ? clean(chosen.text) : ''}" of [${[...el.options].slice(0, 12).map((o) => `"${clean(o.text)}"`).join(', ')}${el.options.length > 12 ? ', …' : ''}]`;
    }
    if (el.getAttribute('aria-expanded')) line += el.getAttribute('aria-expanded') === 'true' ? ' (expanded)' : ' (collapsed)';
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') line += ' (disabled)';
    if (el.localName === 'a') {
      const href = el.getAttribute('href') || '';
      if (href && !href.startsWith('javascript:')) line += ` → ${href.slice(0, 120)}`;
    }
    return line;
  };
  const byText = (text) => {
    const want = clean(text).toLowerCase();
    const candidates = [...document.querySelectorAll(INTERACTIVE + ', label')].filter(visible);
    const exact = candidates.find((el) => labelOf(el).toLowerCase() === want);
    if (exact) return exact;
    const partial = candidates.find((el) => labelOf(el).toLowerCase().includes(want));
    if (partial) return partial;
    // Any visible element whose own text it is (headings, list items).
    const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (clean(node.textContent).toLowerCase().includes(want) && node.parentElement && visible(node.parentElement)) {
        return node.parentElement;
      }
    }
    return null;
  };
  const find = (spec) => {
    if (spec.ref !== undefined && spec.ref !== null) {
      const el = refs.get(Number(spec.ref))?.deref();
      if (!el || !el.isConnected) throw new Error(`element [${spec.ref}] is gone: read the page again (chrome_snapshot)`);
      return el;
    }
    if (spec.selector) {
      const el = document.querySelector(spec.selector);
      if (!el) throw new Error(`no element matches \`${spec.selector}\``);
      return el;
    }
    if (spec.text) {
      const el = byText(spec.text);
      if (!el) throw new Error(`nothing on the page says "${spec.text}"`);
      return el;
    }
    throw new Error('name an element: `ref` (from chrome_snapshot), `selector` or `text`');
  };
  window.__phAgent = {
    snapshot(limit) {
      refs.clear();
      nextRef = 1;
      const elements = [];
      for (const el of document.querySelectorAll(INTERACTIVE)) {
        if (elements.length >= limit) break;
        if (!visible(el)) continue;
        const ref = nextRef++;
        refs.set(ref, new WeakRef(el));
        elements.push(describe(el, ref));
      }
      const doc = document.scrollingElement || document.documentElement;
      const text = (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 6000);
      const frames = document.querySelectorAll('iframe').length;
      return {
        title: document.title, url: location.href, elements, text,
        scroll: { y: Math.round(doc.scrollTop), height: Math.round(doc.scrollHeight), view: innerHeight },
        frames,
        focused: document.activeElement && document.activeElement !== document.body ? labelOf(document.activeElement) : null,
      };
    },
    // The middle of an element, scrolled into view, in viewport pixels.
    locate(spec) {
      const el = find(spec);
      el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
      const r = el.getBoundingClientRect();
      if (r.width <= 0 && r.height <= 0) throw new Error('the element is not visible');
      return { x: r.x + r.width / 2, y: r.y + r.height / 2, label: labelOf(el), tag: el.localName };
    },
    // Focuses a field (selecting its text when `clear`); says whether it
    // is a password field.
    focus(spec, clear) {
      const el = find(spec);
      el.scrollIntoView({ block: 'center', behavior: 'instant' });
      el.focus();
      if (clear) {
        if ('select' in el && typeof el.select === 'function') el.select();
        else if (el.isContentEditable) {
          const range = document.createRange();
          range.selectNodeContents(el);
          const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range);
        }
      }
      return { password: el.localName === 'input' && el.type === 'password' };
    },
    focusedIsPassword() {
      const el = document.activeElement;
      return !!el && el.localName === 'input' && el.type === 'password';
    },
    select(spec, values) {
      const el = find(spec);
      if (el.localName !== 'select') throw new Error('that element is not a list to choose from (select)');
      const wanted = values.map((v) => clean(v).toLowerCase());
      let found = 0;
      for (const option of el.options) {
        const match = wanted.includes(option.value.toLowerCase()) || wanted.includes(clean(option.text).toLowerCase());
        if (el.multiple) option.selected = match; else if (match && !found) option.selected = true;
        if (match) found++;
      }
      if (!found) throw new Error(`no option is ${values.map((v) => `"${v}"`).join(' or ')}`);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return [...el.selectedOptions].map((o) => clean(o.text));
    },
    element(spec) { return find(spec); },
    has(spec) {
      try {
        const el = find(spec);
        return visible(el);
      } catch (_) { return false; }
    },
    scrollBy(spec, dx, dy) {
      const target = spec ? find(spec) : (document.scrollingElement || document.documentElement);
      target.scrollBy({ left: dx, top: dy, behavior: 'instant' });
      const doc = document.scrollingElement || document.documentElement;
      return { y: Math.round(doc.scrollTop), height: Math.round(doc.scrollHeight), view: innerHeight };
    },
    ready() { return document.readyState; },
    // A check that a person is here (reCAPTCHA, hCaptcha, Cloudflare
    // Turnstile, Google's unusual-traffic page) showing on the page.
    // Invisible score checks (reCAPTCHA v3) do not count.
    humanCheck() {
      if (/\/sorry\//.test(location.pathname) && /(^|\.)google\./.test(location.hostname)) return true;
      if (/^just a moment/i.test(document.title)) return true;
      const frames = document.querySelectorAll('iframe[src*="recaptcha"], iframe[src*="hcaptcha.com"], iframe[src*="challenges.cloudflare.com"]');
      return [...frames].some((frame) => !/size=invisible/.test(frame.src) && visible(frame));
    },
  };
})()"#;

/// Calls `function` of `object` (`__phAgent`, `__phOverlay`) with JSON
/// arguments, as an expression for the isolated world.
pub fn call(object: &str, function: &str, arguments: &[serde_json::Value]) -> String {
    let arguments: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
    format!("window.{object}.{function}({})", arguments.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn calls_quote_their_arguments() {
        assert_eq!(
            call(
                "__phAgent",
                "locate",
                &[json!({ "selector": "a[href=\"x\"]" })]
            ),
            r#"window.__phAgent.locate({"selector":"a[href=\"x\"]"})"#
        );
    }
}
