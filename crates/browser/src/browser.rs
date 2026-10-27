//! One project's browser: a headless Chrome page and everything known about it.
//!
//! The pane and agents share this entity. Chrome starts on first use (the
//! pane opening, or an agent calling a browser tool) and keeps its profile in
//! the data directory, so logins survive restarts. Frames are only streamed
//! while the pane is visible; agents use one-off screenshots instead.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::{
    AppContext as _, AsyncApp, Context, EventEmitter, Keystroke, SharedString, Subscription, Task,
    WeakEntity,
};
use ph_media::{Frame, PointerKind, SurfacePointer};
use ph_store::Store;
use serde_json::{Value, json};

use crate::cdp::{Cdp, CdpEvent};
use crate::chrome::{self, Chrome};
use crate::input;
use crate::logs::Logs;
use crate::scripts;

/// How long agent navigation waits for the page's load event.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// Shortest time between two frames shown in the pane (about 30 a second).
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// A browser nobody has looked at or used for this long is stopped to free
/// its memory. Its profile (logins) stays; the next use starts it again.
const IDLE_STOP: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq)]
pub enum BrowserStatus {
    Stopped,
    Starting,
    Running,
    /// No Chromium-family browser is installed (or the chosen one is gone).
    NoBrowser,
    Failed(SharedString),
}

pub enum BrowserEvent {
    /// A new screencast frame.
    Frame(Frame),
    /// URL, title, loading state or logs changed.
    Changed,
}

/// The page's size in CSS pixels, and device pixels per CSS pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// A fixed screen the page is shown at instead of the pane's size, for UI
/// test runs on several sizes. `mobile` lays the page out as on a phone
/// (it honors the page's `<meta name="viewport">`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emulation {
    pub width: u32,
    pub height: u32,
    pub mobile: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            scale: 1.0,
        }
    }
}

struct Running {
    /// Kept alive for the connection's lifetime; dropping it stops Chrome.
    _chrome: Chrome,
    /// Browser-level commands (targets).
    browser: Cdp,
    /// The page shown in the pane.
    cdp: Cdp,
    target_id: String,
    /// `http://127.0.0.1:<port>`, only when the user opened a port.
    devtools: Option<String>,
}

/// A JavaScript dialog waiting for an answer. Headless Chrome cannot show
/// it, so the pane does.
#[derive(Debug, Clone)]
pub struct Dialog {
    /// `alert`, `confirm`, `prompt` or `beforeunload`.
    pub kind: String,
    pub message: String,
}

type StartTask = Shared<Task<Result<Cdp, SharedString>>>;

pub struct Browser {
    project_id: i64,
    store: Store,
    status: BrowserStatus,
    running: Option<Running>,
    starting: Option<StartTask>,
    /// Counts stops, so a start that finishes after one is dropped.
    generation: u64,
    url: String,
    title: String,
    loading: bool,
    main_frame: Option<String>,
    pub logs: Logs,
    frame: Option<Frame>,
    /// CSS pixels per pixel of the latest frame, for mapping input.
    css_per_px: f32,
    viewport: Viewport,
    /// A fixed screen size that overrides the pane's, while set.
    emulation: Option<Emulation>,
    /// Whether the pane wants a live stream.
    screencast: bool,
    /// Navigations waited on, by the loader Chrome gave each.
    load_waiters: Vec<(String, smol::channel::Sender<()>)>,
    /// The latest loaders that finished, for a wait that starts late.
    loaded: std::collections::VecDeque<String>,
    dialog: Option<Dialog>,
    /// A new pane size waiting to settle.
    pending_viewport: Option<Viewport>,
    _resize: Option<Task<()>>,
    /// When the last screencast frame was shown.
    last_frame: Instant,
    /// Last time the pane was shown or anything used the browser.
    last_used: Instant,
    _events: Option<Task<()>>,
    _idle: Option<Task<()>>,
    /// Stops Chrome when the app quits, so no headless browser is left behind.
    _quit: Subscription,
}

impl EventEmitter<BrowserEvent> for Browser {}

impl Browser {
    pub fn new(project_id: i64, store: Store, cx: &mut Context<Self>) -> Self {
        let url = store.setting::<String>(&url_key(project_id));
        let quit = cx.on_app_quit(|this, _| {
            this.running = None;
            async {}
        });
        Self {
            project_id,
            store,
            status: BrowserStatus::Stopped,
            running: None,
            starting: None,
            generation: 0,
            url,
            title: String::new(),
            loading: false,
            main_frame: None,
            logs: Logs::default(),
            frame: None,
            css_per_px: 1.0,
            viewport: Viewport::default(),
            emulation: None,
            screencast: false,
            load_waiters: Vec::new(),
            loaded: std::collections::VecDeque::new(),
            dialog: None,
            pending_viewport: None,
            _resize: None,
            last_frame: Instant::now(),
            last_used: Instant::now(),
            _events: None,
            _idle: None,
            _quit: quit,
        }
    }

