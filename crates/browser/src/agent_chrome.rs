//! Agents' Chrome: the user's own Chrome (or Edge, Brave, Chromium), reached
//! through PilotHouse's Chrome extension, for agents to use websites as a
//! person would, with the user's sign-ins.
//!
//! - **Only the PilotHouse tab group.** Agents open their tabs in a tab group
//!   named "PilotHouse"; the user shares a tab of theirs by putting it in that
//!   group (or with the extension's toolbar button). Tabs outside it are never
//!   listed, read or touched, and a tab taken out of the group is let go.
//! - **A tab per agent** (each agent session is an *owner*): tabs it opens are
//!   its own; a tab one of its pages opens (a sign-in popup) becomes its
//!   current tab; a shared tab it takes over is given back, not closed.
//! - **Visible control**: while an agent acts, its tab glows, a pointer goes
//!   where it clicks, and a bar names the agent with a Stop button. Stop
//!   pauses that agent here until the user lets it continue.
//! - **Attached only while used.** The extension attaches Chrome's debugger
//!   to a tab when an agent acts in it, and lets go once it has been idle a
//!   while ([`IDLE_AFTER`]), so Chrome's "started debugging this browser" bar
//!   is not up all the time.
//! - **Logs** per tab, while attached: console messages and errors with where
//!   they came from, and every request with its headers (credentials hidden),
//!   timing, size and, on request, its body.
//!
//! The extension reaches the app only through Chrome's native messaging and
//! the local bridge ([`crate::extension`], [`crate::bridge`]): there is no
//! network port, and it passes on only the DevTools methods used here.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::FutureExt as _;
use futures::future::Shared;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, SharedString, Task,
    WeakEntity,
};
use ph_store::Store;
use serde_json::{Value, json};

use crate::agent_scripts::{self, AGENT, BINDING, OVERLAY, WORLD};
use crate::bridge::{self, BridgeEvent};
use crate::cdp::{Cdp, CdpEvent, CdpInbox};
use crate::extension;
use crate::logs::{Level, Logs};

mod devtools;
mod inbox;
mod shared_text;
pub use inbox::{BrowserHost, ChatNews, ChatNote, Delivery, SendTo};
pub use shared_text::TabText;

/// How long navigation waits for the page's load event.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// The glow stays this long after an agent's last action: long enough to
/// stay on while the agent thinks between steps, so it does not flicker.
const GLOW_AFTER: Duration = Duration::from_secs(20);
/// The debugger lets go of a tab idle this long (and not paused).
const IDLE_AFTER: Duration = Duration::from_secs(30);
/// The pointer's glide to where it clicks (the overlay animates it).
const POINTER_GLIDE: Duration = Duration::from_millis(230);
/// The longest a screenshot may take before the tab is brought forward.
const SHOT_TIMEOUT: Duration = Duration::from_secs(12);
/// Most of a response body returned to an agent.
const MAX_BODY: usize = 64 * 1024;
/// Most of a script's result returned to an agent.
const MAX_VALUE: usize = 100 * 1024;
/// Elements listed when reading a page.
const SNAPSHOT_ELEMENTS: usize = 250;

/// What agents are told when a page wants something only the user does.
const ASK_TO_SIGN_IN: &str = "That is a password field: agents do not type passwords. Ask the user to sign in themselves in that tab of their Chrome, then go on.";

#[derive(Debug, Clone, PartialEq)]
pub enum AgentChromeStatus {
    /// No browser's extension is connected.
    Disconnected,
    /// The extension in this browser is connected (`Chrome 131`).
    Connected { browser: SharedString },
    /// The bridge could not listen (it says why).
    Failed(SharedString),
}

pub enum AgentChromeEvent {
    Changed,
}

/// Who acts: an agent session (its key) and how to name it to the user.
#[derive(Debug, Clone)]
pub struct Who {
    pub owner: String,
    pub label: String,
}

/// An element, as an agent names it.
#[derive(Debug, Clone)]
pub enum Target {
    /// A number from the last read of the page, a CSS selector, or text on it.
    Element(Value),
    /// A point in the viewport, in CSS pixels.
    Point(f64, f64),
}

/// A JavaScript dialog waiting for an answer.
#[derive(Debug, Clone)]
pub struct PageDialog {
    pub kind: String,
    pub message: String,
    pub default_prompt: String,
}

impl PageDialog {
    fn waiting(&self) -> anyhow::Error {
        anyhow!(
            "A {} dialog is open: “{}”. Answer it with chrome_dialog (accept or dismiss) before doing anything else on this page.",
            self.kind,
            self.message
        )
    }
}

/// A file an agent's page downloaded. Chrome saves it where it saves the
/// user's downloads.
#[derive(Debug, Clone)]
pub struct Download {
    pub url: String,
    pub name: String,
    /// Where it was saved, once found there.
    pub path: Option<PathBuf>,
    /// `inProgress`, `completed` or `canceled`.
    pub state: String,
    pub received: u64,
    pub total: u64,
    /// When it began: only a file written since then can be it.
    pub started: std::time::SystemTime,
}

/// A connected extension: one native messaging host, in one browser.
struct Link {
    /// The browser connection; a tab's session is `for_session(tab id)`.
    browser: Cdp,
    inbox: CdpInbox,
    writer: bridge::Writer,
    _events: Task<()>,
}

struct Owner {
    label: String,
    /// Its tabs, oldest first.
    tabs: Vec<String>,
    current: Option<String>,
    /// The user pressed Stop on its tab.
    paused: bool,
    /// The browser went away with its tabs open.
    lost: bool,
    /// Said with the next result (a popup became the current tab…).
    notes: Vec<String>,
}

struct Tab {
    /// Chrome's tab id, which is also its session with the extension.
    target: String,
    /// A tab the user shared that the agent took over: given back, not
    /// closed, when the agent is done with it.
    shared: bool,
    /// Chrome's debugger is attached to it (let go when idle).
    attached: bool,
    /// Actions running in it now: the debugger stays meanwhile.
    busy: Arc<AtomicUsize>,
    cdp: Cdp,
    owner: String,
    url: String,
    title: String,
    main_frame: Option<String>,
    loading: bool,
    /// The latest loads that finished, for a wait that starts late.
    loaded: VecDeque<String>,
    load_waiters: Vec<(String, smol::channel::Sender<()>)>,
    logs: Logs,
    dialog: Option<PageDialog>,
    dialog_waiters: Vec<smol::channel::Sender<PageDialog>>,
    /// The isolated world's execution context in the current document.
    world: Option<i64>,
    /// Console and network records the agent has been told about.
    console_seen: u64,
    network_seen: u64,
    /// Records already counted in an action's result.
    hinted: u64,
    /// When its page last opened a window (a link to a new tab, a popup):
    /// only then is a new tab from it the agent's.
    opened_window: Option<Instant>,
    /// The debugger being let go of (idle): attaching again waits for it,
    /// or its detach, arriving after, would undo the attach.
    letting_go: Option<Shared<Task<()>>>,
    /// What the agent emulates in it (a device, a network…), and its
    /// mocked responses: they end when the debugger lets go.
    emulating: Vec<String>,
    mocks: Vec<devtools::Mock>,
    /// The debugger stays at least until then (emulation and mocks).
    keep_until: Option<Instant>,
    /// A heap snapshot being read, and the last one's summary.
    heap: Option<String>,
    heap_too_large: bool,
    heap_summary: Option<devtools::HeapSummary>,
    _glow: Option<Task<()>>,
    _idle: Option<Task<()>>,
}

pub struct AgentChrome {
    store: Store,
    status: AgentChromeStatus,
    links: HashMap<u64, Link>,
    /// The link agents use: the first to say hello. One browser at a time.
    active: Option<u64>,
    tabs: Vec<Tab>,
    owners: HashMap<String, Owner>,
    downloads: Vec<(String, Download)>,
    /// Where what the extension brings goes (set by the workspace shell).
    host: Option<BrowserHost>,
    /// Chats waiting for the user, and at work, for the extension.
    chats: (Vec<ChatNote>, Vec<ChatNote>),
    /// What the extension was last told of them.
    sent_status: Option<Value>,
    /// When each tab was last handed to the user.
    handed_off: HashMap<String, Instant>,
    _bridge: Option<Task<()>>,
}

impl EventEmitter<AgentChromeEvent> for AgentChrome {}

struct AgentChromeHub(Entity<AgentChrome>);

impl Global for AgentChromeHub {}

/// What an action works on: the agent's tab and its connection. While it
/// lives, the tab counts as in use.
#[derive(Clone)]
struct Page {
    cdp: Cdp,
    target: String,
    /// The agent session it is for.
    owner: String,
    label: String,
    _busy: Arc<Busy>,
}

/// Counts an action in its tab for as long as it lives.
struct Busy(Arc<AtomicUsize>);

impl Busy {
    fn new(count: &Arc<AtomicUsize>) -> Arc<Self> {
        count.fetch_add(1, Ordering::SeqCst);
        Arc::new(Self(count.clone()))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Tab {
    fn page(&self, owner: &str, label: &str) -> Page {
        Page {
            cdp: self.cdp.clone(),
            target: self.target.clone(),
            owner: owner.to_string(),
            label: label.to_string(),
            _busy: Busy::new(&self.busy),
        }
    }
}

impl AgentChrome {
    pub fn init(store: Store, cx: &mut App) {
        let chrome = cx.new(|cx: &mut Context<Self>| {
            let mut this = Self {
                store,
                status: AgentChromeStatus::Disconnected,
                links: HashMap::new(),
                active: None,
                tabs: Vec::new(),
                owners: HashMap::new(),
                downloads: Vec::new(),
                host: None,
                chats: (Vec::new(), Vec::new()),
                sent_status: None,
                handed_off: HashMap::new(),
                _bridge: None,
            };
            match bridge::listen() {
                Ok(events) => {
                    this._bridge = Some(cx.spawn(async move |this, cx| {
                        while let Ok(event) = events.recv().await {
                            if this
                                .update(cx, |this: &mut Self, cx| this.bridge_event(event, cx))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }));
                }
                Err(err) => {
                    tracing::warn!("agents' Chrome: the extension's channel: {err:#}");
                    this.status = AgentChromeStatus::Failed(format!("{err:#}").into());
                }
            }
            this
        });
        cx.set_global(AgentChromeHub(chrome));
        // The extension's files, for the user to load (Settings says where).
        cx.background_spawn(async {
            if let Err(err) = extension::write_extension() {
                tracing::warn!("writing PilotHouse's Chrome extension: {err:#}");
            }
        })
        .detach();
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<AgentChromeHub>().map(|hub| hub.0.clone())
    }

    pub fn status(&self) -> &AgentChromeStatus {
        &self.status
    }

    /// Agents with a tab open, and their tabs' titles.
    pub fn agents(&self) -> Vec<(String, Vec<String>)> {
        self.owners
            .values()
            .filter(|owner| !owner.tabs.is_empty())
            .map(|owner| {
                let titles = owner
                    .tabs
                    .iter()
                    .filter_map(|target| self.tab(target))
                    .map(|tab| {
                        if tab.title.is_empty() {
                            tab.url.clone()
                        } else {
                            tab.title.clone()
                        }
                    })
                    .collect();
                (owner.label.clone(), titles)
            })
            .collect()
    }

    fn tab(&self, target: &str) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.target == target)
    }

    fn tab_mut(&mut self, target: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.target == target)
    }

    // ---- The extension ---------------------------------------------------

