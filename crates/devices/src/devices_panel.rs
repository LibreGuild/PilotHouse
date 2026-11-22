//! The Devices pane: pick a device or start an emulator, see and use its
//! screen, mark it up, and read its log, UI hierarchy and files in a drawer
//! under it.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, DragMoveEvent, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    px, uniform_list,
};
use ph_media::annotate::Shape;
use ph_media::{Frame, LiveTools, SurfaceEvent, SurfaceView, ToChat};
use ph_ui::MenuButton as _;
use ph_ui::{EmptyState, Icons};

use crate::adb::{Adb, RemoteFile};
use crate::manager::{DeviceManager, DevicesChanged};
use crate::screen::{DeviceScreen, ScreenEvent, StreamMode, StreamQuality, SystemKey};
use crate::ui_tree::{self, UiNode};

/// Rows shown in the UI and Files drawers.
const DRAWER_ROWS: usize = 500;
/// The drawer's height until the user drags it, and its limits.
const DRAWER_HEIGHT: f32 = 280.0;
const DRAWER_MIN: f32 = 120.0;
/// Room always left for the screen above the drawer.
const SCREEN_MIN: f32 = 160.0;
/// How long a mirrored screen keeps streaming after the pane is hidden:
/// coming back within it (from a look at a file, the terminal, the log)
/// finds the picture there, not a new connection being made.
const LINGER: std::time::Duration = std::time::Duration::from_secs(20);

/// What is dragged when the drawer is resized.
struct ResizeDrawer;

/// Nothing follows the pointer: the drawer edge itself moves.
struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drawer {
    Logcat,
    Ui,
    Files,
}

pub struct DevicesPanel {
    manager: Entity<DeviceManager>,
    screen: Option<Entity<DeviceScreen>>,
    /// How sharp a phone streams (Settings key `devices.stream_quality`).
    quality: StreamQuality,
    /// The device the pane works with while shown (its files, layout and
    /// apps), mirrored or not.
    session: Option<String>,
    /// Whether to mirror each device's screen, as the user chose (Settings
    /// key `devices.mirror`), by what tells the device apart (so a phone's
    /// choice holds on a cable and over Wi-Fi); a device not in it:
    /// emulators yes, phones no.
    mirror: std::collections::HashMap<String, bool>,
    surface: Entity<SurfaceView>,
    tools: Entity<LiveTools>,
    drawer: Option<Drawer>,
    drawer_height: Pixels,
    /// The workspace's Logcat, shown in the drawer under the screen (none
    /// in a build that leaves Logcat out).
    logcat: Option<Entity<crate::LogcatPanel>>,
    ui_nodes: Rc<[UiNode]>,
    /// Which of `ui_nodes` the Layout drawer lists (those with something
    /// to tell them by), sorted out once per layout: the pane draws again
    /// with every frame of the screen.
    ui_listed: Rc<[usize]>,
    /// The Layout row outlined on the screen (by its bounds): clicked
    /// again, its outline goes.
    outlined: Option<(i32, i32, i32, i32)>,
    files_dir: String,
    files: Rc<[RemoteFile]>,
    /// The folder being listed: the one asked for last replaces it, so an
    /// answer that comes late never shows in place of a newer one.
    _listing: Option<Task<()>>,
    packages: Rc<[String]>,
    /// A short message about the last action ("Installed", an error…).
    status: Option<SharedString>,
    screen_error: Option<SharedString>,
    active: bool,
    /// Hidden a moment ago, the screen still streaming (see [`LINGER`]).
    lingering: bool,
    _linger: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    _screen_subscription: Option<Subscription>,
}

impl EventEmitter<ToChat> for DevicesPanel {}

/// The user chose how sharp a phone streams; the app keeps the choice.
pub struct StreamQualityChanged(pub StreamQuality);

impl EventEmitter<StreamQualityChanged> for DevicesPanel {}

/// The user asked for the device's log with the pane to itself (Logcat is
/// a tool of its own too).
pub struct OpenLogcat;

impl EventEmitter<OpenLogcat> for DevicesPanel {}

/// The user chose to mirror a device's screen, or not; the app keeps it.
pub struct MirrorChanged {
    /// What tells the device apart ([`crate::adb::DeviceInfo::identity`]).
    pub serial: String,
    pub mirror: bool,
}

impl EventEmitter<MirrorChanged> for DevicesPanel {}

