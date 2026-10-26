//! The Terminal tab: one or more shells in the workspace's folders, each the
//! shell the user picks (zsh, bash, PowerShell…). The shells belong to the
//! workspace ([`WorkspaceTerminals`]); this panel only shows them, with one
//! [`TerminalView`] per shell made when first shown.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::Selectable as _;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel},
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, EntityId, Focusable as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
    px,
};
use ph_ui::MenuButton as _;
use ph_ui::{EmptyState, Icons};

use crate::terminal::Terminal;
use crate::terminal_view::{TerminalView, TerminalViewEvent};
use crate::workspace_terminals::WorkspaceTerminals;

#[derive(Debug, Clone)]
pub enum TerminalPanelEvent {
    /// Ask the agent about `text` from the terminal named `terminal`.
    Ask { terminal: String, text: String },
    /// Start a coding agent's CLI (by agent id) in a new terminal in `cwd`,
    /// with PilotHouse following what it does.
    StartAgent { agent: &'static str, cwd: PathBuf },
    /// Pick one of the agent's past sessions in `cwd` to resume.
    ResumeAgent { agent: &'static str, cwd: PathBuf },
    /// Open a link ⌘-clicked in a terminal's output; `cwd` is where its
    /// shell started (a relative path is looked for there first).
    Open {
        link: crate::links::Link,
        cwd: PathBuf,
    },
    /// Show or hide the session of the agent in `terminal` beside it.
    ToggleTranscript(Entity<Terminal>),
}

/// Whether a terminal has an agent session to show beside it.
pub type HasTranscript = Rc<dyn Fn(&Entity<Terminal>, &App) -> bool>;

impl gpui_kit::EventEmitter<TerminalPanelEvent> for TerminalPanel {}

pub struct TerminalPanel {
    terminals: Entity<WorkspaceTerminals>,
    /// A view per shell shown so far.
    views: HashMap<EntityId, (Entity<TerminalView>, Subscription)>,
    /// Agent sessions shown beside their terminal, by terminal.
    transcripts: HashMap<EntityId, AnyView>,
    has_transcript: Option<HasTranscript>,
    _observe: Subscription,
}

impl TerminalPanel {
    /// Shows the workspace's `terminals`.
    pub fn new(terminals: Entity<WorkspaceTerminals>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&terminals, |this, terminals, cx| {
                // Views of closed shells go.
                let open: Vec<EntityId> = terminals
                    .read(cx)
                    .terminals()
                    .iter()
                    .map(Entity::entity_id)
                    .collect();
                this.views.retain(|id, _| open.contains(id));
                this.transcripts.retain(|id, _| open.contains(id));
                cx.notify();
            }),
            terminals,
            views: HashMap::new(),
            transcripts: HashMap::new(),
            has_transcript: None,
        }
    }

    /// Offers *Transcript* on the terminals `check` passes.
    pub fn set_has_transcript(&mut self, check: HasTranscript) {
        self.has_transcript = Some(check);
    }

    /// Shows `view` beside `terminal` (`None`: hides it).
    pub fn set_transcript(
        &mut self,
        terminal: EntityId,
        view: Option<AnyView>,
        cx: &mut Context<Self>,
    ) {
        match view {
            Some(view) => self.transcripts.insert(terminal, view),
            None => self.transcripts.remove(&terminal),
        };
        cx.notify();
    }

    pub fn shows_transcript(&self, terminal: EntityId) -> bool {
        self.transcripts.contains_key(&terminal)
    }

