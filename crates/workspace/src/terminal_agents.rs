//! Coding agents run as their own CLI in a workspace terminal, followed by
//! PilotHouse. Their state shows on the terminal tab, on the Agents board
//! and in notifications, like a chat's. How PilotHouse knows, best first:
//!
//! 1. **Hooks** (Claude Code started from PilotHouse): per-launch settings
//!    (`claude --settings <file>`, nothing written to `~/.claude`) whose
//!    command, `pilothouse agent-hook`, passes each event to the app
//!    (`agent.event`): the prompt, each tool, the question, the last reply.
//! 2. **Program status** (OSC 7501, see `ph_terminal::program_status`):
//!    Codex, Antigravity and Claude Code report their state in their output,
//!    wherever they were started, even by hand.
//! 3. **The title** (Gemini CLI): ◇ ready, ✦ working, ✋ action required.
//!
//! [`sync`] looks at every terminal once a second for 2 and 3.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::{App, Entity, Global, WeakEntity};
use ph_agent::ChatState;
use ph_agent::agents::AgentSpec;
use ph_store::paths;
use ph_store::projects::ProjectId;
use ph_terminal::{AgentBadge, Terminal, WorkspaceTerminals};
use serde_json::Value;

/// How much of a session file's end is read for the last reply.
const TRANSCRIPT_TAIL: u64 = 256 * 1024;

/// How PilotHouse follows an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Hooks,
    ProgramStatus,
    Title,
}

/// One agent CLI in a terminal.
pub(crate) struct TerminalAgent {
    /// Its id: in hook commands (`--pane`), else from its terminal.
    pub pane: String,
    pub source: Source,
    pub project: ProjectId,
    pub terminal: WeakEntity<Terminal>,
    pub agent: &'static AgentSpec,
    pub state: ChatState,
    /// The first thing asked, as its name.
    pub title: Option<String>,
    /// What it is doing, asks, or said last.
    pub line: Option<String>,
    /// The agent's own session id and its session file.
    pub session_id: Option<String>,
    pub transcript: Option<PathBuf>,
    pub updated_at: i64,
    /// When PilotHouse first saw it (Unix milliseconds).
    pub since: i64,
}

#[derive(Default)]
pub(crate) struct TerminalAgents {
    agents: Vec<TerminalAgent>,
    /// Sessions resumed in a terminal whose agent has not said anything
    /// yet, by pane: they become its `session_id` once it does.
    resumed: HashMap<String, String>,
}

impl Global for TerminalAgents {}

impl TerminalAgents {
    /// The agents whose terminal is still open.
    pub(crate) fn all(cx: &App) -> Vec<&TerminalAgent> {
        cx.try_global::<Self>()
            .map(|agents| {
                agents
                    .agents
                    .iter()
                    .filter(|agent| agent.terminal.upgrade().is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The agent running in `terminal`.
    pub(crate) fn of_terminal<'a>(
        terminal: &Entity<Terminal>,
        cx: &'a App,
    ) -> Option<&'a TerminalAgent> {
        cx.try_global::<Self>()?
            .agents
            .iter()
            .find(|agent| agent.terminal.upgrade().as_ref() == Some(terminal))
    }

    /// The agent running in `pane`.
    pub(crate) fn find<'a>(pane: &str, cx: &'a App) -> Option<&'a TerminalAgent> {
        cx.try_global::<Self>()?
            .agents
            .iter()
            .find(|agent| agent.pane == pane)
    }

    /// The user looked at agent `pane`: its news is seen.
    pub(crate) fn seen(pane: &str, cx: &mut App) {
        let Some(agent) = cx
            .default_global::<Self>()
            .agents
            .iter_mut()
            .find(|agent| agent.pane == pane && agent.state == ChatState::Done)
        else {
            return;
        };
        agent.state = ChatState::Idle;
        let terminal = agent.terminal.clone();
        _ = terminal.update(cx, |terminal, cx| terminal.acknowledge(cx));
    }
}

/// The id of an agent found from its terminal.
fn terminal_pane(terminal: &Entity<Terminal>) -> String {
    format!("t{}", terminal.entity_id())
}

/// The agent an OSC 7501 `app` (or a terminal's name) means.
fn agent_named(name: &str) -> Option<&'static AgentSpec> {
    let name = name.to_lowercase();
    let id = if name.contains("claude") {
        "claude"
    } else if name.contains("codex") {
        "codex"
    } else if name.contains("gemini") {
        "gemini"
    } else if name.contains("agy") || name.contains("antigravity") {
        "antigravity"
    } else {
        return None;
    };
    ph_agent::agents::find(id)
}

