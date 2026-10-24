//! The Devices pane: pick a device or start an emulator, see and use its
//! screen, mark it up, and read logcat, the UI hierarchy and files.

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ph_media::annotate::Shape;
use ph_media::{Frame, LiveTools, SurfaceEvent, SurfaceView, ToChat};
use ph_ui::MenuButton as _;
use ph_ui::{EmptyState, Icons};

use crate::adb::{Adb, RemoteFile};
use crate::logcat::{Logcat, Priority};
use crate::manager::{DeviceManager, DevicesChanged};
use crate::screen::{DeviceScreen, ScreenEvent, SystemKey};
use crate::ui_tree::{self, UiNode};

/// Rows shown in the drawer.
const DRAWER_ROWS: usize = 300;
/// Logcat lines sent to chat.
const SEND_LINES: usize = 80;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drawer {
    Logcat,
    Ui,
    Files,
}

pub struct DevicesPanel {
    manager: Entity<DeviceManager>,
    screen: Option<Entity<DeviceScreen>>,
    logcat: Option<Entity<Logcat>>,
    surface: Entity<SurfaceView>,
    tools: Entity<LiveTools>,
    drawer: Option<Drawer>,
    log_filter: Entity<InputState>,
    errors_only: bool,
    ui_nodes: Vec<UiNode>,
    files_dir: String,
    files: Vec<RemoteFile>,
    packages: Vec<String>,
    /// A short message about the last action ("Installed", an error…).
    status: Option<SharedString>,
    screen_error: Option<SharedString>,
    active: bool,
    _subscriptions: Vec<Subscription>,
    _screen_subscription: Option<Subscription>,
}

impl EventEmitter<ToChat> for DevicesPanel {}

