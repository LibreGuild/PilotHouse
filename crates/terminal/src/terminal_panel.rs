//! The Terminal tab: one or more shells in the workspace's folders, and agent
//! CLIs without a chat (see [`crate::cli_agents`]). The shells belong to the
//! workspace ([`WorkspaceTerminals`]); this panel only shows them, with one
//! [`TerminalView`] per shell made when first shown.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EntityId, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
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
}

impl gpui_kit::EventEmitter<TerminalPanelEvent> for TerminalPanel {}

pub struct TerminalPanel {
    terminals: Entity<WorkspaceTerminals>,
    /// A view per shell shown so far.
    views: HashMap<EntityId, (Entity<TerminalView>, Subscription)>,
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
                cx.notify();
            }),
            terminals,
            views: HashMap::new(),
        }
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
            let TerminalViewEvent::Ask(text) = event;
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
    fn focus(&mut self, terminal: &Entity<Terminal>, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Starts an agent CLI in its own terminal, named after it.
    pub fn start_cli(
        &mut self,
        agent: &crate::cli_agents::CliAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_in(agent.name, agent.command, None, window, cx);
    }

    /// "+": a new shell; with several folders, a menu of them.
    fn render_new_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("new-terminal")
            .ghost()
            .xsmall()
            .mr_2()
            .icon(Icons::Plus)
            .tooltip("New terminal")
            .accessibility_label("New terminal");
        let folders = self.terminals.read(cx).folders().to_vec();
        if folders.len() < 2 {
            return button
                .on_click(cx.listener(|this, _, window, cx| this.new_terminal(window, cx)))
                .into_any_element();
        }
        let panel = cx.weak_entity();
        button
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                folders
                    .iter()
                    .fold(menu.label("New terminal in"), |menu, (name, path)| {
                        let (panel, path) = (panel.clone(), path.clone());
                        menu.item(PopupMenuItem::new(name.clone()).on_click(
                            move |_, window, cx| {
                                _ = panel.update(cx, |panel, cx| {
                                    panel.new_terminal_in(path.clone(), window, cx)
                                });
                            },
                        ))
                    })
            })
            .into_any_element()
    }

    /// Agent CLIs found on this machine; looked up each time it opens, so
    /// one installed meanwhile shows up.
    fn render_cli_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.weak_entity();
        Button::new("terminal-cli-agents")
            .ghost()
            .xsmall()
            .icon(Icons::Bot)
            .tooltip("Start an agent CLI")
            .accessibility_label("Start an agent CLI")
            .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                let installed = crate::cli_agents::installed();
                if installed.is_empty() {
                    let names: Vec<_> = crate::cli_agents::CLI_AGENTS
                        .iter()
                        .map(|agent| agent.program)
                        .collect();
                    return menu
                        .label("No other agent CLIs found")
                        .label(format!("Looked for: {}", names.join(", ")));
                }
                let menu = menu.label("Start in a terminal");
                installed.into_iter().fold(menu, |menu, agent| {
                    let panel = panel.clone();
                    menu.item(
                        PopupMenuItem::new(agent.name).on_click(move |_, window, cx| {
                            _ = panel.update(cx, |panel, cx| panel.start_cli(agent, window, cx));
                        }),
                    )
                })
            })
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(terminal) = self.terminals.read(cx).active().cloned() {
            return self.view(&terminal, cx).into_any_element();
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
        let titles: Vec<SharedString> = terminals
            .terminals()
            .iter()
            .map(|terminal| short_title(terminal.read(cx).title()))
            .collect();
        let tabs = titles.into_iter().enumerate().map(|(ix, title)| {
            Tab::new().label(title).suffix(
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
                    .child(self.render_cli_menu(cx))
                    .child(self.render_new_button(cx)),
            )
            .child(div().flex_1().min_h_0().child(self.render_body(cx)))
    }
}

/// Most of a long title is its start (`user@host:`); keep the end, where the
/// folder or program is.
fn short_title(title: &str) -> SharedString {
    const MAX: usize = 28;
    let count = title.chars().count();
    if count <= MAX {
        return title.to_string().into();
    }
    let tail: String = title.chars().skip(count - (MAX - 1)).collect();
    format!("…{tail}").into()
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_titles_keep_their_end() {
        assert_eq!(super::short_title("zsh").as_ref(), "zsh");
        let short = super::short_title("ada@host:/Users/ada/code/LibreGuild/PilotHouse");
        assert_eq!(short.chars().count(), 28);
        assert!(short.ends_with("PilotHouse") && short.starts_with('…'));
    }
}
