//! JavaScript run inside the page.

/// Keeps everything in one pane: links that would open a new tab, and
/// `window.open`, navigate the current page instead. Runs on every document.
pub const SINGLE_PANE: &str = r#"(() => {
  if (window.__pilothouse) return;
  window.__pilothouse = true;
  window.open = (url) => { if (url) location.href = url; return window; };
  document.addEventListener('click', (event) => {
    const link = event.target instanceof Element && event.target.closest('a[target]');
    if (link && link.target !== '_self') link.target = '_self';
  }, true);
})()"#;

/// Shared helper: a short, stable CSS selector for an element.
const SELECTOR_OF: &str = r#"
const selectorOf = (el) => {
  const esc = (s) => (window.CSS && CSS.escape) ? CSS.escape(s) : s;
  for (const attr of ['data-testid', 'data-test', 'data-cy']) {
    const v = el.getAttribute && el.getAttribute(attr);
    if (v) return `[${attr}="${v}"]`;
  }
  const parts = [];
  for (let node = el; node && node.nodeType === 1 && parts.length < 5; node = node.parentElement) {
    if (node.id) { parts.unshift('#' + esc(node.id)); break; }
    let part = node.localName;
    const parent = node.parentElement;
    if (parent) {
      const same = [...parent.children].filter((c) => c.localName === node.localName);
      if (same.length > 1) part += `:nth-of-type(${same.indexOf(node) + 1})`;
    }
    parts.unshift(part);
  }
  return parts.join(' > ');
};
const labelOf = (el) => (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.title || el.alt || '').trim().replace(/\s+/g, ' ').slice(0, 80);
"#;

/// Describes the element at CSS point (x, y) for the element picker.
pub fn pick(x: f32, y: f32) -> String {
    format!(
        r#"(() => {{ {SELECTOR_OF}
  const el = document.elementFromPoint({x}, {y});
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return {{
    selector: selectorOf(el),
    tag: el.localName,
    text: labelOf(el),
    html: el.outerHTML.slice(0, 800),
    styles: (() => {{ const s = getComputedStyle(el); return `${{s.display}}; ${{s.width}} × ${{s.height}}; font ${{s.fontSize}} ${{s.fontFamily.split(',')[0]}}; color ${{s.color}}; background ${{s.backgroundColor}}`; }})(),
    css: (() => {{
      const s = getComputedStyle(el);
      // Only what is not the default (`display` is in `styles` already).
      const keep = ['position', 'flex-direction', 'align-items', 'justify-content', 'gap',
        'margin', 'padding', 'border', 'border-radius', 'box-shadow', 'font-weight', 'line-height',
        'text-align', 'opacity', 'z-index', 'overflow'];
      const quiet = ['none', 'normal', 'auto', 'static', '0px', '1', 'visible', 'start', 'stretch', '400', 'row'];
      return keep.map(k => [k, s.getPropertyValue(k)])
        .filter(([, v]) => v && !quiet.includes(v) && !/^0px none\b/.test(v))
        .map(([k, v]) => `${{k}}: ${{v}}`).join('; ');
    }})(),
    source: (() => {{
      // Where the element comes from, when the page is a dev build that
      // says: React (before 19), Vue, Svelte, or a data-source attribute.
      for (let n = el; n && n.nodeType === 1; n = n.parentElement) {{
        const svelte = n.__svelte_meta;
        if (svelte && svelte.loc) return `${{svelte.loc.file}}:${{svelte.loc.line}}`;
        const fiberKey = Object.keys(n).find(k => k.startsWith('__reactFiber$'));
        if (fiberKey) for (let f = n[fiberKey]; f; f = f.return) {{
          const s = f._debugSource; if (s) return `${{s.fileName}}:${{s.lineNumber}}`;
        }}
        const vue = n.__vueParentComponent;
        if (vue && vue.type && vue.type.__file) return vue.type.__file;
        const attr = n.getAttribute('data-source') || n.getAttribute('data-inspector-relative-path');
        if (attr) return attr;
      }}
      return null;
    }})(),
    rect: {{ x: r.x, y: r.y, width: r.width, height: r.height }},
  }};
}})()"#
    )
}

/// The center of the element matching `selector`, scrolled into view.
pub fn locate(selector: &str) -> String {
    let selector = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(() => {{
  const el = document.querySelector({selector});
  if (!el) return null;
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  const r = el.getBoundingClientRect();
  return {{ x: r.x + r.width / 2, y: r.y + r.height / 2 }};
}})()"#
    )
}

/// Whether the focused element is a password field.
pub const FOCUSED_IS_PASSWORD: &str = r#"(() => { const el = document.activeElement; return !!el && el.tagName === 'INPUT' && el.type === 'password'; })()"#;

pub fn focus(selector: &str) -> String {
    let selector = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(() => {{ const el = document.querySelector({selector}); if (!el) return false; el.focus(); if ('select' in el) el.select(); return true; }})()"#
    )
}

/// A compact outline for agents: interactive elements with selectors, plus
/// the visible text, so an agent can act without reading raw HTML.
pub fn snapshot() -> String {
    ["(() => {", SELECTOR_OF, SNAPSHOT_BODY, "})()"].concat()
}

const SNAPSHOT_BODY: &str = r#"
const visible = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
const query = 'a[href], button, input, select, textarea, [role=button], [role=link], [role=tab], [role=checkbox], [contenteditable=true], [onclick]';
const elements = [...document.querySelectorAll(query)].filter(visible).slice(0, 150).map((el) => {
  const kind = el.getAttribute('role') || (el.localName === 'input' ? `input[${el.type}]` : el.localName);
  return `${kind} "${labelOf(el) || el.name || ''}" → ${selectorOf(el)}`;
});
const text = (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 4000);
return { title: document.title, url: location.href, elements, text };
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_are_quoted_safely() {
        let script = locate(r#"a[href="x"]"#);
        assert!(script.contains(r#"document.querySelector("a[href=\"x\"]")"#));
        assert!(pick(1.0, 2.0).contains("elementFromPoint(1, 2)"));
    }
}
