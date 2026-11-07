//! Studio tools: an agent looks at the picture or video open in PilotHouse
//! Studio and changes it while the user watches; every change can be
//! undone, and nothing is written until it saves.

use serde_json::{Value, json};

use super::{Tool, with_project};

fn text(result: &Value) -> String {
    result["text"].as_str().unwrap_or_default().to_string()
}

const FILE: &str =
    "The file's path or name, when several are open (else the one the user used last).";

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "studio_get",
        description: concat!(
            "Shows the picture or video the user is editing in ",
            ph_brand::name!(),
            " Studio, with a guide to positions, times and marks. A picture: an image of it (the result, or with view \"original\" the original as turned, to place marks and crops on), its sizes, the edit so far (crop, colors, marks with their ids, size, frame) and how it saves. A video: its length, size and frame rate, the parts kept (and where they land in the result), its marks and when they show, and frames of the result at the source times in `at` (up to 6; else four across it). Look before changing it, and after, to check."
        ),
        schema: || {
            json!({ "type": "object", "properties": {
                "view": { "type": "string", "enum": ["result", "original"], "description": "What the images show (default result)." },
                "at": { "type": "array", "items": { "type": "number" }, "description": "A video: the source times (seconds) to see frames at." },
                "file": { "type": "string", "description": FILE }
            } })
        },
        build: |arguments, project_id, _| Ok(("studio.get", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "studio_edit",
        description: "Changes the picture or video open in Studio; the user sees each change at once and can undo it. Positions are pixels of the original (see studio_get). Marks (`add`, `update` by id with only what changes, `remove` ids or \"all\"): rect, ellipse, arrow, line, pen, text (align left, center or right), step, highlight, blur, pixelate, spotlight; on a video each also has `start` and `end` (source seconds) and `fade`. A picture also takes `set` (crop, adjust, resize, frame; null removes), `transform` (rotate_left, rotate_right, flip_horizontal, flip_vertical), `export` (format, quality, max_kb, palette). A video also takes `cut` [{from, to}], `trim` {from, to}, `speed` [{from, to, speed}] (sped-up parts are silent), `split` [t], `reset_timeline`, `set` (crop, resize, mute), `export` (format mp4/hevc/gif/webm, max_mb, fps, gif_width). `reset` goes back to the original. Example: {\"add\": [{\"type\": \"arrow\", \"from\": [100, 80], \"to\": [240, 160]}, {\"type\": \"blur\", \"x\": 400, \"y\": 30, \"width\": 220, \"height\": 28}], \"set\": {\"frame\": {\"padding\": 48}}}.",
        schema: || {
            json!({ "type": "object", "properties": {
                "set": { "type": "object", "description": "crop {x, y, width, height}, adjust {brightness, contrast, saturation, grayscale, invert, blur, sharpen}, resize {width, height}, frame {padding, background, radius, shadow}." },
                "transform": { "type": "array", "items": { "type": "string", "enum": ["rotate_left", "rotate_right", "flip_horizontal", "flip_vertical"] } },
                "add": { "type": "array", "items": { "type": "object" }, "description": "Marks, each with a type." },
                "update": { "type": "array", "items": { "type": "object" }, "description": "Each {id, …only what changes}." },
                "remove": { "description": "Mark ids, or \"all\"." },
                "export": { "type": "object", "description": "A picture: format (png, jpeg, webp, gif, bmp, tiff, ico), quality (JPEG 1-100), max_kb, palette. A video: format (mp4, hevc, gif, webm), max_mb, fps, gif_width." },
                "cut": { "type": "array", "items": { "type": "object" }, "description": "A video: [{from, to}] source seconds to remove." },
                "trim": { "type": "object", "description": "A video: {from, to}, the source seconds to keep." },
                "speed": { "type": "array", "items": { "type": "object" }, "description": "A video: [{from, to, speed}]." },
                "split": { "type": "array", "items": { "type": "number" } },
                "reset_timeline": { "type": "boolean" },
                "reset": { "type": "boolean" },
                "file": { "type": "string", "description": FILE }
            } })
        },
        build: |arguments, project_id, _| Ok(("studio.edit", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "studio_undo",
        description: "Undoes the last change in Studio (or redoes it, with redo: true).",
        schema: || {
            json!({ "type": "object", "properties": {
                "redo": { "type": "boolean" },
                "file": { "type": "string", "description": FILE }
            } })
        },
        build: |arguments, project_id, _| Ok(("studio.undo", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "studio_list",
        description: concat!(
            "The pictures and videos open in ",
            ph_brand::name!(),
            " Studio for this workspace."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("studio.list", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "studio_export",
        description: "Saves what is open in Studio as a new file, as its export settings say (the format follows the path's extension). `path` is relative to the file's folder; an existing file is replaced only with overwrite: true. A picture can be written over its original with replace_original: true, only when the user asked; a video never is. A video's export can take a while; the user sees its progress.",
        schema: || {
            json!({ "type": "object", "properties": {
                "path": { "type": "string" },
                "overwrite": { "type": "boolean" },
                "replace_original": { "type": "boolean" },
                "file": { "type": "string", "description": FILE }
            } })
        },
        build: |arguments, project_id, _| {
            Ok(("studio.export", with_project(arguments, project_id)))
        },
        format: text,
    },
];
