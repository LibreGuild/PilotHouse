//! The `slides.*` control methods behind the MCP bridge's `slides_*`
//! tools: an agent reads the presentation open in Studio, sees its slides
//! as pictures, and changes it with the same operations the editor uses,
//! as the user watches. Each call is one step the user can undo; the
//! presentation saves itself.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use gpui_kit::{App, AppContext as _, Entity, Task, WeakEntity};
use ph_control::ControlServer;
use serde_json::{Value, json};

use super::document::{DeckDocument, ExportFormat};
use super::model::*;
use super::ops::{Op, slide_index};

/// How decks work, for the agent.
pub const GUIDE: &str = "\
Units are points; the slide's size is in `size` (16:9 is 960 × 540), x right and y down from the top left. \
Colours are theme slots (dark1, light1, dark2, light2, accent1…accent6; `accent1+40` lighter, `accent1-25` darker, by percent) or #RRGGBB / #RRGGBBAA. Prefer slots so the theme can recolour the deck. Fonts: `heading`, `body`, `code` (the theme's) or a family name. \
Slides are named by id (s3) or number (3); elements by id (e12). Placeholders (title, subtitle, body, body2, heading, picture, statement, caption …) come from the layout and take its place and style: fill them with set_text rather than adding text boxes. \
Text anywhere may be Markdown: **bold**, *italic*, ~~strike~~, `code`, [label](url), lines starting `- ` are bullets (two spaces more per level), `1. ` numbered. \
Elements: {\"type\": …, \"frame\": {\"x\", \"y\", \"w\", \"h\", \"rotation\"}, \"name\", \"opacity\"} plus the kind's fields: \
shape {geometry: rect, round_rect, ellipse, triangle, right_triangle, diamond, parallelogram, trapezoid, pentagon, hexagon, octagon, chevron, home_plate, right_arrow, left_arrow, up_arrow, down_arrow, left_right_arrow, star5, star6, plus, heart, callout, line, path (with `path`: SVG path data in the frame's box); radius; fill; line {color, width, dash: solid/dash/dot/dash_dot, head, tail (arrowheads)}; shadow {} or {color, blur, distance, angle, opacity}; text}; \
text {text, fill, line} (a text box; text {paragraphs, anchor: top/middle/bottom, insets [l,t,r,b], autofit: shrink/none/grow, wrap} or Markdown); \
picture {media (a file in the space's media folder: slides_edit `import` copies one in), fit: cover/contain/stretch, crop [l,t,r,b] fractions, radius, line, shadow, alt}; \
table {columns (relative widths), rows: [[{text, fill, span: [cols, rows]}]], header, banded, border, accent, font_size}; \
chart {chart: column/bar/stacked_column/line/area/pie/doughnut/scatter, title, categories, series [{name, values, x (scatter), color}], legend, labels, x_title, y_title, font_size}; \
code {language, code, font_size, dark, line_numbers}; icon {icon (a Lucide name: slides_themes icons lists them), color, stroke}; \
svg {source} (any SVG you write: diagrams, illustrations; it must parse); group {children}; \
raw (a PowerPoint object PilotHouse keeps as it was, such as a video or an equation: move, resize or remove it, nothing more). \
A line whose ends are fixed to elements (a connector) is a shape with geometry line and `connect` {start: [element id, side], end: [id, side]} (sides 0 top, 1 right, 2 bottom, 3 left): its ends follow those elements as they move. \
Fills: a colour string, {\"type\": \"gradient\", \"stops\": [{\"at\": 0, \"color\": …}], \"angle\": 90, \"radial\": false}, {\"type\": \"picture\", \"media\": …, \"fit\": \"cover\"}, or \"none\". \
Runs (style_text `run`): font, size, bold, italic, underline, strike, color, highlight, link, baseline (percent: 30 superscript, -25 subscript). Paragraphs (`paragraph`): align left/center/right/justify, level, bullet (\"none\", {\"char\": \"→\"}, \"number\"), space_before, space_after, line_spacing (1.0 single). \
Ops (slides_edit `ops`, applied in order as one step, all or none): \
add_slide {layout, after or at, title, subtitle, body, body2, notes, elements} · duplicate_slide {slide} · remove_slide {slide} · move_slide {slide, to} · update_slide {slide, layout, background (a fill or null), notes, hidden, transition: none/fade/push/wipe/cover/zoom/dissolve, transition_from: left/right/top/bottom, transition_seconds} · \
animate {id, effect: appear/fade/fly_in/zoom/wipe, from: left/right/top/bottom, start: on_click/with_previous/after_previous, seconds, delay, at (its place in the slide's order, from 1)} (how an element comes in when presenting; again to change it) · unanimate {id} · \
add_comment {slide, text, id (the element it is about), x, y} · reply_comment {comment, text} · resolve_comment {comment, resolved} · remove_comment {comment} (comments are the user's notes to each other and to you: answer them by replying, and resolve them once done) · \
add_element {slide, element, behind} · update_element {id, set (merged in; null clears)} · remove_element {id} · order {id, to: front/back/forward/backward} · \
set_text {id, text (Markdown; styles kept where text is unchanged)} · style_text {id, paragraphs [indexes from 0] or find (text), run {…}, paragraph {…}} · replace {find, replace, id} · \
group {ids, name} · ungroup {id} · align {ids, to: left/center/right/top/middle/bottom, slide} · distribute {ids, axis: horizontal/vertical} · \
set_theme {theme (built-in id), saved (a saved theme's name), colors {slot: #hex}, fonts {heading, body, code}, background (a fill)} · set_size {preset or width and height, scale} · set_title {title} · add_section {name, slide} · remove_section {name}. \
Good slides: one idea each, short titles, at most about six bullets, generous space, the theme's colours; check your work with slides_get render and fix overlaps and text that does not fit.";

