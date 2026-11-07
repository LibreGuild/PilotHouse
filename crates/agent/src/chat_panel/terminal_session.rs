//! A chat that was an agent CLI in a terminal (kind `terminal`), its
//! terminal closed: the agent's own session file shown read only, and ways
//! to go on: in a terminal again (the CLI resumes its session), or in a new
//! chat that starts from a recap of it.

use std::path::PathBuf;

use gpui_kit::component::menu::PopupMenuItem;

use super::*;
use crate::cli_transcript::{FindJob, FindSession, Format, TranscriptView};

/// The closed terminal session on screen.
pub(super) struct TerminalSession {
    pub session: Session,
    pub agent: &'static AgentSpec,
    /// Where the agent worked.
    pub cwd: PathBuf,
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
                // Found by its id among the agent's sessions in the folder.
                let find: FindSession = Box::new(move |_| {
                    let (id, folder) = (id.clone(), folder.clone());
                    let job: FindJob = Box::new(move || {
                        crate::cli_sessions::sessions(agent_id, &folder)
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
                }))
            }
            _ => None,
        };
        self.terminal_session = Some(TerminalSession {
            session,
            agent,
            cwd,
            view,
        });
    }

    /// The closed terminal session: its title, its transcript, and what can
    /// be done next.
    pub(super) fn render_terminal_session(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(shown) = &self.terminal_session else {
            return div().into_any_element();
        };
        let agent = shown.agent;
        let theme = cx.theme();
        let header = h_flex()
            .flex_none()
            .h(px(40.))
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(agent.mark(px(16.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(shown.session.title.clone()),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(Icons::Terminal).xsmall())
                    .child("Terminal session · read only"),
            );
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
                    "{} ran in a terminal; its conversation is in {}'s own files.",
                    agent.name, agent.name
                ))
                .when(shown.session.acp_session_id.is_none(), |this| {
                    this.child("PilotHouse did not learn which of its sessions it was.")
                })
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(header)
            .child(body)
            .child(self.render_terminal_actions(shown, cx))
            .into_any_element()
    }

    /// Go on in a terminal (the CLI resumes the session, or starts anew
    /// when it is not known), or in a new chat.
    fn render_terminal_actions(
        &self,
        shown: &TerminalSession,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent = shown.agent;
        let session = shown.session.acp_session_id.clone();
        let panel = cx.weak_entity();
        let resume = {
            let (panel, session, cwd) = (panel.clone(), session.clone(), shown.cwd.clone());
            Button::new("terminal-session-resume")
                .primary()
                .small()
                .icon(Icons::Terminal)
                .label(if session.is_some() {
                    "Continue in terminal"
                } else {
                    "Start again in a terminal"
                })
                .tooltip(format!("{} in a terminal of this workspace", agent.name))
                .on_click(move |_, _, cx| {
                    _ = panel.update(cx, |_, cx| {
                        cx.emit(ChatPanelEvent::ResumeInTerminal {
                            agent: agent.id,
                            session: session.clone(),
                            cwd: cwd.clone(),
                        })
                    });
                })
        };
        // From a recap of what the transcript shows, with any agent.
        let in_chat = shown.view.clone().map(|view| {
            let panel = panel.clone();
            Button::new("terminal-session-chat")
                .small()
                .icon(Icons::MessageSquare)
                .label("Continue in a new chat")
                .tooltip("A new chat that starts from this session, with any agent")
                .menu(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                    AGENTS.iter().fold(menu, |menu, agent| {
                        let (panel, view) = (panel.clone(), view.clone());
                        menu.item(PopupMenuItem::new(agent.name).on_click(move |_, _, cx| {
                            let Some((recap, title)) = view.read(cx).recap(cx) else {
                                return;
                            };
                            _ = panel.update(cx, |_, cx| {
                                cx.emit(ChatPanelEvent::ContinueInChat {
                                    agent,
                                    recap,
                                    title,
                                })
                            });
                        }))
                    })
                })
        });
        // No transcript to recap: the agent takes its session back itself,
        // where it can.
        let load = (shown.view.is_none() && !agent.quirks.no_session_load)
            .then_some(session)
            .flatten()
            .map(|session| {
                let title = shown.session.title.clone();
                Button::new("terminal-session-load")
                    .small()
                    .icon(Icons::MessageSquare)
                    .label("Continue in a new chat")
                    .tooltip(format!("{} takes the session back in a chat", agent.name))
                    .on_click(move |_, _, cx| {
                        _ = panel.update(cx, |_, cx| {
                            cx.emit(ChatPanelEvent::ResumeInChat {
                                agent: agent.id,
                                session: session.clone(),
                                title: title.clone(),
                            })
                        });
                    })
            });
        h_flex()
            .flex_none()
            .px_3()
            .py_2()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(resume)
            .children(in_chat)
            .children(load)
            .into_any_element()
    }
}
