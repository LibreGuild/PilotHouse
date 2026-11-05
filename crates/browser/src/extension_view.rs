//! Settings → Chrome extension: letting agents use the user's own Chrome
//! through PilotHouse's extension ([`crate::extension`]). It shows whether
//! the extension is connected, where its folder is, how to load it, and
//! sets up (or removes) the native messaging host it talks to the app with.

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, FontWeight, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div,
};
use ph_ui::Icons;

use crate::extension::{self, Layout, Registration};
use crate::{AgentChrome, AgentChromeStatus};

pub struct ChromeExtensionView {
    registration: Registration,
    /// What the last Set up or Remove said.
    message: Option<(bool, SharedString)>,
    _chrome: Option<Subscription>,
}

impl ChromeExtensionView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Written now too, in case the folder was removed since the start.
        if let Err(err) = extension::write_extension() {
            tracing::warn!("writing PilotHouse's Chrome extension: {err:#}");
        }
        let chrome =
            AgentChrome::global(cx).map(|chrome| cx.observe(&chrome, |_, _, cx| cx.notify()));
        Self {
            registration: registration(),
            message: None,
            _chrome: chrome,
        }
    }

    fn set_up(&mut self, cx: &mut Context<Self>) {
        self.message = Some(match Layout::current().and_then(|layout| extension::set_up(&layout)) {
            Ok(browsers) => (
                true,
                format!(
                    "Set up for {}. Load the extension (below) if you have not, then restart Chrome once.",
                    browsers.join(", ")
                )
                .into(),
            ),
            Err(err) => (false, format!("{err:#}").into()),
        });
        self.registration = registration();
        cx.notify();
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        self.message = Some(
            match Layout::current().and_then(|layout| extension::remove(&layout)) {
                Ok(()) => (
                    true,
                    "Removed. Remove the extension in chrome://extensions too, if you like.".into(),
                ),
                Err(err) => (false, format!("{err:#}").into()),
            },
        );
        self.registration = registration();
        cx.notify();
    }
}

fn registration() -> Registration {
    Layout::current()
        .map(|layout| extension::registration(&layout))
        .unwrap_or_default()
}

impl Render for ChromeExtensionView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let status = AgentChrome::global(cx)
            .map(|chrome| chrome.read(cx).status().clone())
            .unwrap_or(AgentChromeStatus::Disconnected);
        let set_up = !self.registration.browsers.is_empty();
        let (dot, headline): (gpui_kit::Hsla, String) = match (&status, set_up) {
            (AgentChromeStatus::Connected { browser }, _) => {
                (theme.success, format!("Connected to {browser}"))
            }
            (AgentChromeStatus::Failed(reason), _) => (
                theme.danger,
                format!("The extension cannot connect: {reason}"),
            ),
            (AgentChromeStatus::Disconnected, true) => (
                theme.warning,
                "Set up: waiting for Chrome (load the extension, and keep Chrome open)".into(),
            ),
            (AgentChromeStatus::Disconnected, false) => (muted, "Not set up".into()),
        };
        let folder = extension::extension_dir();
        let folder_text = folder.display().to_string();
        let step = |number: usize, text: &'static str| {
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .w_4()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{number}.")),
                )
                .child(div().flex_1().min_w_0().text_sm().child(text))
        };
        v_flex()
            .w_full()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().size_2().flex_none().rounded_full().bg(dot))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(headline),
                            ),
                    )
                    .child(div().text_xs().text_color(muted).child(
                        "Agents can use your own Chrome, with your sign-ins, but only the tabs in its PilotHouse tab group: tabs they open, and tabs you put in the group (or share with the PilotHouse button in Chrome's toolbar). Your other tabs are never listed, read or touched. Without the extension, agents use PilotHouse's built-in browser.",
                    )),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("1. Let Chrome reach PilotHouse"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().text_xs().text_color(muted).child(if set_up {
                                format!("Set up for {}.", self.registration.browsers.join(", "))
                            } else {
                                "Registers PilotHouse with Chrome, Chromium, Edge and Brave for your user, so that only its extension can talk to the app. Nothing is installed in the browser.".to_string()
                            }))
                            .child(
                                Button::new("chrome-extension-set-up")
                                    .small()
                                    .when(set_up, |button| button.outline())
                                    .when(!set_up, |button| button.primary())
                                    .label(if set_up { "Set up again" } else { "Set up" })
                                    .on_click(cx.listener(|this, _, _, cx| this.set_up(cx))),
                            )
                            .when(set_up, |row| {
                                row.child(
                                    Button::new("chrome-extension-remove")
                                        .small()
                                        .ghost()
                                        .label("Remove")
                                        .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                                )
                            }),
                    )
                    .when(self.registration.elsewhere, |this| {
                        this.child(div().text_xs().text_color(theme.warning).child(
                            "Another PilotHouse data folder is set up in a browser too: Set up here to use this one.",
                        ))
                    })
                    .when_some(self.message.clone(), |this, (ok, message)| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(if ok { theme.success } else { theme.danger })
                                .child(message),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("2. Load the extension in Chrome"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(muted)
                                    .child(folder_text.clone()),
                            )
                            .child(
                                Button::new("chrome-extension-copy")
                                    .small()
                                    .ghost()
                                    .icon(Icons::Copy)
                                    .label("Copy")
                                    .on_click(move |_, _, cx| {
                                        ph_ui::copy_with_toast(
                                            folder_text.clone(),
                                            "the extension's folder",
                                            cx,
                                        )
                                    }),
                            )
                            .child(
                                Button::new("chrome-extension-reveal")
                                    .small()
                                    .ghost()
                                    .icon(Icons::FolderOpen)
                                    .label("Reveal")
                                    .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                            ),
                    )
                    .child(step(1, "Open chrome://extensions in Chrome (or edge://extensions, brave://extensions)."))
                    .child(step(2, "Turn on Developer mode (top right)."))
                    .child(step(3, "Click Load unpacked and choose the folder above."))
                    .child(step(4, "Pin PilotHouse from the puzzle-piece menu: its button shares the tab in front with agents, or stops sharing it."))
                    .child(step(5, "When an agent opens a page, a purple PilotHouse tab group appears. While an agent acts, its tab glows; Chrome also shows that PilotHouse is debugging the browser.")),
            )
            .child(div().text_xs().text_color(muted).child(
                "For agents to upload files, turn on “Allow access to file URLs” in the extension's Details. After PilotHouse updates, click the reload arrow on the extension in chrome://extensions.",
            ))
    }
}