    fn bridge_event(&mut self, event: BridgeEvent, cx: &mut Context<Self>) {
        match event {
            BridgeEvent::Opened { id, writer } => {
                let out = writer.clone();
                let sink: crate::cdp::Sink = Arc::new(move |text| bridge::send(&out, text));
                let (browser, inbox, events) = Cdp::with_sink(sink);
                let pump = cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv().await {
                        if this
                            .update(cx, |this, cx| this.link_event(id, event, cx))
                            .is_err()
                        {
                            return;
                        }
                    }
                });
                self.links.insert(
                    id,
                    Link {
                        browser,
                        inbox,
                        writer,
                        _events: pump,
                    },
                );
            }
            BridgeEvent::Message { id, message } => {
                if let Some(link) = self.links.get(&id) {
                    link.inbox.deliver(message);
                }
            }
            BridgeEvent::Closed { id } => {
                if let Some(link) = self.links.remove(&id) {
                    link.inbox.close();
                }
                if self.active == Some(id) {
                    self.active = None;
                    self.forget_browser(cx);
                }
            }
        }
    }

    fn link_event(&mut self, id: u64, event: CdpEvent, cx: &mut Context<Self>) {
        if event.session.is_none() && event.method == "PilotHouse.hello" {
            let browser: String = event.params["browser"]
                .as_str()
                .unwrap_or("Chrome")
                .chars()
                .take(60)
                .collect();
            match self.active {
                None => {
                    self.active = Some(id);
                    self.status = AgentChromeStatus::Connected {
                        browser: browser.into(),
                    };
                    self.sent_status = None;
                    self.push_status();
                    cx.notify();
                    cx.emit(AgentChromeEvent::Changed);
                }
                // The same browser again, before its old link's close was
                // heard (its extension reloaded, its worker restarted): the
                // old link is stale, and the new one takes over.
                Some(active)
                    if active != id
                        && matches!(
                            &self.status,
                            AgentChromeStatus::Connected { browser: known } if known.as_ref() == browser.as_str()
                        ) =>
                {
                    if let Some(old) = self.links.remove(&active) {
                        old.inbox.close();
                    }
                    self.forget_browser(cx);
                    self.active = Some(id);
                    self.status = AgentChromeStatus::Connected {
                        browser: browser.into(),
                    };
                    self.sent_status = None;
                    self.push_status();
                    cx.notify();
                    cx.emit(AgentChromeEvent::Changed);
                }
                // One browser at a time: another one's extension waits.
                Some(active) if active != id => {
                    if let Some(link) = self.links.remove(&id) {
                        let _ = bridge::send(
                            &link.writer,
                            json!({ "method": "PilotHouse.busy" }).to_string(),
                        );
                        link.inbox.close();
                    }
                }
                Some(_) => {}
            }
            return;
        }
        if self.active == Some(id) {
            self.handle_event(event, cx);
        }
    }

    /// The connection to the browser, when an extension is connected.
    fn browser(&self) -> Result<Cdp> {
        self.active
            .and_then(|id| self.links.get(&id))
            .map(|link| link.browser.clone())
            .ok_or_else(extension::not_connected)
    }

    fn forget_browser(&mut self, cx: &mut Context<Self>) {
        self.tabs.clear();
        for owner in self.owners.values_mut() {
            owner.lost |= !owner.tabs.is_empty();
            owner.tabs.clear();
            owner.current = None;
        }
        if !matches!(self.status, AgentChromeStatus::Failed(_)) {
            self.status = AgentChromeStatus::Disconnected;
        }
        cx.notify();
        cx.emit(AgentChromeEvent::Changed);
    }

    // ---- Events ----------------------------------------------------------

    fn handle_event(&mut self, event: CdpEvent, cx: &mut Context<Self>) {
        let Some(session) = event.session.clone() else {
            self.handle_browser_event(&event, cx);
            return;
        };
        let Some(index) = self.tabs.iter().position(|tab| tab.target == session) else {
            return;
        };
        let params = &event.params;
        match event.method.as_str() {
            "Page.frameNavigated" => {
                let frame = &params["frame"];
                if frame.get("parentId").is_none() {
                    let tab = &mut self.tabs[index];
                    tab.main_frame = frame["id"].as_str().map(str::to_string);
                    let url = frame["url"].as_str().unwrap_or_default().to_string();
                    if url != tab.url && !url.is_empty() && url != "about:blank" {
                        tab.logs.navigated(&url);
                    }
                    tab.url = url;
                    tab.world = None;
                }
            }
            "Page.frameStartedLoading" => {
                let tab = &mut self.tabs[index];
                if params["frameId"].as_str() == tab.main_frame.as_deref() {
                    tab.loading = true;
                }
            }
            "Page.frameStoppedLoading" | "Page.loadEventFired" => {
                let tab = &mut self.tabs[index];
                if event.method == "Page.loadEventFired"
                    || params["frameId"].as_str() == tab.main_frame.as_deref()
                {
                    tab.loading = false;
                }
            }
            "Page.lifecycleEvent" => {
                let tab = &mut self.tabs[index];
                if params["name"] != "load"
                    || params["frameId"].as_str() != tab.main_frame.as_deref()
                {
                    return;
                }
                let loader = params["loaderId"].as_str().unwrap_or_default().to_string();
                tab.load_waiters.retain(|(waiting, done)| {
                    let finished = *waiting == loader;
                    if finished {
                        let _ = done.try_send(());
                    }
                    !finished && !done.is_closed()
                });
                tab.loaded.push_back(loader);
                if tab.loaded.len() > 16 {
                    tab.loaded.pop_front();
                }
            }
            "Runtime.executionContextCreated" => {
                let context = &params["context"];
                let tab = &mut self.tabs[index];
                if context["name"] == WORLD
                    && context["auxData"]["frameId"].as_str() == tab.main_frame.as_deref()
                {
                    tab.world = context["id"].as_i64();
                }
            }
            "Runtime.executionContextDestroyed" => {
                let tab = &mut self.tabs[index];
                if params["executionContextId"].as_i64() == tab.world {
                    tab.world = None;
                }
            }
            "Runtime.executionContextsCleared" => self.tabs[index].world = None,
            "Page.javascriptDialogOpening" => {
                let dialog = PageDialog {
                    kind: params["type"].as_str().unwrap_or("alert").to_string(),
                    message: params["message"].as_str().unwrap_or_default().to_string(),
                    default_prompt: params["defaultPrompt"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                };
                let tab = &mut self.tabs[index];
                for waiter in tab.dialog_waiters.drain(..) {
                    let _ = waiter.try_send(dialog.clone());
                }
                tab.dialog = Some(dialog);
            }
            "Page.javascriptDialogClosed" => self.tabs[index].dialog = None,
            "Fetch.requestPaused" => {
                self.answer_paused(index, params, cx);
                return;
            }
            "HeapProfiler.addHeapSnapshotChunk" => {
                self.heap_chunk(index, params);
                return;
            }
            "Page.windowOpen" => {
                self.tabs[index].opened_window = Some(Instant::now());
                return;
            }
            "Runtime.bindingCalled" if params["name"] == BINDING => {
                let stop = params["payload"] == "stop";
                let owner = self.tabs[index].owner.clone();
                self.set_paused(&owner, stop, cx);
            }
            // Downloads an agent's page started; Chrome saves them where it
            // saves the user's.
            "Page.downloadWillBegin" => {
                let guid = params["guid"].as_str().unwrap_or_default().to_string();
                let name =
                    sanitize_name(params["suggestedFilename"].as_str().unwrap_or("download"));
                self.downloads.push((
                    guid,
                    Download {
                        url: params["url"].as_str().unwrap_or_default().to_string(),
                        name,
                        path: None,
                        state: "inProgress".into(),
                        received: 0,
                        total: 0,
                        started: std::time::SystemTime::now(),
                    },
                ));
                if self.downloads.len() > 100 {
                    self.downloads.remove(0);
                }
                return;
            }
            "Page.downloadProgress" => {
                let Some(guid) = params["guid"].as_str() else {
                    return;
                };
                let Some((_, download)) =
                    self.downloads.iter_mut().find(|(known, _)| known == guid)
                else {
                    return;
                };
                download.state = params["state"].as_str().unwrap_or("inProgress").to_string();
                download.received = params["receivedBytes"].as_f64().unwrap_or(0.0) as u64;
                download.total = params["totalBytes"].as_f64().unwrap_or(0.0) as u64;
                if download.state == "completed" {
                    download.path = downloads_folder().and_then(|folder| {
                        saved_download(&folder, &download.name, download.started, download.received)
                    });
                }
                return;
            }
            // Logs: kept for the agent to ask about; nothing to redraw.
            method => {
                self.tabs[index].logs.record(method, params);
                return;
            }
        }
        cx.notify();
    }

    /// What the extension tells of the PilotHouse group's tabs.
    fn handle_browser_event(&mut self, event: &CdpEvent, cx: &mut Context<Self>) {
        let params = &event.params;
        if self.extension_note(&event.method, params, cx) {
            return;
        }
        let Some(target) = params["tabId"].as_u64().map(|id| id.to_string()) else {
            return;
        };
        match event.method.as_str() {
            // A tab an agent's page opened (a link to a new tab, a sign-in
            // popup) becomes that agent's current tab.
            "PilotHouse.tabCreated" => {
                // Only a tab the agent's page itself just opened: a tab the
                // user opens next to it stays a shared tab of the group.
                let Some(opener) = params["openerTabId"]
                    .as_u64()
                    .and_then(|opener| self.tab_mut(&opener.to_string()))
                    .filter(|tab| {
                        tab.opened_window
                            .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
                    })
                else {
                    cx.emit(AgentChromeEvent::Changed);
                    return;
                };
                opener.opened_window = None;
                let owner = opener.owner.clone();
                if self.tab(&target).is_some() {
                    return;
                }
                let (url, title) = (
                    params["url"].as_str().unwrap_or_default().to_string(),
                    params["title"].as_str().unwrap_or_default().to_string(),
                );
                self.adopt(owner, target, url, title, cx);
            }
            "PilotHouse.tabUpdated" => {
                if let Some(tab) = self.tab_mut(&target) {
                    tab.title = params["title"].as_str().unwrap_or_default().to_string();
                    if let Some(url) = params["url"].as_str() {
                        tab.url = url.to_string();
                    }
                    cx.notify();
                    cx.emit(AgentChromeEvent::Changed);
                }
            }
            "PilotHouse.tabRemoved" => self.forget_tab(&target, cx),
            // The user took the tab out of the group: it is theirs again.
            "PilotHouse.tabLeft" => {
                let Some((tab, _)) = self.remove_tab(&target) else {
                    return;
                };
                if let Some(owner) = self.owners.get_mut(&tab.owner) {
                    owner.notes.push(format!(
                        "The user took your tab ({}) out of the PilotHouse group, so you can no longer use it.",
                        tab.url
                    ));
                }
                cx.notify();
                cx.emit(AgentChromeEvent::Changed);
            }
            "PilotHouse.tabJoined" => cx.emit(AgentChromeEvent::Changed),
            "PilotHouse.detached" => {
                let reason = params["reason"].as_str().unwrap_or_default().to_string();
                let Some(tab) = self.tab_mut(&target) else {
                    return;
                };
                tab.attached = false;
                tab.world = None;
                tab.main_frame = None;
                tab.loading = false;
                tab._idle = None;
                let (owner, url) = (tab.owner.clone(), tab.url.clone());
                self.overrides_ended(&target, "Chrome's debugger let go of the tab");
                // The user closed Chrome's "started debugging" bar.
                if reason == "canceled_by_user"
                    && let Some(owner) = self.owners.get_mut(&owner)
                {
                    owner.notes.push(format!(
                        "The user cancelled PilotHouse's control of your tab ({url}) in Chrome. Ask the user before you use the browser again."
                    ));
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn forget_tab(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some((tab, was_current)) = self.remove_tab(target) else {
            return;
        };
        if was_current && let Some(owner) = self.owners.get_mut(&tab.owner) {
            owner.notes.push(match &owner.current {
                Some(_) => format!(
                    "Your tab ({}) was closed; the one before it is current again.",
                    tab.url
                ),
                None => format!("Your tab ({}) was closed.", tab.url),
            });
        }
        cx.notify();
        cx.emit(AgentChromeEvent::Changed);
    }

    /// Lets go of the tabs of agent sessions that ended (their bridge
    /// process is gone): they stay open for the user, but are no longer
    /// watched, so a page still busy (a live dashboard) costs nothing here.
    fn retire_finished(&mut self, cx: &mut Context<Self>) {
        let ended: Vec<String> = self
            .owners
            .keys()
            .filter(|owner| owner_pid(owner).is_some_and(|pid| !process_alive(pid)))
            .cloned()
            .collect();
        let browser = self.browser().ok();
        for owner in ended {
            let Some(state) = self.owners.remove(&owner) else {
                continue;
            };
            for target in state.tabs {
                let Some((tab, _)) = self.remove_tab(&target) else {
                    continue;
                };
                let Some(browser) = browser.clone() else {
                    continue;
                };
                if tab.attached {
                    cx.background_spawn(let_go(browser, tab.cdp.clone(), tab.world, target))
                        .detach();
                }
            }
        }
    }

    /// Takes a tab out of its agent's; the one before it becomes current.
    /// Says whether it was the agent's current tab.
    fn remove_tab(&mut self, target: &str) -> Option<(Tab, bool)> {
        let index = self.tabs.iter().position(|tab| tab.target == target)?;
        let tab = self.tabs.remove(index);
        let mut was_current = false;
        if let Some(owner) = self.owners.get_mut(&tab.owner) {
            owner.tabs.retain(|known| known != target);
            if owner.current.as_deref() == Some(target) {
                owner.current = owner.tabs.last().cloned();
                was_current = true;
            }
        }
        Some((tab, was_current))
    }

    /// Stop (or let continue) an agent: its tab shows it, and its actions
    /// wait for the user.
    fn set_paused(&mut self, owner: &str, paused: bool, cx: &mut Context<Self>) {
        let Some(state) = self.owners.get_mut(owner) else {
            return;
        };
        state.paused = paused;
        let label = state.label.clone();
        let targets = state.tabs.clone();
        for target in targets {
            // Stop gives the user their tab back as it was: no emulation,
            // no mocked requests.
            if paused
                && let Some(tab) = self.tab(&target)
                && tab.attached
                && (!tab.emulating.is_empty() || !tab.mocks.is_empty())
            {
                let cdp = tab.cdp.clone();
                cx.background_spawn(async move {
                    for (method, params) in [
                        ("Emulation.clearDeviceMetricsOverride", json!({})),
                        ("Emulation.setTouchEmulationEnabled", json!({ "enabled": false })),
                        ("Network.emulateNetworkConditions", json!({ "offline": false, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 })),
                        ("Emulation.setCPUThrottlingRate", json!({ "rate": 1 })),
                        ("Emulation.setEmulatedMedia", json!({ "media": "", "features": [] })),
                        ("Emulation.clearGeolocationOverride", json!({})),
                        ("Emulation.setTimezoneOverride", json!({ "timezoneId": "" })),
                        ("Emulation.setLocaleOverride", json!({})),
                        ("Fetch.disable", json!({})),
                    ] {
                        cdp.call(method, params).await.ok();
                    }
                })
                .detach();
                self.overrides_ended(&target, "the user pressed Stop");
            }
            if let Some(tab) = self.tab_mut(&target) {
                if !tab.attached {
                    continue;
                }
                let page = tab.page(owner, &label);
                tab._glow = None;
                let call = if paused {
                    agent_scripts::call("__phOverlay", "show", &[json!(label), json!(true)])
                } else {
                    agent_scripts::call("__phOverlay", "hide", &[])
                };
                cx.spawn(async move |this, cx| {
                    let _ = in_world(&this, &page, &call, cx).await;
                })
                .detach();
            }
        }
        cx.notify();
        cx.emit(AgentChromeEvent::Changed);
    }

    /// Emulation and mocks in tab `target` ended (`why`): its agent hears.
    fn overrides_ended(&mut self, target: &str, why: &str) {
        let Some(tab) = self.tab_mut(target) else {
            return;
        };
        let mut ended = Vec::new();
        if !tab.emulating.is_empty() {
            ended.push(format!("the emulation ({})", tab.emulating.join(", ")));
        }
        if !tab.mocks.is_empty() {
            ended.push(format!("{} mocked request rule(s)", tab.mocks.len()));
        }
        tab.emulating.clear();
        tab.mocks.clear();
        tab.keep_until = None;
        if ended.is_empty() {
            return;
        }
        let owner = tab.owner.clone();
        if let Some(owner) = self.owners.get_mut(&owner) {
            owner.notes.push(format!(
                "{} ended: {why}. Set them again if you still need them.",
                ended.join(" and ")
            ));
        }
    }

    // ---- Tabs ------------------------------------------------------------

    fn owner_mut(&mut self, who: &Who) -> &mut Owner {
        let owner = self
            .owners
            .entry(who.owner.clone())
            .or_insert_with(|| Owner {
                label: who.label.clone(),
                tabs: Vec::new(),
                current: None,
                paused: false,
                lost: false,
                notes: Vec::new(),
            });
        owner.label = who.label.clone();
        owner
    }

    /// The agent's current page (the debugger attached again when it let go
    /// while idle); with `open`, one is opened when it has none (or `fresh`:
    /// a new one anyway).
    fn page(
        &mut self,
        who: &Who,
        open: bool,
        fresh: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Page>> {
        self.retire_finished(cx);
        let label = who.label.clone();
        let owner = self.owner_mut(who);
        if owner.paused {
            return Task::ready(Err(anyhow!(
                "The user pressed Stop on your Chrome tab. Do not use the browser until they say to go on; they can also press “Let it continue” on the tab."
            )));
        }
        let (current, lost) = (owner.current.clone(), owner.lost);
        if !fresh
            && let Some(target) = current
            && let Some(tab) = self.tab(&target)
        {
            if let Some(dialog) = &tab.dialog {
                return Task::ready(Err(dialog.waiting()));
            }
            let page = tab.page(&who.owner, &label);
            if tab.attached {
                self.idle_later(&target, cx);
                return Task::ready(Ok(page));
            }
            let letting_go = tab.letting_go.clone();
            let browser = match self.browser() {
                Ok(browser) => browser,
                Err(err) => return Task::ready(Err(err)),
            };
            return cx.spawn(async move |this, cx| {
                if let Some(letting_go) = letting_go {
                    letting_go.await;
                }
                attach(&browser, &page.target).await?;
                this.update(cx, |this, cx| {
                    if let Some(tab) = this.tab_mut(&page.target) {
                        tab.attached = true;
                        tab.letting_go = None;
                        tab.world = None;
                        tab.main_frame = None;
                        tab.loading = false;
                    }
                    this.idle_later(&page.target, cx);
                })?;
                Ok(page)
            });
        }
        if !open {
            let notes = std::mem::take(&mut self.owner_mut(who).notes);
            return Task::ready(Err(if lost {
                anyhow!(
                    "Chrome or its PilotHouse extension went away (perhaps the user quit Chrome), so your tab is gone: open the page again with chrome_open (sign-ins are kept)."
                )
            } else if !notes.is_empty() {
                // Its tabs closed (Chrome quitting closes them first).
                anyhow!("{} Open a page with chrome_open.", notes.join(" "))
            } else {
                anyhow!("You have no page open in Chrome yet: open one with chrome_open.")
            }));
        }
        let browser = match self.browser() {
            Ok(browser) => browser,
            Err(err) => return Task::ready(Err(err)),
        };
        self.owner_mut(who).lost = false;
        let owner = who.owner.clone();
        cx.spawn(async move |this, cx| {
            let opened = browser
                .call("PilotHouse.openTab", json!({ "url": "about:blank" }))
                .await?;
            let target = opened["tabId"]
                .as_u64()
                .context("Chrome did not open a tab")?
                .to_string();
            let cdp = attach(&browser, &target).await?;
            this.update(cx, |this, cx| {
                this.add_tab(&owner, target.clone(), cdp, false);
                this.idle_later(&target, cx);
                let tab = this.tab(&target).context("the tab closed")?;
                Ok(tab.page(&owner, &label))
            })?
        })
    }

    fn add_tab(&mut self, owner: &str, target: String, cdp: Cdp, shared: bool) {
        self.tabs.push(Tab {
            target: target.clone(),
            shared,
            attached: true,
            busy: Arc::default(),
            cdp,
            owner: owner.to_string(),
            url: String::new(),
            title: String::new(),
            main_frame: None,
            loading: false,
            loaded: VecDeque::new(),
            load_waiters: Vec::new(),
            logs: Logs::default(),
            dialog: None,
            dialog_waiters: Vec::new(),
            world: None,
            console_seen: 0,
            network_seen: 0,
            hinted: 0,
            opened_window: None,
            letting_go: None,
            emulating: Vec::new(),
            mocks: Vec::new(),
            keep_until: None,
            heap: None,
            heap_too_large: false,
            heap_summary: None,
            _glow: None,
            _idle: None,
        });
        if let Some(state) = self.owners.get_mut(owner) {
            state.tabs.push(target.clone());
            state.current = Some(target);
        }
    }

    /// A tab a page of `owner` opened: attached, and the agent's current.
    fn adopt(
        &mut self,
        owner: String,
        target: String,
        url: String,
        title: String,
        cx: &mut Context<Self>,
    ) {
        let Ok(browser) = self.browser() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Ok(cdp) = attach(&browser, &target).await else {
                return;
            };
            // What it shows now: changes while it was being attached were
            // not followed.
            let now = evaluate_main(&cdp, "[location.href, document.title]")
                .await
                .unwrap_or_default();
            let url = now[0].as_str().filter(|url| !url.is_empty()).map_or(url, str::to_string);
            let title = now[1].as_str().map_or(title, str::to_string);
            let _ = this.update(cx, |this, cx| {
                if this.tab(&target).is_some() {
                    return;
                }
                this.add_tab(&owner, target.clone(), cdp, false);
                if let Some(tab) = this.tab_mut(&target) {
                    tab.url = url;
                    tab.title = title;
                }
                if let Some(state) = this.owners.get_mut(&owner) {
                    state.notes.push(
                        "Your page opened a new tab; it is now your current tab (chrome_tabs lists them).".into(),
                    );
                }
                this.idle_later(&target, cx);
                cx.notify();
                cx.emit(AgentChromeEvent::Changed);
            });
        })
        .detach();
    }

    /// Keeps the glow on for a moment after an action.
    fn glow(&mut self, page: &Page, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_mut(&page.target) else {
            return;
        };
        let page = page.clone();
        tab._glow = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GLOW_AFTER).await;
            let call = agent_scripts::call("__phOverlay", "hide", &[]);
            let _ = in_world(&this, &page, &call, cx).await;
        }));
    }

    /// Lets go of the tab (Chrome's debugger detaches, its bar goes) once
    /// nothing has happened in it for [`IDLE_AFTER`], unless its agent is
    /// stopped (the Stop bar's "Let it continue" needs the debugger) or a
    /// dialog in it waits for an answer (`chrome_dialog` answers through the
    /// debugger, and only it hears the dialog close).
    fn idle_later(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_mut(target) else {
            return;
        };
        let target = target.to_string();
        tab._idle = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(IDLE_AFTER).await;
                let ready = this.update(cx, |this, _| {
                    let tab = this.tab(&target)?;
                    let paused = this
                        .owners
                        .get(&tab.owner)
                        .is_some_and(|owner| owner.paused);
                    let idle = tab.attached
                        && !paused
                        && tab.dialog.is_none()
                        && tab.busy.load(Ordering::SeqCst) == 0
                        && tab.keep_until.is_none_or(|until| Instant::now() >= until);
                    Some(idle.then(|| (this.browser().ok(), tab.cdp.clone(), tab.world)))
                });
                match ready {
                    Ok(Some(Some((Some(browser), cdp, world)))) => {
                        let going = {
                            let target = target.clone();
                            cx.spawn(async move |_| let_go(browser, cdp, world, target).await)
                                .shared()
                        };
                        let _ = this.update(cx, |this, _| {
                            if let Some(tab) = this.tab_mut(&target) {
                                tab.attached = false;
                                tab.world = None;
                                tab.main_frame = None;
                                tab.loading = false;
                                tab.letting_go = Some(going.clone());
                            }
                            this.overrides_ended(
                                &target,
                                "the tab was idle, so Chrome's debugger let go",
                            );
                        });
                        going.await;
                        return;
                    }
                    // Still in use: look again later.
                    Ok(Some(None)) => continue,
                    _ => return,
                }
            }
        }));
    }

    // ---- Actions ---------------------------------------------------------

    /// Opens `url` in the agent's tab (a new tab with `new_tab`) and waits
    /// for it to load.
    pub fn open(
        &mut self,
        who: &Who,
        url: &str,
        new_tab: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let url = crate::normalize_url(url);
        let page = self.page(who, true, new_tab, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let result = guarded(
                &this,
                &page,
                cx,
                page.cdp.call("Page.navigate", json!({ "url": url })),
            )
            .await?;
            if let Some(error) = result["errorText"].as_str() {
                bail!("could not open {url}: {error}");
            }
            if let Some(loader) = result["loaderId"].as_str() {
                wait_for_loader(&this, &page, loader, cx).await?;
            }
            summary(&this, &page, cx).await
        })
    }

    /// Back, forward or reload.
    pub fn history(
        &mut self,
        who: &Who,
        action: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        let action = action.to_string();
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            match action.as_str() {
                "reload" => {
                    guarded(&this, &page, cx, page.cdp.call("Page.reload", json!({}))).await?;
                }
                "back" | "forward" => {
                    let history = page
                        .cdp
                        .call("Page.getNavigationHistory", json!({}))
                        .await?;
                    let current = history["currentIndex"].as_i64().unwrap_or(0);
                    let entries = history["entries"].as_array().cloned().unwrap_or_default();
                    let index = if action == "back" {
                        current - 1
                    } else {
                        current + 1
                    };
                    let Some(entry) = usize::try_from(index)
                        .ok()
                        .and_then(|index| entries.get(index))
                    else {
                        bail!("there is no page to go {action} to");
                    };
                    guarded(
                        &this,
                        &page,
                        cx,
                        page.cdp.call(
                            "Page.navigateToHistoryEntry",
                            json!({ "entryId": entry["id"] }),
                        ),
                    )
                    .await?;
                }
                other => bail!("unknown action `{other}`: back, forward or reload"),
            }
            wait_until_ready(&this, &page, cx).await;
            summary(&this, &page, cx).await
        })
    }

    /// The agent's tabs: `list` (with the tabs the user shared), `new`
    /// (optionally at `url`), `switch` to or `close` the tab at `index` (a
    /// shared tab is given back, not closed), and `take` a tab the user
    /// shared, `tab` (by id; without one, the shared tab on screen).
    pub fn tabs(
        &mut self,
        who: &Who,
        action: &str,
        index: Option<usize>,
        url: Option<String>,
        tab: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        match action {
            "new" => {
                let Some(url) = url else {
                    let page = self.page(who, true, true, cx);
                    return cx.spawn(async move |this, cx| {
                        let page = page.await?;
                        summary(&this, &page, cx).await
                    });
                };
                self.open(who, &url, true, cx)
            }
            "take" => self.take(who, tab, cx),
            "list" => {
                let mine = self.tab_list(who);
                let Ok(browser) = self.browser() else {
                    return Task::ready(Ok(json!({ "text": mine })));
                };
                let agents: Vec<String> = self.tabs.iter().map(|tab| tab.target.clone()).collect();
                cx.background_spawn(async move {
                    let shared = shared_tabs(&browser, &agents).await.unwrap_or_default();
                    let mut text = mine;
                    if shared.is_empty() {
                        text.push_str(
                            "\n\nThe user has shared no other tabs. They can share one by putting it in the PilotHouse tab group (or with the PilotHouse button in Chrome's toolbar).",
                        );
                    } else {
                        text.push_str(
                            "\n\nTabs the user shared (in the PilotHouse group; chrome_tabs take with the id works in one, without an id the one on screen):",
                        );
                        for tab in &shared {
                            let place = match (tab.showing, tab.focused) {
                                (_, true) => " (on screen, the user is in it)",
                                (true, false) => " (on screen)",
                                _ => "",
                            };
                            text.push_str(&format!(
                                "\n[{}] {} — {}{place}",
                                tab.target, tab.title, tab.url
                            ));
                        }
                    }
                    Ok(json!({ "text": text }))
                })
            }
            "switch" | "close" => {
                let owner = self.owner_mut(who);
                let tabs = owner.tabs.clone();
                let label = owner.label.clone();
                let pick = |index: Option<usize>| -> Result<String> {
                    let index = index.context(
                        "give the tab's `index` (chrome_tabs with action list shows them)",
                    )?;
                    tabs.get(index)
                        .cloned()
                        .with_context(|| format!("you have no tab {index}"))
                };
                let target = match pick(index) {
                    Ok(target) => target,
                    Err(err) => return Task::ready(Err(err)),
                };
                if action == "switch" {
                    self.owner_mut(who).current = Some(target);
                    return Task::ready(Ok(json!({ "text": self.tab_list(who) })));
                }
                let browser = match self.browser() {
                    Ok(browser) => browser,
                    Err(err) => return Task::ready(Err(err)),
                };
                let given_back = self
                    .tab(&target)
                    .filter(|tab| tab.shared)
                    .map(|tab| (tab.page(&who.owner, &label), tab.attached));
                cx.spawn(async move |this, cx| {
                    // The user's tab stays open: the overlay goes, and the
                    // agent lets go of it.
                    if let Some((page, attached)) = given_back {
                        if attached {
                            let hide = agent_scripts::call("__phOverlay", "hide", &[]);
                            let _ = in_world(&this, &page, &hide, cx).await;
                        }
                        let target = page.target.clone();
                        drop(page);
                        this.update(cx, |this, cx| {
                            this.remove_tab(&target);
                            cx.notify();
                            cx.emit(AgentChromeEvent::Changed);
                        })?;
                        if attached {
                            browser
                                .call("PilotHouse.detach", json!({ "tabId": tab_id(&target)? }))
                                .await
                                .ok();
                        }
                        return Ok(
                            json!({ "text": "Gave the tab back to the user; it stays open." }),
                        );
                    }
                    browser
                        .call("PilotHouse.closeTab", json!({ "tabId": tab_id(&target)? }))
                        .await?;
                    this.update(cx, |this, cx| {
                        this.forget_tab(&target, cx);
                    })?;
                    Ok(json!({ "text": "Closed." }))
                })
            }
            other => Task::ready(Err(anyhow!(
                "unknown action `{other}`: list, new, switch, close or take"
            ))),
        }
    }

    /// Takes over a tab the user shared (by `id` from the list; without one,
    /// the shared tab on screen): it becomes the agent's current tab, with
    /// what the user typed in it kept.
    fn take(
        &mut self,
        who: &Who,
        id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        if self.owner_mut(who).paused {
            return Task::ready(Err(anyhow!(
                "The user pressed Stop on your Chrome tab. Do not use the browser until they say to go on."
            )));
        }
        let id = id.map(|id| id.trim().trim_matches(['[', ']']).to_string());
        // A tab an agent already works in.
        if let Some(id) = &id
            && let Some(tab) = self.tab(id)
        {
            if tab.owner != who.owner {
                let other = self
                    .owners
                    .get(&tab.owner)
                    .map(|owner| owner.label.clone())
                    .unwrap_or_else(|| "another agent".into());
                return Task::ready(Err(anyhow!(
                    "{other} is working in that tab; it cannot be taken."
                )));
            }
            let target = tab.target.clone();
            self.owner_mut(who).current = Some(target);
            return Task::ready(Ok(
                json!({ "text": "That tab is already yours; it is now your current tab." }),
            ));
        }
        let browser = match self.browser() {
            Ok(browser) => browser,
            Err(err) => return Task::ready(Err(err)),
        };
        let agents: Vec<String> = self.tabs.iter().map(|tab| tab.target.clone()).collect();
        let (owner, label) = (who.owner.clone(), who.label.clone());
        cx.spawn(async move |this, cx| {
            let shared = shared_tabs(&browser, &agents).await?;
            let chosen = match &id {
                Some(id) => shared
                    .iter()
                    .find(|tab| tab.target == *id)
                    .with_context(|| format!("the user has shared no tab [{id}] (chrome_tabs list shows the shared ones)"))?,
                None => shared.iter().find(|tab| tab.showing).context(
                    "no shared tab is on screen: ask the user which one, or to put the tab in the PilotHouse group (chrome_tabs list shows the shared ones)",
                )?,
            };
            let (target, url, title) = (chosen.target.clone(), chosen.url.clone(), chosen.title.clone());
            let cdp = attach(&browser, &target).await?;
            let page = this.update(cx, |this, cx| {
                // Another agent took it meanwhile.
                if this.tab(&target).is_some() {
                    return None;
                }
                this.add_tab(&owner, target.clone(), cdp, true);
                if let Some(tab) = this.tab_mut(&target) {
                    tab.url = url;
                    tab.title = title;
                }
                this.idle_later(&target, cx);
                cx.notify();
                cx.emit(AgentChromeEvent::Changed);
                this.tab(&target).map(|tab| tab.page(&owner, &label))
            })?;
            let Some(page) = page else {
                bail!("another agent took that tab just now");
            };
            begin(&this, &page, cx).await;
            let mut result = summary(&this, &page, cx).await?;
            if let Some(notes) = result["notes"].as_array_mut() {
                notes.insert(
                    0,
                    json!("You took over a tab the user shared; it is your current tab. Keep what the user already typed or chose: fill only what they asked you to. chrome_tabs close gives it back without closing it."),
                );
            }
            Ok(result)
        })
    }

    fn tab_list(&self, who: &Who) -> String {
        let Some(owner) = self.owners.get(&who.owner) else {
            return "You have no tabs.".into();
        };
        let lines: Vec<String> = owner
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, target)| {
                let tab = self.tab(target)?;
                let current = if owner.current.as_deref() == Some(target) {
                    " (current)"
                } else {
                    ""
                };
                let shared = if tab.shared {
                    " (shared by the user)"
                } else {
                    ""
                };
                Some(format!(
                    "{index}: {} — {}{current}{shared}",
                    tab.title, tab.url
                ))
            })
            .collect();
        if lines.is_empty() {
            "You have no tabs.".into()
        } else {
            format!("Your tabs:\n{}", lines.join("\n"))
        }
    }

    /// Reads the page: numbered elements (for `ref`), the text, the scroll.
    pub fn snapshot(&mut self, who: &Who, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            let mut read = in_world(
                &this,
                &page,
                &agent_scripts::call("__phAgent", "snapshot", &[json!(SNAPSHOT_ELEMENTS)]),
                cx,
            )
            .await?;
            let notes = notes(&this, &page, cx)?;
            read["notes"] = notes;
            Ok(read)
        })
    }

    /// A screenshot of the page (`full_page`: all of it), or of an element.
    /// (`with_overlay` keeps the glow in, for checking how it looks.)
    pub fn screenshot(
        &mut self,
        who: &Who,
        full_page: bool,
        element: Option<Value>,
        with_overlay: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            let mut options = json!({ "format": "png", "captureBeyondViewport": full_page });
            if let Some(element) = element {
                let rect = in_world(
                    &this,
                    &page,
                    &format!(
                        "(() => {{ const el = window.__phAgent.element({element}); el.scrollIntoView({{ block: 'center', behavior: 'instant' }}); const r = el.getBoundingClientRect(); return {{ x: r.x + scrollX, y: r.y + scrollY, width: r.width, height: r.height }}; }})()"
                    ),
                    cx,
                )
                .await?;
                options["clip"] = json!({
                    "x": rect["x"], "y": rect["y"], "width": rect["width"], "height": rect["height"], "scale": 1
                });
                options["captureBeyondViewport"] = json!(true);
            }
            let conceal = |hidden: bool| agent_scripts::call("__phOverlay", "conceal", &[json!(hidden)]);
            if !with_overlay {
                let _ = in_world(&this, &page, &conceal(true), cx).await;
            }
            let shot = capture(&page, &options, cx).await;
            if !with_overlay {
                let _ = in_world(&this, &page, &conceal(false), cx).await;
            }
            let mut result = summary(&this, &page, cx).await?;
            result["images"] = json!([{ "data": shot?["data"], "mimeType": "image/png" }]);
            Ok(result)
        })
    }

    /// Clicks an element or a point: `button` left, right or middle;
    /// `count` 2 for a double click; `modifiers` held (Alt, Control, Meta,
    /// Shift).
    pub fn click(
        &mut self,
        who: &Who,
        target: Target,
        button: String,
        count: u32,
        modifiers: u32,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let (x, y) = point_of(&this, &page, &target, cx).await?;
            glide(&this, &page, x, y, cx).await;
            let cdp = page.cdp.clone();
            let press = async move {
                cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseMoved", "x": x, "y": y, "modifiers": modifiers }),
                )
                .await?;
                for click in 1..=count.max(1) {
                    for kind in ["mousePressed", "mouseReleased"] {
                        cdp.call(
                            "Input.dispatchMouseEvent",
                            json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": click, "modifiers": modifiers }),
                        )
                        .await?;
                    }
                }
                anyhow::Ok(())
            };
            let pulse = agent_scripts::call("__phOverlay", "pulse", &[json!(x), json!(y)]);
            let _ = in_world(&this, &page, &pulse, cx).await;
            guarded(&this, &page, cx, press).await?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    /// Moves the pointer over an element or point (menus that open on hover).
    pub fn hover(
        &mut self,
        who: &Who,
        target: Target,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let (x, y) = point_of(&this, &page, &target, cx).await?;
            glide(&this, &page, x, y, cx).await;
            guarded(
                &this,
                &page,
                cx,
                page.cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseMoved", "x": x, "y": y }),
                ),
            )
            .await?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    /// Drags from one element or point to another.
    pub fn drag(
        &mut self,
        who: &Who,
        from: Target,
        to: Target,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let (x0, y0) = point_of(&this, &page, &from, cx).await?;
            glide(&this, &page, x0, y0, cx).await;
            let cdp = page.cdp.clone();
            cdp.call("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x0, "y": y0 })).await?;
            cdp.call(
                "Input.dispatchMouseEvent",
                json!({ "type": "mousePressed", "x": x0, "y": y0, "button": "left", "clickCount": 1 }),
            )
            .await?;
            let (x1, y1) = point_of(&this, &page, &to, cx).await?;
            const STEPS: u32 = 12;
            for step in 1..=STEPS {
                let t = step as f64 / STEPS as f64;
                let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
                cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseMoved", "x": x, "y": y, "button": "left", "buttons": 1 }),
                )
                .await?;
                let _ = in_world(&this, &page, &agent_scripts::call("__phOverlay", "move", &[json!(x), json!(y)]), cx).await;
                cx.background_executor().timer(Duration::from_millis(16)).await;
            }
            guarded(
                &this,
                &page,
                cx,
                cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseReleased", "x": x1, "y": y1, "button": "left", "clickCount": 1 }),
                ),
            )
            .await?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    /// Types `text`: into `element` first when given (its text replaced with
    /// `clear`), else where the focus is. `slowly` sends a key at a time
    /// (for fields that react to each key); `submit` presses Enter after.
    /// Never into a password field: the user signs in themselves.
    #[allow(clippy::too_many_arguments)]
    pub fn type_text(
        &mut self,
        who: &Who,
        text: String,
        element: Option<Value>,
        clear: bool,
        slowly: bool,
        submit: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let password = match &element {
                Some(element) => {
                    let point = in_world(&this, &page, &agent_scripts::call("__phAgent", "locate", std::slice::from_ref(element)), cx).await?;
                    if let (Some(x), Some(y)) = (point["x"].as_f64(), point["y"].as_f64()) {
                        glide(&this, &page, x, y, cx).await;
                    }
                    let focused =
                        in_world(&this, &page, &agent_scripts::call("__phAgent", "focus", &[element.clone(), json!(clear)]), cx)
                            .await?;
                    focused["password"] == json!(true)
                }
                None => in_world(&this, &page, "window.__phAgent.focusedIsPassword()", cx).await? == json!(true),
            };
            if password {
                let _ = this.update(cx, |this, _| {
                    this.hand_off(
                        &page.target,
                        &page.label,
                        &format!("Sign in on this page yourself (agents never type passwords), then tell {} to go on.", page.label),
                    )
                });
                bail!("{ASK_TO_SIGN_IN}");
            }
            let cdp = page.cdp.clone();
            let typing = async move {
                if slowly {
                    for character in text.chars() {
                        let key = character.to_string();
                        cdp.call("Input.dispatchKeyEvent", json!({ "type": "keyDown", "key": key, "text": key, "unmodifiedText": key }))
                            .await?;
                        cdp.call("Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": key })).await?;
                    }
                } else {
                    cdp.call("Input.insertText", json!({ "text": text })).await?;
                }
                if submit {
                    for event in key_events("Enter", 0)? {
                        cdp.call("Input.dispatchKeyEvent", event).await?;
                    }
                }
                anyhow::Ok(())
            };
            guarded(&this, &page, cx, typing).await?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    /// Chooses options of a list (`<select>`) by value or text.
    pub fn select(
        &mut self,
        who: &Who,
        element: Value,
        values: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let chosen = in_world(
                &this,
                &page,
                &agent_scripts::call("__phAgent", "select", &[element, json!(values)]),
                cx,
            )
            .await?;
            settle(cx).await;
            let mut result = summary(&this, &page, cx).await?;
            result["chosen"] = chosen;
            Ok(result)
        })
    }

    /// Presses a key or a combination: `Enter`, `Escape`, `ArrowDown`,
    /// `Control+A`, `Meta+Shift+K`…
    pub fn press(
        &mut self,
        who: &Who,
        keys: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let (key, modifiers) = parse_combo(&keys)?;
            let events = key_events(&key, modifiers)?;
            // Characters (and pasting) into a password field are typing it.
            let shortcut = modifiers & (MODIFIER_CTRL | MODIFIER_META) != 0;
            let writes = (key.chars().count() == 1 && !shortcut)
                || (shortcut && key.eq_ignore_ascii_case("v"));
            if writes
                && in_world(&this, &page, "window.__phAgent.focusedIsPassword()", cx).await?
                    == json!(true)
            {
                let _ = this.update(cx, |this, _| {
                    this.hand_off(
                        &page.target,
                        &page.label,
                        &format!("Sign in on this page yourself (agents never type passwords), then tell {} to go on.", page.label),
                    )
                });
                bail!("{ASK_TO_SIGN_IN}");
            }
            let cdp = page.cdp.clone();
            let pressing = async move {
                for event in events {
                    cdp.call("Input.dispatchKeyEvent", event).await?;
                }
                anyhow::Ok(())
            };
            guarded(&this, &page, cx, pressing).await?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    /// Scrolls by `dx`, `dy` CSS pixels, the page or (with `element`) what
    /// holds it, with the mouse wheel as a person would.
    pub fn scroll(
        &mut self,
        who: &Who,
        dx: f64,
        dy: f64,
        element: Option<Value>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let (x, y) = match &element {
                Some(element) => {
                    point_of(&this, &page, &Target::Element(element.clone()), cx).await?
                }
                None => {
                    let size =
                        in_world(&this, &page, "({ w: innerWidth, h: innerHeight })", cx).await?;
                    (
                        size["w"].as_f64().unwrap_or(800.0) / 2.0,
                        size["h"].as_f64().unwrap_or(600.0) / 2.0,
                    )
                }
            };
            guarded(
                &this,
                &page,
                cx,
                page.cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy }),
                ),
            )
            .await?;
            settle(cx).await;
            let position = in_world(
                &this,
                &page,
                &agent_scripts::call("__phAgent", "scrollBy", &[Value::Null, json!(0), json!(0)]),
                cx,
            )
            .await?;
            let mut result = summary(&this, &page, cx).await?;
            result["scroll"] = position;
            Ok(result)
        })
    }

    /// Waits until an element appears (or with `gone`, goes away), the page
    /// shows `text`, its address contains `url`, or no request is pending
    /// for half a second; with none of these, until the page has loaded.
    #[allow(clippy::too_many_arguments)]
    pub fn wait(
        &mut self,
        who: &Who,
        element: Option<Value>,
        gone: bool,
        url: Option<String>,
        network_idle: bool,
        timeout: Duration,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            let started = Instant::now();
            let mut quiet_since: Option<Instant> = None;
            loop {
                if let Some(dialog) = this.update(cx, |this, _| {
                    this.tab(&page.target).and_then(|tab| tab.dialog.clone())
                })? {
                    return Err(dialog.waiting());
                }
                let done = if let Some(element) = &element {
                    let seen = in_world(
                        &this,
                        &page,
                        &agent_scripts::call("__phAgent", "has", std::slice::from_ref(element)),
                        cx,
                    )
                    .await
                    .unwrap_or(Value::Bool(false))
                        == json!(true);
                    seen != gone
                } else if let Some(url) = &url {
                    this.update(cx, |this, _| {
                        this.tab(&page.target)
                            .is_some_and(|tab| tab.url.contains(url.as_str()))
                    })?
                } else if network_idle {
                    let pending = this.update(cx, |this, _| {
                        this.tab(&page.target).map_or(0, |tab| {
                            tab.logs
                                .network
                                .iter()
                                .filter(|request| {
                                    request.status.is_none()
                                        && request.failed.is_none()
                                        && request.started.elapsed() < Duration::from_secs(30)
                                })
                                .count()
                        })
                    })?;
                    if pending == 0 {
                        let since = *quiet_since.get_or_insert_with(Instant::now);
                        since.elapsed() >= Duration::from_millis(500)
                    } else {
                        quiet_since = None;
                        false
                    }
                } else {
                    in_world(&this, &page, "document.readyState", cx).await.ok()
                        == Some(json!("complete"))
                };
                if done {
                    let mut result = summary(&this, &page, cx).await?;
                    result["waited_ms"] = json!(started.elapsed().as_millis() as u64);
                    return Ok(result);
                }
                if started.elapsed() >= timeout {
                    bail!("still not there after {} s", timeout.as_secs_f32());
                }
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
            }
        })
    }

    /// Runs JavaScript in the page (the page's own world) and returns its
    /// JSON value; promises are awaited.
    pub fn evaluate(
        &mut self,
        who: &Who,
        expression: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let cdp = page.cdp.clone();
            let value = guarded(&this, &page, cx, async move {
                evaluate_main(&cdp, &expression).await
            })
            .await?;
            let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
            if text.len() > MAX_VALUE {
                let mut cut = MAX_VALUE;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                text.push_str("\n… (cut)");
            }
            Ok(json!({ "text": text }))
        })
    }

    /// Console messages and errors, oldest first: `level` (error, warn,
    /// info, debug) and up; `contains` some text; `new_only` since the
    /// last time; `clear` forgets them after.
    pub fn console(
        &mut self,
        who: &Who,
        level: Option<String>,
        contains: Option<String>,
        new_only: bool,
        limit: usize,
        clear: bool,
    ) -> Result<Value> {
        let tab = self.current_tab(who)?;
        let least = match level.as_deref() {
            Some("error") => Level::Error,
            Some("warn") | Some("warning") => Level::Warning,
            Some("info") | Some("log") => Level::Info,
            _ => Level::Debug,
        };
        let since = if new_only { tab.console_seen } else { 0 };
        let contains = contains.map(|text| text.to_lowercase());
        let lines: Vec<String> = tab
            .logs
            .console
            .iter()
            .filter(|line| line.seq >= since && rank(line.level) >= rank(least))
            .filter(|line| {
                contains
                    .as_ref()
                    .is_none_or(|text| line.text.to_lowercase().contains(text))
            })
            .rev()
            .take(limit)
            .map(|line| {
                let mut text = format!("[{}] {}", line.level.label(), line.text);
                if let Some(source) = &line.source {
                    text.push_str(&format!("  ({source})"));
                }
                if let Some(stack) = &line.stack {
                    for frame in stack.lines().take(8) {
                        text.push_str(&format!("\n    {frame}"));
                    }
                }
                text
            })
            .collect();
        let text = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
        let target = tab.target.clone();
        let mark = tab.logs.mark();
        let tab = self.tab_mut(&target).expect("found above");
        tab.console_seen = mark;
        if clear {
            tab.logs.console.clear();
        }
        Ok(
            json!({ "text": if text.is_empty() { "Nothing logged.".to_string() } else { text }, "url": tab.url }),
        )
    }

    /// Requests, oldest first, with their id (for chrome_request).
    pub fn network(
        &mut self,
        who: &Who,
        errors_only: bool,
        contains: Option<String>,
        kind: Option<String>,
        new_only: bool,
        limit: usize,
    ) -> Result<Value> {
        let tab = self.current_tab(who)?;
        let since = if new_only { tab.network_seen } else { 0 };
        let contains = contains.map(|text| text.to_lowercase());
        let kind = kind.map(|kind| kind.to_lowercase());
        let lines: Vec<String> = tab
            .logs
            .network
            .iter()
            .filter(|request| request.seq >= since && (!errors_only || request.is_error()))
            .filter(|request| {
                contains
                    .as_ref()
                    .is_none_or(|text| request.url.to_lowercase().contains(text))
            })
            .filter(|request| {
                kind.as_ref()
                    .is_none_or(|kind| request.kind.to_lowercase() == *kind)
            })
            .rev()
            .take(limit)
            .map(|request| {
                let status = match (&request.failed, request.status) {
                    (Some(error), _) => error.clone(),
                    (None, Some(status)) => format!("{status} {}", request.status_text)
                        .trim()
                        .to_string(),
                    (None, None) => "pending".into(),
                };
                let mut details = vec![request.kind.to_lowercase()];
                if let Some(size) = request.size {
                    details.push(human_size(size));
                }
                if let Some(ms) = request.duration_ms {
                    details.push(format!("{ms} ms"));
                }
                format!(
                    "#{} {} {} → {status} ({})",
                    request.id,
                    request.method,
                    request.url,
                    details.join(", ")
                )
            })
            .collect();
        let text = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
        let target = tab.target.clone();
        let mark = tab.logs.mark();
        let tab = self.tab_mut(&target).expect("found above");
        tab.network_seen = mark;
        Ok(json!({ "text": if text.is_empty() { "No requests.".to_string() } else { text } }))
    }

    /// One request in full: headers (credentials hidden), what was sent and,
    /// with `body`, what came back (text, cut at 64 KB).
    pub fn request(
        &mut self,
        who: &Who,
        id: String,
        body: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let id = id.trim_start_matches('#').to_string();
        let found = self.current_tab(who).and_then(|tab| {
            let request = tab
                .logs
                .network
                .iter()
                .find(|request| request.id == id)
                .cloned()
                .with_context(|| {
                    format!("no request #{id} on your current page (chrome_network lists them)")
                })?;
            Ok((tab.cdp.clone(), request))
        });
        let (cdp, request) = match found {
            Ok(found) => found,
            Err(err) => return Task::ready(Err(err)),
        };
        cx.background_spawn(async move {
            let mut text = format!(
                "{} {}\nStatus: {}\nType: {} {}\n",
                request.method,
                request.url,
                match (&request.failed, request.status) {
                    (Some(error), _) => error.clone(),
                    (None, Some(status)) => format!("{status} {}", request.status_text),
                    (None, None) => "pending".into(),
                },
                request.kind,
                request.mime
            );
            if let Some(ms) = request.duration_ms {
                text.push_str(&format!("Time: {ms} ms\n"));
            }
            if let Some(size) = request.size {
                text.push_str(&format!("Size: {}\n", human_size(size)));
            }
            text.push_str("\nRequest headers:\n");
            for (name, value) in &request.request_headers {
                text.push_str(&format!("  {name}: {value}\n"));
            }
            if let Some(data) = &request.post_data {
                text.push_str(&format!("\nSent:\n{data}\n"));
            }
            if !request.response_headers.is_empty() {
                text.push_str("\nResponse headers:\n");
                for (name, value) in &request.response_headers {
                    text.push_str(&format!("  {name}: {value}\n"));
                }
            }
            if body {
                match cdp
                    .call(
                        "Network.getResponseBody",
                        json!({ "requestId": request.id }),
                    )
                    .await
                {
                    Ok(reply) if reply["base64Encoded"] == json!(true) => {
                        text.push_str(&format!("\nBody: binary ({}), not shown.\n", request.mime));
                    }
                    Ok(reply) => {
                        let mut content =
                            crate::logs::redact_body(reply["body"].as_str().unwrap_or_default());
                        if content.len() > MAX_BODY {
                            let mut cut = MAX_BODY;
                            while !content.is_char_boundary(cut) {
                                cut -= 1;
                            }
                            content.truncate(cut);
                            content.push_str("\n… (cut)");
                        }
                        text.push_str(&format!("\nBody:\n{content}\n"));
                    }
                    Err(err) => text.push_str(&format!("\nBody: not available ({err:#}).\n")),
                }
            }
            Ok(json!({ "text": text }))
        })
    }

    /// Answers the page's dialog (alert, confirm, prompt, leaving the page).
    pub fn dialog(
        &mut self,
        who: &Who,
        accept: bool,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let tab = match self.current_tab(who) {
            Ok(tab) => tab,
            Err(err) => return Task::ready(Err(err)),
        };
        let Some(dialog) = tab.dialog.clone() else {
            return Task::ready(Err(anyhow!("No dialog is open on your page.")));
        };
        let page = tab.page(&who.owner, &who.label);
        cx.spawn(async move |this, cx| {
            let mut params = json!({ "accept": accept });
            if let Some(text) = text.or_else(|| accept.then_some(dialog.default_prompt.clone())) {
                params["promptText"] = json!(text);
            }
            page.cdp.call("Page.handleJavaScriptDialog", params).await?;
            this.update(cx, |this, _| {
                if let Some(tab) = this.tab_mut(&page.target) {
                    tab.dialog = None;
                }
            })?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    /// Puts files in a file field. Only files of the workspace's folders, or
    /// ones an agent's page downloaded, can be given.
    pub fn upload(
        &mut self,
        who: &Who,
        project_id: Option<i64>,
        element: Value,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let allowed = self.allowed_folders(project_id);
        let downloaded: Vec<PathBuf> = self
            .downloads
            .iter()
            .filter_map(|(_, download)| download.path.clone())
            .collect();
        let files = match checked_files(&paths, &allowed, &downloaded) {
            Ok(files) => files,
            Err(err) => return Task::ready(Err(err)),
        };
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            let world = world(&this, &page, cx).await?;
            let found = page
                .cdp
                .call(
                    "Runtime.evaluate",
                    json!({
                        "expression": agent_scripts::call("__phAgent", "element", &[element]),
                        "contextId": world,
                        "returnByValue": false,
                    }),
                )
                .await?;
            if let Some(details) = found.get("exceptionDetails") {
                bail!("{}", exception_text(details));
            }
            let object = found["result"]["objectId"]
                .as_str()
                .context("that is not an element")?;
            page.cdp
                .call(
                    "DOM.setFileInputFiles",
                    json!({ "files": files, "objectId": object }),
                )
                .await
                .map_err(|err| {
                    if err.to_string().contains("Not allowed") {
                        anyhow!(
                            "Chrome did not let PilotHouse's extension give files to the page: ask the user to turn on “Allow access to file URLs” for the PilotHouse extension (chrome://extensions → PilotHouse → Details), then try again."
                        )
                    } else {
                        err.context("that is not a file field (input type=file)")
                    }
                })?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    fn allowed_folders(&self, project_id: Option<i64>) -> Vec<PathBuf> {
        project_id
            .and_then(|id| self.store.project(id).ok().flatten())
            .map(|project| project.folders())
            .unwrap_or_default()
    }

    /// Files agents' pages downloaded, newest last.
    pub fn downloads_text(&self) -> Value {
        let lines: Vec<String> = self
            .downloads
            .iter()
            .map(|(_, download)| {
                let progress = match download.state.as_str() {
                    "completed" => "done".to_string(),
                    "canceled" => "canceled".to_string(),
                    _ if download.total > 0 => {
                        format!("{}%", download.received * 100 / download.total)
                    }
                    _ => format!("{} so far", human_size(download.received)),
                };
                let place = match &download.path {
                    Some(path) => path.display().to_string(),
                    None => "where Chrome saves downloads".to_string(),
                };
                format!(
                    "{} ({progress}) → {place}\n    from {}",
                    download.name, download.url
                )
            })
            .collect();
        json!({ "text": if lines.is_empty() {
            "Nothing downloaded yet. Chrome saves downloads where it saves the user's (usually their Downloads folder).".to_string()
        } else {
            lines.join("\n")
        } })
    }

    fn current_tab(&self, who: &Who) -> Result<&Tab> {
        self.owners
            .get(&who.owner)
            .and_then(|owner| owner.current.as_deref())
            .and_then(|target| self.tab(target))
            .context("You have no page open in Chrome yet: open one with chrome_open.")
    }
}

/// A tab's id as the extension takes it.
fn tab_id(target: &str) -> Result<u64> {
    target
        .parse()
        .with_context(|| format!("not a Chrome tab: {target}"))
}

/// A tab the user shared (in the PilotHouse group) that no agent works in.
struct SharedTab {
    target: String,
    title: String,
    url: String,
    /// The tab in front in its window.
    showing: bool,
    /// In the window the user is in.
    focused: bool,
}

/// The tabs of the PilotHouse group that are not `agents'`, the one the
/// user is in first. The extension lists no other tabs.
async fn shared_tabs(browser: &Cdp, agents: &[String]) -> Result<Vec<SharedTab>> {
    let listed = browser.call("PilotHouse.listTabs", json!({})).await?;
    let mut tabs: Vec<SharedTab> = listed["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tab| {
            let target = tab["tabId"].as_u64()?.to_string();
            (!agents.contains(&target)).then(|| SharedTab {
                target,
                title: tab["title"].as_str().unwrap_or_default().to_string(),
                url: tab["url"].as_str().unwrap_or_default().to_string(),
                showing: tab["active"] == json!(true),
                focused: tab["focused"] == json!(true),
            })
        })
        .collect();
    tabs.sort_by_key(|tab| (!(tab.showing && tab.focused), !tab.showing));
    Ok(tabs)
}

/// Attaches Chrome's debugger to a tab of the group (the extension refuses
/// any other): its events, the overlay and helpers on every page, and the
/// Stop button's binding.
async fn attach(browser: &Cdp, target: &str) -> Result<Cdp> {
    browser
        .call("PilotHouse.attach", json!({ "tabId": tab_id(target)? }))
        .await
        .context("could not reach the tab")?;
    let page = browser.for_session(target);
    for domain in [
        "Page.enable",
        "Runtime.enable",
        "Log.enable",
        "Network.enable",
    ] {
        page.call(domain, json!({})).await?;
    }
    page.call("Page.setLifecycleEventsEnabled", json!({ "enabled": true }))
        .await?;
    page.call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": format!("{OVERLAY};{AGENT}"), "worldName": WORLD }),
    )
    .await?;
    page.call(
        "Runtime.addBinding",
        json!({ "name": BINDING, "executionContextName": WORLD }),
    )
    .await?;
    Ok(page)
}

/// Lets go of a tab: the overlay goes, then the debugger.
async fn let_go(browser: Cdp, page: Cdp, world: Option<i64>, target: String) {
    if let Some(id) = world {
        let hide = agent_scripts::call("__phOverlay", "hide", &[]);
        page.call(
            "Runtime.evaluate",
            json!({ "expression": hide, "contextId": id }),
        )
        .await
        .ok();
    }
    if let Ok(id) = tab_id(&target) {
        browser
            .call("PilotHouse.detach", json!({ "tabId": id }))
            .await
            .ok();
    }
}

/// The user's Downloads folder, where Chrome saves by default.
fn downloads_folder() -> Option<PathBuf> {
    ph_env::var("HOME")
        .or_else(|| ph_env::var("USERPROFILE"))
        .map(|home| PathBuf::from(home).join("Downloads"))
}

/// Where Chrome saved a finished download of `name` in `folder`: the file
/// by that name, or by the name Chrome gives when it is taken (`name (1).ext`
/// …), written since the download `started` and holding exactly `size`
/// bytes. Only such a file is the download: a file the user already had
/// under that name never is, so an agent can never be pointed at it (it is
/// what chrome_upload lets go back up). `None` when there is no such file
/// (Chrome saving elsewhere, say).
fn saved_download(
    folder: &std::path::Path,
    name: &str,
    started: std::time::SystemTime,
    size: u64,
) -> Option<PathBuf> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    // A second's slack for file systems that keep times coarsely.
    let since = started
        .checked_sub(std::time::Duration::from_secs(1))
        .unwrap_or(started);
    std::iter::once(name.to_string())
        .chain((1..=50).map(|n| format!("{stem} ({n}){ext}")))
        .map(|candidate| folder.join(candidate))
        .filter_map(|path| {
            let meta = std::fs::symlink_metadata(&path).ok()?;
            let written = meta.modified().ok()?;
            (meta.is_file() && meta.len() == size && written >= since).then_some((written, path))
        })
        .max_by_key(|(written, _)| *written)
        .map(|(_, path)| path)
}

