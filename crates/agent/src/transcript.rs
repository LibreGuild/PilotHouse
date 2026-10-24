//! The conversation model: what a chat shows, built from ACP updates.
//!
//! Entries are plain data except agent replies, which keep a markdown view
//! state so streamed chunks are appended without re-parsing the whole reply.
//! Each entry serialises to a `(role, body)` pair for the store.

use agent_client_protocol_schema::v1::{
    ContentBlock, PlanEntryStatus, ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate,
    ToolKind,
};
use gpui_kit::component::text::TextViewState;
use gpui_kit::{App, AppContext as _, Entity};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::connection::SignInMethod;
use crate::permission::PermissionRequest;
use crate::review_card::ReviewCard;

/// Diff lines shown before "… N more lines" in a tool card.
const MAX_DIFF_LINES: usize = 80;

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
        /// The rendered markdown, made when the reply is first shown: a
        /// long stored chat opens without parsing replies no one scrolls to.
        view: Option<Entity<TextViewState>>,
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
    #[serde(skip)]
    pub expanded: bool,
}

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
            expanded: false,
        }
    }

    pub fn from_call(call: &ToolCall) -> Self {
        let mut entry = Self {
            id: call.tool_call_id.0.to_string(),
            title: call.title.clone(),
            kind: (&call.kind).into(),
            status: (&call.status).into(),
            output: String::new(),
            diffs: Vec::new(),
            expanded: false,
        };
        entry.set_content(&call.content);
        entry
    }

    pub fn apply(&mut self, update: &ToolCallUpdate) {
        let fields = &update.fields;
        if let Some(title) = &fields.title {
            self.title = title.clone();
        }
        if let Some(kind) = &fields.kind {
            self.kind = kind.into();
        }
        if let Some(status) = &fields.status {
            self.status = status.into();
        }
        if let Some(content) = &fields.content {
            self.set_content(content);
        }
    }

    fn set_content(&mut self, content: &[ToolCallContent]) {
        let mut output = String::new();
        let mut diffs = Vec::new();
        for item in content {
            match item {
                ToolCallContent::Content(content) => {
                    if let Some(text) = block_text(&content.content) {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(&text);
                    }
                }
                ToolCallContent::Diff(diff) => diffs.push(FileDiff::new(
                    &diff.path.to_string_lossy(),
                    diff.old_text.as_deref().unwrap_or_default(),
                    &diff.new_text,
                )),
                _ => {}
            }
        }
        self.output = output;
        self.diffs = diffs;
    }
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

// ---- Persistence ------------------------------------------------------------

/// The stored body of an entry. Live-only entries (permission prompts) are
/// stored as a notice describing their outcome.
///
/// The tag is `entry`, not `kind`: tool entries have a `kind` field of their own.
#[derive(Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
enum StoredEntry {
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
        let view = cx.new(|cx| TextViewState::markdown(&text, cx));
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

/// Parses a `session/update` notification's update, tolerating update types
/// newer than this build (they are ignored, not treated as errors).
pub fn parse_update(
    params: &Value,
) -> Option<(String, agent_client_protocol_schema::v1::SessionUpdate)> {
    let notification: agent_client_protocol_schema::v1::SessionNotification =
        serde_json::from_value(params.clone()).ok()?;
    Some((notification.session_id.0.to_string(), notification.update))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            title: "mcp__pilothouse__board_list".into(),
            kind: ToolKindLabel::Other,
            status: ToolStatus::Done,
            output: "ok".into(),
            diffs: Vec::new(),
            expanded: false,
        };
        assert_eq!(tool.display_title(), "board list · pilothouse");
        let body = serde_json::to_string(&StoredEntry::Tool(tool.clone())).unwrap();
        match serde_json::from_str::<StoredEntry>(&body).unwrap() {
            StoredEntry::Tool(restored) => assert_eq!(restored, tool),
            _ => panic!("expected a tool entry"),
        }
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
        let (session, _) = parse_update(&params).unwrap();
        assert_eq!(session, "s1");
    }
}