impl DevicesPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let manager = DeviceManager::global(cx);
        let surface = cx.new(|cx| SurfaceView::new("", cx));
        let tools = cx.new(|cx| LiveTools::new(surface.clone(), "device", true, cx));
        let subscriptions = vec![
            cx.subscribe(&manager, |this, _, _: &DevicesChanged, cx| {
                this.sync_session(cx)
            }),
            cx.subscribe(&surface, |this, _, event: &SurfaceEvent, cx| {
                this.surface_event(event, cx)
            }),
            cx.subscribe(&tools, |_, _, event: &ToChat, cx| cx.emit(event.clone())),
            cx.observe(&manager, |_, _, cx| cx.notify()),
            // The outline was taken off the screen (cleared, or the screen
            // pressed): the Layout row is no longer the one outlined.
            cx.observe(&surface, |this, surface, cx| {
                if this.outlined.is_some() && !surface.read(cx).has_highlight() {
                    this.outlined = None;
                    cx.notify();
                }
            }),
        ];
        Self {
            manager,
            screen: None,
            quality: StreamQuality::default(),
            session: None,
            mirror: std::collections::HashMap::new(),
            surface,
            tools,
            drawer: None,
            drawer_height: px(DRAWER_HEIGHT),
            logcat: None,
            ui_nodes: Rc::default(),
            ui_listed: Rc::default(),
            outlined: None,
            files_dir: "/sdcard".into(),
            files: Rc::default(),
            _listing: None,
            packages: Rc::default(),
            status: None,
            screen_error: None,
            active: false,
            lingering: false,
            _linger: None,
            _subscriptions: subscriptions,
            _screen_subscription: None,
        }
    }

    /// The workspace's Logcat, for the drawer under the screen.
    pub fn set_logcat(&mut self, logcat: Entity<crate::LogcatPanel>, cx: &mut Context<Self>) {
        self.logcat = Some(logcat);
        cx.notify();
    }

    /// Streams phones this sharp, the open one again at once.
    pub fn set_stream_quality(&mut self, quality: StreamQuality, cx: &mut Context<Self>) {
        if self.quality == quality {
            return;
        }
        self.quality = quality;
        if self.screen.take().is_some() {
            self.sync_session(cx);
        }
        cx.notify();
    }

    /// The user's mirroring choices, as saved.
    pub fn set_mirror_choices(
        &mut self,
        mirror: std::collections::HashMap<String, bool>,
        cx: &mut Context<Self>,
    ) {
        self.mirror = mirror;
        self.sync_session(cx);
    }

    /// Whether `device`'s screen is mirrored: as the user chose, else an
    /// emulator yes (that is what it was started for) and a phone no (one
    /// may be plugged in only for its log or its files).
    fn wants_mirror(&self, device: &crate::adb::DeviceInfo) -> bool {
        self.mirror
            .get(device.identity())
            // A choice kept before devices were told by their own serial.
            .or_else(|| self.mirror.get(&device.serial))
            .copied()
            .unwrap_or(device.emulator)
    }

    /// Mirrors the device's screen, or stops; remembered for the device.
    fn set_mirroring(&mut self, mirror: bool, cx: &mut Context<Self>) {
        let Some(serial) = self
            .manager
            .read(cx)
            .selected()
            .filter(|device| Some(&device.serial) == self.session.as_ref())
            .map(|device| device.identity().to_string())
        else {
            return;
        };
        self.mirror.insert(serial.clone(), mirror);
        cx.emit(MirrorChanged { serial, mirror });
        // The screen is made (or dropped) for the same device.
        self.session = None;
        self.sync_session(cx);
    }

    /// Shown or hidden: the screen streams while shown, and a little
    /// longer.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        self._linger = None;
        self.lingering = !active && self.screen.is_some();
        if active {
            self.manager
                .update(cx, |manager, cx| manager.ensure_tracking(cx));
        } else if self.lingering {
            self._linger = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(LINGER).await;
                let _ = this.update(cx, |this, cx| {
                    this.lingering = false;
                    this.sync_session(cx);
                });
            }));
            return;
        }
        self.sync_session(cx);
    }

    pub fn tools(&self) -> &Entity<LiveTools> {
        &self.tools
    }

    /// The device screen with the user's marks, for `@screenshot`: only
    /// while it is mirrored and streaming (the last picture of a screen
    /// that failed is not the device's screen now).
    pub fn composite(&self, cx: &App) -> Option<Frame> {
        if self.screen.is_none() || self.screen_error.is_some() {
            return None;
        }
        self.surface.read(cx).composite()
    }

    fn adb(&self, cx: &App) -> Option<Adb> {
        self.manager.read(cx).adb().cloned()
    }

    fn selected_serial(&self, cx: &App) -> Option<String> {
        let manager = self.manager.read(cx);
        manager
            .selected()
            .filter(|device| device.is_ready())
            .map(|device| device.serial.clone())
    }

    /// Works with the selected device while the pane is shown (its screen
    /// too, when it is mirrored); lets go of it otherwise or when the device
    /// changes.
    fn sync_session(&mut self, cx: &mut Context<Self>) {
        // A screen held while hidden is that of the device it was: another
        // one chosen meanwhile is not mirrored unseen.
        if self.lingering && self.selected_serial(cx) != self.session {
            self.lingering = false;
            self._linger = None;
        }
        let wanted = (self.active || self.lingering)
            .then(|| self.selected_serial(cx))
            .flatten();
        let mirror = self
            .manager
            .read(cx)
            .selected()
            .is_some_and(|device| self.wants_mirror(device));
        // Same device, mirrored as wanted and working: keep it. A failed
        // screen (the emulator restarted, the phone reconnected) is opened
        // again.
        if wanted == self.session && mirror == self.screen.is_some() && self.screen_error.is_none()
        {
            return;
        }
        let same_device = wanted == self.session;
        self.session = wanted.clone();
        self.screen = None;
        self._screen_subscription = None;
        self.screen_error = None;
        self.outlined = None;
        // The last picture and the outline on it were the old screen's:
        // kept, they would show (and take clicks) as the new one's until
        // its first frame.
        self.surface.update(cx, |surface, cx| {
            surface.clear_frame(cx);
            surface.set_highlight(None, cx);
        });
        if !same_device {
            self.set_ui_nodes(Vec::new());
            self.files = Rc::default();
            self.packages = Rc::default();
            // What was asked of the old device: its answers are dropped.
            self._listing = None;
            self.status = None;
        }
        let (Some(serial), Some(adb)) = (wanted, self.adb(cx)) else {
            cx.notify();
            return;
        };
        if !same_device {
            self.load_packages(adb.clone(), serial.clone(), cx);
            match self.drawer {
                Some(Drawer::Files) => self.open_dir(self.files_dir.clone(), cx),
                Some(Drawer::Ui) => self.refresh_ui(cx),
                // The log follows the device by itself.
                Some(Drawer::Logcat) | None => {}
            }
        }
        if !mirror {
            cx.notify();
            return;
        }
        let name = self
            .manager
            .read(cx)
            .selected()
            .map(|device| device.display_name())
            .unwrap_or_else(|| serial.clone());
        self.tools.update(cx, |tools, _| tools.set_source(name));
        let quality = self.quality;
        let screen = cx.new(|cx| DeviceScreen::new(serial.clone(), adb.clone(), quality, cx));
        self._screen_subscription = Some(cx.subscribe(
            &screen,
            |this, _, event: &ScreenEvent, cx| match event {
                ScreenEvent::Frame(frame) => {
                    this.screen_error = None;
                    this.surface
                        .update(cx, |surface, cx| surface.set_frame(frame.clone(), cx));
                    this.tools.update(cx, |tools, _| tools.push_frame(frame));
                }
                ScreenEvent::Failed(message) => {
                    this.screen_error = Some(message.clone().into());
                    cx.notify();
                }
                ScreenEvent::Clipboard(text) => {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone()));
                }
            },
        ));
        self.screen = Some(screen);
        cx.notify();
    }

    fn load_packages(&mut self, adb: Adb, serial: String, cx: &mut Context<Self>) {
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let packages = cx
                .background_spawn(async move { adb.packages(&serial) })
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                // Another device was chosen meanwhile: these are not its apps.
                if this.session != session {
                    return;
                }
                this.packages = packages.into();
                cx.notify();
            });
        })
        .detach();
    }

    fn surface_event(&mut self, event: &SurfaceEvent, cx: &mut Context<Self>) {
        let Some(screen) = self.screen.clone() else {
            return;
        };
        match event {
            SurfaceEvent::Pointer(pointer) => {
                screen.update(cx, |screen, _| screen.pointer(*pointer))
            }
            SurfaceEvent::Key(keystroke) if ph_media::editing_shortcut(keystroke, cx) => {
                match keystroke.key.as_str() {
                    "v" => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            screen.update(cx, |screen, _| screen.paste(&text));
                        }
                    }
                    // The phone's selection, to this computer's clipboard.
                    "c" => screen.update(cx, |screen, _| screen.copy(false)),
                    "x" => screen.update(cx, |screen, _| screen.copy(true)),
                    _ => {}
                }
            }
            SurfaceEvent::Key(keystroke) => screen.update(cx, |screen, _| screen.key(keystroke)),
            SurfaceEvent::Picked { x, y } => self.pick(*x, *y, cx),
        }
    }

    /// Frame pixels ↔ device pixels, from the latest frame and display size.
    fn frame_scale(&self, cx: &App) -> f32 {
        let frame_width = self
            .surface
            .read(cx)
            .frame()
            .map(|frame| frame.width)
            .unwrap_or(1);
        let device_width = self.ui_nodes.iter().map(|node| node.bounds.2).max();
        match device_width {
            Some(width) if width > 0 => frame_width as f32 / width as f32,
            _ => 1.0,
        }
    }

    /// Element picker: the UI node under the point, outlined and sent to chat.
    fn pick(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        let frame_width = self
            .surface
            .read(cx)
            .frame()
            .map(|frame| frame.width)
            .unwrap_or(1) as f32;
        let device = self
            .manager
            .read(cx)
            .selected()
            .map(|device| device.display_name())
            .unwrap_or_default();
        self.status = Some("Reading the screen…".into());
        cx.notify();
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let nodes = cx
                .background_spawn(
                    async move { adb.ui_dump(&serial).map(|xml| ui_tree::parse(&xml)) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                // Another device was chosen meanwhile: not its layout.
                if this.session != session {
                    return;
                }
                this.status = None;
                let nodes = match nodes {
                    Ok(nodes) => nodes,
                    Err(err) => {
                        this.status = Some(format!("{err:#}").into());
                        cx.notify();
                        return;
                    }
                };
                let device_width = nodes.iter().map(|node| node.bounds.2).max().unwrap_or(1) as f32;
                let scale = frame_width / device_width;
                let (dx, dy) = ((x / scale) as i32, (y / scale) as i32);
                if let Some(node) = ui_tree::node_at(&nodes, dx, dy).cloned() {
                    this.outline(node.bounds, scale, cx);
                    cx.emit(ToChat::Text {
                        label: format!(
                            "{} element",
                            node.class.rsplit('.').next().unwrap_or("Android")
                        ),
                        text: format!(
                            "Android element on {device} (device pixels):\n{}",
                            node.describe()
                        ),
                    });
                }
                this.set_ui_nodes(nodes);
                cx.notify();
            });
        })
        .detach();
    }

    /// Outlines the element with these `bounds` on the screen, in place of
    /// the one outlined before; the outlined one again takes its outline
    /// away. The outline is the surface's highlight, apart from the user's
    /// own marks: their Undo and Clear neither take it nor are taken for it.
    fn outline(&mut self, bounds: (i32, i32, i32, i32), scale: f32, cx: &mut Context<Self>) {
        let again = self.outlined == Some(bounds);
        self.outlined = (!again).then_some(bounds);
        let shape = (!again).then(|| {
            let (l, t, r, b) = bounds;
            Shape::Rect {
                from: (l as f32 * scale, t as f32 * scale),
                to: (r as f32 * scale, b as f32 * scale),
            }
        });
        self.surface
            .update(cx, |surface, cx| surface.set_highlight(shape, cx));
        cx.notify();
    }

    /// Takes the device's layout, and sorts out the elements the Layout
    /// drawer lists.
    fn set_ui_nodes(&mut self, nodes: Vec<UiNode>) {
        self.ui_listed = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                !node.text.is_empty()
                    || !node.resource_id.is_empty()
                    || !node.description.is_empty()
                    || node.clickable
            })
            .map(|(ix, _)| ix)
            .collect();
        self.ui_nodes = nodes.into();
    }

    fn refresh_ui(&mut self, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        self.run(
            cx,
            "Reading the screen…",
            move || adb.ui_dump(&serial).map(|xml| ui_tree::parse(&xml)),
            |this, nodes, _| {
                this.set_ui_nodes(nodes);
                None
            },
        );
    }

    fn open_dir(&mut self, dir: String, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        let list_dir = dir.clone();
        self.status = Some("Listing files…".into());
        cx.notify();
        self._listing = Some(cx.spawn(async move |this, cx| {
            let listed = cx
                .background_spawn(async move { adb.list_dir(&serial, &list_dir) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status = match listed {
                    Ok(files) => {
                        this.files_dir = dir;
                        this.files = files.into();
                        None
                    }
                    Err(err) => Some(format!("{err:#}").into()),
                };
                cx.notify();
            });
        }));
    }

    fn pull(&mut self, file: &RemoteFile, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        let remote = format!("{}/{}", self.files_dir.trim_end_matches('/'), file.name);
        let Some(downloads) =
            dirs::download_dir().or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        else {
            return;
        };
        let local = unused_path(&downloads, &file.name);
        let shown = local.display().to_string();
        self.run(
            cx,
            "Downloading…",
            move || adb.pull(&serial, &remote, &local),
            move |_, (), _| Some(format!("Saved to {shown}")),
        );
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        let dir = self.files_dir.clone();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Upload".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let reopen = dir.clone();
                this.run(
                    cx,
                    "Uploading…",
                    move || adb.push(&serial, &path, &dir),
                    move |this, (), cx| {
                        this.open_dir(reopen, cx);
                        Some("Uploaded".into())
                    },
                );
            });
        })
        .detach();
    }

    fn install_apk(&mut self, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Install".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(apk) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if !crate::sdk::is_apk(&apk) {
                    this.status = Some("Choose an .apk file.".into());
                    cx.notify();
                    return;
                }
                let reload_adb = adb.clone();
                let reload_serial = serial.clone();
                this.run(
                    cx,
                    "Installing…",
                    move || adb.install(&serial, &apk),
                    move |this, (), cx| {
                        this.load_packages(reload_adb, reload_serial, cx);
                        Some("Installed".into())
                    },
                );
            });
        })
        .detach();
    }

    fn launch(&mut self, package: String, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        self.run(
            cx,
            "Launching…",
            move || adb.launch(&serial, &package),
            |_, (), _| None,
        );
    }

    /// Runs a blocking adb job with a status line, then `done` on success.
    fn run<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        working: &'static str,
        job: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) -> Option<String> + 'static,
    ) {
        self.status = Some(working.into());
        cx.notify();
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job() }).await;
            let _ = this.update(cx, |this, cx| {
                // The device was changed (or the pane hidden) while the job
                // ran: its answer is not about the device shown now.
                if this.session != session {
                    return;
                }
                this.status = match result {
                    Ok(value) => done(this, value, cx).map(Into::into),
                    Err(err) => Some(format!("{err:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn set_drawer(&mut self, drawer: Drawer, cx: &mut Context<Self>) {
        self.drawer = if self.drawer == Some(drawer) {
            None
        } else {
            Some(drawer)
        };
        match self.drawer {
            Some(Drawer::Ui) if self.ui_nodes.is_empty() => self.refresh_ui(cx),
            Some(Drawer::Files) if self.files.is_empty() => {
                self.open_dir(self.files_dir.clone(), cx)
            }
            Some(Drawer::Logcat) => {
                if let Some(logcat) = &self.logcat {
                    logcat.update(cx, |logcat, cx| logcat.start(cx));
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn system_key(&self, key: SystemKey, cx: &mut Context<Self>) {
        if let Some(screen) = &self.screen {
            screen.update(cx, |screen, _| screen.system_key(key));
        }
    }

    // ---- Rendering ----

    fn render_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mirroring = self.screen.is_some();
        // The row in a narrow pane: the device's name gives way first (to
        // an ellipsis), then the live tools go into the ⋯ menu, leaving in
        // the row only what is going on. Nothing is cut off at the edge.
        // What the row has beside the tools: its padding and gaps, the
        // least of the device's name, Back, Home and Recents, the dividers,
        // ⋯ and the button that stops mirroring.
        const BESIDE_TOOLS: f32 = 16. + 104. + 3. * 28. + 18. + 2. * 28. + 12.;
        let (width, probe) = ph_ui::measure::width_of("device-toolbar", window, cx);
        let slow = self
            .screen
            .as_ref()
            .and_then(|screen| slow_stream_note(screen.read(cx).mode()));
        let needed = BESIDE_TOOLS
            + self.tools.read(cx).width(false, cx)
            + if slow.is_some() { 28. } else { 0. };
        let fold = mirroring
            && width.is_some_and(|width| ph_ui::measure::design_px(width, window) < needed);
        self.tools
            .update(cx, |tools, cx| tools.set_folded(fold, cx));
        // Folded and idle, the tools show nothing: no divider for them.
        let tools_width = self.tools.read(cx).width(fold, cx);
        let tools_shown = tools_width > 0.;
        let manager = self.manager.read(cx);
        let selected = manager.selected().cloned();
        let has_device = self.session.is_some();
        let name: SharedString = match &selected {
            Some(device) => device.display_name().into(),
            None if manager.devices().is_empty() => "No device".into(),
            None => "Choose a device".into(),
        };
        // The room the row has left for the name, after its padding and
        // gaps and everything else in it.
        let others = 16.
            + 40.
            + if mirroring { 3. * 24. + 9. } else { 0. }
            + if slow.is_some() { 24. } else { 0. }
            + if mirroring && tools_shown {
                tools_width + 9.
            } else {
                0.
            }
            + match (has_device, mirroring) {
                (false, _) => 0.,
                (true, true) => 2. * 24.,
                (true, false) => 24. + 84.,
            };
        let (label, cut) = match width {
            Some(width) => fit_name(&name, ph_ui::measure::design_px(width, window) - others),
            None => (name.clone(), false),
        };
        let icon_button = |id: &'static str, icon: Icon, label: &'static str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(ph_ui::tip(label))
                .accessibility_label(label)
        };
        let panel = cx.entity().downgrade();
        let manager_entity = self.manager.clone();
        let tools = self.tools.clone();
        h_flex()
            .relative()
            .flex_none()
            .h_11()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(probe)
            .child(
                Button::new("device-picker")
                    .ghost()
                    .small()
                    .when(cut, |this| this.tooltip(ph_ui::tip(name.clone())))
                    .icon(
                        selected
                            .as_ref()
                            .map_or(Icons::TabletSmartphone, |device| link_icon(device.link())),
                    )
                    .label(label)
                    .dropdown_caret(true)
                    .menu(gpui_kit::Anchor::TopLeft, move |menu, window, cx| {
                        device_menu(menu, &manager_entity, window, cx)
                    }),
            )
            .child(div().flex_1())
            .when(mirroring, |this| {
                this.child(
                    icon_button("device-back", Icon::new(IconName::ChevronLeft), "Back").on_click(
                        cx.listener(|this, _, _, cx| this.system_key(SystemKey::Back, cx)),
                    ),
                )
                .child(
                    icon_button("device-home", Icon::new(Icons::House), "Home").on_click(
                        cx.listener(|this, _, _, cx| this.system_key(SystemKey::Home, cx)),
                    ),
                )
                .child(
                    icon_button("device-recents", Icon::new(Icons::Square), "Recent apps")
                        .on_click(
                            cx.listener(|this, _, _, cx| this.system_key(SystemKey::Recents, cx)),
                        ),
                )
                .child(div().flex_none().w_px().h_4().mx_1().bg(cx.theme().border))
                // Said only when it is not video: why the picture is slow.
                .when_some(slow, |this, note| {
                    this.child(
                        Button::new("device-slow-stream")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::TriangleAlert).text_color(cx.theme().warning))
                            .tooltip(ph_ui::tip(note))
                            .accessibility_label(note),
                    )
                })
                // Pick, mark, capture, record: here, where they hide nothing
                // of the screen.
                .when(tools_shown, |this| {
                    this.child(self.tools.clone())
                        .child(div().flex_none().w_px().h_4().mx_1().bg(cx.theme().border))
                })
            })
            .when(has_device, |this| {
                // An emulator streams over its own channel at one size.
                let quality = (mirroring
                    && !self
                        .screen
                        .as_ref()
                        .is_some_and(|screen| screen.read(cx).is_emulator_stream()))
                .then_some(self.quality);
                let packages = self.packages.clone();
                this.child(
                    // What is done once in a while, in one menu.
                    icon_button("device-more", Icon::new(IconName::Ellipsis), "More").menu(
                        gpui_kit::Anchor::TopRight,
                        move |menu, window, cx| {
                            // The live tools, while the row has no room
                            // for them.
                            let menu = if fold {
                                LiveTools::menu(&tools, menu, window, cx)
                            } else {
                                menu
                            };
                            let install = panel.clone();
                            let menu = menu.item(
                                PopupMenuItem::new("Install an APK…")
                                    .icon(Icons::Package)
                                    .on_click(move |_, _, cx| {
                                        _ = install.update(cx, |this, cx| this.install_apk(cx));
                                    }),
                            );
                            let (launcher, apps) = (panel.clone(), packages.clone());
                            let menu = menu.submenu(
                                "Launch an app",
                                window,
                                cx,
                                move |menu, window, _| {
                                    if apps.is_empty() {
                                        return menu.label("No apps installed");
                                    }
                                    apps.iter().fold(
                                        menu.scrollable(true).max_h(ph_ui::scaled_px(360., window)),
                                        |menu, package| {
                                            let (panel, package) =
                                                (launcher.clone(), package.clone());
                                            menu.item(PopupMenuItem::new(package.clone()).on_click(
                                                move |_, _, cx| {
                                                    let package = package.clone();
                                                    _ = panel.update(cx, |this, cx| {
                                                        this.launch(package, cx)
                                                    });
                                                },
                                            ))
                                        },
                                    )
                                },
                            );
                            let Some(current) = quality else {
                                return menu;
                            };
                            let sharper = panel.clone();
                            menu.submenu("Stream quality", window, cx, move |menu, _, _| {
                                StreamQuality::ALL.into_iter().fold(
                                    menu.label("Lower is smoother on a slow phone"),
                                    |menu, quality| {
                                        let panel = sharper.clone();
                                        menu.item(
                                            PopupMenuItem::new(quality.label())
                                                .checked(quality == current)
                                                .on_click(move |_, _, cx| {
                                                    _ = panel.update(cx, |this, cx| {
                                                        this.set_stream_quality(quality, cx);
                                                        cx.emit(StreamQualityChanged(quality));
                                                    });
                                                }),
                                        )
                                    },
                                )
                            })
                        },
                    ),
                )
                .child(
                    // Named while it is the thing to do; a mirrored screen
                    // has the row for its own controls.
                    Button::new("device-mirror")
                        .ghost()
                        .small()
                        .icon(if mirroring {
                            Icons::EyeOff
                        } else {
                            Icons::Smartphone
                        })
                        .when(!mirroring, |this| this.label("Mirror"))
                        .tooltip(if mirroring {
                            "Stop mirroring: the device's screen is no longer shown here"
                        } else {
                            "Show the device's screen here, live"
                        })
                        .accessibility_label(if mirroring {
                            "Stop mirroring"
                        } else {
                            "Mirror"
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_mirroring(!mirroring, cx)),
                        ),
                )
            })
    }

    /// What covers the screen when there is nothing to show.
    fn render_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let manager = self.manager.read(cx);
        let content: AnyElement = if manager.looking_for_sdk() {
            loading("Looking for the Android SDK…".into(), cx)
        } else if manager.adb().is_none() {
            EmptyState::new("Android SDK not found")
                .icon(Icons::Smartphone)
                .description(crate::sdk::Sdk::missing_message())
                .into_any_element()
        } else if let Some(avd) = manager.booting().first() {
            loading(
                format!("Starting {avd}… the first boot can take a minute."),
                cx,
            )
        } else if let Some(device) = manager.selected().filter(|device| !device.is_ready()) {
            let hint = if device.state == "unauthorized" {
                "Unlock the phone and allow USB debugging for this computer."
            } else {
                "The device is not ready yet."
            };
            EmptyState::new(format!("{} is {}", device.display_name(), device.state))
                .icon(IconName::TriangleAlert)
                .description(hint)
                .into_any_element()
        } else if let Some(device) = manager
            .selected()
            .filter(|device| device.is_ready() && self.screen.is_none())
        {
            // Connected, not mirrored: what can be done without the screen.
            let kind = if device.emulator { "Emulator" } else { "Phone" };
            EmptyState::new(device.display_name())
                .icon(if device.emulator {
                    Icons::TabletSmartphone
                } else {
                    Icons::Smartphone
                })
                .description(format!(
                    "{kind} connected. Mirror its screen to see and use it here, or read its log and files without it. Agents can use it either way."
                ))
                .action(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .justify_center()
                        .child(
                            Button::new("card-mirror")
                                .small()
                                .primary()
                                .icon(Icons::Smartphone)
                                .label("Mirror screen")
                                .on_click(cx.listener(|this, _, _, cx| this.set_mirroring(true, cx))),
                        )
                        .when(self.logcat.is_some(), |this| {
                            this.child(
                                Button::new("card-logcat")
                                    .small()
                                    .outline()
                                    .icon(Icons::ScrollText)
                                    .label("Logcat")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.drawer != Some(Drawer::Logcat) {
                                            this.set_drawer(Drawer::Logcat, cx)
                                        }
                                    })),
                            )
                        })
                        .child(
                            Button::new("card-files")
                                .small()
                                .outline()
                                .icon(Icons::FolderOpen)
                                .label("Files")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.drawer != Some(Drawer::Files) {
                                        this.set_drawer(Drawer::Files, cx)
                                    }
                                })),
                        ),
                )
                .into_any_element()
        } else if self.screen.is_none() {
            let avds = manager.avds().to_vec();
            let manager_entity = self.manager.clone();
            let new_emulator = {
                let manager = self.manager.clone();
                Button::new("new-avd")
                    .small()
                    .when(avds.is_empty(), |this| this.primary())
                    .when(!avds.is_empty(), |this| this.ghost())
                    .icon(Icons::Plus)
                    .label("New emulator")
                    .on_click(move |_, window, cx| {
                        crate::avd_dialog::open_new(manager.clone(), window, cx)
                    })
            };
            EmptyState::new("No device connected")
                .icon(Icons::TabletSmartphone)
                .description(if avds.is_empty() {
                    "Make an emulator, or connect a phone with USB debugging on. Agents can use it too."
                } else {
                    "Start an emulator, or connect a phone with USB debugging on. Agents can use it too."
                })
                .action(h_flex().gap_2().flex_wrap().justify_center().children(avds.into_iter().take(3).enumerate().map(|(ix, avd)| {
                        let manager = manager_entity.clone();
                        Button::new(("start-avd", ix))
                            .small()
                            .when(ix == 0, |this| this.primary())
                            .when(ix > 0, |this| this.outline())
                            .icon(Icons::Play)
                            .label(avd.replace('_', " "))
                            .on_click(move |_, _, cx| {
                                manager.update(cx, |manager, cx| manager.start_emulator(&avd, cx)).detach();
                            })
                    })).child(new_emulator))
                .into_any_element()
        } else if let Some(error) = &self.screen_error {
            EmptyState::new("The screen is not available")
                .icon(IconName::TriangleAlert)
                .description(error.clone())
                .into_any_element()
        } else if self.surface.read(cx).frame().is_none() {
            loading("Connecting to the screen…".into(), cx)
        } else {
            return None;
        };
        Some(
            div()
                .id("devices-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .bg(cx.theme().background)
                .child(content)
                .into_any_element(),
        )
    }

    fn render_drawer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str,
                   label: &'static str,
                   icon: Icons,
                   drawer: Drawer,
                   this: &Self,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .selected(this.drawer == Some(drawer))
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_drawer(drawer, cx)))
        };
        let open = self.drawer.is_some();
        let header = h_flex()
            .relative()
            .flex_none()
            .h_9()
            .px_2()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary.opacity(0.5))
            .when(open, |this| {
                // The top edge drags the drawer taller or shorter.
                this.child(
                    div()
                        .id("drawer-resize")
                        .absolute()
                        .top(ph_ui::rems_from_px(-3.))
                        .left_0()
                        .right_0()
                        .h(ph_ui::rems_from_px(6.))
                        .cursor_row_resize()
                        .on_drag(ResizeDrawer, |_, _, _, cx| cx.new(|_| NoGhost)),
                )
            })
            .when(self.logcat.is_some(), |this| {
                this.child(tab(
                    "drawer-logcat",
                    "Logcat",
                    Icons::ScrollText,
                    Drawer::Logcat,
                    self,
                    cx,
                ))
            })
            .child(tab(
                "drawer-ui",
                "Layout",
                Icons::LayoutPanelTop,
                Drawer::Ui,
                self,
                cx,
            ))
            .child(tab(
                "drawer-files",
                "Files",
                Icons::FolderOpen,
                Drawer::Files,
                self,
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .px_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .children(self.status.clone()),
            )
            .when(self.drawer == Some(Drawer::Logcat), |this| {
                this.child(
                    Button::new("drawer-logcat-tool")
                        .ghost()
                        .xsmall()
                        .icon(Icons::Maximize2)
                        .tooltip("Give Logcat the whole pane")
                        .accessibility_label("Give Logcat the whole pane")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenLogcat))),
                )
            })
            .when(open, |this| {
                this.child(
                    Button::new("drawer-close")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronDown)
                        .tooltip("Close the drawer")
                        .accessibility_label("Close the drawer")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(drawer) = this.drawer {
                                this.set_drawer(drawer, cx);
                            }
                        })),
                )
            });
        let body = match self.drawer {
            None => None,
            Some(Drawer::Logcat) => self
                .logcat
                .as_ref()
                .map(|logcat| logcat.read(cx).view().clone().into_any_element()),
            Some(Drawer::Ui) => Some(self.render_ui_tree(cx)),
            Some(Drawer::Files) => Some(self.render_files(cx)),
        };
        v_flex()
            .flex_none()
            .child(header)
            .children(body.map(|body| {
                div()
                    .flex()
                    .flex_col()
                    .h(self.drawer_height)
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(body)
            }))
    }

    fn resize_drawer(&mut self, event: &DragMoveEvent<ResizeDrawer>, cx: &mut Context<Self>) {
        let area = event.bounds;
        // The drawer's header stays under the pointer.
        let height = area.bottom() - event.event.position.y - px(36.);
        let most = (area.size.height - px(SCREEN_MIN + 80.)).max(px(DRAWER_MIN));
        self.drawer_height = height.clamp(px(DRAWER_MIN), most);
        cx.notify();
    }

    fn render_ui_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let scale = self.frame_scale(cx);
        let (nodes, listed) = (self.ui_nodes.clone(), self.ui_listed.clone());
        let outlined = self.outlined;
        let panel = cx.entity().downgrade();
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} elements · click one to outline it, again to clear",
                                listed.len()
                            )),
                    )
                    .child(
                        Button::new("ui-refresh")
                            .ghost()
                            .xsmall()
                            .icon(Icons::RotateCw)
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_ui(cx))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(
                        // Only the rows in view are built: the pane draws
                        // again with every frame of the screen.
                        uniform_list(
                            "ui-rows",
                            listed.len().min(DRAWER_ROWS),
                            move |rows, _, cx| {
                                rows.filter_map(|ix| {
                                    let node = nodes.get(*listed.get(ix)?)?;
                                    let class = node
                                        .class
                                        .rsplit('.')
                                        .next()
                                        .unwrap_or(&node.class)
                                        .to_string();
                                    let label = if !node.text.is_empty() {
                                        format!("“{}”", node.text)
                                    } else if !node.description.is_empty() {
                                        node.description.clone()
                                    } else {
                                        String::new()
                                    };
                                    let id = node
                                        .resource_id
                                        .rsplit('/')
                                        .next()
                                        .unwrap_or_default()
                                        .to_string();
                                    let bounds = node.bounds;
                                    let picked = outlined == Some(bounds);
                                    let panel = panel.clone();
                                    Some(
                                        h_flex()
                                            .id(("ui-node", ix))
                                            .h_6()
                                            .px_2()
                                            .gap_2()
                                            .rounded(cx.theme().radius)
                                            .cursor_pointer()
                                            .when(picked, |this| this.bg(cx.theme().list_active))
                                            .hover(|style| style.bg(cx.theme().list_hover))
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .px_1p5()
                                                    .rounded(cx.theme().radius)
                                                    .bg(cx.theme().link.opacity(0.14))
                                                    .text_color(cx.theme().link)
                                                    .child(class),
                                            )
                                            .child(div().min_w_0().truncate().child(label))
                                            .when(!id.is_empty(), |this| {
                                                this.child(
                                                    div()
                                                        .flex_none()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(format!("#{id}")),
                                                )
                                            })
                                            .child(div().flex_1())
                                            .when(node.clickable, |this| {
                                                this.child(
                                                    Icon::new(Icons::Pointer)
                                                        .xsmall()
                                                        .text_color(cx.theme().muted_foreground),
                                                )
                                            })
                                            .tooltip({
                                                let line = ph_ui::tip(node.describe());
                                                move |window, cx| {
                                                    gpui_kit::component::tooltip::Tooltip::new(
                                                        line.clone(),
                                                    )
                                                    .build(window, cx)
                                                }
                                            })
                                            .on_click(move |_, _, cx| {
                                                _ = panel.update(cx, |this, cx| {
                                                    this.outline(bounds, scale, cx)
                                                });
                                            }),
                                    )
                                })
                                .collect()
                            },
                        )
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .into_any_element()
    }

    fn render_files(&self, cx: &mut Context<Self>) -> AnyElement {
        let parent = self.files_dir.rsplit_once('/').map(|(parent, _)| {
            if parent.is_empty() {
                "/".to_string()
            } else {
                parent.to_string()
            }
        });
        let (files, dir) = (self.files.clone(), self.files_dir.clone());
        let panel = cx.entity().downgrade();
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .when_some(parent.filter(|_| self.files_dir != "/"), |this, parent| {
                        this.child(
                            Button::new("files-up")
                                .ghost()
                                .xsmall()
                                .icon(IconName::ArrowUp)
                                .tooltip("Up")
                                .accessibility_label("Up")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_dir(parent.clone(), cx)
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(self.files_dir.clone()),
                    )
                    .child(
                        Button::new("files-upload")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .label("Upload")
                            .on_click(cx.listener(|this, _, _, cx| this.push(cx))),
                    ),
            )
            .child(
                v_flex().flex_1().min_h_0().px_2().text_sm().child(
                    // Only the rows in view are built, as in the Layout list.
                    uniform_list(
                        "file-rows",
                        files.len().min(DRAWER_ROWS),
                        move |rows, _, cx| {
                            rows.filter_map(|ix| {
                                let file = files.get(ix)?.clone();
                                let path = format!("{}/{}", dir.trim_end_matches('/'), file.name);
                                let panel = panel.clone();
                                Some(
                                    h_flex()
                                        .id(("remote-file", ix))
                                        .group("remote-file")
                                        .h_7()
                                        .gap_2()
                                        .px_2()
                                        .rounded(cx.theme().radius)
                                        .hover(|style| style.bg(cx.theme().list_hover))
                                        .child(if file.is_dir {
                                            ph_ui::folder_icon_element(
                                                &file.name,
                                                ph_ui::rems_from_px(16.),
                                            )
                                        } else {
                                            ph_ui::file_icon_element(
                                                &file.name,
                                                ph_ui::rems_from_px(16.),
                                            )
                                        })
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .child(file.name.clone()),
                                        )
                                        .map(|this| {
                                            if file.is_dir {
                                                this.cursor_pointer().on_click(move |_, _, cx| {
                                                    _ = panel.update(cx, |this, cx| {
                                                        this.open_dir(path.clone(), cx)
                                                    });
                                                })
                                            } else {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(human_size(file.size)),
                                                )
                                                .child(
                                                    div()
                                                        .invisible()
                                                        .group_hover("remote-file", |style| {
                                                            style.visible()
                                                        })
                                                        .child(
                                                            Button::new(("pull-file", ix))
                                                                .ghost()
                                                                .xsmall()
                                                                .icon(Icons::FolderDown)
                                                                .tooltip("Save to Downloads")
                                                                .accessibility_label(
                                                                    "Save to Downloads",
                                                                )
                                                                .on_click(move |_, _, cx| {
                                                                    _ = panel.update(
                                                                        cx,
                                                                        |this, cx| {
                                                                            this.pull(&file, cx)
                                                                        },
                                                                    );
                                                                }),
                                                        ),
                                                )
                                            }
                                        }),
                                )
                            })
                            .collect()
                        },
                    )
                    .flex_1()
                    .min_h_0(),
                ),
            )
            .into_any_element()
    }
}

