//! A session's other form: its agent's own program (its CLI), shown in the
//! chat's place. A chat and the CLI go on with the same conversation, the
//! agent's session, so going from one to the other changes what is typed
//! into and nothing else.
//!
//! What keeps that true:
//!
//! - **One program at a time.** A chat hands its conversation over at rest
//!   and ends its agent session (`ChatThread::hand_over`); coming back, the
//!   CLI's terminal is closed before the chat loads the session again.
//! - **Never on a guess.** The CLI goes on with the session the chat had,
//!   by its id. Coming back needs the id the CLI is on; while that is not
//!   known, the switch waits and says so.
//! - **Nothing said is lost.** The chat keeps how much the agent's session
//!   file held as it left (a mark); what the file holds after it was said
//!   in the CLI, and is added to the chat's transcript when it comes back.
//!
//! The CLI's terminal belongs to the chat: it is among the workspace's
//! agent terminals (`TerminalHub::agent_terminals`), not the Terminal
//! tab's, and is followed like any agent CLI (`terminal_agents`), bound to
//! the chat's row so the Bridge and the tabs show one session.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};
use gpui_kit::{AnyWindowHandle, EntityId, Focusable as _, Subscription, Task, WeakEntity};
use ph_agent::cli_transcript::{self, Record};
use ph_agent::{ChatState, CliView, Form};
use ph_store::Store;
use ph_store::projects::ProjectId;
use ph_store::sessions::{Session, SessionId};
use ph_terminal::{Terminal, TerminalHub, TerminalView, TerminalViewEvent};

use super::*;
use crate::terminal_agents::{self, Launch, TerminalAgents};

/// The setting that keeps, for each session now in its CLI form, how many
/// steps its agent's session file held as it went there (by session id).
const MARKS: &str = "chat.cli_marks";

/// The views of the agent programs this window's chat shows, by terminal.
/// A view holds its terminal, so one is let go when its terminal closes.
pub(super) type CliViews = Rc<RefCell<HashMap<EntityId, (Entity<TerminalView>, Subscription)>>>;

/// The terminal of the agent program session `session` shows as, while it
/// is open: one of the workspace's agent terminals, bound to the session.
fn cli_terminal(project: ProjectId, session: SessionId, cx: &App) -> Option<Entity<Terminal>> {
    let terminal = TerminalAgents::of_session(session, cx)?
        .terminal
        .upgrade()?;
    TerminalHub::existing_agent_terminals(project, cx)?
        .read(cx)
        .terminals()
        .contains(&terminal)
        .then_some(terminal)
}

/// Whether `terminal` is a chat's (shown in the chat, not in the Terminal
/// tab), and which chat's.
pub(crate) fn chat_of(
    project: ProjectId,
    terminal: &Entity<Terminal>,
    cx: &App,
) -> Option<SessionId> {
    TerminalHub::existing_agent_terminals(project, cx)?
        .read(cx)
        .terminals()
        .contains(terminal)
        .then(|| TerminalAgents::of_terminal(terminal, cx)?.session)
        .flatten()
}

fn mark_of(store: &Store, session: SessionId) -> usize {
    store
        .setting::<HashMap<String, usize>>(MARKS)
        .get(&session.to_string())
        .copied()
        .unwrap_or(0)
}

fn set_mark(store: &Store, session: SessionId, mark: Option<usize>) {
    let mut marks: HashMap<String, usize> = store.setting(MARKS);
    match mark {
        Some(mark) => marks.insert(session.to_string(), mark),
        None => marks.remove(&session.to_string()),
    };
    if let Err(err) = store.set_setting(MARKS, &marks) {
        tracing::warn!("keeping a session's mark: {err:#}");
    }
}

impl WorkspaceScreen {
    /// What the chat panel asks for the CLI of the session it shows: the
    /// view of its terminal, made the first time, with the keyboard in it
    /// (it was just asked for).
    pub(super) fn cli_view(
        project: ProjectId,
        screen: WeakEntity<Self>,
        views: CliViews,
        handle: AnyWindowHandle,
    ) -> CliView {
        Rc::new(move |session, window, cx| {
            let terminal = cli_terminal(project, session, cx)?;
            let id = terminal.entity_id();
            if let Some((view, _)) = views.borrow().get(&id) {
                return Some(view.clone().into());
            }
            let view = cx.new(|cx| TerminalView::new(terminal.clone(), cx));
            let screen = screen.clone();
            let subscription = cx.subscribe(&view, move |view, event: &TerminalViewEvent, cx| {
                // "Ask about this" is for a terminal beside the chat;
                // here the agent would be asked about itself.
                let TerminalViewEvent::Open(link) = event else {
                    return;
                };
                let cwd = view.read(cx).terminal().read(cx).cwd().to_path_buf();
                let (screen, link) = (screen.clone(), link.clone());
                _ = handle.update(cx, |_, window, cx| {
                    _ = screen.update(cx, |this, cx| this.open_link(&link, Some(&cwd), window, cx));
                });
            });
            view.read(cx).focus_handle(cx).focus(window, cx);
            views.borrow_mut().insert(id, (view.clone(), subscription));
            Some(view.into())
        })
    }

