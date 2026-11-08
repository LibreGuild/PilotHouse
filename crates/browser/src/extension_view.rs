//! Settings → Chrome extension: letting agents use the user's own Chrome
//! through PilotHouse's extension ([`crate::extension`]). It shows whether
//! the extension is connected, where its folder is, how to load it, and
//! sets up (or removes) the native messaging host it talks to the app with.

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
    /// What the last Set up or Remove said.
    message: Option<(bool, SharedString)>,
    /// Saved workflows (read again when the extension saves one).
    workflows: Vec<Workflow>,
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
                this.workflows = chrome.read(cx).workflows();
                cx.notify();
            })
        });
        let workflows = AgentChrome::global(cx)
            .map(|chrome| chrome.read(cx).workflows())
            .unwrap_or_default();
        Self {
            registration: registration(),
            message: None,
            workflows,
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
                "None yet. In Chrome, right-click a page → PilotHouse → Record a workflow, do the steps, then press Stop and save.",
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
                                .label("Schedule…")
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
                        "Agents can use your own Chrome, with your sign-ins, but only the tabs in its PilotHouse tab group: tabs they open, and tabs you put in the group (in the PilotHouse panel: “Let agents use this tab”). And you can send pages, screenshots, elements and bug reports from Chrome to a chat. Your other tabs are read only when you ask. Without the extension, agents use PilotHouse's built-in browser.",
                    )),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(concat!("1. Let Chrome reach ", ph_brand::name!())),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().text_xs().text_color(muted).child(if set_up {
                                format!("Set up for {}.", self.registration.browsers.join(", "))
                            } else {
                                format!("Registers {} with Chrome, Chromium, Edge and Brave for your user, so that only its extension can talk to the app. Nothing is installed in the browser.", ph_brand::NAME)
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
                            concat!("Another ", ph_brand::name!(), " data folder is set up in a browser too: Set up here to use this one."),
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
                    .child(step(4, "Pin PilotHouse from the puzzle-piece menu: its button opens the PilotHouse panel (what waits for you, agents in Chrome, sharing the tab, sending from it, recording)."))
                    .child(step(5, "When an agent opens a page, a purple PilotHouse tab group appears. While an agent acts, its tab glows; Chrome also shows that PilotHouse is debugging the browser.")),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("In Chrome"),
                    )
                    .child(step(1, "Right-click a page → PilotHouse: send the selection, the page, a link, an image, an area or an element to a chat; record a bug report or a workflow."))
                    .child(step(2, "Shortcuts: ⌥⇧P (Alt+Shift+P) sends the selection or the page, ⌥⇧S an area, ⌥⇧E an element, ⌥⇧O opens the panel. Change them at chrome://extensions/shortcuts."))
                    .child(step(3, "What you send waits in the chat's message box (the chat on screen, a new chat or quick chat: the panel's “Send to”) until you send it.")),
            )
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
