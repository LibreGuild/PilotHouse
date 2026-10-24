//! Board tools: the project plan as cards.

use anyhow::Context as _;
use serde_json::{Value, json};

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "board_list",
        description: "List every card on the project board with its id, column and assignee.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |_, project_id, _| Ok(("board.list", json!({ "project_id": project_id }))),
        format: format_cards,
    },
    Tool {
        name: "board_add_cards",
        description: "Add cards to the bottom of the To do column, in order. Use it to plan work as small, verifiable steps.",
        schema: || {
            json!({
                "type": "object",
                "required": ["cards"],
                "properties": {
                    "cards": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["title"],
                            "properties": {
                                "title": { "type": "string" },
                                "description": { "type": "string" },
                                "acceptance": { "type": "string", "description": "Acceptance criteria, one per line." }
                            }
                        }
                    }
                }
            })
        },
        build: |arguments, project_id, _| {
            let cards = arguments
                .get("cards")
                .cloned()
                .context("`cards` is required")?;
            Ok((
                "board.add_cards",
                json!({ "project_id": project_id, "cards": cards }),
            ))
        },
        format: format_cards,
    },
    Tool {
        name: "board_next_card",
        description: "Claim the top card in To do: it moves to In progress and is assigned to you. Returns nothing when the column is empty.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |_, project_id, agent| {
            Ok((
                "board.next_card",
                json!({ "project_id": project_id, "agent": agent }),
            ))
        },
        format: |result| match result.get("card").filter(|card| !card.is_null()) {
            Some(card) => format!("Claimed:\n{}", format_card(card)),
            None => "The To do column is empty.".to_string(),
        },
    },
    Tool {
        name: "board_update_card",
        description: "Change a card's title, description, acceptance criteria or column (todo, in_progress, review, done).",
        schema: || {
            json!({
                "type": "object",
                "required": ["card_id"],
                "properties": {
                    "card_id": { "type": "integer" },
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "acceptance": { "type": "string" },
                    "status": { "type": "string", "enum": ["todo", "in_progress", "review", "done"] }
                }
            })
        },
        build: |arguments, project_id, _| {
            Ok(("board.update_card", with_project(arguments, project_id)))
        },
        format: |result| format!("Updated:\n{}", format_card(&result["card"])),
    },
    Tool {
        name: "board_add_note",
        description: "Add a progress note to a card's activity log, e.g. what you changed or why you are blocked.",
        schema: || {
            json!({
                "type": "object",
                "required": ["card_id", "text"],
                "properties": {
                    "card_id": { "type": "integer" },
                    "text": { "type": "string" }
                }
            })
        },
        build: |arguments, project_id, agent| {
            let mut arguments = with_project(arguments, project_id);
            arguments["agent"] = json!(agent);
            Ok(("board.add_note", arguments))
        },
        format: |_| "Note added.".to_string(),
    },
];

fn format_cards(result: &Value) -> String {
    let cards = result
        .get("cards")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if cards.is_empty() {
        return "The board is empty.".to_string();
    }
    cards
        .iter()
        .map(format_card)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A compact, readable card summary for the agent.
fn format_card(card: &Value) -> String {
    let mut text = format!(
        "#{} [{}] {}",
        card["id"],
        card["status"].as_str().unwrap_or("?"),
        card["title"].as_str().unwrap_or_default()
    );
    if let Some(assignee) = card["assignee"].as_str() {
        text.push_str(&format!(" (assigned to {assignee})"));
    }
    for (label, key) in [("Description", "description"), ("Acceptance", "acceptance")] {
        if let Some(value) = card[key].as_str().filter(|value| !value.trim().is_empty()) {
            text.push_str(&format!("\n{label}: {value}"));
        }
    }
    let tests: Vec<&str> = card["tests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !tests.is_empty() {
        text.push_str(&format!(
            "\nUI tests that must pass before done (run them with test_run): {}",
            tests.join(", ")
        ));
    }
    text
}