pub fn register_control_methods(cx: &mut App) {
    ControlServer::handle_async("slides.get", get, cx);
    ControlServer::handle("slides.edit", edit, cx);
    ControlServer::handle("slides.undo", undo, cx);
    ControlServer::handle("slides.themes", themes, cx);
    ControlServer::handle_async("slides.export", export, cx);
}

/// The presentations open, for agents to find.
#[derive(Default)]
struct Decks(Vec<WeakEntity<DeckDocument>>);

impl gpui_kit::Global for Decks {}

/// Keeps `document` findable by agents while it is open.
pub(crate) fn register(document: WeakEntity<DeckDocument>, cx: &mut App) {
    let decks = cx.default_global::<Decks>();
    decks.0.retain(|known| known.upgrade().is_some());
    decks.0.push(document);
}

/// The presentation of the space `project_id` names.
fn find(params: &Value, cx: &mut App) -> Result<Entity<DeckDocument>> {
    let project = params
        .get("project_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing project_id"))?;
    let decks = cx.default_global::<Decks>();
    decks.0.retain(|known| known.upgrade().is_some());
    let open: Vec<Entity<DeckDocument>> = decks.0.iter().filter_map(WeakEntity::upgrade).collect();
    let document = open
        .into_iter()
        .filter(|document| document.read(cx).project() == Some(project))
        .max_by_key(|document| document.read(cx).used)
        .ok_or_else(|| {
            anyhow!("no presentation is open for this chat; the slides_* tools work in a presentation's chat in Studio")
        })?;
    document.update(cx, |document, _| document.touch());
    Ok(document)
}

/// Text in a line, cut to `most` characters.
fn short(text: &str, most: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > most {
        format!("{}…", line.chars().take(most).collect::<String>())
    } else {
        line
    }
}

fn round(value: f32) -> f64 {
    (value as f64 * 10.).round() / 10.
}

