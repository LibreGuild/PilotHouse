//! A workspace's terminals while PilotHouse runs, held outside any window
//! ([`TerminalHub`]), so shells and runs keep going when the Terminal tab is
//! closed or rebuilt. [`crate::TerminalPanel`] shows them.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Subscription};

use crate::terminal::{Terminal, TerminalEvent};

/// Every workspace's terminals, by workspace id.
#[derive(Default)]
pub struct TerminalHub {
    terminals: HashMap<i64, Entity<WorkspaceTerminals>>,
}

impl Global for TerminalHub {}

impl TerminalHub {
    /// The terminals of workspace `id` (shells start in `cwd`), made on
    /// first use.
    pub fn terminals(id: i64, cwd: PathBuf, cx: &mut App) -> Entity<WorkspaceTerminals> {
        if let Some(terminals) = Self::existing(id, cx) {
            return terminals;
        }
        let terminals = cx.new(|_| WorkspaceTerminals::new(cwd));
        cx.default_global::<Self>()
            .terminals
            .insert(id, terminals.clone());
        terminals
    }

    pub fn existing(id: i64, cx: &App) -> Option<Entity<WorkspaceTerminals>> {
        cx.try_global::<Self>()?.terminals.get(&id).cloned()
    }

    /// Closes a removed workspace's terminals (their shells stop).
    pub fn remove(id: i64, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.global_mut::<Self>().terminals.remove(&id);
        }
    }
}

pub struct WorkspaceTerminals {
    /// Where a new shell starts: the primary folder.
    cwd: PathBuf,
    /// The workspace's folders (named, the primary first), so a new
    /// terminal or a run can start in any of them.
    folders: Vec<(String, PathBuf)>,
    terminals: Vec<Entity<Terminal>>,
    active: usize,
    /// Why the last shell could not start.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceTerminals {
    fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            folders: Vec::new(),
            terminals: Vec::new(),
            active: 0,
            error: None,
            _subscriptions: Vec::new(),
        }
    }

    pub fn terminals(&self) -> &[Entity<Terminal>] {
        &self.terminals
    }

    pub fn active(&self) -> Option<&Entity<Terminal>> {
        self.terminals.get(self.active)
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn folders(&self) -> &[(String, PathBuf)] {
        &self.folders
    }

    /// The workspace's folders changed (the primary first).
    pub fn set_folders(&mut self, cwd: PathBuf, folders: Vec<(String, PathBuf)>) {
        self.cwd = cwd;
        self.folders = folders;
    }

    pub fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.terminals.len() {
            self.active = ix;
            cx.notify();
        }
    }

    /// A new shell in the primary folder.
    pub fn new_terminal(&mut self, cx: &mut Context<Self>) -> Option<Entity<Terminal>> {
        let cwd = self.cwd.clone();
        self.new_terminal_in(cwd, cx)
    }

    /// A new shell in folder `cwd`, shown.
    pub fn new_terminal_in(
        &mut self,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        let shell = match Terminal::start(cwd) {
            Ok(shell) => shell,
            Err(err) => {
                self.error = Some(format!("Could not start a shell: {err:#}").into());
                cx.notify();
                return None;
            }
        };
        let terminal = cx.new(|cx| Terminal::new(shell, cx));
        self._subscriptions.push(cx.subscribe(
            &terminal,
            |this, terminal, event: &TerminalEvent, cx| match event {
                // A program that failed (an `ssh` that could not connect)
                // keeps its tab, so what it said can be read.
                TerminalEvent::Exited
                    if terminal.read(cx).exit_code().is_some_and(|code| code != 0) =>
                {
                    cx.notify()
                }
                TerminalEvent::Exited => this.close(&terminal, cx),
                TerminalEvent::TitleChanged => cx.notify(),
            },
        ));
        self.terminals.push(terminal.clone());
        self.active = self.terminals.len() - 1;
        self.error = None;
        cx.notify();
        Some(terminal)
    }

    /// Runs `command` in the terminal named `label`, opening one (in `cwd`,
    /// else the primary folder) if needed, and shows it. A command still
    /// running there is stopped first.
    pub fn run_in(
        &mut self,
        label: &str,
        command: &str,
        cwd: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        let existing = self
            .terminals
            .iter()
            .position(|terminal| terminal.read(cx).label().is_some_and(|name| name == label));
        let terminal = match existing {
            Some(ix) => {
                self.active = ix;
                self.terminals[ix].clone()
            }
            None => {
                let cwd = cwd.unwrap_or_else(|| self.cwd.clone());
                let terminal = self.new_terminal_in(cwd, cx)?;
                terminal.update(cx, |terminal, _| terminal.set_label(label.to_string()));
                terminal
            }
        };
        let command = command.to_string();
        if existing.is_some() {
            // Ctrl-C clears typed input too, so give the old process a moment.
            terminal.read(cx).interrupt();
            let again = terminal.clone();
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                again.update(cx, |terminal, _| terminal.run_command(&command));
            })
            .detach();
        } else {
            terminal.read(cx).run_command(&command);
        }
        cx.notify();
        Some(terminal)
    }

    /// Closes `terminal` (its shell stops).
    pub fn close(&mut self, terminal: &Entity<Terminal>, cx: &mut Context<Self>) {
        let Some(ix) = self
            .terminals
            .iter()
            .position(|existing| existing == terminal)
        else {
            return;
        };
        self.terminals.remove(ix);
        self.active = active_after_close(self.active, ix, self.terminals.len());
        cx.notify();
    }

    pub fn close_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminals.get(ix).cloned() {
            self.close(&terminal, cx);
        }
    }
}

/// The tab shown after closing tab `closed` (`active` was shown; `left`
/// remain): the same terminal if another closed, else the one that took its
/// place, else the last.
fn active_after_close(active: usize, closed: usize, left: usize) -> usize {
    let active = if closed < active { active - 1 } else { active };
    active.min(left.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::active_after_close;

    #[test]
    fn closing_a_tab_keeps_a_sensible_one_shown() {
        assert_eq!(active_after_close(2, 0, 3), 1, "the shown one moves left");
        assert_eq!(active_after_close(1, 1, 2), 1, "the next takes its place");
        assert_eq!(
            active_after_close(2, 2, 2),
            1,
            "the last closed: the new last"
        );
        assert_eq!(active_after_close(0, 3, 3), 0, "another closed");
        assert_eq!(active_after_close(0, 0, 0), 0, "none left");
    }
}
