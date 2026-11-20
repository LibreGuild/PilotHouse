//! A new session that is to start in an agent's CLI: which agent's.
//!
//! Where new sessions open as a CLI (Settings → Agents), *New chat* used
//! to start the CLI of the agent last used, at once. Whoever wanted
//! another agent had to switch to the chat, pick it there and switch back,
//! with a session started for nothing behind them. So the agent is asked
//! first: the installed agents that have a CLI, the one last used leading,
//! each of its accounts on a row of its own. A pick starts that CLI; the
//! header's switch shows the chat instead, where nothing has started.
//!
//! Nothing starts until one is picked. With one agent and one account
//! there is nothing to pick, and whoever shows the panel starts it.

use super::*;

/// An agent's CLI in one of its accounts (empty: the agent's own), as a
/// new session can start in.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Choice {
    pub agent: &'static AgentSpec,
    pub account: String,
    /// The account's name, for an agent that has more than its own.
    pub account_name: Option<String>,
}

/// What a new CLI session can start in: every installed agent that has a
/// CLI (all of them while what is installed is not known yet), `first`
/// leading, each with its accounts.
pub(super) fn choices(
    first: &'static AgentSpec,
    installed: Option<Vec<&'static AgentSpec>>,
    profiles: &crate::profiles::Profiles,
) -> Vec<Choice> {
    let mut found: Vec<&'static AgentSpec> = installed
        .unwrap_or_else(|| AGENTS.iter().collect())
        .into_iter()
        .filter(|agent| agents::has_cli(agent.id))
        .collect();
    // The one new sessions start with leads; the rest keep their order.
    found.sort_by_key(|agent| agent.id != first.id);
    found
        .into_iter()
        .flat_map(|agent| {
            let profile = profiles.of(agent.id);
            let named = !profile.accounts.is_empty();
            std::iter::once(Choice {
                agent,
                account: String::new(),
                account_name: named.then(|| "Default".to_string()),
            })
            .chain(profile.accounts.iter().map(move |account| Choice {
                agent,
                account: account.id.clone(),
                account_name: Some(account.name.clone()),
            }))
            .collect::<Vec<_>>()
        })
        .collect()
}

impl ChatPanel {
    /// Whether sessions have their CLI form in this panel (a workspace's
    /// chat with the terminal feature; not a review's chat, a read-only
    /// place or the quick chat).
    pub(super) fn has_cli_form(&self) -> bool {
        self.cli_view.is_some()
            && self.context.is_none()
            && !self.project.read_only
            && ph_brand::enabled(ph_brand::Feature::Terminal)
    }

    fn cli_choices(&self, cx: &App) -> Vec<Choice> {
        choices(
            self.active.read(cx).agent(),
            agents::installed(),
            &crate::profiles::Profiles::known(),
        )
    }

    /// The new chat in front is to start in a CLI: asks which agent's.
    /// `false` when there is nothing to ask (one agent, one account, or
    /// no CLI form here): whoever asked decides then.
    pub fn ask_cli_agent(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.has_cli_form() || self.cli_choices(cx).len() < 2 {
            return false;
        }
        self.cli_pick = true;
        cx.notify();
        true
    }

    /// Whether the agent of a new CLI session is being asked.
    pub fn asks_cli_agent(&self) -> bool {
        self.cli_pick
    }

    /// The new chat in front is `agent`'s, in `account`: what was picked
    /// for a new CLI session, before its CLI starts.
    pub fn start_with(&mut self, agent: &'static AgentSpec, account: &str, cx: &mut Context<Self>) {
        if self.active.read(cx).agent().id != agent.id {
            // One warmed up already cannot change its agent: a new one.
            if self.active.read(cx).has_started() {
                let _ = self.store.set_setting(DEFAULT_AGENT_KEY, &agent.id);
                self.set_session(None, cx);
            } else {
                self.choose_agent(agent, cx);
            }
        }
        self.cli_pick = false;
        self.active
            .update(cx, |thread, cx| thread.set_account(account, cx));
        cx.notify();
    }

