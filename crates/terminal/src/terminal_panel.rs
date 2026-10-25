//! The Terminal tab: one or more shells in the workspace's folders, each the
//! shell the user picks (zsh, bash, PowerShell…). The shells belong to the
//! workspace ([`WorkspaceTerminals`]); this panel only shows them, with one
//! [`TerminalView`] per shell made when first shown.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
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
                shells
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
                    })
            })
            .into_any_element()
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
            .map(|terminal| {
                let terminal = terminal.read(cx);
                short_title(
                    terminal.title(),
                    terminal.shell(),
                    terminal.label().map(|label| label.as_ref()),
                )
            })
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
                    .child(self.render_new_button(cx)),
            )
            .child(div().flex_1().min_h_0().child(self.render_body(cx)))
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