/// Gemini CLI's state from its title: `◇  Ready (folder)`,
/// `✦ …`/`⏲  Working…`, `✋  Action Required`.
fn title_state(title: &str) -> Option<ChatState> {
    match title.trim_start().chars().next()? {
        '\u{270B}' => Some(ChatState::NeedsYou),
        '\u{25C7}' => Some(ChatState::Idle),
        '\u{2726}' | '\u{23F2}' => Some(ChatState::Working),
        _ => None,
    }
}

/// What a terminal's agent is and does, read from the terminal.
struct Found {
    agent: &'static AgentSpec,
    source: Source,
    state: ChatState,
    line: Option<String>,
    title: Option<String>,
}

/// Reads agent `known` (its last state, if followed already) from
/// `terminal`: its program status, else a Gemini CLI's title.
fn read_terminal(terminal: &Terminal, known: Option<ChatState>) -> Option<Found> {
    use ph_terminal::program_status::ProgramState;
    let label = terminal.label().map(|label| label.to_string());
    if let Some(status) = terminal.program_status() {
        let agent = status
            .app
            .as_deref()
            .and_then(agent_named)
            .or_else(|| label.as_deref().and_then(agent_named))?;
        let state = match (terminal.badge(), status.state) {
            (Some(AgentBadge::Done), _) => ChatState::Done,
            (_, ProgramState::Working) => ChatState::Working,
            (_, ProgramState::Blocked) => ChatState::NeedsYou,
            (_, ProgramState::Error) => ChatState::Failed,
            _ => ChatState::Idle,
        };
        // Its own title, else the one it gave the terminal (Claude names it
        // after the session), without a leading status glyph.
        let terminal_title = terminal
            .program_title()
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .trim()
            .to_string();
        let title = status.title.clone().or_else(|| {
            (!terminal_title.is_empty() && Some(terminal_title.as_str()) != label.as_deref())
                .then_some(terminal_title)
        });
        return Some(Found {
            agent,
            source: Source::ProgramStatus,
            state,
            line: status.msg.clone(),
            title,
        });
    }
    if !label
        .as_deref()
        .is_some_and(|label| label.starts_with("Gemini CLI"))
    {
        return None;
    }
    let state = match (known, title_state(terminal.program_title())?) {
        // Ready after work is news until looked at.
        (Some(ChatState::Working), ChatState::Idle) => ChatState::Done,
        (Some(ChatState::Done), ChatState::Idle) if terminal.badge() == Some(AgentBadge::Done) => {
            ChatState::Done
        }
        (_, state) => state,
    };
    Some(Found {
        agent: agent_named("gemini")?,
        source: Source::Title,
        state,
        line: None,
        title: None,
    })
}

/// Finds agents from what their terminals say (program status, Gemini's
/// title) and brings their state up to date. Called once a second.
pub(crate) fn sync(cx: &mut App) {
    // A hooked agent's news is seen once its tab was looked at (that
    // clears the tab's dot).
    let mut agents = std::mem::take(&mut cx.default_global::<TerminalAgents>().agents);
    for agent in &mut agents {
        if agent.source == Source::Hooks
            && agent.state == ChatState::Done
            && agent
                .terminal
                .upgrade()
                .is_some_and(|terminal| terminal.read(cx).badge() != Some(AgentBadge::Done))
        {
            agent.state = ChatState::Idle;
        }
    }
    cx.default_global::<TerminalAgents>().agents = agents;
    let mut seen_panes = Vec::new();
    for (project, terminals) in ph_terminal::TerminalHub::all(cx) {
        let list = terminals.read(cx).terminals().to_vec();
        for terminal in list {
            let pane = terminal_pane(&terminal);
            let agents = cx.default_global::<TerminalAgents>();
            let hooked = agents.agents.iter().any(|agent| {
                agent.source == Source::Hooks
                    && agent.terminal.upgrade().as_ref() == Some(&terminal)
            });
            if hooked {
                continue;
            }
            let known = agents
                .agents
                .iter()
                .find(|agent| agent.pane == pane)
                .map(|agent| agent.state);
            let Some(found) = read_terminal(terminal.read(cx), known) else {
                continue;
            };
            seen_panes.push(pane.clone());
            if found.source == Source::Title {
                let badge = match found.state {
                    ChatState::Working => Some(AgentBadge::Working),
                    ChatState::NeedsYou => Some(AgentBadge::NeedsYou),
                    ChatState::Done => Some(AgentBadge::Done),
                    _ => None,
                };
                terminal.update(cx, |terminal, cx| terminal.set_badge(badge, cx));
            }
            let agents = cx.default_global::<TerminalAgents>();
            match agents.agents.iter_mut().find(|agent| agent.pane == pane) {
                Some(agent) => {
                    if agent.state != found.state
                        || (found.line.is_some() && agent.line != found.line)
                    {
                        agent.updated_at = now();
                    }
                    agent.state = found.state;
                    // A program's line is as it says now (none once done).
                    if found.line.is_some() || found.source == Source::ProgramStatus {
                        agent.line = found.line;
                    }
                    if found.title.is_some() {
                        agent.title = found.title;
                    }
                }
                None => {
                    let session_id = agents.resumed.remove(&pane);
                    agents.agents.push(TerminalAgent {
                        pane,
                        source: found.source,
                        project,
                        terminal: terminal.downgrade(),
                        agent: found.agent,
                        state: found.state,
                        title: found.title,
                        line: found.line,
                        session_id,
                        transcript: None,
                        updated_at: now(),
                        since: now(),
                    })
                }
            }
        }
    }
    // Agents whose terminal went, or whose program stopped saying, go.
    let agents = cx.default_global::<TerminalAgents>();
    agents.agents.retain(|agent| {
        agent.terminal.upgrade().is_some()
            && (agent.source == Source::Hooks || seen_panes.contains(&agent.pane))
    });
}