/// An element in a line of the outline.
fn sketch(element: &Element) -> Value {
    let frame = element.frame;
    let kind = serde_json::to_value(&element.kind)
        .ok()
        .and_then(|kind| kind.get("type").cloned())
        .unwrap_or(Value::Null);
    let mut out = json!({
        "id": element.id,
        "type": kind,
        "frame": [round(frame.x), round(frame.y), round(frame.w), round(frame.h)],
    });
    if frame.rotation.abs() > 0.01 {
        out["rotation"] = json!(round(frame.rotation));
    }
    if let Some(place) = element.placeholder {
        out["placeholder"] = serde_json::to_value(place).unwrap_or(Value::Null);
    }
    if element.name != element.kind.label() && !element.name.is_empty() {
        out["name"] = json!(element.name);
    }
    match &element.kind {
        Kind::Picture(picture) => {
            out["media"] = json!(picture.media);
        }
        Kind::Chart(chart) => {
            out["chart"] = serde_json::to_value(chart.chart).unwrap_or(Value::Null);
            out["title"] = json!(chart.title);
        }
        Kind::Table(table) => {
            out["rows"] = json!(table.rows.len());
            out["columns"] = json!(table.rows.iter().map(Vec::len).max().unwrap_or(0));
            if let Some(first) = table.rows.first() {
                out["header"] = json!(
                    first
                        .iter()
                        .map(|cell| short(&cell.text.plain(), 30))
                        .collect::<Vec<_>>()
                );
            }
        }
        Kind::Icon(icon) => {
            out["icon"] = json!(icon.icon);
        }
        Kind::Group(group) => {
            out["children"] = group.children.iter().map(sketch).collect();
        }
        Kind::Code(code) => {
            out["code"] = json!(short(&code.code, 80));
        }
        _ => {}
    }
    if let Kind::Shape(Shape {
        connect: Some(connect),
        ..
    }) = &element.kind
    {
        out["connect"] = serde_json::to_value(connect).unwrap_or(Value::Null);
    }
    if let Kind::Raw(raw) = &element.kind {
        out["what"] = json!(raw.what);
    }
    if let Some(text) = element.kind.text() {
        let plain = text.plain();
        if !plain.trim().is_empty() {
            out["text"] = json!(short(&plain, 160));
        }
    }
    out
}

