//! A session in its CLI form (kind `terminal`): the agent's own program,
//! shown in the chat's place while it runs. When it does not (the app was
//! restarted, the program ended), the agent's session file shows read only
//! with a way to go on in the CLI; the header's switch goes on in the chat.

use std::path::PathBuf;

use super::*;
use crate::cli_transcript::{FindJob, FindSession, Format, TranscriptView};

/// The closed terminal session on screen.
pub(super) struct TerminalSession {
    pub session: Session,
    pub agent: &'static AgentSpec,
    /// Its session file as a chat, for agents whose files PilotHouse reads.
    pub view: Option<Entity<TranscriptView>>,
}

impl ChatPanel {
    /// Shows `session`, an agent CLI's chat, read only.
    pub(super) fn show_terminal_session(&mut self, session: Session, cx: &mut Context<Self>) {
        let agent = agents::find(&session.agent_id).unwrap_or(&AGENTS[0]);
        let cwd = session
            .cwd
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.project.path.clone());
        let view = match (Format::of(agent.id), session.acp_session_id.clone()) {
            (Some(format), Some(id)) => {
                let (agent_id, folder) = (agent.id, cwd.clone());
                // Where its account keeps its files.
                let environment = crate::profiles::Profiles::load(&self.store)
                    .of(agent.id)
                    .environment_for(&session.account);
                // Found by its id among the agent's sessions in the folder.
                let find: FindSession = Box::new(move |_| {
                    let (id, folder) = (id.clone(), folder.clone());
                    let environment = environment.clone();
                    let job: FindJob = Box::new(move || {
                        crate::cli_sessions::sessions_in(agent_id, &folder, &environment)
                            .into_iter()
                            .find(|session| session.id == id)
                            .and_then(|session| session.path)
                    });
                    Some(job)
                });
                let project = ThreadProject {
                    path: cwd.clone(),
                    ..self.project.clone()
                };
                let store = self.store.clone();
                Some(cx.new(|cx| {
                    TranscriptView::in_project(store, project, agent, format, find, cx)
                        .without_bar()
                        .ended()
                }))
            }
            _ => None,
        };
        self.terminal_session = Some(TerminalSession {
            session,
            agent,
            view,
        });
    }

    /// A session in its CLI form: the agent's program when it runs here,
    /// else what its session file says (read only), with a way to go on.
    pub(super) fn render_cli(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(shown) = &self.terminal_session else {
            return div().into_any_element();
        };
        let header = self.render_header(window, cx).into_any_element();
        let running = self
            .cli_view
            .as_ref()
            .and_then(|view| view(shown.session.id, window, cx));
        if let Some(view) = running {
            return v_flex()
                .size_full()
                .child(header)
                .child(div().flex_1().min_h_0().child(view))
                .into_any_element();
        }
        let agent = shown.agent;
        let theme = cx.theme();
        let body = match &shown.view {
            Some(view) => div()
                .flex_1()
                .min_h_0()
                .child(view.clone())
                .into_any_element(),
            None => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .px_6()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "{} ran in its CLI; its conversation is in {}'s own files.",
                    agent.name, agent.name
                ))
                .when(shown.session.acp_session_id.is_none(), |this| {
                    this.child(format!(
                        "{} did not learn which of its sessions it was.",
                        ph_brand::NAME
                    ))
                })
                .into_any_element(),
        };
        // Nothing starts by itself: the CLI comes back when asked for.
        let go_on = self.cli_view.is_some().then(|| {
            h_flex()
                .flex_none()
                .px_3()
                .py_2()
                .gap_3()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{}'s CLI is not running.", agent.name)),
                )
                .child(
                    Button::new("cli-continue")
                        .primary()
                        .small()
                        .icon(Icons::Terminal)
                        .label(if shown.session.acp_session_id.is_some() {
                            "Continue"
                        } else {
                            "Start again"
                        })
                        .tooltip(ph_ui::tip(format!(
                            "Go on with this session in {}'s CLI",
                            agent.name
                        )))
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(ChatPanelEvent::ShowAs(Form::Cli))),
                        ),
                )
        });
        v_flex()
            .size_full()
            .child(header)
            .child(body)
            .children(go_on)
            .into_any_element()
    }
}