/// The isolated world of the page's current document, made (with the
/// overlay and helpers in it) when the page has none yet.
async fn world(this: &WeakEntity<AgentChrome>, page: &Page, cx: &mut AsyncApp) -> Result<i64> {
    let (known, frame) = this.update(cx, |this, _| {
        this.tab(&page.target)
            .map(|tab| (tab.world, tab.main_frame.clone()))
            .unwrap_or_default()
    })?;
    if let Some(id) = known {
        return Ok(id);
    }
    let frame = match frame {
        Some(frame) => frame,
        None => page.cdp.call("Page.getFrameTree", json!({})).await?["frameTree"]["frame"]["id"]
            .as_str()
            .context("the page has no frame")?
            .to_string(),
    };
    let created = page
        .cdp
        .call(
            "Page.createIsolatedWorld",
            json!({ "frameId": frame, "worldName": WORLD }),
        )
        .await?;
    let id = created["executionContextId"]
        .as_i64()
        .context("could not reach the page")?;
    page.cdp
        .call(
            "Runtime.evaluate",
            json!({ "expression": format!("{OVERLAY};{AGENT}"), "contextId": id }),
        )
        .await?;
    this.update(cx, |this, _| {
        if let Some(tab) = this.tab_mut(&page.target) {
            tab.world = Some(id);
            tab.main_frame.get_or_insert(frame);
        }
    })?;
    Ok(id)
}

