//! Settings → Agents: how the agents on this computer are run.
//!
//! At the top, how a new session opens (as a chat, or as the agent's own
//! CLI). Then the agents found here, each with what it is started by and
//! its state; one opens to what can be set for it: its command, what is
//! added to it, its environment and its accounts ([`crate::profiles`]).
//! Last, the agents not found, each with the command that installs it.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use ph_store::Store;
use ph_ui::MenuButton as _;

use crate::agents::{AGENTS, AgentSpec};
use crate::profiles::{self, OpenAs, Profiles};
use crate::setup::{self, CheckState};

/// Runs an agent's install command where the user sees it run (a terminal).
/// Whoever shows the view has the terminals; without one the command can
/// still be copied.
pub type Install = Rc<dyn Fn(&'static AgentSpec, &mut Window, &mut App)>;

pub struct AgentsView {
    store: Store,
    profiles: Profiles,
    install: Option<Install>,
    /// The agent whose settings are open (one at a time).
    open: Option<&'static str>,
    command: Entity<InputState>,
    arguments: Entity<InputState>,
    environment: Entity<TextareaState>,
    account_name: Entity<InputState>,
    account_environment: Entity<InputState>,
    /// Why the last change was not kept.
    error: Option<SharedString>,
}

impl AgentsView {
    pub fn new(
        store: Store,
        install: Option<Install>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        Self {
            profiles: Profiles::load(&store),
            store,
            install,
            open: None,
            command: input("The agent's own command", window, cx),
            arguments: input("Added to every start, e.g. --verbose", window, cx),
            environment: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("NAME=value, one a line")
                    .rows(2)
            }),
            account_name: input("Name, e.g. Work", window, cx),
            account_environment: input(
                "What it is started with, e.g. CLAUDE_CONFIG_DIR=~/.claude-work",
                window,
                cx,
            ),
            error: None,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        self.error = self
            .profiles
            .save(&self.store)
            .err()
            .map(|err| format!("Could not save: {err:#}").into());
        cx.notify();
    }

    /// Opens agent `id`'s settings (or closes them), its fields as saved.
    fn toggle(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == Some(id) {
            self.open = None;
            return cx.notify();
        }
        self.open = Some(id);
        let profile = self.profiles.of(id);
        let usual = crate::agents::cli_of(id).unwrap_or_default();
        self.command.update(cx, |input, cx| {
            input.set_placeholder(usual, window, cx);
            input.set_value(profile.command.clone(), window, cx)
        });
        self.arguments.update(cx, |input, cx| {
            input.set_value(arguments_text(&profile.arguments), window, cx)
        });
        self.environment.update(cx, |input, cx| {
            input.set_value(profiles::environment_text(&profile.environment), window, cx)
        });
        cx.notify();
    }

    /// Keeps what the open agent's fields say.
    fn keep_fields(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.open else {
            return;
        };
        let command = self.command.read(cx).value().trim().to_string();
        let arguments = profiles::words(&self.arguments.read(cx).value());
        let environment = profiles::parse_environment(&self.environment.read(cx).value());
        self.profiles.update(id, |profile| {
            profile.command = command;
            profile.arguments = arguments;
            profile.environment = environment;
        });
        self.save(cx);
    }

    fn add_account(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.account_name.read(cx).value().trim().to_string();
        let environment = profiles::parse_environment(&self.account_environment.read(cx).value());
        if name.is_empty() {
            self.error = Some("Give the account a name.".into());
            return cx.notify();
        }
        // An account that sets nothing would be the agent's own again.
        if environment.is_empty() {
            self.error = Some(
                "Say what the account is started with, as NAME=value (the folder it keeps its sign-in in)."
                    .into(),
            );
            return cx.notify();
        }
        self.profiles.update(id, |profile| {
            let account = profile.add_account(&name);
            if let Some(added) = profile
                .accounts
                .iter_mut()
                .find(|known| known.id == account)
            {
                added.environment = environment;
            }
        });
        for input in [&self.account_name, &self.account_environment] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.save(cx);
    }

    fn heading(
        text: impl Into<SharedString>,
        note: impl Into<SharedString>,
        cx: &App,
    ) -> AnyElement {
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(text.into()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(note.into()),
            )
            .into_any_element()
    }

    /// How a new session opens, for every agent.
    fn render_open_as(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_3()
            .child(div().flex_1().min_w_0().child(Self::heading(
                "Open agents as",
                "How a new session opens: in the chat, or in the agent's own CLI. Either way it is one conversation, and Chat | CLI in a session's header goes to the other.",
                cx,
            )))
            .child(
                TabBar::new("agents-open-as")
                    .segmented()
                    .small()
                    .flex_none()
                    .selected_index(usize::from(self.profiles.open_as == OpenAs::Cli))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.profiles.open_as = if *ix == 1 { OpenAs::Cli } else { OpenAs::Chat };
                        this.save(cx);
                    }))
                    .child(Tab::new().label("Chat"))
                    .child(Tab::new().label("CLI")),
            )
            .into_any_element()
    }

    /// An agent found on this computer: what it is started by, its state,
    /// how its sessions open, and (opened) what can be set for it.
    fn render_installed(
        &self,
        spec: &'static AgentSpec,
        ready: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let profile = self.profiles.of(spec.id);
        let open = self.open == Some(spec.id);
        let usual = crate::agents::cli_of(spec.id).unwrap_or(spec.cli);
        let started_by = match profile.command.trim() {
            "" => usual.to_string(),
            command => command.to_string(),
        };
        let started_by = std::iter::once(started_by)
            .chain(profile.arguments.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let opens = match profile.open_as {
            None => "As all agents",
            Some(OpenAs::Chat) => "Chat",
            Some(OpenAs::Cli) => "CLI",
        };
        let id = spec.id;
        let view = cx.weak_entity();
        let current = profile.open_as;
        let line = h_flex()
            .id(SharedString::from(format!("agent-row-{id}")))
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(SharedString::from(format!("{} settings", spec.name)))
            .w_full()
            .gap_2()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(id, window, cx)))
            .child(spec.mark(ph_ui::rems_from_px(16.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().child(spec.name))
                    .child(
                        div()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(started_by),
                    ),
            )
            .when(!profile.accounts.is_empty(), |this| {
                let accounts = profile.accounts.len() + 1;
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{accounts} accounts")),
                )
            })
            .child(
                div()
                    .flex_none()
                    // Its own click, not the row's.
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation()
                    })
                    .child(
                        Button::new(SharedString::from(format!("agent-opens-{id}")))
                            .xsmall()
                            .outline()
                            .label(opens)
                            .tooltip("How a new session with this agent opens")
                            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                                [
                                    ("As all agents", None),
                                    ("Chat", Some(OpenAs::Chat)),
                                    ("CLI", Some(OpenAs::Cli)),
                                ]
                                .into_iter()
                                .fold(
                                    menu,
                                    |menu, (label, choice)| {
                                        let view = view.clone();
                                        menu.item(
                                            PopupMenuItem::new(label)
                                                .checked(choice == current)
                                                .on_click(move |_, _, cx| {
                                                    _ = view.update(cx, |this, cx| {
                                                        this.profiles.update(id, |profile| {
                                                            profile.open_as = choice
                                                        });
                                                        this.save(cx);
                                                    });
                                                }),
                                        )
                                    },
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1p5()
                    .child(
                        div()
                            .size(ph_ui::rems_from_px(6.))
                            .rounded_full()
                            .bg(if ready {
                                cx.theme().success
                            } else {
                                cx.theme().warning
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if ready { "Ready" } else { "Needs setting up" }),
                    ),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            );
        v_flex()
            .w_full()
            .gap_3()
            .px_3()
            .py_2p5()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(line)
            .when(open, |this| {
                this.child(self.render_settings(spec, ready, cx))
            })
            .into_any_element()
    }

    /// What can be set for the open agent.
    fn render_settings(
        &self,
        spec: &'static AgentSpec,
        ready: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = spec.id;
        let profile = self.profiles.of(id);
        let label = |text: &'static str, note: &'static str, cx: &App| {
            v_flex()
                .w(ph_ui::rems_from_px(150.))
                .flex_none()
                .child(div().text_xs().child(text))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
        };
        let field = |text: &'static str, note: &'static str, input: AnyElement, cx: &App| {
            h_flex()
                .items_start()
                .gap_3()
                .child(label(text, note, cx))
                .child(div().flex_1().min_w_0().child(input))
        };
        let accounts = profile.accounts.iter().map(|account| {
            let account_id = account.id.clone();
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(ph_ui::rems_from_px(110.))
                        .flex_none()
                        .truncate()
                        .text_xs()
                        .child(account.name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            profiles::environment_text(&account.environment).replace('\n', "  "),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("agent-account-remove-{id}-{account_id}")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip(ph_ui::tip("Remove the account here (its sign-in and sessions stay where the agent keeps them)"))
                        .accessibility_label(format!("Remove the account {}", account.name))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.profiles.update(id, |profile| {
                                profile.accounts.retain(|known| known.id != account_id)
                            });
                            this.save(cx);
                        })),
                )
        });
        v_flex()
            .gap_3()
            // An agent not ready says what it needs first.
            .when(!ready, |this| {
                this.child(crate::setup_view::card(
                    spec,
                    &setup::checks(spec),
                    None,
                    cx,
                ))
            })
            .child(field(
                "Command",
                "Its CLI, when not the usual one",
                Input::new(&self.command).small().into_any_element(),
                cx,
            ))
            .child(field(
                "Arguments",
                "Added when its CLI starts",
                Input::new(&self.arguments).small().into_any_element(),
                cx,
            ))
            .child(field(
                "Environment",
                "Set for it, as a chat and as its CLI",
                Textarea::new(&self.environment).small().into_any_element(),
                cx,
            ))
            .child(
                h_flex().justify_end().child(
                    Button::new(SharedString::from(format!("agent-save-{id}")))
                        .small()
                        .primary()
                        .label("Save")
                        .on_click(cx.listener(|this, _, _, cx| this.keep_fields(cx))),
                ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(Self::heading(
                        "Accounts",
                        format!(
                            "A second sign-in of {}: what it is started with, usually the folder it keeps its sign-in and sessions in. You pick the account when a session starts, and a session stays in it.",
                            spec.name
                        ),
                        cx,
                    ))
                    .children(accounts)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .w(ph_ui::rems_from_px(140.))
                                    .flex_none()
                                    .child(Input::new(&self.account_name).small()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.account_environment).small()),
                            )
                            .child(
                                Button::new(SharedString::from(format!("agent-account-add-{id}")))
                                    .small()
                                    .outline()
                                    .label("Add account")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.add_account(id, window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// An agent not found: the command that installs it, to run where it
    /// can be watched, or to copy.
    fn render_missing(&self, spec: &'static AgentSpec, cx: &mut Context<Self>) -> AnyElement {
        let command = spec.install_command();
        let install = self.install.clone();
        h_flex()
            .w_full()
            .gap_2()
            .px_3()
            .py_2p5()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(spec.mark(ph_ui::rems_from_px(16.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().child(spec.name))
                    .child(
                        div()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(command),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "agent-install-copy-{}",
                    spec.id
                )))
                .ghost()
                .xsmall()
                .icon(IconName::Copy)
                .tooltip("Copy the command")
                .accessibility_label(format!("Copy the command that installs {}", spec.name))
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()))
                }),
            )
            .when_some(install, |this, install| {
                this.child(
                    Button::new(SharedString::from(format!("agent-install-{}", spec.id)))
                        .small()
                        .outline()
                        .label("Install")
                        .tooltip("Runs the command in a terminal, where you see it")
                        .on_click(move |_, window, cx| install(spec, window, cx)),
                )
            })
            .into_any_element()
    }
}