    /// The chat panel's switch: the session in front goes on in its other
    /// form. A switch that cannot be made now says why and changes nothing.
    pub(super) fn show_as(&mut self, form: Form, window: &mut Window, cx: &mut Context<Self>) {
        if !ph_brand::enabled(ph_brand::Feature::Terminal) {
            return;
        }
        let shown = self.chat.read(cx).form();
        let done = match (shown, form) {
            (Form::Chat, Form::Cli) => self.chat_to_cli(window, cx),
            (Form::Cli, Form::Cli) => self.continue_cli(window, cx),
            (Form::Cli, Form::Chat) => self.cli_to_chat(false, window, cx),
            (Form::Chat, Form::Chat) => Ok(()),
        };
        if let Err(why) = done {
            use gpui_kit::component::WindowExt as _;
            window.push_notification(format!("{why:#}"), cx);
        }
        cx.notify();
    }

    /// The chat in front goes on in its agent's CLI: once the agent's side
    /// has let go of the conversation, and not before.
    fn chat_to_cli(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        let store = AppState::store(cx).clone();
        let thread = self.chat.read(cx).active_thread().clone();
        let (handover, released) = thread
            .update(cx, |thread, cx| thread.hand_over(cx))
            .map_err(|why| anyhow!("{why}"))?;
        let agent = handover.agent;
        // A chat nothing was sent in has no row yet: its CLI gets one,
        // named by the first thing asked there.
        let (session, named) = match handover.session.clone() {
            Some(session) => (session, true),
            None => (
                store.create_session(
                    self.project.id,
                    &format!("{} session", agent.name),
                    agent.id,
                )?,
                false,
            ),
        };
        let cwd = handover.cwd.to_string_lossy().into_owned();
        store.set_session_place(session.id, "terminal", Some(&cwd))?;
        if let Some(resume) = &handover.resume {
            store.set_session_acp_id(session.id, Some(resume))?;
        }
        // The mark: what the agent's record holds now was said in the chat
        // (and is in its transcript); what comes after, in the CLI. Read
        // off the UI thread, before the CLI is there to add to it.
        let mark = match handover.resume.clone() {
            Some(resume) => {
                let folder = handover.cwd.clone();
                let agent_id = agent.id;
                let environment = ph_agent::profiles::Profiles::load(&store)
                    .of(agent.id)
                    .environment_for(&handover.account);
                cx.background_spawn(async move {
                    Record {
                        agent: agent_id,
                        id: &resume,
                        cwd: &folder,
                        environment: &environment,
                        file: None,
                    }
                    .step_count()
                    // A record that cannot be read: all that can be read
                    // of it later is taken as new.
                    .unwrap_or(0)
                })
            }
            // A session of its own starts: all of its record is new.
            None => Task::ready(0),
        };
        // Read from the store again when it comes back, with what the CLI
        // added.
        let chats = self.chat.read(cx).chats().clone();
        chats.update(cx, |chats, cx| chats.forget(session.id, cx));
        // Nothing is sent in the chat meanwhile: that would take the
        // conversation up again under the CLI about to start.
        self.chat.update(cx, |chat, cx| {
            chat.set_busy(Some(format!("Opening {}'s CLI…", agent.name)), cx)
        });
        cx.spawn_in(window, async move |this, cx| {
            let mark = mark.await;
            // Never two programs on one conversation: the CLI starts once
            // the agent's side said it let go (its process ended, or its
            // session closed).
            released.await;
            _ = this.update_in(cx, |this, window, cx| {
                set_mark(&store, session.id, Some(mark));
                this.chat.update(cx, |chat, cx| chat.set_busy(None, cx));
                let started = this.start_cli(
                    &session,
                    handover.cwd,
                    handover.resume,
                    named,
                    (handover.model, handover.mode),
                    cx,
                );
                // Shown as it now is either way: one that did not start
                // says so and offers to go on.
                this.show_session_again(session.id, cx);
                if let Err(why) = started {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(format!("{why:#}"), cx);
                }
            });
        })
        .detach();
        Ok(())
    }

    /// A new session starts in the CLI of the agent (and account of it)
    /// picked for it.
    pub(super) fn start_new_cli(
        &mut self,
        agent: &str,
        account: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = ph_agent::agents::find(agent) else {
            return;
        };
        self.chat
            .update(cx, |chat, cx| chat.start_with(agent, account, cx));
        self.show_as(Form::Cli, window, cx);
    }

