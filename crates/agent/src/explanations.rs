//! Explanations of changes (see `ph_review::explanation`), from the chats'
//! side: word that one was written, and what the chats that made a change
//! were asked.
//!
//! An explanation's "why" cannot be read off the code. It is in the chats:
//! what the developer asked for, how the agent summed up its work, what it
//! ran. [`asked_about`] finds the requests whose edits wrote the lines
//! being explained, from the stored transcripts; nothing is asked of an
//! agent for it. A change is matched to its requests by its text, not by
//! when it was made: work is not always committed in the order it was done.
//!
//! What a turn wrote is read in two ways. Its edit calls carry diffs, where
//! the agent edits through its edit tool. Many do not, or not always (a
//! shell command with a heredoc, `sed`, a script): for those, the files are
//! compared between the checkpoint taken before the request and the one
//! taken before the next (`ph_git::checkpoint`), which tells what the turn
//! changed however it changed it.

use std::collections::HashSet;
use std::path::Path;

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global};
use ph_review::explanation::prompt::Asked;
use ph_store::Store;
use ph_store::projects::ProjectId;

use crate::transcript::{StoredEntry, ToolEntry, ToolKindLabel, ToolStatus};

/// The title of a chat that writes an explanation starts with this (the
/// Bridge files such chats with the runs the app started itself).
pub const CHAT_TITLE: &str = "Explain: ";

/// Word that an explanation was written (or given up), for every view that
/// shows it: its chat may run in another window or in the background.
pub struct Explanations;

/// Explanation `id` was saved, or its turn ended without one (then it is
/// gone from the store).
pub struct ExplanationWritten {
    pub id: i64,
}

impl EventEmitter<ExplanationWritten> for Explanations {}

struct GlobalExplanations(Entity<Explanations>);

impl Global for GlobalExplanations {}

impl Explanations {
    /// The app's one emitter, to subscribe to.
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(explanations) = cx.try_global::<GlobalExplanations>() {
            return explanations.0.clone();
        }
        let explanations = cx.new(|_| Self);
        cx.set_global(GlobalExplanations(explanations.clone()));
        explanations
    }

    /// Says explanation `id` changed: views of it read the store again.
    pub fn written(id: i64, cx: &mut App) {
        Self::global(cx).update(cx, |_, cx| cx.emit(ExplanationWritten { id }));
    }
}

/// How many of a workspace's chats are read for what was asked (the most
/// recently used).
const CHATS_READ: usize = 15;
/// How many commands of one turn are told.
const COMMANDS_TOLD: usize = 8;
/// How many turns (the newest) are looked at by their checkpoints, when
/// their edit calls do not show the change: each is a `git diff`.
const CHECKPOINTS_READ: usize = 40;

/// One request of a chat, with what its edits touched.
struct Turn {
    asked: Asked,
    at: i64,
    /// The files its edits changed (as the agent named them), each with
    /// the lines it added, and the lines it removed.
    edits: Vec<ph_git::content::LinesChanged>,
    /// The checkpoint of the files as they were before the request.
    checkpoint: Option<String>,
    /// The checkpoint that ends it: the one before the chat's next request,
    /// else before the next request in any of the workspace's chats; `None`
    /// when nothing was asked since (it runs to the files as they are).
    until: Option<String>,
}

/// A line long enough to tell one edit from another (a lone brace is in
/// every change).
fn telling(line: &str) -> Option<&str> {
    let line = line.trim();
    (line.len() >= 8).then_some(line)
}

impl Turn {
    /// Whether its edits touched `path` (from the repository's root).
    fn touched(&self, root: &Path, path: &str) -> bool {
        self.edits
            .iter()
            .any(|(file, ..)| same_file(root, file, path))
    }

    /// Whether it wrote a line reading `text` in `path`.
    fn wrote(&self, root: &Path, path: &str, text: &str) -> bool {
        let text = text.trim();
        !text.is_empty()
            && self.edits.iter().any(|(file, added, _)| {
                same_file(root, file, path) && added.iter().any(|line| line.trim() == text)
            })
    }

