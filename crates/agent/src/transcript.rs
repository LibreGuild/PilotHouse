//! The conversation model: what a chat shows, built from ACP updates.
//!
//! Entries are plain data except agent replies, which keep a markdown view
//! state so streamed chunks are appended without re-parsing the whole reply.
//! Each entry serialises to a `(role, body)` pair for the store.

use agent_client_protocol_schema::v1::{
    ContentBlock, PlanEntryStatus, ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate,
    ToolKind,
};
use gpui_kit::{App, AppContext as _, Entity};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::connection::SignInMethod;
use crate::permission::PermissionRequest;
use crate::review_card::ReviewCard;

/// Diff lines shown before "… N more lines" in a tool card.
const MAX_DIFF_LINES: usize = 80;
/// A tool's output is kept to its first lines (the card shows about 20): a
/// long agent run reads thousands of files, and keeping each whole held
/// hundreds of MB for text no one sees.
const OUTPUT_LINES: usize = 60;
const OUTPUT_BYTES: usize = 8_000;
/// The most pictures of one tool call that are kept.
const MAX_TOOL_IMAGES: usize = 8;
/// Plans and other calls whose text is the point (a plan to approve, a
/// subagent's answer) keep up to this much.
const WHOLE_OUTPUT_BYTES: usize = 64_000;
/// A command's output keeps its first lines and, above all, its last: a
/// test run or a build says what went wrong at its end, and that is what a
/// reader opens the call for.
const COMMAND_HEAD_LINES: usize = 20;
const COMMAND_TAIL_LINES: usize = 200;
const COMMAND_BYTES: usize = 32_000;

pub enum Entry {
    User {
        text: String,
        /// What was attached (files, cards, screenshots, logs), for display.
        attachments: Vec<AttachmentSummary>,
        /// The files as they were before this message (a Git checkpoint
        /// commit), to restore them.
        checkpoint: Option<String>,
    },
    Agent {
        text: String,
        /// The rendered reply (Markdown and diagrams), made when it is
        /// first shown: a long stored chat opens without parsing replies no
        /// one scrolls to.
        view: Option<Entity<crate::reply::Reply>>,
    },
    /// Files were put back to a checkpoint; `undo` reverses it.
    Restored {
        text: String,
        undo: Option<String>,
    },
    Thought {
        text: String,
        expanded: bool,
    },
    Tool(ToolEntry),
    Plan(Vec<PlanItem>),
    Permission(Entity<PermissionRequest>),
    Notice {
        text: String,
        is_error: bool,
    },
    /// The agent needs a login; offers the ways it can sign in itself.
    SignIn {
        message: String,
        methods: Vec<SignInMethod>,
    },
    /// A PR review's findings, as a card.
    Review {
        review_id: i64,
        card: Entity<ReviewCard>,
    },
}

/// How an attachment is shown in the user's message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentSummary {
    pub label: String,
    /// A screenshot on disk, shown as a thumbnail.
    #[serde(default)]
    pub image: Option<std::path::PathBuf>,
    /// The text that was sent (log lines, a card), shown on click.
    #[serde(default)]
    pub text: Option<String>,
    /// The file that was attached, read when shown.
    #[serde(default)]
    pub file: Option<std::path::PathBuf>,
}

