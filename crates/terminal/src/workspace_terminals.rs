//! A workspace's terminals while PilotHouse runs, held outside any window
//! ([`TerminalHub`]), so shells and runs keep going when the Terminal tab is
//! closed or rebuilt. [`crate::TerminalPanel`] shows them.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, Global, SharedString, Subscription,
};

use crate::terminal::{Terminal, TerminalEvent};

/// Every workspace's terminals, by workspace id.
#[derive(Default)]
pub struct TerminalHub {
    terminals: HashMap<i64, Entity<WorkspaceTerminals>>,
    /// Each workspace's agent CLIs that belong to a chat: they show in the
    /// chat, not among the Terminal tab's, and are not kept between runs
    /// (their chat is, and resumes them).
    agents: HashMap<i64, Entity<WorkspaceTerminals>>,
    /// Where each workspace's terminal text is kept between runs.
    state_dir: Option<PathBuf>,
}

/// How much of each terminal is kept when PilotHouse quits.
const KEPT_LINES: usize = 2000;
/// Most terminals of a workspace brought back.
const KEPT_TERMINALS: usize = 8;
/// The breath between two terminals restored (see [`TerminalHub::restore`]).
const RESTORE_PAUSE: std::time::Duration = std::time::Duration::from_millis(40);

/// A terminal as it was when PilotHouse quit.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    text: String,
    /// The folder its shell was in (where it went with `cd`, where the
    /// system tells): the one brought back starts there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder: Option<PathBuf>,
    /// The name its tab had, when it had one of its own (a run's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
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

    /// The terminals of workspace `id`'s agent CLIs shown in its chats
    /// (they start in `cwd` unless told), made on first use.
    pub fn agent_terminals(id: i64, cwd: PathBuf, cx: &mut App) -> Entity<WorkspaceTerminals> {
        if let Some(terminals) = Self::existing_agent_terminals(id, cx) {
            return terminals;
        }
        let terminals = cx.new(|_| WorkspaceTerminals::new(cwd));
        cx.default_global::<Self>()
            .agents
            .insert(id, terminals.clone());
        terminals
    }

    pub fn existing_agent_terminals(id: i64, cx: &App) -> Option<Entity<WorkspaceTerminals>> {
        cx.try_global::<Self>()?.agents.get(&id).cloned()
    }

    /// Every workspace's terminals, with the workspace's id: the Terminal
    /// tab's, and its chats' agent CLIs.
    pub fn all(cx: &App) -> Vec<(i64, Entity<WorkspaceTerminals>)> {
        cx.try_global::<Self>()
            .map(|hub| {
                hub.terminals
                    .iter()
                    .chain(&hub.agents)
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
                .map(|terminal| {
                    let terminal = terminal.read(cx);
                    Kept {
                        text: terminal.history_text(KEPT_LINES),
                        folder: Some(terminal.current_dir()),
                        label: terminal.label().map(|label| label.to_string()),
                    }
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
        if Self::existing(id, cx).is_none() {
            return false;
        }
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
        let mut kept = kept.into_iter().take(KEPT_TERMINALS);
        let Some(first) = kept.next() else {
            return false;
        };
        let restored = Self::reopen(id, &first, cx);
        // The others after it, one at a time with the window drawn in
        // between. Starting a shell takes a moment (a pseudo console and a
        // process; longer on Windows), and eight in one go held the window
        // up as it opened.
        let rest: Vec<Kept> = kept.collect();
        if !rest.is_empty() {
            cx.spawn(async move |cx| {
                for kept in rest {
                    cx.background_executor().timer(RESTORE_PAUSE).await;
                    if !cx.update(|cx| Self::reopen(id, &kept, cx)) {
                        break;
                    }
                }
            })
            .detach();
        }
        restored
    }

    /// Opens one terminal workspace `id` had, a new shell with the old text
    /// above it, behind the terminal in front. False when the workspace's
    /// terminals are gone or no shell would start.
    fn reopen(id: i64, kept: &Kept, cx: &mut App) -> bool {
        let Some(terminals) = Self::existing(id, cx) else {
            return false;
        };
        let (front, had) = {
            let terminals = terminals.read(cx);
            (terminals.active_index(), terminals.terminals().len())
        };
        // In the folder it was in, while that is still there.
        let folder = kept.folder.clone().filter(|folder| folder.is_dir());
        let Some(terminal) = terminals.update(cx, |terminals, cx| match folder {
            Some(folder) => terminals.new_terminal_in(folder, cx),
            None => terminals.new_terminal(cx),
        }) else {
            return false;
        };
        if let Some(label) = &kept.label {
            terminal.update(cx, |terminal, _| terminal.set_label(label.clone()));
        }
        terminal.read(cx).show_restored(
            &kept.text,
            concat!("from the last run of ", ph_brand::name!()),
        );
        // The first one restored is the one in front, as it was; one opened
        // by hand meanwhile stays there.
        terminals.update(cx, |terminals, cx| {
            terminals.select(if had == 0 { 0 } else { front }, cx)
        });
        true
    }

    /// Closes a removed workspace's terminals (their shells stop) and
    /// forgets their text kept from the last run.
    pub fn remove(id: i64, cx: &mut App) {
        if cx.has_global::<Self>() {
            let hub = cx.global_mut::<Self>();
            hub.terminals.remove(&id);
            hub.agents.remove(&id);
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
    /// Terminals shown side by side, each split in order, left to right. A
    /// terminal is in one split at most, and a split has two or more; each
    /// keeps its own tab.
    splits: Vec<Vec<EntityId>>,
    /// Why the last shell could not start.
    error: Option<SharedString>,
    /// Each terminal's events, dropped with its tab.
    _subscriptions: std::collections::HashMap<gpui_kit::EntityId, Subscription>,
}

impl WorkspaceTerminals {
    fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            folders: Vec::new(),
            terminals: Vec::new(),
            active: 0,
            splits: Vec::new(),
            error: None,
            _subscriptions: Default::default(),
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

    /// The terminals shown beside `terminal`, it included, left to right
    /// (`None` when it is shown alone).
    pub fn split_of(&self, terminal: EntityId) -> Option<Vec<Entity<Terminal>>> {
        let split = self.splits.iter().find(|split| split.contains(&terminal))?;
        Some(
            split
                .iter()
                .filter_map(|id| self.terminals.iter().find(|t| t.entity_id() == *id))
                .cloned()
                .collect(),
        )
    }

    /// Shows the terminal `terminal` (a tab of it, or a pane of its split).
    pub fn select_terminal(&mut self, terminal: EntityId, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .terminals
            .iter()
            .position(|known| known.entity_id() == terminal)
            && ix != self.active
        {
            self.active = ix;
            cx.notify();
        }
    }

    /// A new `shell` (the default for `None`) beside the terminal shown, in
    /// its folder: the two (or more) side by side, the new one in front.
    /// Its tab comes right after the split's others.
    pub fn split(
        &mut self,
        shell: Option<&ph_env::Shell>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        let Some(beside) = self.active().cloned() else {
            return self.new_terminal_with(self.cwd.clone(), shell, cx);
        };
        let beside_id = beside.entity_id();
        if self
            .splits
            .iter()
            .any(|split| split.contains(&beside_id) && split.len() >= MAX_SPLIT)
        {
            return None;
        }
        let cwd = beside.read(cx).cwd().to_path_buf();
        let terminal = self.start(cwd, shell, cx)?;
        let id = terminal.entity_id();
        let split = match self
            .splits
            .iter_mut()
            .find(|split| split.contains(&beside_id))
        {
            Some(split) => split,
            None => {
                self.splits.push(vec![beside_id]);
                self.splits.last_mut().expect("just added")
            }
        };
        split.push(id);
        let last = split[split.len() - 2];
        let after = self
            .terminals
            .iter()
            .position(|known| known.entity_id() == last)
            .map_or(self.terminals.len(), |ix| ix + 1);
        self.terminals.insert(after, terminal.clone());
        self.active = after;
        cx.notify();
        Some(terminal)
    }

    /// Whether the terminal shown can take one more beside it.
    pub fn can_split(&self) -> bool {
        let Some(active) = self.active() else {
            return true;
        };
        let id = active.entity_id();
        !self
            .splits
            .iter()
            .any(|split| split.contains(&id) && split.len() >= MAX_SPLIT)
    }

    /// Shows the terminals of `terminal`'s split one at a time again (each
    /// keeps running, in its tab).
    pub fn unsplit(&mut self, terminal: EntityId, cx: &mut Context<Self>) {
        self.splits.retain(|split| !split.contains(&terminal));
        cx.notify();
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
        let terminal = self.start(cwd, shell, cx)?;
        self.terminals.push(terminal.clone());
        self.active = self.terminals.len() - 1;
        cx.notify();
        Some(terminal)
    }

    /// Starts a `shell` in `cwd`, followed (its title, its end), not yet
    /// in a tab.
    fn start(
        &mut self,
        cwd: PathBuf,
        shell: Option<&ph_env::Shell>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        self.follow(Terminal::start_with(cwd, shell), cx)
    }

    /// The terminal of a shell (or a program) that `started`, followed (its
    /// title, its end), not yet in a tab; why it did not start is kept.
    fn follow(
        &mut self,
        started: anyhow::Result<crate::terminal::StartedShell>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        let shell = match started {
            Ok(shell) => shell,
            Err(err) => {
                self.error = Some(format!("Could not start a shell: {err:#}").into());
                cx.notify();
                return None;
            }
        };
        let terminal = cx.new(|cx| Terminal::new(shell, cx));
        self._subscriptions.insert(
            terminal.entity_id(),
            cx.subscribe(
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
            ),
        );
        self.error = None;
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
            self.remove_at(ix);
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
            // A new shell: typed once it shows its prompt.
            terminal.update(cx, |terminal, cx| {
                terminal.run_command_when_ready(command, cx)
            });
        }
        cx.notify();
        Some(terminal)
    }

    /// Runs `command` as the whole point of a new terminal named `label`
    /// in `cwd` (an agent's CLI): the user's shell starts it and ends with
    /// it, so no prompt is left behind and the command is not shown typed.
    /// The shell reads the user's profile as for a terminal of their own,
    /// so `PATH` and aliases are theirs.
    pub fn run_program(
        &mut self,
        label: &str,
        command: &str,
        cwd: PathBuf,
        environment: &[(String, String)],
        cx: &mut Context<Self>,
    ) -> Option<Entity<Terminal>> {
        // On a server (a network drive's folder) the shell is the
        // server's, reached through ssh: the command is typed into it
        // (what is set here would be set for `ssh`, not for the program).
        if ph_remote::remote_of(&cwd).is_some() {
            return self.run_in(label, command, Some(cwd), cx);
        }
        let (program, args) = run_with(ph_env::default_shell(), command);
        let started =
            Terminal::start_program_with(&program.to_string_lossy(), args, cwd, environment).map(
                |mut started| {
                    started.shell = label.to_string().into();
                    started
                },
            );
        let terminal = self.follow(started, cx)?;
        terminal.update(cx, |terminal, _| terminal.set_label(label.to_string()));
        self.terminals.push(terminal.clone());
        self.active = self.terminals.len() - 1;
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
        self.remove_at(ix);
        cx.notify();
    }

    /// Takes the terminal at `ix` out of the tabs and of its split (which
    /// goes on without it; one left alone is a tab again).
    fn remove_at(&mut self, ix: usize) {
        let id = self.terminals.remove(ix).entity_id();
        self._subscriptions.remove(&id);
        self.active = active_after_close(self.active, ix, self.terminals.len());
        for split in &mut self.splits {
            split.retain(|known| *known != id);
        }
        self.splits.retain(|split| split.len() > 1);
    }
}

/// The most terminals side by side: each needs room for its lines.
const MAX_SPLIT: usize = 4;

/// The tab shown after closing tab `closed` (`active` was shown; `left`
/// remain): the same terminal if another closed, else the one that took its
/// place, else the last.
/// `shell` (a program and what it is started with) told to run `command`
/// and end: `-c` for a Unix shell, interactive so that what a profile sets
/// up for interactive shells (aliases, a version manager's `PATH`) is
/// there; `-Command` for PowerShell, `/C` for Command Prompt.
fn run_with((program, mut args): (PathBuf, Vec<String>), command: &str) -> (PathBuf, Vec<String>) {
    // Its name, whichever way its path is written.
    let path = program.to_string_lossy().to_lowercase();
    let file = path.rsplit(['/', '\\']).next().unwrap_or_default();
    let name = file.strip_suffix(".exe").unwrap_or(file);
    match name {
        "cmd" => args.extend(["/C".to_string(), command.to_string()]),
        "pwsh" | "powershell" => args.extend(["-Command".to_string(), command.to_string()]),
        _ => args.extend(["-i".to_string(), "-c".to_string(), command.to_string()]),
    }
    (program, args)
}

fn active_after_close(active: usize, closed: usize, left: usize) -> usize {
    let active = if closed < active { active - 1 } else { active };
    active.min(left.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{active_after_close, run_with};

    #[test]
    fn a_kept_terminal_says_where_it_was_and_what_it_was_named() {
        let kept = super::Kept {
            text: "$ cargo test\n".into(),
            folder: Some(PathBuf::from("/work/api")),
            label: Some("dev server".into()),
        };
        let json = serde_json::to_string(&[&kept]).unwrap();
        let back: Vec<super::Kept> = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back[0].folder.as_deref(),
            Some(std::path::Path::new("/work/api"))
        );
        assert_eq!(back[0].label.as_deref(), Some("dev server"));
        // What an older version kept (the text alone) is still read.
        let old: Vec<super::Kept> = serde_json::from_str(r#"[{"text":"hi"}]"#).unwrap();
        assert_eq!(old[0].text, "hi");
        assert!(old[0].folder.is_none() && old[0].label.is_none());
    }

    #[test]
    fn a_program_is_run_by_the_users_shell() {
        let run = |program: &str, args: &[&str]| {
            let (_, args) = run_with(
                (
                    PathBuf::from(program),
                    args.iter().map(|arg| arg.to_string()).collect(),
                ),
                "claude --resume x",
            );
            args.join(" ")
        };
        assert_eq!(run("/bin/zsh", &["-l"]), "-l -i -c claude --resume x");
        assert_eq!(
            run("/usr/local/bin/fish", &["-l"]),
            "-l -i -c claude --resume x"
        );
        assert_eq!(
            run("C:\\Program Files\\PowerShell\\7\\pwsh.exe", &["-NoLogo"]),
            "-NoLogo -Command claude --resume x"
        );
        assert_eq!(run("cmd.exe", &[]), "/C claude --resume x");
    }

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