/// Runs `expression` in the page's isolated world; tries once more when
/// the document changed under it.
async fn in_world(
    this: &WeakEntity<AgentChrome>,
    page: &Page,
    expression: &str,
    cx: &mut AsyncApp,
) -> Result<Value> {
    for attempt in 0..2 {
        let id = world(this, page, cx).await?;
        let reply = page
            .cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": expression, "contextId": id, "returnByValue": true, "awaitPromise": true }),
            )
            .await;
        match reply {
            Ok(result) => {
                if let Some(details) = result.get("exceptionDetails") {
                    bail!("{}", exception_text(details));
                }
                return Ok(result["result"]
                    .get("value")
                    .cloned()
                    .unwrap_or(Value::Null));
            }
            Err(err) if attempt == 0 && err.to_string().contains("context") => {
                this.update(cx, |this, _| {
                    if let Some(tab) = this.tab_mut(&page.target) {
                        tab.world = None;
                    }
                })?;
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("returns within two attempts")
}

async fn evaluate_main(cdp: &Cdp, expression: &str) -> Result<Value> {
    let result = cdp
        .call(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true }),
        )
        .await?;
    if let Some(details) = result.get("exceptionDetails") {
        bail!("{}", exception_text(details));
    }
    Ok(result["result"]
        .get("value")
        .cloned()
        .unwrap_or(Value::Null))
}

