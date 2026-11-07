//! Quick chat: a small window over every other app, opened from anywhere
//! with a global keyboard shortcut (Settings → Chat), the command palette or
//! `pilothouse quick-chat`, to ask an agent something without going to
//! PilotHouse. Its chats are in the Chats space (no folder): they list under
//! Chats, and go on there.
//!
//! - macOS: a floating panel, over full-screen apps and on every Space,
//!   typed in without bringing PilotHouse's other windows forward.
//! - Windows: a small window kept on top of the others (`HWND_TOPMOST`).
//! - Linux: a small window, raised and focused when it opens; whether it
//!   stays on top is the window manager's to say.
//!
//! The shortcut is the system's (`global-hotkey`: Carbon on macOS,
//! `RegisterHotKey` on Windows, an X11 key grab on Linux), and works while
//! PilotHouse runs. Wayland lets no app take a key for itself: there the
//! desktop's keyboard settings run `pilothouse quick-chat`, which works on
//! every system.

use std::str::FromStr as _;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle, Global,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Pixels, Render, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window,
    WindowBounds, WindowKind, WindowOptions, actions, div, px,
};
use ph_agent::{ChatPanel, ChatPanelEvent};
use ph_brand::Feature;
use ph_store::Store;
use ph_store::projects::Project;
use ph_store::sessions::SessionId;
use ph_theme::layout;
use ph_ui::Icons;
use serde::{Deserialize, Serialize};

use crate::AppState;

/// The shortcut as set in Settings (`Alt+Space`); empty: none.
pub(crate) const SHORTCUT_KEY: &str = "quick_chat.shortcut";
/// Where the window was last (on the screen it was on).
const BOUNDS_KEY: &str = "quick_chat.bounds";
/// The chat it showed last, shown again when it opens.
const SESSION_KEY: &str = "quick_chat.session";
const KEY_CONTEXT: &str = "QuickChat";

actions!(
    quick_chat,
    [
        /// Hide the quick chat window (its chat goes on).
        CloseQuickChat,
        /// A new chat in the quick chat window.
        NewQuickChat,
    ]
);

/// The shortcut until one is set: one that apps rarely take.
pub(crate) fn default_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "Alt+Space"
    } else {
        "Alt+Shift+Space"
    }
}

/// The shortcut in use: as set (empty: none), else the default.
pub(crate) fn shortcut(store: &Store) -> Option<String> {
    let set: Option<String> = store.setting(SHORTCUT_KEY);
    match set {
        Some(set) if set.trim().is_empty() => None,
        Some(set) => Some(set),
        None => Some(default_shortcut().to_string()),
    }
}

/// The shortcut as people read it here: `⌥Space` on macOS, `Alt+Space`
/// elsewhere.
pub(crate) fn shortcut_label(shortcut: &str) -> String {
    let parts: Vec<&str> = shortcut.split('+').map(str::trim).collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return shortcut.to_string();
    };
    let key = match key.to_ascii_lowercase().as_str() {
        "space" => "Space".to_string(),
        other if other.len() == 1 => other.to_ascii_uppercase(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        }
    };
    let modifier = |name: &str| -> &'static str {
        match (
            name.to_ascii_lowercase().as_str(),
            cfg!(target_os = "macos"),
        ) {
            ("ctrl" | "control", true) => "⌃",
            ("alt" | "option", true) => "⌥",
            ("shift", true) => "⇧",
            ("super" | "cmd" | "command" | "meta", true) => "⌘",
            ("ctrl" | "control", false) => "Ctrl+",
            ("alt" | "option", false) => "Alt+",
            ("shift", false) => "Shift+",
            ("super" | "cmd" | "command" | "meta", false) => "Super+",
            _ => "",
        }
    };
    // In the order the system shows them.
    let order = [
        "ctrl", "control", "alt", "option", "shift", "super", "cmd", "command", "meta",
    ];
    let mut ordered: Vec<&str> = modifiers.to_vec();
    ordered.sort_by_key(|name| {
        order
            .iter()
            .position(|known| known.eq_ignore_ascii_case(name))
            .unwrap_or(order.len())
    });
    ordered
        .iter()
        .map(|name| modifier(name))
        .collect::<String>()
        + &key
}

