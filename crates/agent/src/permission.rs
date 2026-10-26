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
    outcome: Option<SharedString>,
    responder: Option<Responder>,
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
            inbox
                .requests
                .retain(|existing| existing.upgrade().is_some());
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