fn exception_text(details: &Value) -> String {
    let text = details["exception"]["description"]
        .as_str()
        .or_else(|| details["exception"]["value"].as_str())
        .or_else(|| details["text"].as_str())
        .unwrap_or("script error");
    // `Error: no element matches …` reads better without the stack.
    text.lines()
        .next()
        .unwrap_or(text)
        .trim_start_matches("Error: ")
        .to_string()
}

/// The glow and the agent's name come on, and stay a moment after.
async fn begin(this: &WeakEntity<AgentChrome>, page: &Page, cx: &mut AsyncApp) {
    let show = agent_scripts::call("__phOverlay", "show", &[json!(page.label), json!(false)]);
    let _ = in_world(this, page, &show, cx).await;
    let _ = this.update(cx, |this, cx| this.glow(page, cx));
}

/// The pointer glides to (x, y) before the action, and the bar lets the
/// agent's input through until [`settle`].
async fn glide(this: &WeakEntity<AgentChrome>, page: &Page, x: f64, y: f64, cx: &mut AsyncApp) {
    let moved = format!(
        "{};{}",
        agent_scripts::call("__phOverlay", "move", &[json!(x), json!(y)]),
        agent_scripts::call("__phOverlay", "through", &[json!(true)])
    );
    if in_world(this, page, &moved, cx).await.is_ok() {
        cx.background_executor().timer(POINTER_GLIDE).await;
    }
}

