// Runs in the extension's isolated world of a page, only when the user asks:
// point at an element and click it to send it to PilotHouse (its selector,
// text, styles, the source file a dev build names, and HTML). Esc cancels.
// The page's own scripts cannot see or change it; the click does not reach
// the page.
(() => {
  const ns = (globalThis.__pilothouse ??= {});
  if (ns.pick) return;

  const CSS = `
    :host { all: initial; }
    .box { position: fixed; border: 2px solid rgb(139, 92, 246); background: rgba(139, 92, 246, 0.10);
      border-radius: 2px; pointer-events: none; display: none; }
    .tag { position: fixed; max-width: 60vw; padding: 3px 7px; border-radius: 4px; background: rgb(139, 92, 246);
      color: white; font: 600 11px/16px ui-monospace, Menlo, monospace; white-space: nowrap; overflow: hidden;
      text-overflow: ellipsis; pointer-events: none; display: none; }
    .hint { position: fixed; top: 16px; left: 50%; transform: translateX(-50%); padding: 8px 14px;
      border-radius: 8px; background: rgba(20, 18, 32, 0.92); color: white;
      font: 13px/18px system-ui, sans-serif; pointer-events: none; box-shadow: 0 4px 16px rgba(0,0,0,0.3); }
  `;

  const selectorOf = (el) => {
    const esc = (s) => (globalThis.CSS && CSS.escape ? CSS.escape(s) : s);
    for (const attr of ['data-testid', 'data-test', 'data-cy']) {
      const value = el.getAttribute && el.getAttribute(attr);
      if (value) return `[${attr}="${value}"]`;
    }
    const parts = [];
    for (let node = el; node && node.nodeType === 1 && parts.length < 5; node = node.parentElement) {
      if (node.id) {
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

  const labelOf = (el) => (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder
    || el.title || el.alt || '').trim().replace(/\s+/g, ' ').slice(0, 120);

  /** Where the element comes from, when the page is a dev build that says:
   * React (before 19), Vue, Svelte, or a data-source attribute. */
  const sourceOf = (el) => {
    for (let node = el; node && node.nodeType === 1; node = node.parentElement) {
      try {
        const svelte = node.__svelte_meta;
        if (svelte && svelte.loc) return `${svelte.loc.file}:${svelte.loc.line}`;
        const attr = node.getAttribute('data-source') || node.getAttribute('data-inspector-relative-path');
        if (attr) return attr;
      } catch (error) {
        // A property the page made unreadable.
      }
    }
    return null;
  };

  const describe = (el) => {
    const s = getComputedStyle(el);
    const keep = ['position', 'flex-direction', 'align-items', 'justify-content', 'gap', 'margin', 'padding',
      'border', 'border-radius', 'box-shadow', 'font-weight', 'line-height', 'text-align', 'opacity',
      'z-index', 'overflow'];
    const quiet = ['none', 'normal', 'auto', 'static', '0px', '1', 'visible', 'start', 'stretch', '400', 'row'];
    const css = keep.map((k) => [k, s.getPropertyValue(k)])
      .filter(([, v]) => v && !quiet.includes(v) && !/^0px none\b/.test(v))
      .map(([k, v]) => `${k}: ${v}`).join('; ');
    const r = el.getBoundingClientRect();
    return {
      selector: selectorOf(el),
      tag: el.localName,
      text: labelOf(el),
      html: el.outerHTML.slice(0, 1500),
      styles: `${s.display}; ${s.width} × ${s.height}; font ${s.fontSize} ${s.fontFamily.split(',')[0]}; color ${s.color}; background ${s.backgroundColor}`,
      css,
      source: sourceOf(el),
      rect: { x: r.x, y: r.y, width: r.width, height: r.height },
      url: location.href,
      title: document.title,
    };
  };

  const element = (root, tag, cls) => {
    const node = document.createElement(tag);
    node.className = cls;
    root.appendChild(node);
    return node;
  };

  ns.pick = () => new Promise((resolve) => {
    const host = document.createElement('pilothouse-picker');
    host.style.setProperty('position', 'fixed', 'important');
    host.style.setProperty('inset', '0', 'important');
    host.style.setProperty('z-index', '2147483647', 'important');
    host.style.setProperty('pointer-events', 'none', 'important');
    const root = host.attachShadow({ mode: 'closed' });
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    root.adoptedStyleSheets = [sheet];
    const box = element(root, 'div', 'box');
    const tag = element(root, 'div', 'tag');
    const hint = element(root, 'div', 'hint');
    hint.textContent = 'Click an element to send it to PilotHouse · Esc to cancel';
    document.documentElement.appendChild(host);

    let current = null;
    const show = (el) => {
      current = el;
      const r = el.getBoundingClientRect();
      box.style.display = 'block';
      box.style.left = `${r.x}px`;
      box.style.top = `${r.y}px`;
      box.style.width = `${r.width}px`;
      box.style.height = `${r.height}px`;
      tag.style.display = 'block';
      tag.textContent = `${selectorOf(el)}  ${Math.round(r.width)} × ${Math.round(r.height)}`;
      tag.style.left = `${Math.max(0, r.x)}px`;
      tag.style.top = `${r.y > 24 ? r.y - 22 : r.y + r.height + 4}px`;
    };
    const stop = (event) => {
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
    };
    const listeners = [];
    const on = (type, handler) => {
      addEventListener(type, handler, true);
      listeners.push([type, handler]);
    };
    const finish = (value) => {
      for (const [type, handler] of listeners) removeEventListener(type, handler, true);
      host.remove();
      requestAnimationFrame(() => requestAnimationFrame(() => resolve(value)));
    };
    on('pointermove', (event) => {
      const el = document.elementFromPoint(event.clientX, event.clientY);
      if (el && el !== current && el !== host) show(el);
    });
    for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup']) on(type, stop);
    on('click', (event) => {
      stop(event);
      const el = document.elementFromPoint(event.clientX, event.clientY) ?? current;
      finish(el ? describe(el) : null);
    });
    on('keydown', (event) => {
      if (event.key !== 'Escape') return;
      stop(event);
      finish(null);
    });
  });
})();
