// PilotHouse's buttons on GitHub, Jira and Linear pages, only on the sites
// the user turned on in the PilotHouse panel (each with Chrome's own
// permission prompt). A small pill at the page's bottom right offers what
// fits the page: review a pull request, start a chat from an issue or a
// failed CI run, or add the issue to the workspace's board. Nothing is
// sent until the user presses a button; × hides the pill on this page.
(() => {
  const ns = (globalThis.__pilothouse ??= {});
  if (ns.buttons) return;
  ns.buttons = true;

  const CSS = `
    :host { all: initial; }
    .pill { position: fixed; right: 16px; bottom: 16px; display: flex; align-items: center; gap: 6px;
      padding: 6px 6px 6px 10px; border-radius: 999px; background: rgba(20, 18, 32, 0.94); color: white;
      font: 12px/16px system-ui, -apple-system, "Segoe UI", sans-serif; box-shadow: 0 6px 24px rgba(0,0,0,0.3); }
    .mark { width: 8px; height: 8px; border-radius: 50%; background: rgb(139, 92, 246); }
    .name { font-weight: 600; margin-right: 2px; }
    button { font: 600 12px/16px system-ui, sans-serif; border-radius: 999px; padding: 4px 10px; cursor: pointer;
      border: 1px solid rgba(255,255,255,0.25); background: transparent; color: white; }
    button:hover { border-color: rgb(167, 139, 250); }
    button.close { border: none; padding: 4px 6px; color: #b9b5c8; }
    button:focus-visible { outline: 2px solid white; outline-offset: 1px; }
    button:disabled { opacity: 0.5; cursor: default; }
    .status { max-width: 280px; color: #d6d3e3; }
    .status.error { color: #fca5a5; }
  `;

  /** What the page is, by its address: a pull request, an issue, a run. */
  const route = () => {
    const { hostname, pathname, search } = location;
    let m;
    if (hostname === 'github.com') {
      if ((m = /^\/([^/]+)\/([^/]+)\/pull\/(\d+)/.exec(pathname))) {
        return { site: 'GitHub', kind: 'pull', owner: m[1], repo: m[2], number: Number(m[3]) };
      }
      if ((m = /^\/([^/]+)\/([^/]+)\/issues\/(\d+)/.exec(pathname))) {
        return { site: 'GitHub', kind: 'issue', key: `${m[1]}/${m[2]}#${m[3]}` };
      }
      if ((m = /^\/([^/]+)\/([^/]+)\/actions\/runs\/(\d+)/.exec(pathname))) {
        return { site: 'GitHub', kind: 'run', key: `${m[1]}/${m[2]} run ${m[3]}`, run: m[3], owner: m[1], repo: m[2] };
      }
      return null;
    }
    if (hostname.endsWith('.atlassian.net')) {
      m = /\/browse\/([A-Z][A-Z0-9_]+-\d+)/.exec(pathname) || /[?&]selectedIssue=([A-Z][A-Z0-9_]+-\d+)/.exec(search);
      return m ? { site: 'Jira', kind: 'issue', key: m[1] } : null;
    }
    if (hostname === 'linear.app') {
      m = /^\/[^/]+\/issue\/([A-Z0-9]+-\d+)/.exec(pathname);
      return m ? { site: 'Linear', kind: 'issue', key: m[1] } : null;
    }
    return null;
  };

  const ACTIONS = {
    pull: [
      { id: 'review', label: 'Review in PilotHouse', title: 'Open this pull request in PilotHouse to review it' },
      { id: 'chat', label: 'Ask about it', title: 'A new chat about this pull request' },
    ],
    issue: [
      { id: 'chat', label: 'Start a chat', title: 'A new chat about this issue, for you to send' },
      { id: 'card', label: 'Add to board', title: 'A card on the board of the workspace on screen' },
    ],
    run: [
      { id: 'chat', label: 'Fix this failure', title: 'A new chat to find why this run failed, for you to send' },
    ],
  };

  /** The page's title, without the site's suffix. */
  const titleOf = () => document.title.replace(/\s+[·|–-]\s+(GitHub|Jira|Linear).*$/i, '').trim();

  const pageText = () => {
    try {
      const clip = ns.clip ? ns.clip({}) : null;
      return (clip?.markdown || document.body?.innerText || '').slice(0, 60000);
    } catch (error) {
      return '';
    }
  };

  const payload = (page, action) => {
    const title = titleOf();
    const base = { url: location.href, title, site: page.site, key: page.key ?? null };
    if (action === 'review') {
      return { ...base, kind: 'review', host: location.hostname, owner: page.owner, repo: page.repo, number: page.number };
    }
    if (action === 'card') return { ...base, kind: 'card', text: pageText() };
    const prompt = page.kind === 'run'
      ? `This CI run failed (${location.href}). Find out why (gh run view ${page.run} --log-failed shows the failing steps' logs), then fix the cause. Tell me the cause before changing a lot.`
      : page.kind === 'pull'
        ? 'Here is a pull request. Summarise what it changes and what to look at closely.'
        : `Here is an issue from ${page.site}. Read it, find where it lives in the code, and propose how to fix it before changing anything.`;
    return { ...base, kind: 'chat', text: pageText(), prompt };
  };

  let shownFor = null;
  let hiddenFor = null;
  let host = null;

  const element = (root, tag, cls, text) => {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text) node.textContent = text;
    root.appendChild(node);
    return node;
  };

  const remove = () => {
    host?.remove();
    host = null;
    shownFor = null;
  };

  const show = (page) => {
    remove();
    host = document.createElement('pilothouse-buttons');
    host.style.setProperty('position', 'fixed', 'important');
    host.style.setProperty('z-index', '2147483646', 'important');
    host.style.setProperty('right', '0', 'important');
    host.style.setProperty('bottom', '0', 'important');
    const root = host.attachShadow({ mode: 'closed' });
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    root.adoptedStyleSheets = [sheet];
    const pill = element(root, 'div', 'pill');
    pill.setAttribute('role', 'region');
    pill.setAttribute('aria-label', 'PilotHouse');
    element(pill, 'span', 'mark');
    element(pill, 'span', 'name', 'PilotHouse');
    const status = element(pill, 'span', 'status');
    const buttons = [];
    for (const action of ACTIONS[page.kind]) {
      const button = element(pill, 'button', '', action.label);
      button.title = action.title;
      button.addEventListener('click', async () => {
        for (const b of buttons) b.disabled = true;
        status.className = 'status';
        status.textContent = 'Sending…';
        try {
          const answer = await chrome.runtime.sendMessage({ type: 'site-action', action: payload(page, action.id) });
          if (!answer?.ok) throw new Error(answer?.error ?? 'PilotHouse did not answer');
          status.textContent = `Done: ${answer.result?.where ?? 'in PilotHouse'}.`;
        } catch (error) {
          status.className = 'status error';
          status.textContent = String(error?.message ?? error).slice(0, 200);
        }
        for (const b of buttons) b.disabled = false;
        setTimeout(() => {
          if (status.isConnected) status.textContent = '';
        }, 6000);
      });
      buttons.push(button);
    }
    const close = element(pill, 'button', 'close', '×');
    close.title = 'Hide on this page';
    close.setAttribute('aria-label', 'Hide PilotHouse’s buttons on this page');
    close.addEventListener('click', () => {
      hiddenFor = location.href;
      remove();
    });
    document.documentElement.appendChild(host);
    shownFor = location.href;
  };

  // These sites change pages without loading them: look again as the
  // address changes.
  const look = () => {
    if (location.href === shownFor) return;
    const page = route();
    if (!page || location.href === hiddenFor) {
      remove();
      return;
    }
    show(page);
  };
  look();
  setInterval(look, 1000);
})();
