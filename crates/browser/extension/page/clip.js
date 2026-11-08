// Runs in the extension's isolated world of a page, only when the user asks
// (Send to PilotHouse): the page's main text, or the selection, as Markdown.
// The page's own scripts cannot see or change it.
(() => {
  const ns = (globalThis.__pilothouse ??= {});
  if (ns.clip) return;

  /** The most text sent, in characters. */
  const MAX = 200000;
  const SKIP = new Set(['script', 'style', 'noscript', 'template', 'svg', 'canvas', 'iframe',
    'object', 'embed', 'button', 'select', 'input', 'textarea', 'dialog', 'video', 'audio']);
  const BOILERPLATE = new Set(['nav', 'header', 'footer', 'aside', 'form']);

  const hidden = (el) => {
    if (el.hidden || el.getAttribute('aria-hidden') === 'true') return true;
    const style = el.ownerDocument.defaultView?.getComputedStyle(el);
    return !!style && (style.display === 'none' || style.visibility === 'hidden');
  };

  const absolute = (href) => {
    try {
      return new URL(href, document.baseURI).href;
    } catch (error) {
      return href;
    }
  };

  const inline = (text) => text.replace(/\s+/g, ' ');

  /** The part of the page that holds its content. */
  const mainOf = () => {
    for (const selector of ['main', 'article', '[role=main]', '#content', '#main', '.content', '.markdown-body']) {
      const found = document.querySelector(selector);
      if (found && (found.innerText || '').trim().length > 200) return found;
    }
    return document.body || document.documentElement;
  };

  class Writer {
    constructor(boilerplate) {
      this.out = [];
      this.boilerplate = boilerplate;
      this.length = 0;
      this.full = false;
    }
    push(text) {
      if (this.full) return;
      this.length += text.length;
      if (this.length > MAX) {
        this.full = true;
        return;
      }
      this.out.push(text);
    }
    block(text) {
      const trimmed = text.replace(/[ \t]+\n/g, '\n').trim();
      if (trimmed) this.push(`\n\n${trimmed}\n\n`);
    }
    text() {
      return this.out.join('').replace(/\n{3,}/g, '\n\n').trim();
    }
  }

  /** Inline content of `node` as Markdown. */
  const inlineOf = (node, writer, live) => {
    let text = '';
    for (const child of node.childNodes) {
      if (child.nodeType === Node.TEXT_NODE) {
        text += inline(child.textContent);
        continue;
      }
      if (child.nodeType !== Node.ELEMENT_NODE) continue;
      const tag = child.localName;
      if (SKIP.has(tag) || (live && hidden(child))) continue;
      switch (tag) {
        case 'br':
          text += '  \n';
          break;
        case 'a': {
          const inner = inlineOf(child, writer, live).trim();
          const href = child.getAttribute('href');
          text += href && !href.startsWith('javascript:') && inner
            ? `[${inner}](${absolute(href)})`
            : inner;
          break;
        }
        case 'img': {
          const src = child.getAttribute('src');
          if (src && !src.startsWith('data:')) text += `![${inline(child.alt || '')}](${absolute(src)})`;
          break;
        }
        case 'code':
        case 'kbd':
        case 'samp': {
          const code = child.textContent.replace(/\s+/g, ' ');
          if (code) text += code.includes('`') ? `\`\` ${code} \`\`` : `\`${code}\``;
          break;
        }
        case 'strong':
        case 'b': {
          const inner = inlineOf(child, writer, live).trim();
          if (inner) text += `**${inner}**`;
          break;
        }
        case 'em':
        case 'i': {
          const inner = inlineOf(child, writer, live).trim();
          if (inner) text += `*${inner}*`;
          break;
        }
        default:
          text += inlineOf(child, writer, live);
      }
    }
    return text;
  };

  const BLOCKS = new Set(['p', 'div', 'section', 'article', 'main', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'ul', 'ol', 'li', 'pre', 'blockquote', 'table', 'hr', 'figure', 'figcaption', 'dl', 'dt', 'dd',
    'header', 'footer', 'nav', 'aside', 'details', 'summary']);

  const hasBlocks = (el) => [...el.children].some((child) => BLOCKS.has(child.localName));

  const listOf = (list, writer, live, depth) => {
    const ordered = list.localName === 'ol';
    let n = Number(list.getAttribute('start')) || 1;
    const lines = [];
    for (const item of list.children) {
      if (item.localName !== 'li' || (live && hidden(item))) continue;
      const nested = [];
      const own = item.cloneNode(true);
      for (const sub of own.querySelectorAll(':scope > ul, :scope > ol')) sub.remove();
      const marker = ordered ? `${n++}.` : '-';
      lines.push(`${'  '.repeat(depth)}${marker} ${inlineOf(own, writer, false).trim()}`);
      for (const sub of item.querySelectorAll(':scope > ul, :scope > ol')) {
        nested.push(listOf(sub, writer, live, depth + 1));
      }
      lines.push(...nested.filter(Boolean));
    }
    return lines.join('\n');
  };

  const tableOf = (table, writer) => {
    const rows = [...table.querySelectorAll('tr')].slice(0, 200).map((row) =>
      [...row.children].map((cell) => inlineOf(cell, writer, false).trim().replace(/\|/g, '\\|')));
    if (!rows.length) return '';
    const width = Math.max(...rows.map((row) => row.length));
    const pad = (row) => [...row, ...Array(width - row.length).fill('')];
    const [head, ...body] = rows;
    return [
      `| ${pad(head).join(' | ')} |`,
      `| ${Array(width).fill('---').join(' | ')} |`,
      ...body.map((row) => `| ${pad(row).join(' | ')} |`),
    ].join('\n');
  };

  const walk = (node, writer, live) => {
    if (writer.full) return;
    for (const child of node.childNodes) {
      if (writer.full) return;
      if (child.nodeType === Node.TEXT_NODE) {
        const text = inline(child.textContent).trim();
        if (text) writer.push(` ${text} `);
        continue;
      }
      if (child.nodeType !== Node.ELEMENT_NODE) continue;
      const tag = child.localName;
      if (SKIP.has(tag) || (live && hidden(child))) continue;
      if (writer.boilerplate && BOILERPLATE.has(tag)) continue;
      const heading = /^h([1-6])$/.exec(tag);
      if (heading) {
        writer.block(`${'#'.repeat(Number(heading[1]))} ${inlineOf(child, writer, live).trim()}`);
      } else if (tag === 'p' || tag === 'figcaption' || tag === 'dt' || tag === 'dd' || tag === 'summary') {
        writer.block(inlineOf(child, writer, live));
      } else if (tag === 'ul' || tag === 'ol') {
        writer.block(listOf(child, writer, live, 0));
      } else if (tag === 'pre') {
        const code = child.querySelector('code');
        const language = /(?:language|lang)-([\w+#-]+)/.exec(`${code?.className ?? ''} ${child.className}`)?.[1] ?? '';
        const text = (code ?? child).textContent.replace(/\n+$/, '');
        const fence = text.includes('```') ? '~~~~' : '```';
        writer.block(`${fence}${language}\n${text}\n${fence}`);
      } else if (tag === 'blockquote') {
        const inner = new Writer(false);
        walk(child, inner, live);
        writer.block(inner.text().split('\n').map((line) => `> ${line}`).join('\n'));
      } else if (tag === 'table') {
        writer.block(tableOf(child, writer));
      } else if (tag === 'hr') {
        writer.block('---');
      } else if (tag === 'img' || tag === 'a' || !hasBlocks(child) && !BLOCKS.has(tag)) {
        const text = inlineOf({ childNodes: [child] }, writer, live).trim();
        if (text) writer.push(` ${text} `);
      } else if (!hasBlocks(child)) {
        writer.block(inlineOf(child, writer, live));
      } else {
        walk(child, writer, live);
      }
    }
  };

  /** The selection's content, as a detached copy. */
  const selection = () => {
    const sel = getSelection();
    if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
    const box = document.createElement('div');
    for (let i = 0; i < sel.rangeCount; i += 1) box.appendChild(sel.getRangeAt(i).cloneContents());
    return { box, plain: sel.toString() };
  };

  ns.clip = ({ selectionOnly } = {}) => {
    const base = { title: document.title, url: location.href };
    const picked = selection();
    if (picked) {
      const writer = new Writer(false);
      walk(picked.box, writer, false);
      const markdown = writer.text() || picked.plain.trim();
      return { ...base, kind: 'selection', markdown, truncated: writer.full };
    }
    if (selectionOnly) return { ...base, kind: 'selection', markdown: '' };
    const writer = new Writer(true);
    walk(mainOf(), writer, true);
    let markdown = writer.text();
    if (!markdown) markdown = (document.body?.innerText ?? '').slice(0, MAX);
    const description = document.querySelector('meta[name=description]')?.content?.trim();
    return { ...base, kind: 'page', markdown, description: description || '', truncated: writer.full };
  };
})();