    /// Whether its edits made (part of) a change to `path` whose added and
    /// removed lines are `lines`: one of the lines it added or removed is
    /// among them.
    fn made(&self, root: &Path, path: &str, lines: &HashSet<&str>) -> bool {
        self.edits.iter().any(|(file, added, removed)| {
            same_file(root, file, path)
                && added
                    .iter()
                    .chain(removed)
                    .filter_map(|line| telling(line))
                    .any(|line| lines.contains(line))
        })
    }

    /// What changed in `paths` between its checkpoint and the one that ends
    /// it, as edits: what it did, however it did it. Empty without a
    /// checkpoint, or when the checkpoint is of another repository or was
    /// cleared away since.
    fn between_checkpoints(
        &self,
        root: &Path,
        paths: &[String],
    ) -> Vec<ph_git::content::LinesChanged> {
        let Some(from) = &self.checkpoint else {
            return Vec::new();
        };
        ph_git::content::lines_between(root, from, self.until.as_deref(), paths).unwrap_or_default()
    }
}

/// Whether `named` (a path as an agent's edit names it: absolute, or from
/// the folder it works in) is file `path` of `root`.
fn same_file(root: &Path, named: &str, path: &str) -> bool {
    let named = named.replace('\\', "/");
    let root = root.to_string_lossy().replace('\\', "/");
    let named = named
        .strip_prefix(root.trim_end_matches('/'))
        .map(|rest| rest.trim_start_matches('/'))
        .unwrap_or(&named)
        .trim_start_matches("./");
    named == path
}

/// A command a chat ran, as the record tells it (`` `cargo test` (14
/// passed) ``); `None` for a call that ran no command or has not ended.
fn told(tool: &ToolEntry) -> Option<String> {
    if tool.kind != ToolKindLabel::Execute
        || matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
    {
        return None;
    }
    // A command over several lines (a heredoc, a script) by its first.
    let whole = tool.title.trim();
    let first = whole.lines().next().unwrap_or_default().trim();
    if first.is_empty() {
        return None;
    }
    let mut command: String = first.chars().take(100).collect();
    if command.len() < whole.len() {
        command.push('…');
    }
    let how = if tool.status == ToolStatus::Failed {
        "failed".to_string()
    } else {
        match crate::evidence::tests_passed(tool) {
            Some(Some(passed)) => format!("{passed} passed"),
            Some(None) => "passed".to_string(),
            None => "ran".to_string(),
        }
    };
    Some(format!("`{command}` ({how})"))
}

/// The turns of the workspace's recent chats, oldest first.
fn turns(store: &Store, project_id: ProjectId) -> Vec<Turn> {
    let mut sessions = store.sessions(project_id).unwrap_or_default();
    // Chats that made changes: not reviews, not the ones that explain.
    sessions.retain(|session| {
        matches!(session.kind.as_str(), "chat" | "parallel")
            && !session.title.starts_with(CHAT_TITLE)
    });
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    sessions.truncate(CHATS_READ);
    let mut turns = Vec::new();
    for session in sessions {
        let agent = crate::agents::find(&session.agent_id)
            .map_or(session.agent_id.as_str(), |spec| spec.name)
            .to_string();
        let mut turn: Option<Turn> = None;
        for message in store.messages(session.id).unwrap_or_default() {
            let Ok(entry) = serde_json::from_str::<StoredEntry>(&message.body) else {
                continue;
            };
            match entry {
                StoredEntry::User {
                    text, checkpoint, ..
                } => {
                    turns.extend(turn.take());
                    turn = Some(Turn {
                        asked: Asked {
                            chat: session.title.clone(),
                            agent: agent.clone(),
                            session: Some(session.id),
                            request: text,
                            outcome: String::new(),
                            commands: Vec::new(),
                        },
                        at: message.created_at,
                        edits: Vec::new(),
                        checkpoint,
                        until: None,
                    });
                }
                StoredEntry::Agent { text } => {
                    if let Some(turn) = turn.as_mut()
                        && !text.trim().is_empty()
                    {
                        turn.asked.outcome = text;
                    }
                }
                StoredEntry::Tool(tool) => {
                    let Some(turn) = turn.as_mut() else {
                        continue;
                    };
                    for tool in std::iter::once(&tool).chain(tool.children.iter()) {
                        for diff in &tool.diffs {
                            let with = |mark: char| {
                                diff.lines
                                    .iter()
                                    .filter_map(|line| line.strip_prefix(mark))
                                    .map(str::to_string)
                                    .collect::<Vec<_>>()
                            };
                            turn.edits.push((diff.path.clone(), with('+'), with('-')));
                        }
                        if let Some(command) = told(tool) {
                            // A command run again is told as it ended.
                            let name = command.rsplit_once(" (").map_or("", |(name, _)| name);
                            turn.asked.commands.retain(|known| !known.starts_with(name));
                            turn.asked.commands.push(command);
                        }
                    }
                }
                _ => {}
            }
        }
        turns.extend(turn);
    }
    for turn in &mut turns {
        let extra = turn.asked.commands.len().saturating_sub(COMMANDS_TOLD);
        turn.asked.commands.drain(..extra);
    }
    turns.sort_by_key(|turn| turn.at);
    // Where each turn ends: before the same chat's next request, else
    // before the next request of any chat.
    for ix in 0..turns.len() {
        let later = &turns[ix + 1..];
        let chat = turns[ix].asked.session;
        let next = later
            .iter()
            .find(|turn| turn.asked.session == chat && turn.checkpoint.is_some())
            .or_else(|| later.iter().find(|turn| turn.checkpoint.is_some()))
            .and_then(|turn| turn.checkpoint.clone());
        // The same checkpoint again: nothing changed in between.
        turns[ix].until = next;
    }
    turns
}