/// The deck in outline: every slide and its elements in short.
fn outline(document: &DeckDocument) -> Value {
    let deck = document.deck();
    let slides: Vec<Value> = deck
        .slides
        .iter()
        .enumerate()
        .map(|(index, slide)| {
            let mut out = json!({
                "n": index + 1,
                "id": slide.id,
                "layout": slide.layout,
                "elements": slide.elements.iter().map(sketch).collect::<Vec<_>>(),
            });
            if slide.hidden {
                out["hidden"] = json!(true);
            }
            if !slide.notes.trim().is_empty() {
                out["notes"] = json!(short(&slide.notes, 120));
            }
            if slide.background.is_some() {
                out["background"] = serde_json::to_value(&slide.background).unwrap_or(Value::Null);
            }
            if slide.transition != Transition::None {
                out["transition"] = json!({
                    "kind": slide.transition,
                    "from": slide.transition_from,
                    "seconds": round(slide.transition_seconds),
                });
            }
            if !slide.animations.is_empty() {
                out["animations"] = serde_json::to_value(&slide.animations).unwrap_or(Value::Null);
            }
            if !slide.comments.is_empty() {
                out["comments"] = slide
                    .comments
                    .iter()
                    .map(|comment| {
                        json!({
                            "id": comment.id,
                            "author": comment.author,
                            "text": comment.text,
                            "element": comment.element,
                            "resolved": comment.resolved,
                            "replies": comment.replies.iter().map(|reply| json!({ "author": reply.author, "text": reply.text })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
            }
            out
        })
        .collect();
    let selection = &document.selection;
    json!({
        "title": deck.title,
        "folder": document.folder().to_string_lossy(),
        "size": { "preset": deck.size.preset, "width": deck.size.width, "height": deck.size.height },
        "theme": {
            "name": deck.theme.name,
            "fonts": deck.theme.fonts,
            "colors": deck.theme.colors.iter().map(|(slot, hex)| (slot.clone(), json!(hex))).collect::<serde_json::Map<_, _>>(),
            "layouts": deck.theme.layouts.iter().map(|layout| layout.id.clone()).collect::<Vec<_>>(),
        },
        "sections": deck.sections,
        "selected": {
            "slide": selection.slide,
            "elements": selection.elements,
            "text": selection.text,
        },
        "slides": slides,
    })
}

/// The slides `value` names (numbers or ids, `"all"`), at most `most`.
fn slides_named(deck: &Deck, value: &Value, most: usize) -> Result<Vec<usize>> {
    let list: Vec<Value> = match value {
        Value::Null => Vec::new(),
        Value::String(text) if text == "all" => (1..=deck.slides.len()).map(|n| json!(n)).collect(),
        Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    let mut indexes = Vec::new();
    for item in list {
        let index = slide_index(deck, &item)?;
        if !indexes.contains(&index) {
            indexes.push(index);
        }
    }
    if indexes.len() > most {
        bail!("at most {most} slides at once");
    }
    Ok(indexes)
}

fn get(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let document = match find(&params, cx) {
        Ok(document) => document,
        Err(err) => return Task::ready(Err(err)),
    };
    let doc = document.read(cx);
    let deck = doc.deck();
    let detail = match params.get("slide").filter(|slide| !slide.is_null()) {
        Some(slide) => match slide_index(deck, slide) {
            Ok(index) => Some(index),
            Err(err) => return Task::ready(Err(err)),
        },
        None => None,
    };
    let render = match params.get("render") {
        Some(render) => slides_named(deck, render, 8),
        None => Ok(detail.into_iter().collect()),
    };
    let render = match render {
        Ok(render) => render,
        Err(err) => return Task::ready(Err(err)),
    };
    let mut text = match detail {
        Some(index) => format!(
            "Slide {} in full:\n{}",
            index + 1,
            serde_json::to_string(&*deck.slides[index]).unwrap_or_default()
        ),
        None => format!(
            "The presentation (elements in short; slides_get with `slide` shows one in full):\n{}",
            serde_json::to_string(&outline(doc)).unwrap_or_default()
        ),
    };
    if params.get("guide").and_then(Value::as_bool) != Some(false) {
        text.push_str("\n\n");
        text.push_str(GUIDE);
    }
    if !render.is_empty() {
        text.push_str(&format!(
            "\n\nAttached: slide {} as drawn, 960 pixels wide ({} pixels a point).",
            render
                .iter()
                .map(|index| (index + 1).to_string())
                .collect::<Vec<_>>()
                .join(", "),
            round(960. / deck.size.width.max(1.)),
        ));
    }
    let deck = deck.clone();
    let media = doc.media();
    cx.background_spawn(async move {
        let mut images = Vec::new();
        for index in render {
            let image = super::raster::slide(&deck, index, 960, Some(&media), false)?;
            let mut png = Vec::new();
            image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
            images.push(json!({
                "data": base64::engine::general_purpose::STANDARD.encode(&png),
                "mimeType": "image/png",
            }));
        }
        Ok(json!({ "text": text, "images": images }))
    })
}

fn edit(params: Value, cx: &mut App) -> Result<Value> {
    let document = find(&params, cx)?;
    let mut done = Vec::new();
    // Pictures first, so ops can use them.
    let mut imported = Vec::new();
    if let Some(files) = params.get("import").and_then(Value::as_array) {
        let space = document.read(cx).space().clone();
        for file in files {
            let path = file.as_str().ok_or_else(|| anyhow!("import: paths"))?;
            let mut path = PathBuf::from(path);
            if path.is_relative() {
                path = space.path.join(path);
            }
            let name = space.add_media(&path)?;
            done.push(format!("imported {} as media `{name}`", path.display()));
            imported.push(name);
        }
    }
    let ops: Vec<Op> = match params.get("ops") {
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                serde_json::from_value(item.clone()).map_err(|err| anyhow!("op {}: {err}", ix + 1))
            })
            .collect::<Result<_>>()?,
        Some(Value::Null) | None => Vec::new(),
        Some(_) => bail!("ops: a list of operations"),
    };
    if ops.is_empty() && imported.is_empty() {
        bail!("nothing to change: give `ops` (see slides_get's guide)");
    }
    if !ops.is_empty() {
        let count = ops.len();
        let results = document.update(cx, |document, cx| {
            document.settle();
            let results = document.apply("agent", ops, cx);
            document.settle();
            results
        })?;
        let made: Vec<String> = results.into_iter().flatten().collect();
        done.push(if made.is_empty() {
            format!("{count} change{}", if count == 1 { "" } else { "s" })
        } else {
            format!(
                "{count} change{} (made or found: {})",
                if count == 1 { "" } else { "s" },
                made.join(", ")
            )
        });
    }
    Ok(json!({
        "text": format!(
            "Done: {}. The user sees it now; look with slides_get render to check it.",
            done.join("; ")
        ),
        "imported": imported,
    }))
}

fn undo(params: Value, cx: &mut App) -> Result<Value> {
    let document = find(&params, cx)?;
    let redo = params.get("redo").and_then(Value::as_bool) == Some(true);
    let changed = document.update(cx, |document, cx| {
        let can = if redo {
            document.can_redo()
        } else {
            document.can_undo()
        };
        if redo {
            document.redo(cx)
        } else {
            document.undo(cx)
        }
        can
    });
    let text = match (changed, redo) {
        (true, false) => "Undone.",
        (true, true) => "Redone.",
        (false, false) => "Nothing to undo.",
        (false, true) => "Nothing to redo.",
    };
    Ok(json!({ "text": text }))
}

fn themes(params: Value, cx: &mut App) -> Result<Value> {
    let document = find(&params, cx)?;
    let deck = document.read(cx).deck();
    let built_in: Vec<Value> = super::theme::builtins()
        .filter_map(|(id, name)| {
            let theme = super::theme::builtin(id)?;
            Some(json!({
                "id": id,
                "name": name,
                "fonts": theme.fonts,
                "colors": theme.colors.iter().map(|(slot, hex)| (slot.clone(), json!(hex))).collect::<serde_json::Map<_, _>>(),
            }))
        })
        .collect();
    let saved: Vec<String> = super::space::saved_themes(&super::space::themes_dir())
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let layouts: Vec<Value> = deck
        .theme
        .layouts
        .iter()
        .map(|layout| {
            json!({
                "id": layout.id,
                "name": layout.name,
                "placeholders": layout.placeholders.iter().map(|place| json!({
                    "kind": place.kind,
                    "frame": [round(place.frame.x), round(place.frame.y), round(place.frame.w), round(place.frame.h)],
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut out = json!({
        "built_in": built_in,
        "saved": saved,
        "current": deck.theme.name,
        "layouts": layouts,
        "sizes": SIZES.iter().map(|(id, name, w, h)| json!({ "id": id, "name": name, "width": w, "height": h })).collect::<Vec<_>>(),
    });
    let icons = params.get("icons");
    if let Some(icons) = icons.filter(|icons| !icons.is_null() && icons != &&json!(false)) {
        let filter = icons.as_str().unwrap_or_default().to_lowercase();
        let names: Vec<String> = super::icons::names()
            .into_iter()
            .filter(|name| filter.is_empty() || name.contains(&filter))
            .collect();
        out["icons"] = json!(names);
    }
    Ok(json!({
        "text": format!(
            "{}\n\nset_theme with `theme` (a built-in id) or `saved` (a name) restyles every slide; colors and fonts change the current theme.",
            serde_json::to_string(&out)?
        ),
    }))
}

fn export(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let document = match find(&params, cx) {
        Ok(document) => document,
        Err(err) => return Task::ready(Err(err)),
    };
    let doc = document.read(cx);
    let format = match params.get("format").and_then(Value::as_str) {
        Some(format) => match ExportFormat::from_extension(format) {
            Some(format) => format,
            None => return Task::ready(Err(anyhow!("format: pptx, pdf or png"))),
        },
        None => params
            .get("path")
            .and_then(Value::as_str)
            .and_then(|path| std::path::Path::new(path).extension())
            .and_then(|extension| ExportFormat::from_extension(&extension.to_string_lossy()))
            .unwrap_or(ExportFormat::Pptx),
    };
    let mut path = match params.get("path").and_then(Value::as_str) {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_relative() {
                doc.space().exports().join(path)
            } else {
                path
            }
        }
        None => doc.export_path(format.extension()),
    };
    if path.extension().is_none() {
        path.set_extension(format.extension());
    }
    if path.exists() && params.get("overwrite").and_then(Value::as_bool) != Some(true) {
        return Task::ready(Err(anyhow!(
            "{} exists; give overwrite: true to replace it, or another path",
            path.display()
        )));
    }
    let slides = match params.get("slides") {
        Some(slides) => match slides_named(doc.deck(), slides, 10_000) {
            Ok(slides) => slides,
            Err(err) => return Task::ready(Err(err)),
        },
        None => Vec::new(),
    };
    let notes = params.get("notes").and_then(Value::as_bool) == Some(true);
    let width = params
        .get("width")
        .and_then(Value::as_u64)
        .map_or(1920, |width| width.clamp(320, 7680) as u32);
    let writing = document.update(cx, |document, cx| {
        document.export(format, path, slides, notes, width, cx)
    });
    cx.spawn(async move |_| {
        let written = writing.await?;
        let size = std::fs::metadata(&written)
            .map(|meta| {
                if meta.is_dir() {
                    String::new()
                } else {
                    format!(" ({})", crate::control::kilobytes(meta.len() as usize))
                }
            })
            .unwrap_or_default();
        Ok(json!({ "text": format!("Wrote {}{size}.", written.display()) }))
    })
}