/// Where to act: an element's middle (scrolled into view) or a point.
async fn point_of(
    this: &WeakEntity<AgentChrome>,
    page: &Page,
    target: &Target,
    cx: &mut AsyncApp,
) -> Result<(f64, f64)> {
    match target {
        Target::Point(x, y) => Ok((*x, *y)),
        Target::Element(element) => {
            let point = in_world(
                this,
                page,
                &agent_scripts::call("__phAgent", "locate", std::slice::from_ref(element)),
                cx,
            )
            .await?;
            Ok((
                point["x"]
                    .as_f64()
                    .context("the element has no place on the page")?,
                point["y"]
                    .as_f64()
                    .context("the element has no place on the page")?,
            ))
        }
    }
}

/// Runs an action, unless the page opens a dialog meanwhile (a click on
/// "Delete" asking to confirm): then the agent is told at once, rather than
/// waiting for a page that cannot go on until the dialog is answered.
async fn guarded<T>(
    this: &WeakEntity<AgentChrome>,
    page: &Page,
    cx: &mut AsyncApp,
    action: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let (sender, opened) = smol::channel::bounded(1);
    this.update(cx, |this, _| {
        if let Some(tab) = this.tab_mut(&page.target) {
            tab.dialog_waiters.push(sender);
        }
    })?;
    let dialog = async move {
        match opened.recv().await {
            Ok(dialog) => Err(dialog.waiting()),
            // Never answered: the action decides.
            Err(_) => std::future::pending().await,
        }
    };
    smol::future::or(action, dialog).await
}