impl AttachmentSummary {
    pub fn preview(&self) -> crate::attachment_view::Preview {
        use crate::attachment_view::Preview;
        match (&self.image, &self.text, &self.file) {
            (Some(image), _, _) => Preview::Image(image.clone()),
            (_, Some(text), _) => Preview::Text(text.clone()),
            (_, _, Some(file)) => Preview::File(file.clone()),
            _ => Preview::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolEntry {
    pub id: String,
    pub title: String,
    pub kind: ToolKindLabel,
    pub status: ToolStatus,
    /// Text output of the tool (already trimmed for display).
    pub output: String,
    pub diffs: Vec<FileDiff>,
    /// A subagent's own tool calls, when this call started one (Claude
    /// Code's Agent/Task tool).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ToolEntry>,
    /// What the subagent said, newest last (cut to the last few KB).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_text: String,
    /// How a command went, when the agent says (its exit code, and what it
    /// printed while it ran).
    #[serde(default, skip_serializing_if = "CommandRun::is_unknown")]
    pub run: CommandRun,
    /// The pictures the tool gave back (a screenshot), saved as files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<std::path::PathBuf>,
    #[serde(skip)]
    pub expanded: bool,
    /// When the call started, if in this run of the app.
    #[serde(skip)]
    pub started: Option<std::time::Instant>,
    /// When its subagent last did something, if in this run of the app.
    #[serde(skip)]
    pub touched: Option<std::time::Instant>,
}

/// What an agent reports of a command beside the tool call itself: Codex
/// sends what the command prints as it prints it, and both Codex and Claude
/// Code its exit code (`_meta.terminal_output_delta`, `terminal_output` and
/// `terminal_exit` on a tool call's updates, the convention editors share).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandRun {
    /// The exit code, once the command ended and the agent knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// What the command printed so far, as it came. Its newest part is held
    /// (see [`STREAM_BYTES`]); `output` is drawn from this.
    #[serde(skip)]
    stream: String,
    /// The lines that fell off the start of `stream`.
    #[serde(skip)]
    fell_off: usize,
}

impl CommandRun {
    fn is_unknown(&self) -> bool {
        self.exit.is_none()
    }
}

/// How much of a running command's output is held to draw from, at least
/// (up to twice this between cuts): more than is drawn, so that the lines a
/// progress bar rewrote are still whole.
const STREAM_BYTES: usize = 64_000;

/// How long a subagent counts as at work after its last step (Claude runs
/// subagents in the background: their call ends as they start).
const SUBAGENT_QUIET: std::time::Duration = std::time::Duration::from_secs(10);

/// How much of a subagent's text is kept.
const SUBAGENT_TEXT: usize = 4_000;

/// What a tool does, reduced to the cases the UI distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKindLabel {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    Running,
    Done,
    Failed,
}

/// One file's change, as a compact unified diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    /// `+`/`-`/` ` prefixed lines, capped at [`MAX_DIFF_LINES`].
    pub lines: Vec<String>,
    pub hidden_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanItem {
    pub content: String,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

impl From<&ToolKind> for ToolKindLabel {
    fn from(kind: &ToolKind) -> Self {
        match kind {
            ToolKind::Read => Self::Read,
            ToolKind::Edit => Self::Edit,
            ToolKind::Delete => Self::Delete,
            ToolKind::Move => Self::Move,
            ToolKind::Search => Self::Search,
            ToolKind::Execute => Self::Execute,
            ToolKind::Think => Self::Think,
            ToolKind::Fetch => Self::Fetch,
            _ => Self::Other,
        }
    }
}

impl From<&ToolCallStatus> for ToolStatus {
    fn from(status: &ToolCallStatus) -> Self {
        match status {
            ToolCallStatus::Pending => Self::Pending,
            ToolCallStatus::InProgress => Self::Running,
            ToolCallStatus::Completed => Self::Done,
            ToolCallStatus::Failed => Self::Failed,
            _ => Self::Running,
        }
    }
}

/// A tool title that is the call dumped as JSON (Cline's `editor:
/// {"path": …, "new_text": …}`) said in words: what it does, to what
/// ("Edit src/app.rs"). Other titles stay as the agent wrote them.
pub(crate) fn readable_title(
    title: &str,
    kind: ToolKindLabel,
    raw_input: Option<&serde_json::Value>,
) -> String {
    use serde_json::Value;
    let Some((name, rest)) = title.split_once(": ") else {
        return title.to_string();
    };
    let rest = rest.trim_start();
    if name.is_empty()
        || name.contains(char::is_whitespace)
        || !(rest.starts_with("{\"") || rest.starts_with('['))
    {
        return title.to_string();
    }
    // The call's input beside it, else the dump (parsed only then: it can
    // hold a whole file, and titles come with each update of the call).
    let parsed;
    let input = match raw_input {
        Some(input) => input,
        None => {
            parsed = serde_json::from_str::<Value>(rest).ok();
            let Some(input) = parsed.as_ref() else {
                return title.to_string();
            };
            input
        }
    };
    let input = Some(input);
    // A string, the first of a list, or the path of the first item.
    fn named(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Array(items) => named(items.first()?),
            Value::Object(item) => ["path", "file_path", "file"]
                .iter()
                .find_map(|key| item.get(*key)?.as_str().map(str::to_string)),
            _ => None,
        }
    }
    const KEYS: [&str; 11] = [
        "path",
        "file_path",
        "filePath",
        "file",
        "paths",
        "files",
        "command",
        "commands",
        "pattern",
        "query",
        "url",
    ];
    let target = input.and_then(|input| KEYS.iter().find_map(|key| named(input.get(*key)?)));
    let verb = match kind {
        ToolKindLabel::Read => "Read".to_string(),
        ToolKindLabel::Edit => "Edit".to_string(),
        ToolKindLabel::Delete => "Delete".to_string(),
        ToolKindLabel::Move => "Move".to_string(),
        ToolKindLabel::Search => "Search".to_string(),
        ToolKindLabel::Execute => "Run".to_string(),
        ToolKindLabel::Fetch => "Fetch".to_string(),
        _ => name.replace('_', " "),
    };
    match target {
        Some(target) => {
            let line = target.lines().next().unwrap_or_default();
            let count = line.chars().count();
            // A long path keeps its end (the file); anything else its start.
            let short: String = if count <= 120 {
                line.to_string()
            } else if line.contains('/') || line.contains('\\') {
                let tail: String = line.chars().skip(count - 119).collect();
                format!("…{tail}")
            } else {
                line.chars().take(120).collect()
            };
            format!("{verb} {short}")
        }
        None => verb,
    }
}

impl ToolEntry {
    /// The title for display: MCP tools arrive as `mcp__server__tool_name`,
    /// shown as "tool name · server".
    pub fn display_title(&self) -> String {
        match self
            .title
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
        {
            Some((server, tool)) => format!("{} · {server}", tool.replace('_', " ")),
            None => self.title.clone(),
        }
    }

