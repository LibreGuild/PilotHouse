//! Changes to a deck: one set for the editor and for agents (who send
//! them as JSON through `slides_edit`). Each says what it changed or made.
//!
//! Slides are named by id (`s3`) or number (`3`, from 1); elements by id
//! (`e12`), which stays the element's for good. Elements are given as JSON:
//! the kind's fields may sit beside `frame` with a `type`
//! (`{"type": "shape", "geometry": "ellipse", "frame": {…}}`), and text may
//! be Markdown.

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

use super::model::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// A new slide in a layout, after a slide or at a number (else last),
    /// its placeholders filled from `title`, `subtitle`, `body` (Markdown),
    /// `body2`, with more elements if given.
    AddSlide {
        #[serde(default)]
        layout: Option<String>,
        #[serde(default)]
        after: Option<Value>,
        #[serde(default)]
        at: Option<usize>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        subtitle: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        body2: Option<String>,
        #[serde(default)]
        notes: Option<String>,
        #[serde(default)]
        elements: Vec<Value>,
    },
    DuplicateSlide {
        slide: Value,
    },
    RemoveSlide {
        slide: Value,
    },
    /// To number `to` (from 1).
    MoveSlide {
        slide: Value,
        to: usize,
    },
    UpdateSlide {
        slide: Value,
        #[serde(default)]
        layout: Option<String>,
        /// A fill, or `null` for the layout's.
        #[serde(default)]
        background: Option<Value>,
        #[serde(default)]
        notes: Option<String>,
        #[serde(default)]
        hidden: Option<bool>,
        #[serde(default)]
        transition: Option<Transition>,
        /// Push, wipe, cover: from where.
        #[serde(default)]
        transition_from: Option<Direction>,
        #[serde(default)]
        transition_seconds: Option<f32>,
    },
    /// Element `id` comes in when presenting (the fields given change an
    /// animation it has); `at` is its place in the slide's order (from 1;
    /// else last).
    Animate {
        id: String,
        #[serde(default)]
        effect: Option<Effect>,
        #[serde(default)]
        from: Option<Direction>,
        #[serde(default)]
        start: Option<Start>,
        #[serde(default)]
        seconds: Option<f32>,
        #[serde(default)]
        delay: Option<f32>,
        #[serde(default)]
        at: Option<usize>,
    },
    /// No animation on element `id`.
    Unanimate {
        id: String,
    },
    /// A comment on a slide: at an element (`id`), or a place (`x`, `y`).
    AddComment {
        slide: Value,
        text: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
        #[serde(default)]
        author: Option<String>,
    },
    ReplyComment {
        comment: String,
        text: String,
        #[serde(default)]
        author: Option<String>,
    },
    ResolveComment {
        comment: String,
        #[serde(default = "yes")]
        resolved: bool,
    },
    RemoveComment {
        comment: String,
    },
    AddElement {
        slide: Value,
        element: Value,
        /// Behind the others (else in front).
        #[serde(default)]
        behind: bool,
    },
    /// Merges `set` into the element (a JSON merge patch: `null` clears).
    UpdateElement {
        id: String,
        set: Value,
    },
    RemoveElement {
        id: String,
    },
    /// `front`, `back`, `forward` or `backward`.
    Order {
        id: String,
        to: String,
    },
    /// The element's text, as Markdown (styles the paragraphs keep when
    /// their text did not change).
    SetText {
        id: String,
        text: String,
    },
    /// Style for the runs (and paragraph settings) of the given paragraphs
    /// (from 0; all when absent), or of the text matching `find`.
    StyleText {
        id: String,
        #[serde(default)]
        paragraphs: Option<Vec<usize>>,
        #[serde(default)]
        find: Option<String>,
        #[serde(default)]
        run: Value,
        #[serde(default)]
        paragraph: Value,
    },
    /// Replaces text everywhere (or in one element), keeping its style.
    Replace {
        find: String,
        replace: String,
        #[serde(default)]
        id: Option<String>,
    },
    Group {
        ids: Vec<String>,
        #[serde(default)]
        name: Option<String>,
    },
    Ungroup {
        id: String,
    },
    /// Lines elements up: `left`, `center`, `right`, `top`, `middle`,
    /// `bottom`; against the slide when `slide` is true or there is one.
    Align {
        ids: Vec<String>,
        to: String,
        #[serde(default)]
        slide: bool,
    },
    /// Spaces three or more elements evenly: `horizontal` or `vertical`.
    Distribute {
        ids: Vec<String>,
        axis: String,
    },
    /// A built-in theme by id, or a whole theme, or colours and fonts over
    /// the current one.
    SetTheme {
        #[serde(default)]
        theme: Option<String>,
        #[serde(default)]
        custom: Option<Box<Theme>>,
        /// A theme the user saved, by name.
        #[serde(default)]
        saved: Option<String>,
        /// `{"accent1": "#2563EB", …}` (or `[["accent1", "#2563EB"], …]`).
        #[serde(default)]
        colors: Value,
        #[serde(default)]
        fonts: Option<Value>,
        #[serde(default)]
        background: Option<Value>,
    },
    /// A preset (`16:9`, `4:3`, `a4`, `9:16` …) or a width and height in
    /// points; content scaled to it unless `scale` is false.
    SetSize {
        #[serde(default)]
        preset: Option<String>,
        #[serde(default)]
        width: Option<f32>,
        #[serde(default)]
        height: Option<f32>,
        #[serde(default = "yes")]
        scale: bool,
    },
    SetTitle {
        title: String,
    },
    AddSection {
        name: String,
        slide: Value,
    },
    RemoveSection {
        name: String,
    },
}

fn yes() -> bool {
    true
}

/// Applies `op` to `deck`: what it made (a slide or element id), if
/// anything.
pub fn apply(deck: &mut Deck, op: Op) -> Result<Option<String>> {
    let made = apply_op(deck, op)?;
    tidy(deck);
    Ok(made)
}

/// Animations of elements gone (removed, grouped) are let go, and
/// comments on them stay where they were.
pub fn tidy(deck: &mut Deck) {
    for slide in &mut deck.slides {
        let alive = |id: &String| slide.elements.iter().any(|element| &element.id == id);
        let stale_animation = slide
            .animations
            .iter()
            .any(|animation| !alive(&animation.id));
        let stale_comment = slide
            .comments
            .iter()
            .any(|comment| comment.element.as_ref().is_some_and(|id| !alive(id)));
        if !stale_animation && !stale_comment {
            continue;
        }
        let slide = Arc::make_mut(slide);
        let ids: Vec<String> = slide
            .elements
            .iter()
            .map(|element| element.id.clone())
            .collect();
        slide
            .animations
            .retain(|animation| ids.contains(&animation.id));
        for comment in &mut slide.comments {
            if comment.element.as_ref().is_some_and(|id| !ids.contains(id)) {
                comment.element = None;
            }
        }
    }
}

