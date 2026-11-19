//! Settings → Chrome extension: letting agents use the user's own Chrome
//! through PilotHouse's extension ([`crate::extension`]). It shows whether
//! the extension is connected, where its folder is, how to load it, and
//! sets up (or removes) the native messaging host it talks to the app with.
//!
//! The way there is a guide in steps, each a sentence beside a drawing of
//! where in Chrome it happens ([`crate::extension_guide`]). Once Chrome is
//! connected the steps fold away, and what the extension does stays.

use std::collections::HashSet;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div,
};
use ph_ui::Icons;

use crate::extension::{self, Layout, Registration};
use crate::workflows::Workflow;
use crate::{AgentChrome, AgentChromeStatus, WorkflowUse};

pub struct ChromeExtensionView {
    registration: Registration,
    /// Whether the setup steps show, once the user has said: they show by
    /// themselves until Chrome is connected, and fold away then.
    steps_open: Option<bool>,
    /// What the last Set up or Remove said.
    message: Option<(bool, SharedString)>,
    /// Saved workflows, and the version they were read at: read again only
    /// when one is saved, renamed or deleted, not on each of the many
    /// notifications of the agents' tabs.
    workflows: Vec<Workflow>,
    workflows_version: u64,
    /// Workflows whose steps are shown.
    open_steps: HashSet<u64>,
    /// The workflow being renamed, and its new name.
    renaming: Option<(u64, Entity<InputState>)>,
    /// The workflow whose deletion waits for a yes.
    deleting: Option<u64>,
    workflow_error: Option<SharedString>,
    _chrome: Option<Subscription>,
}