impl DevicesPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let manager = DeviceManager::global(cx);
        let surface = cx.new(|cx| SurfaceView::new("", cx));
        let tools = cx.new(|cx| LiveTools::new(surface.clone(), "device", true, cx));
        let log_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter: tag, package or text"));
        let subscriptions = vec![
            cx.subscribe(&manager, |this, _, _: &DevicesChanged, cx| {
                this.sync_session(cx)
            }),
            cx.subscribe(&surface, |this, _, event: &SurfaceEvent, cx| {
                this.surface_event(event, cx)
            }),
            cx.subscribe(&tools, |_, _, event: &ToChat, cx| cx.emit(event.clone())),
            cx.observe(&manager, |_, _, cx| cx.notify()),
            cx.observe(&log_filter, |_, _, cx| cx.notify()),
        ];
        Self {
            manager,
            screen: None,
            logcat: None,
            surface,
            tools,
            drawer: None,
            log_filter,
            errors_only: false,
            ui_nodes: Vec::new(),
            files_dir: "/sdcard".into(),
            files: Vec::new(),
            packages: Vec::new(),
            status: None,
            screen_error: None,
            active: false,
            _subscriptions: subscriptions,
            _screen_subscription: None,
        }
    }

    /// Shown or hidden: the screen streams (and logcat runs) only while shown.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.manager
                .update(cx, |manager, cx| manager.ensure_tracking(cx));
        }
        self.sync_session(cx);
    }

    pub fn tools(&self) -> &Entity<LiveTools> {
        &self.tools
    }

    /// The device screen with the user's marks, for `@screenshot`.
    pub fn composite(&self, cx: &App) -> Option<Frame> {
        self.surface.read(cx).composite()
    }

    /// Recent logcat errors, for `@log`.
    pub fn recent_logs(&self, cx: &App) -> Option<String> {
        let logcat = self.logcat.as_ref()?.read(cx);
        let lines = logcat.matching("", true, 40);
        if lines.is_empty() {
            return None;
        }
        let text: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        Some(format!("Logcat errors:\n```\n{}\n```", text.join("\n")))
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

    /// Opens the selected device's screen (and logcat) while the pane is
    /// shown; closes them otherwise or when the device changes.
    fn sync_session(&mut self, cx: &mut Context<Self>) {
        let wanted = self.active.then(|| self.selected_serial(cx)).flatten();
        let current = self
            .screen
            .as_ref()
            .map(|screen| screen.read(cx).serial().to_string());
        // Same device and still working: keep it. A failed screen (the
        // emulator restarted, the phone reconnected) is opened again.
        if wanted == current && self.screen_error.is_none() {
            self.sync_logcat(cx);
            return;
        }
        self.screen = None;
        self.logcat = None;
        self._screen_subscription = None;
        self.screen_error = None;
        self.ui_nodes.clear();
        self.files.clear();
        let (Some(serial), Some(adb)) = (wanted, self.adb(cx)) else {
            cx.notify();
            return;
        };
        let name = self
            .manager
            .read(cx)
            .selected()
            .map(|device| device.display_name())
            .unwrap_or_else(|| serial.clone());
        self.tools.update(cx, |tools, _| tools.set_source(name));
        let screen = cx.new(|cx| DeviceScreen::new(serial.clone(), adb.clone(), cx));
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
            },
        ));
        self.screen = Some(screen);
        self.sync_logcat(cx);
        self.load_packages(adb, serial, cx);
        cx.notify();
    }

    /// Logcat runs only while its drawer is open.
    fn sync_logcat(&mut self, cx: &mut Context<Self>) {
        let wanted = self.active && self.drawer == Some(Drawer::Logcat) && self.screen.is_some();
        if wanted && self.logcat.is_none() {
            let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
                return;
            };
            let logcat = cx.new(|cx| Logcat::new(&adb, &serial, cx));
            self._subscriptions
                .push(cx.observe(&logcat, |_, _, cx| cx.notify()));
            self.logcat = Some(logcat);
        } else if !wanted {
            self.logcat = None;
        }
    }

    fn load_packages(&mut self, adb: Adb, serial: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let packages = cx
                .background_spawn(async move { adb.packages(&serial) })
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.packages = packages;
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
            SurfaceEvent::Key(keystroke) if keystroke.modifiers.platform => {
                if keystroke.key == "v"
                    && let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                {
                    screen.update(cx, |screen, _| screen.type_text(&text));
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
        cx.spawn(async move |this, cx| {
            let nodes = cx
                .background_spawn(
                    async move { adb.ui_dump(&serial).map(|xml| ui_tree::parse(&xml)) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
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
                    this.outline(&node, scale, cx);
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
                this.ui_nodes = nodes;
                cx.notify();
            });
        })
        .detach();
    }

    fn outline(&self, node: &UiNode, scale: f32, cx: &mut Context<Self>) {
        let (l, t, r, b) = node.bounds;
        let shape = Shape::Rect {
            from: (l as f32 * scale, t as f32 * scale),
            to: (r as f32 * scale, b as f32 * scale),
        };
        self.surface
            .update(cx, |surface, cx| surface.add_annotation(shape, cx));
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
                this.ui_nodes = nodes;
                None
            },
        );
    }

    fn open_dir(&mut self, dir: String, cx: &mut Context<Self>) {
        let (Some(adb), Some(serial)) = (self.adb(cx), self.selected_serial(cx)) else {
            return;
        };
        let list_dir = dir.clone();
        self.run(
            cx,
            "Listing files…",
            move || adb.list_dir(&serial, &list_dir),
            move |this, files, _| {
                this.files_dir = dir;
                this.files = files;
                None
            },
        );
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
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job() }).await;
            let _ = this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(value) => done(this, value, cx).map(Into::into),
                    Err(err) => Some(format!("{err:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn send_logcat(&mut self, cx: &mut Context<Self>) {
        let Some(logcat) = &self.logcat else { return };
        let filter = self.log_filter.read(cx).value().to_string();
        let lines: Vec<String> = logcat
            .read(cx)
            .matching(&filter, self.errors_only, SEND_LINES)
            .into_iter()
            .map(|line| line.text.clone())
            .collect();
        if lines.is_empty() {
            return;
        }
        let device = self
            .manager
            .read(cx)
            .selected()
            .map(|device| device.display_name())
            .unwrap_or_default();
        cx.emit(ToChat::Text {
            label: "logcat".into(),
            text: format!("Logcat from {device}:\n```\n{}\n```", lines.join("\n")),
        });
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
            _ => {}
        }
        self.sync_logcat(cx);
        cx.notify();
    }

    fn system_key(&self, key: SystemKey, cx: &mut Context<Self>) {
        if let Some(screen) = &self.screen {
            screen.update(cx, |screen, _| screen.system_key(key));
        }
    }

    // ---- Rendering ----

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let manager = self.manager.read(cx);
        let selected = manager.selected().cloned();
        let has_device = self.screen.is_some();
        let label: SharedString = match &selected {
            Some(device) => device.display_name().into(),
            None if manager.devices().is_empty() => "No device".into(),
            None => "Choose a device".into(),
        };
        let icon_button = |id: &'static str, icon: Icon, label: &'static str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(label)
                .accessibility_label(label)
        };
        let panel = cx.entity().downgrade();
        let manager_entity = self.manager.clone();
        h_flex()
            .flex_none()
            .h_11()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("device-picker")
                    .ghost()
                    .small()
                    .icon(
                        if selected.as_ref().is_some_and(|device| !device.emulator) {
                            Icons::Smartphone
                        } else {
                            Icons::TabletSmartphone
                        },
                    )
                    .label(label)
                    .dropdown_caret(true)
                    .menu(gpui_kit::Anchor::TopLeft, move |menu, window, cx| {
                        device_menu(menu, &manager_entity, window, cx)
                    }),
            )
            .child(div().flex_1())
            .when(has_device, |this| {
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
                .child(div().w_px().h_4().mx_1().bg(cx.theme().border))
                .child(
                    icon_button(
                        "device-install",
                        Icon::new(Icons::Package),
                        "Install an APK",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.install_apk(cx))),
                )
                .child(
                    icon_button("device-launch", Icon::new(Icons::Play), "Launch an app").menu(
                        gpui_kit::Anchor::TopLeft,
                        {
                            let packages = self.packages.clone();
                            move |menu, _, _| {
                                if packages.is_empty() {
                                    return menu.label("No apps installed");
                                }
                                packages.iter().fold(
                                    menu.scrollable(true).max_h(gpui_kit::px(360.)),
                                    |menu, package| {
                                        let panel = panel.clone();
                                        let package = package.clone();
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
                            }
                        },
                    ),
                )
            })
    }

    /// What covers the screen when there is nothing to show.
    fn render_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let manager = self.manager.read(cx);
        let content: AnyElement = if manager.adb().is_none() {
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
                    .label("New emulator…")
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
                   drawer: Drawer,
                   this: &Self,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .xsmall()
                .selected(this.drawer == Some(drawer))
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_drawer(drawer, cx)))
        };
        let header = h_flex()
            .flex_none()
            .h_8()
            .px_2()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(tab("drawer-logcat", "Logcat", Drawer::Logcat, self, cx))
            .child(tab("drawer-ui", "UI", Drawer::Ui, self, cx))
            .child(tab("drawer-files", "Files", Drawer::Files, self, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .px_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .children(self.status.clone()),
            );
        let body = match self.drawer {
            None => None,
            Some(Drawer::Logcat) => Some(self.render_logcat(cx)),
            Some(Drawer::Ui) => Some(self.render_ui_tree(cx)),
            Some(Drawer::Files) => Some(self.render_files(cx)),
        };
        v_flex().flex_none().child(header).children(body)
    }

    fn render_logcat(&self, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.log_filter.read(cx).value().to_string();
        let rows: Vec<(SharedString, Option<gpui_kit::Hsla>)> = self
            .logcat
            .as_ref()
            .map(|logcat| {
                logcat
                    .read(cx)
                    .matching(&filter, self.errors_only, DRAWER_ROWS)
                    .into_iter()
                    // Newest first, like the browser's console.
                    .rev()
                    .map(|line| {
                        let color = match line.priority {
                            Priority::Error => Some(cx.theme().danger),
                            Priority::Warn => Some(cx.theme().warning),
                            Priority::Verbose | Priority::Debug => {
                                Some(cx.theme().muted_foreground)
                            }
                            Priority::Info => None,
                        };
                        (line.text.clone().into(), color)
                    })
                    .collect()
            })
            .unwrap_or_default();
        v_flex()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .child(div().flex_1().child(Input::new(&self.log_filter).xsmall()))
                    .child(
                        Button::new("logcat-errors")
                            .ghost()
                            .xsmall()
                            .label("Errors only")
                            .selected(self.errors_only)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.errors_only = !this.errors_only;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("logcat-send")
                            .ghost()
                            .xsmall()
                            .icon(Icons::SendHorizontal)
                            .label("Send to chat")
                            .on_click(cx.listener(|this, _, _, cx| this.send_logcat(cx))),
                    )
                    .child(
                        Button::new("logcat-clear")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("Clear")
                            .accessibility_label("Clear logcat")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(logcat) = &this.logcat {
                                    logcat.update(cx, |logcat, cx| logcat.clear(cx));
                                }
                            })),
                    ),
            )
            .child(log_rows("logcat-rows", rows, cx))
            .into_any_element()
    }

    fn render_ui_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let scale = self.frame_scale(cx);
        let nodes: Vec<UiNode> = self
            .ui_nodes
            .iter()
            .filter(|node| {
                !node.text.is_empty()
                    || !node.resource_id.is_empty()
                    || !node.description.is_empty()
                    || node.clickable
            })
            .cloned()
            .collect();
        v_flex()
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
                                "{} elements · click one to outline it",
                                nodes.len()
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
                    .id("ui-rows")
                    .h_48()
                    .overflow_y_scroll()
                    .px_3()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .children(
                        nodes
                            .into_iter()
                            .take(DRAWER_ROWS)
                            .enumerate()
                            .map(|(ix, node)| {
                                let line = node.describe();
                                div()
                                    .id(("ui-node", ix))
                                    .py_0p5()
                                    .px_1()
                                    .rounded(cx.theme().radius)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(cx.theme().list_hover))
                                    .child(line)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.outline(&node, scale, cx)
                                    }))
                            }),
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
        v_flex()
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
                v_flex()
                    .id("file-rows")
                    .h_48()
                    .overflow_y_scroll()
                    .px_2()
                    .text_sm()
                    .children(
                        self.files
                            .iter()
                            .take(DRAWER_ROWS)
                            .enumerate()
                            .map(|(ix, file)| {
                                let file = file.clone();
                                let path = format!(
                                    "{}/{}",
                                    self.files_dir.trim_end_matches('/'),
                                    file.name
                                );
                                h_flex()
                                    .id(("remote-file", ix))
                                    .group("remote-file")
                                    .gap_2()
                                    .px_1()
                                    .py_0p5()
                                    .rounded(cx.theme().radius)
                                    .hover(|style| style.bg(cx.theme().list_hover))
                                    .child(
                                        Icon::new(if file.is_dir {
                                            IconName::Folder
                                        } else {
                                            IconName::File
                                        })
                                        .xsmall()
                                        .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(file.name.clone()),
                                    )
                                    .map(|this| {
                                        if file.is_dir {
                                            this.cursor_pointer().on_click(cx.listener(
                                                move |this, _, _, cx| {
                                                    this.open_dir(path.clone(), cx)
                                                },
                                            ))
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
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.pull(&file, cx)
                                                                },
                                                            )),
                                                    ),
                                            )
                                        }
                                    })
                            }),
                    ),
            )
            .into_any_element()
    }
}