fn apply_op(deck: &mut Deck, op: Op) -> Result<Option<String>> {
    match op {
        Op::AddSlide {
            layout,
            after,
            at,
            title,
            subtitle,
            body,
            body2,
            notes,
            elements,
        } => {
            let layout = layout.unwrap_or_else(|| {
                if deck.slides.is_empty() {
                    "title".into()
                } else {
                    "title_content".into()
                }
            });
            if deck.theme.layout(&layout).is_none() {
                bail!(
                    "no layout `{layout}`; the theme has {}",
                    layout_ids(&deck.theme)
                );
            }
            let mut slide = new_slide(deck, &layout);
            for (kind, text) in [
                (PlaceholderKind::Title, title),
                (PlaceholderKind::Subtitle, subtitle),
                (PlaceholderKind::Body, body),
                (PlaceholderKind::Body2, body2),
            ] {
                let Some(text) = text else {
                    continue;
                };
                // Titles fill whichever title-like place the layout has.
                let element = slide.elements.iter_mut().find(|element| {
                    element.placeholder == Some(kind)
                        || (kind == PlaceholderKind::Title
                            && element.placeholder == Some(PlaceholderKind::Statement))
                        || (kind == PlaceholderKind::Subtitle
                            && element.placeholder == Some(PlaceholderKind::Caption))
                });
                let Some(element) = element else {
                    bail!("layout `{layout}` has no place for the {kind:?} text");
                };
                if let Some(body) = element.kind.text_mut() {
                    let fresh = super::markdown::to_text(&text);
                    body.paragraphs = fresh.paragraphs;
                }
            }
            slide.notes = notes.unwrap_or_default();
            for element in elements {
                let element = element_from(deck, element)?;
                slide.elements.push(element);
            }
            let index = match (after, at) {
                (Some(after), _) => slide_index(deck, &after)? + 1,
                (None, Some(at)) => at.saturating_sub(1).min(deck.slides.len()),
                (None, None) => deck.slides.len(),
            };
            let id = slide.id.clone();
            deck.slides.insert(index, Arc::new(slide));
            Ok(Some(id))
        }
        Op::DuplicateSlide { slide } => {
            let index = slide_index(deck, &slide)?;
            let mut copy = (*deck.slides[index]).clone();
            copy.id = deck.new_id("s");
            // What names an element by its id (animations, comments,
            // lines' ends) names the copy's own from here on.
            let mut renamed: HashMap<String, String> = HashMap::new();
            for element in &mut copy.elements {
                let mut before = Vec::new();
                visit(element, &mut |element| before.push(element.id.clone()));
                renumber(deck, element);
                let mut before = before.into_iter();
                visit(element, &mut |element| {
                    if let Some(old) = before.next() {
                        renamed.insert(old, element.id.clone());
                    }
                });
            }
            let rename = |id: &mut String| {
                if let Some(new) = renamed.get(id) {
                    *id = new.clone();
                }
            };
            for animation in &mut copy.animations {
                rename(&mut animation.id);
            }
            for comment in &mut copy.comments {
                // An id of its own: with the original's, a reply here
                // would go to the original.
                comment.id = deck.new_id("c");
                if let Some(element) = &mut comment.element {
                    rename(element);
                }
            }
            for element in &mut copy.elements {
                each_element(element, &mut |element| {
                    if let Kind::Shape(Shape {
                        connect: Some(connect),
                        ..
                    }) = &mut element.kind
                    {
                        for (end, _) in [&mut connect.start, &mut connect.end].into_iter().flatten()
                        {
                            rename(end);
                        }
                    }
                });
            }
            let id = copy.id.clone();
            deck.slides.insert(index + 1, Arc::new(copy));
            Ok(Some(id))
        }
        Op::RemoveSlide { slide } => {
            let index = slide_index(deck, &slide)?;
            let removed = deck.slides.remove(index);
            // A section starting there starts at the next slide, unless
            // one starts there already: it was this slide alone, and goes
            // with it (two sections on one slide would show as one).
            let next = deck
                .slides
                .get(index)
                .map(|slide| slide.id.clone())
                .filter(|next| !deck.sections.iter().any(|section| &section.slide == next));
            deck.sections.retain_mut(|section| {
                if section.slide != removed.id {
                    return true;
                }
                match &next {
                    Some(next) => {
                        section.slide = next.clone();
                        true
                    }
                    None => false,
                }
            });
            Ok(None)
        }
        Op::MoveSlide { slide, to } => {
            let index = slide_index(deck, &slide)?;
            let moved = deck.slides.remove(index);
            let to = to.saturating_sub(1).min(deck.slides.len());
            deck.slides.insert(to, moved);
            Ok(None)
        }
        Op::UpdateSlide {
            slide,
            layout,
            background,
            notes,
            hidden,
            transition,
            transition_from,
            transition_seconds,
        } => {
            let index = slide_index(deck, &slide)?;
            if let Some(layout) = layout {
                change_layout(deck, index, &layout)?;
            }
            let slide = Arc::make_mut(&mut deck.slides[index]);
            if let Some(background) = background {
                in_range(&background).context("background")?;
                slide.background = if background.is_null() {
                    None
                } else {
                    Some(serde_json::from_value(background).context("background")?)
                };
            }
            if let Some(notes) = notes {
                slide.notes = notes;
            }
            if let Some(hidden) = hidden {
                slide.hidden = hidden;
            }
            if let Some(transition) = transition {
                slide.transition = transition;
            }
            if let Some(from) = transition_from {
                slide.transition_from = from;
            }
            if let Some(seconds) = transition_seconds {
                slide.transition_seconds = seconds.clamp(0.1, 5.);
            }
            Ok(None)
        }
        Op::Animate {
            id,
            effect,
            from,
            start,
            seconds,
            delay,
            at,
        } => {
            let (index, _) = locate(deck, &id).with_context(|| {
                format!(
                    "{} (animations are for elements on a slide, not in a group)",
                    missing(&id)
                )
            })?;
            let slide = Arc::make_mut(&mut deck.slides[index]);
            let existing = slide
                .animations
                .iter()
                .position(|animation| animation.id == id);
            let mut animation = existing
                .map(|ix| slide.animations.remove(ix))
                .unwrap_or_else(|| Animation {
                    id: id.clone(),
                    ..Animation::default()
                });
            if let Some(effect) = effect {
                animation.effect = effect;
            }
            if let Some(from) = from {
                animation.from = from;
            }
            if let Some(start) = start {
                animation.start = start;
            }
            if let Some(seconds) = seconds {
                animation.seconds = seconds.clamp(0.05, 10.);
            }
            if let Some(delay) = delay {
                animation.delay = delay.clamp(0., 30.);
            }
            let place = match (at, existing) {
                (Some(at), _) => at.saturating_sub(1),
                (None, Some(ix)) => ix,
                (None, None) => slide.animations.len(),
            };
            let place = place.min(slide.animations.len());
            slide.animations.insert(place, animation);
            Ok(None)
        }
        Op::Unanimate { id } => {
            let (index, _) = locate(deck, &id).with_context(|| missing(&id))?;
            Arc::make_mut(&mut deck.slides[index])
                .animations
                .retain(|animation| animation.id != id);
            Ok(None)
        }
        Op::AddComment {
            slide,
            text,
            id,
            x,
            y,
            author,
        } => {
            let index = slide_index(deck, &slide)?;
            let (cx, cy) = match &id {
                Some(id) => {
                    let element = deck.slides[index]
                        .elements
                        .iter()
                        .find(|element| &element.id == id)
                        .with_context(|| missing(id))?;
                    (element.frame.x + element.frame.w, element.frame.y)
                }
                // A number too large to hold reads as infinite, and would
                // be saved as `null`.
                None => (
                    x.filter(|x| x.is_finite()).unwrap_or(24.),
                    y.filter(|y| y.is_finite()).unwrap_or(24.),
                ),
            };
            let comment = Comment {
                id: deck.new_id("c"),
                author: author.unwrap_or_else(user_name),
                text,
                x: cx,
                y: cy,
                element: id,
                resolved: false,
                at: now(),
                replies: Vec::new(),
            };
            let made = comment.id.clone();
            Arc::make_mut(&mut deck.slides[index])
                .comments
                .push(comment);
            Ok(Some(made))
        }
        Op::ReplyComment {
            comment,
            text,
            author,
        } => {
            let found = comment_mut(deck, &comment)?;
            found.replies.push(Reply {
                author: author.unwrap_or_else(user_name),
                text,
                at: now(),
            });
            Ok(None)
        }
        Op::ResolveComment { comment, resolved } => {
            comment_mut(deck, &comment)?.resolved = resolved;
            Ok(None)
        }
        Op::RemoveComment { comment } => {
            for slide in &mut deck.slides {
                if slide.comments.iter().any(|known| known.id == comment) {
                    Arc::make_mut(slide)
                        .comments
                        .retain(|known| known.id != comment);
                    return Ok(None);
                }
            }
            bail!("no comment `{comment}`")
        }
        Op::AddElement {
            slide,
            element,
            behind,
        } => {
            let index = slide_index(deck, &slide)?;
            let element = element_from(deck, element)?;
            let id = element.id.clone();
            let elements = &mut Arc::make_mut(&mut deck.slides[index]).elements;
            if behind {
                elements.insert(0, element);
            } else {
                elements.push(element);
            }
            Ok(Some(id))
        }
        Op::UpdateElement { id, set } => {
            // Anything but an object would take the whole element's place
            // (a string or a number, which cannot even hold its id).
            if !set.is_object() {
                bail!("{id}: `set` is an object of the fields to change");
            }
            if set.get("kind").is_some_and(|kind| !kind.is_object()) {
                bail!("{id}: `kind` is an object with a `type`");
            }
            in_range(&set).with_context(|| id.clone())?;
            let (_, element) = deck.element(&id).with_context(|| missing(&id))?;
            let mut value = serde_json::to_value(element)?;
            let set = flatten_kind(set, value.get("kind").and_then(|kind| kind.get("type")));
            if let Some(kind) = set.get("kind")
                && let (Some(new), Some(old)) = (
                    kind.get("type").and_then(Value::as_str),
                    value["kind"].get("type").and_then(Value::as_str),
                )
                && new != old
            {
                // Another kind: its fields start fresh.
                value["kind"] = json!({ "type": new });
            }
            crate::edit::merge(&mut value, &set);
            value["id"] = Value::String(id.clone());
            let mut updated: Element =
                serde_json::from_value(value).map_err(|err| anyhow!("{id}: {err}"))?;
            // What the change left as it was has been checked before (an
            // SVG is not parsed again to move it).
            if updated.kind != element.kind {
                check(&updated)?;
                clamp_sizes(&mut updated);
            }
            // A group given another frame takes its children along, unless
            // the change placed them itself.
            if let (Kind::Group(before), Kind::Group(after)) = (&element.kind, &updated.kind)
                && before.children == after.children
                && updated.frame != element.frame
            {
                let to = std::mem::replace(&mut updated.frame, element.frame);
                set_frame(&mut updated, to);
            }
            *find_mut(deck, &id).with_context(|| missing(&id))? = updated;
            Ok(None)
        }
        Op::RemoveElement { id } => {
            remove(deck, &id).with_context(|| missing(&id))?;
            Ok(None)
        }
        Op::Order { id, to } => {
            let (slide, index) = locate(deck, &id).with_context(|| missing(&id))?;
            let elements = &mut Arc::make_mut(&mut deck.slides[slide]).elements;
            let element = elements.remove(index);
            let at = match to.as_str() {
                "front" => elements.len(),
                "back" => 0,
                "forward" => (index + 1).min(elements.len()),
                "backward" => index.saturating_sub(1),
                other => bail!("order to `{other}`: front, back, forward or backward"),
            };
            elements.insert(at, element);
            Ok(None)
        }
        Op::SetText { id, text } => {
            let element = find_mut(deck, &id).with_context(|| missing(&id))?;
            let body = element
                .kind
                .text_mut()
                .with_context(|| format!("{id} holds no text"))?;
            let fresh = super::markdown::to_text(&text);
            body.paragraphs = keep_styles(&body.paragraphs, fresh.paragraphs);
            Ok(None)
        }
        Op::StyleText {
            id,
            paragraphs,
            find,
            run,
            paragraph,
        } => {
            in_range(&run).context("run")?;
            in_range(&paragraph).context("paragraph")?;
            let element = find_mut(deck, &id).with_context(|| missing(&id))?;
            let body = element
                .kind
                .text_mut()
                .with_context(|| format!("{id} holds no text"))?;
            let count = body.paragraphs.len();
            let chosen: Vec<usize> = paragraphs.unwrap_or_else(|| (0..count).collect());
            for ix in chosen {
                let Some(target) = body.paragraphs.get_mut(ix) else {
                    bail!("{id} has {count} paragraphs");
                };
                if !paragraph.is_null() {
                    let mut value = serde_json::to_value(&*target)?;
                    crate::edit::merge(&mut value, &paragraph);
                    *target = serde_json::from_value(value).context("paragraph")?;
                }
                if run.is_null() {
                    continue;
                }
                if let Some(find) = &find {
                    split_runs_at(target, find);
                }
                for each in &mut target.runs {
                    if find.as_ref().is_some_and(|find| &each.text != find) {
                        continue;
                    }
                    let mut value = serde_json::to_value(&*each)?;
                    crate::edit::merge(&mut value, &run);
                    *each = serde_json::from_value(value).context("run")?;
                }
            }
            Ok(None)
        }
        Op::Replace { find, replace, id } => {
            if find.is_empty() {
                bail!("nothing to find");
            }
            let mut count = 0;
            for slide in &mut deck.slides {
                // Worked on a copy, kept only where something was found,
                // so the other slides stay shared.
                let before = count;
                let mut copy = (**slide).clone();
                for element in &mut copy.elements {
                    each_element(element, &mut |element| {
                        if id.as_ref().is_some_and(|id| &element.id != id) {
                            return;
                        }
                        for body in bodies(&mut element.kind) {
                            for paragraph in &mut body.paragraphs {
                                for run in &mut paragraph.runs {
                                    count += run.text.matches(&find).count();
                                    run.text = run.text.replace(&find, &replace);
                                }
                            }
                        }
                    });
                }
                if count > before {
                    *slide = Arc::new(copy);
                }
            }
            Ok(Some(format!("{count} replaced")))
        }
        Op::Group { ids, name } => {
            if ids.len() < 2 {
                bail!("group two or more elements");
            }
            let mut slide = None;
            let mut members = Vec::new();
            for id in &ids {
                let (at, _) = locate(deck, id).with_context(|| missing(id))?;
                if slide.is_some_and(|slide| slide != at) {
                    bail!("grouped elements are on one slide");
                }
                slide = Some(at);
            }
            let slide = slide.expect("two or more ids");
            let elements = &mut Arc::make_mut(&mut deck.slides[slide]).elements;
            let first = elements
                .iter()
                .position(|element| ids.contains(&element.id))
                .unwrap_or(0);
            elements.retain(|element| {
                if ids.contains(&element.id) {
                    members.push(element.clone());
                    false
                } else {
                    true
                }
            });
            let bounds = bounds(members.iter().map(|element| element.frame));
            let id = deck.new_id("e");
            let group = Element {
                id: id.clone(),
                name: name.unwrap_or_else(|| "Group".into()),
                frame: bounds,
                kind: Kind::Group(Group { children: members }),
                ..Element::default()
            };
            let elements = &mut Arc::make_mut(&mut deck.slides[slide]).elements;
            elements.insert(first.min(elements.len()), group);
            Ok(Some(id))
        }
        Op::Ungroup { id } => {
            let (slide, index) = locate(deck, &id).with_context(|| missing(&id))?;
            let elements = &mut Arc::make_mut(&mut deck.slides[slide]).elements;
            let Kind::Group(group) = &elements[index].kind else {
                bail!("{id} is not a group");
            };
            let children = group.children.clone();
            elements.splice(index..=index, children);
            Ok(None)
        }
        Op::Align { ids, to, slide } => {
            let frames: Vec<Frame> = ids
                .iter()
                .map(|id| {
                    find_mut(deck, id)
                        .map(|element| element.frame)
                        .with_context(|| missing(id))
                })
                .collect::<Result<_>>()?;
            let against = if slide || ids.len() == 1 {
                Frame::new(0., 0., deck.size.width, deck.size.height)
            } else {
                bounds(frames.iter().copied())
            };
            for id in &ids {
                let element = find_mut(deck, id).expect("found above");
                let mut frame = element.frame;
                match to.as_str() {
                    "left" => frame.x = against.x,
                    "center" => frame.x = against.x + (against.w - frame.w) / 2.,
                    "right" => frame.x = against.x + against.w - frame.w,
                    "top" => frame.y = against.y,
                    "middle" => frame.y = against.y + (against.h - frame.h) / 2.,
                    "bottom" => frame.y = against.y + against.h - frame.h,
                    other => bail!("align to `{other}`"),
                }
                set_frame(element, frame);
            }
            Ok(None)
        }
        Op::Distribute { ids, axis } => {
            if ids.len() < 3 {
                bail!("distribute three or more elements");
            }
            let horizontal = match axis.as_str() {
                "horizontal" => true,
                "vertical" => false,
                other => bail!("distribute `{other}`: horizontal or vertical"),
            };
            let mut items: Vec<(String, Frame)> = ids
                .iter()
                .map(|id| {
                    find_mut(deck, id)
                        .map(|element| (id.clone(), element.frame))
                        .with_context(|| missing(id))
                })
                .collect::<Result<_>>()?;
            let start = |frame: &Frame| if horizontal { frame.x } else { frame.y };
            let length = |frame: &Frame| if horizontal { frame.w } else { frame.h };
            items.sort_by(|a, b| start(&a.1).total_cmp(&start(&b.1)));
            let first = start(&items[0].1);
            let last = items.last().expect("three or more");
            let end = start(&last.1) + length(&last.1);
            let total: f32 = items.iter().map(|(_, frame)| length(frame)).sum();
            let gap = (end - first - total) / (items.len() - 1) as f32;
            let mut at = first;
            for (id, frame) in items {
                let element = find_mut(deck, &id).expect("found above");
                let mut placed = element.frame;
                if horizontal {
                    placed.x = at;
                } else {
                    placed.y = at;
                }
                set_frame(element, placed);
                at += length(&frame) + gap;
            }
            Ok(None)
        }
        Op::SetTheme {
            theme,
            custom,
            saved,
            colors,
            fonts,
            background,
        } => {
            if let Some(id) = theme {
                let fresh = super::theme::builtin_for(&id, &deck.size).with_context(|| {
                    format!(
                        "no theme `{id}`; built in: {}",
                        super::theme::builtins()
                            .map(|(id, _)| id)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
                set_theme(deck, fresh);
            }
            if let Some(mut custom) = custom {
                check_theme(&mut custom)?;
                set_theme(deck, *custom);
            }
            if let Some(name) = saved {
                let theme =
                    super::space::load_theme(&super::space::themes_dir(), &name, &deck.size)?;
                set_theme(deck, theme);
            }
            let colors: Vec<(String, String)> = match colors {
                Value::Null => Vec::new(),
                Value::Object(map) => map
                    .into_iter()
                    .map(|(slot, hex)| (slot, hex.as_str().unwrap_or_default().to_string()))
                    .collect(),
                other => serde_json::from_value(other).context("colors")?,
            };
            for (slot, hex) in colors {
                if !SLOTS.contains(&slot.as_str()) {
                    bail!("no colour slot `{slot}`: {}", SLOTS.join(", "));
                }
                if Rgba::parse_hex(&hex).is_none() {
                    bail!("`{hex}` is not a colour (#RRGGBB)");
                }
                match deck.theme.colors.iter_mut().find(|(name, _)| *name == slot) {
                    Some(entry) => entry.1 = hex,
                    None => deck.theme.colors.push((slot, hex)),
                }
            }
            if let Some(fonts) = fonts {
                let mut value = serde_json::to_value(&deck.theme.fonts)?;
                crate::edit::merge(&mut value, &fonts);
                deck.theme.fonts = serde_json::from_value(value).context("fonts")?;
            }
            if let Some(background) = background {
                in_range(&background).context("background")?;
                deck.theme.background = serde_json::from_value(background).context("background")?;
            }
            Ok(None)
        }
        Op::SetSize {
            preset,
            width,
            height,
            scale,
        } => {
            let size = match (preset, width, height) {
                (Some(preset), _, _) => SlideSize::preset(&preset).with_context(|| {
                    format!(
                        "no size `{preset}`: {}",
                        SIZES
                            .iter()
                            .map(|(id, ..)| *id)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?,
                (None, Some(width), Some(height)) if width >= 72. && height >= 72. => SlideSize {
                    width: width.min(4000.),
                    height: height.min(4000.),
                    preset: None,
                },
                _ => bail!("give a preset, or a width and height of at least 72 points"),
            };
            resize(deck, size, scale);
            Ok(None)
        }
        Op::SetTitle { title } => {
            deck.title = title;
            Ok(None)
        }
        Op::AddSection { name, slide } => {
            let index = slide_index(deck, &slide)?;
            let id = deck.slides[index].id.clone();
            deck.sections
                .retain(|section| section.slide != id && section.name != name);
            deck.sections.push(Section { name, slide: id });
            let order: Vec<String> = deck.slides.iter().map(|slide| slide.id.clone()).collect();
            deck.sections
                .sort_by_key(|section| order.iter().position(|id| *id == section.slide));
            Ok(None)
        }
        Op::RemoveSection { name } => {
            deck.sections.retain(|section| section.name != name);
            Ok(None)
        }
    }
}

/// The comment `id`, anywhere in the deck.
fn comment_mut<'a>(deck: &'a mut Deck, id: &str) -> Result<&'a mut Comment> {
    let index = deck
        .slides
        .iter()
        .position(|slide| slide.comments.iter().any(|comment| comment.id == id))
        .with_context(|| format!("no comment `{id}`"))?;
    Arc::make_mut(&mut deck.slides[index])
        .comments
        .iter_mut()
        .find(|comment| comment.id == id)
        .with_context(|| format!("no comment `{id}`"))
}

/// Who is writing: the account's name, as the system knows it.
pub fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .map(|name| {
            let mut chars = name.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .unwrap_or_else(|| "You".into())
}

/// Seconds since 1970.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn missing(id: &str) -> String {
    format!("no element `{id}`")
}

fn layout_ids(theme: &Theme) -> String {
    theme
        .layouts
        .iter()
        .map(|layout| layout.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl Deck {
    /// A fresh id: `s7`, `e12`.
    pub fn new_id(&mut self, prefix: &str) -> String {
        // Never one already used (an agent may have written ids itself).
        loop {
            let id = format!("{prefix}{}", self.next_id);
            self.next_id += 1;
            let taken = self.slides.iter().any(|slide| {
                slide.id == id
                    || slide.comments.iter().any(|comment| comment.id == id)
                    || slide.elements.iter().any(|element| {
                        let mut found = false;
                        visit(element, &mut |element| found |= element.id == id);
                        found
                    })
            });
            if !taken {
                return id;
            }
        }
    }

    pub fn slide(&self, id: &str) -> Option<&Slide> {
        self.slides
            .iter()
            .find(|slide| slide.id == id)
            .map(|slide| &**slide)
    }

    /// The element with `id`, anywhere (in groups too), and its slide's
    /// index.
    pub fn element(&self, id: &str) -> Option<(usize, &Element)> {
        for (index, slide) in self.slides.iter().enumerate() {
            for element in &slide.elements {
                if let Some(found) = find_in(element, id) {
                    return Some((index, found));
                }
            }
        }
        None
    }
}

fn find_in<'a>(element: &'a Element, id: &str) -> Option<&'a Element> {
    if element.id == id {
        return Some(element);
    }
    if let Kind::Group(group) = &element.kind {
        return group.children.iter().find_map(|child| find_in(child, id));
    }
    None
}

fn visit(element: &Element, each: &mut impl FnMut(&Element)) {
    each(element);
    if let Kind::Group(group) = &element.kind {
        for child in &group.children {
            visit(child, each);
        }
    }
}

fn each_element(element: &mut Element, each: &mut impl FnMut(&mut Element)) {
    each(element);
    if let Kind::Group(group) = &mut element.kind {
        for child in &mut group.children {
            each_element(child, each);
        }
    }
}

/// The text bodies an element's kind holds (a table's cells too).
fn bodies(kind: &mut Kind) -> Vec<&mut TextBody> {
    match kind {
        Kind::Shape(shape) => shape.text.as_mut().into_iter().collect(),
        Kind::Text(text) => vec![&mut text.text],
        Kind::Table(table) => table
            .rows
            .iter_mut()
            .flatten()
            .map(|cell| &mut cell.text)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn find_mut<'a>(deck: &'a mut Deck, id: &str) -> Option<&'a mut Element> {
    fn find<'a>(element: &'a mut Element, id: &str) -> Option<&'a mut Element> {
        if element.id == id {
            return Some(element);
        }
        if let Kind::Group(group) = &mut element.kind {
            return group.children.iter_mut().find_map(|child| find(child, id));
        }
        None
    }
    // Only the slide holding it is made its own (see [`Deck::slides`]).
    let index = slide_of(deck, id)?;
    Arc::make_mut(&mut deck.slides[index])
        .elements
        .iter_mut()
        .find_map(|element| find(element, id))
}

/// The index of the slide holding element `id` (in a group or not).
pub fn slide_of(deck: &Deck, id: &str) -> Option<usize> {
    fn holds(element: &Element, id: &str) -> bool {
        element.id == id
            || matches!(&element.kind, Kind::Group(group) if group.children.iter().any(|child| holds(child, id)))
    }
    deck.slides
        .iter()
        .position(|slide| slide.elements.iter().any(|element| holds(element, id)))
}

/// A top-level element's slide and place.
fn locate(deck: &Deck, id: &str) -> Option<(usize, usize)> {
    deck.slides.iter().enumerate().find_map(|(slide, each)| {
        each.elements
            .iter()
            .position(|element| element.id == id)
            .map(|index| (slide, index))
    })
}

fn remove(deck: &mut Deck, id: &str) -> Option<Element> {
    fn take(elements: &mut Vec<Element>, id: &str) -> Option<Element> {
        if let Some(index) = elements.iter().position(|element| element.id == id) {
            return Some(elements.remove(index));
        }
        elements
            .iter_mut()
            .find_map(|element| match &mut element.kind {
                Kind::Group(group) => take(&mut group.children, id),
                _ => None,
            })
    }
    let index = slide_of(deck, id)?;
    take(&mut Arc::make_mut(&mut deck.slides[index]).elements, id)
}

/// The index of the slide `reference` names: its id, or its number.
pub fn slide_index(deck: &Deck, reference: &Value) -> Result<usize> {
    let number = match reference {
        Value::Number(number) => number.as_u64().map(|n| n as usize),
        Value::String(text) => text.parse::<usize>().ok(),
        _ => None,
    };
    if let Some(number) = number {
        return number
            .checked_sub(1)
            .filter(|index| *index < deck.slides.len())
            .with_context(|| format!("no slide {number}: there are {}", deck.slides.len()));
    }
    let id = reference.as_str().unwrap_or_default();
    deck.slides
        .iter()
        .position(|slide| slide.id == id)
        .with_context(|| format!("no slide `{id}`"))
}

/// A new slide in `layout`, its placeholders empty.
pub fn new_slide(deck: &mut Deck, layout: &str) -> Slide {
    let id = deck.new_id("s");
    let places = deck
        .theme
        .layout(layout)
        .map(|layout| layout.placeholders.clone())
        .unwrap_or_default();
    // Footers and slide numbers are the layout's to show, as in
    // PowerPoint, not boxes on each new slide.
    let elements = places
        .into_iter()
        .filter(|place| {
            !matches!(
                place.kind,
                PlaceholderKind::Footer | PlaceholderKind::SlideNumber
            )
        })
        .map(|place| placeholder_element(deck, &place))
        .collect();
    Slide {
        id,
        layout: layout.into(),
        elements,
        ..Slide::default()
    }
}

fn placeholder_element(deck: &mut Deck, place: &Placeholder) -> Element {
    let kind = if place.kind == PlaceholderKind::Picture {
        Kind::Picture(Picture::default())
    } else {
        Kind::Text(TextBox {
            text: TextBody {
                anchor: place.anchor,
                autofit: place.autofit,
                ..TextBody::default()
            },
            ..TextBox::default()
        })
    };
    Element {
        id: deck.new_id("e"),
        name: place.kind.label().into(),
        frame: place.frame,
        placeholder: Some(place.kind),
        kind,
        ..Element::default()
    }
}

/// Moves slide `index` to `layout`: its placeholders to the new places,
/// empty ones the layout lacks gone, new ones added.
fn change_layout(deck: &mut Deck, index: usize, layout: &str) -> Result<()> {
    let places = deck
        .theme
        .layout(layout)
        .map(|layout| layout.placeholders.clone())
        .with_context(|| {
            format!(
                "no layout `{layout}`; the theme has {}",
                layout_ids(&deck.theme)
            )
        })?;
    let mut elements = std::mem::take(&mut Arc::make_mut(&mut deck.slides[index]).elements);
    elements.retain_mut(|element| {
        let Some(kind) = element.placeholder else {
            return true;
        };
        match places.iter().find(|place| place.kind == kind) {
            Some(place) => {
                element.frame = place.frame;
                true
            }
            None if is_empty(element) => false,
            None => {
                // Kept where it was, no longer a placeholder.
                element.placeholder = None;
                true
            }
        }
    });
    for place in &places {
        if !elements
            .iter()
            .any(|element| element.placeholder == Some(place.kind))
        {
            let element = placeholder_element(deck, place);
            elements.push(element);
        }
    }
    let slide = Arc::make_mut(&mut deck.slides[index]);
    slide.elements = elements;
    slide.layout = layout.into();
    Ok(())
}

/// Whether a placeholder was never filled.
pub fn is_empty(element: &Element) -> bool {
    match &element.kind {
        Kind::Text(text) => text.text.plain().trim().is_empty(),
        Kind::Shape(shape) => shape
            .text
            .as_ref()
            .is_none_or(|text| text.plain().trim().is_empty()),
        Kind::Picture(picture) => picture.media.is_empty(),
        _ => false,
    }
}

/// An element from an agent's JSON, with a new id.
pub fn element_from(deck: &mut Deck, value: Value) -> Result<Element> {
    in_range(&value).context("element")?;
    let value = flatten_kind(value, None);
    let kind = value.get("kind");
    let shape_text = kind
        .and_then(|kind| kind.get("type"))
        .and_then(Value::as_str)
        == Some("shape")
        && kind.and_then(|kind| kind.get("text")).is_some();
    let anchored = kind
        .and_then(|kind| kind.get("text"))
        .and_then(|text| text.get("anchor"))
        .is_some();
    let mut element: Element =
        serde_json::from_value(value).map_err(|err| anyhow!("element: {err}"))?;
    // Text in a shape sits in its middle, as PowerPoint puts it.
    if shape_text
        && let Kind::Shape(Shape {
            text: Some(body), ..
        }) = &mut element.kind
    {
        if !anchored {
            body.anchor = Anchor::Middle;
        }
        for paragraph in &mut body.paragraphs {
            paragraph.align.get_or_insert(Align::Center);
        }
    }
    check(&element)?;
    clamp_sizes(&mut element);
    renumber(deck, &mut element);
    if element.name.is_empty() {
        element.name = element.kind.label().into();
    }
    Ok(element)
}

/// Refuses JSON holding a number an `f32` cannot hold: read into one it
/// becomes infinite, which is saved as `null`, and the deck would not open
/// again.
fn in_range(value: &Value) -> Result<()> {
    let fits = match value {
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| number.abs() <= f32::MAX as f64),
        Value::Array(items) => return items.iter().try_for_each(in_range),
        Value::Object(fields) => return fields.values().try_for_each(in_range),
        _ => true,
    };
    if !fits {
        bail!("the number {value} is out of range");
    }
    Ok(())
}

/// The largest SVG graphic taken.
const SVG_LARGEST: usize = 2 * 1024 * 1024;

/// Refuses what would not draw: SVG that does not parse, unknown icons.
fn check(element: &Element) -> Result<()> {
    match &element.kind {
        Kind::Svg(art) => {
            if art.source.len() > SVG_LARGEST {
                bail!("the SVG is over 2 MB; simplify it");
            }
            // Before it is read: reading one that nests very deeply uses
            // up the stack, which ends the app.
            if super::raster::too_deep(art.source.as_bytes()) {
                bail!("the SVG nests too deeply to draw; flatten its groups");
            }
            resvg::usvg::Tree::from_str(&art.source, &super::raster::inert_options())
                .map_err(|err| anyhow!("the SVG does not draw: {err}"))?;
        }
        Kind::Icon(icon) if super::icons::svg(&icon.icon).is_none() => {
            bail!(
                "no icon `{}`; slides_themes with icons lists them",
                icon.icon
            );
        }
        Kind::Group(group) => {
            for child in &group.children {
                check(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Text sizes an element's kind sets itself, kept to what the layout
/// takes (as runs' sizes are when drawn): a size of 0 has no line height,
/// and the layout ends the app on one.
fn clamp_sizes(element: &mut Element) {
    each_element(element, &mut |element| match &mut element.kind {
        Kind::Table(table) => table.font_size = table.font_size.clamp(1., 400.),
        Kind::Code(code) => code.font_size = code.font_size.clamp(1., 400.),
        _ => {}
    });
}

/// A whole theme from an agent: its text sizes kept to what the layout
/// takes, and refused when a number in it would not read back once saved
/// (one too large for an `f32` is infinite, and saved as `null`).
fn check_theme(theme: &mut Theme) -> Result<()> {
    let reads_back = serde_json::to_value(&*theme)
        .ok()
        .and_then(|json| serde_json::from_value::<Theme>(json).ok())
        .is_some();
    if !reads_back {
        bail!("custom: a number is out of range");
    }
    let clamp = |size: &mut f32| *size = size.clamp(1., 400.);
    clamp(&mut theme.title.size);
    clamp(&mut theme.body.size);
    theme.decorations.iter_mut().for_each(clamp_sizes);
    for layout in &mut theme.layouts {
        layout.decorations.iter_mut().for_each(clamp_sizes);
        for place in &mut layout.placeholders {
            if let Some(size) = &mut place.size {
                clamp(size);
            }
        }
    }
    Ok(())
}

/// Gives `element` the frame `to`. A group's children are placed on the
/// slide, not in the group, so they go with its box: moved as far, and
/// scaled with it when its size changed (turning it moves none of them:
/// the group is drawn turned about its centre).
pub fn set_frame(element: &mut Element, to: Frame) {
    let from = std::mem::replace(&mut element.frame, to);
    let Kind::Group(group) = &mut element.kind else {
        return;
    };
    // A box that is not a number (a division by a zero size somewhere)
    // would take every child with it.
    if ![from.x, from.y, from.w, from.h, to.x, to.y, to.w, to.h]
        .iter()
        .all(|value| value.is_finite())
    {
        return;
    }
    // A box without a size keeps its children's sizes: scaled to nothing,
    // they would never come back.
    let scale = |from: f32, to: f32| {
        if from > 0. && to > 0. { to / from } else { 1. }
    };
    let (sx, sy) = (scale(from.w, to.w), scale(from.h, to.h));
    if sx == 1. && sy == 1. && from.x == to.x && from.y == to.y {
        return;
    }
    for child in &mut group.children {
        each_element(child, &mut |child| {
            let frame = &mut child.frame;
            frame.x = to.x + (frame.x - from.x) * sx;
            frame.y = to.y + (frame.y - from.y) * sy;
            frame.w *= sx;
            frame.h *= sy;
        });
    }
}

/// Moves `element` by (`dx`, `dy`), a group's children with it.
pub fn move_by(element: &mut Element, dx: f32, dy: f32) {
    let frame = element.frame;
    set_frame(
        element,
        Frame {
            x: frame.x + dx,
            y: frame.y + dy,
            ..frame
        },
    );
}

/// New ids for an element and all inside it.
pub fn renumber(deck: &mut Deck, element: &mut Element) {
    element.id = deck.new_id("e");
    if let Kind::Group(group) = &mut element.kind {
        for child in &mut group.children {
            renumber(deck, child);
        }
    }
}

/// The element fields that are not its kind's.
const ELEMENT_FIELDS: [&str; 6] = ["id", "name", "frame", "placeholder", "opacity", "kind"];

/// Moves a kind's fields given beside the element's (with a `type`, or
/// for `known_type`, the kind it is) into `kind`.
fn flatten_kind(value: Value, known_type: Option<&Value>) -> Value {
    let Value::Object(mut object) = value else {
        return value;
    };
    let loose: Vec<String> = object
        .keys()
        .filter(|key| !ELEMENT_FIELDS.contains(&key.as_str()))
        .cloned()
        .collect();
    if loose.is_empty() {
        return Value::Object(object);
    }
    let mut kind = match object.remove("kind") {
        Some(Value::Object(kind)) => kind,
        _ => serde_json::Map::new(),
    };
    for key in loose {
        if let Some(field) = object.remove(&key) {
            kind.insert(key, field);
        }
    }
    if !kind.contains_key("type")
        && let Some(known) = known_type
    {
        kind.insert("type".into(), known.clone());
    }
    object.insert("kind".into(), Value::Object(kind));
    Value::Object(object)
}

/// New paragraphs keeping the styles of the old ones whose text matches
/// (the whole line), and the first old run's style for changed ones.
fn keep_styles(old: &[Paragraph], fresh: Vec<Paragraph>) -> Vec<Paragraph> {
    fresh
        .into_iter()
        .enumerate()
        .map(|(ix, mut paragraph)| {
            let Some(before) = old.get(ix) else {
                return paragraph;
            };
            if before.plain() == paragraph.plain() && paragraph.runs.iter().all(plain_run) {
                return before.clone();
            }
            if let Some(style) = before.runs.first() {
                for run in &mut paragraph.runs {
                    if plain_run(run) {
                        let text = std::mem::take(&mut run.text);
                        *run = Run {
                            text,
                            ..style.clone()
                        };
                    }
                }
            }
            paragraph.align = paragraph.align.or(before.align);
            paragraph.space_before = paragraph.space_before.or(before.space_before);
            paragraph.space_after = paragraph.space_after.or(before.space_after);
            paragraph.line_spacing = paragraph.line_spacing.or(before.line_spacing);
            paragraph
        })
        .collect()
}

fn plain_run(run: &Run) -> bool {
    run.same_style(&Run::default())
}

/// Splits a paragraph's runs so `find` is runs of its own.
fn split_runs_at(paragraph: &mut Paragraph, find: &str) {
    if find.is_empty() {
        return;
    }
    let mut out = Vec::new();
    for run in paragraph.runs.drain(..) {
        let mut rest = run.text.as_str();
        while let Some(at) = rest.find(find) {
            if at > 0 {
                out.push(Run {
                    text: rest[..at].to_string(),
                    ..run.clone()
                });
            }
            out.push(Run {
                text: find.to_string(),
                ..run.clone()
            });
            rest = &rest[at + find.len()..];
        }
        if !rest.is_empty() || run.text.is_empty() {
            out.push(Run {
                text: rest.to_string(),
                ..run.clone()
            });
        }
    }
    paragraph.runs = out;
}

/// The box around `frames`.
pub fn bounds(frames: impl Iterator<Item = Frame>) -> Frame {
    let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for frame in frames {
        left = left.min(frame.x);
        top = top.min(frame.y);
        right = right.max(frame.x + frame.w);
        bottom = bottom.max(frame.y + frame.h);
    }
    if left > right {
        return Frame::default();
    }
    Frame::new(left, top, right - left, bottom - top)
}

/// A new theme for the deck: its layouts made for the deck's size, and
/// slides' placeholders moved to the new places.
fn set_theme(deck: &mut Deck, theme: Theme) {
    deck.theme = theme;
    for index in 0..deck.slides.len() {
        let layout = deck.slides[index].layout.clone();
        let layout = if deck.theme.layout(&layout).is_some() {
            layout
        } else {
            "blank".into()
        };
        let _ = change_layout(deck, index, &layout);
    }
}

/// The deck at `size`: layouts remade, and elements scaled to it when
/// `scale` (keeping their proportions, centred), else left where they are.
pub fn resize(deck: &mut Deck, size: SlideSize, scale: bool) {
    let from = deck.size.clone();
    deck.theme = super::theme::resized(&deck.theme, &from, &size);
    deck.size = size.clone();
    if scale {
        let factor = (size.width / from.width).min(size.height / from.height);
        let (dx, dy) = (
            (size.width - from.width * factor) / 2.,
            (size.height - from.height * factor) / 2.,
        );
        for slide in &mut deck.slides {
            for element in &mut Arc::make_mut(slide).elements {
                if element.placeholder.is_some() {
                    continue;
                }
                // A group's children too: they are placed on the slide.
                each_element(element, &mut |element| {
                    let frame = &mut element.frame;
                    frame.x = frame.x * factor + dx;
                    frame.y = frame.y * factor + dy;
                    frame.w *= factor;
                    frame.h *= factor;
                });
            }
        }
    }
    for index in 0..deck.slides.len() {
        let layout = deck.slides[index].layout.clone();
        let _ = change_layout(deck, index, &layout);
    }
}

/// A deck with one title slide in `theme`, at `size`.
pub fn new_deck(title: &str, theme: &str, size: SlideSize) -> Deck {
    let mut deck = Deck {
        title: title.into(),
        theme: super::theme::builtin_for(theme, &size)
            .unwrap_or_else(|| super::theme::builtin_for("clean", &size).expect("clean")),
        size,
        ..Deck::default()
    };
    let _ = apply(
        &mut deck,
        Op::AddSlide {
            layout: Some("title".into()),
            after: None,
            at: None,
            title: (!title.is_empty()).then(|| title.to_string()),
            subtitle: None,
            body: None,
            body2: None,
            notes: None,
            elements: Vec::new(),
        },
    );
    deck
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(value: Value) -> Op {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn slides_and_elements() {
        let mut deck = new_deck("Plan", "clean", SlideSize::default());
        assert_eq!(deck.slides.len(), 1);
        let title = deck.slides[0]
            .elements
            .iter()
            .find(|element| element.placeholder == Some(PlaceholderKind::Title))
            .unwrap();
        assert_eq!(title.kind.text().unwrap().plain(), "Plan");

        let id = apply(
            &mut deck,
            op(json!({"op": "add_slide", "title": "Why", "body": "- fast\n- small"})),
        )
        .unwrap()
        .unwrap();
        assert_eq!(deck.slides[1].id, id);
        assert_eq!(deck.slides[1].layout, "title_content");

        // An element with its kind's fields beside it.
        let shape = apply(
            &mut deck,
            op(json!({"op": "add_element", "slide": 2, "element": {
                "type": "shape", "geometry": "ellipse", "fill": "accent2",
                "frame": {"x": 10, "y": 10, "w": 50, "h": 50}, "text": "**Hi**"}})),
        )
        .unwrap()
        .unwrap();
        let (_, element) = deck.element(&shape).unwrap();
        let Kind::Shape(shape_kind) = &element.kind else {
            panic!("a shape")
        };
        assert_eq!(shape_kind.geometry, Geometry::Ellipse);
        assert_eq!(shape_kind.fill, Fill::solid("accent2"));
        assert_eq!(
            shape_kind.text.as_ref().unwrap().paragraphs[0].runs[0].bold,
            Some(true)
        );

        // Updated by a patch with the kind's fields beside.
        apply(
            &mut deck,
            op(json!({"op": "update_element", "id": shape, "set": {"geometry": "star5", "frame": {"x": 20}}})),
        )
        .unwrap();
        let (_, element) = deck.element(&shape).unwrap();
        assert_eq!(element.frame.x, 20.);
        assert_eq!(element.frame.w, 50., "the rest of the frame kept");
        let Kind::Shape(shape_kind) = &element.kind else {
            panic!("a shape")
        };
        assert_eq!(shape_kind.geometry, Geometry::Star5);

        apply(&mut deck, op(json!({"op": "duplicate_slide", "slide": 2}))).unwrap();
        assert_eq!(deck.slides.len(), 3);
        assert_ne!(deck.slides[2].elements[0].id, deck.slides[1].elements[0].id);
        apply(
            &mut deck,
            op(json!({"op": "move_slide", "slide": 3, "to": 1})),
        )
        .unwrap();
        assert_eq!(deck.slides[0].layout, "title_content");
        apply(&mut deck, op(json!({"op": "remove_slide", "slide": 1}))).unwrap();
        assert_eq!(deck.slides.len(), 2);
    }

    #[test]
    fn text_keeps_its_style() {
        let mut deck = new_deck("", "clean", SlideSize::default());
        let id = deck.slides[0].elements[0].id.clone();
        apply(
            &mut deck,
            op(json!({"op": "set_text", "id": id, "text": "One\nTwo"})),
        )
        .unwrap();
        apply(
            &mut deck,
            op(json!({"op": "style_text", "id": id, "paragraphs": [0], "run": {"color": "accent3", "size": 30}})),
        )
        .unwrap();
        apply(
            &mut deck,
            op(json!({"op": "set_text", "id": id, "text": "One\nTwo!"})),
        )
        .unwrap();
        let (_, element) = deck.element(&id).unwrap();
        let body = element.kind.text().unwrap();
        assert_eq!(body.paragraphs[0].runs[0].size, Some(30.));
        assert_eq!(body.paragraphs[1].plain(), "Two!");
        apply(
            &mut deck,
            op(json!({"op": "style_text", "id": id, "paragraphs": [1], "find": "wo", "run": {"bold": true}})),
        )
        .unwrap();
        let (_, element) = deck.element(&id).unwrap();
        let runs = &element.kind.text().unwrap().paragraphs[1].runs;
        assert_eq!(
            runs.iter().map(|run| run.text.as_str()).collect::<Vec<_>>(),
            ["T", "wo", "!"]
        );
        assert_eq!(runs[1].bold, Some(true));
        assert_eq!(runs[0].bold, None);
    }

    #[test]
    fn layouts_themes_sizes_groups() {
        let mut deck = new_deck("T", "clean", SlideSize::default());
        apply(
            &mut deck,
            op(json!({"op": "add_slide", "title": "A", "body": "x"})),
        )
        .unwrap();
        apply(
            &mut deck,
            op(json!({"op": "update_slide", "slide": 2, "layout": "two_content"})),
        )
        .unwrap();
        let places: Vec<_> = deck.slides[1]
            .elements
            .iter()
            .filter_map(|element| element.placeholder)
            .collect();
        assert!(places.contains(&PlaceholderKind::Body2));
        apply(
            &mut deck,
            op(json!({"op": "set_theme", "theme": "midnight"})),
        )
        .unwrap();
        assert_eq!(deck.theme.name, "Midnight");
        apply(&mut deck, op(json!({"op": "set_size", "preset": "4:3"}))).unwrap();
        assert_eq!(deck.size.width, 720.);
        let title = deck.slides[1]
            .elements
            .iter()
            .find(|e| e.placeholder == Some(PlaceholderKind::Title))
            .unwrap();
        assert!(title.frame.x + title.frame.w <= 720.5);

        let a = apply(&mut deck, op(json!({"op": "add_element", "slide": 2, "element": {"type": "shape", "frame": {"x": 0, "y": 0, "w": 10, "h": 10}}}))).unwrap().unwrap();
        let b = apply(&mut deck, op(json!({"op": "add_element", "slide": 2, "element": {"type": "shape", "frame": {"x": 100, "y": 50, "w": 10, "h": 10}}}))).unwrap().unwrap();
        let group = apply(&mut deck, op(json!({"op": "group", "ids": [a, b]})))
            .unwrap()
            .unwrap();
        let (_, element) = deck.element(&group).unwrap();
        assert_eq!(element.frame, Frame::new(0., 0., 110., 60.));
        assert!(deck.element(&a).is_some(), "found inside the group");
        apply(&mut deck, op(json!({"op": "ungroup", "id": group}))).unwrap();
        assert!(deck.element(&group).is_none());
        let result = apply(
            &mut deck,
            op(json!({"op": "replace", "find": "A", "replace": "B"})),
        )
        .unwrap();
        assert_eq!(result.as_deref(), Some("1 replaced"));
    }

    fn shape(deck: &mut Deck, slide: usize, frame: [f32; 4]) -> String {
        let [x, y, w, h] = frame;
        apply(
            deck,
            op(json!({"op": "add_element", "slide": slide, "element": {
                "type": "shape", "frame": {"x": x, "y": y, "w": w, "h": h}}})),
        )
        .unwrap()
        .unwrap()
    }

    fn frame_of(deck: &Deck, id: &str) -> [f32; 4] {
        let frame = deck.element(id).unwrap().1.frame;
        [frame.x, frame.y, frame.w, frame.h]
    }

    #[test]
    fn update_element_takes_only_an_object() {
        let mut deck = new_deck("T", "clean", SlideSize::default());
        let id = shape(&mut deck, 1, [10., 10., 50., 50.]);
        let before = deck.clone();
        // A patch sent as a string, a number or nothing: refused, where it
        // once took the element's place (or ended the app).
        for set in [
            json!("{\"fill\":\"accent2\"}"),
            json!(3),
            json!([1]),
            json!(null),
        ] {
            let result = apply(
                &mut deck,
                op(json!({"op": "update_element", "id": id, "set": set})),
            );
            assert!(result.is_err(), "{set}");
        }
        assert!(
            apply(
                &mut deck,
                op(json!({"op": "update_element", "id": id, "set": {"kind": null}})),
            )
            .is_err()
        );
        assert_eq!(deck, before);
    }

    #[test]
    fn a_copied_slide_names_its_own_elements() {
        let mut deck = new_deck("T", "clean", SlideSize::default());
        let a = shape(&mut deck, 1, [0., 0., 10., 10.]);
        let b = shape(&mut deck, 1, [100., 0., 10., 10.]);
        let line = apply(
            &mut deck,
            op(json!({"op": "add_element", "slide": 1, "element": {
                "type": "shape", "geometry": "line", "frame": {"x": 10, "y": 5, "w": 90, "h": 0},
                "connect": {"start": [a, 1], "end": [b, 3]}}})),
        )
        .unwrap()
        .unwrap();
        apply(&mut deck, op(json!({"op": "animate", "id": a}))).unwrap();
        let comment = apply(
            &mut deck,
            op(json!({"op": "add_comment", "slide": 1, "id": b, "text": "Bigger?"})),
        )
        .unwrap()
        .unwrap();
        apply(&mut deck, op(json!({"op": "duplicate_slide", "slide": 1}))).unwrap();

        let (original, copy) = (&deck.slides[0], &deck.slides[1]);
        let twin = |id: &str| {
            let at = original.elements.iter().position(|e| e.id == id).unwrap();
            copy.elements[at].id.clone()
        };
        assert_ne!(twin(&a), a);
        assert_eq!(copy.animations.len(), 1, "the copy keeps its animations");
        assert_eq!(copy.animations[0].id, twin(&a));
        assert_eq!(copy.comments[0].element, Some(twin(&b)));
        assert_ne!(copy.comments[0].id, comment);
        let Kind::Shape(Shape {
            connect: Some(connect),
            ..
        }) = &deck.element(&twin(&line)).unwrap().1.kind
        else {
            panic!("a line fixed to shapes")
        };
        assert_eq!(connect.start, Some((twin(&a), 1)));
        assert_eq!(connect.end, Some((twin(&b), 3)));

        // Resolving the copy's comment leaves the original's open.
        let copied = copy.comments[0].id.clone();
        apply(
            &mut deck,
            op(json!({"op": "resolve_comment", "comment": copied})),
        )
        .unwrap();
        assert!(!deck.slides[0].comments[0].resolved);
        assert!(deck.slides[1].comments[0].resolved);
    }

    #[test]
    fn a_group_takes_its_children_along() {
        let mut deck = new_deck("T", "clean", SlideSize::default());
        apply(&mut deck, op(json!({"op": "add_slide", "layout": "blank"}))).unwrap();
        let a = shape(&mut deck, 2, [100., 100., 20., 20.]);
        let b = shape(&mut deck, 2, [180., 140., 20., 20.]);
        let group = apply(&mut deck, op(json!({"op": "group", "ids": [a, b]})))
            .unwrap()
            .unwrap();
        assert_eq!(frame_of(&deck, &group), [100., 100., 100., 60.]);

        // Aligned to the slide's left: moved, its children as far.
        apply(
            &mut deck,
            op(json!({"op": "align", "ids": [group], "to": "left"})),
        )
        .unwrap();
        assert_eq!(frame_of(&deck, &group), [0., 100., 100., 60.]);
        assert_eq!(frame_of(&deck, &a), [0., 100., 20., 20.]);
        assert_eq!(frame_of(&deck, &b), [80., 140., 20., 20.]);

        // Given a frame twice the size elsewhere: scaled inside it.
        apply(
            &mut deck,
            op(json!({"op": "update_element", "id": group, "set": {
                "frame": {"x": 200, "y": 0, "w": 200, "h": 120}}})),
        )
        .unwrap();
        assert_eq!(frame_of(&deck, &a), [200., 0., 40., 40.]);
        assert_eq!(frame_of(&deck, &b), [360., 80., 40., 40.]);
        // Turned only: the group is drawn turned, its children stay.
        apply(
            &mut deck,
            op(json!({"op": "update_element", "id": group, "set": {"frame": {"rotation": 90}}})),
        )
        .unwrap();
        assert_eq!(frame_of(&deck, &b), [360., 80., 40., 40.]);

        // Another slide size scales what is in the group as well.
        apply(&mut deck, op(json!({"op": "set_size", "preset": "4:3"}))).unwrap();
        let (outer, inner) = (frame_of(&deck, &group), frame_of(&deck, &b));
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(
            near(outer[2], 150.) && near(inner[2], 30.),
            "{outer:?} {inner:?}"
        );
        assert!(
            near(inner[0] + inner[2], outer[0] + outer[2])
                && near(inner[1] + inner[3], outer[1] + outer[3]),
            "still in its corner of the group: {outer:?} {inner:?}"
        );

        // A group that is a number short stays where it is.
        let mut element = deck.element(&group).unwrap().1.clone();
        let children = element.kind.clone();
        set_frame(&mut element, Frame::new(f32::NAN, 0., 10., 10.));
        assert_eq!(element.kind, children);
    }

    #[test]
    fn numbers_too_large_are_refused() {
        let mut deck = new_deck("T", "clean", SlideSize::default());
        let id = shape(&mut deck, 1, [10., 10., 50., 50.]);
        let before = deck.clone();
        // Above what an `f32` holds: read as infinite, saved as `null`,
        // and the deck would not open again.
        for change in [
            json!({"op": "update_element", "id": id, "set": {"opacity": 1e39}}),
            json!({"op": "update_element", "id": id, "set": {"line": {"width": -1e39}}}),
            json!({"op": "add_element", "slide": 1, "element": {
                "type": "picture", "media": "a.png", "radius": 1e300}}),
            json!({"op": "update_slide", "slide": 1, "background": {
                "type": "solid", "color": "accent1", "opacity": 1e39}}),
            json!({"op": "style_text", "id": id, "run": {"size": 1e39}}),
            json!({"op": "set_theme", "custom": {"title": {"size": 20, "space_after": 1e39}}}),
        ] {
            assert!(apply(&mut deck, op(change.clone())).is_err(), "{change}");
        }
        assert_eq!(deck, before);
        // A comment placed out of range is put where one without a place is.
        apply(
            &mut deck,
            op(json!({"op": "add_comment", "slide": 1, "text": "Here", "x": 1e39, "y": 5})),
        )
        .unwrap();
        assert_eq!(deck.slides[0].comments[0].x, 24.);
        let saved = serde_json::to_string(&deck).unwrap();
        assert_eq!(serde_json::from_str::<Deck>(&saved).unwrap(), deck);
    }

    #[test]
    fn text_sizes_stay_what_the_layout_takes() {
        let mut deck = new_deck("T", "clean", SlideSize::default());
        let code = apply(
            &mut deck,
            op(json!({"op": "add_element", "slide": 1, "element": {
                "type": "code", "code": "a\n\nb", "font_size": 0}})),
        )
        .unwrap()
        .unwrap();
        let size = |deck: &Deck, id: &str| match &deck.element(id).unwrap().1.kind {
            Kind::Code(code) => code.font_size,
            Kind::Table(table) => table.font_size,
            _ => panic!("code or a table"),
        };
        assert_eq!(size(&deck, &code), 1.);
        apply(
            &mut deck,
            op(json!({"op": "update_element", "id": code, "set": {"font_size": 5000}})),
        )
        .unwrap();
        assert_eq!(size(&deck, &code), 400.);
        // In a group too (children are given in full, with their `kind`).
        let table = json!({"kind": {"type": "table", "rows": [[{"text": "a"}]], "font_size": 0}});
        let group = apply(
            &mut deck,
            op(json!({"op": "add_element", "slide": 1, "element": {
                "type": "group", "children": [table]}})),
        )
        .unwrap()
        .unwrap();
        let Kind::Group(group) = &deck.element(&group).unwrap().1.kind else {
            panic!("a group")
        };
        assert_eq!(size(&deck, &group.children[0].id), 1.);
        // And a whole theme's sizes.
        let mut theme = serde_json::to_value(&deck.theme).unwrap();
        theme["title"]["size"] = json!(0);
        theme["layouts"][0]["placeholders"][0]["size"] = json!(0);
        apply(&mut deck, op(json!({"op": "set_theme", "custom": theme}))).unwrap();
        assert_eq!(deck.theme.title.size, 1.);
        assert_eq!(deck.theme.layouts[0].placeholders[0].size, Some(1.));
    }

    #[test]
    fn an_emptied_section_goes_with_its_slide() {
        let mut deck = new_deck("T", "clean", SlideSize::default());
        for _ in 0..2 {
            apply(&mut deck, op(json!({"op": "add_slide"}))).unwrap();
        }
        apply(
            &mut deck,
            op(json!({"op": "add_section", "name": "One", "slide": 2})),
        )
        .unwrap();
        apply(
            &mut deck,
            op(json!({"op": "add_section", "name": "Two", "slide": 3})),
        )
        .unwrap();
        let third = deck.slides[2].id.clone();
        apply(&mut deck, op(json!({"op": "remove_slide", "slide": 2}))).unwrap();
        let sections: Vec<(&str, &str)> = deck
            .sections
            .iter()
            .map(|section| (section.name.as_str(), section.slide.as_str()))
            .collect();
        assert_eq!(sections, [("Two", third.as_str())]);
        // With no section after it, the section moves on to the next slide.
        apply(&mut deck, op(json!({"op": "add_slide"}))).unwrap();
        apply(&mut deck, op(json!({"op": "remove_slide", "slide": 2}))).unwrap();
        assert_eq!(deck.sections[0].slide, deck.slides[1].id);
    }
}