/// Answers `agent.event` from `pilothouse agent-hook`, and clears the
/// hook settings of an earlier run.
pub(crate) fn install(cx: &mut App) {
    let _ = std::fs::remove_dir_all(settings_dir());
    ph_control::ControlServer::handle(
        "agent.event",
        |params, cx| {
            let pane = params["pane"].as_str().unwrap_or_default().to_string();
            apply(&pane, &params["event"], cx);
            // The Agents board and title bar show its line and time; nothing
            // else they observe changed.
            cx.refresh_windows();
            Ok(Value::Null)
        },
        cx,
    );
}

fn settings_dir() -> PathBuf {
    paths::data_dir().join("terminal-agents")
}

/// Starts agent `agent_id`'s CLI in a new terminal of `terminals` in `cwd`,
/// followed by PilotHouse; `resume` continues one of its sessions.
pub(crate) fn launch(
    agent_id: &str,
    project: ProjectId,
    terminals: &Entity<WorkspaceTerminals>,
    cwd: PathBuf,
    resume: Option<&str>,
    cx: &mut App,
) -> anyhow::Result<Entity<Terminal>> {
    let agent = ph_agent::agents::find(agent_id)
        .ok_or_else(|| anyhow::anyhow!("{agent_id} is not a known agent"))?;
    let resume = resume.filter(|session| is_session_id(session));
    if agent_id != "claude" {
        // Followed by what it says in the terminal (program status, title).
        let command = match (agent_id, resume) {
            ("codex", Some(session)) => format!("codex resume {session}"),
            ("codex", None) => "codex".to_string(),
            ("gemini", Some(session)) => format!("gemini --resume {session}"),
            ("gemini", None) => "gemini".to_string(),
            ("antigravity", Some(session)) => format!("agy --conversation {session}"),
            ("antigravity", None) => "agy".to_string(),
            _ => anyhow::bail!("{} cannot run in a terminal", agent.name),
        };
        let label = free_label(agent.name, terminals, cx);
        let terminal = terminals
            .update(cx, |terminals, cx| {
                terminals.run_in(&label, &command, Some(cwd), cx)
            })
            .ok_or_else(|| anyhow::anyhow!("Could not open a terminal"))?;
        if let Some(session) = resume {
            cx.default_global::<TerminalAgents>()
                .resumed
                .insert(terminal_pane(&terminal), session.to_string());
        }
        return Ok(terminal);
    }
    let pane = format!(
        "p{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or_default()
    );
    let settings = settings_dir().join(format!("{pane}.json"));
    std::fs::create_dir_all(settings_dir())?;
    std::fs::write(
        &settings,
        serde_json::to_vec_pretty(&hook_settings(&hook_command(&pane)?))?,
    )?;
    let mut command = format!("claude --settings {}", quoted(&settings));
    if let Some(session) = resume {
        command.push_str(&format!(" --resume {session}"));
    }
    let label = free_label(agent.name, terminals, cx);
    let terminal = terminals
        .update(cx, |terminals, cx| {
            terminals.run_in(&label, &command, Some(cwd), cx)
        })
        .ok_or_else(|| anyhow::anyhow!("Could not open a terminal"))?;
    let agents = cx.default_global::<TerminalAgents>();
    agents
        .agents
        .retain(|agent| agent.terminal.upgrade().is_some());
    agents.agents.push(TerminalAgent {
        pane,
        source: Source::Hooks,
        project,
        terminal: terminal.downgrade(),
        agent,
        state: ChatState::Idle,
        title: None,
        line: None,
        session_id: resume.map(str::to_string),
        transcript: None,
        updated_at: now(),
        since: now(),
    });
    Ok(terminal)
}

