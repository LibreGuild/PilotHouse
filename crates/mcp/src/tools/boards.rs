//! Whiteboard tools: what an agent needs beside its own file tools to work
//! on a board: in the chat beside a board, or in the chat of the workspace
//! a board was made from. It changes the board by editing the board's
//! file; these make a board for a workspace, say how a board is written,
//! and what is on one now.

use serde_json::{Value, json};

use super::{Tool, with_project};

fn text(result: &Value) -> String {
    result["text"].as_str().unwrap_or_default().to_string()
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "board_create",
        description: concat!(
            "Makes a ",
            ph_brand::name!(),
            " whiteboard for this workspace, when the user asks to draw, map or sketch something about what is here: a flowchart, an architecture or system diagram, a map of the code, a plan on notes. The board is kept with the user's artifacts (not in the workspace) and shows beside this chat while you draw; the answer says where its file is. Then read board_guide once, write the board's file, and call board_look. Use this, not artifact_publish and not a picture or a Mermaid block: the user can then move things, add to it by hand and pin notes for you. Not in a board's own chat, which has its board."
        ),
        schema: || {
            json!({ "type": "object", "properties": {
                "title": { "type": "string", "description": "What the board is called." }
            }, "required": ["title"] })
        },
        build: |arguments, project_id, _| Ok(("board.create", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "board_list",
        description: "The whiteboards of this workspace (made with board_create), each with its id, title and where its file is; the first is the one board_look shows unless `board` names another.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("board.list", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "board_guide",
        description: concat!(
            "How a ",
            ph_brand::name!(),
            " whiteboard is written: its one file (board.json), the kinds of things on a board (notes, shapes, text, sections, arrows tied to what they join, tables, stickers, pictures), their fields and colours, and how to lay out a flow, a map or a plan so that it reads well. Read it once before your first change to a board in a chat."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |_, _, _| Ok(("board.guide", json!({}))),
        format: text,
    },
    Tool {
        name: "board_look",
        description: "What is on this chat's whiteboard (the one beside it, or this workspace's): its sections with what lies in each, what lies loose, what is joined to what (each thing with its id and place), what the user has selected (\"this\" in their message), and whether they changed the board by hand since you last looked. Call it before changing a board, and after, to check what you wrote reads as you meant.",
        schema: || {
            json!({ "type": "object", "properties": {
                "board": { "type": "string", "description": "Which board, by its id or title, when this workspace has several (board_list shows them). Left out: the board beside this chat, or the one of this workspace changed last." }
            } })
        },
        build: |arguments, project_id, _| Ok(("board.look", with_project(arguments, project_id))),
        format: text,
    },
];