impl ChromeExtensionView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Written now too, in case the folder was removed since the start.
        if let Err(err) = extension::write_extension() {
            tracing::warn!("writing PilotHouse's Chrome extension: {err:#}");
        }
        let chrome = AgentChrome::global(cx).map(|chrome| {
            cx.observe(&chrome, |this: &mut Self, chrome, cx| {
                let chrome = chrome.read(cx);
                if chrome.workflows_version() != this.workflows_version {
                    this.workflows_version = chrome.workflows_version();
                    this.workflows = chrome.workflows();
                }
                cx.notify();
            })
        });
        let (workflows, workflows_version) = AgentChrome::global(cx)
            .map(|chrome| {
                let chrome = chrome.read(cx);
                (chrome.workflows(), chrome.workflows_version())
            })
            .unwrap_or_default();
        Self {
            registration: registration(),
            steps_open: None,
            message: None,
            workflows,
            workflows_version,
            open_steps: HashSet::new(),
            renaming: None,
            deleting: None,
            workflow_error: None,
            _chrome: chrome,
        }
    }

    fn start_rename(&mut self, workflow: &Workflow, window: &mut Window, cx: &mut Context<Self>) {
        let name = workflow.name.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Workflow name");
            state.set_value(name, window, cx);
            state
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.renaming = Some((workflow.id, input));
        self.workflow_error = None;
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, input)) = self.renaming.take() else {
            return;
        };
        let name = input.read(cx).value().to_string();
        if let Some(chrome) = AgentChrome::global(cx)
            && let Err(err) = chrome.update(cx, |chrome, cx| chrome.rename_workflow(id, &name, cx))
        {
            self.workflow_error = Some(format!("{err:#}").into());
            self.renaming = Some((id, input));
        }
        cx.notify();
    }

    fn delete_workflow(&mut self, id: u64, cx: &mut Context<Self>) {
        self.deleting = None;
        if let Some(chrome) = AgentChrome::global(cx)
            && let Err(err) = chrome.update(cx, |chrome, cx| chrome.delete_workflow(id, cx))
        {
            self.workflow_error = Some(format!("{err:#}").into());
        }
        cx.notify();
    }

    fn use_workflow(
        &mut self,
        workflow: &Workflow,
        using: WorkflowUse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = AgentChrome::global(cx).and_then(|chrome| chrome.read(cx).host()) else {
            return;
        };
        // The chat or the schedule opens in place of Settings.
        window.close_dialog(cx);
        (host.use_workflow)(workflow.clone(), using, cx);
    }

    /// Saved workflows: their steps, a test or a schedule made of them,
    /// renaming and deleting.
    fn render_workflows(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mut list = v_flex().gap_2();
        if self.workflows.is_empty() {
            list = list.child(div().text_xs().text_color(muted).child(
                "None yet. In Chrome, click PilotHouse in the toolbar, then Record → A workflow; do the steps and press Stop and save.",
            ));
        }
        for workflow in &self.workflows {
            let id = workflow.id;
            let site = url_host(&workflow.start_url);
            let open = self.open_steps.contains(&id);
            let renaming = self
                .renaming
                .as_ref()
                .filter(|(renamed, _)| *renamed == id)
                .map(|(_, input)| input.clone());
            let title: AnyElement = match renaming {
                Some(input) => h_flex()
                    .gap_1()
                    .flex_1()
                    .child(div().flex_1().child(Input::new(&input).small()))
                    .child(
                        Button::new(("workflow-rename-save", id))
                            .small()
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.finish_rename(cx))),
                    )
                    .child(
                        Button::new(("workflow-rename-cancel", id))
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.renaming = None;
                                this.workflow_error = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
                None => v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(workflow.name.clone()),
                    )
                    .child(div().text_xs().text_color(muted).truncate().child(format!(
                        "{} steps · {site} · recorded {}",
                        workflow.steps.len(),
                        recorded_on(workflow.recorded_at)
                    )))
                    .into_any_element(),
            };
            let actions = if self.deleting == Some(id) {
                h_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.danger).child("Delete it?"))
                    .child(
                        Button::new(("workflow-delete-yes", id))
                            .small()
                            .danger()
                            .label("Delete")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.delete_workflow(id, cx)),
                            ),
                    )
                    .child(
                        Button::new(("workflow-delete-no", id))
                            .small()
                            .ghost()
                            .label("Keep")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.deleting = None;
                                cx.notify();
                            })),
                    )
            } else {
                let (test, schedule, rename) =
                    (workflow.clone(), workflow.clone(), workflow.clone());
                h_flex()
                    .gap_1()
                    .child(
                        Button::new(("workflow-steps", id))
                            .small()
                            .ghost()
                            .label(if open { "Hide steps" } else { "Steps" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.open_steps.remove(&id) {
                                    this.open_steps.insert(id);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("workflow-test", id))
                            .small()
                            .ghost()
                            .label("Make a test")
                            .tooltip("A chat drafts a Playwright test of it, for you to send")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.use_workflow(&test, WorkflowUse::Test, window, cx)
                            })),
                    )
                    .when(ph_brand::enabled(ph_brand::Feature::Schedules), |row| {
                        row.child(
                            Button::new(("workflow-schedule", id))
                                .small()
                                .ghost()
                                .label("Schedule")
                                .tooltip("A schedule runs it with an agent and reports what it saw")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.use_workflow(&schedule, WorkflowUse::Schedule, window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new(("workflow-rename", id))
                            .small()
                            .ghost()
                            .icon(Icons::Pencil)
                            .tooltip("Rename")
                            .accessibility_label("Rename")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_rename(&rename, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("workflow-delete", id))
                            .small()
                            .ghost()
                            .icon(Icons::Trash)
                            .tooltip("Delete")
                            .accessibility_label("Delete")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.deleting = Some(id);
                                cx.notify();
                            })),
                    )
            };
            list = list.child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .child(h_flex().gap_2().child(title).child(actions))
                    .when(open, |card| {
                        card.child(
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(muted)
                                .child(format!(
                                    "Starts at {}\n{}",
                                    workflow.start_url,
                                    workflow.describe()
                                )),
                        )
                    }),
            );
        }
        v_flex()
            .gap_1()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Workflows"))
            .child(div().text_xs().text_color(muted).child(
                "Steps you recorded in Chrome. Agents can run them in their tab (chrome_run_workflow), stopping where a password or a person is needed.",
            ))
            .child(list)
            .when_some(self.workflow_error.clone(), |this, error| {
                this.child(div().text_xs().text_color(theme.danger).child(error))
            })
            .into_any_element()
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

/// The address of Chrome's extensions page.
const EXTENSIONS_PAGE: &str = "chrome://extensions";