/// A device as the menu names it: its model, its Android version, and for
/// one on the network where it is (two of a model, or one phone on a cable
/// and over Wi-Fi, are then told apart).
fn device_label(device: &crate::adb::DeviceInfo) -> String {
    let mut label = if device.is_ready() {
        device.display_name()
    } else {
        format!("{} ({})", device.display_name(), device.state)
    };
    if let Some(about) = device
        .about
        .as_ref()
        .filter(|about| !about.android.is_empty())
    {
        label.push_str(&format!(" · Android {}", about.android));
    }
    if device.link() == crate::adb::Link::Network {
        let host = device
            .serial
            .rsplit_once(':')
            .map_or("Wi-Fi", |(host, _)| host);
        // A phone adb found by itself has a long name, not an address.
        let host = if crate::adb::is_wireless(host) {
            "Wi-Fi"
        } else {
            host
        };
        label.push_str(&format!(" · {host}"));
    }
    label
}

fn link_icon(link: crate::adb::Link) -> Icons {
    match link {
        crate::adb::Link::Usb => Icons::Smartphone,
        crate::adb::Link::Network => Icons::Wifi,
        crate::adb::Link::Emulator => Icons::TabletSmartphone,
    }
}

/// The device menu: connected devices to choose from, saved ones to
/// connect again, emulators to start, connecting and disconnecting; what
/// is done to emulators once in a while (new, wipe, delete) is one step
/// further in.
pub fn device_menu(
    menu: PopupMenu,
    manager: &Entity<DeviceManager>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let state = manager.read(cx);
    let selected = state.selected().cloned();
    let devices = state.devices().to_vec();
    let saved: Vec<crate::SavedDevice> = state
        .saved()
        .iter()
        .filter(|saved| !state.is_connected(saved))
        .take(8)
        .cloned()
        .collect();
    let stopped: Vec<String> = state
        .avds()
        .iter()
        .filter(|avd| !state.is_running(avd))
        .cloned()
        .collect();

    let mut menu = menu.label("Connected");
    if devices.is_empty() {
        menu = menu.item(PopupMenuItem::new("None").disabled(true));
    }
    for device in devices {
        let serial = device.serial.clone();
        let picker = manager.clone();
        menu = menu.item(
            PopupMenuItem::new(device_label(&device))
                .icon(link_icon(device.link()))
                .checked(
                    selected
                        .as_ref()
                        .is_some_and(|chosen| chosen.serial == serial),
                )
                .on_click(move |_, _, cx| {
                    picker.update(cx, |manager, cx| manager.select(&serial, cx))
                }),
        );
    }
    if !saved.is_empty() {
        menu = menu.separator().label("Connect again");
        for device in saved {
            let connector = manager.clone();
            let address = device.address.clone();
            menu = menu.item(
                PopupMenuItem::new(format!("{} · {}", device.title(), device.address))
                    .icon(Icons::Wifi)
                    .on_click(move |_, window, cx| {
                        connect_saved(&connector, address.clone(), window, cx)
                    }),
            );
        }
    }
    if !stopped.is_empty() {
        menu = menu.separator().label("Start an emulator");
        for avd in &stopped {
            let (starter, avd) = (manager.clone(), avd.clone());
            menu = menu.item(
                PopupMenuItem::new(avd.replace('_', " "))
                    .icon(Icons::Play)
                    .on_click(move |_, _, cx| {
                        starter
                            .update(cx, |manager, cx| manager.start_emulator(&avd, cx))
                            .detach();
                    }),
            );
        }
    }
    let connector = manager.clone();
    menu = menu.separator().item(
        PopupMenuItem::new("Connect a device…")
            .icon(Icons::Plug)
            .on_click(move |_, window, cx| {
                crate::connect_dialog::open(connector.clone(), window, cx)
            }),
    );
    // What ends the chosen device's connection, or the device itself.
    match selected.as_ref().map(|device| (device.link(), device)) {
        Some((crate::adb::Link::Network, device)) => {
            let (disconnector, serial) = (manager.clone(), device.serial.clone());
            menu = menu.item(
                PopupMenuItem::new(format!("Disconnect {}", device.display_name()))
                    .icon(Icons::WifiOff)
                    .on_click(move |_, _, cx| {
                        disconnector.read(cx).disconnect(&serial, cx);
                    }),
            );
        }
        Some((crate::adb::Link::Emulator, device)) => {
            let (stopper, serial) = (manager.clone(), device.serial.clone());
            menu = menu.item(
                PopupMenuItem::new(format!("Stop {}", device.display_name()))
                    .icon(Icons::Power)
                    .on_click(move |_, _, cx| {
                        stopper.read(cx).stop_emulator(&serial, cx);
                    }),
            );
        }
        _ => {}
    }
    let manager = manager.clone();
    menu.separator()
        .submenu("Emulators", window, cx, move |menu, window, cx| {
            let maker = manager.clone();
            let menu = menu.item(
                PopupMenuItem::new("New emulator…")
                    .icon(Icons::Plus)
                    .on_click(move |_, window, cx| {
                        crate::avd_dialog::open_new(maker.clone(), window, cx)
                    }),
            );
            // Wiping and deleting need the emulator stopped, so only those
            // are listed.
            if stopped.is_empty() {
                return menu;
            }
            let (wipe_list, wiper) = (stopped.clone(), manager.clone());
            let (delete_list, deleter) = (stopped.clone(), manager.clone());
            menu.separator()
                .submenu("Wipe data", window, cx, move |menu, _, _| {
                    wipe_list.iter().fold(menu, |menu, avd| {
                        let (manager, avd) = (wiper.clone(), avd.clone());
                        menu.item(PopupMenuItem::new(avd.replace('_', " ")).on_click(
                            move |_, window, cx| {
                                crate::avd_dialog::confirm_wipe(
                                    manager.clone(),
                                    avd.clone(),
                                    window,
                                    cx,
                                )
                            },
                        ))
                    })
                })
                .submenu("Delete", window, cx, move |menu, _, _| {
                    delete_list.iter().fold(menu, |menu, avd| {
                        let (manager, avd) = (deleter.clone(), avd.clone());
                        menu.item(PopupMenuItem::new(avd.replace('_', " ")).on_click(
                            move |_, window, cx| {
                                crate::avd_dialog::confirm_delete(
                                    manager.clone(),
                                    avd.clone(),
                                    window,
                                    cx,
                                )
                            },
                        ))
                    })
                })
        })
}