/// A keystroke pressed in Settings as a shortcut (`alt-shift-k` as GPUI
/// writes it, here `Alt+Shift+K`), or why it cannot be one: it needs a
/// modifier (or is a function key), and a key that is not one.
pub(crate) fn shortcut_of(keystroke: &gpui_kit::Keystroke) -> Result<String, &'static str> {
    let modifiers = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if matches!(
        key,
        "shift" | "control" | "alt" | "platform" | "function" | "cmd" | "ctrl" | "fn" | ""
    ) {
        return Err("Hold the modifiers and press a key.");
    }
    let function_key =
        key.len() > 1 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit());
    let any_modifier = modifiers.control || modifiers.alt || modifiers.shift || modifiers.platform;
    if !any_modifier && !function_key {
        return Err("A shortcut needs Ctrl, Alt, Shift or ⌘/Win with its key.");
    }
    if modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.platform {
        return Err("Shift alone types: add Ctrl, Alt or ⌘/Win.");
    }
    let key = match key {
        "space" => "Space".to_string(),
        "enter" => "Enter".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "home" | "end" | "pageup" | "pagedown" | "up" | "down" | "left" | "right" => {
            return Err("Pick a letter, digit, Space or function key.");
        }
        "escape" | "tab" => return Err("That key is taken: pick another."),
        key if key.chars().count() == 1 => key.to_ascii_uppercase(),
        key if function_key => key.to_ascii_uppercase(),
        _ => return Err("Pick a letter, digit, Space or function key."),
    };
    let mut parts: Vec<&str> = Vec::new();
    if modifiers.control {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.platform {
        parts.push("Super");
    }
    let text = parts.join("+") + if parts.is_empty() { "" } else { "+" } + &key;
    // The system's own reading of it decides.
    HotKey::from_str(&text)
        .map(|_| text)
        .map_err(|_| "That key cannot be a global shortcut: pick another.")
}

/// Lets go of the shortcut while a new one is pressed in Settings (the
/// system would take it first); [`apply_shortcut`] takes one again.
fn pause_shortcut(cx: &mut App) {
    if let Some(keys) = cx.default_global::<QuickChat>().keys.as_mut()
        && let Some(current) = keys.current.take()
    {
        let _ = keys.manager.unregister(current);
    }
}

/// Saves `shortcut` (`None`: none) and takes it.
fn set_shortcut(shortcut: Option<&str>, cx: &mut App) {
    let _ = AppState::store(cx).set_setting(SHORTCUT_KEY, &shortcut.unwrap_or_default());
    apply_shortcut(cx);
}

/// Settings → Chat → Quick chat: the shortcut, pressed to set it.
#[derive(gpui_kit::IntoElement)]
pub(crate) struct ShortcutRecorder;

/// What the recorder is doing, kept from frame to frame.
struct Recording {
    on: bool,
    error: Option<&'static str>,
    focus: FocusHandle,
}