    /// The *Transcript* toggle, for an agent's terminal.
    fn render_transcript_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let terminal = self.terminals.read(cx).active().cloned()?;
        let shown = self.shows_transcript(terminal.entity_id());
        if !shown
            && !self
                .has_transcript
                .as_ref()
                .is_some_and(|check| check(&terminal, cx))
        {
            return None;
        }
        Some(
            Button::new("terminal-transcript")
                .ghost()
                .xsmall()
                .icon(Icons::MessageSquare)
                .selected(shown)
                .tooltip(if shown {
                    "Hide the agent's transcript"
                } else {
                    "Show the agent's transcript beside it"
                })
                .accessibility_label("Transcript")
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(TerminalPanelEvent::ToggleTranscript(terminal.clone()))
                }))
                .into_any_element(),
        )
    }

    pub fn terminals(&self) -> &Entity<WorkspaceTerminals> {
        &self.terminals
    }

    /// The view of `terminal`, made the first time.
    fn view(
        &mut self,
        terminal: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        if let Some((view, _)) = self.views.get(&terminal.entity_id()) {
            return view.clone();
        }
        let view = cx.new(|cx| TerminalView::new(terminal.clone(), cx));
        let subscription = cx.subscribe(&view, |_, view, event: &TerminalViewEvent, cx| {
            let text = match event {
                TerminalViewEvent::Ask(text) => text,
                TerminalViewEvent::Open(link) => {
                    let cwd = view.read(cx).terminal().read(cx).cwd().to_path_buf();
                    return cx.emit(TerminalPanelEvent::Open {
                        link: link.clone(),
                        cwd,
                    });
                }
            };
            // A run's name says what it is; a shell's title is noise.
            let name = view
                .read(cx)
                .terminal()
                .read(cx)
                .label()
                .map_or_else(|| "Terminal output".to_string(), |label| label.to_string());
            cx.emit(TerminalPanelEvent::Ask {
                terminal: name,
                text: text.clone(),
            })
        });
        self.views
            .insert(terminal.entity_id(), (view.clone(), subscription));
        view
    }

    /// Puts the keyboard in `terminal`.
    pub fn focus(
        &mut self,
        terminal: &Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Looking at it is seeing its news.
        terminal.update(cx, |terminal, cx| terminal.acknowledge(cx));
        let view = self.view(terminal, cx);
        view.read(cx).focus_handle(cx).focus(window, cx);
    }

    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self
            .terminals
            .update(cx, |terminals, cx| terminals.new_terminal(cx))
        {
            self.focus(&terminal, window, cx);
        }
    }

    /// A new shell in folder `cwd`.
    pub fn new_terminal_in(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(terminal) = self
            .terminals
            .update(cx, |terminals, cx| terminals.new_terminal_in(cwd, cx))
        {
            self.focus(&terminal, window, cx);
        }
    }

    /// Runs `command` in the terminal named `label` (see
    /// [`WorkspaceTerminals::run_in`]) and puts the keyboard there.
    pub fn run_in(
        &mut self,
        label: &str,
        command: &str,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.terminals.update(cx, |terminals, cx| {
            terminals.run_in(label, command, cwd, cx)
        }) {
            self.focus(&terminal, window, cx);
        }
    }

    /// A new `shell` in folder `cwd`.
    fn new_shell(
        &mut self,
        cwd: PathBuf,
        shell: Option<ph_env::Shell>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.terminals.update(cx, |terminals, cx| {
            terminals.new_terminal_with(cwd, shell.as_ref(), cx)
        }) {
            self.focus(&terminal, window, cx);
        }
    }

    /// "+": the shells installed here (the default first), each in the
    /// primary folder or, with several folders, in the one picked from its
    /// submenu. A folder on a network drive gets the server's shell, so
    /// there it simply opens one.
    fn render_new_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("new-terminal")
            .ghost()
            .xsmall()
            .mr_2()
            .icon(Icons::Plus)
            .tooltip("New terminal")
            .accessibility_label("New terminal");
        let terminals = self.terminals.read(cx);
        let (cwd, folders) = (terminals.cwd().to_path_buf(), terminals.folders().to_vec());
        if ph_remote::remote_of(&cwd).is_some() {
            return button
                .on_click(cx.listener(|this, _, window, cx| this.new_terminal(window, cx)))
                .into_any_element();
        }
        let panel = cx.weak_entity();
        button
            .menu(gpui_kit::Anchor::TopRight, move |menu, window, cx| {
                // Looked up as it opens: a shell installed meanwhile shows.
                let shells = ph_env::shells();
                let agent_panel = panel.clone();
                let agent_cwd = cwd.clone();
                let menu = shells
                    .into_iter()
                    .fold(menu.label("New terminal"), |menu, shell| {
                        let label = if shell.default {
                            format!("{} (default)", shell.name)
                        } else {
                            shell.name.clone()
                        };
                        if folders.len() < 2 {
                            let (panel, cwd) = (panel.clone(), cwd.clone());
                            return menu.item(PopupMenuItem::new(label).on_click(
                                move |_, window, cx| {
                                    _ = panel.update(cx, |panel, cx| {
                                        panel.new_shell(
                                            cwd.clone(),
                                            Some(shell.clone()),
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            ));
                        }
                        let (panel, folders) = (panel.clone(), folders.clone());
                        let submenu = PopupMenu::build(window, cx, move |menu, _, _| {
                            folders
                                .iter()
                                .fold(menu.label("In folder"), |menu, (name, path)| {
                                    let (panel, path, shell) =
                                        (panel.clone(), path.clone(), shell.clone());
                                    menu.item(PopupMenuItem::new(name.clone()).on_click(
                                        move |_, window, cx| {
                                            _ = panel.update(cx, |panel, cx| {
                                                panel.new_shell(
                                                    path.clone(),
                                                    Some(shell.clone()),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ))
                                })
                        });
                        menu.item(PopupMenuItem::submenu(label, submenu))
                    });
                // Agents whose CLI PilotHouse follows in a terminal.
                const AGENTS: [(&str, &str); 4] = [
                    ("claude", "Claude Code"),
                    ("codex", "Codex"),
                    ("gemini", "Gemini CLI"),
                    ("antigravity", "Antigravity"),
                ];
                let menu = AGENTS.iter().fold(
                    menu.separator().label("Agent in a terminal"),
                    |menu, (agent, name)| {
                        let (panel, cwd) = (agent_panel.clone(), agent_cwd.clone());
                        menu.item(PopupMenuItem::new(*name).on_click(move |_, _, cx| {
                            _ = panel.update(cx, |_, cx| {
                                cx.emit(TerminalPanelEvent::StartAgent {
                                    agent,
                                    cwd: cwd.clone(),
                                })
                            });
                        }))
                    },
                );
                let resume = {
                    let (panel, cwd) = (agent_panel.clone(), agent_cwd.clone());
                    PopupMenu::build(window, cx, move |menu, _, _| {
                        AGENTS.iter().fold(menu, |menu, (agent, name)| {
                            let (panel, cwd) = (panel.clone(), cwd.clone());
                            menu.item(PopupMenuItem::new(*name).on_click(move |_, _, cx| {
                                _ = panel.update(cx, |_, cx| {
                                    cx.emit(TerminalPanelEvent::ResumeAgent {
                                        agent,
                                        cwd: cwd.clone(),
                                    })
                                });
                            }))
                        })
                    })
                };
                menu.item(PopupMenuItem::submenu("Resume a session…", resume))
            })
            .into_any_element()
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(terminal) = self.terminals.read(cx).active().cloned() {
            let view = self.view(&terminal, cx);
            let Some(transcript) = self.transcripts.get(&terminal.entity_id()).cloned() else {
                return view.into_any_element();
            };
            // Side by side; the divider drags.
            return h_resizable(SharedString::from(format!(
                "terminal-transcript-split-{}",
                terminal.entity_id()
            )))
            .child(
                resizable_panel()
                    .size_range(px(160.)..gpui_kit::Pixels::MAX)
                    .child(div().size_full().child(view)),
            )
            .child(
                resizable_panel()
                    .size(px(260.))
                    .size_range(px(200.)..gpui_kit::Pixels::MAX)
                    .child(
                        div()
                            .size_full()
                            .bg(cx.theme().background)
                            .child(transcript),
                    ),
            )
            .into_any_element();
        }
        let description = self
            .terminals
            .read(cx)
            .error()
            .cloned()
            .unwrap_or_else(|| "Open a shell in the workspace's folder.".into());
        EmptyState::new("No terminal")
            .icon(Icons::Terminal)
            .description(description)
            .action(
                Button::new("open-terminal")
                    .outline()
                    .small()
                    .icon(Icons::Plus)
                    .label("New terminal")
                    .on_click(cx.listener(|this, _, window, cx| this.new_terminal(window, cx))),
            )
            .into_any_element()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminals = self.terminals.read(cx);
        let active = terminals.active_index();
        let titles: Vec<(SharedString, Option<crate::AgentBadge>)> = terminals
            .terminals()
            .iter()
            .map(|terminal| {
                let terminal = terminal.read(cx);
                let title = short_title(
                    terminal.title(),
                    terminal.shell(),
                    terminal.label().map(|label| label.as_ref()),
                );
                (title, terminal.badge())
            })
            .collect();
        let tabs = titles.into_iter().enumerate().map(|(ix, (title, badge))| {
            Tab::new()
                .label(title)
                .when_some(badge, |tab, badge| tab.prefix(badge_glyph(badge, cx)))
                .suffix(
                    Button::new(("close-terminal", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .accessibility_label("Close terminal")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.terminals
                                .update(cx, |terminals, cx| terminals.close_index(ix, cx))
                        })),
                )
        });
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .pl_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .min_w_0()
                    .child(
                        // Shells set long titles (user@host:path); the tabs
                        // give way so the buttons stay visible.
                        div().flex_1().min_w_0().overflow_hidden().child(
                            TabBar::new("terminal-tabs")
                                .underline()
                                .small()
                                .selected_index(active)
                                .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                    let ix = *ix;
                                    this.terminals
                                        .update(cx, |terminals, cx| terminals.select(ix, cx));
                                    let terminal = this.terminals.read(cx).active().cloned();
                                    if let Some(terminal) = terminal {
                                        this.focus(&terminal, window, cx);
                                    }
                                }))
                                .children(tabs),
                        ),
                    )
                    .children(self.render_transcript_button(cx))
                    .child(self.render_new_button(cx)),
            )
            .child(div().flex_1().min_h_0().child(self.render_body(cx)))
    }
}

/// An agent's state on its tab: a spinner while it works, amber when it
/// waits for the user, green when it finished a turn not looked at.
fn badge_glyph(badge: crate::AgentBadge, cx: &App) -> AnyElement {
    let dot = |color| {
        div()
            .ml_1()
            .size(px(7.))
            .rounded_full()
            .bg(color)
            .into_any_element()
    };
    match badge {
        crate::AgentBadge::Working => div()
            .ml_1()
            .child(
                gpui_kit::component::spinner::Spinner::new()
                    .xsmall()
                    .color(cx.theme().info),
            )
            .into_any_element(),
        crate::AgentBadge::NeedsYou => dot(cx.theme().warning),
        crate::AgentBadge::Done => dot(cx.theme().success),
    }
}

/// A tab's name: a run's own name, else what the shell says it is doing,
/// made short. A prompt title (`ada@host:~/code/app`) becomes the shell and
/// the folder (`zsh · app`); a program's title (`vim notes.md`) stays, cut.
fn short_title(title: &str, shell: &str, label: Option<&str>) -> SharedString {
    const MAX: usize = 24;
    if let Some(label) = label {
        return cut(label, MAX);
    }
    let title = title.trim();
    let path = match title.split_once(':') {
        // `user@host:path`
        Some((who, path)) if who.contains('@') && !who.contains(' ') => Some(path.trim()),
        _ if title.starts_with(['/', '~']) || title.contains(":\\") => Some(title),
        _ => None,
    };
    if let Some(path) = path {
        let folder = path
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("/");
        return cut(&format!("{shell} · {folder}"), MAX);
    }
    if title.is_empty() {
        return cut(shell, MAX);
    }
    cut(title, MAX)
}

/// `text` cut to `max` characters, with an ellipsis.
fn cut(text: &str, max: usize) -> SharedString {
    if text.chars().count() <= max {
        return text.to_string().into();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut.into()
}

#[cfg(test)]
mod tests {
    use super::short_title;

    #[test]
    fn tab_names_are_short() {
        let name = |title: &str| short_title(title, "zsh", None).to_string();
        assert_eq!(
            name("ada@host:/Users/ada/code/LibreGuild/PilotHouse"),
            "zsh · PilotHouse"
        );
        assert_eq!(name("ada@host:~"), "zsh · ~");
        assert_eq!(name("ada@host: ~/code/app/"), "zsh · app");
        assert_eq!(name("/usr/local/src"), "zsh · src");
        assert_eq!(name("C:\\Users\\ada\\app"), "zsh · app");
        assert_eq!(name("vim notes.md"), "vim notes.md");
        assert_eq!(name(""), "zsh");
        assert_eq!(
            name("cargo watch -x test --all-features"),
            "cargo watch -x test --a…"
        );
        assert_eq!(
            short_title("x", "zsh", Some("server: dev")).as_ref(),
            "server: dev"
        );
    }
}