/// What was asked in the workspace's chats in the turns that made a
/// change, oldest first. `changed`: the change's files (paths from `root`,
/// the repository's folder), each with the lines it adds and removes
/// (`ph_git::content::changed_lines`). A turn made it when one of its edits
/// to such a file wrote or took out one of those lines: by its edit calls,
/// or by what changed between its checkpoints.
pub fn asked_about(
    store: &Store,
    project_id: ProjectId,
    root: &Path,
    changed: &[(String, Vec<String>)],
) -> Vec<Asked> {
    let paths: Vec<String> = changed.iter().map(|(path, _)| path.clone()).collect();
    let changed: Vec<(&str, HashSet<&str>)> = changed
        .iter()
        .map(|(path, lines)| {
            (
                path.as_str(),
                lines.iter().filter_map(|line| telling(line)).collect(),
            )
        })
        .collect();
    let turns = turns(store, project_id);
    let mut made: Vec<bool> = turns
        .iter()
        .map(|turn| {
            changed
                .iter()
                .any(|(path, lines)| turn.made(root, path, lines))
        })
        .collect();
    // The newest turns whose edit calls do not show it: by checkpoints.
    let mut left = CHECKPOINTS_READ;
    for (ix, turn) in turns.iter().enumerate().rev() {
        if made[ix] || turn.checkpoint.is_none() {
            continue;
        }
        if left == 0 {
            break;
        }
        left -= 1;
        made[ix] = turn
            .between_checkpoints(root, &paths)
            .iter()
            .any(|(file, added, removed)| {
                changed.iter().any(|(path, lines)| {
                    file == path
                        && added
                            .iter()
                            .chain(removed)
                            .filter_map(|line| telling(line))
                            .any(|line| lines.contains(line))
                })
            });
    }
    turns
        .into_iter()
        .zip(made)
        .filter(|(_, made)| *made)
        .map(|(turn, _)| turn.asked)
        .collect()
}