impl gpui_kit::RenderOnce for ShortcutRecorder {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state("quick-chat-shortcut", cx, |_, cx| Recording {
            on: false,
            error: None,
            focus: cx.focus_handle(),
        });
        let (on, error, focus) = {
            let state = state.read(cx);
            (state.on, state.error, state.focus.clone())
        };
        let current = cx
            .try_global::<QuickChat>()
            .and_then(|state| state.shortcut.clone());
        let label = if on {
            "Press the keys…".to_string()
        } else {
            current
                .as_deref()
                .map(shortcut_label)
                .unwrap_or_else(|| "Off".into())
        };
        let problem = if on { None } else { shortcut_problem(cx) };
        let theme = cx.theme();
        let field = div()
            .id("quick-chat-shortcut")
            .track_focus(&focus)
            .min_w(px(140.))
            .px_3()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(if on { theme.ring } else { theme.input })
            .bg(theme.background)
            .text_sm()
            .text_center()
            .cursor_pointer()
            .child(label)
            .on_click({
                let (state, focus) = (state.clone(), focus.clone());
                move |_, window, cx| {
                    pause_shortcut(cx);
                    state.update(cx, |state, cx| {
                        state.on = true;
                        state.error = None;
                        cx.notify();
                    });
                    window.focus(&focus, cx);
                }
            })
            // Seen before any binding, while recording.
            .capture_key_down({
                let state = state.clone();
                move |event, _, cx| {
                    if !state.read(cx).on {
                        return;
                    }
                    cx.stop_propagation();
                    if event.keystroke.key == "escape"
                        && event.keystroke.modifiers.number_of_modifiers() == 0
                    {
                        state.update(cx, |state, cx| {
                            state.on = false;
                            cx.notify();
                        });
                        apply_shortcut(cx);
                        return;
                    }
                    match shortcut_of(&event.keystroke) {
                        Ok(text) => {
                            state.update(cx, |state, cx| {
                                state.on = false;
                                state.error = None;
                                cx.notify();
                            });
                            set_shortcut(Some(&text), cx);
                        }
                        // Modifiers alone: wait for the key.
                        Err(_) if event.keystroke.key.is_empty() => {}
                        Err(reason) => state.update(cx, |state, cx| {
                            state.error = Some(reason);
                            cx.notify();
                        }),
                    }
                }
            })
            .on_mouse_down_out({
                let state = state.clone();
                move |_, _, cx| {
                    if state.read(cx).on {
                        state.update(cx, |state, cx| {
                            state.on = false;
                            cx.notify();
                        });
                        apply_shortcut(cx);
                    }
                }
            });
        v_flex()
            .gap_1()
            .items_end()
            .child(
                h_flex()
                    .gap_1()
                    .child(field)
                    .child(
                        Button::new("quick-chat-shortcut-reset")
                            .ghost()
                            .small()
                            .label("Reset")
                            .tooltip(format!("Back to {}", shortcut_label(default_shortcut())))
                            .on_click(|_, _, cx| {
                                let _ =
                                    AppState::store(cx).set_setting(SHORTCUT_KEY, &None::<String>);
                                apply_shortcut(cx);
                            }),
                    )
                    .child(
                        Button::new("quick-chat-shortcut-off")
                            .ghost()
                            .small()
                            .label("Turn off")
                            .on_click(|_, _, cx| set_shortcut(None, cx)),
                    ),
            )
            .children(error.or(problem.as_deref().map(|_| "")).map(|_| {
                div()
                    .max_w(px(360.))
                    .text_xs()
                    .text_color(theme.warning)
                    .child(
                        error
                            .map(str::to_string)
                            .or(problem.clone())
                            .unwrap_or_default(),
                    )
            }))
    }
}

/// Whether this is a Wayland session, where no app can take a key.
fn wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind == "wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some())
}

#[derive(Default)]
struct QuickChat {
    /// The window while it is open, and its view.
    window: Option<(AnyWindowHandle, WeakEntity<QuickChatWindow>)>,
    keys: Option<Keys>,
    /// The shortcut set (as saved), for Settings to show without reading
    /// the store on each draw.
    shortcut: Option<String>,
    /// Why the shortcut is not in use, for Settings.
    problem: Option<String>,
}

impl Global for QuickChat {}

/// The system's shortcut, while one is held.
struct Keys {
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

/// Listens for the shortcut and takes the one set (once, at start).
pub(crate) fn init(cx: &mut App) {
    if !ph_brand::enabled(Feature::QuickChat) {
        return;
    }
    cx.bind_keys([
        KeyBinding::new("escape", CloseQuickChat, Some(KEY_CONTEXT)),
        KeyBinding::new(ph_ui::keys("secondary-n"), NewQuickChat, Some(KEY_CONTEXT)),
    ]);
    // Pressed on the system's thread for it; acted on here.
    let (pressed, presses) = smol::channel::unbounded::<()>();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed {
            let _ = pressed.try_send(());
        }
    }));
    cx.spawn(async move |cx| {
        while presses.recv().await.is_ok() {
            cx.update(toggle);
        }
    })
    .detach();
    apply_shortcut(cx);
}