    pub fn status(&self) -> &BrowserStatus {
        &self.status
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    /// Where other DevTools clients (e.g. a browser MCP server) can attach,
    /// when the user opened a port for them.
    pub fn devtools_url(&self) -> Option<String> {
        self.running
            .as_ref()
            .and_then(|running| running.devtools.clone())
    }

    pub fn devtools_port_enabled(&self) -> bool {
        self.store.setting::<bool>(&port_key(self.project_id))
    }

    /// Opens or closes the DevTools port (restarting Chrome). Off by default:
    /// any local process could drive the browser through it.
    pub fn set_devtools_port(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let _ = self.store.set_setting(&port_key(self.project_id), &enabled);
        let was_running = self.running.is_some() || self.starting.is_some();
        self.stop(cx);
        if was_running {
            self.ensure_started(cx);
        }
    }

    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Answers the open dialog (OK or Cancel).
    pub fn answer_dialog(&mut self, accept: bool, cx: &mut Context<Self>) {
        if self.dialog.take().is_some()
            && let Some(cdp) = self.cdp()
        {
            cdp.fire("Page.handleJavaScriptDialog", json!({ "accept": accept }));
        }
        cx.notify();
    }

    /// The connection, starting Chrome first if needed. Callers that arrive
    /// while it starts share the same startup.
    pub fn connection(&mut self, cx: &mut Context<Self>) -> Task<Result<Cdp>> {
        self.last_used = Instant::now();
        if let Some(running) = &self.running {
            return Task::ready(Ok(running.cdp.clone()));
        }
        let start = match &self.starting {
            Some(start) => start.clone(),
            None => {
                let Some(binary) = chrome::find_chrome(self.chosen_browser().as_deref()) else {
                    // Nothing to start; the pane asks the user to choose one.
                    self.status = BrowserStatus::NoBrowser;
                    cx.notify();
                    cx.emit(BrowserEvent::Changed);
                    return Task::ready(Err(anyhow!(
                        "No browser was found. Install Chrome, Edge, Brave or Chromium, or choose one in the Browser pane."
                    )));
                };
                let start = self.start(binary, cx);
                self.starting = Some(start.clone());
                start
            }
        };
        cx.spawn(async move |_, _| start.await.map_err(|err| anyhow!("{err}")))
    }

    /// Starts Chrome (if needed) without waiting, e.g. when the pane opens.
    pub fn ensure_started(&mut self, cx: &mut Context<Self>) {
        if self.running.is_none() && self.starting.is_none() {
            self.connection(cx).detach();
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.running = None;
        self.starting = None;
        self._events = None;
        self._idle = None;
        self.dialog = None;
        self.status = BrowserStatus::Stopped;
        self.loading = false;
        cx.notify();
        cx.emit(BrowserEvent::Changed);
    }

    /// The browser the user chose in the pane, if any.
    fn chosen_browser(&self) -> Option<PathBuf> {
        let path: String = self.store.setting(chrome::CHOSEN_SETTING);
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    /// Uses `picked` (a browser program or macOS app) from now on, and starts
    /// it. Fails if it is not a program.
    pub fn choose_browser(
        &mut self,
        picked: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let program = chrome::resolve_choice(picked)
            .with_context(|| format!("{} is not a browser program", picked.display()))?;
        self.store
            .set_setting(chrome::CHOSEN_SETTING, &program.to_string_lossy())?;
        self.stop(cx);
        self.ensure_started(cx);
        Ok(())
    }

    fn start(&mut self, binary: PathBuf, cx: &mut Context<Self>) -> StartTask {
        self.status = BrowserStatus::Starting;
        cx.notify();
        let profile = chrome::profile_dir(self.project_id);
        let open_port = self.devtools_port_enabled();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let launched = cx
                .background_spawn(async move {
                    let mut chrome = Chrome::launch(&binary, &profile, open_port)?;
                    let (reader, writer) = (chrome.reader.take(), chrome.writer.take());
                    let (browser, events) = Cdp::connect(
                        reader.context("Chrome pipe")?,
                        writer.context("Chrome pipe")?,
                    )?;
                    anyhow::Ok((chrome, browser, events))
                })
                .await;
            let result = match launched {
                Ok((chrome, browser, events)) => match prepare(&browser).await {
                    Ok((cdp, target_id)) => this
                        .update(cx, |this, cx| {
                            // Stopped while starting: this Chrome (dropped
                            // here, which ends it) is not wanted any more.
                            if this.generation != generation {
                                return Err(anyhow!("the browser was stopped"));
                            }
                            let running = Running {
                                _chrome: chrome,
                                browser,
                                cdp: cdp.clone(),
                                target_id,
                                devtools: None,
                            };
                            this.started(running, events, cx);
                            Ok(cdp)
                        })
                        .map_err(|err| anyhow!("{err}"))
                        .and_then(|started| started),
                    Err(err) => Err(err),
                },
                Err(err) => Err(err),
            };
            result.map_err(|err| {
                let message: SharedString = format!("{err:#}").into();
                let _ = this.update(cx, |this, cx| {
                    // A later start is not this one's to clear.
                    if this.generation != generation {
                        return;
                    }
                    this.starting = None;
                    this.status = BrowserStatus::Failed(message.clone());
                    cx.notify();
                    cx.emit(BrowserEvent::Changed);
                });
                message
            })
        })
        .shared()
    }

    fn started(
        &mut self,
        mut running: Running,
        events: smol::channel::Receiver<CdpEvent>,
        cx: &mut Context<Self>,
    ) {
        running.devtools = running._chrome.devtools.clone();
        let cdp = running.cdp.clone();
        self.running = Some(running);
        self.starting = None;
        self.status = BrowserStatus::Running;
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.handle_event(event, cx))
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.running = None;
                this.status = BrowserStatus::Failed("The browser closed.".into());
                cx.notify();
                cx.emit(BrowserEvent::Changed);
            });
        }));
        self._idle = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                let stopped = this.update(cx, |this, cx| {
                    let idle = !this.screencast && this.last_used.elapsed() > IDLE_STOP;
                    if idle {
                        tracing::info!("stopping the idle browser of project {}", this.project_id);
                        this.stop(cx);
                    }
                    idle
                });
                if stopped.unwrap_or(true) {
                    return;
                }
            }
        }));
        self.apply_viewport();
        if self.screencast {
            self.start_screencast();
        }
        // Return to where this project's browser was last time.
        if !self.url.is_empty() && self.url != "about:blank" {
            let url = self.url.clone();
            cdp.fire("Page.navigate", json!({ "url": url }));
        }
        cx.notify();
        cx.emit(BrowserEvent::Changed);
    }

    fn cdp(&self) -> Option<&Cdp> {
        self.running.as_ref().map(|running| &running.cdp)
    }

    fn handle_event(&mut self, event: CdpEvent, cx: &mut Context<Self>) {
        let Some(running) = &self.running else { return };
        if event.session.is_none() {
            self.handle_browser_event(&event, cx);
            return;
        }
        // Events of other targets (closed strays) are ignored.
        if event.session.as_deref() != running.cdp.session() {
            return;
        }
        let params = &event.params;
        match event.method.as_str() {
            "Page.screencastFrame" => self.receive_frame(params, cx),
            "Page.frameNavigated" => {
                let frame = &params["frame"];
                if frame.get("parentId").is_none() {
                    self.main_frame = frame["id"].as_str().map(str::to_string);
                    let url = frame["url"].as_str().unwrap_or_default().to_string();
                    if url != self.url {
                        if url.starts_with("http") || url.starts_with("file:") {
                            let _ = self.store.set_setting(&url_key(self.project_id), &url);
                        }
                        self.url = url;
                        self.title.clear();
                    }
                }
            }
            "Page.frameStartedLoading" => {
                if params["frameId"].as_str() == self.main_frame.as_deref() {
                    self.loading = true;
                }
            }
            "Page.loadEventFired" | "Page.frameStoppedLoading" => {
                if event.method == "Page.loadEventFired"
                    || params["frameId"].as_str() == self.main_frame.as_deref()
                {
                    self.loading = false;
                    self.refresh_title(cx);
                }
            }
            // A navigation's own load: an earlier page finishing loading
            // must not end the wait for a later one.
            "Page.lifecycleEvent" => {
                if params["name"] != "load"
                    || params["frameId"].as_str() != self.main_frame.as_deref()
                {
                    return;
                }
                let loader = params["loaderId"].as_str().unwrap_or_default().to_string();
                self.load_waiters.retain(|(waiting, done)| {
                    let finished = *waiting == loader;
                    if finished {
                        let _ = done.try_send(());
                    }
                    !finished && !done.is_closed()
                });
                self.loaded.push_back(loader);
                if self.loaded.len() > 16 {
                    self.loaded.pop_front();
                }
                return;
            }
            "Page.javascriptDialogOpening" => self.open_dialog(params),
            "Page.javascriptDialogClosed" => self.dialog = None,
            method => {
                if !self.logs.record(method, params) {
                    return;
                }
            }
        }
        cx.notify();
        cx.emit(BrowserEvent::Changed);
    }

    /// Browser-level events: keep one page (close any other the site opens)
    /// and notice when ours goes away.
    fn handle_browser_event(&mut self, event: &CdpEvent, cx: &mut Context<Self>) {
        let Some(running) = &self.running else { return };
        let info = &event.params["targetInfo"];
        let target = info["targetId"]
            .as_str()
            .or_else(|| event.params["targetId"].as_str());
        match event.method.as_str() {
            "Target.targetCreated"
                if info["type"] == "page" && target != Some(running.target_id.as_str()) =>
            {
                running
                    .browser
                    .fire("Target.closeTarget", json!({ "targetId": target }));
            }
            "Target.targetCrashed" | "Target.targetDestroyed"
                if target == Some(running.target_id.as_str()) =>
            {
                self.stop(cx);
                self.status =
                    BrowserStatus::Failed("The page crashed. Try again to reopen it.".into());
                cx.notify();
            }
            _ => {}
        }
    }

    /// A page dialog. Alerts are acknowledged. Confirmations and prompts are
    /// asked in the pane while it is shown; with nobody watching they are
    /// cancelled, so an agent can never confirm "Delete…?" on its own.
    fn open_dialog(&mut self, params: &Value) {
        let kind = params["type"].as_str().unwrap_or("alert").to_string();
        let message = params["message"].as_str().unwrap_or_default().to_string();
        let ask =
            self.screencast && (kind == "confirm" || kind == "prompt" || kind == "beforeunload");
        let answer = if ask {
            "waiting for you in the pane"
        } else if kind == "confirm" || kind == "prompt" {
            "cancelled"
        } else {
            "accepted"
        };
        self.logs.record(
            "Log.entryAdded",
            &json!({ "entry": { "level": "info", "text": format!("{kind} dialog: {message} ({answer})") } }),
        );
        if ask {
            self.dialog = Some(Dialog { kind, message });
        } else if let Some(cdp) = self.cdp() {
            let accept = !(kind == "confirm" || kind == "prompt");
            cdp.fire("Page.handleJavaScriptDialog", json!({ "accept": accept }));
        }
    }

    /// Decodes a frame off the UI thread, then acks it so Chrome sends the
    /// next one. One frame in flight keeps the stream at the speed we draw.
    fn receive_frame(&mut self, params: &Value, cx: &mut Context<Self>) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        let session_id = params["sessionId"].clone();
        let data = params["data"].as_str().unwrap_or_default().to_string();
        let css_width = params["metadata"]["deviceWidth"].as_f64().unwrap_or(0.0) as f32;
        cx.spawn(async move |this, cx| {
            let frame = cx
                .background_spawn(async move {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
                    Frame::decode(&bytes)
                })
                .await;
            // At most ~30 frames a second: the ack is held back, and Chrome
            // sends nothing until it gets one, so animations cost half the
            // redraws without dropping the final picture.
            let shown = this.read_with(cx, |this, _| this.last_frame).ok();
            if let Some(wait) =
                shown.map(|shown| MIN_FRAME_INTERVAL.saturating_sub(shown.elapsed()))
                && !wait.is_zero()
            {
                cx.background_executor().timer(wait).await;
            }
            cdp.fire(
                "Page.screencastFrameAck",
                json!({ "sessionId": session_id }),
            );
            if let Ok(frame) = frame {
                let _ = this.update(cx, |this, cx| {
                    if css_width > 0.0 {
                        this.css_per_px = css_width / frame.width as f32;
                    }
                    this.last_frame = Instant::now();
                    this.frame = Some(frame.clone());
                    cx.emit(BrowserEvent::Frame(frame));
                });
            }
        })
        .detach();
    }

    fn refresh_title(&mut self, cx: &mut Context<Self>) {
        let Some(cdp) = self.cdp().cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Ok(title) = evaluate(&cdp, "document.title").await {
                let _ = this.update(cx, |this, cx| {
                    this.title = title.as_str().unwrap_or_default().to_string();
                    cx.notify();
                    cx.emit(BrowserEvent::Changed);
                });
            }
        })
        .detach();
    }

    // ---- The pane ----

    /// Streams frames while `on` (the pane is visible).
    pub fn set_screencast(&mut self, on: bool, cx: &mut Context<Self>) {
        self.last_used = Instant::now();
        if self.screencast == on {
            return;
        }
        self.screencast = on;
        if on {
            self.ensure_started(cx);
            self.start_screencast();
        } else if let Some(cdp) = self.cdp() {
            cdp.fire("Page.stopScreencast", json!({}));
        }
    }

    /// Resizes the page to the pane. Ignored when unchanged.
    /// Resizes the page to the pane. Ignored when unchanged; applied once
    /// the size has settled, so dragging a divider does not resize the page
    /// (and restart the stream) on every frame.
    pub fn set_viewport(&mut self, viewport: Viewport, cx: &mut Context<Self>) {
        let pending = self.pending_viewport.unwrap_or(self.viewport);
        if viewport == pending || viewport.width < 50 || viewport.height < 50 {
            return;
        }
        self.pending_viewport = Some(viewport);
        self._resize = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |this, _| {
                if let Some(viewport) = this.pending_viewport.take() {
                    this.viewport = viewport;
                    this.apply_viewport();
                    if this.screencast {
                        this.start_screencast();
                    }
                }
            });
        }));
    }

    /// Shows the page at `emulation` instead of the pane's size (`None`:
    /// follow the pane again).
    pub fn emulate(&mut self, emulation: Option<Emulation>, cx: &mut Context<Self>) {
        if self.emulation == emulation {
            return;
        }
        self.emulation = emulation;
        self.apply_viewport();
        if self.screencast {
            self.start_screencast();
        }
        cx.notify();
        cx.emit(BrowserEvent::Changed);
    }

    /// The page's size, scale and whether it is laid out as on a phone.
    fn screen(&self) -> (u32, u32, f32, bool) {
        match self.emulation {
            Some(Emulation {
                width,
                height,
                mobile,
            }) => (width, height, 1.0, mobile),
            None => (
                self.viewport.width,
                self.viewport.height,
                self.viewport.scale,
                false,
            ),
        }
    }

    fn apply_viewport(&self) {
        let Some(cdp) = self.cdp() else { return };
        let (width, height, scale, mobile) = self.screen();
        cdp.fire(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": width, "height": height, "deviceScaleFactor": scale, "mobile": mobile }),
        );
    }

    fn start_screencast(&self) {
        let Some(cdp) = self.cdp() else { return };
        let (width, height, scale, _) = self.screen();
        cdp.fire(
            "Page.startScreencast",
            json!({
                "format": "jpeg",
                "quality": 82,
                "maxWidth": (width as f32 * scale) as u32,
                "maxHeight": (height as f32 * scale) as u32,
                "everyNthFrame": 1,
            }),
        );
    }

    /// Opens `input` as typed in the address bar.
    pub fn open(&mut self, input: &str, cx: &mut Context<Self>) {
        let url = normalize_url(input);
        self.url = url.clone();
        self.loading = true;
        let connection = self.connection(cx);
        cx.spawn(async move |_, _| {
            if let Ok(cdp) = connection.await {
                cdp.fire("Page.navigate", json!({ "url": url }));
            }
        })
        .detach();
        cx.notify();
    }

    pub fn go_back(&self) {
        self.run_script("history.back()");
    }

    pub fn go_forward(&self) {
        self.run_script("history.forward()");
    }

    pub fn reload(&self) {
        if let Some(cdp) = self.cdp() {
            cdp.fire("Page.reload", json!({}));
        }
    }

    fn run_script(&self, expression: &str) {
        if let Some(cdp) = self.cdp() {
            cdp.fire("Runtime.evaluate", json!({ "expression": expression }));
        }
    }

    /// Forwards pane input (frame pixels) to the page.
    pub fn pointer(&self, pointer: SurfacePointer) {
        let Some(cdp) = self.cdp() else { return };
        let (x, y) = (pointer.x * self.css_per_px, pointer.y * self.css_per_px);
        let button = if pointer.right_button {
            "right"
        } else {
            "left"
        };
        let params = match pointer.kind {
            PointerKind::Down => {
                json!({ "type": "mousePressed", "x": x, "y": y, "button": button, "buttons": 1, "clickCount": pointer.click_count.max(1) })
            }
            PointerKind::Up => {
                json!({ "type": "mouseReleased", "x": x, "y": y, "button": button, "clickCount": pointer.click_count.max(1) })
            }
            PointerKind::Move => {
                json!({ "type": "mouseMoved", "x": x, "y": y, "button": "left", "buttons": 1 })
            }
            PointerKind::Scroll { dx, dy } => json!({
                "type": "mouseWheel", "x": x, "y": y,
                "deltaX": dx * self.css_per_px, "deltaY": dy * self.css_per_px,
            }),
        };
        cdp.fire("Input.dispatchMouseEvent", params);
    }

    pub fn key(&self, keystroke: &Keystroke) {
        let (Some(cdp), Some(events)) = (self.cdp(), input::key_events(keystroke)) else {
            return;
        };
        for event in events {
            cdp.fire("Input.dispatchKeyEvent", event);
        }
    }

    pub fn insert_text(&self, text: &str) {
        if let Some(cdp) = self.cdp() {
            cdp.fire("Input.insertText", json!({ "text": text }));
        }
    }

    /// The page's selected text, for copying to the clipboard.
    pub fn selected_text(&mut self, cx: &mut Context<Self>) -> Task<Result<String>> {
        let connection = self.connection(cx);
        cx.background_spawn(async move {
            let value = evaluate(&connection.await?, "window.getSelection().toString()").await?;
            Ok(value.as_str().unwrap_or_default().to_string())
        })
    }

    /// The element under a frame-pixel point, with its bounds in frame pixels.
    pub fn pick(&mut self, x: f32, y: f32, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let scale = self.css_per_px;
        let connection = self.connection(cx);
        cx.background_spawn(async move {
            let mut element =
                evaluate(&connection.await?, &scripts::pick(x * scale, y * scale)).await?;
            if element.is_null() {
                bail!("there is no element there");
            }
            let rect = element["rect"].clone();
            element["frame_rect"] = json!({
                "x": rect["x"].as_f64().unwrap_or(0.0) / scale as f64,
                "y": rect["y"].as_f64().unwrap_or(0.0) / scale as f64,
                "width": rect["width"].as_f64().unwrap_or(0.0) / scale as f64,
                "height": rect["height"].as_f64().unwrap_or(0.0) / scale as f64,
            });
            Ok(element)
        })
    }

    // ---- Agent actions (used by the control socket) ----

    /// Navigates and waits for that page's load event (or a timeout).
    pub fn navigate(&mut self, url: &str, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let url = normalize_url(url);
        let connection = self.connection(cx);
        cx.spawn(async move |this, cx| {
            let cdp = connection.await?;
            let result = cdp.call("Page.navigate", json!({ "url": url })).await?;
            if let Some(error) = result["errorText"].as_str() {
                bail!("could not open {url}: {error}");
            }
            // A move within the page (`#section`) has no loader and no load.
            if let Some(loader) = result["loaderId"].as_str() {
                let (done, loaded) = smol::channel::bounded(1);
                this.update(cx, |this, _| {
                    if this.loaded.iter().any(|known| known == loader) {
                        let _ = done.try_send(());
                    } else {
                        this.load_waiters.push((loader.to_string(), done));
                    }
                })?;
                wait_for_load(loaded, cx).await;
            }
            let title = evaluate(&cdp, "document.title").await.unwrap_or_default();
            let mut summary = page_summary(&this, cx)?;
            summary["title"] = title;
            Ok(summary)
        })
    }

    pub fn screenshot(&mut self, full_page: bool, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let connection = self.connection(cx);
        cx.spawn(async move |this, cx| {
            let cdp = connection.await?;
            let shot = cdp
                .call(
                    "Page.captureScreenshot",
                    json!({ "format": "png", "captureBeyondViewport": full_page }),
                )
                .await?;
            let mut summary = page_summary(&this, cx)?;
            summary["images"] = json!([{ "data": shot["data"], "mimeType": "image/png" }]);
            Ok(summary)
        })
    }

    /// Clicks an element (by selector) or a CSS-pixel point.
    pub fn click(&mut self, target: ClickTarget, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let connection = self.connection(cx);
        cx.spawn(async move |this, cx| {
            let cdp = connection.await?;
            let (x, y) = match target {
                ClickTarget::Point(x, y) => (x, y),
                ClickTarget::Selector(selector) => {
                    let point = evaluate(&cdp, &scripts::locate(&selector)).await?;
                    if point.is_null() {
                        bail!("no element matches `{selector}`");
                    }
                    (
                        point["x"].as_f64().unwrap_or(0.0) as f32,
                        point["y"].as_f64().unwrap_or(0.0) as f32,
                    )
                }
            };
            for kind in ["mousePressed", "mouseReleased"] {
                cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
                )
                .await?;
            }
            settle(cx).await;
            page_summary(&this, cx)
        })
    }

    /// Types text, optionally into `selector` first and pressing Enter after.
    pub fn type_text(
        &mut self,
        text: String,
        selector: Option<String>,
        submit: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let connection = self.connection(cx);
        cx.spawn(async move |this, cx| {
            let cdp = connection.await?;
            if let Some(selector) = selector {
                let found = evaluate(&cdp, &scripts::focus(&selector)).await?;
                if found != Value::Bool(true) {
                    bail!("no element matches `{selector}`");
                }
            }
            // Said in the result, so a recording never keeps a password.
            let secret = evaluate(&cdp, scripts::FOCUSED_IS_PASSWORD).await? == Value::Bool(true);
            cdp.call("Input.insertText", json!({ "text": text }))
                .await?;
            if submit {
                for event in input::named_key_events("Enter") {
                    cdp.call("Input.dispatchKeyEvent", event).await?;
                }
            }
            settle(cx).await;
            let mut summary = page_summary(&this, cx)?;
            if secret {
                summary["secret"] = json!(true);
            }
            Ok(summary)
        })
    }

    pub fn press(&mut self, key: String, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let connection = self.connection(cx);
        cx.spawn(async move |this, cx| {
            let cdp = connection.await?;
            let events = input::named_key_events(&key);
            if events.is_empty() {
                bail!("unknown key `{key}`");
            }
            for event in events {
                cdp.call("Input.dispatchKeyEvent", event).await?;
            }
            settle(cx).await;
            page_summary(&this, cx)
        })
    }

    pub fn scroll(&mut self, dy: f32, cx: &mut Context<Self>) -> Task<Result<Value>> {
        // The middle of the page as laid out: an emulated screen when set.
        let (width, height) = match &self.emulation {
            Some(emulation) => (emulation.width, emulation.height),
            None => (self.viewport.width, self.viewport.height),
        };
        let connection = self.connection(cx);
        cx.spawn(async move |this, cx| {
            let cdp = connection.await?;
            cdp.call(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseWheel", "x": width / 2, "y": height / 2, "deltaX": 0, "deltaY": dy }),
            )
            .await?;
            settle(cx).await;
            page_summary(&this, cx)
        })
    }

    pub fn evaluate(&mut self, expression: String, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let connection = self.connection(cx);
        cx.background_spawn(async move {
            let value = evaluate(&connection.await?, &expression).await?;
            Ok(json!({ "value": value }))
        })
    }

    /// Forgets cached pages and files (cookies and logins stay), so the
    /// next load fetches what the server has now. UI test runs start with it.
    pub fn clear_cache(&mut self, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let connection = self.connection(cx);
        cx.background_spawn(async move {
            connection
                .await?
                .call("Network.clearBrowserCache", json!({}))
                .await?;
            Ok(json!({}))
        })
    }

    pub fn snapshot(&mut self, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let connection = self.connection(cx);
        cx.background_spawn(async move { evaluate(&connection.await?, &scripts::snapshot()).await })
    }
}