/// Arguments as text to edit, each written so that Save reads it back as
/// it is: one with spaces or quote characters in quotes
/// ([`profiles::quoted`]).
fn arguments_text(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| profiles::quoted(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Render for AgentsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // What was found is remembered for a few seconds (`setup::checks`).
        let found: Vec<(&'static AgentSpec, bool, bool)> = AGENTS
            .iter()
            .map(|spec| {
                let checks = setup::checks(spec);
                let installed = checks
                    .first()
                    .is_some_and(|check| check.state == CheckState::Ok);
                (spec, installed, setup::ready(&checks))
            })
            .collect();
        let installed: Vec<AnyElement> = found
            .iter()
            .filter(|(_, installed, _)| *installed)
            .map(|(spec, _, ready)| self.render_installed(spec, *ready, cx))
            .collect();
        let missing: Vec<AnyElement> = found
            .iter()
            .filter(|(_, installed, _)| !*installed)
            .map(|(spec, _, _)| self.render_missing(spec, cx))
            .collect();
        let count = |text: &'static str, n: usize, cx: &App| {
            h_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(text),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(n.to_string()),
                )
        };
        v_flex()
            .w_full()
            .gap_4()
            .child(self.render_open_as(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
            .when(!installed.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(count("Installed", installed.len(), cx))
                        .children(installed),
                )
            })
            .when(!missing.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(count("Not installed", missing.len(), cx))
                        .children(missing),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_shown_as_they_were_typed() {
        let arguments = vec![
            "--model".to_string(),
            "opus".to_string(),
            "be brief".to_string(),
        ];
        let text = arguments_text(&arguments);
        assert_eq!(text, "--model opus \"be brief\"");
        assert_eq!(profiles::words(&text), arguments);
        // Opening an agent's settings and saving them changes nothing, also
        // for an argument that holds quote characters, or is empty.
        let quoting = vec![
            "--append-system-prompt".to_string(),
            "say \"hi\" now".to_string(),
            r#"{"x":"y"}"#.to_string(),
            r#"it's "both""#.to_string(),
            String::new(),
        ];
        assert_eq!(profiles::words(&arguments_text(&quoting)), quoting);
    }
}
