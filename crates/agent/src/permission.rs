//! Agent permission requests and the approval inbox.
//!
//! When an agent asks "may I run this?", or asks a question with a form,
//! PilotHouse shows it inline in its chat *and* in one inbox that collects
//! every pending request across all projects. Answering in either place
//! answers the agent once.

use agent_client_protocol_schema::v1::{PermissionOption, PermissionOptionKind};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, WeakEntity,
};

use crate::form::Form;

/// How the user answered.
pub enum Reply {
    /// Picked a permission option, by id.
    Chose(String),
    /// Filled in the form.
    Answered(serde_json::Map<String, serde_json::Value>),
    /// Would not answer the form.
    Declined,
    /// The question went away (turn cancelled, agent exited).
    Cancelled,
}

/// The agent's answer callback.
type Responder = Box<dyn FnOnce(Reply, &mut App)>;

/// One question from an agent, waiting for the user.
pub struct PermissionRequest {
    /// Where it came from, for the inbox: "Project · Chat title".
    pub source: SharedString,
    /// What the agent wants to do, e.g. "Run `cargo test`".
    pub title: SharedString,
    pub options: Vec<PermissionChoice>,
    /// A question with a form, instead of options.
    form: Option<Form>,
    /// The plan to approve, when the agent asks to leave plan mode.
    pub plan: Option<gpui_kit::Entity<crate::reply::Reply>>,
    /// What is being approved, whole, to read before answering.
    pub asked: Asked,
    outcome: Option<SharedString>,
    responder: Option<Responder>,
}

/// What an approval is about, beyond its title: a title is one line, and
/// "Run a script" or "Edit tide.py" is not enough to say yes to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Asked {
    /// A command in full, or what a tool is called with, a line per field.
    pub input: Option<SharedString>,
    /// The edits the action would make.
    pub diffs: Vec<crate::transcript::FileDiff>,
}

/// The most of an input that a card shows.
const INPUT_CHARS: usize = 4_000;
/// The most of one field of it.
const FIELD_CHARS: usize = 600;

impl Asked {
    /// What the card of an action shows under `title`: `diffs` as they are,
    /// and from the action's `input` what the title does not already say.
    pub fn of(
        title: &str,
        kind: crate::transcript::ToolKindLabel,
        input: Option<&serde_json::Value>,
        diffs: Vec<crate::transcript::FileDiff>,
    ) -> Self {
        use crate::transcript::ToolKindLabel as Kind;
        let text = match kind {
            Kind::Execute => input.and_then(command_of),
            // Their title names the file or the search, and an edit shows
            // as its diff.
            Kind::Read | Kind::Search | Kind::Think => None,
            Kind::Edit | Kind::Delete | Kind::Move if !diffs.is_empty() => None,
            _ => input.and_then(fields_of),
        };
        let input = text
            .map(|text| text.trim().to_string())
            // Nothing twice: a short command is its own title.
            .filter(|text| !text.is_empty() && !title.contains(text.as_str()))
            .map(|text| clip(&text, INPUT_CHARS).into());
        Self { input, diffs }
    }

    pub fn is_empty(&self) -> bool {
        self.input.is_none() && self.diffs.is_empty()
    }
}

/// The command an agent asks to run, from its tool's input: a string, the
/// script a shell is started with (`["bash", "-lc", "…"]`), or the words
/// of a command.
fn command_of(input: &serde_json::Value) -> Option<String> {
    use serde_json::Value;
    let words = |items: &[Value]| -> Vec<String> {
        items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()
    };
    ["command", "cmd", "script", "commands"]
        .iter()
        .find_map(|key| match input.get(*key)? {
            Value::String(command) => Some(command.clone()),
            // Several commands, one a line.
            Value::Array(items) if *key == "commands" => Some(words(items).join("\n")),
            Value::Array(items) => {
                let words = words(items);
                match words.as_slice() {
                    [_shell, flag, script] if flag.starts_with('-') && flag.ends_with('c') => {
                        Some(script.clone())
                    }
                    _ => Some(words.join(" ")),
                }
            }
            _ => None,
        })
}