    /// In place of the chat: the agents a new CLI session can start in.
    pub(super) fn render_cli_pick(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = self.render_header(window, cx).into_any_element();
        let theme = cx.theme();
        let folder = self
            .project
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.project.name.clone());
        let rows: Vec<AnyElement> = self
            .cli_choices(cx)
            .into_iter()
            .map(|choice| {
                let Choice {
                    agent,
                    account,
                    account_name,
                } = choice;
                let id = match account.is_empty() {
                    true => format!("cli-pick-{}", agent.id),
                    false => format!("cli-pick-{}-{account}", agent.id),
                };
                let said = match &account_name {
                    Some(name) => format!("{} · {name}", agent.name),
                    None => agent.name.to_string(),
                };
                h_flex()
                    .id(SharedString::from(id))
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(SharedString::from(said))
                    .h_10()
                    .px_3()
                    .gap_3()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_sm()
                    .hover(|style| style.bg(theme.list_hover))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ChatPanelEvent::StartCli {
                            agent: agent.id,
                            account: account.clone(),
                        });
                    }))
                    .child(
                        div()
                            .flex_none()
                            .child(agent.mark(ph_ui::rems_from_px(18.))),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(agent.name))
                    .children(account_name.map(|name| {
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(name)
                    }))
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    )
                    .test_support()
                    .into_any_element()
            })
            .collect();
        v_flex()
            .size_full()
            .child(header)
            .child(
                super::render::centered_scroll("cli-pick").child(
                    v_flex()
                        .my_auto()
                        .w_full()
                        .max_w(ph_ui::rems_from_px(420.))
                        .gap_4()
                        .child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                                        .child("Which agent?"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_center()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("Its CLI starts in {folder}.")),
                                ),
                        )
                        .child(v_flex().gap_1p5().children(rows))
                        .child(
                            div()
                                .text_xs()
                                .text_center()
                                .text_color(theme.muted_foreground)
                                .child(
                                    "New sessions open in the CLI (Settings → Agents). For a chat, switch to Chat above.",
                                ),
                        ),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{Account, Profile, Profiles};

    fn agent(id: &str) -> &'static AgentSpec {
        agents::find(id).unwrap()
    }

    #[test]
    fn the_agent_last_used_leads_and_each_account_is_a_choice() {
        let installed = vec![agent("claude"), agent("codex"), agent("gemini")];
        let plain = choices(
            agent("codex"),
            Some(installed.clone()),
            &Profiles::default(),
        );
        assert_eq!(
            plain
                .iter()
                .map(|choice| (choice.agent.id, choice.account.as_str()))
                .collect::<Vec<_>>(),
            vec![("codex", ""), ("claude", ""), ("gemini", "")],
            "the one last used first, the rest in their order"
        );
        // An agent's own account needs no name while it is the only one.
        assert!(plain.iter().all(|choice| choice.account_name.is_none()));

        let mut profiles = Profiles::default();
        let mut claude = Profile::default();
        claude.accounts.push(Account {
            id: "work".into(),
            name: "Work".into(),
            environment: Default::default(),
        });
        profiles.agents.insert("claude".into(), claude);
        let with_accounts = choices(agent("codex"), Some(installed), &profiles);
        assert_eq!(
            with_accounts
                .iter()
                .map(|choice| (choice.agent.id, choice.account_name.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("codex", None),
                ("claude", Some("Default")),
                ("claude", Some("Work")),
                ("gemini", None),
            ]
        );
        // Not looked for yet: every agent that has a CLI is offered.
        let unknown = choices(agent("claude"), None, &Profiles::default());
        assert_eq!(unknown[0].agent.id, "claude");
        assert!(unknown.len() > 2);
        assert!(
            unknown
                .iter()
                .all(|choice| agents::has_cli(choice.agent.id))
        );
        // One agent, one account: nothing to ask.
        assert_eq!(
            choices(
                agent("codex"),
                Some(vec![agent("codex")]),
                &Profiles::default()
            )
            .len(),
            1
        );
    }
}