    /// An empty entry, for building one field by field.
    pub fn placeholder() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            kind: ToolKindLabel::Other,
            status: ToolStatus::Pending,
            output: String::new(),
            diffs: Vec::new(),
            children: Vec::new(),
            agent_text: String::new(),
            run: CommandRun::default(),
            images: Vec::new(),
            expanded: false,
            started: None,
            touched: None,
        }
    }

    /// Whether the call started a subagent (Claude Code's Agent tool, Codex's
    /// `spawnAgent`): it has steps or words of its own, or is a running
    /// "think" call, which is how Claude's starts.
    pub fn is_subagent(&self) -> bool {
        !self.children.is_empty()
            || !self.agent_text.is_empty()
            || (self.kind == ToolKindLabel::Think && self.is_running())
            // Codex names its call; its subagent's own steps stay unseen.
            || self.title == "spawnAgent"
    }

    /// A subagent's latest step: its newest tool call, else the last line
    /// it said.
    pub fn last_step(&self) -> Option<String> {
        self.children
            .last()
            .map(ToolEntry::display_title)
            .or_else(|| {
                self.agent_text
                    .lines()
                    .rev()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(str::to_string)
            })
    }

    /// Whether the call is still going (it, or a subagent step under it).
    pub fn is_running(&self) -> bool {
        matches!(self.status, ToolStatus::Pending | ToolStatus::Running)
    }

    /// Whether its subagent is at work: the call or one of its steps runs,
    /// or it did something moments ago. Only meaningful while the turn
    /// goes on.
    pub fn is_busy(&self) -> bool {
        self.is_running()
            || self.children.iter().any(ToolEntry::is_running)
            || self
                .touched
                .is_some_and(|touched| touched.elapsed() < SUBAGENT_QUIET)
    }

    /// Adds a subagent's tool call, or replaces it when known.
    pub fn child_call(&mut self, call: &ToolCall) {
        self.touched = Some(std::time::Instant::now());
        let child = Self::from_call(call);
        match self.children.iter_mut().find(|known| known.id == child.id) {
            Some(known) => *known = child,
            None => self.children.push(child),
        }
    }

    pub fn child_update(&mut self, update: &ToolCallUpdate) {
        self.touched = Some(std::time::Instant::now());
        if let Some(child) = self
            .children
            .iter_mut()
            .find(|child| child.id == update.tool_call_id.0.as_ref())
        {
            child.apply(update);
        }
    }

    /// Appends what the subagent said.
    pub fn push_agent_text(&mut self, text: &str) {
        self.touched = Some(std::time::Instant::now());
        self.agent_text.push_str(text);
        if self.agent_text.len() > SUBAGENT_TEXT {
            let mut cut = self.agent_text.len() - SUBAGENT_TEXT;
            while !self.agent_text.is_char_boundary(cut) {
                cut += 1;
            }
            self.agent_text.drain(..cut);
        }
    }

    pub fn from_call(call: &ToolCall) -> Self {
        let mut entry = Self {
            id: call.tool_call_id.0.to_string(),
            title: readable_title(&call.title, (&call.kind).into(), call.raw_input.as_ref()),
            kind: (&call.kind).into(),
            status: (&call.status).into(),
            output: String::new(),
            diffs: Vec::new(),
            children: Vec::new(),
            agent_text: String::new(),
            run: CommandRun::default(),
            images: Vec::new(),
            expanded: false,
            started: Some(std::time::Instant::now()),
            touched: None,
        };
        entry.set_content(&call.content);
        entry.take_report(call.meta.as_ref());
        entry
    }

    pub fn apply(&mut self, update: &ToolCallUpdate) {
        let fields = &update.fields;
        if let Some(kind) = &fields.kind {
            self.kind = kind.into();
        }
        if let Some(title) = &fields.title {
            self.title = readable_title(title, self.kind, fields.raw_input.as_ref());
        }
        if let Some(status) = &fields.status {
            self.status = status.into();
        }
        if let Some(content) = &fields.content {
            self.set_content(content);
        }
        self.take_report(update.meta.as_ref());
    }

    /// Takes what the agent reports of a command beside its tool call: the
    /// next piece of what it printed, and how it ended.
    fn take_report(&mut self, meta: Option<&agent_client_protocol_schema::v1::Meta>) {
        let Some(meta) = meta else {
            return;
        };
        for key in ["terminal_output_delta", "terminal_output"] {
            if let Some(chunk) = meta.get(key).and_then(|output| output["data"].as_str()) {
                self.push_output(chunk);
            }
        }
        if let Some(code) = meta
            .get("terminal_exit")
            .and_then(|exit| exit["exit_code"].as_i64())
        {
            self.run.exit = i32::try_from(code).ok();
        }
    }

    /// Adds `chunk` to what a running command printed, and draws the output
    /// again from all that is held.
    pub fn push_output(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let run = &mut self.run;
        run.stream.push_str(chunk);
        // Cut back only now and then, to half: every piece draws the output
        // again, and that must cost the same however much was printed.
        if run.stream.len() > 2 * STREAM_BYTES {
            // Whole lines fall off the start; a single endless line is cut
            // where a character starts.
            let mut cut = run.stream.len() - STREAM_BYTES;
            cut = match run.stream[cut..].find('\n') {
                Some(at) => cut + at + 1,
                None => {
                    while !run.stream.is_char_boundary(cut) {
                        cut += 1;
                    }
                    cut
                }
            };
            run.fell_off += run.stream[..cut].matches('\n').count();
            run.stream.drain(..cut);
        }
        self.output = ends(
            &plain(&run.stream),
            COMMAND_HEAD_LINES,
            COMMAND_TAIL_LINES,
            COMMAND_BYTES,
            run.fell_off,
        );
    }

    fn set_content(&mut self, content: &[ToolCallContent]) {
        let text = content_text(content);
        // Content without text (Claude Code names the terminal it reported
        // the output of) is not the command printing nothing.
        if text.is_empty() && !self.run.stream.is_empty() {
            return self.set_diffs(content);
        }
        self.output = match self.kind {
            // What a command printed, as a terminal would have left it,
            // with its end kept.
            ToolKindLabel::Execute => {
                // The whole output replaces what came in pieces.
                self.run.stream.clear();
                self.run.fell_off = 0;
                ends(
                    &plain(&text),
                    COMMAND_HEAD_LINES,
                    COMMAND_TAIL_LINES,
                    COMMAND_BYTES,
                    0,
                )
            }
            ToolKindLabel::Think | ToolKindLabel::Other => {
                head(&text, usize::MAX, WHOLE_OUTPUT_BYTES)
            }
            // Reads and searches are what grows: keep their start.
            _ => head(&text, OUTPUT_LINES, OUTPUT_BYTES),
        };
        self.set_diffs(content);
    }

    /// Keeps the pictures in a tool's result (a screenshot tool gives one
    /// back) as files beside the app's other captures, to show with the
    /// call. Each is written once: a call's content comes with every
    /// update of it.
    fn keep_images(&mut self, content: &[ToolCallContent]) {
        self.keep_images_in(content, &ph_store::paths::data_dir().join("captures"));
    }

    /// [`Self::keep_images`], in folder `dir`.
    fn keep_images_in(&mut self, content: &[ToolCallContent], dir: &std::path::Path) {
        let pictures = content.iter().filter_map(|item| match item {
            ToolCallContent::Content(content) => match &content.content {
                ContentBlock::Image(image) => Some(image),
                _ => None,
            },
            _ => None,
        });
        // A tool's id may hold anything; a file's name must not.
        let name: String = self
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(48)
            .collect();
        let mut images = Vec::new();
        for (ix, image) in pictures.enumerate().take(MAX_TOOL_IMAGES) {
            let extension = match image.mime_type.as_str() {
                "image/png" => "png",
                "image/jpeg" | "image/jpg" => "jpg",
                "image/webp" => "webp",
                "image/gif" => "gif",
                // Not one the app draws.
                _ => continue,
            };
            let path = dir.join(format!("tool-{name}-{ix}.{extension}"));
            if !path.is_file() {
                let Some(bytes) = crate::attachments::unbase64(&image.data) else {
                    continue;
                };
                if std::fs::create_dir_all(dir).is_err() || std::fs::write(&path, bytes).is_err() {
                    continue;
                }
            }
            images.push(path);
        }
        if !images.is_empty() {
            self.images = images;
        }
    }

    fn set_diffs(&mut self, content: &[ToolCallContent]) {
        self.keep_images(content);
        self.diffs = content
            .iter()
            .filter_map(|item| match item {
                ToolCallContent::Diff(diff) => Some(FileDiff::new(
                    &diff.path.to_string_lossy(),
                    diff.old_text.as_deref().unwrap_or_default(),
                    &diff.new_text,
                )),
                _ => None,
            })
            .collect();
    }
}