pub enum ClickTarget {
    Selector(String),
    Point(f32, f32),
}

/// Finds (or opens) the page, attaches to it, enables the domains PilotHouse
/// listens to, and keeps links in one pane. Returns the page's connection
/// and target id.
async fn prepare(browser: &Cdp) -> Result<(Cdp, String)> {
    let targets = browser.call("Target.getTargets", json!({})).await?;
    let existing = targets["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| target["type"] == "page")
        .and_then(|target| target["targetId"].as_str())
        .map(str::to_string);
    let target_id = match existing {
        Some(id) => id,
        None => browser
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await?["targetId"]
            .as_str()
            .context("Chrome did not open a page")?
            .to_string(),
    };
    let attached = browser
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await?;
    let session = attached["sessionId"]
        .as_str()
        .context("could not attach to the page")?;
    let page = browser.for_session(session);
    // Tell us about new pages, so strays opened by sites can be closed.
    browser
        .call("Target.setDiscoverTargets", json!({ "discover": true }))
        .await?;
    for domain in [
        "Page.enable",
        "Runtime.enable",
        "Log.enable",
        "Network.enable",
    ] {
        page.call(domain, json!({})).await?;
    }
    // Headless Chrome names itself `HeadlessChrome` in its user agent and
    // client hints, which many sites answer with a CAPTCHA: present it as
    // the Chrome it is.
    if let Ok(version) = browser.call("Browser.getVersion", json!({})).await
        && let Some(agent) = version["userAgent"].as_str()
    {
        page.call("Network.setUserAgentOverride", user_agent_override(agent))
            .await
            .ok();
    }
    // `Page.lifecycleEvent`s name the loader, so a wait ends on its own load.
    page.call("Page.setLifecycleEventsEnabled", json!({ "enabled": true }))
        .await?;
    page.call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": scripts::SINGLE_PANE }),
    )
    .await?;
    page.fire(
        "Runtime.evaluate",
        json!({ "expression": scripts::SINGLE_PANE }),
    );
    Ok((page, target_id))
}

