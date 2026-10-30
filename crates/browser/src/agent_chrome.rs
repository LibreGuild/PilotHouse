//! Agents' Chrome: a real, visible Chrome (or Edge) that PilotHouse starts
//! with a profile of its own, for agents to use websites as a person would.
//!
//! - **One browser, one profile** for every workspace and chat
//!   ([`chrome::agent_profile_dir`]): a site signed in to once stays signed
//!   in for all of them, and across restarts.
//! - **A tab per agent** (each agent session is an *owner*): the first takes
//!   the window's first tab, the others open windows of their own, so they
//!   never act in each other's pages. Tabs a page opens (a sign-in popup)
//!   become the agent's current tab; a tab the user closes is forgotten.
//! - **Visible control**: while an agent acts, its tab glows, a pointer goes
//!   where it clicks, and a bar names the agent with a Stop button. Stop
//!   pauses that agent here until the user lets it continue.
//! - **Logs** per tab: console messages and errors with where they came
//!   from, and every request with its headers (credentials hidden), timing,
//!   size and, on request, its body.
//!
//! Chrome is driven over its private debugging pipe (no port another
//! program could use) and ends when PilotHouse does. If the user quits it,
//! the next agent action starts it again; logins stay in the profile.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, SharedString, Task,
    WeakEntity,
};
use ph_store::Store;
use serde_json::{Value, json};

use crate::agent_scripts::{self, AGENT, BINDING, OVERLAY, WORLD};
use crate::cdp::{Cdp, CdpEvent};
use crate::chrome::{self, Chrome};
use crate::logs::{Level, Logs};

/// How long navigation waits for the page's load event.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// The glow stays this long after an agent's last action: long enough to
/// stay on while the agent thinks between steps, so it does not flicker.
const GLOW_AFTER: Duration = Duration::from_secs(20);
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

#[derive(Debug, Clone, PartialEq)]
pub enum AgentChromeStatus {
    Stopped,
    Starting,
    Running,
    /// No Chromium-family browser is installed.
    NoBrowser,
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

#[derive(Debug, Clone)]
pub struct Download {
    pub url: String,
    pub name: String,
    pub path: PathBuf,
    /// `inProgress`, `completed` or `canceled`.
    pub state: String,
    pub received: u64,
    pub total: u64,
}

struct Running {
    /// Kept for the connection's lifetime; dropping it ends Chrome.
    _chrome: Chrome,
    browser: Cdp,
}

struct Owner {
    label: String,
    /// Its tabs, oldest first.
    tabs: Vec<String>,
    current: Option<String>,
    /// The user pressed Stop on its tab.
    paused: bool,
    /// Chrome was quit with its tabs open.
    lost: bool,
    /// Said with the next result (a popup became the current tab…).
    notes: Vec<String>,
}

struct Tab {
    target: String,
    session: String,
    /// A tab the user opened that the agent took over: given back, not
    /// closed, when the agent is done with it.
    user: bool,
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
    _glow: Option<Task<()>>,
}

type StartTask = Shared<Task<Result<Cdp, SharedString>>>;

pub struct AgentChrome {
    store: Store,
    status: AgentChromeStatus,
    running: Option<Running>,
    starting: Option<StartTask>,
    /// Counts stops, so a start that finishes after one is dropped.
    generation: u64,
    tabs: Vec<Tab>,
    owners: HashMap<String, Owner>,
    /// The first window's tab, not given to an agent yet.
    spare: Option<String>,
    downloads: Vec<(String, Download)>,
    _events: Option<Task<()>>,
}

impl EventEmitter<AgentChromeEvent> for AgentChrome {}

struct AgentChromeHub(Entity<AgentChrome>);

impl Global for AgentChromeHub {}

/// What an action works on: the agent's tab and its connection.
#[derive(Clone)]
struct Page {
    cdp: Cdp,
    target: String,
    /// The agent session it is for.
    owner: String,
    label: String,
}

impl AgentChrome {
    pub fn init(store: Store, cx: &mut App) {
        let chrome = cx.new(|_| Self {
            store,
            status: AgentChromeStatus::Stopped,
            running: None,
            starting: None,
            generation: 0,
            tabs: Vec::new(),
            owners: HashMap::new(),
            spare: None,
            downloads: Vec::new(),
            _events: None,
        });
        cx.set_global(AgentChromeHub(chrome));
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

    /// Where downloads go: a folder of the user's Downloads.
    pub fn downloads_dir() -> PathBuf {
        let home = ph_env::var("HOME")
            .or_else(|| ph_env::var("USERPROFILE"))
            .map(PathBuf::from);
        match home {
            Some(home) if home.join("Downloads").is_dir() => {
                home.join("Downloads").join("PilotHouse")
            }
            _ => ph_store::paths::data_dir().join("downloads"),
        }
    }

    fn tab(&self, target: &str) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.target == target)
    }