/// The text parts of a tool's content, whole.
pub fn content_text(content: &[ToolCallContent]) -> String {
    let mut output = String::new();
    for item in content {
        if let ToolCallContent::Content(content) = item
            && let Some(text) = block_text(&content.content)
        {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&text);
        }
    }
    output
}

/// The start of `output`, at most `lines` lines and `bytes`, with a note of
/// what was left out.
fn head(output: &str, lines: usize, bytes: usize) -> String {
    let mut end = output
        .match_indices('\n')
        .nth(lines.saturating_sub(1))
        .map_or(output.len(), |(at, _)| at)
        .min(bytes);
    if end == output.len() {
        return output.to_string();
    }
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    let left = output[end..]
        .lines()
        .filter(|line| !line.is_empty())
        .count();
    format!("{}\n… {left} more lines", output[..end].trim_end())
}

/// The start and the end of `output`: at most `first` lines, then a note of
/// how many were left out, then the last `last` lines, within `bytes` (the
/// end is what is kept when that is too much). `before` lines are already
/// gone from the start of `output`: then there is no start to keep, and the
/// note counts them too.
fn ends(output: &str, first: usize, last: usize, bytes: usize, before: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let first = if before > 0 { 0 } else { first };
    if before == 0 && lines.len() <= first + last && output.len() <= bytes {
        return output.to_string();
    }
    // The end comes first: the start is what is left before it.
    let tail_from = lines.len().saturating_sub(last);
    let head = &lines[..first.min(tail_from)];
    let mut tail = &lines[tail_from..];
    let head_bytes: usize = head.iter().map(|line| line.len() + 1).sum();
    // Too much even so: lines come off the tail's start, never its end.
    let room = bytes.saturating_sub(head_bytes.min(bytes / 4));
    while tail.len() > 1 && tail.iter().map(|line| line.len() + 1).sum::<usize>() > room {
        tail = &tail[1..];
    }
    let head: Vec<&str> = head
        .iter()
        .scan(0usize, |used, line| {
            *used += line.len() + 1;
            (*used <= bytes / 4).then_some(*line)
        })
        .collect();
    let left = before + lines.len() - head.len() - tail.len();
    let mut kept = String::with_capacity(bytes.min(output.len()) + 48);
    for line in &head {
        kept.push_str(line);
        kept.push('\n');
    }
    if left > 0 {
        kept.push_str(&format!(
            "… {left} line{} left out …\n",
            if left == 1 { "" } else { "s" }
        ));
    }
    for (ix, line) in tail.iter().enumerate() {
        // One line longer than everything allowed keeps its end.
        if line.len() > room {
            let mut from = line.len() - room;
            while !line.is_char_boundary(from) {
                from += 1;
            }
            kept.push('…');
            kept.push_str(&line[from..]);
        } else {
            kept.push_str(line);
        }
        if ix + 1 < tail.len() || output.ends_with('\n') {
            kept.push('\n');
        }
    }
    kept
}

/// `output` as a terminal would have left it on screen, without the
/// terminal: colour and cursor sequences are taken out, and a line a
/// program rewrote in place (a progress bar's carriage returns) keeps its
/// last state. A chat draws text, not escape codes.
pub(crate) fn plain(output: &str) -> String {
    let mut text = String::with_capacity(output.len());
    let mut chars = output.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            text.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final letter.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to a bell or the string terminator.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next_if_eq(&'\\');
                        break;
                    }
                }
            }
            // A two-character sequence (charset, keypad mode…).
            Some('(' | ')' | '#') => {
                chars.next();
            }
            _ => {}
        }
    }
    if !text.contains('\r') {
        return text;
    }
    let mut shown = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        // `\r\n` is a line's end; a `\r` inside it starts the line again.
        let body = body.strip_suffix('\r').unwrap_or(body);
        shown.push_str(body.rsplit('\r').next().unwrap_or(body));
        shown.push_str(end);
    }
    shown
}

impl FileDiff {
    pub fn new(path: &str, old: &str, new: &str) -> Self {
        let diff = similar::TextDiff::from_lines(old, new);
        let mut lines = Vec::new();
        let (mut added, mut removed) = (0, 0);
        for group in diff.grouped_ops(2) {
            for op in group {
                for change in diff.iter_changes(&op) {
                    let sign = match change.tag() {
                        similar::ChangeTag::Insert => {
                            added += 1;
                            '+'
                        }
                        similar::ChangeTag::Delete => {
                            removed += 1;
                            '-'
                        }
                        similar::ChangeTag::Equal => ' ',
                    };
                    lines.push(format!("{sign}{}", change.value().trim_end_matches('\n')));
                }
            }
        }
        let hidden_lines = lines.len().saturating_sub(MAX_DIFF_LINES);
        lines.truncate(MAX_DIFF_LINES);
        Self {
            path: path.to_string(),
            added,
            removed,
            lines,
            hidden_lines,
        }
    }
}

impl From<&PlanEntryStatus> for PlanStatus {
    fn from(status: &PlanEntryStatus) -> Self {
        match status {
            PlanEntryStatus::Pending => Self::Pending,
            PlanEntryStatus::InProgress => Self::InProgress,
            PlanEntryStatus::Completed => Self::Completed,
            _ => Self::Pending,
        }
    }
}

/// The readable text of a content block, if it has any.
pub fn block_text(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(text) => Some(text.text.clone()),
        ContentBlock::ResourceLink(link) => Some(format!("[{}]({})", link.name, link.uri)),
        _ => None,
    }
}

// ---- Runs of steps -------------------------------------------------------------

/// Whether `entry` is a step of the agent's work that folds into a run:
/// a tool call, a thought, or the blank text between them.
fn is_step(entry: &Entry) -> bool {
    match entry {
        Entry::Tool(_) | Entry::Thought { .. } => true,
        Entry::Agent { text, .. } => text.trim().is_empty(),
        _ => false,
    }
}