async fn wait_for_loader(
    this: &WeakEntity<AgentChrome>,
    page: &Page,
    loader: &str,
    cx: &mut AsyncApp,
) -> Result<()> {
    let (done, loaded) = smol::channel::bounded(1);
    let (dialog_sender, dialog_opened) = smol::channel::bounded(1);
    this.update(cx, |this, _| {
        if let Some(tab) = this.tab_mut(&page.target) {
            if tab.loaded.iter().any(|known| known == loader) {
                let _ = done.try_send(());
            } else {
                tab.load_waiters.push((loader.to_string(), done));
            }
            tab.dialog_waiters.push(dialog_sender);
        }
    })?;
    let timer = cx.background_executor().timer(LOAD_TIMEOUT);
    let finished = async {
        loaded.recv().await.ok();
        Ok(())
    };
    let dialog = async {
        match dialog_opened.recv().await {
            Ok(dialog) => Err(dialog.waiting()),
            Err(_) => std::future::pending().await,
        }
    };
    let late = async {
        timer.await;
        // Slow pages: the agent reads what is there.
        Ok(())
    };
    smol::future::or(smol::future::or(finished, dialog), late).await
}

async fn wait_until_ready(this: &WeakEntity<AgentChrome>, page: &Page, cx: &mut AsyncApp) {
    let started = Instant::now();
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
    while started.elapsed() < LOAD_TIMEOUT {
        if in_world(this, page, "document.readyState", cx).await.ok() == Some(json!("complete")) {
            return;
        }
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
    }
}

/// A screenshot, bringing the tab forward when Chrome does not draw it
/// (a tab in the background of a minimized window).
async fn capture(page: &Page, options: &Value, cx: &mut AsyncApp) -> Result<Value> {
    for attempt in 0..2 {
        let shot = page.cdp.call("Page.captureScreenshot", options.clone());
        let late = async {
            cx.background_executor().timer(SHOT_TIMEOUT).await;
            Err(anyhow!("Chrome did not draw the tab"))
        };
        match smol::future::or(shot, late).await {
            Ok(shot) => return Ok(shot),
            Err(err) if attempt == 0 => {
                tracing::debug!("screenshot: {err:#}; bringing the tab forward");
                if let Ok(id) = tab_id(&page.target) {
                    page.cdp
                        .browser()
                        .call("PilotHouse.activate", json!({ "tabId": id }))
                        .await
                        .ok();
                }
            }
            Err(err) => {
                return Err(err.context(
                    "the tab may be in a minimized window; ask the user to bring it back",
                ));
            }
        }
    }
    unreachable!("returns within two attempts")
}

/// Gives the page a moment to react before reporting back.
async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(350))
        .await;
}

/// After an action: the bar takes the user's clicks again.
async fn release(this: &WeakEntity<AgentChrome>, page: &Page, cx: &mut AsyncApp) {
    let call = agent_scripts::call("__phOverlay", "through", &[json!(false)]);
    let _ = in_world(this, page, &call, cx).await;
}

/// After an action: the page, and what the agent should know (a dialog,
/// new errors, failed requests, a new tab). When the action closed the tab
/// (a popup's "Close"), the agent's current tab is described instead.
async fn summary(this: &WeakEntity<AgentChrome>, page: &Page, cx: &mut AsyncApp) -> Result<Value> {
    let alive = this.update(cx, |this, _| this.tab(&page.target).is_some())?;
    if alive {
        release(this, page, cx).await;
    }
    let page = if alive {
        page.clone()
    } else {
        let current = this.update(cx, |this, _| {
            let target = this.owners.get(&page.owner)?.current.clone()?;
            let tab = this.tab(&target)?;
            Some(tab.page(&page.owner, &page.label))
        })?;
        match current {
            Some(current) => current,
            None => {
                let notes = this.update(cx, |this, _| {
                    this.owners
                        .get_mut(&page.owner)
                        .map(|owner| std::mem::take(&mut owner.notes))
                        .unwrap_or_default()
                })?;
                return Ok(json!({ "title": "", "url": "", "notes": notes }));
            }
        }
    };
    // The title as the page has it now (Chrome tells of changes late).
    let mut human_check = false;
    if let Ok(now) = in_world(
        this,
        &page,
        "({ title: document.title, url: location.href, check: __phAgent.humanCheck() })",
        cx,
    )
    .await
    {
        human_check = now["check"] == json!(true);
        this.update(cx, |this, _| {
            if let Some(tab) = this.tab_mut(&page.target) {
                if let Some(title) = now["title"].as_str() {
                    tab.title = title.to_string();
                }
                if let Some(url) = now["url"].as_str() {
                    tab.url = url.to_string();
                }
            }
        })?;
    }
    let mut notes = notes(this, &page, cx)?;
    if human_check {
        let _ = this.update(cx, |this, _| {
            this.hand_off(
                &page.target,
                &page.label,
                &format!(
                    "This page asks to prove a person is here. Complete it in the tab, then tell {} to go on.",
                    page.label
                ),
            )
        });
    }
    if human_check && let Some(notes) = notes.as_array_mut() {
        notes.push(json!(
            "The page asks to prove a person is here (a CAPTCHA or similar check). Do not try to \
             solve it: ask the user to complete it in that tab of their Chrome, then go on."
        ));
    }
    this.update(cx, |this, _| {
        let tab = this.tab(&page.target);
        json!({
            "title": tab.map(|tab| tab.title.clone()).unwrap_or_default(),
            "url": tab.map(|tab| tab.url.clone()).unwrap_or_default(),
            "notes": notes,
        })
    })
}

