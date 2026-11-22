//! Logcat as a tool of its own: the chosen device's log, beside whatever
//! else is open. It needs no screen: connecting to a device and reading its
//! log is all it takes. The same log shows under a mirrored screen, as a
//! drawer of the Devices pane ([`LogcatPanel::view`]).
//!
//! The device is the one chosen for the app ([`DeviceManager::selected`]),
//! changed from its header as from the Devices pane. The log starts when it
//! is first asked for (the tool or the drawer is shown, an app is run) and
//! then runs for as long as that device is in use: what the app wrote while
//! another tool was in front, a crash above all, is there on coming back.

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use ph_media::ToChat;
use ph_ui::{EmptyState, Icons, MenuButton as _};

use crate::adb::Adb;
use crate::devices_panel::device_menu;
use crate::logcat::{Crash, Logcat, LogcatEvent};
use crate::logcat_view::{LogcatView, LogcatViewEvent, SourceRef};
use crate::manager::{DeviceManager, DevicesChanged};

pub enum LogcatPanelEvent {
    /// A stack frame was clicked: show its file at its line.
    OpenSource(SourceRef),
    /// Long messages wrap, or not: the app keeps the choice.
    WrapChanged(bool),
    /// An app of the open project crashed.
    Crashed(Crash),
}

pub struct LogcatPanel {
    manager: Entity<DeviceManager>,
    view: Entity<LogcatView>,
    logcat: Option<Entity<Logcat>>,
    /// The device whose log runs.
    serial: Option<String>,
    /// The log was asked for: it runs while a device is in use.
    wanted: bool,
    /// The open project's application ids, read from its build files.
    declared: Vec<String>,
    /// Apps of the project run from here: theirs are known for sure.
    ran: Vec<String>,
    _subscriptions: Vec<Subscription>,
    _crashes: Option<Subscription>,
}

impl EventEmitter<ToChat> for LogcatPanel {}
impl EventEmitter<LogcatPanelEvent> for LogcatPanel {}

