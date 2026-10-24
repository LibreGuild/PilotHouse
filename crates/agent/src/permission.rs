//! Agent permission requests and the approval inbox.
//!
//! When an agent asks "may I run this?", PilotHouse shows the question inline
//! in its chat *and* in one inbox that collects every pending request across
//! all projects. Answering in either place answers the agent once.

use agent_client_protocol_schema::v1::{PermissionOption, PermissionOptionKind};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, WeakEntity,
};

/// The agent's answer callback. `None` means the request was cancelled.
type Responder = Box<dyn FnOnce(Option<String>, &mut App)>;

/// One question from an agent, waiting for the user.
pub struct PermissionRequest {
    /// Where it came from, for the inbox: "Project · Chat title".
    pub source: SharedString,
    /// What the agent wants to do, e.g. "Run `cargo test`".
    pub title: SharedString,
    pub options: Vec<PermissionChoice>,
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
            outcome: None,
            responder: Some(Box::new(responder)),
        }
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
        let Some(responder) = self.responder.take() else {
            return;
        };
        self.outcome = self
            .options
            .iter()
            .find(|choice| choice.id == choice_id)
            .map(|choice| choice.label.clone());
        responder(Some(choice_id.to_string()), cx);
        cx.emit(PermissionResolved);
        cx.notify();
    }

    /// Withdraws the question (the turn was cancelled or the agent exited).
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some(responder) = self.responder.take() else {
            return;
        };
        self.outcome = Some("Cancelled".into());
        responder(None, cx);
        cx.emit(PermissionResolved);
        cx.notify();
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