/// The run of steps entry `ix` belongs to: the steps around it with no
/// prose, question or message between, when it holds two tool calls or
/// more (one alone stays its own row).
pub fn run_of(entries: &[Entry], ix: usize) -> Option<std::ops::Range<usize>> {
    if !entries.get(ix).is_some_and(is_step) {
        return None;
    }
    let start = entries[..ix]
        .iter()
        .rposition(|entry| !is_step(entry))
        .map_or(0, |before| before + 1);
    let end = entries[ix..]
        .iter()
        .position(|entry| !is_step(entry))
        .map_or(entries.len(), |after| ix + after);
    let tools = entries[start..end]
        .iter()
        .filter(|entry| matches!(entry, Entry::Tool(_)))
        .count();
    (tools >= 2).then_some(start..end)
}

/// The tool calls of a run.
pub fn run_tools(entries: &[Entry], run: std::ops::Range<usize>) -> Vec<&ToolEntry> {
    entries[run]
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

/// What a run did, in a line: "Edited 2 files, ran 3 commands", and how
/// many of its calls failed.
pub fn run_summary(tools: &[&ToolEntry]) -> (String, usize) {
    let mut counts: Vec<(&str, &str, usize)> = Vec::new();
    let mut add = |verb: &'static str, noun: &'static str| match counts
        .iter_mut()
        .find(|(known, ..)| *known == verb)
    {
        Some((_, _, count)) => *count += 1,
        None => counts.push((verb, noun, 1)),
    };
    for tool in tools {
        match tool.kind {
            _ if tool.is_subagent() => add("ran", "subagent"),
            ToolKindLabel::Read => add("read", "file"),
            ToolKindLabel::Edit | ToolKindLabel::Delete | ToolKindLabel::Move => {
                add("edited", "file")
            }
            ToolKindLabel::Execute => add("ran", "command"),
            ToolKindLabel::Search => add("searched", "time"),
            ToolKindLabel::Fetch => add("fetched", "page"),
            ToolKindLabel::Think | ToolKindLabel::Other => add("used", "tool"),
        }
    }
    let parts: Vec<String> = counts
        .iter()
        .map(|(verb, noun, count)| match (*noun, *count) {
            ("time", 1) => format!("{verb} once"),
            ("time", n) => format!("{verb} {n} times"),
            (noun, 1) => format!("{verb} 1 {noun}"),
            (noun, n) => format!("{verb} {n} {noun}s"),
        })
        .collect();
    let mut text = parts.join(", ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    let failed = tools
        .iter()
        .filter(|tool| tool.status == ToolStatus::Failed)
        .count();
    (text, failed)
}

/// The files a turn changed (every edit of its tool calls and their
/// subagents' calls), merged by path, with the lines added and removed.
pub fn turn_changes(entries: &[Entry]) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    // By path, so a turn editing thousands of files stays linear.
    let mut index: std::collections::HashMap<String, usize> = Default::default();
    for diff in turn_diffs(entries) {
        match index.get(&diff.path) {
            Some(&ix) => {
                let file = &mut files[ix];
                file.added += diff.added;
                file.removed += diff.removed;
                file.lines.extend(diff.lines.iter().cloned());
                file.hidden_lines += diff.hidden_lines;
            }
            None => {
                index.insert(diff.path.clone(), files.len());
                files.push(diff.clone());
            }
        }
    }
    files
}

/// How many files a turn changed, and the lines added and removed: what
/// [`turn_changes`] adds up, without copying any diff (it is shown under
/// every turn, every frame).
pub fn turn_change_counts(entries: &[Entry]) -> (usize, usize, usize) {
    let mut paths = std::collections::HashSet::new();
    let (mut added, mut removed) = (0, 0);
    for diff in turn_diffs(entries) {
        paths.insert(diff.path.as_str());
        added += diff.added;
        removed += diff.removed;
    }
    (paths.len(), added, removed)
}

fn turn_diffs(entries: &[Entry]) -> impl Iterator<Item = &FileDiff> {
    entries.iter().flat_map(|entry| match entry {
        Entry::Tool(tool) => tool
            .diffs
            .iter()
            .chain(tool.children.iter().flat_map(|child| child.diffs.iter()))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    })
}

// ---- Persistence ------------------------------------------------------------

/// The stored body of an entry. Live-only entries (permission prompts) are
/// stored as a notice describing their outcome.
///
/// The tag is `entry`, not `kind`: tool entries have a `kind` field of their own.
#[derive(Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub(crate) enum StoredEntry {
    User {
        text: String,
        #[serde(default)]
        attachments: Vec<AttachmentSummary>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<String>,
    },
    Agent {
        text: String,
    },
    Restored {
        text: String,
        #[serde(default)]
        undo: Option<String>,
    },
    Thought {
        text: String,
    },
    Tool(ToolEntry),
    Plan {
        items: Vec<PlanItem>,
    },
    Notice {
        text: String,
        is_error: bool,
    },
    Review {
        review_id: i64,
    },
}

impl Entry {
    pub fn review(review_id: i64, cx: &mut App) -> Self {
        let card = cx.new(|cx| ReviewCard::new(review_id, cx));
        Entry::Review { review_id, card }
    }

    pub fn agent(text: String, cx: &mut App) -> Self {
        let view = cx.new(|cx| crate::reply::Reply::new(&text, cx));
        Entry::Agent {
            text,
            view: Some(view),
        }
    }

    /// Makes the markdown of an agent reply if it has none yet (see
    /// [`Entry::Agent::view`]).
    pub fn prepare(&mut self, cx: &mut App) {
        if let Entry::Agent { text, view: None } = self {
            let text = text.clone();
            *self = Entry::agent(text, cx);
        }
    }

