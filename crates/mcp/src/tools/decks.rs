//! Deck tools: what an agent needs beside its own file tools to work on a
//! deck (Slides): in the chat beside a deck, or in the chat of the
//! workspace a deck was made from. It changes the deck by editing the
//! deck's files; these make a deck for a workspace, say how a deck is
//! written, show how it looks as drawn (what does not fit, pictures of
//! slides), read and answer the user's comments, turn a PowerPoint file
//! into slides, and export.

use serde_json::{Value, json};

use super::{Tool, with_project};

fn text(result: &Value) -> String {
    result["text"].as_str().unwrap_or_default().to_string()
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "deck_create",
        description: concat!(
            "Makes a ",
            ph_brand::name!(),
            " Slides deck for this workspace, when the user asks for a presentation, a deck, slides or a PowerPoint about what is here. The deck is kept with the user's artifacts (not in the workspace) and shows beside this chat while you write it; the answer says where its files are. Then read deck_guide once, write the slides as files, and call deck_look. Use this, not artifact_publish and not a script that writes a .pptx: the user can then change the deck by hand, comment on it, present it and export it to PowerPoint or PDF. Not in a deck's own chat, which has its deck."
        ),
        schema: || {
            json!({ "type": "object", "properties": {
                "title": { "type": "string", "description": "What the deck is called." },
                "look": { "type": "string", "enum": ["paper", "midnight", "clean", "forest", "bold", "slate"], "description": "The look it starts in (default paper: warm, serif headings). midnight is dark, clean is plain and light. You can rewrite deck.css after." }
            }, "required": ["title"] })
        },
        build: |arguments, project_id, _| Ok(("deck.create", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "deck_list",
        description: "The decks of this workspace (made with deck_create), each with its id, title, number of slides and where its files are; the first is the one the deck_ tools work on unless `deck` names another.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("deck.list", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "deck_guide",
        description: concat!(
            "How a ",
            ph_brand::name!(),
            " Slides deck is made: its files (deck.json, deck.css, one HTML file a slide), how to write a slide that fits and that the user can change by hand, the deck's look and what it offers, charts, shapes, pictures, and how to work on a deck with the user. Read it once before your first change to a deck in a chat."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |_, _, _| Ok(("deck.guide", json!({}))),
        format: text,
    },
    Tool {
        name: "deck_look",
        description: "Shows this chat's deck (the one beside it, or this workspace's) as it is drawn: every slide with its file and heading, what does not fit or cannot be read on each (text running off the slide or cut off in its box, text too small or too faint, a picture that did not load), the slide on the stage and what the user has selected, and the open comments. Call it after changing slides, and fix what it finds before you answer. `render` attaches pictures of slides (numbers from 1, files, or \"all\"; 8 at most a call): look at a slide whose layout you are unsure of.",
        schema: || {
            json!({ "type": "object", "properties": {
                "render": { "description": "Slides to attach pictures of: a number, a file (slides/cover.html), a list of them, or \"all\"." },
                "deck": { "type": "string", "description": "Which deck, by its id or title, when this workspace has several (deck_list shows them). Left out: the deck beside this chat, or the one of this workspace changed last." }
            } })
        },
        build: |arguments, project_id, _| Ok(("deck.look", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "deck_comments",
        description: "The user's comments on this chat's deck (the one beside it, or this workspace's). `op` \"list\" (the default) shows the open ones, each with its id, slide and the element it is about (its data-id); \"reply\" answers one (`id`, `text`); \"resolve\" marks it done, with `text` as the answer. After making what a comment asks for, resolve it with a sentence saying what you changed.",
        schema: || {
            json!({ "type": "object", "properties": {
                "op": { "type": "string", "enum": ["list", "reply", "resolve"] },
                "id": { "type": "string", "description": "The comment (c3)." },
                "text": { "type": "string", "description": "What you answer." },
                "deck": { "type": "string", "description": "Which deck, by its id or title, when this workspace has several (deck_list shows them). Left out: the deck beside this chat, or the one of this workspace changed last." }
            } })
        },
        build: |arguments, project_id, _| {
            Ok(("deck.comments", with_project(arguments, project_id)))
        },
        format: text,
    },
    Tool {
        name: "deck_import",
        description: "Turns a PowerPoint file (.pptx) into slides of this chat's deck (the one beside it, or this workspace's), after the ones it has: text as text (each box at the place it had), pictures into media/, tables as tables, charts as charts with their numbers, speaker notes kept. Animations, SmartArt, video and the file's template are left out (the answer says what was). `path` is the file (one the user gave you; relative paths are from the deck's folder, where references/ is). In an empty deck the deck takes the file's size, colours and fonts; `look: true` does so in any deck. Then call deck_look and tidy the slides.",
        schema: || {
            json!({ "type": "object", "properties": {
                "path": { "type": "string", "description": "The .pptx file." },
                "look": { "type": "boolean", "description": "Take the file's size, colours and fonts as the deck's look (default: only in a deck without slides)." },
                "deck": { "type": "string", "description": "Which deck, by its id or title, when this workspace has several (deck_list shows them). Left out: the deck beside this chat, or the one of this workspace changed last." }
            }, "required": ["path"] })
        },
        build: |arguments, project_id, _| Ok(("deck.import", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "deck_export",
        description: "Writes this chat's deck (the one beside it, or this workspace's) into its exports folder: `format` \"pdf\" (a page a slide, as drawn), \"pptx\" (PowerPoint: text, tables and charts stay editable; set in the deck's fonts), \"pptx-basic\" (the same in fonts every computer has), \"html\" (one web page that needs nothing beside it) or \"png\" (a picture of `slide`, a number or a file; the first without). Never writes over a file. Only when the user asks.",
        schema: || {
            json!({ "type": "object", "properties": {
                "format": { "type": "string", "enum": ["pdf", "pptx", "pptx-basic", "html", "png"] },
                "slide": { "description": "For png: the slide, by number (from 1) or file." },
                "deck": { "type": "string", "description": "Which deck, by its id or title, when this workspace has several (deck_list shows them). Left out: the deck beside this chat, or the one of this workspace changed last." }
            }, "required": ["format"] })
        },
        build: |arguments, project_id, _| Ok(("deck.export", with_project(arguments, project_id))),
        format: text,
    },
];
