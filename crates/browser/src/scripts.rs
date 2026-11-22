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
// What was typed into a field is never its name (a password or a card number
// would reach the agent as a label): only a button's value is the words on it.
const labelOf = (el) => {
  const field = el.localName === 'input' || el.localName === 'textarea';
  const button = el.localName === 'input' && ['submit', 'button', 'reset'].includes(el.type);
  const label = el.getAttribute('aria-label') || (field && el.labels && el.labels.length ? el.labels[0].innerText : '')
    || (field ? '' : el.innerText) || (button ? el.value : '') || el.placeholder || el.title || el.alt
    || (field ? el.getAttribute('name') || el.id : '') || '';
  return label.trim().replace(/\s+/g, ' ').slice(0, 80);
};
"#;

/// Finding an element the way a person names it: by what it is (its role)
/// and what it says (its name), with the words of the row or card around it
/// when several say the same. A recorded UI test keeps that instead of the
/// element's place in the page, which moves when a row is added above it.
///
/// `describe(el)` is what a recording keeps for an element; `locate(spec)`
/// finds it again. Needs [`SELECTOR_OF`] before it.
const LOCATE: &str = r#"
const __clean = (s) => (s || '').trim().replace(/\s+/g, ' ').slice(0, 80);
const __seen = (el) => {
  if (!el || !el.isConnected) return false;
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return false;
  const s = getComputedStyle(el);
  return s.visibility !== 'hidden' && s.display !== 'none';
};
const roleOf = (el) => {
  const given = (el.getAttribute('role') || '').trim().split(/\s+/)[0];
  if (given) return given;
  const tag = el.localName;
  if (tag === 'button' || tag === 'summary') return 'button';
  if (tag === 'a') return el.hasAttribute('href') ? 'link' : '';
  if (tag === 'select') return 'combobox';
  if (tag === 'textarea') return 'textbox';
  if (tag === 'input') {
    const type = (el.getAttribute('type') || 'text').toLowerCase();
    if (['button', 'submit', 'reset', 'image'].includes(type)) return 'button';
    if (type === 'checkbox' || type === 'radio') return type;
    if (type === 'range') return 'slider';
    if (type === 'search') return 'searchbox';
    if (type === 'number') return 'spinbutton';
    if (['hidden', 'file', 'color'].includes(type)) return '';
    return 'textbox';
  }
  if (/^h[1-6]$/.test(tag)) return 'heading';
  if (tag === 'img') return 'img';
  if (tag === 'li') return 'listitem';
  if (tag === 'tr') return 'row';
  if (tag === 'option') return 'option';
  return '';
};
// What is typed into a field is never its name (see `labelOf`).
const nameOf = (el) => {
  const aria = el.getAttribute('aria-label');
  if (aria && aria.trim()) return __clean(aria);
  const by = el.getAttribute('aria-labelledby');
  if (by) {
    const text = by.split(/\s+/).map((id) => document.getElementById(id)).filter(Boolean).map((n) => n.innerText).join(' ');
    if (text.trim()) return __clean(text);
  }
  const tag = el.localName;
  if (tag === 'input' || tag === 'textarea' || tag === 'select') {
    const type = (el.getAttribute('type') || '').toLowerCase();
    if (tag === 'input' && ['submit', 'button', 'reset'].includes(type)) return __clean(el.value || type);
    if (el.labels && el.labels.length) return __clean(el.labels[0].innerText);
    return __clean(el.placeholder || el.title);
  }
  if (tag === 'img') return __clean(el.alt || el.title);
  return __clean(el.innerText || el.title);
};
const __ROLES = {
  button: 'button, summary, input[type=button], input[type=submit], input[type=reset], input[type=image], [role~=button]',
  link: 'a[href], [role~=link]',
  textbox: 'input, textarea, [role~=textbox]',
  searchbox: 'input[type=search], [role~=searchbox]',
  checkbox: 'input[type=checkbox], [role~=checkbox]',
  radio: 'input[type=radio], [role~=radio]',
  combobox: 'select, [role~=combobox]',
  heading: 'h1, h2, h3, h4, h5, h6, [role~=heading]',
  img: 'img, [role~=img]',
  listitem: 'li, [role~=listitem]',
  row: 'tr, [role~=row]',
  option: 'option, [role~=option]',
  slider: 'input[type=range], [role~=slider]',
  spinbutton: 'input[type=number], [role~=spinbutton]',
};
const __ofRole = (role) => {
  let query = __ROLES[role];
  if (!query) { try { query = `[role~="${CSS.escape(role)}"]`; } catch (e) { return []; } }
  return [...document.querySelectorAll(query)].filter((el) => roleOf(el) === role && __seen(el));
};
// The elements that are (a `role`, when given, and) named `name`, in page order.
const __named = (role, name) => {
  if (role) return __ofRole(role).filter((el) => nameOf(el) === name);
  // By its words alone: the smallest elements that say exactly this.
  const found = [];
  if (!document.body) return found;
  const need = Math.min(name.length, 80);
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!node.data.trim()) continue;
    let el = node.parentElement;
    while (el && el !== document.body && __clean(el.textContent).length < need) el = el.parentElement;
    if (el && el !== document.body && !found.includes(el) && __seen(el) && __clean(el.innerText) === name) found.push(el);
  }
  return found;
};
// How much the smallest thing around `el` that says `within` says in all:
// the less, the closer those words are to it.
const __near = (el, within) => {
  for (let a = el.parentElement; a; a = a.parentElement) {
    const text = (a.innerText || '').replace(/\s+/g, ' ');
    if (text.includes(within)) return text.length;
  }
  return Infinity;
};
const __what = (spec) => {
  const said = spec.name ? ` “${spec.name}”` : '';
  const kind = spec.role || 'element';
  return `${kind}${said}${spec.within ? ` by “${spec.within}”` : ''}`;
};
// A selector that names a hook put there for tests.
const __hook = (selector) => /^\[data-(testid|test|cy)=/.test(selector || '');
const locate = (spec) => {
  const selector = spec.selector || '';
  const bySelector = () => {
    try { const el = document.querySelector(selector); return __seen(el) ? el : null; } catch (e) { return null; }
  };
  if (__hook(selector)) { const el = bySelector(); if (el) return { el }; }
  if (spec.name) {
    let found = __named(spec.role || '', spec.name);
    if (spec.within) {
      const sizes = found.map((el) => __near(el, spec.within));
      const best = Math.min(Infinity, ...sizes);
      found = best === Infinity ? [] : found.filter((_, i) => sizes[i] === best);
    }
    if (spec.nth != null) { if (found[spec.nth]) return { el: found[spec.nth] }; }
    else if (found.length === 1) return { el: found[0] };
    else if (found.length > 1) return { reason: `${__what(spec)} is there ${found.length} times, and the test does not say which one` };
    // Its own id still finds it when its words changed; its place in the
    // page does not count (another element may stand there now).
    if (/^#[^\s>+~]+$/.test(selector)) { const el = bySelector(); if (el) return { el }; }
    const others = spec.role ? [...new Set(__ofRole(spec.role).map(nameOf).filter(Boolean))].slice(0, 8) : [];
    return { reason: `no ${__what(spec)} shows`, others };
  }
  if (selector) { const el = bySelector(); if (el) return { el }; }
  return { reason: selector ? `no element matches \`${selector}\`` : 'the step names no element' };
};
// What a recording keeps for `el`: its role and name, told apart from others
// of the same name by words around it (else by its turn), and its selector.
const describe = (el) => {
  const selector = selectorOf(el);
  const role = roleOf(el) || undefined;
  const name = nameOf(el);
  // Without a role, only a few words are a name (not a paragraph).
  if (!name || (!role && name.length > 60)) return { role, selector };
  const spec = { role, name, selector };
  const same = __named(role || '', name);
  if (same.length > 1) {
    let within;
    search: for (let a = el.parentElement, up = 0; a && a !== document.body && up < 6; a = a.parentElement, up++) {
      const walker = document.createTreeWalker(a, NodeFilter.SHOW_TEXT);
      let tried = 0;
      for (let node = walker.nextNode(); node && tried < 12; node = walker.nextNode()) {
        const words = __clean(node.data);
        if (words.length < 2 || words.length > 60 || words === name || el.contains(node)) continue;
        tried++;
        if (locate({ role, name, within: words }).el === el) { within = words; break search; }
      }
    }
    if (within) spec.within = within; else spec.nth = same.indexOf(el);
  }
  return locate(spec).el === el ? spec : { role, selector };
};
// What a click at a point is a click on: the control around it.
const __control = (el) => (el && el.closest('a[href], button, summary, input, select, textarea, label, [role~=button], [role~=link], [role~=tab], [role~=menuitem], [role~=checkbox], [role~=radio], [role~=switch], [role~=option], [onclick]')) || el;
"#;

/// What a recording keeps for the element a click lands on: the one
/// matching `selector`, else the control at CSS point (`x`, `y`). `null`
/// when there is none.
pub fn describe_click(selector: Option<&str>, x: f32, y: f32) -> String {
    let pick = match selector {
        Some(selector) => format!(
            "document.querySelector({})",
            serde_json::to_string(selector).unwrap_or_default()
        ),
        None => format!("__control(document.elementFromPoint({x}, {y}))"),
    };
    format!(
        "(() => {{ {SELECTOR_OF} {LOCATE}\n  let el = null; try {{ el = {pick}; }} catch (e) {{}}\n  return el ? describe(el) : null;\n}})()"
    )
}

/// What a recording keeps for the field in focus, when it is one (typing
/// elsewhere, in an editor say, is replayed where the focus is).
pub fn describe_focused() -> String {
    format!(
        "(() => {{ {SELECTOR_OF} {LOCATE}\n  const el = document.activeElement;\n  return el && (el.localName === 'input' || el.localName === 'textarea') ? describe(el) : null;\n}})()"
    )
}

/// Finds the element `spec` names (`role`, `name`, `within`, `nth`,
/// `selector`) once it can be used: shown, switched on, at rest and, for a
/// click, not under something else. Scrolls it into view.
///
/// `{ ok: true, x, y }` (its middle), or `{ ok: false, reason, others }`.
/// With `focus`, it is focused (and its text selected) instead.
pub fn resolve(spec: &serde_json::Value, focus: bool) -> String {
    format!(
        r#"(async () => {{ {SELECTOR_OF} {LOCATE}
  const spec = {spec};
  const hit = locate(spec);
  if (!hit.el) return {{ ok: false, reason: hit.reason, others: hit.others || [] }};
  const el = hit.el;
  if (el.disabled || el.getAttribute('aria-disabled') === 'true') return {{ ok: false, reason: `the ${{__what(spec)}} is switched off` }};
  el.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
  const box = () => {{ const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; }};
  const before = box();
  // Two frames (or a moment, where no frames are drawn): an element still
  // sliding in is not clicked where it was.
  await Promise.race([
    new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
    new Promise((done) => setTimeout(done, 120)),
  ]);
  const after = box();
  if (before.some((v, i) => Math.abs(v - after[i]) > 0.5)) return {{ ok: false, reason: `the ${{__what(spec)}} is still moving` }};
  if ({focus}) {{ el.focus(); if ('select' in el) el.select(); return {{ ok: true }}; }}
  const x = after[0] + after[2] / 2, y = after[1] + after[3] / 2;
  const top = document.elementFromPoint(x, y);
  // A field's label (a styled checkbox, say) stands for the field.
  const label = el.closest('label') || (el.labels && el.labels[0]);
  if (top && top !== el && !el.contains(top) && !top.contains(el) && !(label && label.contains(top))) {{
    const over = nameOf(top) || top.localName;
    return {{ ok: false, reason: `the ${{__what(spec)}} is covered by “${{over.slice(0, 40)}}”` }};
  }}
  return {{ ok: true, x, y }};
}})()"#
    )
}

/// Everything the page says (all of it, where [`snapshot`] keeps the
/// start), for a test's checks.
pub const PAGE_TEXT: &str = r#"({ url: location.href, text: (document.body ? document.body.innerText : '').slice(0, 2000000) })"#;

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
    fn an_element_is_found_as_the_test_names_it() {
        let spec = serde_json::json!({ "role": "button", "name": "Sign \"in\"" });
        let script = resolve(&spec, false);
        assert!(script.contains(r#""name":"Sign \"in\"""#), "{script}");
        assert!(script.contains("if (false) { el.focus()"));
        assert!(resolve(&spec, true).contains("if (true) { el.focus()"));
        assert!(describe_click(None, 3.0, 4.0).contains("elementFromPoint(3, 4)"));
        assert!(
            describe_click(Some("a[href=\"x\"]"), 0., 0.)
                .contains(r#"document.querySelector("a[href=\"x\"]")"#)
        );
    }

    #[test]
    fn selectors_are_quoted_safely() {
        let script = locate(r#"a[href="x"]"#);
        assert!(script.contains(r#"document.querySelector("a[href=\"x\"]")"#));
        assert!(pick(1.0, 2.0).contains("elementFromPoint(1, 2)"));
    }
}