/// The request that wrote line `text` of `path`: the newest turn that added
/// that line (by its edit calls, or between its checkpoints), else the
/// newest whose edit calls touched the file. For a line that is not
/// committed yet, this is all there is to say why it is there.
pub fn asked_for_line(
    store: &Store,
    project_id: ProjectId,
    root: &Path,
    path: &str,
    text: &str,
) -> Option<Asked> {
    let turns = turns(store, project_id);
    let line = text.trim();
    let paths = [path.to_string()];
    let mut left = CHECKPOINTS_READ;
    let wrote = turns.iter().rev().find(|turn| {
        if turn.wrote(root, path, text) {
            return true;
        }
        if line.is_empty() || turn.checkpoint.is_none() || left == 0 {
            return false;
        }
        left -= 1;
        turn.between_checkpoints(root, &paths)
            .iter()
            .any(|(file, added, _)| file == path && added.iter().any(|added| added.trim() == line))
    });
    wrote
        .or_else(|| turns.iter().rev().find(|turn| turn.touched(root, path)))
        .map(|turn| turn.asked.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::FileDiff;

    fn stored(entry: &StoredEntry) -> String {
        serde_json::to_string(entry).unwrap()
    }

    fn user(text: &str) -> StoredEntry {
        StoredEntry::User {
            text: text.into(),
            attachments: Vec::new(),
            checkpoint: None,
        }
    }

    fn tool(
        title: &str,
        kind: ToolKindLabel,
        status: ToolStatus,
        diffs: Vec<FileDiff>,
    ) -> StoredEntry {
        StoredEntry::Tool(ToolEntry {
            id: title.into(),
            title: title.into(),
            kind,
            status,
            output: "test result: ok. 14 passed; 0 failed".into(),
            diffs,
            children: Vec::new(),
            agent_text: String::new(),
            run: Default::default(),
            images: Vec::new(),
            expanded: false,
            started: None,
            touched: None,
        })
    }

    fn edit(path: &str, new: &str) -> StoredEntry {
        tool(
            "Edit",
            ToolKindLabel::Edit,
            ToolStatus::Done,
            vec![FileDiff::new(path, "", new)],
        )
    }

    #[test]
    fn the_requests_that_touched_a_file_are_found() {
        let store = Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir().join(format!("ph-asked-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = store.add_project(&folder).unwrap();
        let root = project.path.clone();
        let absolute = root.join("src/reset.rs").to_string_lossy().into_owned();
        let chat = store
            .create_session(project.id, "Reset emails", "claude")
            .unwrap();
        for entry in [
            user("Add a forgot-password email"),
            // An absolute path, as most agents give it.
            edit(&absolute, "fn link() {}\nlet hour = 3600;\n"),
            tool(
                "cargo test",
                ToolKindLabel::Execute,
                ToolStatus::Failed,
                Vec::new(),
            ),
            tool(
                "cargo test",
                ToolKindLabel::Execute,
                ToolStatus::Done,
                Vec::new(),
            ),
            StoredEntry::Agent {
                text: "Added the link. Tests pass.".into(),
            },
            user("What does the mailer do?"),
            StoredEntry::Agent {
                text: "It sends mail.".into(),
            },
            user("Make the link last a day"),
            edit("src/reset.rs", "let day = 86400;\n"),
            StoredEntry::Agent {
                text: "Done.".into(),
            },
        ] {
            let role = if matches!(entry, StoredEntry::User { .. }) {
                "user"
            } else {
                "agent"
            };
            store
                .append_message(chat.id, role, &stored(&entry))
                .unwrap();
        }
        // A chat that explains is not one that made changes.
        let explain = store
            .create_session(project.id, "Explain: your changes", "claude")
            .unwrap();
        store
            .append_message(explain.id, "user", &stored(&user("Explain a change")))
            .unwrap();
        store
            .append_message(explain.id, "agent", &stored(&edit("src/reset.rs", "x\n")))
            .unwrap();

        // A change is matched to its requests by its lines.
        let change = |lines: &[&str]| {
            vec![(
                "src/reset.rs".to_string(),
                lines
                    .iter()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>(),
            )]
        };
        let asked = asked_about(
            &store,
            project.id,
            &root,
            &change(&["fn link() {}", "    let day = 86400;"]),
        );
        assert_eq!(
            asked
                .iter()
                .map(|ask| ask.request.as_str())
                .collect::<Vec<_>>(),
            ["Add a forgot-password email", "Make the link last a day"]
        );
        assert_eq!(asked[0].chat, "Reset emails");
        assert_eq!(asked[0].agent, "Claude Code");
        assert_eq!(asked[0].session, Some(chat.id));
        assert_eq!(asked[0].outcome, "Added the link. Tests pass.");
        // The test command is told as it ended.
        assert_eq!(asked[0].commands, ["`cargo test` (14 passed)"]);
        // Only the request whose lines are in the change (the other was
        // committed before, say).
        let asked = asked_about(&store, project.id, &root, &change(&["let day = 86400;"]));
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].request, "Make the link last a day");
        // Lines nobody's edit wrote, another file, or a line every change
        // has: no request.
        assert!(asked_about(&store, project.id, &root, &change(&["fn other() {}"])).is_empty());
        assert!(asked_about(&store, project.id, &root, &change(&["}"])).is_empty());
        let elsewhere = vec![("src/other.rs".to_string(), vec!["fn link() {}".to_string()])];
        assert!(asked_about(&store, project.id, &root, &elsewhere).is_empty());

        // A line knows the request that wrote it; a line nobody's edit
        // shows, the last one that touched its file.
        let hour = asked_for_line(
            &store,
            project.id,
            &root,
            "src/reset.rs",
            "  let hour = 3600;",
        );
        assert_eq!(hour.unwrap().request, "Add a forgot-password email");
        let other = asked_for_line(&store, project.id, &root, "src/reset.rs", "fn unknown() {}");
        assert_eq!(other.unwrap().request, "Make the link last a day");
        assert!(asked_for_line(&store, project.id, &root, "src/mail.rs", "x").is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// An agent that edits through the shell leaves no diffs in its chat:
    /// what its turn did is told by the checkpoints around it.
    #[test]
    fn a_turn_without_diffs_is_known_by_its_checkpoints() {
        let folder = std::env::temp_dir().join(format!("ph-asked-cp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("src")).unwrap();
        let git = |args: &[&str]| {
            let done = std::process::Command::new("git")
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
                .args(args)
                .current_dir(&folder)
                .output()
                .unwrap();
            assert!(done.status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(folder.join("src/store.js"), "export function save() {}\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "first"]);
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project(&folder).unwrap();
        let root = ph_git::content::top(&folder).unwrap();
        let chat = store
            .create_session(project.id, "Remove a note", "codex")
            .unwrap();
        let ask = |text: &str| {
            let checkpoint = ph_git::checkpoint::create(&root, "turn").unwrap();
            let entry = StoredEntry::User {
                text: text.into(),
                attachments: Vec::new(),
                checkpoint,
            };
            store
                .append_message(chat.id, "user", &stored(&entry))
                .unwrap();
        };
        let shell = |title: &str| {
            let entry = tool(title, ToolKindLabel::Execute, ToolStatus::Done, Vec::new());
            store
                .append_message(chat.id, "agent", &stored(&entry))
                .unwrap();
        };

        ask("Add a way to remove a note");
        // The edit is a shell command's: no diff in the chat.
        std::fs::write(
            folder.join("src/store.js"),
            "export function save() {}\nexport function remove(id) {}\n",
        )
        .unwrap();
        std::fs::write(
            folder.join("src/routes.js"),
            "router.delete('/notes/:id');\n",
        )
        .unwrap();
        shell("cat >> src/store.js <<'EOF'");
        ask("Also count the notes");
        std::fs::write(
            folder.join("src/store.js"),
            "export function save() {}\nexport function remove(id) {}\nexport function count() {}\n",
        )
        .unwrap();
        shell("sed -i '' ... src/store.js");

        let changed = ph_git::content::changed_lines(&root, None).unwrap();
        let asked = asked_about(&store, project.id, &root, &changed);
        assert_eq!(
            asked
                .iter()
                .map(|ask| ask.request.as_str())
                .collect::<Vec<_>>(),
            ["Add a way to remove a note", "Also count the notes"]
        );
        // The first turn alone made the new file (one Git does not track).
        let routes: Vec<_> = changed
            .iter()
            .filter(|(path, _)| path == "src/routes.js")
            .cloned()
            .collect();
        let asked = asked_about(&store, project.id, &root, &routes);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].request, "Add a way to remove a note");
        // A line knows its turn the same way.
        let count = asked_for_line(
            &store,
            project.id,
            &root,
            "src/store.js",
            "export function count() {}",
        );
        assert_eq!(count.unwrap().request, "Also count the notes");
        let remove = asked_for_line(
            &store,
            project.id,
            &root,
            "src/store.js",
            "export function remove(id) {}",
        );
        assert_eq!(remove.unwrap().request, "Add a way to remove a note");
        let _ = std::fs::remove_dir_all(&folder);
    }
}