/// The device picker: connected devices, emulators to start or stop, and
/// making, wiping and deleting emulators.
fn device_menu(
    menu: PopupMenu,
    manager: &Entity<DeviceManager>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let state = manager.read(cx);
    let selected = state.selected().map(|device| device.serial.clone());
    let devices = state.devices().to_vec();
    let stopped: Vec<String> = state
        .avds()
        .iter()
        .filter(|avd| !state.is_running(avd))
        .cloned()
        .collect();
    let running_emulator = state
        .selected()
        .filter(|device| device.emulator)
        .map(|device| (device.serial.clone(), device.name.clone()));

    let mut menu = menu.label("Connected");
    if devices.is_empty() {
        menu = menu.item(PopupMenuItem::new("None").disabled(true));
    }
    for device in devices {
        let serial = device.serial.clone();
        let picker = manager.clone();
        let label = if device.is_ready() {
            device.display_name()
        } else {
            format!("{} ({})", device.name, device.state)
        };
        menu = menu.item(
            PopupMenuItem::new(label)
                .icon(if device.emulator {
                    Icons::TabletSmartphone
                } else {
                    Icons::Smartphone
                })
                .checked(selected.as_deref() == Some(serial.as_str()))
                .on_click(move |_, _, cx| {
                    picker.update(cx, |manager, cx| manager.select(&serial, cx))
                }),
        );
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
    if let Some((serial, name)) = running_emulator {
        let stopper = manager.clone();
        menu = menu.separator().item(
            PopupMenuItem::new(format!("Stop {}", name.replace('_', " ")))
                .icon(Icons::Power)
                .on_click(move |_, _, cx| {
                    stopper.read(cx).stop_emulator(&serial, cx);
                }),
        );
    }
    let maker = manager.clone();
    menu = menu.separator().item(
        PopupMenuItem::new("New emulator…")
            .icon(Icons::Plus)
            .on_click(move |_, window, cx| crate::avd_dialog::open_new(maker.clone(), window, cx)),
    );
    // Wiping and deleting need the emulator stopped, so only those are listed.
    if !stopped.is_empty() {
        let (wipe_list, wiper) = (stopped.clone(), manager.clone());
        menu = menu.submenu("Wipe data", window, cx, move |menu, _, _| {
            wipe_list.iter().fold(menu, |menu, avd| {
                let (manager, avd) = (wiper.clone(), avd.clone());
                menu.item(
                    PopupMenuItem::new(format!("{}…", avd.replace('_', " "))).on_click(
                        move |_, window, cx| {
                            crate::avd_dialog::confirm_wipe(
                                manager.clone(),
                                avd.clone(),
                                window,
                                cx,
                            )
                        },
                    ),
                )
            })
        });
        let deleter = manager.clone();
        menu = menu.submenu("Delete emulator", window, cx, move |menu, _, _| {
            stopped.iter().fold(menu, |menu, avd| {
                let (manager, avd) = (deleter.clone(), avd.clone());
                menu.item(
                    PopupMenuItem::new(format!("{}…", avd.replace('_', " "))).on_click(
                        move |_, window, cx| {
                            crate::avd_dialog::confirm_delete(
                                manager.clone(),
                                avd.clone(),
                                window,
                                cx,
                            )
                        },
                    ),
                )
            })
        });
    }
    menu
}

fn loading(text: String, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(Spinner::new().small())
        .child(text)
        .into_any_element()
}

fn log_rows(
    id: &'static str,
    rows: Vec<(SharedString, Option<gpui_kit::Hsla>)>,
    cx: &App,
) -> impl IntoElement {
    v_flex()
        .id(id)
        .h_48()
        .overflow_y_scroll()
        .px_3()
        .py_1()
        .font_family(cx.theme().mono_font_family.clone())
        .text_xs()
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .py_2()
                    .text_color(cx.theme().muted_foreground)
                    .child("Nothing yet."),
            )
        })
        .children(rows.into_iter().map(|(text, color)| {
            div()
                .py_0p5()
                .when_some(color, |this, color| this.text_color(color))
                .child(text)
        }))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self.render_overlay(cx);
        let show_tools = overlay.is_none();
        v_flex()
            .size_full()
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(div().absolute().inset_0().child(self.surface.clone()))
                    .children(overlay)
                    .when(show_tools, |this| {
                        this.child(
                            h_flex()
                                .absolute()
                                .bottom_3()
                                .left_0()
                                .right_0()
                                .justify_center()
                                .child(self.tools.clone()),
                        )
                    }),
            )
            .child(self.render_drawer(cx))
    }
}