/// Takes the shortcut set in Settings (letting go of the one before).
pub(crate) fn apply_shortcut(cx: &mut App) {
    if !ph_brand::enabled(Feature::QuickChat) {
        return;
    }
    let wanted = shortcut(AppState::store(cx));
    let state = cx.default_global::<QuickChat>();
    state.shortcut = wanted.clone();
    state.problem = None;
    if wayland() {
        state.problem = Some(
            "On Wayland the desktop keeps keys for itself: in its keyboard settings, give `pilothouse quick-chat` a shortcut."
                .into(),
        );
        return;
    }
    if state.keys.is_none() {
        match GlobalHotKeyManager::new() {
            Ok(manager) => {
                state.keys = Some(Keys {
                    manager,
                    current: None,
                })
            }
            Err(err) => {
                state.problem = Some(format!("Global shortcuts are not available here: {err}."));
                return;
            }
        }
    }
    let Some(keys) = state.keys.as_mut() else {
        return;
    };
    if let Some(before) = keys.current.take() {
        let _ = keys.manager.unregister(before);
    }
    let Some(text) = wanted else {
        return;
    };
    let problem = match HotKey::from_str(&text) {
        Ok(hotkey) => match keys.manager.register(hotkey) {
            Ok(()) => {
                keys.current = Some(hotkey);
                None
            }
            Err(err) => Some(format!(
                "{} is taken by another app or the system ({err}): pick another.",
                shortcut_label(&text)
            )),
        },
        Err(_) => Some(format!("{text} is not a shortcut: set it again.")),
    };
    state.problem = problem;
}

/// Why the shortcut does not work, if it does not.
pub(crate) fn shortcut_problem(cx: &App) -> Option<String> {
    cx.try_global::<QuickChat>()
        .and_then(|state| state.problem.clone())
}

/// The shortcut, the command or the palette: opens the window, brings it
/// forward, or hides it when it is the one in front.
pub fn toggle(cx: &mut App) {
    if !ph_brand::enabled(Feature::QuickChat) {
        return;
    }
    if let Some((handle, view)) = cx.default_global::<QuickChat>().window.clone() {
        match handle.update(cx, |_, window, _| window.is_window_active()) {
            Ok(true) => return close(cx),
            Ok(false) => {
                let _ = handle.update(cx, |_, window, cx| {
                    window.activate_window();
                    let _ = view.update(cx, |view, cx| view.focus(window, cx));
                });
                return;
            }
            // Closed meanwhile (its own button).
            Err(_) => cx.default_global::<QuickChat>().window = None,
        }
    }
    if let Err(err) = open(cx) {
        tracing::warn!("opening the quick chat failed: {err:#}");
    }
}

