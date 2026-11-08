// Runs in the extension's isolated world of a page, only when the user asks:
// drag a box over the page to choose the area of a screenshot. Esc cancels.
// Built in a closed shadow root without innerHTML or inline style
// attributes, so pages with a strict Content Security Policy show it too.
(() => {
  const ns = (globalThis.__pilothouse ??= {});
  if (ns.area) return;

  const CSS = `
    :host { all: initial; }
    .veil { position: fixed; inset: 0; cursor: crosshair; background: rgba(15, 12, 30, 0.28); }
    .box { position: fixed; border: 2px solid rgb(139, 92, 246); background: rgba(139, 92, 246, 0.08);
      box-shadow: 0 0 0 9999px rgba(15, 12, 30, 0.35); pointer-events: none; display: none; }
    .size { position: fixed; padding: 2px 6px; border-radius: 4px; background: rgb(139, 92, 246);
      color: white; font: 600 11px/16px system-ui, sans-serif; pointer-events: none; display: none; }
    .hint { position: fixed; top: 16px; left: 50%; transform: translateX(-50%); padding: 8px 14px;
      border-radius: 8px; background: rgba(20, 18, 32, 0.92); color: white;
      font: 13px/18px system-ui, sans-serif; pointer-events: none; box-shadow: 0 4px 16px rgba(0,0,0,0.3); }
  `;

  const element = (root, tag, cls) => {
    const node = document.createElement(tag);
    node.className = cls;
    root.appendChild(node);
    return node;
  };

  ns.area = () => new Promise((resolve) => {
    const host = document.createElement('pilothouse-area');
    host.style.setProperty('position', 'fixed', 'important');
    host.style.setProperty('inset', '0', 'important');
    host.style.setProperty('z-index', '2147483647', 'important');
    const root = host.attachShadow({ mode: 'closed' });
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    root.adoptedStyleSheets = [sheet];
    const veil = element(root, 'div', 'veil');
    const box = element(root, 'div', 'box');
    const size = element(root, 'div', 'size');
    const hint = element(root, 'div', 'hint');
    hint.textContent = 'Drag over the part to send to PilotHouse · Esc to cancel';
    document.documentElement.appendChild(host);

    let start = null;
    const rectOf = (event) => {
      const x = Math.min(start.x, event.clientX);
      const y = Math.min(start.y, event.clientY);
      return { x, y, width: Math.abs(event.clientX - start.x), height: Math.abs(event.clientY - start.y) };
    };
    const finish = (rect) => {
      removeEventListener('keydown', onKey, true);
      host.remove();
      // Two frames, so the box is gone from the screenshot taken next.
      requestAnimationFrame(() => requestAnimationFrame(() => resolve(rect)));
    };
    const onKey = (event) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      finish(null);
    };
    addEventListener('keydown', onKey, true);
    veil.addEventListener('pointerdown', (event) => {
      event.preventDefault();
      veil.setPointerCapture(event.pointerId);
      start = { x: event.clientX, y: event.clientY };
      hint.style.display = 'none';
    });
    veil.addEventListener('pointermove', (event) => {
      if (!start) return;
      const r = rectOf(event);
      box.style.display = 'block';
      box.style.left = `${r.x}px`;
      box.style.top = `${r.y}px`;
      box.style.width = `${r.width}px`;
      box.style.height = `${r.height}px`;
      size.style.display = 'block';
      size.textContent = `${Math.round(r.width)} × ${Math.round(r.height)}`;
      size.style.left = `${r.x}px`;
      size.style.top = `${Math.max(0, r.y - 22)}px`;
    });
    veil.addEventListener('pointerup', (event) => {
      if (!start) return;
      const r = rectOf(event);
      // A click without a drag sends the visible page.
      finish(r.width < 4 || r.height < 4
        ? { x: 0, y: 0, width: innerWidth, height: innerHeight, whole: true }
        : r);
    });
  });
})();
