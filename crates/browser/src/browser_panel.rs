//! The browser pane: address bar, live page, annotation tools and logs.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::PopupMenuItem,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use ph_media::annotate::Shape;
use ph_media::{LiveTools, SurfaceEvent, SurfaceView, ToChat};
use ph_ui::MenuButton as _;
use ph_ui::{EmptyState, Icons};

use crate::browser::{Browser, BrowserEvent, BrowserStatus, Viewport};
use crate::hub::BrowserHub;
use crate::logs::Level;

/// Rows shown in the log drawer (the newest).
const DRAWER_ROWS: usize = 200;
/// Lines sent to chat from the drawer.
const SEND_LINES: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drawer {
    Console,
    Network,
}

pub struct BrowserPanel {
    browser: Entity<Browser>,
    surface: Entity<SurfaceView>,
    tools: Entity<LiveTools>,
    address: Entity<InputState>,
    drawer: Option<Drawer>,
    errors_only: bool,
    active: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ToChat> for BrowserPanel {}

impl BrowserPanel {
    /// Asks for a browser program (a macOS app is fine) and uses it.
    fn choose_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use this browser".into()),
        });
        let browser = self.browser.clone();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(picked) = paths.into_iter().next() else {
                return;
            };
            let _ = cx.update(|window, cx| {
                if let Err(err) =
                    browser.update(cx, |browser, cx| browser.choose_browser(&picked, cx))
                {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(
                        gpui_kit::component::notification::Notification::error(format!("{err:#}")),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub fn new(project_id: i64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let browser = BrowserHub::browser(project_id, cx);
        let surface = cx.new(|cx| SurfaceView::new("", cx));
        let tools = cx.new(|cx| LiveTools::new(surface.clone(), "browser", true, cx));
        let address = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).placeholder("Enter an address, e.g. localhost:3000");
            state.set_value(browser.read(cx).url().to_string(), window, cx);
            state
        });

        let subscriptions = vec![
            cx.subscribe_in(
                &browser,
                window,
                |this, browser, event: &BrowserEvent, window, cx| match event {
                    BrowserEvent::Frame(frame) => {
                        this.surface
                            .update(cx, |surface, cx| surface.set_frame(frame.clone(), cx));
                        this.tools.update(cx, |tools, _| tools.push_frame(frame));
                    }
                    BrowserEvent::Changed => {
                        // Follow navigation in the address bar unless the user is typing.
                        let url = browser.read(cx).url().to_string();
                        let editing = this.address.read(cx).focus_handle(cx).is_focused(window);
                        if !editing && this.address.read(cx).value() != url {
                            this.address
                                .update(cx, |state, cx| state.set_value(url, window, cx));
                        }
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &address,
                window,
                |this, address, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let input = address.read(cx).value().to_string();
                        if !input.trim().is_empty() {
                            this.browser
                                .update(cx, |browser, cx| browser.open(&input, cx));
                            this.surface.read(cx).focus_handle(cx).focus(window, cx);
                        }
                    }
                },
            ),
            cx.subscribe(&surface, |this, _, event: &SurfaceEvent, cx| {
                this.surface_event(event, cx)
            }),
            cx.subscribe(&tools, |_, _, event: &ToChat, cx| cx.emit(event.clone())),
            cx.observe(&browser, |_, _, cx| cx.notify()),
        ];
        Self {
            browser,
            surface,
            tools,
            address,
            drawer: None,
            errors_only: false,
            active: false,
            _subscriptions: subscriptions,
        }
    }

    /// Called when the pane is shown or hidden: frames stream only while shown.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            self.browser
                .update(cx, |browser, cx| browser.set_screencast(active, cx));
        }
    }

    pub fn tools(&self) -> &Entity<LiveTools> {
        &self.tools
    }

    /// The current page with the user's marks, for `@screenshot`.
    pub fn composite(&self, cx: &gpui_kit::App) -> Option<ph_media::Frame> {
        self.surface.read(cx).composite()
    }

    /// Recent console errors and failed requests, for `@log`.
    pub fn recent_logs(&self, cx: &gpui_kit::App) -> Option<String> {
        recent_logs(&self.browser, cx)
    }

    fn surface_event(&mut self, event: &SurfaceEvent, cx: &mut Context<Self>) {
        match event {
            SurfaceEvent::Pointer(pointer) => self.browser.read(cx).pointer(*pointer),
            SurfaceEvent::Key(keystroke) if keystroke.modifiers.platform => {
                self.editing_shortcut(&keystroke.key, cx)
            }
            SurfaceEvent::Key(keystroke) => self.browser.read(cx).key(keystroke),
            SurfaceEvent::Picked { x, y } => self.pick(*x, *y, cx),
        }
    }

    /// ⌘C/⌘V/⌘X/⌘A/⌘Z inside the page, through the system clipboard.
    fn editing_shortcut(&mut self, key: &str, cx: &mut Context<Self>) {
        let browser = self.browser.clone();
        match key {
            "v" => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    browser.read(cx).insert_text(&text);
                }
            }
            "c" | "x" => {
                let cut = key == "x";
                let selection = browser.update(cx, |browser, cx| browser.selected_text(cx));
                cx.spawn(async move |_, cx| {
                    if let Ok(text) = selection.await
                        && !text.is_empty()
                    {
                        cx.update(|cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            if cut {
                                browser.update(cx, |browser, cx| {
                                    browser
                                        .evaluate("document.execCommand('delete')".into(), cx)
                                        .detach()
                                });
                            }
                        });
                    }
                })
                .detach();
            }
            "a" | "z" => {
                let command = if key == "a" { "selectAll" } else { "undo" };
                browser.update(cx, |browser, cx| {
                    browser
                        .evaluate(format!("document.execCommand('{command}')"), cx)
                        .detach()
                });
            }
            _ => {}
        }
    }

    /// Element picker: outline the element and send its details to chat.
    fn pick(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        let picked = self
            .browser
            .update(cx, |browser, cx| browser.pick(x, y, cx));
        let url = self.browser.read(cx).url().to_string();
        cx.spawn(async move |this, cx| {
            let Ok(element) = picked.await else { return };
            let _ = this.update(cx, |this, cx| {
                let rect = &element["frame_rect"];
                let (x, y) = (
                    rect["x"].as_f64().unwrap_or(0.0) as f32,
                    rect["y"].as_f64().unwrap_or(0.0) as f32,
                );
                let (w, h) = (
                    rect["width"].as_f64().unwrap_or(0.0) as f32,
                    rect["height"].as_f64().unwrap_or(0.0) as f32,
                );
                this.surface.update(cx, |surface, cx| {
                    surface.add_annotation(
                        Shape::Rect {
                            from: (x, y),
                            to: (x + w, y + h),
                        },
                        cx,
                    )
                });
                let tag = element["tag"].as_str().unwrap_or("element");
                let mut text = format!(
                    "Element on {url}\nSelector: {}\nText: {}\nStyle: {}",
                    element["selector"].as_str().unwrap_or_default(),
                    element["text"].as_str().unwrap_or_default(),
                    element["styles"].as_str().unwrap_or_default(),
                );
                if let Some(css) = element["css"].as_str().filter(|css| !css.is_empty()) {
                    text.push_str(&format!("\nCSS: {css}"));
                }
                if let Some(source) = element["source"].as_str() {
                    text.push_str(&format!("\nSource: {source}"));
                }
                text.push_str(&format!(
                    "\n\n```html\n{}\n```",
                    element["html"].as_str().unwrap_or_default()
                ));
                cx.emit(ToChat::Text {
                    label: format!("<{tag}> element"),
                    text,
                });
            });
        })
        .detach();
    }

    fn send_logs(&mut self, cx: &mut Context<Self>) {
        let browser = self.browser.read(cx);
        let (label, text) = match self.drawer {
            Some(Drawer::Network) => (
                "Network requests",
                browser.logs.network_text(SEND_LINES, self.errors_only),
            ),
            _ => (
                "Console output",
                browser.logs.console_text(SEND_LINES, self.errors_only),
            ),
        };
        if text.is_empty() {
            return;
        }
        let text = format!("{label} from {}:\n```\n{text}\n```", browser.url());
        cx.emit(ToChat::Text {
            label: label.to_lowercase(),
            text,
        });
    }

    /// Keeps the page the size of the pane, in CSS pixels.
    fn sync_viewport(&self, window: &Window, cx: &mut Context<Self>) {
        let size = self.surface.read(cx).view_size();
        let viewport = Viewport {
            width: f32::from(size.width) as u32,
            height: f32::from(size.height) as u32,
            scale: window.scale_factor(),
        };
        self.browser
            .update(cx, |browser, cx| browser.set_viewport(viewport, cx));
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let browser = self.browser.read(cx);
        let loading = browser.is_loading();
        let running = *browser.status() == BrowserStatus::Running;
        let nav = |id: &'static str, icon: IconName, label: &'static str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(label)
                .accessibility_label(label)
        };
        h_flex()
            .flex_none()
            .h_11()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                nav("browser-back", IconName::ArrowLeft, "Back")
                    .on_click(cx.listener(|this, _, _, cx| this.browser.read(cx).go_back())),
            )
            .child(
                nav("browser-forward", IconName::ArrowRight, "Forward")
                    .on_click(cx.listener(|this, _, _, cx| this.browser.read(cx).go_forward())),
            )
            .child(
                Button::new("browser-reload")
                    .ghost()
                    .small()
                    .icon(Icons::RotateCw)
                    .tooltip("Reload")
                    .accessibility_label("Reload")
                    .on_click(cx.listener(|this, _, _, cx| this.browser.read(cx).reload())),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.address)
                        .small()
                        .rounded_full()
                        .prefix(
                            gpui_kit::component::Icon::new(IconName::Globe)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .when(loading, |this| this.suffix(Spinner::new().xsmall())),
                ),
            )
            .child(
                Button::new("browser-menu")
                    .ghost()
                    .small()
                    .icon(IconName::Ellipsis)
                    .accessibility_label("Browser options")
                    .menu(gpui_kit::Anchor::TopRight, {
                        let browser = self.browser.clone();
                        move |menu, _, cx| {
                            let endpoint = browser.read(cx).devtools_url();
                            let port_on = browser.read(cx).devtools_port_enabled();
                            let stop = browser.clone();
                            let toggle_port = browser.clone();
                            menu.item(
                                PopupMenuItem::new("Allow other DevTools clients")
                                    .checked(port_on)
                                    .on_click(move |_, _, cx| {
                                        toggle_port.update(cx, |browser, cx| {
                                            browser.set_devtools_port(!port_on, cx)
                                        })
                                    }),
                            )
                            .when_some(endpoint, |menu, endpoint| {
                                menu.item(
                                    PopupMenuItem::new("Copy DevTools address")
                                        .icon(Icons::Plug)
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                endpoint.clone(),
                                            ))
                                        }),
                                )
                            })
                            .separator()
                            .item(
                                PopupMenuItem::new("Open PilotHouse Chrome (for agents)")
                                    .icon(IconName::Globe)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(crate::OpenAgentChrome), cx)
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(if running {
                                    "Stop the browser"
                                } else {
                                    "Start the browser"
                                })
                                .icon(Icons::Power)
                                .on_click(move |_, _, cx| {
                                    stop.update(cx, |browser, cx| {
                                        if *browser.status() == BrowserStatus::Running {
                                            browser.stop(cx)
                                        } else {
                                            browser.ensure_started(cx)
                                        }
                                    })
                                }),
                            )
                        }
                    }),
            )
    }

    /// What covers the page while Chrome is not showing one.
    /// A page dialog (confirm, prompt, leave page) waiting for the user.
    fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.browser.read(cx).dialog()?.clone();
        let title = match dialog.kind.as_str() {
            "beforeunload" => "Leave this page?",
            "prompt" => "The page asks",
            _ => "The page asks you to confirm",
        };
        Some(
            div()
                .id("browser-dialog")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::black().opacity(0.25))
                .child(
                    v_flex()
                        .w_80()
                        .gap_3()
                        .p_4()
                        .rounded(cx.theme().radius_lg)
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().popover)
                        .shadow_lg()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(dialog.message),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    Button::new("dialog-cancel")
                                        .small()
                                        .outline()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.browser.update(cx, |browser, cx| {
                                                browser.answer_dialog(false, cx)
                                            })
                                        })),
                                )
                                .child(
                                    Button::new("dialog-ok")
                                        .small()
                                        .primary()
                                        .label("OK")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.browser.update(cx, |browser, cx| {
                                                browser.answer_dialog(true, cx)
                                            })
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let browser = self.browser.read(cx);
        let content: AnyElement = match browser.status() {
            BrowserStatus::Running if browser.frame().is_some() || !browser.url().is_empty() => return None,
            BrowserStatus::Running => EmptyState::new("Open a page")
                .icon(Icons::Globe)
                .description("Type an address above. Logins in this browser are kept for this workspace, and agents can use it too.")
                .into_any_element(),
            BrowserStatus::Starting | BrowserStatus::Stopped => h_flex()
                .gap_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(Spinner::new().small())
                .child("Starting the browser…")
                .into_any_element(),
            BrowserStatus::NoBrowser => EmptyState::new("No browser found")
                .icon(Icons::Globe)
                .description("PilotHouse drives a Chromium-family browser: Chrome, Edge, Brave or Chromium. Install one, or choose one you already have.")
                .action(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("browser-choose")
                                .small()
                                .primary()
                                .label("Choose a browser…")
                                .on_click(cx.listener(|this, _, window, cx| this.choose_browser(window, cx))),
                        )
                        .child(
                            Button::new("browser-get-chrome")
                                .small()
                                .outline()
                                .icon(Icons::ExternalLink)
                                .label("Get Chrome")
                                .tooltip("Opens Chrome's download page in your default browser")
                                .on_click(|_, _, cx| cx.open_url(crate::chrome::DOWNLOAD_PAGE)),
                        ),
                )
                .into_any_element(),
            BrowserStatus::Failed(message) => EmptyState::new("The browser could not start")
                .icon(IconName::TriangleAlert)
                .description(message.clone())
                .action(
                    Button::new("browser-retry")
                        .small()
                        .primary()
                        .label("Try again")
                        .on_click(cx.listener(|this, _, _, cx| this.browser.update(cx, |browser, cx| browser.ensure_started(cx)))),
                )
                .into_any_element(),
        };
        Some(
            div()
                .id("browser-overlay")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .child(content)
                .into_any_element(),
        )
    }

    fn render_drawer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let browser = self.browser.clone();
        let logs = &browser.read(cx).logs;
        let console_errors = logs
            .console
            .iter()
            .filter(|line| line.level == Level::Error)
            .count();
        let failed = logs
            .network
            .iter()
            .filter(|request| request.is_error())
            .count();
        let (console_count, network_count) = (logs.console.len(), logs.network.len());
        let tab = |id: &'static str,
                   label: &'static str,
                   count: usize,
                   errors: usize,
                   drawer: Drawer,
                   selected: bool,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .xsmall()
                .selected(selected)
                .label(if errors > 0 {
                    format!("{label} · {errors} ✕")
                } else if count > 0 {
                    format!("{label} · {count}")
                } else {
                    label.to_string()
                })
                .when(errors > 0, |this| this.text_color(cx.theme().danger))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.drawer = if this.drawer == Some(drawer) {
                        None
                    } else {
                        Some(drawer)
                    };
                    cx.notify();
                }))
        };
        let header = h_flex()
            .flex_none()
            .h_8()
            .px_2()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(tab(
                "drawer-console",
                "Console",
                console_count,
                console_errors,
                Drawer::Console,
                self.drawer == Some(Drawer::Console),
                cx,
            ))
            .child(tab(
                "drawer-network",
                "Network",
                network_count,
                failed,
                Drawer::Network,
                self.drawer == Some(Drawer::Network),
                cx,
            ))
            .child(div().flex_1())
            .when(self.drawer.is_some(), |this| {
                this.child(
                    Button::new("drawer-errors")
                        .ghost()
                        .xsmall()
                        .selected(self.errors_only)
                        .label("Errors only")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.errors_only = !this.errors_only;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("drawer-send")
                        .ghost()
                        .xsmall()
                        .icon(Icons::SendHorizontal)
                        .label("Send to chat")
                        .on_click(cx.listener(|this, _, _, cx| this.send_logs(cx))),
                )
                .child(
                    Button::new("drawer-clear")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Delete)
                        .tooltip("Clear")
                        .accessibility_label("Clear logs")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.browser.update(cx, |browser, cx| {
                                browser.logs.clear();
                                cx.notify();
                            })
                        })),
                )
            });

        let logs = &browser.read(cx).logs;
        let rows: Vec<(SharedString, Option<gpui_kit::Hsla>)> = match self.drawer {
            None => Vec::new(),
            Some(Drawer::Console) => logs
                .console
                .iter()
                .rev()
                .filter(|line| !self.errors_only || line.level == Level::Error)
                .take(DRAWER_ROWS)
                .map(|line| {
                    let color = match line.level {
                        Level::Error => Some(cx.theme().danger),
                        Level::Warning => Some(cx.theme().warning),
                        Level::Debug => Some(cx.theme().muted_foreground),
                        Level::Info => None,
                    };
                    (line.text.clone().into(), color)
                })
                .collect(),
            Some(Drawer::Network) => logs
                .network
                .iter()
                .rev()
                .filter(|request| !self.errors_only || request.is_error())
                .take(DRAWER_ROWS)
                .map(|request| {
                    (
                        request.summary().into(),
                        request.is_error().then(|| cx.theme().danger),
                    )
                })
                .collect(),
        };
        v_flex()
            .flex_none()
            .child(header)
            .when(self.drawer.is_some(), |this| {
                this.child(
                    v_flex()
                        .id("drawer-rows")
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
                                .border_b_1()
                                .border_color(cx.theme().border.opacity(0.5))
                                .when_some(color, |this, color| this.text_color(color))
                                .child(text)
                        })),
                )
            })
    }
}

/// Recent console errors and failed requests of a project's browser.
pub fn recent_logs(browser: &Entity<Browser>, cx: &gpui_kit::App) -> Option<String> {
    let browser = browser.read(cx);
    let console = browser.logs.console_text(40, false);
    let network = browser.logs.network_text(20, true);
    if console.is_empty() && network.is_empty() {
        return None;
    }
    let mut text = format!("Browser logs from {}:\n", browser.url());
    if !console.is_empty() {
        text.push_str(&format!("Console:\n```\n{console}\n```\n"));
    }
    if !network.is_empty() {
        text.push_str(&format!("Failed requests:\n```\n{network}\n```\n"));
    }
    Some(text)
}

impl Render for BrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_viewport(window, cx);
        let overlay = self.render_overlay(cx);
        let show_tools = overlay.is_none() && self.browser.read(cx).dialog().is_none();
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
                    .children(self.render_dialog(cx))
                    .when(show_tools, |this| {
                        this.child(div().absolute().inset_0().child(self.tools.clone()))
                    }),
            )
            .child(self.render_drawer(cx))
    }
}