    fn tab_mut(&mut self, target: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.target == target)
    }

    // ---- Starting --------------------------------------------------------

    /// The browser connection, starting Chrome first if needed. Callers that
    /// come while it starts share the same start.
    fn connection(&mut self, cx: &mut Context<Self>) -> Task<Result<Cdp>> {
        if let Some(running) = &self.running {
            return Task::ready(Ok(running.browser.clone()));
        }
        let start = match &self.starting {
            Some(start) => start.clone(),
            None => {
                let chosen: String = self.store.setting(chrome::CHOSEN_SETTING);
                let chosen = (!chosen.is_empty()).then(|| PathBuf::from(chosen));
                let Some(binary) = chrome::find_chrome(chosen.as_deref()) else {
                    self.status = AgentChromeStatus::NoBrowser;
                    cx.notify();
                    cx.emit(AgentChromeEvent::Changed);
                    return Task::ready(Err(anyhow!(
                        "No Chrome or Edge was found on this computer. Ask the user to install Chrome (or Edge, Brave, Chromium), or to choose one in PilotHouse's Browser pane."
                    )));
                };
                let start = self.start(binary, cx);
                self.starting = Some(start.clone());
                start
            }
        };
        cx.spawn(async move |_, _| start.await.map_err(|err| anyhow!("{err}")))
    }

    fn start(&mut self, binary: PathBuf, cx: &mut Context<Self>) -> StartTask {
        self.status = AgentChromeStatus::Starting;
        cx.notify();
        cx.emit(AgentChromeEvent::Changed);
        let generation = self.generation;
        let downloads = Self::downloads_dir();
        cx.spawn(async move |this, cx| {
            let launched = cx
                .background_spawn(async move {
                    let mut chrome = Chrome::launch_visible(
                        &binary,
                        &chrome::agent_profile_dir(),
                        &start_page(),
                    )?;
                    let (reader, writer) = (chrome.reader.take(), chrome.writer.take());
                    let (browser, events) = Cdp::connect(
                        reader.context("Chrome pipe")?,
                        writer.context("Chrome pipe")?,
                    )?;
                    anyhow::Ok((chrome, browser, events))
                })
                .await;
            let prepared = match launched {
                Ok((chrome, browser, events)) => {
                    match prepare_browser(&browser, &downloads).await {
                        Ok(spare) => Ok((chrome, browser, events, spare)),
                        Err(err) => Err(err),
                    }
                }
                Err(err) => Err(err),
            };
            let result = match prepared {
                Ok((chrome, browser, events, spare)) => this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return Err(anyhow!("the browser was stopped"));
                        }
                        this.started(chrome, browser.clone(), events, spare, cx);
                        Ok(browser)
                    })
                    .map_err(|err| anyhow!("{err}"))
                    .and_then(|started| started),
                Err(err) => Err(err),
            };
            result.map_err(|err| {
                let message: SharedString = format!("{err:#}").into();
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.starting = None;
                    this.status = AgentChromeStatus::Failed(message.clone());
                    cx.notify();
                    cx.emit(AgentChromeEvent::Changed);
                });
                message
            })
        })
        .shared()
    }

    fn started(
        &mut self,
        chrome: Chrome,
        browser: Cdp,
        events: smol::channel::Receiver<CdpEvent>,
        spare: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.running = Some(Running {
            _chrome: chrome,
            browser,
        });
        self.starting = None;
        self.spare = spare;
        self.status = AgentChromeStatus::Running;
        let generation = self.generation;
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.handle_event(event, cx))
                    .is_err()
                {
                    return;
                }
            }
            // Chrome closed (the user quit it): start again on next use.
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.forget_browser(cx);
                }
            });
        }));
        cx.notify();
        cx.emit(AgentChromeEvent::Changed);
    }

    fn forget_browser(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.running = None;
        self.starting = None;
        self._events = None;
        self.tabs.clear();
        self.spare = None;
        for owner in self.owners.values_mut() {
            owner.lost |= !owner.tabs.is_empty();
            owner.tabs.clear();
            owner.current = None;
        }
        self.status = AgentChromeStatus::Stopped;
        cx.notify();
        cx.emit(AgentChromeEvent::Changed);
    }

    /// Shows the browser to the user (to sign in to sites, or to watch),
    /// starting it if needed.
    pub fn show(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let connection = self.connection(cx);
        cx.spawn(async move |this, cx| {
            let browser = connection.await?;
            let target = this.update(cx, |this, _| {
                this.spare
                    .clone()
                    .or_else(|| this.tabs.last().map(|tab| tab.target.clone()))
            })?;
            let target = match target {
                Some(target) => target,
                None => {
                    let created = browser
                        .call(
                            "Target.createTarget",
                            json!({ "url": start_page(), "newWindow": true }),
                        )
                        .await?;
                    let target = created["targetId"]
                        .as_str()
                        .context("Chrome did not open a window")?
                        .to_string();
                    this.update(cx, |this, _| this.spare = Some(target.clone()))?;
                    target
                }
            };
            browser
                .call("Target.activateTarget", json!({ "targetId": target }))
                .await?;
            Ok(())
        })
    }

    // ---- Events ----------------------------------------------------------

    fn handle_event(&mut self, event: CdpEvent, cx: &mut Context<Self>) {
        let Some(session) = event.session.clone() else {
            self.handle_browser_event(&event, cx);
            return;
        };
        let Some(index) = self.tabs.iter().position(|tab| tab.session == session) else {
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
            "Runtime.bindingCalled" if params["name"] == BINDING => {
                let stop = params["payload"] == "stop";
                let owner = self.tabs[index].owner.clone();
                self.set_paused(&owner, stop, cx);
            }
            // Logs: kept for the agent to ask about; nothing to redraw.
            method => {
                self.tabs[index].logs.record(method, params);
                return;
            }
        }
        cx.notify();
    }

    fn handle_browser_event(&mut self, event: &CdpEvent, cx: &mut Context<Self>) {
        let params = &event.params;
        match event.method.as_str() {
            "Target.targetCreated" => {
                // A page an agent's page opened (a link to a new tab, a
                // sign-in popup) becomes that agent's current tab.
                let info = &params["targetInfo"];
                let opener = info["openerId"].as_str();
                if info["type"] != "page" {
                    return;
                }
                let Some(owner) = opener
                    .and_then(|opener| self.tab(opener))
                    .map(|tab| tab.owner.clone())
                else {
                    return;
                };
                let Some(target) = info["targetId"].as_str().map(str::to_string) else {
                    return;
                };
                if self.tab(&target).is_some() {
                    return;
                }
                let (url, title) = (
                    info["url"].as_str().unwrap_or_default().to_string(),
                    info["title"].as_str().unwrap_or_default().to_string(),
                );
                self.adopt(owner, target, url, title, cx);
            }
            "Target.targetInfoChanged" => {
                let info = &params["targetInfo"];
                if let Some(tab) = info["targetId"]
                    .as_str()
                    .and_then(|target| self.tabs.iter_mut().find(|tab| tab.target == target))
                {
                    tab.title = info["title"].as_str().unwrap_or_default().to_string();
                    if let Some(url) = info["url"].as_str() {
                        tab.url = url.to_string();
                    }
                    cx.notify();
                    cx.emit(AgentChromeEvent::Changed);
                }
            }
            "Target.targetDestroyed" | "Target.targetCrashed" => {
                if let Some(target) = params["targetId"].as_str() {
                    let crashed = event.method == "Target.targetCrashed";
                    self.forget_tab(target, crashed, cx);
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(target) = params["sessionId"]
                    .as_str()
                    .and_then(|session| self.tabs.iter().find(|tab| tab.session == session))
                    .map(|tab| tab.target.clone())
                {
                    self.forget_tab(&target, false, cx);
                }
            }
            "Browser.downloadWillBegin" => {
                let guid = params["guid"].as_str().unwrap_or_default().to_string();
                let name =
                    sanitize_name(params["suggestedFilename"].as_str().unwrap_or("download"));
                self.downloads.push((
                    guid.clone(),
                    Download {
                        url: params["url"].as_str().unwrap_or_default().to_string(),
                        path: Self::downloads_dir().join(&guid),
                        name,
                        state: "inProgress".into(),
                        received: 0,
                        total: 0,
                    },
                ));
                if self.downloads.len() > 100 {
                    self.downloads.remove(0);
                }
            }
            "Browser.downloadProgress" => {
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
                    // Saved under its id; named as the site named it.
                    let named = unique_path(&Self::downloads_dir(), &download.name);
                    if std::fs::rename(&download.path, &named).is_ok() {
                        download.path = named;
                    }
                }
            }
            _ => {}
        }
    }

    fn forget_tab(&mut self, target: &str, crashed: bool, cx: &mut Context<Self>) {
        if self.spare.as_deref() == Some(target) {
            self.spare = None;
        }
        let Some((tab, was_current)) = self.remove_tab(target) else {
            return;
        };
        if was_current && let Some(owner) = self.owners.get_mut(&tab.owner) {
            owner.notes.push(match (crashed, &owner.current) {
                (true, _) => format!("Your tab ({}) crashed.", tab.url),
                (false, Some(_)) => format!(
                    "Your tab ({}) was closed; the one before it is current again.",
                    tab.url
                ),
                (false, None) => format!("Your tab ({}) was closed.", tab.url),
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
        let browser = self.running.as_ref().map(|running| running.browser.clone());
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
                let (cdp, world, session) = (tab.cdp.clone(), tab.world, tab.session.clone());
                cx.background_spawn(async move {
                    if let Some(id) = world {
                        let hide = agent_scripts::call("__phOverlay", "hide", &[]);
                        cdp.call(
                            "Runtime.evaluate",
                            json!({ "expression": hide, "contextId": id }),
                        )
                        .await
                        .ok();
                    }
                    browser
                        .call("Target.detachFromTarget", json!({ "sessionId": session }))
                        .await
                        .ok();
                })
                .detach();
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
            if let Some(tab) = self.tab_mut(&target) {
                let page = Page {
                    cdp: tab.cdp.clone(),
                    target: target.clone(),
                    owner: owner.to_string(),
                    label: label.clone(),
                };
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

    /// The agent's current page; with `open`, one is opened when it has
    /// none (or `fresh`: a new one anyway).
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
            return Task::ready(Ok(Page {
                cdp: tab.cdp.clone(),
                target,
                owner: who.owner.clone(),
                label,
            }));
        }
        if !open {
            return Task::ready(Err(if lost {
                anyhow!(
                    "Chrome was closed (perhaps by the user), so your tab is gone: open the page again with chrome_open (sign-ins are kept)."
                )
            } else {
                anyhow!("You have no page open in Chrome yet: open one with chrome_open.")
            }));
        }
        self.owner_mut(who).lost = false;
        let first =
            self.tabs.iter().all(|tab| tab.owner == who.owner) && owner_has_no_tabs(self, who);
        // Others' tabs are in front: a window of its own.
        let new_window = !self.tabs.iter().all(|tab| tab.owner == who.owner);
        let connection = self.connection(cx);
        let owner = who.owner.clone();
        cx.spawn(async move |this, cx| {
            let browser = connection.await?;
            // The first agent works in the first window's tab while it is
            // still blank; one the user went to a page in stays theirs.
            let spare = if first {
                blank_tab(&browser).await
            } else {
                None
            };
            let target = match spare {
                Some(target) => target,
                None => {
                    let created = browser
                        .call(
                            "Target.createTarget",
                            json!({ "url": "about:blank", "newWindow": new_window }),
                        )
                        .await?;
                    created["targetId"]
                        .as_str()
                        .context("Chrome did not open a tab")?
                        .to_string()
                }
            };
            let (session, cdp) = attach(&browser, &target).await?;
            this.update(cx, |this, _| {
                if this.spare.as_deref() == Some(target.as_str()) {
                    this.spare = None;
                }
                this.add_tab(&owner, target.clone(), session, cdp.clone());
            })?;
            Ok(Page {
                cdp,
                target,
                owner,
                label,
            })
        })
    }

    fn add_tab(&mut self, owner: &str, target: String, session: String, cdp: Cdp) {
        self.tabs.push(Tab {
            target: target.clone(),
            session,
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
            user: false,
            _glow: None,
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
        let Some(browser) = self.running.as_ref().map(|running| running.browser.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Ok((session, cdp)) = attach(&browser, &target).await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.tab(&target).is_some() {
                    return;
                }
                this.add_tab(&owner, target.clone(), session, cdp);
                if let Some(tab) = this.tab_mut(&target) {
                    tab.url = url;
                    tab.title = title;
                }
                if let Some(state) = this.owners.get_mut(&owner) {
                    state.notes.push(
                        "Your page opened a new tab; it is now your current tab (chrome_tabs lists them).".into(),
                    );
                }
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

    /// The agent's tabs: `list` (with the tabs the user opened), `new`
    /// (optionally at `url`), `switch` to or `close` the tab at `index`
    /// (a tab taken from the user is given back, not closed), and `take`
    /// the user's tab `tab` (by id; without one, the tab on screen).
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
                let Some(browser) = self.running.as_ref().map(|running| running.browser.clone())
                else {
                    return Task::ready(Ok(json!({ "text": mine })));
                };
                let agents: Vec<String> = self.tabs.iter().map(|tab| tab.target.clone()).collect();
                cx.background_spawn(async move {
                    let users = user_tabs(&browser, &agents).await.unwrap_or_default();
                    let mut text = mine;
                    if !users.is_empty() {
                        text.push_str(
                            "\n\nThe user's tabs (chrome_tabs take with the id works in one; without an id, the one on screen):",
                        );
                        for tab in &users {
                            let place = match (tab.showing, tab.focused) {
                                (_, true) => " (on screen, the user is in it)",
                                (true, false) => " (on screen)",
                                _ => "",
                            };
                            text.push_str(&format!(
                                "\n[{}] {} — {}{place}",
                                tab.id(),
                                tab.title,
                                tab.url
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
                let Some(browser) = self.running.as_ref().map(|running| running.browser.clone())
                else {
                    return Task::ready(Err(anyhow!("Chrome is not running")));
                };
                let given_back = self.tab(&target).filter(|tab| tab.user).map(|tab| Page {
                    cdp: tab.cdp.clone(),
                    target: target.clone(),
                    owner: who.owner.clone(),
                    label,
                });
                cx.spawn(async move |this, cx| {
                    // The user's tab stays open: the overlay goes, and the
                    // agent lets go of it.
                    if let Some(page) = given_back {
                        let hide = agent_scripts::call("__phOverlay", "hide", &[]);
                        let _ = in_world(&this, &page, &hide, cx).await;
                        let session = this.update(cx, |this, cx| {
                            let removed = this.remove_tab(&target);
                            cx.notify();
                            cx.emit(AgentChromeEvent::Changed);
                            removed.map(|(tab, _)| tab.session)
                        })?;
                        if let Some(session) = session {
                            browser
                                .call("Target.detachFromTarget", json!({ "sessionId": session }))
                                .await
                                .ok();
                        }
                        return Ok(
                            json!({ "text": "Gave the tab back to the user; it stays open." }),
                        );
                    }
                    browser
                        .call("Target.closeTarget", json!({ "targetId": target }))
                        .await?;
                    this.update(cx, |this, cx| {
                        this.forget_tab(&target, false, cx);
                    })?;
                    Ok(json!({ "text": "Closed." }))
                })
            }
            other => Task::ready(Err(anyhow!(
                "unknown action `{other}`: list, new, switch, close or take"
            ))),
        }
    }

    /// Takes over a tab the user opened (by `id` from the list; without
    /// one, the tab on screen): it becomes the agent's current tab, with
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
        let id = id.map(|id| id.trim().trim_matches(['[', ']']).to_ascii_uppercase());
        // A tab an agent already works in.
        if let Some(id) = &id
            && let Some(tab) = self
                .tabs
                .iter()
                .find(|tab| tab.target.starts_with(id.as_str()))
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
        let Some(browser) = self.running.as_ref().map(|running| running.browser.clone()) else {
            return Task::ready(Err(anyhow!(
                "Chrome is not open, so the user has no tabs in it: ask them to open the page in the PilotHouse Chrome window first."
            )));
        };
        let agents: Vec<String> = self.tabs.iter().map(|tab| tab.target.clone()).collect();
        let (owner, label) = (who.owner.clone(), who.label.clone());
        cx.spawn(async move |this, cx| {
            let users = user_tabs(&browser, &agents).await?;
            let chosen = match &id {
                Some(id) => users
                    .iter()
                    .find(|tab| tab.target.starts_with(id.as_str()))
                    .with_context(|| format!("the user has no tab [{id}] (chrome_tabs list shows them)"))?,
                None => users.iter().find(|tab| tab.showing).context(
                    "no tab of the user's is on screen: ask which one, or give its id from chrome_tabs list",
                )?,
            };
            let (target, url, title) = (chosen.target.clone(), chosen.url.clone(), chosen.title.clone());
            let (session, cdp) = attach(&browser, &target).await?;
            let taken = this.update(cx, |this, cx| {
                // Another agent took it meanwhile.
                if this.tab(&target).is_some() {
                    return false;
                }
                this.add_tab(&owner, target.clone(), session.clone(), cdp.clone());
                if let Some(tab) = this.tab_mut(&target) {
                    tab.user = true;
                    tab.url = url;
                    tab.title = title;
                }
                if this.spare.as_deref() == Some(target.as_str()) {
                    this.spare = None;
                }
                cx.notify();
                cx.emit(AgentChromeEvent::Changed);
                true
            })?;
            if !taken {
                browser
                    .call("Target.detachFromTarget", json!({ "sessionId": session }))
                    .await
                    .ok();
                bail!("another agent took that tab just now");
            }
            let page = Page {
                cdp,
                target,
                owner,
                label,
            };
            begin(&this, &page, cx).await;
            let mut result = summary(&this, &page, cx).await?;
            if let Some(notes) = result["notes"].as_array_mut() {
                notes.insert(
                    0,
                    json!("You took over the user's tab; it is your current tab. Keep what the user already typed or chose: fill only what they asked you to. chrome_tabs close gives it back without closing it."),
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
                let user = if tab.user { " (the user's)" } else { "" };
                Some(format!(
                    "{index}: {} — {}{current}{user}",
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
                bail!(
                    "That is a password field: agents do not type passwords. Ask the user to sign in in the PilotHouse Chrome window (it keeps them signed in), then go on."
                );
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
                bail!(
                    "That is a password field: agents do not type passwords. Ask the user to sign in in the PilotHouse Chrome window (it keeps them signed in), then go on."
                );
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
        let page = Page {
            cdp: tab.cdp.clone(),
            target: tab.target.clone(),
            owner: who.owner.clone(),
            label: who.label.clone(),
        };
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

    /// Puts files in a file field. Only files of the workspace's folders or
    /// of the downloads folder can be given.
    pub fn upload(
        &mut self,
        who: &Who,
        project_id: Option<i64>,
        element: Value,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let allowed = self.allowed_folders(project_id);
        let files = match checked_files(&paths, &allowed) {
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
                .context("that is not a file field (input type=file)")?;
            settle(cx).await;
            summary(&this, &page, cx).await
        })
    }

    fn allowed_folders(&self, project_id: Option<i64>) -> Vec<PathBuf> {
        let mut folders = vec![Self::downloads_dir()];
        if let Some(project) = project_id.and_then(|id| self.store.project(id).ok().flatten()) {
            folders.extend(project.folders());
        }
        folders
    }

    /// Files downloaded in this browser, newest last.
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
                format!(
                    "{} ({progress}) → {}\n    from {}",
                    download.name,
                    download.path.display(),
                    download.url
                )
            })
            .collect();
        json!({ "text": if lines.is_empty() {
            format!("Nothing downloaded yet. Downloads go to {}.", Self::downloads_dir().display())
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

fn owner_has_no_tabs(chrome: &AgentChrome, who: &Who) -> bool {
    chrome
        .owners
        .get(&who.owner)
        .is_none_or(|owner| owner.tabs.is_empty())
}

/// Discovers targets, sends downloads to `downloads`, and returns the first
/// window's tab, for the first agent.
async fn prepare_browser(browser: &Cdp, downloads: &std::path::Path) -> Result<Option<String>> {
    browser
        .call("Target.setDiscoverTargets", json!({ "discover": true }))
        .await?;
    if std::fs::create_dir_all(downloads).is_ok() {
        browser
            .call(
                "Browser.setDownloadBehavior",
                json!({ "behavior": "allowAndName", "downloadPath": downloads, "eventsEnabled": true }),
            )
            .await
            .ok();
    }
    let targets = browser.call("Target.getTargets", json!({})).await?;
    Ok(targets["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| target["type"] == "page")
        .and_then(|target| target["targetId"].as_str())
        .map(str::to_string))
}

/// PilotHouse's page in the agents' Chrome: what it is for, shown in its
/// first tab and in windows opened for the user.
const START_PAGE: &str = include_str!("agent_start.html");

fn start_page_path() -> PathBuf {
    ph_store::paths::data_dir()
        .join("browser")
        .join("pilothouse-chrome.html")
}

/// The start page's address.
fn start_url() -> &'static str {
    static URL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| ph_ui::platform::file_url(&start_page_path()));
    &URL
}

/// The start page's address, written once a run (when it changed); a
/// blank tab when it cannot be written.
fn start_page() -> String {
    static PAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PAGE.get_or_init(|| {
        let path = start_page_path();
        if std::fs::read_to_string(&path).ok().as_deref() != Some(START_PAGE)
            && let Err(err) = std::fs::write(&path, START_PAGE)
        {
            tracing::warn!("writing the agents' Chrome start page: {err:#}");
            return "about:blank".into();
        }
        start_url().to_string()
    })
    .clone()
}

/// A tab the user opened, that no agent works in.
struct UserTab {
    target: String,
    title: String,
    url: String,
    /// The tab on screen in its window.
    showing: bool,
    /// In the window the user is in.
    focused: bool,
}

impl UserTab {
    /// How agents name it: the start of its target id.
    fn id(&self) -> &str {
        &self.target[..self.target.len().min(8)]
    }
}

/// The tabs the user opened (not `agents`'), the one they are in first.
async fn user_tabs(browser: &Cdp, agents: &[String]) -> Result<Vec<UserTab>> {
    let targets = browser.call("Target.getTargets", json!({})).await?;
    let mut tabs = Vec::new();
    for info in targets["targetInfos"].as_array().into_iter().flatten() {
        let Some(target) = info["targetId"].as_str() else {
            continue;
        };
        let url = info["url"].as_str().unwrap_or_default();
        // PilotHouse's own start page is nobody's work.
        if info["type"] != "page"
            || url.starts_with("devtools://")
            || url == start_url()
            || agents.iter().any(|known| known == target)
        {
            continue;
        }
        tabs.push(UserTab {
            target: target.to_string(),
            title: info["title"].as_str().unwrap_or_default().to_string(),
            url: url.to_string(),
            showing: false,
            focused: false,
        });
    }
    // Asked of every tab at once: a slow one does not hold up the others.
    let places =
        futures::future::join_all(tabs.iter().map(|tab| on_screen(browser, &tab.target))).await;
    for (tab, (showing, focused)) in tabs.iter_mut().zip(places) {
        (tab.showing, tab.focused) = (showing, focused);
    }
    tabs.sort_by_key(|tab| (!tab.focused, !tab.showing));
    Ok(tabs)
}

/// Whether a tab is on screen in its window, and whether that window is the
/// one the user is in: asked of the page through a short attachment.
async fn on_screen(browser: &Cdp, target: &str) -> (bool, bool) {
    let Ok(attached) = browser
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
        )
        .await
    else {
        return (false, false);
    };
    let Some(session) = attached["sessionId"].as_str().map(str::to_string) else {
        return (false, false);
    };
    let page = browser.for_session(&session);
    let ask = page.call(
        "Runtime.evaluate",
        json!({ "expression": "[document.visibilityState === 'visible', document.hasFocus()]", "returnByValue": true }),
    );
    // A page held by a dialog does not answer.
    let late = async {
        smol::Timer::after(Duration::from_secs(2)).await;
        Err(anyhow!("no answer"))
    };
    let state = smol::future::or(ask, late).await.ok();
    // Let go of it either way.
    browser
        .call("Target.detachFromTarget", json!({ "sessionId": session }))
        .await
        .ok();
    let value = state.as_ref().map(|state| &state["result"]["value"]);
    value.map_or((false, false), |value| {
        (value[0] == json!(true), value[1] == json!(true))
    })
}

/// A blank tab nobody is attached to (the first window's, when Chrome was
/// slow to open it).
async fn blank_tab(browser: &Cdp) -> Option<String> {
    let targets = browser.call("Target.getTargets", json!({})).await.ok()?;
    targets["targetInfos"]
        .as_array()?
        .iter()
        .find(|target| {
            target["type"] == "page"
                && target["attached"] != json!(true)
                && target["url"].as_str().is_some_and(|url| {
                    matches!(
                        url,
                        "about:blank" | "chrome://newtab/" | "chrome://new-tab-page/"
                    ) || url == start_url()
                })
        })
        .and_then(|target| target["targetId"].as_str())
        .map(str::to_string)
}

/// Attaches to a tab: its events, the overlay and helpers on every page,
/// and the Stop button's binding.
async fn attach(browser: &Cdp, target: &str) -> Result<(String, Cdp)> {
    let attached = browser
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
        )
        .await?;
    let session = attached["sessionId"]
        .as_str()
        .context("could not attach to the tab")?
        .to_string();
    let page = browser.for_session(&session);
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
    // Sites see the browser as a person's: no `navigator.webdriver` (the
    // launch flag that does it shows a warning bar in a visible Chrome).
    page.call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": agent_scripts::NOT_AUTOMATED }),
    )
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
    Ok((session, page))
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
                page.cdp.call("Page.bringToFront", json!({})).await.ok();
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
            Some(Page {
                cdp: tab.cdp.clone(),
                target,
                owner: page.owner.clone(),
                label: page.label.clone(),
            })
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
    if human_check && let Some(notes) = notes.as_array_mut() {
        notes.push(json!(
            "The page asks to prove a person is here (a CAPTCHA or similar check). Do not try to \
             solve it: ask the user to complete it in the PilotHouse Chrome window, then go on."
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

/// `name` in `dir`, or `name (2)`… when taken.
fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){extension}")))
        .find(|path| !path.exists())
        .expect("some name is free")
}

/// The files to upload: absolute, existing, and inside `allowed` folders.
fn checked_files(paths: &[String], allowed: &[PathBuf]) -> Result<Vec<String>> {
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
            });
            if !inside {
                bail!(
                    "{} is outside the workspace's folders and the downloads folder; only those files can be uploaded",
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
    fn downloads_get_safe_unique_names() {
        assert_eq!(sanitize_name("../a:b?.pdf"), "_a_b_.pdf");
        assert_eq!(sanitize_name("  "), "download");
        let dir = std::env::temp_dir().join(format!("ph-downloads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.pdf"), "").unwrap();
        assert_eq!(unique_path(&dir, "report.pdf"), dir.join("report (2).pdf"));
        assert_eq!(unique_path(&dir, "new.txt"), dir.join("new.txt"));
        let _ = std::fs::remove_dir_all(&dir);
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
                &allowed
            )
            .is_ok()
        );
        let outside = checked_files(
            &[root.join("secret.txt").to_string_lossy().into_owned()],
            &allowed,
        );
        assert!(outside.unwrap_err().to_string().contains("outside"));
        // A way out through `..` is still outside.
        let sneaky = inside.join("../secret.txt").to_string_lossy().into_owned();
        assert!(checked_files(&[sneaky], &allowed).is_err());
        assert!(checked_files(&[], &allowed).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
