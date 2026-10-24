//! `board.*` control-socket methods, used by the MCP bridge on behalf of agents.
//!
//! Every method takes a `project_id`. Cards are returned as plain JSON so the
//! bridge can hand them to agents without knowing the store's types.

use anyhow::{Context as _, Result, bail};
use gpui_kit::App;
use ph_control::ControlServer;
use ph_store::Store;
use ph_store::board::{Card, CardPatch, CardStatus, NewCard};
use ph_store::projects::ProjectId;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::BoardHub;

/// Registers the board methods against `store`.
pub fn register_control_methods(store: Store, cx: &mut App) {
    method(&store, "board.list", cx, |store, params, _| {
        let project_id = project_id(&params)?;
        let cards = store.cards(project_id)?;
        let cards: Vec<Value> = cards.iter().map(|card| with_tests(store, card)).collect();
        Ok(json!({ "cards": cards }))
    });

    method(&store, "board.add_cards", cx, |store, params, cx| {
        #[derive(Deserialize)]
        struct Params {
            project_id: ProjectId,
            cards: Vec<NewCard>,
        }
        let params: Params =
            serde_json::from_value(params).context("expected project_id and cards")?;
        let cards = store.add_cards(params.project_id, &params.cards)?;
        BoardHub::notify(params.project_id, cx);
        Ok(json!({ "cards": cards.iter().map(card_json).collect::<Vec<_>>() }))
    });

    method(&store, "board.next_card", cx, |store, params, cx| {
        let project_id = project_id(&params)?;
        let agent = params
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("agent");
        let card = store.claim_next_card(project_id, agent)?;
        BoardHub::notify(project_id, cx);
        Ok(json!({ "card": card.as_ref().map(|card| with_tests(store, card)) }))
    });

    method(&store, "board.update_card", cx, |store, params, cx| {
        #[derive(Deserialize)]
        struct Params {
            card_id: i64,
            /// The caller's project; the card must belong to it.
            project_id: Option<ProjectId>,
            title: Option<String>,
            description: Option<String>,
            acceptance: Option<String>,
            status: Option<String>,
            assignee: Option<String>,
        }
        let params: Params = serde_json::from_value(params).context("expected card_id")?;
        check_project(store, params.card_id, params.project_id)?;
        let status = match params.status.as_deref() {
            Some(key) => Some(CardStatus::from_key(key).with_context(|| {
                format!("unknown status `{key}`; use todo, in_progress, review or done")
            })?),
            None => None,
        };
        if let Some(status) = status {
            crate::may_finish(store, params.card_id, status)?;
        }
        let card = store.update_card(
            params.card_id,
            &CardPatch {
                title: params.title,
                description: params.description,
                acceptance: params.acceptance,
                status,
                assignee: params.assignee.map(Some),
            },
        )?;
        BoardHub::notify(card.project_id, cx);
        Ok(json!({ "card": with_tests(store, &card) }))
    });

    method(&store, "board.add_note", cx, |store, params, cx| {
        let card_id = params
            .get("card_id")
            .and_then(Value::as_i64)
            .context("missing `card_id`")?;
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .context("missing `text`")?;
        if text.trim().is_empty() {
            bail!("note text is empty");
        }
        let author = params
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("agent");
        let card = check_project(
            store,
            card_id,
            params.get("project_id").and_then(Value::as_i64),
        )?;
        store.add_card_event(card_id, author, text)?;
        BoardHub::notify(card.project_id, cx);
        Ok(json!({ "ok": true }))
    });
}

fn method(
    store: &Store,
    name: &'static str,
    cx: &mut App,
    handler: impl Fn(&Store, Value, &mut App) -> Result<Value> + 'static,
) {
    let store = store.clone();
    ControlServer::handle(name, move |params, cx| handler(&store, params, cx), cx);
}

/// The card, if it exists and (when the caller says which project it works
/// in) belongs to that project. Agents only ever see their own project's
/// board, so a card id from elsewhere reads as unknown.
fn check_project(store: &Store, card_id: i64, project_id: Option<ProjectId>) -> Result<Card> {
    match store.card(card_id)? {
        Some(card) if project_id.is_none_or(|project| project == card.project_id) => Ok(card),
        _ => bail!("no card with id {card_id} on this board"),
    }
}

fn project_id(params: &Value) -> Result<ProjectId> {
    params
        .get("project_id")
        .and_then(Value::as_i64)
        .context("missing `project_id`")
}

/// A card with the UI tests it needs to pass before Done, for agents.
fn with_tests(store: &Store, card: &Card) -> Value {
    let mut value = card_json(card);
    value["tests"] = json!(store.card_tests(card.id).unwrap_or_default());
    value
}

pub(crate) fn card_json(card: &Card) -> Value {
    json!({
        "id": card.id,
        "title": card.title,
        "description": card.description,
        "acceptance": card.acceptance,
        "status": card.status.key(),
        "assignee": card.assignee,
    })
}