/// `Network.setUserAgentOverride` parameters that describe headless Chrome
/// like the desktop browser: no `Headless` in the user agent, and client
/// hints (`Sec-CH-UA`) with the browser's own brand.
fn user_agent_override(agent: &str) -> Value {
    let agent = agent.replace("HeadlessChrome", "Chrome");
    let full_version = agent
        .split_whitespace()
        .find_map(|part| part.strip_prefix("Chrome/"))
        .unwrap_or("0.0.0.0")
        .to_string();
    let major = full_version.split('.').next().unwrap_or("0").to_string();
    // Edge names its own version (`Edg/…`) beside Chromium's.
    let edge = agent
        .split_whitespace()
        .find_map(|part| part.strip_prefix("Edg/"))
        .map(str::to_string);
    let brand = if edge.is_some() {
        "Microsoft Edge"
    } else {
        "Google Chrome"
    };
    let brands = |version: &str, full: bool| {
        let own = match &edge {
            Some(edge) if full => edge.clone(),
            Some(edge) => edge.split('.').next().unwrap_or("0").to_string(),
            None => version.to_string(),
        };
        json!([
            { "brand": "Chromium", "version": version },
            { "brand": brand, "version": own },
            { "brand": "Not.A/Brand", "version": if full { "99.0.0.0" } else { "99" } },
        ])
    };
    let (platform, navigator_platform) = if cfg!(target_os = "macos") {
        ("macOS", "MacIntel")
    } else if cfg!(windows) {
        ("Windows", "Win32")
    } else if cfg!(target_arch = "aarch64") {
        ("Linux", "Linux aarch64")
    } else {
        ("Linux", "Linux x86_64")
    };
    let architecture = if cfg!(target_arch = "aarch64") {
        "arm"
    } else {
        "x86"
    };
    json!({
        "userAgent": agent,
        "platform": navigator_platform,
        "userAgentMetadata": {
            "brands": brands(&major, false),
            "fullVersionList": brands(&full_version, true),
            "fullVersion": full_version,
            "platform": platform,
            "platformVersion": "",
            "architecture": architecture,
            "model": "",
            "mobile": false,
            "bitness": "64",
        },
    })
}