/// Hides the window; its chat goes on (in the Chats space).
fn close(cx: &mut App) {
    if let Some((handle, _)) = cx.default_global::<QuickChat>().window.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Saved window bounds, in screen coordinates.
#[derive(Serialize, Deserialize, Clone, Copy)]
struct Saved {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

const DEFAULT_SIZE: (f32, f32) = (440., 560.);
const MIN_SIZE: (f32, f32) = (320., 240.);

/// Where it opens: where it was, while that is still on a screen; else
/// centred on the main screen.
fn bounds(store: &Store, cx: &App) -> Bounds<Pixels> {
    let saved: Option<Saved> = store.setting(BOUNDS_KEY);
    saved
        .map(|saved| Bounds {
            origin: gpui_kit::point(px(saved.x), px(saved.y)),
            size: Size::new(
                px(saved.width.max(MIN_SIZE.0)),
                px(saved.height.max(MIN_SIZE.1)),
            ),
        })
        .filter(|bounds| {
            // At least its title bar on a screen still there.
            let bar = Bounds {
                origin: bounds.origin,
                size: Size::new(bounds.size.width, layout::TITLE_BAR_HEIGHT),
            };
            cx.displays()
                .iter()
                .any(|display| display.bounds().intersects(&bar))
        })
        .unwrap_or_else(|| {
            Bounds::centered(None, Size::new(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx)
        })
}

fn open(cx: &mut App) -> anyhow::Result<()> {
    let store = AppState::store(cx).clone();
    let project = store.chats_space(&ph_store::paths::scratch_root())?;
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds(&store, cx))),
        window_min_size: Some(Size::new(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        // A floating panel on macOS; elsewhere a window, kept on top.
        kind: if cfg!(target_os = "macos") {
            WindowKind::PopUp
        } else {
            WindowKind::Normal
        },
        is_minimizable: false,
        focus: true,
        show: true,
        ..TitleBar::window_options()
    };
    if let Some(titlebar) = options.titlebar.as_mut() {
        let (x, y) = layout::TRAFFIC_LIGHTS;
        titlebar.traffic_light_position = Some(gpui_kit::point(x, y));
        titlebar.title = Some("Quick chat".into());
    }
    let (handle, view) = gpui_kit::open_window(options, cx, move |window, cx| {
        ph_theme::apply_appearance(&AppState::appearance(cx), Some(window), cx);
        stay_on_top(window);
        cx.new(|cx| QuickChatWindow::new(store, project, window, cx))
    })?;
    let _ = handle.update(cx, |_, window, cx| {
        window.activate_window();
        view.update(cx, |view, cx| view.focus(window, cx));
    });
    cx.default_global::<QuickChat>().window = Some((handle, view.downgrade()));
    Ok(())
}

/// Windows: above every window that is not on top itself.
#[cfg(windows)]
fn stay_on_top(window: &Window) {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    let Ok(handle) = window.window_handle() else {
        return;
    };
    if let RawWindowHandle::Win32(win32) = handle.as_raw() {
        // SAFETY: the window is open; this only changes its z-order.
        unsafe {
            SetWindowPos(
                win32.hwnd.get() as _,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

/// macOS: the panel floats already; Linux: the window manager's to say.
#[cfg(not(windows))]
fn stay_on_top(_: &Window) {}

/// The quick chat window: a chat of the Chats space.
pub(crate) struct QuickChatWindow {
    store: Store,
    project: Project,
    chat: Entity<ChatPanel>,
    focus: FocusHandle,
    /// Where it is, not saved yet, and the wait before saving it.
    bounds: Option<Saved>,
    save_bounds: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl QuickChatWindow {
    fn new(store: Store, project: Project, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = cx.new(|cx| ChatPanel::new(store.clone(), project.clone(), window, cx));
        // The chat it showed last, while it is still there.
        let last: Option<SessionId> = store.setting(SESSION_KEY);
        let session = last
            .and_then(|id| store.session(id).ok().flatten())
            .filter(|session| session.project_id == project.id && session.kind == "chat");
        chat.update(cx, |chat, cx| chat.set_session(session, cx));
        let events = cx.subscribe_in(&chat, window, |this, chat, event, window, cx| {
            match event {
                ChatPanelEvent::SessionsChanged => {
                    let shown = chat.read(cx).active_session_id(cx);
                    let _ = this.store.set_setting(SESSION_KEY, &shown);
                    // The Chats list in the main window shows it.
                    crate::session_changes::note(this.project.id, cx);
                }
                ChatPanelEvent::SelectSession(id) => {
                    let session = id.and_then(|id| this.store.session(id).ok().flatten());
                    let none = session.is_none();
                    chat.update(cx, |chat, cx| {
                        chat.set_session(session, cx);
                        if none {
                            chat.focus_composer(window, cx);
                        }
                    });
                    let _ = this.store.set_setting(SESSION_KEY, id);
                }
                // Anything that needs a workspace goes on in PilotHouse.
                _ => {}
            }
        });
        // Where it is, for next time: saved once it stops moving (bounds
        // change every frame of a drag), or as it closes.
        let bounds = cx.observe_window_bounds(window, |this, window, cx| {
            let bounds = window.bounds();
            this.bounds = Some(Saved {
                x: bounds.origin.x.into(),
                y: bounds.origin.y.into(),
                width: bounds.size.width.into(),
                height: bounds.size.height.into(),
            });
            this.save_bounds = cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                _ = this.update(cx, |this, _| this.keep_bounds());
            });
        });
        // Closed with its own button: no longer the open one.
        let mine = window.window_handle().window_id();
        let released = cx.on_release(move |this, cx| {
            this.keep_bounds();
            let state = cx.default_global::<QuickChat>();
            if state
                .window
                .as_ref()
                .is_some_and(|(handle, _)| handle.window_id() == mine)
            {
                state.window = None;
            }
        });
        Self {
            store,
            project,
            chat,
            focus: cx.focus_handle(),
            bounds: None,
            save_bounds: Task::ready(()),
            _subscriptions: vec![events, bounds, released],
        }
    }

    /// Saves where the window is, if it moved since last saved.
    fn keep_bounds(&mut self) {
        if let Some(saved) = self.bounds.take() {
            let _ = self.store.set_setting(BOUNDS_KEY, &saved);
        }
    }

    /// The keyboard goes to the composer.
    fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat
            .update(cx, |chat, cx| chat.focus_composer(window, cx));
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.set_session(None, cx);
            chat.focus_composer(window, cx);
        });
        let _ = self.store.set_setting(SESSION_KEY, &None::<SessionId>);
    }

    /// The chat in the main window, under Chats.
    fn open_in_app(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let session = self.chat.read(cx).active_session_id(cx);
        let project = self.project.id;
        cx.defer(move |cx| {
            let Some((handle, workspace)) = crate::open::main_workspace(cx) else {
                return;
            };
            let _ = handle.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| match session {
                    Some(session) => workspace.open_chat(project, session, window, cx),
                    None => workspace.new_chat_in(project, window, cx),
                });
                window.activate_window();
            });
            cx.activate(true);
            close(cx);
        });
    }
}