    pub fn to_stored(&self, cx: &App) -> Option<(&'static str, String)> {
        let stored = match self {
            Entry::User {
                text,
                attachments,
                checkpoint,
            } => StoredEntry::User {
                text: text.clone(),
                attachments: attachments.clone(),
                checkpoint: checkpoint.clone(),
            },
            Entry::Restored { text, undo } => StoredEntry::Restored {
                text: text.clone(),
                undo: undo.clone(),
            },
            Entry::Agent { text, .. } => StoredEntry::Agent { text: text.clone() },
            Entry::Thought { text, .. } => StoredEntry::Thought { text: text.clone() },
            Entry::Tool(tool) => StoredEntry::Tool(tool.clone()),
            Entry::Plan(items) => StoredEntry::Plan {
                items: items.clone(),
            },
            Entry::Notice { text, is_error } => StoredEntry::Notice {
                text: text.clone(),
                is_error: *is_error,
            },
            Entry::Permission(request) => StoredEntry::Notice {
                text: request.read(cx).summary(),
                is_error: false,
            },
            Entry::SignIn { message, .. } => StoredEntry::Notice {
                text: message.clone(),
                is_error: true,
            },
            Entry::Review { review_id, .. } => StoredEntry::Review {
                review_id: *review_id,
            },
        };
        let role = match &stored {
            StoredEntry::User { .. } => "user",
            _ => "agent",
        };
        serde_json::to_string(&stored).ok().map(|body| (role, body))
    }

    /// Rebuilds an entry from the store; unknown shapes are skipped.
    pub fn from_stored(body: &str, cx: &mut App) -> Option<Self> {
        let stored: StoredEntry = serde_json::from_str(body).ok()?;
        Some(match stored {
            StoredEntry::User {
                text,
                attachments,
                checkpoint,
            } => Entry::User {
                text,
                attachments,
                checkpoint,
            },
            StoredEntry::Restored { text, undo } => Entry::Restored { text, undo },
            StoredEntry::Agent { text } => Entry::Agent { text, view: None },
            StoredEntry::Thought { text } => Entry::Thought {
                text,
                expanded: false,
            },
            StoredEntry::Tool(tool) => Entry::Tool(tool),
            StoredEntry::Plan { items } => Entry::Plan(items),
            StoredEntry::Notice { text, is_error } => Entry::Notice { text, is_error },
            StoredEntry::Review { review_id } => Entry::review(review_id, cx),
        })
    }
}

/// A parsed `session/update` notification.
pub struct Update {
    pub session_id: String,
    pub update: agent_client_protocol_schema::v1::SessionUpdate,
    /// The tool call that started the subagent this update comes from
    /// (Claude Code stamps `_meta.claudeCode.parentToolUseId` on a
    /// subagent's messages and tool calls).
    pub parent: Option<String>,
    /// Claude's subscription limit report riding on a usage update.
    pub rate_limit: Option<Value>,
}

