//! Logcat as a tool of its own: the chosen device's log, beside whatever
//! else is open (by default at the bottom of the window, where long lines
//! have room). It needs no screen: connecting to a device and reading its
//! log is all it takes.
//!
//! The device is the one chosen for the app ([`DeviceManager::selected`]),
//! changed from its header as from the Devices pane. Logcat runs only while
//! the tool is on screen.

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
use crate::logcat::Logcat;
use crate::logcat_view::LogcatView;
use crate::manager::{DeviceManager, DevicesChanged};

pub struct LogcatPanel {
    manager: Entity<DeviceManager>,
    view: Entity<LogcatView>,
    logcat: Option<Entity<Logcat>>,
    /// The device whose log runs.
    serial: Option<String>,
    active: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ToChat> for LogcatPanel {}

impl LogcatPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let manager = DeviceManager::global(cx);
        let view = cx.new(|cx| LogcatView::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(&manager, |this, _, _: &DevicesChanged, cx| this.sync(cx)),
            cx.subscribe(&view, |_, _, event: &ToChat, cx| cx.emit(event.clone())),
            cx.observe(&manager, |_, _, cx| cx.notify()),
        ];
        Self {
            manager,
            view,
            logcat: None,
            serial: None,
            active: false,
            _subscriptions: subscriptions,
        }
    }

    /// On screen or not: logcat runs only while it is.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.manager
                .update(cx, |manager, cx| manager.ensure_tracking(cx));
        }
        self.sync(cx);
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

    /// Follows the chosen device while on screen; stops otherwise.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let wanted = self.active.then(|| self.selected_serial(cx)).flatten();
        if wanted == self.serial && (wanted.is_none() || self.logcat.is_some()) {
            return;
        }
        self.serial = wanted.clone();
        self.logcat = match (&wanted, self.adb(cx)) {
            (Some(serial), Some(adb)) => {
                self.load_packages(adb.clone(), serial.clone(), cx);
                Some(cx.new(|cx| Logcat::new(&adb, serial, cx)))
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
        let empty = if manager.adb().is_none() {
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