    /// A session in its CLI form whose program is not running goes on:
    /// the CLI starts again and resumes it.
    fn continue_cli(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        let session = self
            .chat
            .read(cx)
            .shown_session(cx)
            .ok_or_else(|| anyhow!("There is no session to go on with."))?;
        if cli_terminal(self.project.id, session.id, cx).is_some() {
            return Ok(());
        }
        // Going on already in a terminal of the Terminal tab (started
        // there by hand): that one, not a second program on it.
        let elsewhere = session.acp_session_id.as_deref().and_then(|id| {
            TerminalAgents::of_cli_session(&session.agent_id, id, cx)
                .and_then(|running| running.terminal.upgrade())
        });
        if let Some(terminal) = elsewhere {
            self.show_terminal(&terminal, window, cx);
            return Ok(());
        }
        let cwd = session
            .cwd
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.project.path.clone());
        let resume = session.acp_session_id.clone();
        self.start_cli(&session, cwd, resume, true, (None, None), cx)
    }

    /// The session in front, in its CLI form, goes on in the chat.
    /// `looked`: the agent's own session was just looked for.
    fn cli_to_chat(
        &mut self,
        looked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let session = self
            .chat
            .read(cx)
            .shown_session(cx)
            .ok_or_else(|| anyhow!("There is no session to go on with."))?;
        let agent = ph_agent::agents::find(&session.agent_id)
            .ok_or_else(|| anyhow!("{} is not a known agent", session.agent_id))?;
        let running = TerminalAgents::of_session(session.id, cx).map(|running| {
            (
                running.state,
                // Something happened there: it works, waits, or said
                // something (its name alone may be the chat's own).
                running.state != ChatState::Idle || running.line.is_some(),
                running.session_id.clone(),
                running.transcript.clone(),
            )
        });
        let (state, asked, cli_id, file) = running.unwrap_or((ChatState::Idle, false, None, None));
        match state {
            ChatState::Working => bail!(
                "{} is at work in its CLI. Wait for it, or stop it there, then switch to the chat.",
                agent.name
            ),
            // A question, a permission, a sign-in: answering is done
            // there, and closing it now would drop what it is in.
            ChatState::NeedsYou => bail!(
                "{} waits for you in its CLI. Answer it there, or leave the CLI, then switch to the chat.",
                agent.name
            ),
            _ => {}
        }
        let cli_id = cli_id.or_else(|| session.acp_session_id.clone());
        // Never on a guess: something was said there, and which of the
        // agent's sessions holds it is not known yet (an agent without
        // hooks says so only in its files, which are looked at every so
        // often). Looked for now, and the switch goes on once it is
        // known, rather than asking for a second press a moment later.
        if cli_id.is_none() && asked {
            if looked {
                bail!(
                    "{} has not written this session down yet, so what was said in its CLI cannot be read. Try again once it has answered.",
                    agent.name
                );
            }
            let found = crate::terminal_agents::find_session_now(session.id, cx);
            cx.spawn_in(window, async move |this, cx| {
                found.await;
                _ = this.update_in(cx, |this, window, cx| {
                    // Still the session in front, in its CLI: it goes on
                    // (or says why not, now for good).
                    let front = this.chat.read(cx).shown_session(cx).map(|shown| shown.id);
                    if front == Some(session.id) && this.chat.read(cx).form() == Form::Cli {
                        if let Err(why) = this.cli_to_chat(true, window, cx) {
                            use gpui_kit::component::WindowExt as _;
                            window.push_notification(format!("{why:#}"), cx);
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
            return Ok(());
        }
        // What is known of it by now is kept first (the name the first
        // thing asked gave it, its own session): the look that keeps it
        // every second may not have come by since.
        crate::terminal_agents::save_now(cx);
        // The program ends first: never two on one conversation.
        self.close_cli(session.id, cx);
        let store = AppState::store(cx).clone();
        let mark = mark_of(&store, session.id);
        let cwd = session
            .cwd
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.project.path.clone());
        let environment = ph_agent::profiles::Profiles::load(&store)
            .of(agent.id)
            .environment_for(&session.account);
        let taken = {
            let (store, id, cli_id, agent_id) =
                (store.clone(), session.id, cli_id.clone(), agent.id);
            cx.background_spawn(async move {
                // Nothing was said there: there is nothing to take in.
                let Some(cli_id) = cli_id else {
                    return Ok(Some(0));
                };
                Record {
                    agent: agent_id,
                    id: &cli_id,
                    cwd: &cwd,
                    environment: &environment,
                    file,
                }
                .take_in(&store, id, mark)
            })
        };
        cx.spawn_in(window, async move |this, cx| {
            let taken = taken.await;
            _ = this.update_in(cx, |this, window, cx| {
                // Something was said in the CLI and is not in the chat
                // now: said, with what is known of why, since the chat
                // would otherwise look as if nothing had been.
                let name = agent.name;
                let missing = match &taken {
                    // Its record was read and holds nothing new.
                    Ok(Some(0)) if asked => Some(format!(
                        "{name}'s own record of this session holds nothing new, so what was said in its CLI is not in this chat. If {name} kept no record of it (its transcript saving was off), it will not remember it either."
                    )),
                    // Its record cannot be read here: the agent has it.
                    Ok(None) if asked => Some(format!(
                        "What was said in {name}'s CLI cannot be shown in this chat: {name}'s own record of it could not be read. {name} still knows it."
                    )),
                    Err(err) => {
                        tracing::warn!("reading a session's CLI part into its chat: {err:#}");
                        Some(format!(
                            "What was said in {name}'s CLI could not be read into this chat. {name} still knows it."
                        ))
                    }
                    _ => None,
                };
                if let Some(missing) = missing {
                    let _ = cli_transcript::note(&store, session.id, &missing);
                }
                set_mark(&store, session.id, None);
                let _ = store.set_session_place(session.id, "chat", session.cwd.as_deref());
                if let Some(cli_id) = &cli_id {
                    let _ = store.set_session_acp_id(session.id, Some(cli_id));
                }
                let chats = this.chat.read(cx).chats().clone();
                chats.update(cx, |chats, cx| chats.forget(session.id, cx));
                this.show_session_again(session.id, cx);
                this.chat
                    .update(cx, |chat, cx| chat.focus_composer(window, cx));
            });
        })
        .detach();
        Ok(())
    }

    /// Starts `session`'s agent CLI among the workspace's agent terminals,
    /// bound to the session. `named`: the session has its name already.
    fn start_cli(
        &mut self,
        session: &Session,
        cwd: PathBuf,
        resume: Option<String>,
        named: bool,
        (model, mode): (Option<String>, Option<String>),
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let terminals =
            TerminalHub::agent_terminals(self.project.id, self.project.path.clone(), cx);
        // A CLI that ends (the user left it, it failed to start) closes
        // its terminal: its view is let go, and the chat shows that it is
        // not running.
        if self._cli_watch.is_none() {
            self._cli_watch = Some(cx.observe(&terminals, |this, terminals, cx| {
                let open: Vec<EntityId> = terminals
                    .read(cx)
                    .terminals()
                    .iter()
                    .map(Entity::entity_id)
                    .collect();
                this.cli_views
                    .borrow_mut()
                    .retain(|terminal, _| open.contains(terminal));
                this.chat.update(cx, |_, cx| cx.notify());
                cx.emit(WorkspaceScreenEvent::SessionsChanged);
                cx.notify();
            }));
        }
        terminal_agents::launch(
            Launch {
                agent_id: &session.agent_id,
                project: self.project.id,
                cwd,
                resume: resume.as_deref(),
                chat: Some((session.id, named)),
                model: model.as_deref(),
                mode: mode.as_deref(),
            },
            &terminals,
            cx,
        )?;
        cx.notify();
        Ok(())
    }

    /// Types `text` into the CLI of the session in front, for the user to
    /// send there (it is pasted, not entered). A session whose CLI is not
    /// running says so: nothing is typed into nowhere.
    pub(super) fn type_in_cli(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self
            .chat
            .read(cx)
            .active_session_id(cx)
            .and_then(|session| cli_terminal(self.project.id, session, cx));
        let Some(terminal) = terminal else {
            use gpui_kit::component::WindowExt as _;
            window.push_notification(
                "This session's CLI is not running. Continue it, or switch to the chat, then send that again.",
                cx,
            );
            return;
        };
        terminal.read(cx).paste_text(text);
        if let Some((view, _)) = self.cli_views.borrow().get(&terminal.entity_id()) {
            view.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// Closes the CLI of `session` (its program ends).
    pub(crate) fn close_cli(&mut self, session: SessionId, cx: &mut Context<Self>) {
        let Some(terminal) = cli_terminal(self.project.id, session, cx) else {
            return;
        };
        self.cli_views.borrow_mut().remove(&terminal.entity_id());
        if let Some(terminals) = TerminalHub::existing_agent_terminals(self.project.id, cx) {
            terminals.update(cx, |terminals, cx| terminals.close(&terminal, cx));
        }
    }

    /// Shows session `id` as it is stored now (its form changed).
    fn show_session_again(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.sessions = AppState::store(cx)
            .sessions(self.project.id)
            .unwrap_or_default()
            .into();
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned();
        self.active_session = Some(id);
        self.chat
            .update(cx, |chat, cx| chat.set_session(session, cx));
        self.refresh_evidence(cx);
        self.save_layout(cx);
        cx.emit(WorkspaceScreenEvent::SessionsChanged);
        cx.notify();
    }
}