/// Finds the session file of the agent in `terminal`: the one its hooks
/// name, the one it resumed, or the newest in its folder since it started.
/// The looking on disk is the returned job's, to run off the UI thread.
pub(crate) fn session_file(
    terminal: &WeakEntity<Terminal>,
    cx: &App,
) -> Option<ph_agent::cli_transcript::FindJob> {
    let terminal = terminal.upgrade()?;
    let agent = TerminalAgents::of_terminal(&terminal, cx)?;
    if let Some(path) = agent.transcript.clone() {
        return Some(Box::new(move || Some(path)));
    }
    let (agent_id, cwd) = (agent.agent.id, terminal.read(cx).cwd().to_path_buf());
    let (session_id, since) = (agent.session_id.clone(), agent.since);
    Some(Box::new(move || {
        let sessions = crate::agent_sessions::sessions(agent_id, &cwd);
        if let Some(session) = session_id
            .as_ref()
            .and_then(|id| sessions.iter().find(|session| &session.id == id))
            // A resumed session the user went on from (`/clear`) is older.
            .filter(|session| {
                !sessions
                    .iter()
                    .any(|newer| newer.updated_at > session.updated_at && newer.updated_at >= since)
            })
        {
            return session.path.clone();
        }
        // Sessions keep their file's time; a little slack for the clock.
        sessions
            .into_iter()
            .find(|session| session.updated_at >= since - 5_000)
            .and_then(|session| session.path)
    }))
}

/// `name`, or `name 2`, … : the first not taken by a terminal.
fn free_label(name: &str, terminals: &Entity<WorkspaceTerminals>, cx: &App) -> String {
    let taken: Vec<String> = terminals
        .read(cx)
        .terminals()
        .iter()
        .filter_map(|terminal| terminal.read(cx).label().map(|label| label.to_string()))
        .collect();
    (1..)
        .map(|n| match n {
            1 => name.to_string(),
            n => format!("{name} {n}"),
        })
        .find(|label| !taken.contains(label))
        .unwrap_or_else(|| name.to_string())
}

/// The hook command: this program, told where the app listens and which
/// terminal the agent runs in.
fn hook_command(pane: &str) -> anyhow::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!(
        "{} agent-hook --socket {} --pane {pane}",
        quoted(&exe),
        quoted(&paths::control_socket_path())
    ))
}