/// A step of the guide: its number (a tick once it is done), what to do in
/// a sentence, and a drawing of where. The drawing goes under the words
/// where the page is too narrow for both.
fn guide_step(
    number: Option<usize>,
    done: bool,
    title: impl Into<SharedString>,
    body: impl IntoElement,
    drawing: AnyElement,
    cx: &gpui_kit::App,
) -> gpui_kit::Div {
    let theme = cx.theme();
    let mark = match number {
        Some(_) if done => Some(
            gpui_kit::component::Icon::new(gpui_kit::component::IconName::CircleCheck)
                .small()
                .text_color(theme.success)
                .into_any_element(),
        ),
        Some(number) => Some(
            h_flex()
                .flex_none()
                .size(ph_ui::rems_from_px(18.))
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(number.to_string())
                .into_any_element(),
        ),
        None => None,
    };
    h_flex()
        .w_full()
        .flex_wrap()
        .items_start()
        .gap_x_4()
        .gap_y_2()
        .child(
            v_flex()
                .flex_1()
                .min_w(ph_ui::rems_from_px(240.))
                .gap_1p5()
                .child(
                    h_flex().gap_2().children(mark).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.into()),
                    ),
                )
                .child(body),
        )
        .child(drawing)
}

/// A sentence of the guide.
fn guide_text(text: impl Into<SharedString>, cx: &gpui_kit::App) -> gpui_kit::Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A shortcut of the extension's and what it does.
fn shortcut(key: &'static str, does: &'static str, cx: &gpui_kit::App) -> gpui_kit::Div {
    let theme = cx.theme();
    // Chrome's own names for the keys, as its shortcuts page shows them.
    let keys = if cfg!(target_os = "macos") {
        format!("⌥⇧{key}")
    } else {
        format!("Alt+Shift+{key}")
    };
    h_flex()
        .gap_2()
        .child(
            div()
                .flex_none()
                .min_w(ph_ui::rems_from_px(if cfg!(target_os = "macos") {
                    40.
                } else {
                    84.
                }))
                .px_1p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .child(keys),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(does),
        )
}