/// A tool's input as `name: value` lines, strings as they are.
fn fields_of(input: &serde_json::Value) -> Option<String> {
    let fields = input.as_object()?;
    let lines: Vec<String> = fields
        .iter()
        .filter_map(|(name, value)| {
            let value = match value {
                serde_json::Value::Null => return None,
                serde_json::Value::String(text) if text.is_empty() => return None,
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            Some(format!("{name}: {}", clip(&value, FIELD_CHARS)))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// The first `chars` characters of `text`, with a mark when there is more.
fn clip(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct PermissionChoice {
    pub id: String,
    pub label: SharedString,
    /// Whether choosing it lets the action run.
    pub allows: bool,
}

impl PermissionChoice {
    pub fn from_option(option: &PermissionOption) -> Self {
        Self {
            id: option.option_id.0.to_string(),
            label: option.name.clone().into(),
            allows: matches!(
                option.kind,
                PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PermissionResolved;

impl EventEmitter<PermissionResolved> for PermissionRequest {}

impl PermissionRequest {
    pub fn new(
        source: SharedString,
        title: SharedString,
        options: Vec<PermissionChoice>,
        responder: impl FnOnce(Option<String>, &mut App) + 'static,
    ) -> Self {
        Self {
            source,
            title,
            options,
            form: None,
            plan: None,
            asked: Asked::default(),
            outcome: None,
            responder: Some(Box::new(move |reply, cx| match reply {
                Reply::Chose(option) => responder(Some(option), cx),
                _ => responder(None, cx),
            })),
        }
    }

    /// A question asked with a form.
    pub fn question(
        source: SharedString,
        form: Form,
        responder: impl FnOnce(Reply, &mut App) + 'static,
    ) -> Self {
        Self {
            source,
            title: form.headline().into(),
            options: Vec::new(),
            form: Some(form),
            plan: None,
            asked: Asked::default(),
            outcome: None,
            responder: Some(Box::new(responder)),
        }
    }

    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    /// Changes the form's answers (before it is sent).
    pub fn edit_form(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Form)) {
        if let Some(form) = self.form.as_mut()
            && self.responder.is_some()
        {
            change(form);
            cx.notify();
        }
    }

    /// Sends the form's answers.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.form.as_ref().filter(|form| form.is_ready()) else {
            return;
        };
        let (content, summary) = (form.content(), form.summary());
        self.finish(Reply::Answered(content), summary.into(), cx);
    }

    /// Declines to answer the form.
    pub fn decline(&mut self, cx: &mut Context<Self>) {
        self.finish(Reply::Declined, "Skipped".into(), cx);
    }

    fn finish(&mut self, reply: Reply, outcome: SharedString, cx: &mut Context<Self>) {
        let Some(responder) = self.responder.take() else {
            return;
        };
        self.outcome = Some(outcome);
        responder(reply, cx);
        cx.emit(PermissionResolved);
        cx.notify();
    }

    pub fn is_pending(&self) -> bool {
        self.responder.is_some()
    }

    /// It is answered by picking one of [`Self::options`], with nothing to
    /// fill in or read first: a card can offer the options as they are.
    pub fn is_choice(&self) -> bool {
        self.form.is_none() && self.plan.is_none() && !self.options.is_empty()
    }

    /// The chosen option's label once answered, e.g. "Allow once".
    pub fn outcome(&self) -> Option<&SharedString> {
        self.outcome.as_ref()
    }

    pub fn summary(&self) -> String {
        match &self.outcome {
            Some(outcome) => format!("{} — {}", self.title, outcome),
            None => format!("{} — not answered", self.title),
        }
    }

    /// Answers with `choice_id`. Later calls are ignored.
    pub fn choose(&mut self, choice_id: &str, cx: &mut Context<Self>) {
        let outcome = self
            .options
            .iter()
            .find(|choice| choice.id == choice_id)
            .map(|choice| choice.label.clone())
            .unwrap_or_default();
        self.finish(Reply::Chose(choice_id.to_string()), outcome, cx);
    }

    /// Withdraws the question (the turn was cancelled or the agent exited).
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.finish(Reply::Cancelled, "Cancelled".into(), cx);
    }
}

/// All pending requests, across projects.
pub struct ApprovalInbox {
    requests: Vec<WeakEntity<PermissionRequest>>,
}

impl ApprovalInbox {
    pub fn pending(&self, cx: &App) -> Vec<Entity<PermissionRequest>> {
        self.requests
            .iter()
            .filter_map(WeakEntity::upgrade)
            .filter(|request| request.read(cx).is_pending())
            .collect()
    }
}

struct InboxGlobal(Entity<ApprovalInbox>);

impl Global for InboxGlobal {}

impl ApprovalInbox {
    /// The app-wide inbox, created on first use.
    pub fn global(cx: &mut App) -> Entity<ApprovalInbox> {
        if !cx.has_global::<InboxGlobal>() {
            let inbox = cx.new(|_| ApprovalInbox {
                requests: Vec::new(),
            });
            cx.set_global(InboxGlobal(inbox));
        }
        cx.global::<InboxGlobal>().0.clone()
    }

    pub fn add(request: &Entity<PermissionRequest>, cx: &mut App) {
        let inbox = Self::global(cx);
        let weak = request.downgrade();
        inbox.update(cx, |inbox, cx| {
            // Answered requests go (a chat keeps them for its transcript).
            inbox.requests.retain(|existing| {
                existing
                    .upgrade()
                    .is_some_and(|request| request.read(cx).is_pending())
            });
            inbox.requests.push(weak);
            cx.notify();
        });
        // Re-render the inbox badge when the request is answered anywhere.
        let inbox_for_events = inbox.clone();
        cx.subscribe(request, move |_, _: &PermissionResolved, cx| {
            inbox_for_events.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{FileDiff, ToolKindLabel};
    use serde_json::json;

    fn input(asked: &Asked) -> Option<&str> {
        asked.input.as_deref()
    }

    #[test]
    fn a_card_shows_the_whole_command() {
        // A short command is its own title: nothing twice.
        let asked = Asked::of(
            "Run `cargo test`",
            ToolKindLabel::Execute,
            Some(&json!({ "command": "cargo test", "description": "Run the tests" })),
            Vec::new(),
        );
        assert!(asked.is_empty());
        // A script is more than its title's first line.
        let script = "set -e\ncd server\nrm -rf build\nmake release";
        let asked = Asked::of(
            "Run set -e",
            ToolKindLabel::Execute,
            Some(&json!({ "command": script })),
            Vec::new(),
        );
        assert_eq!(input(&asked), Some(script));
        // Codex names the shell and its script; Antigravity lists commands.
        let asked = Asked::of(
            "Run a command",
            ToolKindLabel::Execute,
            Some(&json!({ "command": ["/bin/zsh", "-lc", "git push --force"] })),
            Vec::new(),
        );
        assert_eq!(input(&asked), Some("git push --force"));
        let asked = Asked::of(
            "Run a command",
            ToolKindLabel::Execute,
            Some(&json!({ "command": ["git", "status"] })),
            Vec::new(),
        );
        assert_eq!(input(&asked), Some("git status"));
        let asked = Asked::of(
            "Run commands",
            ToolKindLabel::Execute,
            Some(&json!({ "commands": ["npm ci", "npm test"] })),
            Vec::new(),
        );
        assert_eq!(input(&asked), Some("npm ci\nnpm test"));
        // A command with no input known says nothing more.
        assert!(Asked::of("Run it", ToolKindLabel::Execute, None, Vec::new()).is_empty());
    }

    #[test]
    fn a_card_shows_an_edit_as_its_diff_and_a_tool_as_its_input() {
        let diff = FileDiff::new("tide.py", "a\n", "b\n");
        let asked = Asked::of(
            "Edit tide.py",
            ToolKindLabel::Edit,
            Some(&json!({ "file_path": "tide.py", "old_string": "a", "new_string": "b" })),
            vec![diff.clone()],
        );
        assert_eq!(asked.diffs, vec![diff]);
        assert_eq!(asked.input, None, "the diff says it");
        // An edit the agent sent no diff for still says what it is.
        let asked = Asked::of(
            "Edit a file",
            ToolKindLabel::Edit,
            Some(&json!({ "file_path": "tide.py", "new_string": "b" })),
            Vec::new(),
        );
        assert_eq!(input(&asked), Some("file_path: tide.py\nnew_string: b"));
        // Another program's tool: what it is called with.
        let asked = Asked::of(
            "create_issue",
            ToolKindLabel::Other,
            Some(
                &json!({ "title": "Tides are off", "labels": ["bug"], "body": "", "draft": null }),
            ),
            Vec::new(),
        );
        assert_eq!(
            input(&asked),
            Some("title: Tides are off\nlabels: [\"bug\"]")
        );
        // Reading is said by its title; a long value is cut.
        assert!(
            Asked::of(
                "Read tide.py",
                ToolKindLabel::Read,
                Some(&json!({ "path": "tide.py" })),
                Vec::new()
            )
            .is_empty()
        );
        let asked = Asked::of(
            "Fetch",
            ToolKindLabel::Fetch,
            Some(&json!({ "url": "é".repeat(5_000) })),
            Vec::new(),
        );
        let shown = input(&asked).unwrap();
        assert!(shown.chars().count() < 700 && shown.ends_with('…'));
    }
}
