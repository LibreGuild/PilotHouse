//! A terminal agent's own session file shown as a chat, read only: what it
//! was asked, what it said and which tools it ran, beside its terminal
//! (*Transcript* on the terminal's tab). The file is read again while it
//! grows.
//!
//! - Claude Code: `…/projects/<folder>/<session>.jsonl` rows of `user` and
//!   `assistant` messages (`text`, `tool_use` blocks).
//! - Codex: `…/sessions/…/rollout-*.jsonl`, `response_item` rows (`message`,
//!   and in older versions `function_call`, `local_shell_call`) and the
//!   commands it ran (`item_completed` events).
//! - Gemini CLI: `…/chats/session-*.jsonl`, a row per `user` or `gemini`
//!   message (written again by `id` as it grows, with its `toolCalls`), and
//!   `$rewindTo` rows that take messages back.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Task,
    Window, div, px,
};
use serde_json::Value;

use crate::agents::AgentSpec;
use crate::thread::{ChatThread, ThreadProject};
use crate::transcript::{Entry, ToolEntry, ToolKindLabel, ToolStatus};

/// Whose session file it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Claude,
    Codex,
    Gemini,
}

impl Format {
    /// The format of agent `id`'s session files, when it keeps readable ones.
    pub fn of(agent_id: &str) -> Option<Self> {
        match agent_id {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}

/// Finds the session file to show (a new one can start in the same
/// terminal, after `/clear`): what it needs from the app, then a job that
/// looks on disk, off the UI thread.
pub type FindSession = Box<dyn Fn(&gpui_kit::App) -> Option<FindJob>>;
pub type FindJob = Box<dyn FnOnce() -> Option<PathBuf> + Send>;

/// How often the session file is looked for again.
const FIND_EVERY: u32 = 10;

/// The last bytes of a session file read (long sessions keep their end).
const READ: u64 = 4 * 1024 * 1024;

/// One step of the history, before it becomes a transcript entry.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Step {
    User(String),
    Agent(String),
    Tool(String),
}

/// The steps a session file holds.
#[cfg(test)]
pub(crate) fn parse(format: Format, text: &str) -> Vec<Step> {
    let mut parsed = Parsed::default();
    parsed.feed(format, text);
    parsed.steps(format)
}

/// What the rows of a session file read so far say. Rows can come a few
/// at a time (the file grows): each is read once.
#[derive(Default)]
pub(crate) struct Parsed {
    steps: Vec<Step>,
    /// Gemini CLI's messages: rows write them again by id as they grow,
    /// and take them back (`$rewindTo`), so steps come from the last state.
    messages: Vec<Value>,
}

impl Parsed {
    /// Reads whole lines of JSON rows.
    pub(crate) fn feed(&mut self, format: Format, text: &str) {
        for row in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            match format {
                Format::Claude => claude_row(&mut self.steps, &row),
                Format::Codex => codex_row(&mut self.steps, &row),
                Format::Gemini => gemini_row(&mut self.messages, row),
            }
        }
    }

    pub(crate) fn steps(&self, format: Format) -> Vec<Step> {
        match format {
            Format::Gemini => gemini_steps(&self.messages),
            _ => self.steps.clone(),
        }
    }
}