/// Parses a `session/update` notification's update, tolerating update types
/// newer than this build (they are ignored, not treated as errors).
pub fn parse_update(params: &Value) -> Option<Update> {
    let notification: agent_client_protocol_schema::v1::SessionNotification =
        serde_json::from_value(params.clone()).ok()?;
    let parent = params["update"]["_meta"]["claudeCode"]["parentToolUseId"]
        .as_str()
        .map(str::to_string);
    let rate_limit = params["update"]["_meta"]
        .get("_claude/rateLimit")
        .filter(|info| info.is_object())
        .cloned();
    Some(Update {
        session_id: notification.session_id.0.to_string(),
        update: notification.update,
        parent,
        rate_limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_tool_titles_read_as_words() {
        let input = serde_json::json!({ "path": "tide.py", "new_text": "x" });
        assert_eq!(
            readable_title(
                r#"editor: {"new_text":"x"#,
                ToolKindLabel::Edit,
                Some(&input)
            ),
            "Edit tide.py"
        );
        assert_eq!(
            readable_title(
                r#"run_commands: {"commands":["ls -la"]}"#,
                ToolKindLabel::Execute,
                None
            ),
            "Run ls -la"
        );
        assert_eq!(
            readable_title(
                r#"read_files: {"files":[{"path":"/w/a.py"}]}"#,
                ToolKindLabel::Read,
                None
            ),
            "Read /w/a.py"
        );
        let deep = format!("/{}/tide.py", "folder/".repeat(30));
        let input = serde_json::json!({ "path": deep });
        let title = readable_title(r#"read: {"path":"x"}"#, ToolKindLabel::Read, Some(&input));
        assert!(
            title.starts_with("Read …") && title.ends_with("/tide.py"),
            "{title}"
        );
        // Titles that are words already stay.
        assert_eq!(
            readable_title("Read src/main.rs", ToolKindLabel::Read, Some(&input)),
            "Read src/main.rs"
        );
        assert_eq!(
            readable_title("Note: {braces} in prose", ToolKindLabel::Other, None),
            "Note: {braces} in prose"
        );
    }

    #[test]
    fn a_long_output_keeps_its_start() {
        let long: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let kept = head(&long, OUTPUT_LINES, OUTPUT_BYTES);
        assert!(kept.starts_with("line 0\nline 1\n"));
        assert!(
            kept.ends_with("… 440 more lines"),
            "{}",
            &kept[kept.len() - 40..]
        );
        assert_eq!(head("short\n", OUTPUT_LINES, OUTPUT_BYTES), "short\n");
        // A byte cap that lands inside a character backs off to its start.
        let wide = "é".repeat(5_000);
        assert!(head(&wide, OUTPUT_LINES, OUTPUT_BYTES).len() <= OUTPUT_BYTES + 32);
    }

    #[test]
    fn a_command_keeps_the_end_of_what_it_printed() {
        let long: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let kept = ends(
            &long,
            COMMAND_HEAD_LINES,
            COMMAND_TAIL_LINES,
            COMMAND_BYTES,
            0,
        );
        assert!(kept.starts_with("line 0\nline 1\n"));
        assert!(kept.contains("line 19\n… 780 lines left out …\nline 800\n"));
        assert!(
            kept.ends_with("line 998\nline 999\n"),
            "the failure is at the end"
        );
        // Short output is kept whole.
        assert_eq!(ends("ok\nfine\n", 20, 200, 32_000, 0), "ok\nfine\n");
        // Too many bytes: the tail's start goes, its end stays.
        let wide: String = (0..300)
            .map(|i| format!("{i} {}\n", "x".repeat(400)))
            .collect();
        let kept = ends(
            &wide,
            COMMAND_HEAD_LINES,
            COMMAND_TAIL_LINES,
            COMMAND_BYTES,
            0,
        );
        assert!(kept.len() <= COMMAND_BYTES + 64, "{}", kept.len());
        assert!(kept.contains("\n299 x"));
        // One endless line (minified output) keeps its end.
        let one = format!("{}END", "é".repeat(40_000));
        let kept = ends(&one, 20, 200, 32_000, 0);
        assert!(kept.len() <= 32_000 + 8 && kept.ends_with("END"));
        // A command's call keeps its end; a read keeps its start.
        let content = |text: &str| {
            vec![ToolCallContent::Content(
                agent_client_protocol_schema::v1::Content::new(ContentBlock::Text(
                    agent_client_protocol_schema::v1::TextContent::new(text.to_string()),
                )),
            )]
        };
        let mut call = ToolEntry {
            id: "1".into(),
            title: "cargo test".into(),
            kind: ToolKindLabel::Execute,
            status: ToolStatus::Failed,
            output: String::new(),
            diffs: Vec::new(),
            children: Vec::new(),
            agent_text: String::new(),
            run: Default::default(),
            images: Vec::new(),
            expanded: false,
            started: None,
            touched: None,
        };
        call.set_content(&content(&format!(
            "{long}\u{1b}[31mFAILED\u{1b}[0m 2 tests\n"
        )));
        assert!(
            call.output.ends_with("line 999\nFAILED 2 tests\n"),
            "{:?}",
            &call.output[call.output.len() - 40..]
        );
        call.kind = ToolKindLabel::Read;
        call.set_content(&content(&long));
        assert!(call.output.ends_with("… 940 more lines"));
    }

    /// What Codex sends while a command runs, and both Codex and Claude Code
    /// at its end.
    #[test]
    fn a_running_command_shows_what_it_prints_and_how_it_ended() {
        let update = |meta: Value, fields: Value| -> ToolCallUpdate {
            let mut update = serde_json::json!({ "toolCallId": "call_1", "_meta": meta });
            for (key, value) in fields.as_object().unwrap() {
                update[key] = value.clone();
            }
            serde_json::from_value(update).unwrap()
        };
        let chunk = |data: &str| {
            update(
                serde_json::json!({ "terminal_output_delta": { "terminal_id": "call_1", "data": data } }),
                serde_json::json!({}),
            )
        };
        let mut call = ToolEntry {
            id: "call_1".into(),
            kind: ToolKindLabel::Execute,
            ..ToolEntry::placeholder()
        };
        call.apply(&chunk("running 2 tests\n"));
        call.apply(&chunk("test a ... ok\ntest b ... "));
        assert_eq!(call.output, "running 2 tests\ntest a ... ok\ntest b ... ");
        call.apply(&chunk("\u{1b}[31mFAILED\u{1b}[0m\n"));
        assert!(
            call.output.ends_with("test b ... FAILED\n"),
            "{:?}",
            call.output
        );
        // The end: Claude Code names the terminal as the content; that is
        // not the output going away.
        call.apply(&update(
            serde_json::json!({ "terminal_exit": { "terminal_id": "call_1", "exit_code": 101, "signal": null } }),
            serde_json::json!({ "status": "failed", "content": [{ "type": "terminal", "terminalId": "call_1" }] }),
        ));
        assert_eq!(call.run.exit, Some(101));
        assert_eq!(call.status, ToolStatus::Failed);
        assert!(call.output.ends_with("test b ... FAILED\n"));
        // The exit code is kept with the chat; the pieces are not.
        let stored = serde_json::to_value(&call).unwrap();
        assert_eq!(stored["run"]["exit"], 101);
        let back: ToolEntry = serde_json::from_value(stored).unwrap();
        assert_eq!(back.output, call.output);
        // An older stored call has no `run`.
        let old: ToolEntry = serde_json::from_value(serde_json::json!({
            "id": "1", "title": "ls", "kind": "execute", "status": "done", "output": "", "diffs": []
        }))
        .unwrap();
        assert_eq!(old.run.exit, None);
        // An unknown exit code (an interrupted command) says nothing.
        call.apply(&update(
            serde_json::json!({ "terminal_exit": { "terminal_id": "call_1", "exit_code": null } }),
            serde_json::json!({}),
        ));
        assert_eq!(call.run.exit, Some(101));

        // A command that prints without end: the newest is held, and the
        // note counts what fell off.
        let mut noisy = ToolEntry {
            kind: ToolKindLabel::Execute,
            ..ToolEntry::placeholder()
        };
        for piece in 0..400 {
            let lines: String = (piece * 100..piece * 100 + 100)
                .map(|i| format!("line {i} of a long build log\n"))
                .collect();
            noisy.push_output(&lines);
        }
        assert!(noisy.run.stream.len() <= 2 * STREAM_BYTES);
        assert!(noisy.output.len() <= COMMAND_BYTES + 64);
        assert!(
            noisy
                .output
                .starts_with("… 39800 lines left out …\nline 39800 "),
            "{:?}",
            &noisy.output[..60]
        );
        assert!(noisy.output.ends_with("line 39999 of a long build log\n"));
        // A progress bar's rewrites do not pile up.
        let mut bar = ToolEntry {
            kind: ToolKindLabel::Execute,
            ..ToolEntry::placeholder()
        };
        for i in 0..=100 {
            bar.push_output(&format!("\r{i}%"));
        }
        assert_eq!(bar.output, "100%");
        // Whole output sent at the end replaces the pieces.
        bar.apply(&update(
            serde_json::json!({}),
            serde_json::json!({ "content": [{ "type": "content", "content": { "type": "text", "text": "done\n" } }] }),
        ));
        assert_eq!(bar.output, "done\n");
    }

    /// A tool that gives a picture back (a screenshot) shows it: it was
    /// dropped, and the call said one line of text.
    #[test]
    fn a_picture_a_tool_gives_back_is_kept() {
        let dir = std::env::temp_dir().join(format!("ph-tool-images-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let png: &[u8] = b"\x89PNG\r\n\x1a\nnot really a picture";
        let content: Vec<ToolCallContent> = serde_json::from_value(serde_json::json!([
            { "type": "content", "content": { "type": "text", "text": "Took a screenshot." } },
            { "type": "content", "content": {
                "type": "image", "mimeType": "image/png",
                "data": crate::attachments::base64(png)
            } },
            // Not a kind the app draws: passed over.
            { "type": "content", "content": {
                "type": "image", "mimeType": "image/tiff", "data": "AAAA"
            } },
            { "type": "content", "content": {
                "type": "image", "mimeType": "image/png", "data": "not base64!"
            } },
        ]))
        .unwrap();
        let mut call = ToolEntry {
            id: "toolu_01/../x".into(),
            ..ToolEntry::placeholder()
        };
        call.keep_images_in(&content, &dir);
        assert_eq!(call.images.len(), 1);
        let path = call.images[0].clone();
        assert_eq!(std::fs::read(&path).unwrap(), png);
        assert!(path.starts_with(&dir), "{}", path.display());
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "tool-toolu_01x-0.png",
            "a file's name holds nothing of a path"
        );
        // The same content again (every update carries it) writes nothing
        // new, and an update without pictures keeps the ones there are.
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        call.keep_images_in(&content, &dir);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written
        );
        call.keep_images_in(&content[..1], &dir);
        assert_eq!(call.images, vec![path]);
        // Kept with the chat.
        let stored = serde_json::to_value(&call).unwrap();
        let back: ToolEntry = serde_json::from_value(stored).unwrap();
        assert_eq!(back.images, call.images);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_reads_as_a_terminal_left_it() {
        // Colours and cursor moves are taken out.
        assert_eq!(
            plain("\u{1b}[1;32m   Compiling\u{1b}[0m app v0.1\u{1b}[K\n"),
            "   Compiling app v0.1\n"
        );
        // A title or a link sequence, ended either way.
        assert_eq!(
            plain("a\u{1b}]0;title\u{7}b\u{1b}]8;;http://x\u{1b}\\c"),
            "abc"
        );
        // A progress bar that rewrote its line shows as it ended.
        assert_eq!(plain("10%\r50%\r100% done\r\nnext\n"), "100% done\nnext\n");
        assert_eq!(plain("plain text\n"), "plain text\n");
    }

    #[test]
    fn diff_counts_and_caps_lines() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\nd\n";
        let diff = FileDiff::new("x.txt", old, new);
        assert_eq!((diff.added, diff.removed), (2, 1));
        assert!(diff.lines.contains(&"-b".to_string()));
        assert!(diff.lines.contains(&"+B".to_string()));

        let many: String = (0..500).map(|n| format!("{n}\n")).collect();
        let big = FileDiff::new("y.txt", "", &many);
        assert_eq!(big.lines.len(), MAX_DIFF_LINES);
        assert_eq!(big.hidden_lines, 500 - MAX_DIFF_LINES);
    }

    #[test]
    fn tool_entries_round_trip_through_the_store() {
        let tool = ToolEntry {
            id: "t1".into(),
            title: "mcp__pilothouse__test_list".into(),
            kind: ToolKindLabel::Other,
            status: ToolStatus::Done,
            output: "ok".into(),
            diffs: Vec::new(),
            children: Vec::new(),
            agent_text: String::new(),
            run: Default::default(),
            images: Vec::new(),
            expanded: false,
            started: None,
            touched: None,
        };
        assert_eq!(tool.display_title(), "test list · pilothouse");
        let body = serde_json::to_string(&StoredEntry::Tool(tool.clone())).unwrap();
        match serde_json::from_str::<StoredEntry>(&body).unwrap() {
            StoredEntry::Tool(restored) => assert_eq!(restored, tool),
            _ => panic!("expected a tool entry"),
        }
    }

    fn tool(kind: ToolKindLabel, status: ToolStatus) -> Entry {
        Entry::Tool(ToolEntry {
            kind,
            status,
            ..ToolEntry::placeholder()
        })
    }

    fn text(text: &str) -> Entry {
        Entry::Agent {
            text: text.into(),
            view: None,
        }
    }

    #[test]
    fn steps_fold_into_runs_between_prose() {
        let entries = vec![
            text("Looking."),
            tool(ToolKindLabel::Read, ToolStatus::Done),
            text("\n"),
            tool(ToolKindLabel::Read, ToolStatus::Done),
            tool(ToolKindLabel::Execute, ToolStatus::Failed),
            text("Found it."),
            tool(ToolKindLabel::Edit, ToolStatus::Done),
        ];
        assert_eq!(run_of(&entries, 2), Some(1..5));
        assert_eq!(run_of(&entries, 0), None, "prose is not a step");
        assert_eq!(run_of(&entries, 6), None, "one call alone stays a row");
        let tools = run_tools(&entries, 1..5);
        assert_eq!(
            run_summary(&tools),
            ("Read 2 files, ran 1 command".to_string(), 1)
        );
    }

    #[test]
    fn turn_changes_merge_by_file() {
        let edit = |path: &str, old: &str, new: &str| {
            Entry::Tool(ToolEntry {
                diffs: vec![FileDiff::new(path, old, new)],
                ..ToolEntry::placeholder()
            })
        };
        let entries = vec![
            edit("a.rs", "", "x\n"),
            edit("a.rs", "x\n", "y\n"),
            edit("b.rs", "", "z\n"),
        ];
        let files = turn_changes(&entries);
        assert_eq!(files.len(), 2);
        assert_eq!((files[0].added, files[0].removed), (2, 1));
        // The footer's count agrees, without copying the diffs.
        assert_eq!(turn_change_counts(&entries), (2, 3, 1));
    }

    #[test]
    fn unknown_session_updates_are_ignored() {
        let params = serde_json::json!({
            "sessionId": "s1",
            "update": { "sessionUpdate": "some_future_update", "x": 1 }
        });
        assert!(parse_update(&params).is_none());

        let params = serde_json::json!({
            "sessionId": "s1",
            "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "hi" } }
        });
        let parsed = parse_update(&params).unwrap();
        assert_eq!(parsed.session_id, "s1");
        assert!(parsed.parent.is_none());

        let params = serde_json::json!({
            "sessionId": "s1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "from a subagent" },
                "_meta": { "claudeCode": { "parentToolUseId": "toolu_1" } }
            }
        });
        assert_eq!(
            parse_update(&params).unwrap().parent.as_deref(),
            Some("toolu_1")
        );
    }
}