/// Claude Code settings with a hook on every event PilotHouse follows.
fn hook_settings(command: &str) -> Value {
    let hook =
        serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    let tool_hook = serde_json::json!([{ "matcher": "*", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    serde_json::json!({
        "hooks": {
            "SessionStart": hook,
            "UserPromptSubmit": hook,
            "PreToolUse": tool_hook,
            "PostToolUse": tool_hook,
            "Notification": hook,
            "Stop": hook,
            "SessionEnd": hook,
        }
    })
}

/// `path` quoted for the shell it is typed into: in single quotes, where
/// sh, zsh, bash, fish and PowerShell expand neither `$` nor backticks. A
/// quote in it is written as each reads it: `'\''` (POSIX shells, fish),
/// `''` (PowerShell). Command Prompt has no single quotes: double ones.
fn quoted(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) {
        let (shell, _) = ph_env::default_shell();
        if shell
            .file_stem()
            .is_some_and(|name| name.eq_ignore_ascii_case("cmd"))
        {
            return format!("\"{text}\"");
        }
        return format!("'{}'", text.replace('\'', "''"));
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// A hook event from agent `pane`.
fn apply(pane: &str, event: &Value, cx: &mut App) {
    let Some(agent) = cx
        .default_global::<TerminalAgents>()
        .agents
        .iter_mut()
        .find(|agent| agent.pane == pane)
    else {
        return;
    };
    if let Some(session) = event["session_id"].as_str().filter(|id| is_session_id(id)) {
        agent.session_id = Some(session.to_string());
    }
    if let Some(path) = event["transcript_path"].as_str() {
        agent.transcript = Some(PathBuf::from(path));
    }
    agent.updated_at = now();
    match event["hook_event_name"].as_str().unwrap_or_default() {
        "UserPromptSubmit" => {
            let prompt = event["prompt"].as_str().unwrap_or_default();
            if agent.title.is_none() {
                agent.title = first_line(prompt);
            }
            agent.state = ChatState::Working;
            agent.line = None;
        }
        "PreToolUse" => {
            agent.state = ChatState::Working;
            agent.line = Some(tool_line(
                event["tool_name"].as_str().unwrap_or("a tool"),
                &event["tool_input"],
            ));
        }
        "PostToolUse" => agent.state = ChatState::Working,
        "Notification" => {
            let message = event["message"].as_str().unwrap_or_default();
            // Claude also says it is idle after a while; only a question
            // needs the user.
            if !message.to_lowercase().contains("waiting for your input") {
                agent.state = ChatState::NeedsYou;
                agent.line = first_line(message);
            }
        }
        "Stop" => {
            agent.state = ChatState::Done;
            agent.line = event["last_assistant_message"]
                .as_str()
                .and_then(first_line)
                .or_else(|| agent.transcript.as_deref().and_then(last_reply))
                .or(agent.line.take());
        }
        "SessionEnd" => agent.state = ChatState::Idle,
        _ => {}
    }
    let badge = match agent.state {
        ChatState::Working => Some(AgentBadge::Working),
        ChatState::NeedsYou => Some(AgentBadge::NeedsYou),
        ChatState::Done => Some(AgentBadge::Done),
        ChatState::Failed | ChatState::Idle => None,
    };
    let terminal = agent.terminal.clone();
    _ = terminal.update(cx, |terminal, cx| terminal.set_badge(badge, cx));
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
}

/// "Bash: cargo test", "Edit: src/main.rs".
fn tool_line(tool: &str, input: &Value) -> String {
    let detail = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ]
    .iter()
    .find_map(|key| input[*key].as_str())
    .and_then(first_line);
    match detail {
        Some(detail) => format!("{tool}: {detail}"),
        None => tool.to_string(),
    }
}

/// The first line of the last reply in a Claude Code session file.
fn last_reply(path: &Path) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(TRANSCRIPT_TAIL)))
        .ok()?;
    // Bytes: the seek can land inside a character.
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    tail.lines().rev().find_map(|line| {
        let row: Value = serde_json::from_str(line).ok()?;
        if row["type"] != "assistant" {
            return None;
        }
        row["message"]["content"]
            .as_array()?
            .iter()
            .rev()
            .find_map(|block| {
                (block["type"] == "text")
                    .then(|| block["text"].as_str())
                    .flatten()
            })
            .and_then(first_line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn quotes_paths_for_the_shell() {
        assert_eq!(quoted(Path::new("/a b/$x`y`")), "'/a b/$x`y`'");
        assert_eq!(quoted(Path::new("/it's")), "'/it'\\''s'");
    }

    #[test]
    fn hooks_call_back_with_the_pane() {
        let settings = hook_settings("\"/app/pilothouse\" agent-hook --pane p1");
        let stop = &settings["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(stop["type"], "command");
        assert!(stop["command"].as_str().unwrap().contains("--pane p1"));
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "*");
    }

    #[test]
    fn reads_the_last_reply_and_tool_lines() {
        let dir = std::env::temp_dir().join(format!("ph-term-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.jsonl");
        std::fs::write(
            &file,
            [
                r#"{"type":"user","message":{"content":"hi"}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done.\nAll good."}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(last_reply(&file).as_deref(), Some("Done."));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            tool_line("Bash", &serde_json::json!({"command": "cargo test\n--all"})),
            "Bash: cargo test"
        );
        assert!(!is_session_id("x; rm -rf /"));
        assert_eq!(
            title_state("\u{25C7}  Ready (app)   "),
            Some(ChatState::Idle)
        );
        assert_eq!(
            title_state("\u{270B}  Action Required (app)"),
            Some(ChatState::NeedsYou)
        );
        assert_eq!(title_state("zsh"), None);
        assert_eq!(agent_named("agy").map(|spec| spec.id), Some("antigravity"));
    }
}