/// Runs `expression` in the page and returns its JSON value.
async fn evaluate(cdp: &Cdp, expression: &str) -> Result<Value> {
    let result = cdp
        .call(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
    if let Some(details) = result.get("exceptionDetails") {
        let message = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("script error");
        bail!("{message}");
    }
    Ok(result["result"]
        .get("value")
        .cloned()
        .unwrap_or(Value::Null))
}

async fn wait_for_load(loaded: smol::channel::Receiver<()>, cx: &mut AsyncApp) {
    let timer = cx.background_executor().timer(LOAD_TIMEOUT);
    smol::future::or(
        async {
            loaded.recv().await.ok();
        },
        timer,
    )
    .await;
}

/// Gives the page a moment to react to input before reporting back.
async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(400))
        .await;
}

fn page_summary(this: &WeakEntity<Browser>, cx: &mut AsyncApp) -> Result<Value> {
    this.read_with(
        cx,
        |browser, _| json!({ "url": browser.url, "title": browser.title }),
    )
    .context("the browser was closed")
}

fn url_key(project_id: i64) -> String {
    format!("browser.{project_id}.url")
}

fn port_key(project_id: i64) -> String {
    format!("browser.{project_id}.devtools_port")
}

/// Address-bar input to a URL. Local addresses get `http`, the rest `https`.
/// There is no web search: PilotHouse sends nothing to a search engine.
pub fn normalize_url(input: &str) -> String {
    let input = input.trim();
    if input.contains("://") || input.starts_with("about:") || input.starts_with("data:") {
        return input.to_string();
    }
    let host = input.split(['/', ':']).next().unwrap_or_default();
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".test")
        || host.parse::<std::net::IpAddr>().is_ok();
    if local || input.starts_with(':') {
        let input = if input.starts_with(':') {
            format!("localhost{input}")
        } else {
            input.to_string()
        };
        format!("http://{input}")
    } else {
        format!("https://{input}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_bar_input_becomes_a_url() {
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url(":5173/app"), "http://localhost:5173/app");
        assert_eq!(normalize_url("127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("http://x.dev"), "http://x.dev");
    }

    #[test]
    fn headless_chrome_presents_as_chrome() {
        let value = user_agent_override(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/131.0.6778.86 Safari/537.36",
        );
        let agent = value["userAgent"].as_str().unwrap();
        assert!(!agent.contains("Headless") && agent.contains("Chrome/131.0.6778.86"));
        let metadata = &value["userAgentMetadata"];
        assert_eq!(metadata["brands"][1]["brand"], "Google Chrome");
        assert_eq!(metadata["brands"][1]["version"], "131");
        assert_eq!(metadata["fullVersion"], "131.0.6778.86");
    }
}