fn notes(this: &WeakEntity<AgentChrome>, page: &Page, cx: &mut AsyncApp) -> Result<Value> {
    this.update(cx, |this, _| {
        let mut notes: Vec<String> = Vec::new();
        if let Some(state) = this.owners.get_mut(&page.owner) {
            notes.append(&mut state.notes);
        }
        if let Some(tab) = this.tab_mut(&page.target) {
            if let Some(dialog) = &tab.dialog {
                notes.push(dialog.waiting().to_string());
            }
            let since = tab.hinted;
            let errors: Vec<&str> = tab
                .logs
                .console
                .iter()
                .filter(|line| line.seq >= since && line.level == Level::Error)
                .map(|line| line.text.as_str())
                .collect();
            if let Some(first) = errors.first() {
                let first: String = first.chars().take(160).collect();
                notes.push(format!(
                    "{} new console error{} (first: {first}); see chrome_console.",
                    errors.len(),
                    if errors.len() == 1 { "" } else { "s" }
                ));
            }
            let failed = tab
                .logs
                .network
                .iter()
                .filter(|request| request.seq >= since && request.is_error())
                .count();
            if failed > 0 {
                notes.push(format!(
                    "{failed} failed request{}; see chrome_network with errors_only.",
                    if failed == 1 { "" } else { "s" }
                ));
            }
            if tab.loading {
                notes.push("The page is still loading.".into());
            }
            tab.hinted = tab.logs.mark();
        }
        json!(notes)
    })
}

fn rank(level: Level) -> u8 {
    match level {
        Level::Debug => 0,
        Level::Info => 1,
        Level::Warning => 2,
        Level::Error => 3,
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// A file name a site suggested, safe to save under.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned
    }
}

/// The files to upload: absolute, existing, and inside `allowed` folders or
/// among `downloaded` files.
fn checked_files(
    paths: &[String],
    allowed: &[PathBuf],
    downloaded: &[PathBuf],
) -> Result<Vec<String>> {
    if paths.is_empty() {
        bail!("give the files to upload (`paths`)");
    }
    paths
        .iter()
        .map(|path| {
            let path = PathBuf::from(path);
            let real = path
                .canonicalize()
                .with_context(|| format!("{} does not exist", path.display()))?;
            if !real.is_file() {
                bail!("{} is not a file", path.display());
            }
            let inside = allowed.iter().any(|folder| {
                folder
                    .canonicalize()
                    .is_ok_and(|folder| real.starts_with(folder))
            }) || downloaded
                .iter()
                .any(|file| file.canonicalize().is_ok_and(|file| file == real));
            if !inside {
                bail!(
                    "{} is outside the workspace's folders and is not a file you downloaded; only those can be uploaded",
                    path.display()
                );
            }
            Ok(real.to_string_lossy().into_owned())
        })
        .collect()
}

/// `Control+Shift+K` as a key and DevTools modifier bits (Alt 1, Control 2,
/// Meta 4, Shift 8).
/// The agent session's process, from its owner key (`<agent>-<pid>`).
fn owner_pid(owner: &str) -> Option<u32> {
    owner
        .rsplit_once('-')?
        .1
        .parse()
        .ok()
        .filter(|pid| *pid > 1)
}

/// Whether process `pid` still runs.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 sends nothing; it only asks whether it could.
    let sent = unsafe { libc::kill(pid, 0) } == 0;
    sent || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the handle is checked before use and closed after.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        read && code == STILL_ACTIVE as u32
    }
}

/// DevTools' modifier bits (Alt 1, Shift 8).
const MODIFIER_CTRL: u32 = 2;
const MODIFIER_META: u32 = 4;

fn parse_combo(combo: &str) -> Result<(String, u32)> {
    // `Control++`: the key is the plus itself.
    let (rest, plus) = match combo.strip_suffix("++") {
        Some(rest) => (rest, true),
        None => (combo, combo == "+"),
    };
    let mut parts: Vec<&str> = rest
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if plus {
        parts.retain(|part| *part != "+");
        parts.push("+");
    }
    let Some((key, held)) = parts.split_last() else {
        bail!("give a key, like Enter or Control+A");
    };
    let mut modifiers = 0;
    for modifier in held {
        modifiers |= match modifier.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "control" | "ctrl" => MODIFIER_CTRL,
            "meta" | "cmd" | "command" | "super" => MODIFIER_META,
            "shift" => 8,
            other => bail!("unknown modifier `{other}`: Alt, Control, Meta or Shift"),
        };
    }
    Ok((key.to_string(), modifiers))
}

/// The DevTools key events (down, up) for `key` with `modifiers`.
fn key_events(key: &str, modifiers: u32) -> Result<Vec<Value>> {
    let named: Option<(&str, &str, u32, Option<&str>)> = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(("Enter", "Enter", 13, Some("\r"))),
        "tab" => Some(("Tab", "Tab", 9, None)),
        "escape" | "esc" => Some(("Escape", "Escape", 27, None)),
        "backspace" => Some(("Backspace", "Backspace", 8, None)),
        "delete" => Some(("Delete", "Delete", 46, None)),
        "space" | " " => Some((" ", "Space", 32, Some(" "))),
        "arrowleft" | "left" => Some(("ArrowLeft", "ArrowLeft", 37, None)),
        "arrowup" | "up" => Some(("ArrowUp", "ArrowUp", 38, None)),
        "arrowright" | "right" => Some(("ArrowRight", "ArrowRight", 39, None)),
        "arrowdown" | "down" => Some(("ArrowDown", "ArrowDown", 40, None)),
        "home" => Some(("Home", "Home", 36, None)),
        "end" => Some(("End", "End", 35, None)),
        "pageup" => Some(("PageUp", "PageUp", 33, None)),
        "pagedown" => Some(("PageDown", "PageDown", 34, None)),
        _ => None,
    };
    if let Some((key, code, vk, text)) = named {
        // A shortcut types nothing.
        let text = text.filter(|_| modifiers & 0b110 == 0);
        let mut down = json!({
            "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers,
        });
        if let Some(text) = text {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        let up = json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers });
        return Ok(vec![down, up]);
    }
    if let Some(number) = key
        .strip_prefix(['F', 'f'])
        .and_then(|n| n.parse::<u32>().ok())
        && (1..=12).contains(&number)
    {
        let name = format!("F{number}");
        let vk = 111 + number;
        return Ok(vec![
            json!({ "type": "rawKeyDown", "key": name, "code": name, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
            json!({ "type": "keyUp", "key": name, "code": name, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
        ]);
    }
    let mut characters = key.chars();
    let (Some(character), None) = (characters.next(), characters.next()) else {
        bail!(
            "unknown key `{key}`: a single character, or Enter, Tab, Escape, Backspace, Delete, Space, Arrow keys, Home, End, PageUp, PageDown, F1–F12"
        );
    };
    let upper = character.to_ascii_uppercase();
    let code = if upper.is_ascii_alphabetic() {
        format!("Key{upper}")
    } else if character.is_ascii_digit() {
        format!("Digit{character}")
    } else {
        String::new()
    };
    let vk = if upper.is_ascii_alphanumeric() {
        upper as u32
    } else {
        0
    };
    let shortcut = modifiers & 0b110 != 0;
    let mut down = json!({
        "type": if shortcut { "rawKeyDown" } else { "keyDown" },
        "key": character.to_string(), "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers,
    });
    if !shortcut {
        down["text"] = json!(character.to_string());
        down["unmodifiedText"] = json!(character.to_string());
    } else {
        // Editing shortcuts need their command named on macOS.
        let command = match upper {
            'A' => Some("selectAll"),
            'C' => Some("copy"),
            'V' => Some("paste"),
            'X' => Some("cut"),
            'Z' if modifiers & 8 != 0 => Some("redo"),
            'Z' => Some("undo"),
            _ => None,
        };
        if let Some(command) = command {
            down["commands"] = json!([command]);
        }
    }
    let up = json!({ "type": "keyUp", "key": character.to_string(), "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers });
    Ok(vec![down, up])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_is_only_the_file_it_wrote() {
        let folder = std::env::temp_dir().join(format!("ph-downloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        // The user's own file, from before, under the name a page suggests.
        let theirs = folder.join("Statement.pdf");
        std::fs::write(&theirs, b"12345").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&theirs)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
        let started = std::time::SystemTime::now();
        // Not the download: older than it, even at the size it had.
        assert_eq!(saved_download(&folder, "Statement.pdf", started, 5), None);
        // Chrome saved the download beside it, renamed.
        let ours = folder.join("Statement (1).pdf");
        std::fs::write(&ours, b"0123456789").unwrap();
        assert_eq!(
            saved_download(&folder, "Statement.pdf", started, 10),
            Some(ours.clone())
        );
        // The wrong size is not it either.
        assert_eq!(saved_download(&folder, "Statement.pdf", started, 11), None);
        // A name without an extension, saved as it is.
        std::fs::write(folder.join("notes"), b"abc").unwrap();
        assert_eq!(
            saved_download(&folder, "notes", started, 3),
            Some(folder.join("notes"))
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn ended_agent_sessions_are_told_by_their_process() {
        assert_eq!(owner_pid("claude-code-4242"), Some(4242));
        assert_eq!(owner_pid("codex"), None);
        assert!(process_alive(std::process::id()));
        // A finished child process is gone.
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit"][..]
            } else {
                &[][..]
            })
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_alive(pid));
    }

    #[test]
    fn key_combinations() {
        assert_eq!(
            parse_combo("Control+Shift+K").unwrap(),
            ("K".to_string(), 10)
        );
        assert_eq!(parse_combo("Enter").unwrap(), ("Enter".to_string(), 0));
        assert_eq!(parse_combo("Meta++").unwrap(), ("+".to_string(), 4));
        assert!(parse_combo("Hyper+K").is_err());
        let enter = key_events("Enter", 0).unwrap();
        assert_eq!(enter[0]["text"], "\r");
        let select_all = key_events("a", 4).unwrap();
        assert_eq!(select_all[0]["type"], "rawKeyDown");
        assert_eq!(select_all[0]["commands"], json!(["selectAll"]));
        assert!(
            select_all[0].get("text").is_none(),
            "a shortcut types nothing"
        );
        let typed = key_events("é", 0).unwrap();
        assert_eq!(typed[0]["text"], "é");
        assert_eq!(
            key_events("F5", 0).unwrap()[0]["windowsVirtualKeyCode"],
            116
        );
        assert!(key_events("NotAKey", 0).is_err());
    }

    #[test]
    fn downloads_get_safe_names() {
        assert_eq!(sanitize_name("../a:b?.pdf"), "_a_b_.pdf");
        assert_eq!(sanitize_name("  "), "download");
    }

    #[test]
    fn uploads_only_from_allowed_folders() {
        let root = std::env::temp_dir().join(format!("ph-upload-{}", std::process::id()));
        let inside = root.join("project");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("a.png"), "x").unwrap();
        std::fs::write(root.join("secret.txt"), "x").unwrap();
        let allowed = vec![inside.clone()];
        assert!(
            checked_files(
                &[inside.join("a.png").to_string_lossy().into_owned()],
                &allowed,
                &[]
            )
            .is_ok()
        );
        let secret = root.join("secret.txt").to_string_lossy().into_owned();
        let outside = checked_files(std::slice::from_ref(&secret), &allowed, &[]);
        assert!(outside.unwrap_err().to_string().contains("outside"));
        // A way out through `..` is still outside.
        let sneaky = inside.join("../secret.txt").to_string_lossy().into_owned();
        assert!(checked_files(&[sneaky], &allowed, &[]).is_err());
        assert!(checked_files(&[], &allowed, &[]).is_err());
        // A file the agent downloaded may go back up; its neighbours may not.
        std::fs::write(root.join("report.pdf"), "x").unwrap();
        let downloaded = vec![root.join("report.pdf")];
        assert!(
            checked_files(
                &[root.join("report.pdf").to_string_lossy().into_owned()],
                &allowed,
                &downloaded
            )
            .is_ok()
        );
        assert!(checked_files(&[secret], &allowed, &downloaded).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