impl LogcatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let manager = DeviceManager::global(cx);
        let view = cx.new(|cx| LogcatView::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(&manager, |this, _, _: &DevicesChanged, cx| this.sync(cx)),
            cx.subscribe(&view, |_, _, event: &ToChat, cx| cx.emit(event.clone())),
            cx.subscribe(&view, |_, _, event: &LogcatViewEvent, cx| {
                cx.emit(match event {
                    LogcatViewEvent::OpenSource(source) => {
                        LogcatPanelEvent::OpenSource(source.clone())
                    }
                    LogcatViewEvent::WrapChanged(wrap) => LogcatPanelEvent::WrapChanged(*wrap),
                })
            }),
            cx.observe(&manager, |_, _, cx| cx.notify()),
        ];
        Self {
            manager,
            view,
            logcat: None,
            serial: None,
            wanted: false,
            declared: Vec::new(),
            ran: Vec::new(),
            _subscriptions: subscriptions,
            _crashes: None,
        }
    }

    /// The log itself, without this tool's header: for the Devices pane's
    /// drawer, which has the device above it already.
    pub fn view(&self) -> &Entity<LogcatView> {
        &self.view
    }

    /// Starts the log if it is not running: it was shown, or an app was run
    /// whose lines are wanted from its first.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.wanted {
            return;
        }
        self.wanted = true;
        self.manager
            .update(cx, |manager, cx| manager.ensure_tracking(cx));
        self.sync(cx);
    }

    /// Whether long messages wrap, as saved.
    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.view.update(cx, |view, cx| view.set_wrap(wrap, cx));
    }

    /// The folders of the open project: the apps built in them are the
    /// filter's `package:mine`.
    pub fn set_project_folders(
        &mut self,
        folders: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let declared = cx
                .background_spawn(async move {
                    let mut ids: Vec<String> = folders
                        .iter()
                        .flat_map(|folder| crate::android_project::application_ids(folder))
                        .collect();
                    ids.sort();
                    ids.dedup();
                    ids
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.declared = declared;
                this.share_project_apps(window, cx);
            });
        })
        .detach();
    }

    /// An app of the project was run: its lines are the project's from now.
    pub fn add_project_app(&mut self, package: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ran.iter().any(|known| known == package) {
            self.ran.push(package.to_string());
            self.share_project_apps(window, cx);
        }
    }

    fn project_apps(&self) -> Vec<String> {
        let mut apps = self.declared.clone();
        for app in &self.ran {
            if !apps.contains(app) {
                apps.push(app.clone());
            }
        }
        apps
    }

    fn share_project_apps(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let apps = self.project_apps();
        self.view
            .update(cx, |view, cx| view.set_project_apps(apps, window, cx));
    }

    /// Recent errors, for `@log`.
    pub fn recent_logs(&self, cx: &App) -> Option<String> {
        self.view.read(cx).recent_errors(cx)
    }

    fn adb(&self, cx: &App) -> Option<Adb> {
        self.manager.read(cx).adb().cloned()
    }

    fn selected_serial(&self, cx: &App) -> Option<String> {
        self.manager
            .read(cx)
            .selected()
            .filter(|device| device.is_ready())
            .map(|device| device.serial.clone())
    }

    /// Follows the chosen device once the log is wanted.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let wanted = self.wanted.then(|| self.selected_serial(cx)).flatten();
        if wanted == self.serial && (wanted.is_none() || self.logcat.is_some()) {
            return;
        }
        self.serial = wanted.clone();
        self._crashes = None;
        self.logcat = match (&wanted, self.adb(cx)) {
            (Some(serial), Some(adb)) => {
                self.load_packages(adb.clone(), serial.clone(), cx);
                let logcat = cx.new(|cx| Logcat::new(&adb, serial, cx));
                self._crashes = Some(cx.subscribe(&logcat, |this, _, event, cx| {
                    let LogcatEvent::Crashed(crash) = event;
                    // Other apps crash too; only the project's is news here.
                    let mine = crash.process.as_deref().is_some_and(|process| {
                        this.project_apps()
                            .iter()
                            .any(|app| crate::logcat::is_process_of(process, app, true))
                    });
                    if mine {
                        cx.emit(LogcatPanelEvent::Crashed(crash.clone()));
                    }
                }));
                Some(logcat)
            }
            _ => None,
        };
        let device = self
            .manager
            .read(cx)
            .selected()
            .map(|device| device.display_name())
            .unwrap_or_default();
        let logcat = self.logcat.clone();
        self.view
            .update(cx, |view, cx| view.set_logcat(logcat, device, cx));
        cx.notify();
    }

    /// The device's apps, for the app filter (off the UI thread).
    fn load_packages(&mut self, adb: Adb, serial: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let packages = cx
                .background_spawn(async move { adb.packages(&serial) })
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.view
                    .update(cx, |view, cx| view.set_packages(packages, cx));
            });
        })
        .detach();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let manager = self.manager.read(cx);
        let label: SharedString = match manager.selected() {
            Some(device) => device.display_name().into(),
            None if manager.devices().is_empty() => "No device".into(),
            None => "Choose a device".into(),
        };
        let manager_entity = self.manager.clone();
        h_flex()
            .flex_none()
            .h_9()
            .px_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("logcat-device")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Smartphone)
                    .label(label)
                    .dropdown_caret(true)
                    .tooltip("The device whose log this is")
                    .menu(gpui_kit::Anchor::TopLeft, move |menu, window, cx| {
                        device_menu(menu, &manager_entity, window, cx)
                    }),
            )
    }
}

impl Render for LogcatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let manager = self.manager.read(cx);
        // Not said while the SDK is still being looked for.
        let empty = if manager.adb().is_none() && !manager.looking_for_sdk() {
            Some(
                EmptyState::new("Android SDK not found")
                    .icon(Icons::ScrollText)
                    .description(crate::sdk::Sdk::missing_message()),
            )
        } else if self.serial.is_none() {
            Some(
                EmptyState::new(if manager.devices().is_empty() {
                    "No device connected"
                } else {
                    "No device chosen"
                })
                .icon(Icons::ScrollText)
                .description(
                    "Connect a phone with USB debugging on, or start an emulator; its log shows here. No screen needed.",
                ),
            )
        } else {
            None
        };
        v_flex().size_full().child(self.render_header(cx)).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .map(|this| match empty {
                    Some(empty) => this.items_center().justify_center().p_6().child(empty),
                    None => this.child(self.view.clone()),
                }),
        )
    }
}