impl Render for ChromeExtensionView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let status = AgentChrome::global(cx)
            .map(|chrome| chrome.read(cx).status().clone())
            .unwrap_or(AgentChromeStatus::Disconnected);
        let set_up = !self.registration.browsers.is_empty();
        let connected = matches!(status, AgentChromeStatus::Connected { .. });
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
                "Waiting for Chrome: load the extension, and keep Chrome open".into(),
            ),
            (AgentChromeStatus::Disconnected, false) => (muted, "Not set up".into()),
        };
        // The steps are for getting there: once Chrome is connected they
        // fold away, unless asked for.
        let steps_open = self.steps_open.unwrap_or(!connected);
        let folder = extension::extension_dir();
        let folder_text = folder.display().to_string();

        let header = v_flex()
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
                    )
                    .when(connected, |this| {
                        this.child(
                            Button::new("chrome-extension-steps")
                                .xsmall()
                                .ghost()
                                .label(if steps_open {
                                    "Hide the setup steps"
                                } else {
                                    "Show the setup steps"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.steps_open = Some(!steps_open);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(guide_text(
                format!(
                    "Agents can use your own Chrome, with your sign-ins, but only the tabs in the PilotHouse tab group. Your other tabs are read only when you ask.{}",
                    if ph_brand::enabled(ph_brand::Feature::Browser) {
                        concat!(" Without the extension, agents use ", ph_brand::name!(), "'s built-in browser.")
                    } else {
                        ""
                    }
                ),
                cx,
            ));

        // 1. The native messaging host.
        let reach = v_flex()
            .gap_2()
            .child(guide_text(
                if set_up {
                    format!("Set up for {}.", self.registration.browsers.join(", "))
                } else {
                    format!(
                        "Registers {} with Chrome, Chromium, Edge and Brave for your user, so that only its extension can talk to the app. Nothing is installed in the browser.",
                        ph_brand::NAME
                    )
                },
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
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
                this.child(div().text_xs().text_color(theme.warning).child(concat!(
                    "Another ",
                    ph_brand::name!(),
                    " data folder is set up in a browser too: Set up here to use this one."
                )))
            })
            .when_some(self.message.clone(), |this, (ok, message)| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(if ok { theme.success } else { theme.danger })
                        .child(message),
                )
            });

        // 2. Developer mode.
        let developer = v_flex()
            .gap_2()
            .child(guide_text(
                "In Chrome, open chrome://extensions and switch on Developer mode, at the top right. In Edge and Brave it is edge://extensions and brave://extensions.",
                cx,
            ))
            .child(
                h_flex().child(
                    Button::new("chrome-extension-copy-address")
                        .small()
                        .outline()
                        .icon(Icons::Copy)
                        .label("Copy the address")
                        .on_click(|_, _, cx| {
                            ph_ui::copy_with_toast(
                                EXTENSIONS_PAGE.to_string(),
                                "the address",
                                cx,
                            )
                        }),
                ),
            );

        // 3. Load unpacked.
        let load = v_flex()
            .gap_2()
            .child(guide_text(
                "Click Load unpacked and choose this folder:",
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(folder_text.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("chrome-extension-copy")
                            .small()
                            .outline()
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
                            .on_click(move |_, _, cx| ph_ui::platform::reveal(&folder, cx)),
                    ),
            );

        let steps = v_flex()
            .gap_5()
            .child(guide_step(
                Some(1),
                set_up,
                concat!("Let Chrome reach ", ph_brand::name!()),
                reach,
                crate::extension_guide::link(cx),
                cx,
            ))
            .child(guide_step(
                Some(2),
                connected,
                "Turn on Developer mode",
                developer,
                crate::extension_guide::developer_mode(cx),
                cx,
            ))
            .child(guide_step(
                Some(3),
                connected,
                "Load the extension",
                load,
                crate::extension_guide::load_unpacked(cx),
                cx,
            ))
            .child(guide_step(
                Some(4),
                connected,
                "Pin it",
                guide_text(
                    "Open the puzzle-piece menu beside the address and pin PilotHouse, so that its button is always there.",
                    cx,
                ),
                crate::extension_guide::pin(cx),
                cx,
            ));

        let in_chrome = v_flex()
            .gap_5()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("In Chrome"),
            )
            .child(guide_step(
                None,
                false,
                "Everything is under its button",
                guide_text(
                    "Click PilotHouse in the toolbar to send the page, a screenshot or an element to a chat, to record a bug report or a workflow, and to let agents use the tab in front. Where what you send goes is chosen there too.",
                    cx,
                ),
                crate::extension_guide::popup(cx),
                cx,
            ))
            .child(guide_step(
                None,
                false,
                "Agents work in a tab group of their own",
                guide_text(
                    "When an agent opens a page, a purple PilotHouse tab group appears. A tab glows while an agent acts in it, and Chrome says that PilotHouse is debugging the browser. Agents never see a tab outside the group.",
                    cx,
                ),
                crate::extension_guide::tab_group(cx),
                cx,
            ))
            .child(guide_step(
                None,
                false,
                "Right-click what you want to send",
                guide_text(
                    "A page's menu sends what you point at: the selection, a link, an image or the page, a screenshot of an area, or an element.",
                    cx,
                ),
                crate::extension_guide::context_menu(cx),
                cx,
            ))
            .child(guide_step(
                None,
                false,
                "What you send waits for you",
                v_flex()
                    .gap_2()
                    .child(guide_text(
                        "It is put in the chat's message box and goes when you send it.",
                        cx,
                    ))
                    .child(
                        v_flex()
                            .gap_1()
                            .child(shortcut("P", "Send the selection, or the page", cx))
                            .child(shortcut("S", "Screenshot an area", cx))
                            .child(shortcut("E", "Pick an element", cx))
                            .child(shortcut("O", "Open PilotHouse's menu", cx)),
                    )
                    .child(div().text_xs().text_color(muted).child(
                        "The shortcuts are changed at chrome://extensions/shortcuts.",
                    )),
                crate::extension_guide::waiting(cx),
                cx,
            ));

        v_flex()
            .w_full()
            .gap_5()
            .child(header)
            .when(steps_open, |this| this.child(steps))
            .child(div().h(gpui_kit::px(1.)).bg(theme.border))
            .child(in_chrome)
            .child(self.render_workflows(cx))
            .child(div().text_xs().text_color(muted).child(concat!(
                "For agents to upload files, turn on “Allow access to file URLs” in the extension's Details. After ", ph_brand::name!(), " updates, click the reload arrow on the extension in chrome://extensions."),
            ))
    }
}

/// The site of an address, for a workflow's line.
fn url_host(url: &str) -> String {
    url.split_once("://")
        .map(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .unwrap_or(rest)
                .to_string()
        })
        .unwrap_or_else(|| url.to_string())
}

/// `12 Nov 2026` for a time in Unix milliseconds.
fn recorded_on(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_lines_read_well() {
        assert_eq!(recorded_on(0), "1 Jan 1970");
        assert_eq!(recorded_on(1_794_128_077_182), "8 Nov 2026");
        assert_eq!(recorded_on(951_782_400_000), "29 Feb 2000");
        assert_eq!(url_host("https://app.example.com/a?b"), "app.example.com");
        assert_eq!(url_host("not a url"), "not a url");
    }
}