fn claude_row(steps: &mut Vec<Step>, row: &Value) {
    let content = &row["message"]["content"];
    match row["type"].as_str() {
        Some("user") => match content {
            Value::String(text) => push_user(steps, text),
            Value::Array(blocks) => {
                for block in blocks {
                    if block["type"] == "text"
                        && let Some(text) = block["text"].as_str()
                    {
                        push_user(steps, text);
                    }
                }
            }
            _ => {}
        },
        Some("assistant") => {
            for block in content.as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => push_agent(steps, block["text"].as_str()),
                    Some("tool_use") => steps.push(Step::Tool(tool_title(
                        block["name"].as_str().unwrap_or("tool"),
                        &block["input"],
                    ))),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn codex_row(steps: &mut Vec<Step>, row: &Value) {
    if row["type"] == "event_msg"
        && row["payload"]["type"] == "item_completed"
        && row["payload"]["item"]["type"] == "CommandExecution"
    {
        let parts: Vec<&str> = row["payload"]["item"]["command"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        // `zsh -lc "<command>"`: the command is what was asked.
        let command = match parts.as_slice() {
            [_, flag, command] if flag.ends_with('c') => command.to_string(),
            parts => parts.join(" "),
        };
        steps.push(Step::Tool(tool_title(
            "shell",
            &serde_json::json!({ "command": command }),
        )));
        return;
    }
    if row["type"] != "response_item" {
        return;
    }
    let payload = &row["payload"];
    match payload["type"].as_str() {
        Some("message") => {
            let text: String = payload["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            match payload["role"].as_str() {
                Some("user") => push_user(steps, &text),
                Some("assistant") => push_agent(steps, Some(&text)),
                _ => {}
            }
        }
        Some("function_call") => {
            let arguments = payload["arguments"]
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .unwrap_or(Value::Null);
            steps.push(Step::Tool(tool_title(
                payload["name"].as_str().unwrap_or("tool"),
                &arguments,
            )))
        }
        Some("local_shell_call") => {
            let command = payload["action"]["command"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            steps.push(Step::Tool(format!("shell: {command}")))
        }
        _ => {}
    }
}

fn gemini_row(messages: &mut Vec<Value>, row: Value) {
    if let Some(id) = row["$rewindTo"].as_str() {
        if let Some(at) = messages.iter().position(|message| message["id"] == id) {
            messages.truncate(at);
        }
    } else if row["type"].is_string() && row["id"].is_string() {
        match messages
            .iter_mut()
            .find(|message| message["id"] == row["id"])
        {
            Some(message) => *message = row,
            None => messages.push(row),
        }
    }
}

fn gemini_steps(messages: &[Value]) -> Vec<Step> {
    let mut steps = Vec::new();
    for message in messages {
        let text = match &message["content"] {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        match message["type"].as_str() {
            Some("user") => push_user(&mut steps, &text),
            Some("gemini") => {
                for call in message["toolCalls"].as_array().into_iter().flatten() {
                    steps.push(Step::Tool(tool_title(
                        call["name"].as_str().unwrap_or("tool"),
                        &call["args"],
                    )));
                }
                push_agent(&mut steps, Some(&text));
            }
            _ => {}
        }
    }
    steps
}

/// A user message, unless it is context the agent put there.
fn push_user(steps: &mut Vec<Step>, text: &str) {
    let mut text = text.trim();
    // PilotHouse's own note ahead of a chat's first message.
    if text.starts_with("(Workspace note from PilotHouse") {
        text = text.split_once("\n\n").map_or("", |(_, rest)| rest.trim());
    }
    if text.is_empty()
        || text.starts_with('<')
        || text.starts_with("# AGENTS.md")
        || text.starts_with("Here is a list of")
    {
        return;
    }
    steps.push(Step::User(text.to_string()));
}

fn push_agent(steps: &mut Vec<Step>, text: Option<&str>) {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return;
    };
    // Streamed pieces of one reply join up.
    if let Some(Step::Agent(known)) = steps.last_mut() {
        known.push_str("\n\n");
        known.push_str(text);
    } else {
        steps.push(Step::Agent(text.to_string()));
    }
}

/// "Bash: cargo test", "Edit: src/main.rs".
fn tool_title(tool: &str, input: &Value) -> String {
    let detail = [
        "command",
        "cmd",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ]
    .iter()
    .find_map(|key| match &input[*key] {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    })
    .and_then(|text| {
        text.lines()
            .next()
            .map(|line| line.chars().take(160).collect::<String>())
    });
    match detail {
        Some(detail) => format!("{tool}: {detail}"),
        None => tool.to_string(),
    }
}

/// A terminal agent's session, as a read-only chat.
pub struct TranscriptView {
    thread: Entity<ChatThread>,
    find: FindSession,
    path: Option<PathBuf>,
    /// Reads since the file was last looked for.
    reads: u32,
    /// How far the file is read; off with a read, back after it.
    tail: Option<Tail>,
    /// What is shown, as read; a new read only redraws from where it
    /// differs.
    steps: Vec<Step>,
    _poll: Task<()>,
}

impl TranscriptView {
    pub fn new(
        store: ph_store::Store,
        project: &ph_store::projects::Project,
        agent: &'static AgentSpec,
        format: Format,
        find: FindSession,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = ThreadProject::of(project);
        let thread = cx.new(|cx| ChatThread::draft(store, project, agent, cx));
        // Read again while the session grows; the file is found, read and
        // parsed off the UI thread, each time only what was added.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok((find, tail)) = this.update(cx, |this, cx| {
                    let find = (this.path.is_none() || this.reads >= FIND_EVERY)
                        .then(|| (this.find)(cx))
                        .flatten();
                    if find.is_some() {
                        this.reads = 0;
                    }
                    this.reads += 1;
                    let tail = this.tail.take().unwrap_or_default();
                    (find, tail)
                }) else {
                    return;
                };
                let (path, tail, changed) = cx
                    .background_spawn(async move {
                        let mut tail = tail;
                        let found = find.and_then(|find| find());
                        let path = match found {
                            Some(found) if Some(&found) != tail.path.as_ref() => {
                                tail = Tail {
                                    path: Some(found),
                                    ..Tail::default()
                                };
                                tail.path.clone()
                            }
                            _ => tail.path.clone(),
                        };
                        let changed = tail.read_more(format);
                        (path, tail, changed)
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.path = path;
                        let steps = changed.then(|| tail.parsed.steps(format));
                        this.tail = Some(tail);
                        if let Some(steps) = steps {
                            this.show(steps, cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            thread,
            find,
            path: None,
            reads: 0,
            tail: None,
            steps: Vec::new(),
            _poll: poll,
        }
    }

    /// Shows `steps`, redrawing only from the first that changed.
    fn show(&mut self, steps: Vec<Step>, cx: &mut Context<Self>) {
        let keep = self
            .steps
            .iter()
            .zip(&steps)
            .take_while(|(old, new)| old == new)
            .count();
        if keep == steps.len() && keep == self.steps.len() {
            return;
        }
        let entries: Vec<Entry> = steps[keep..]
            .iter()
            .enumerate()
            .map(|(at, step)| step.clone().into_entry(keep + at))
            .collect();
        self.steps = steps;
        self.thread
            .update(cx, |thread, cx| thread.show_history(keep, entries, cx));
        cx.notify();
    }
}

/// A session file read so far: where, how far, and what it said.
#[derive(Default)]
struct Tail {
    path: Option<PathBuf>,
    /// Bytes read (whole lines; a line still being written waits).
    offset: u64,
    parsed: Parsed,
}

impl Tail {
    /// Reads what was added since the last time; whether anything was.
    /// A file that got shorter (written anew) is read from its start. The
    /// first read of a long one starts `READ` bytes from its end. Blocking.
    fn read_more(&mut self, format: Format) -> bool {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let Some(path) = self.path.clone() else {
            return false;
        };
        let Ok(length) = std::fs::metadata(&path).map(|meta| meta.len()) else {
            return false;
        };
        if length < self.offset {
            *self = Tail {
                path: Some(path.clone()),
                ..Tail::default()
            };
        }
        if length == self.offset {
            return false;
        }
        let Ok(mut file) = std::fs::File::open(&path) else {
            return false;
        };
        let mut start = self.offset;
        let mut skip_first = false;
        if start == 0 && length > READ {
            start = length - READ;
            skip_first = true;
        }
        if file.seek(SeekFrom::Start(start)).is_err() {
            return false;
        }
        let mut bytes = Vec::new();
        if file.take(length - start).read_to_end(&mut bytes).is_err() {
            return false;
        }
        // Up to the last whole line; the rest is read again next time.
        let whole = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        let mut text = String::from_utf8_lossy(&bytes[..whole]).into_owned();
        if skip_first {
            // A line cut by the start would not parse anyway.
            text = text
                .split_once('\n')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_default();
        }
        self.offset = start + whole as u64;
        self.parsed.feed(format, &text);
        whole > 0
    }
}

impl Step {
    /// The entry of step `ix` (its id keeps tool runs apart).
    fn into_entry(self, ix: usize) -> Entry {
        match self {
            Step::User(text) => Entry::User {
                text,
                attachments: Vec::new(),
                checkpoint: None,
            },
            Step::Agent(text) => Entry::Agent { text, view: None },
            Step::Tool(title) => Entry::Tool(ToolEntry {
                id: format!("cli-{ix}"),
                title,
                kind: ToolKindLabel::Other,
                status: ToolStatus::Done,
                output: String::new(),
                diffs: Vec::new(),
                children: Vec::new(),
                agent_text: String::new(),
                expanded: false,
                started: None,
                touched: None,
            }),
        }
    }
}

impl Render for TranscriptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let thread = self.thread.clone();
        if thread.read(cx).entries().is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .child("Nothing in this session yet.")
                .into_any_element();
        }
        // Rows are measured again when the view mode changed (as in a chat).
        let mode = crate::view_mode::ViewMode::current(cx);
        thread.update(cx, |thread, cx| thread.set_view_mode(mode, cx));
        let scroller = thread.read(cx).scroller().clone();
        let mut row_style = gpui_kit::StyleRefinement::default();
        row_style.padding.bottom = Some(px(0.).into());
        MessageScroller::new("cli-transcript", scroller, move |ix, _, cx| {
            thread.update(cx, |thread, cx| thread.prepare_entry(ix, cx));
            crate::entry_view::render_entry(&thread, ix, cx)
        })
        .with_row_style(row_style)
        .size_full()
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_sessions() {
        let text = [
            r#"{"type":"user","message":{"content":"Fix the bug"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed."}]}}"#,
        ]
        .join("\n");
        assert_eq!(
            parse(Format::Claude, &text),
            vec![
                Step::User("Fix the bug".into()),
                Step::Agent("Looking.".into()),
                Step::Tool("Bash: cargo test".into()),
                Step::Agent("Fixed.".into()),
            ]
        );
    }

    #[test]
    fn reads_codex_and_gemini_sessions() {
        let codex = [
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"List files"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\",\"-la\"]}"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Two files."}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","command":["/bin/zsh","-lc","cargo test"]}}}"#,
        ]
        .join("\n");
        assert_eq!(
            parse(Format::Codex, &codex),
            vec![
                Step::User("List files".into()),
                Step::Tool("shell: ls -la".into()),
                Step::Agent("Two files.".into()),
                Step::Tool("shell: cargo test".into()),
            ]
        );
        let gemini = [
            r#"{"sessionId":"s","kind":"main"}"#,
            r#"{"id":"0","type":"user","content":[{"text":"<session_context>x"}]}"#,
            r#"{"id":"1","type":"user","content":[{"text":"Hi"}]}"#,
            r#"{"$set":{"lastUpdated":"now"}}"#,
            r#"{"id":"2","type":"gemini","content":""}"#,
            r#"{"id":"2","type":"gemini","content":"Hello","toolCalls":[{"name":"read_file","args":{"file_path":"a.rs"}}]}"#,
            r#"{"id":"3","type":"user","content":[{"text":"Cancelled one"}]}"#,
            r#"{"id":"4","type":"gemini","content":"Never mind"}"#,
            r#"{"$rewindTo":"3"}"#,
        ]
        .join("\n");
        assert_eq!(
            parse(Format::Gemini, &gemini),
            vec![
                Step::User("Hi".into()),
                Step::Tool("read_file: a.rs".into()),
                Step::Agent("Hello".into()),
            ]
        );
    }

    #[test]
    fn reads_only_what_was_added() {
        use std::io::Write as _;
        let path = std::env::temp_dir().join(format!("ph-transcript-{}.jsonl", std::process::id()));
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"One\"}}\n",
        )
        .unwrap();
        let mut tail = Tail {
            path: Some(path.clone()),
            ..Tail::default()
        };
        assert!(tail.read_more(Format::Claude));
        assert!(!tail.read_more(Format::Claude), "nothing new");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        // A line still being written waits for its end.
        write!(file, "{{\"type\":\"user\",\"message\":").unwrap();
        assert!(!tail.read_more(Format::Claude));
        writeln!(file, "{{\"content\":\"Two\"}}}}").unwrap();
        assert!(tail.read_more(Format::Claude));
        assert_eq!(
            tail.parsed.steps(Format::Claude),
            vec![Step::User("One".into()), Step::User("Two".into())]
        );
        // Written anew (shorter): read from the start.
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"New\"}}\n",
        )
        .unwrap();
        assert!(tail.read_more(Format::Claude));
        assert_eq!(
            tail.parsed.steps(Format::Claude),
            vec![Step::User("New".into())]
        );
        let _ = std::fs::remove_file(&path);
    }
}
