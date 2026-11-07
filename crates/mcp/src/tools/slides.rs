//! Slides tools: an agent reads the presentation open in PilotHouse
//! Studio (beside its chat), looks at its slides, and changes them with
//! the editor's own operations while the user watches; every call can be
//! undone, and the presentation saves itself.

use serde_json::{Value, json};

use super::{Tool, with_project};

fn text(result: &Value) -> String {
    result["text"].as_str().unwrap_or_default().to_string()
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "slides_get",
        description: concat!(
            "Shows the presentation open in ",
            ph_brand::name!(),
            " Studio beside this chat: its size and theme, every slide with its elements in short (ids, kinds, frames in points, text), what the user has selected (\"this\" in their message), and a guide to the deck model and every operation. `slide` (a number or id) shows one slide in full; `render` (numbers or ids, up to 8, or \"all\" for small decks) attaches pictures of slides as drawn. Look before changing, and render after, to check the result."
        ),
        schema: || {
            json!({ "type": "object", "properties": {
                "slide": { "description": "A slide number (from 1) or id, shown in full (and rendered unless `render` says otherwise)." },
                "render": { "description": "Slides to attach pictures of: a list of numbers or ids, or \"all\"." },
                "guide": { "type": "boolean", "description": "Include the guide (default true; leave it out once read)." }
            } })
        },
        build: |arguments, project_id, _| Ok(("slides.get", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "slides_edit",
        description: "Changes the presentation open in Studio with a list of operations, applied in order as one step the user can undo (all or none); they see it at once. Operations: add_slide, duplicate_slide, remove_slide, move_slide, update_slide (layout, background, notes, transitions), add_element, update_element, remove_element, order, set_text, style_text, replace, group, ungroup, align, distribute, animate, unanimate, add_comment, reply_comment, resolve_comment, remove_comment, set_theme, set_size, set_title, add_section, remove_section (fields in slides_get's guide). Text may be Markdown. `import` copies pictures (paths, relative to the presentation's folder) into its media first, for picture elements and fills to use by the names returned. Example: {\"ops\": [{\"op\": \"add_slide\", \"layout\": \"title_content\", \"title\": \"Why now\", \"body\": \"- Costs fell 40%\\n- Demand doubled\"}, {\"op\": \"add_element\", \"slide\": 2, \"element\": {\"type\": \"icon\", \"icon\": \"trending-up\", \"color\": \"accent1\", \"frame\": {\"x\": 820, \"y\": 40, \"w\": 80, \"h\": 80}}}]}.",
        schema: || {
            json!({ "type": "object", "properties": {
                "ops": { "type": "array", "items": { "type": "object" }, "description": "Operations, each with `op` and its fields." },
                "import": { "type": "array", "items": { "type": "string" }, "description": "Pictures (PNG, JPEG, GIF, WebP, SVG) to copy into the presentation's media." }
            } })
        },
        build: |arguments, project_id, _| Ok(("slides.edit", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "slides_undo",
        description: "Undoes the last change to the presentation (or redoes it, with redo: true).",
        schema: || json!({ "type": "object", "properties": { "redo": { "type": "boolean" } } }),
        build: |arguments, project_id, _| Ok(("slides.undo", with_project(arguments, project_id))),
        format: text,
    },
    Tool {
        name: "slides_themes",
        description: "The themes a presentation can take (built in, with their colours and fonts, and the user's saved ones), the current theme's layouts and their placeholders' places, the slide sizes, and with `icons` (true, or a word to filter by) the icon names an icon element takes.",
        schema: || {
            json!({ "type": "object", "properties": {
                "icons": { "description": "true for every icon name, or a word to filter them (\"arrow\", \"chart\")." }
            } })
        },
        build: |arguments, project_id, _| {
            Ok(("slides.themes", with_project(arguments, project_id)))
        },
        format: text,
    },
    Tool {
        name: "slides_export",
        description: "Writes the presentation as PPTX (editable in PowerPoint, Keynote, Google Slides), PDF (with notes pages when `notes`) or PNG (one picture, or a folder of them for several slides; `width` in pixels, default 1920). `path` is relative to the presentation's exports folder (default: its name); an existing file is replaced only with overwrite: true. Only when the user asks.",
        schema: || {
            json!({ "type": "object", "properties": {
                "format": { "type": "string", "enum": ["pptx", "pdf", "png"] },
                "path": { "type": "string" },
                "slides": { "description": "Slide numbers or ids (default all)." },
                "notes": { "type": "boolean" },
                "width": { "type": "integer" },
                "overwrite": { "type": "boolean" }
            } })
        },
        build: |arguments, project_id, _| {
            Ok(("slides.export", with_project(arguments, project_id)))
        },
        format: text,
    },
];