/// Connects to a saved device from the menu: there is no dialog to say how
/// it went, so a notice does.
fn connect_saved(
    manager: &Entity<DeviceManager>,
    address: String,
    window: &mut Window,
    cx: &mut App,
) {
    window.push_notification(Notification::info(format!("Connecting to {address}…")), cx);
    let connecting = manager.update(cx, |manager, cx| manager.connect(&address, cx));
    window
        .spawn(cx, async move |cx| {
            let connected = connecting.await;
            let _ = cx.update(|window, cx| {
                let notice = match connected {
                    Ok(serial) => Notification::success(format!("Connected to {serial}.")),
                    Err(err) => Notification::error(crate::connect_dialog::explain(
                        &address,
                        &format!("{err:#}"),
                    )),
                };
                window.push_notification(notice, cx);
            });
        })
        .detach();
}

/// What to say of a screen that does not come as video.
/// `name` as the toolbar's device picker has room to say it in `room`
/// pixels (at the default interface size), and whether it was cut: a long
/// name ends in an ellipsis before anything in the row is pushed out of it.
fn fit_name(name: &SharedString, room: f32) -> (SharedString, bool) {
    // The picker's icon, caret and padding; then about this much a letter.
    const AROUND: f32 = 48.;
    const LETTER: f32 = 7.6;
    let letters = (((room - AROUND) / LETTER).floor().max(4.)) as usize;
    if name.chars().count() <= letters {
        return (name.clone(), false);
    }
    let kept: String = name.chars().take(letters - 1).collect();
    (format!("{}…", kept.trim_end()).into(), true)
}

