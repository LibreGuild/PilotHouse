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
    /// Where each workspace's terminal text is kept between runs.
    state_dir: Option<PathBuf>,
}

/// How much of each terminal is kept when PilotHouse quits.
const KEPT_LINES: usize = 2000;
/// Most terminals of a workspace brought back.
const KEPT_TERMINALS: usize = 8;

/// A terminal as it was when PilotHouse quit.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    text: String,
}

impl Global for TerminalHub {}

impl TerminalHub {
    /// The terminals of workspace `id` (shells start in `cwd`), made on
    /// first use, with those it had when PilotHouse last quit (however they
    /// are first wanted: the tab, a run, an agent).
    pub fn terminals(id: i64, cwd: PathBuf, cx: &mut App) -> Entity<WorkspaceTerminals> {
        if let Some(terminals) = Self::existing(id, cx) {
            return terminals;
        }
        let terminals = cx.new(|_| WorkspaceTerminals::new(cwd));
        cx.default_global::<Self>()
            .terminals
            .insert(id, terminals.clone());
        Self::restore(id, cx);
        terminals
    }

    pub fn existing(id: i64, cx: &App) -> Option<Entity<WorkspaceTerminals>> {
        cx.try_global::<Self>()?.terminals.get(&id).cloned()
    }

    /// Every workspace's terminals, with the workspace's id.
    pub fn all(cx: &App) -> Vec<(i64, Entity<WorkspaceTerminals>)> {
        cx.try_global::<Self>()
            .map(|hub| {
                hub.terminals
                    .iter()
                    .map(|(id, terminals)| (*id, terminals.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keeps every workspace's terminal text in `dir` when PilotHouse
    /// quits, for [`Self::restore`] on the next start.
    pub fn keep_between_runs(dir: PathBuf, cx: &mut App) {
        cx.default_global::<Self>().state_dir = Some(dir);
        cx.on_app_quit(|cx| {
            Self::save(cx);
            async {}
        })
        .detach();
    }

    fn save(cx: &App) {
        let Some(hub) = cx.try_global::<Self>() else {
            return;
        };
        let Some(dir) = hub.state_dir.clone() else {
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        for (id, terminals) in &hub.terminals {
            let kept: Vec<Kept> = terminals
                .read(cx)
                .terminals()
                .iter()
                .take(KEPT_TERMINALS)
                .map(|terminal| Kept {
                    text: terminal.read(cx).history_text(KEPT_LINES),
                })
                .filter(|kept| !kept.text.trim().is_empty())
                .collect();
            let file = dir.join(format!("{id}.json"));
            if kept.is_empty() {
                let _ = std::fs::remove_file(&file);
            } else if let Ok(json) = serde_json::to_vec(&kept) {
                let _ = std::fs::write(&file, json);
            }
        }
    }

    /// Opens the terminals workspace `id` had when PilotHouse last quit,
    /// each a new shell with the old text above it. False when there were
    /// none (or none were kept).
    fn restore(id: i64, cx: &mut App) -> bool {
        let Some(terminals) = Self::existing(id, cx) else {
            return false;
        };
        let Some(dir) = cx
            .try_global::<Self>()
            .and_then(|hub| hub.state_dir.clone())
        else {
            return false;
        };
        let file = dir.join(format!("{id}.json"));
        let Ok(json) = std::fs::read(&file) else {
            return false;
        };
        let _ = std::fs::remove_file(&file);
        let Ok(kept) = serde_json::from_slice::<Vec<Kept>>(&json) else {
            return false;
        };
        let mut restored = false;
        for kept in kept.iter().take(KEPT_TERMINALS) {
            let terminal = terminals.update(cx, |terminals, cx| terminals.new_terminal(cx));
            if let Some(terminal) = terminal {
                terminal
                    .read(cx)
                    .show_restored(&kept.text, "from the last run of PilotHouse");
                restored = true;
            }
        }
        if restored {
            terminals.update(cx, |terminals, cx| terminals.select(0, cx));
        }
        restored
    }

    /// Closes a removed workspace's terminals (their shells stop) and
    /// forgets their text kept from the last run.
    pub fn remove(id: i64, cx: &mut App) {
        if cx.has_global::<Self>() {
            let hub = cx.global_mut::<Self>();
            hub.terminals.remove(&id);
            if let Some(dir) = &hub.state_dir {
                let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
            }
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
    /// Where a new shell starts (the workspace's primary folder).
    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

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
        self.new_terminal_with(cwd, None, cx)
    }

    /// A new `shell` (the default for `None`) in folder `cwd`, shown.
    pub fn new_terminal_with(
        &mut self,
        cwd: PathBuf,
        shell: Option<&ph_env::Shell>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        let shell = match Terminal::start_with(cwd, shell) {
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
                TerminalEvent::TitleChanged | TerminalEvent::ProgramStatus => cx.notify(),
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
        // One whose program ended (a shell on a server that could not be
        // reached keeps its tab) takes nothing more: a fresh one replaces it.
        if let Some(ix) = self.terminals.iter().position(|terminal| {
            let terminal = terminal.read(cx);
            terminal.has_exited() && terminal.label().is_some_and(|name| name == label)
        }) {
            self.terminals.remove(ix);
            self.active = active_after_close(self.active, ix, self.terminals.len());
        }
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