impl Render for QuickChatWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = TitleBar::new().h(layout::TITLE_BAR_HEIGHT).child(
            h_flex()
                .flex_1()
                .min_w_0()
                .pr_2()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Quick chat"),
                )
                .child(
                    Button::new("quick-chat-new")
                        // Windows treats the bar as a drag area otherwise.
                        .occlude()
                        .ghost()
                        .small()
                        .icon(Icons::SquarePen)
                        .accessibility_label("New chat")
                        .tooltip("New chat")
                        .on_click(cx.listener(|this, _, window, cx| this.new_chat(window, cx))),
                )
                .child(
                    Button::new("quick-chat-open")
                        .occlude()
                        .ghost()
                        .small()
                        .icon(Icons::ExternalLink)
                        .accessibility_label(format!("Open in {}", ph_brand::NAME))
                        .tooltip(format!("Open in {}", ph_brand::NAME))
                        .on_click(cx.listener(|this, _, window, cx| this.open_in_app(window, cx))),
                ),
        );
        v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &CloseQuickChat, _, cx| cx.defer(close)))
            .on_action(cx.listener(|this, _: &NewQuickChat, window, cx| this.new_chat(window, cx)))
            .size_full()
            .bg(cx.theme().background)
            .child(bar)
            .child(div().flex_1().min_h_0().child(self.chat.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_shortcuts() {
        assert!(HotKey::from_str("Alt+Space").is_ok());
        assert!(HotKey::from_str("Alt+Shift+Space").is_ok());
    }

    #[test]
    fn keystrokes_become_shortcuts() {
        fn stroke(text: &'static str) -> gpui_kit::Keystroke {
            gpui_kit::Keystroke::parse(text).unwrap()
        }
        assert_eq!(
            shortcut_of(&stroke("alt-space")).as_deref(),
            Ok("Alt+Space")
        );
        assert_eq!(
            shortcut_of(&stroke("ctrl-shift-k")).as_deref(),
            Ok("Ctrl+Shift+K")
        );
        assert_eq!(shortcut_of(&stroke("f9")).as_deref(), Ok("F9"));
        assert!(shortcut_of(&stroke("k")).is_err(), "no modifier");
        assert!(
            shortcut_of(&stroke("shift-k")).is_err(),
            "Shift alone types"
        );
        assert!(shortcut_of(&stroke("alt-escape")).is_err());
    }

    #[test]
    fn shortcuts_read_as_the_system_shows_them() {
        let label = shortcut_label("Shift+Alt+Space");
        if cfg!(target_os = "macos") {
            assert_eq!(label, "⌥⇧Space");
        } else {
            assert_eq!(label, "Alt+Shift+Space");
        }
    }
}