fn slow_stream_note(mode: StreamMode) -> Option<&'static str> {
    match mode {
        StreamMode::Connecting | StreamMode::Video => None,
        StreamMode::RawFrames => Some(
            "This computer has no H.264 decoder, so the phone sends whole frames: it is slower to use while mirrored.",
        ),
        StreamMode::Snapshots => {
            Some("This device streams no video: its screen is shown in snapshots, a few a second.")
        }
    }
}

fn loading(text: String, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(ph_ui::Loader::new().small())
        .child(text)
        .into_any_element()
}

/// `dir/name`, or `dir/name (2)`… so a download never overwrites a file.
fn unused_path(dir: &std::path::Path, name: &str) -> PathBuf {
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
        .unwrap_or(candidate)
}

pub(crate) fn human_size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.0} KB", bytes as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

impl Render for DevicesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self.render_overlay(cx);
        v_flex()
            .size_full()
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ResizeDrawer>, _, cx| {
                    this.resize_drawer(event, cx)
                }),
            )
            .child(self.render_toolbar(window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(div().absolute().inset_0().child(self.surface.clone()))
                    .children(overlay),
            )
            .child(self.render_drawer(cx))
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn a_long_device_name_is_cut_to_the_room_there_is() {
        let name: SharedString = "Galaxy S20 FE 5G".into();
        assert_eq!(fit_name(&name, 400.), (name.clone(), false));
        assert_eq!(fit_name(&name, 110.), ("Galaxy…".into(), true));
        // Never to nothing: a few letters tell the devices apart.
        assert_eq!(fit_name(&name, 0.), ("Gal…".into(), true));
    }
}
