//! The slides editor: the slides down the left, the tools to add things
//! across the top, the slide being edited in the middle (select, move,
//! resize, turn, type in place), under it the bar for what is selected and
//! the speaker notes.
//!
//! Every change goes through the document ([`DeckDocument`]), so it can be
//! undone, is saved a moment later, and shows wherever the deck is drawn
//! (the rail, presenting). An agent's changes arrive the same way.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, TextareaState},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ClipboardEntry, ClipboardItem, Context, Entity,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, Styled as _, Subscription, UniformListScrollHandle, Window, div, px,
};
use ph_ui::{Icons, MenuButton as _};
use serde_json::json;

use super::document::{DeckDocument, ExportFormat, Selection};
use super::model::*;
use super::ops::Op;

mod bar;
mod canvas;
mod dialogs;
pub mod geometry;
mod present;
mod rail;
mod side;
mod toolbar;

pub use present::present;

/// What the editor asks of its window.
#[derive(Debug, Clone)]
pub enum SlidesEditorEvent {
    /// A message for the agent, for the user to finish and send.
    Ask(String),
}

impl gpui_kit::EventEmitter<SlidesEditorEvent> for SlidesEditor {}

const CONTEXT: &str = "SlidesEditor";
/// What copied elements are marked with on the clipboard.
const CLIPBOARD: &str = "pilothouse-slides:";

gpui_kit::actions!(
    slides,
    [
        Undo,
        Redo,
        Delete,
        Duplicate,
        Copy,
        Cut,
        Paste,
        Group,
        Ungroup,
        SelectAll,
        Cancel,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        NudgeLeftFar,
        NudgeRightFar,
        NudgeUpFar,
        NudgeDownFar,
        EditText,
        NewSlide,
        Present,
        PresentFromStart,
        BringForward,
        SendBackward,
        BringToFront,
        SendToBack,
        PreviousSlide,
        NextSlide,
        Find,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        gpui_kit::KeyBinding::new("secondary-z", Undo, context),
        gpui_kit::KeyBinding::new("secondary-shift-z", Redo, context),
        gpui_kit::KeyBinding::new("ctrl-y", Redo, context),
        gpui_kit::KeyBinding::new("backspace", Delete, context),
        gpui_kit::KeyBinding::new("delete", Delete, context),
        gpui_kit::KeyBinding::new("secondary-d", Duplicate, context),
        gpui_kit::KeyBinding::new("secondary-c", Copy, context),
        gpui_kit::KeyBinding::new("secondary-x", Cut, context),
        gpui_kit::KeyBinding::new("secondary-v", Paste, context),
        gpui_kit::KeyBinding::new("secondary-g", Group, context),
        gpui_kit::KeyBinding::new("secondary-shift-g", Ungroup, context),
        gpui_kit::KeyBinding::new("secondary-a", SelectAll, context),
        gpui_kit::KeyBinding::new("escape", Cancel, context),
        gpui_kit::KeyBinding::new("left", NudgeLeft, context),
        gpui_kit::KeyBinding::new("right", NudgeRight, context),
        gpui_kit::KeyBinding::new("up", NudgeUp, context),
        gpui_kit::KeyBinding::new("down", NudgeDown, context),
        gpui_kit::KeyBinding::new("shift-left", NudgeLeftFar, context),
        gpui_kit::KeyBinding::new("shift-right", NudgeRightFar, context),
        gpui_kit::KeyBinding::new("shift-up", NudgeUpFar, context),
        gpui_kit::KeyBinding::new("shift-down", NudgeDownFar, context),
        gpui_kit::KeyBinding::new("enter", EditText, context),
        gpui_kit::KeyBinding::new("secondary-m", NewSlide, context),
        gpui_kit::KeyBinding::new("secondary-enter", Present, context),
        gpui_kit::KeyBinding::new("f5", PresentFromStart, context),
        gpui_kit::KeyBinding::new("secondary-shift-enter", PresentFromStart, context),
        gpui_kit::KeyBinding::new("secondary-]", BringForward, context),
        gpui_kit::KeyBinding::new("secondary-[", SendBackward, context),
        gpui_kit::KeyBinding::new("secondary-shift-]", BringToFront, context),
        gpui_kit::KeyBinding::new("secondary-shift-[", SendToBack, context),
        gpui_kit::KeyBinding::new("pageup", PreviousSlide, context),
        gpui_kit::KeyBinding::new("pagedown", NextSlide, context),
        gpui_kit::KeyBinding::new("secondary-f", Find, context),
    ]);
    present::bind_keys(cx);
}

/// Text being typed in place.
pub(crate) struct Editing {
    pub id: String,
    pub input: Entity<TextareaState>,
    /// What it held as Markdown when typing began.
    pub before: String,
    /// It is code (typed as is, not Markdown).
    pub code: bool,
}

pub struct SlidesEditor {
    pub(crate) doc: Entity<DeckDocument>,
    pub(crate) focus: FocusHandle,
    /// The slide being edited, and its id (kept across changes).
    pub(crate) current: usize,
    current_id: Option<String>,
    /// Selected elements of the current slide (top level), by id.
    pub(crate) selected: Vec<String>,
    pub(crate) drag: Option<canvas::Drag>,
    pub(crate) guides: Vec<geometry::Guide>,
    /// The canvas as last laid out (window coordinates).
    pub(crate) bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(crate) placement: Option<canvas::Placement>,
    /// 1 fits the slide in the canvas.
    pub(crate) zoom: f32,
    /// Screen pixels the zoomed slide is moved by.
    pub(crate) pan: [f32; 2],
    pub(crate) editing: Option<Editing>,
    pub(crate) notes: Entity<TextareaState>,
    pub(crate) notes_shown: bool,
    pub(crate) rail: UniformListScrollHandle,
    /// The rail's slides in view in the frame being drawn.
    pub(crate) rail_view: Option<std::ops::Range<usize>>,
    /// The pointer's look over the canvas, and the element under it.
    pub(crate) cursor: gpui_kit::CursorStyle,
    pub(crate) hovered: Option<String>,
    /// The picture being cropped by hand.
    pub(crate) cropping: Option<String>,
    /// Where a line's end being dragged can be fixed (sides of elements
    /// near it).
    pub(crate) sites: Vec<[f32; 2]>,
    /// The side panel shown, and its state.
    pub(crate) side: Option<side::Side>,
    pub(crate) comment_input: Entity<gpui_kit::component::input::InputState>,
    /// The comment being replied to.
    pub(crate) replying: Option<String>,
    pub(crate) references: Vec<side::Reference>,
    /// Watching the field being typed in, and a dialog's fields.
    pub(crate) typing_watch: Option<Subscription>,
    pub(crate) dialog_watch: Option<Subscription>,
    pub(crate) dialog_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl SlidesEditor {
    pub fn new(doc: Entity<DeckDocument>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Speaker notes for this slide")
                .auto_grow(2, 8)
        });
        let mut subscriptions = vec![cx.observe_in(&doc, window, |this, _, window, cx| {
            this.follow_document(window, cx);
            cx.notify();
        })];
        subscriptions.push(cx.subscribe_in(
            &notes,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    let Some(id) = this.current_id.clone() else {
                        return;
                    };
                    this.doc.update(cx, |doc, cx| {
                        let Some(index) = doc.deck().slides.iter().position(|s| s.id == id) else {
                            return;
                        };
                        if doc.deck().slides[index].notes == text {
                            return;
                        }
                        doc.update(
                            "notes",
                            |deck| std::sync::Arc::make_mut(&mut deck.slides[index]).notes = text,
                            cx,
                        );
                    });
                }
            },
        ));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let current_id = doc
            .read(cx)
            .deck()
            .slides
            .first()
            .map(|slide| slide.id.clone());
        let mut this = Self {
            doc,
            focus,
            current: 0,
            current_id,
            selected: Vec::new(),
            drag: None,
            guides: Vec::new(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            placement: None,
            zoom: 1.,
            pan: [0., 0.],
            editing: None,
            notes,
            notes_shown: true,
            rail: UniformListScrollHandle::new(),
            rail_view: None,
            cursor: gpui_kit::CursorStyle::Arrow,
            hovered: None,
            cropping: None,
            sites: Vec::new(),
            side: None,
            comment_input: Self::comment_field(window, cx),
            replying: None,
            references: Vec::new(),
            typing_watch: None,
            dialog_watch: None,
            dialog_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.show_notes(window, cx);
        this
    }

    pub fn document(&self) -> &Entity<DeckDocument> {
        &self.doc
    }

    pub(crate) fn deck<'a>(&self, cx: &'a App) -> &'a Deck {
        self.doc.read(cx).deck()
    }

    pub(crate) fn slide<'a>(&self, cx: &'a App) -> Option<&'a Slide> {
        self.deck(cx).slides.get(self.current).map(|slide| &**slide)
    }

    /// The selected elements, in their order on the slide.
    pub(crate) fn selected_elements<'a>(&self, cx: &'a App) -> Vec<&'a Element> {
        self.slide(cx)
            .map(|slide| {
                slide
                    .elements
                    .iter()
                    .filter(|element| self.selected.contains(&element.id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// After a change (the user's, undo's or an agent's): the current
    /// slide followed by id, the selection kept to what is still there,
    /// the notes shown as they are.
    fn follow_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let deck = self.doc.read(cx).deck();
        let count = deck.slides.len();
        if let Some(index) = self
            .current_id
            .as_ref()
            .and_then(|id| deck.slides.iter().position(|slide| &slide.id == id))
        {
            self.current = index;
        } else {
            self.current = self.current.min(count.saturating_sub(1));
            self.current_id = deck.slides.get(self.current).map(|slide| slide.id.clone());
            self.selected.clear();
        }
        let ids: Vec<String> = deck
            .slides
            .get(self.current)
            .map(|slide| {
                slide
                    .elements
                    .iter()
                    .map(|element| element.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.selected.retain(|id| ids.contains(id));
        if self.editing.as_ref().is_some_and(|editing| {
            !ids.contains(&editing.id) && deck.element(&editing.id).is_none()
        }) {
            self.editing = None;
        }
        self.show_notes(window, cx);
        self.share_selection(cx);
    }

    /// The notes field shows the current slide's notes (unless it is being
    /// typed in).
    fn show_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let notes = self
            .slide(cx)
            .map(|slide| slide.notes.clone())
            .unwrap_or_default();
        let input = self.notes.clone();
        let typing = input.read(cx).focus_handle(cx).is_focused(window);
        if !typing && input.read(cx).value().as_ref() != notes {
            input.update(cx, |input, cx| input.set_value(notes, window, cx));
        }
    }

    /// Tells the document what is selected, for the agent.
    pub(crate) fn share_selection(&mut self, cx: &mut Context<Self>) {
        let slide = self.current_id.clone();
        let elements = self.selected.clone();
        let text = self.editing.as_ref().and_then(|editing| {
            let input = editing.input.read(cx);
            let range = input.selected_range();
            let value = input.value();
            (range.start < range.end)
                .then(|| value.get(range.clone()).map(str::to_string))
                .flatten()
        });
        let selection = Selection {
            slide,
            elements,
            text,
        };
        self.doc.update(cx, |doc, _| doc.selection = selection);
    }

    pub(crate) fn go_to(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_editing(window, cx);
        let count = self.deck(cx).slides.len();
        if count == 0 {
            return;
        }
        let index = index.min(count - 1);
        if index != self.current {
            self.selected.clear();
        }
        self.current = index;
        self.current_id = self
            .deck(cx)
            .slides
            .get(index)
            .map(|slide| slide.id.clone());
        self.rail
            .scroll_to_item(index, gpui_kit::ScrollStrategy::Top);
        self.show_notes(window, cx);
        self.share_selection(cx);
        cx.notify();
    }

    pub(crate) fn select(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if self.selected != ids {
            if self
                .cropping
                .as_ref()
                .is_some_and(|id| ids.as_slice() != std::slice::from_ref(id))
            {
                self.cropping = None;
            }
            self.selected = ids;
            self.share_selection(cx);
            cx.notify();
        }
    }

    /// Changes the deck, as one undo step named `what`.
    pub(crate) fn change(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut Deck),
        cx: &mut Context<Self>,
    ) {
        self.doc.update(cx, |doc, cx| doc.update(what, change, cx));
    }

    /// Applies operations as one step, telling the user if they fail.
    pub(crate) fn apply(
        &mut self,
        what: &'static str,
        ops: Vec<Op>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Option<String>>> {
        let result = self.doc.update(cx, |doc, cx| {
            doc.settle();
            let result = doc.apply(what, ops, cx);
            doc.settle();
            result
        });
        match result {
            Ok(made) => Some(made),
            Err(err) => {
                window.push_notification(Notification::error(format!("{err:#}")), cx);
                None
            }
        }
    }

    /// Changes every selected element.
    pub(crate) fn change_selected(
        &mut self,
        what: &'static str,
        change: impl Fn(&mut Element),
        cx: &mut Context<Self>,
    ) {
        let ids = self.selected.clone();
        if ids.is_empty() {
            return;
        }
        let index = self.current;
        self.change(
            what,
            move |deck| {
                let Some(slide) = deck.slides.get_mut(index) else {
                    return;
                };
                let slide = std::sync::Arc::make_mut(slide);
                for element in &mut slide.elements {
                    if ids.contains(&element.id) {
                        change(element);
                    }
                }
            },
            cx,
        );
    }

    /// Adds `element` (JSON, as agents give it) to the current slide and
    /// selects it.
    pub(crate) fn insert(
        &mut self,
        element: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.finish_editing(window, cx);
        if self.deck(cx).slides.is_empty() {
            self.apply(
                "slide",
                vec![Op::AddSlide {
                    layout: Some("blank".into()),
                    after: None,
                    at: None,
                    title: None,
                    subtitle: None,
                    body: None,
                    body2: None,
                    notes: None,
                    elements: Vec::new(),
                }],
                window,
                cx,
            );
            self.go_to(0, window, cx);
        }
        let slide = self.current_id.clone()?;
        let made = self.apply(
            "insert",
            vec![Op::AddElement {
                slide: json!(slide),
                element,
                behind: false,
            }],
            window,
            cx,
        )?;
        let id = made.into_iter().flatten().next()?;
        self.select(vec![id.clone()], cx);
        self.focus.focus(window, cx);
        Some(id)
    }

    /// The slide's middle, and a frame of `w` × `h` centred on it.
    pub(crate) fn centered(&self, w: f32, h: f32, cx: &App) -> serde_json::Value {
        let size = &self.deck(cx).size;
        let (w, h) = (w.min(size.width * 0.9), h.min(size.height * 0.9));
        json!({ "x": (size.width - w) / 2., "y": (size.height - h) / 2., "w": w, "h": h })
    }

    // Commands.

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_editing(window, cx);
        self.doc.update(cx, |doc, cx| doc.undo(cx));
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_editing(window, cx);
        self.doc.update(cx, |doc, cx| doc.redo(cx));
    }

    pub(crate) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        let ops = self
            .selected
            .iter()
            .map(|id| Op::RemoveElement { id: id.clone() })
            .collect();
        self.apply("delete", ops, window, cx);
        self.select(Vec::new(), cx);
    }

    pub(crate) fn duplicate(&mut self, _: &Duplicate, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            // Nothing selected: the slide.
            if let Some(id) = self.current_id.clone() {
                let made = self.apply(
                    "duplicate",
                    vec![Op::DuplicateSlide { slide: json!(id) }],
                    window,
                    cx,
                );
                if made.is_some() {
                    self.go_to(self.current + 1, window, cx);
                }
            }
            return;
        }
        let copies = self.copies(cx, 12.);
        self.paste_elements(copies, window, cx);
    }

    /// The selected elements as JSON, moved by `offset` points.
    fn copies(&self, cx: &App, offset: f32) -> Vec<serde_json::Value> {
        self.selected_elements(cx)
            .into_iter()
            .map(|element| {
                let mut element = element.clone();
                element.frame.x += offset;
                element.frame.y += offset;
                element.placeholder = None;
                serde_json::to_value(&element).unwrap_or_default()
            })
            .collect()
    }

    fn paste_elements(
        &mut self,
        elements: Vec<serde_json::Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slide) = self.current_id.clone() else {
            return;
        };
        let ops = elements
            .into_iter()
            .map(|element| Op::AddElement {
                slide: json!(slide),
                element,
                behind: false,
            })
            .collect();
        if let Some(made) = self.apply("paste", ops, window, cx) {
            self.select(made.into_iter().flatten().collect(), cx);
        }
    }

    pub(crate) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let copies = self.copies(cx, 0.);
        if copies.is_empty() {
            return;
        }
        let text = format!("{CLIPBOARD}{}", serde_json::Value::Array(copies));
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        self.delete(&Delete, window, cx);
    }

    /// Pastes elements copied here, a picture, or text as a text box.
    pub(crate) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        if let Some(text) = item.text() {
            if let Some(json) = text.strip_prefix(CLIPBOARD)
                && let Ok(serde_json::Value::Array(mut elements)) = serde_json::from_str(json)
            {
                // Pasted onto the elements copied: a little apart.
                let occupied: Vec<(f32, f32)> = self
                    .slide(cx)
                    .map(|slide| {
                        slide
                            .elements
                            .iter()
                            .map(|e| (e.frame.x, e.frame.y))
                            .collect()
                    })
                    .unwrap_or_default();
                for element in &mut elements {
                    let at = (
                        element["frame"]["x"].as_f64().unwrap_or(0.) as f32,
                        element["frame"]["y"].as_f64().unwrap_or(0.) as f32,
                    );
                    if occupied
                        .iter()
                        .any(|o| (o.0 - at.0).abs() < 0.5 && (o.1 - at.1).abs() < 0.5)
                    {
                        element["frame"]["x"] = json!(at.0 + 12.);
                        element["frame"]["y"] = json!(at.1 + 12.);
                    }
                }
                self.paste_elements(elements, window, cx);
                return;
            }
            if !text.trim().is_empty() {
                let frame = self.centered(self.deck(cx).size.width * 0.6, 80., cx);
                self.insert(
                    json!({ "type": "text", "text": text.trim(), "frame": frame }),
                    window,
                    cx,
                );
                return;
            }
        }
        let image = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image.clone()),
            _ => None,
        });
        if let Some(image) = image {
            let extension = match image.format {
                gpui_kit::ImageFormat::Jpeg => "jpg",
                gpui_kit::ImageFormat::Gif => "gif",
                gpui_kit::ImageFormat::Webp => "webp",
                gpui_kit::ImageFormat::Svg => "svg",
                _ => "png",
            };
            // Decoded and made smaller off the UI thread (a 5K screenshot
            // takes a while).
            let space = self.doc.read(cx).space().clone();
            let added =
                cx.background_spawn(async move { space.add_media_bytes(&image.bytes, extension) });
            cx.spawn_in(window, async move |this, cx| {
                let added = added.await;
                _ = this.update_in(cx, |this, window, cx| match added {
                    Ok(name) => this.insert_picture(name, window, cx),
                    Err(err) => window
                        .push_notification(Notification::error(format!("Not pasted: {err:#}")), cx),
                });
            })
            .detach();
        }
    }

    /// A picture from the space's media, as large as fits nicely.
    pub(crate) fn insert_picture(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let media = self.doc.read(cx).media();
        let (iw, ih) = image::image_dimensions(media.join(&name)).unwrap_or((800, 600));
        let size = &self.deck(cx).size;
        let fit = (size.width * 0.6 / iw as f32).min(size.height * 0.6 / ih as f32);
        let (w, h) = (iw as f32 * fit, ih as f32 * fit);
        let frame = self.centered(w, h, cx);
        self.insert(
            json!({ "type": "picture", "media": name, "fit": "cover", "frame": frame }),
            window,
            cx,
        );
    }

    /// Asks for pictures and adds them.
    pub(crate) fn pick_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Insert".into()),
        });
        let space = self.doc.read(cx).space().clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let added = cx
                .background_spawn(async move {
                    paths
                        .iter()
                        .map(|path| space.add_media(path))
                        .collect::<Vec<_>>()
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| {
                for added in added {
                    match added {
                        Ok(name) => this.insert_picture(name, window, cx),
                        Err(err) => {
                            window.push_notification(Notification::error(format!("{err:#}")), cx)
                        }
                    }
                }
            });
        })
        .detach();
    }

    fn group(&mut self, _: &Group, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.len() < 2 {
            return;
        }
        let ids = self.selected.clone();
        if let Some(made) = self.apply("group", vec![Op::Group { ids, name: None }], window, cx) {
            self.select(made.into_iter().flatten().collect(), cx);
        }
    }

    fn ungroup(&mut self, _: &Ungroup, window: &mut Window, cx: &mut Context<Self>) {
        let groups: Vec<(String, Vec<String>)> = self
            .selected_elements(cx)
            .into_iter()
            .filter_map(|element| match &element.kind {
                Kind::Group(group) => Some((
                    element.id.clone(),
                    group
                        .children
                        .iter()
                        .map(|child| child.id.clone())
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        if groups.is_empty() {
            return;
        }
        let ops = groups
            .iter()
            .map(|(id, _)| Op::Ungroup { id: id.clone() })
            .collect();
        if self.apply("ungroup", ops, window, cx).is_some() {
            self.select(
                groups
                    .into_iter()
                    .flat_map(|(_, children)| children)
                    .collect(),
                cx,
            );
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self
            .slide(cx)
            .map(|slide| {
                slide
                    .elements
                    .iter()
                    .map(|element| element.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.select(ids, cx);
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.finish_editing(window, cx);
        } else if self.cropping.take().is_some() {
            cx.notify();
        } else if self.drag.take().is_some() {
            self.guides.clear();
            cx.notify();
        } else {
            self.select(Vec::new(), cx);
        }
    }

    fn nudge(&mut self, dx: f32, dy: f32, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            // Nothing selected: the arrows go through the slides.
            let next = if dx + dy > 0. {
                self.current + 1
            } else {
                self.current.saturating_sub(1)
            };
            self.go_to(next, window, cx);
            return;
        }
        self.change_selected(
            "nudge",
            move |element| {
                element.frame.x += dx;
                element.frame.y += dy;
            },
            cx,
        );
    }

    fn edit_text(&mut self, _: &EditText, window: &mut Window, cx: &mut Context<Self>) {
        if let [id] = self.selected.as_slice() {
            let id = id.clone();
            self.start_editing(&id, window, cx);
        }
    }

    /// A new slide after this one, in the layout that usually follows.
    pub(crate) fn new_slide(
        &mut self,
        layout: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let after = self.current_id.clone();
        let layout = layout.or_else(|| {
            let deck = self.deck(cx);
            match deck
                .slides
                .get(self.current)
                .map(|slide| slide.layout.as_str())
            {
                None => Some("title".into()),
                Some("title" | "section" | "closing") => Some("title_content".into()),
                Some(current) => Some(current.to_string()),
            }
        });
        let made = self.apply(
            "slide",
            vec![Op::AddSlide {
                layout,
                after: after.map(|id| json!(id)),
                at: None,
                title: None,
                subtitle: None,
                body: None,
                body2: None,
                notes: None,
                elements: Vec::new(),
            }],
            window,
            cx,
        );
        if let Some(id) = made.into_iter().flatten().flatten().next()
            && let Some(index) = self.deck(cx).slides.iter().position(|slide| slide.id == id)
        {
            self.go_to(index, window, cx);
        }
    }

    pub(crate) fn order(&mut self, to: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Front first for "forward", so the selection keeps its order.
        let mut ids = self.selected.clone();
        if matches!(to, "forward" | "front") {
            ids.reverse();
        }
        let ops = ids
            .into_iter()
            .map(|id| Op::Order { id, to: to.into() })
            .collect();
        self.apply("order", ops, window, cx);
    }

    pub(crate) fn present_from(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_editing(window, cx);
        present::present(self.doc.clone(), index, cx);
    }

    /// Writes the deck as `format` in its exports folder, then shows it.
    pub(crate) fn export(
        &mut self,
        format: ExportFormat,
        slides: Vec<usize>,
        notes: bool,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_editing(window, cx);
        let doc = self.doc.read(cx);
        let path = path.unwrap_or_else(|| {
            let mut path = doc.export_path(format.extension());
            if slides.len() == 1 && format == ExportFormat::Png {
                let stem = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                path.set_file_name(format!("{stem} - slide {}.png", slides[0] + 1));
            } else if notes {
                let stem = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                path.set_file_name(format!("{stem} with notes.pdf"));
            }
            path
        });
        let writing = self.doc.update(cx, |doc, cx| {
            doc.export(format, path, slides, notes, 1920, cx)
        });
        cx.spawn_in(window, async move |_, cx| {
            let result = writing.await;
            _ = cx.update(|window, cx| match result {
                Ok(written) => {
                    window.push_notification(
                        Notification::success(format!(
                            "Exported {}.",
                            written.file_name().unwrap_or_default().to_string_lossy()
                        )),
                        cx,
                    );
                    cx.reveal_path(&written);
                }
                Err(err) => window
                    .push_notification(Notification::error(format!("Not exported: {err:#}")), cx),
            });
        })
        .detach();
    }

    /// Writes the deck over the PowerPoint file it was opened from, once
    /// the user says so.
    pub(crate) fn save_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_editing(window, cx);
        let Some(original) = self
            .deck(cx)
            .source
            .as_ref()
            .and_then(|source| source.original.clone())
        else {
            return;
        };
        let name = original
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let answer = window.prompt(
            gpui_kit::PromptLevel::Warning,
            &format!("Replace {name}?"),
            Some("The presentation is written over the PowerPoint file it was opened from. Its earlier content is kept only in this presentation's versions."),
            &["Replace", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let Ok(writing) = this.update(cx, |this, cx| {
                this.doc.update(cx, |doc, cx| {
                    doc.export(
                        ExportFormat::Pptx,
                        original.clone(),
                        Vec::new(),
                        false,
                        1920,
                        cx,
                    )
                })
            }) else {
                return;
            };
            let result = writing.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok(written) => {
                    let stamp = super::space::modified_secs(&written);
                    this.doc.update(cx, |doc, cx| doc.saved_original(stamp, cx));
                    window
                        .push_notification(Notification::success(format!("Saved to {name}.")), cx);
                }
                Err(err) => {
                    window.push_notification(Notification::error(format!("Not saved: {err:#}")), cx)
                }
            });
        })
        .detach();
    }

    /// Asks where, and exports there (the format from the name).
    pub(crate) fn export_as(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let doc = self.doc.read(cx);
        let suggested = doc.export_path(format.extension());
        let folder = suggested.parent().map(PathBuf::from).unwrap_or_default();
        let name = suggested
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = chosen.await else {
                return;
            };
            let format = path
                .extension()
                .and_then(|extension| ExportFormat::from_extension(&extension.to_string_lossy()))
                .unwrap_or_else(|| {
                    path.set_extension(format.extension());
                    format
                });
            _ = this.update_in(cx, |this, window, cx| {
                this.export(format, Vec::new(), false, Some(path), window, cx)
            });
        })
        .detach();
    }

    // The window's title bar.

    pub fn title_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let doc = self.doc.read(cx);
        let (can_undo, can_redo) = (doc.can_undo(), doc.can_redo());
        let error = doc.save_error().map(str::to_string);
        let undo_key = if cfg!(target_os = "macos") {
            "⌘Z"
        } else {
            "Ctrl+Z"
        };
        let editor = cx.weak_entity();
        let export = Button::new("slides-export")
            .primary()
            .small()
            .icon(Icons::Download)
            .label("Export")
            .dropdown_caret(true)
            .menu(gpui_kit::Anchor::TopRight, {
                let editor = editor.clone();
                move |menu, _, cx| {
                    let current = editor.upgrade().map_or(0, |editor| editor.read(cx).current);
                    let item = |label: &'static str,
                                format: ExportFormat,
                                slides: Vec<usize>,
                                notes: bool| {
                        let editor = editor.clone();
                        PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            _ = editor.update(cx, |this, cx| {
                                this.export(format, slides.clone(), notes, None, window, cx)
                            });
                        })
                    };
                    let save_as = editor.clone();
                    let dialog = editor.clone();
                    // Opened from a PowerPoint file: saved back to it.
                    let original = editor.upgrade().and_then(|editor| {
                        let source = editor.read(cx).deck(cx).source.clone()?;
                        let original = source.original?;
                        let pptx = original
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("pptx"));
                        pptx.then(|| {
                            original
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        })
                    });
                    let back = editor.clone();
                    let menu = match original {
                        Some(name) => menu
                            .item(PopupMenuItem::new(format!("Save to {name}")).on_click(
                                move |_, window, cx| {
                                    _ = back.update(cx, |this, cx| this.save_back(window, cx));
                                },
                            ))
                            .separator(),
                        None => menu,
                    };
                    menu.item(item(
                        "PowerPoint (.pptx)",
                        ExportFormat::Pptx,
                        Vec::new(),
                        false,
                    ))
                    .item(item("PDF", ExportFormat::Pdf, Vec::new(), false))
                    .item(item(
                        "PDF with speaker notes",
                        ExportFormat::Pdf,
                        Vec::new(),
                        true,
                    ))
                    .item(item(
                        "Pictures of every slide (PNG)",
                        ExportFormat::Png,
                        Vec::new(),
                        false,
                    ))
                    .item(item(
                        "Picture of this slide (PNG)",
                        ExportFormat::Png,
                        vec![current],
                        false,
                    ))
                    .separator()
                    .item(
                        PopupMenuItem::new("Export as…").on_click(move |_, window, cx| {
                            _ = save_as.update(cx, |this, cx| {
                                this.export_as(ExportFormat::Pptx, window, cx)
                            });
                        }),
                    )
                    .item(
                        PopupMenuItem::new("More options…").on_click(move |_, window, cx| {
                            _ = dialog.update(cx, |this, cx| this.open_export(window, cx));
                        }),
                    )
                }
            });
        let more = Button::new("slides-more")
            .ghost()
            .small()
            .icon(Icons::Ellipsis)
            .tooltip("More")
            .menu(gpui_kit::Anchor::TopRight, {
                let editor = editor.clone();
                move |menu, _, _| {
                    let (rename, theme, reveal, versions) = (
                        editor.clone(),
                        editor.clone(),
                        editor.clone(),
                        editor.clone(),
                    );
                    menu.item(
                        PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                            _ = rename.update(cx, |this, cx| this.open_rename(window, cx));
                        }),
                    )
                    .item(PopupMenuItem::new("Save theme for other decks…").on_click(
                        move |_, window, cx| {
                            _ = theme.update(cx, |this, cx| this.open_save_theme(window, cx));
                        },
                    ))
                    .item(
                        PopupMenuItem::new("Earlier versions…").on_click(move |_, window, cx| {
                            _ = versions.update(cx, |this, cx| this.open_versions(window, cx));
                        }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(reveal_label()).on_click(move |_, _, cx| {
                            if let Some(editor) = reveal.upgrade() {
                                let folder = editor.read(cx).doc.read(cx).folder().to_path_buf();
                                cx.reveal_path(&folder.join(super::space::DECK));
                            }
                        }),
                    )
                }
            });
        h_flex()
            .gap_1()
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .max_w(px(220.))
                        .truncate()
                        .child(format!("Not saved: {error}")),
                )
            })
            .child(
                Button::new("slides-undo")
                    .ghost()
                    .small()
                    .icon(Icons::Undo2)
                    .disabled(!can_undo)
                    .tooltip(format!("Undo ({undo_key})"))
                    .on_click(cx.listener(|this, _, window, cx| this.undo(&Undo, window, cx))),
            )
            .child(
                Button::new("slides-redo")
                    .ghost()
                    .small()
                    .icon(Icons::Redo2)
                    .disabled(!can_redo)
                    .tooltip("Redo")
                    .on_click(cx.listener(|this, _, window, cx| this.redo(&Redo, window, cx))),
            )
            .child(more)
            .child({
                let editor = editor.clone();
                Button::new("slides-present")
                    .ghost()
                    .small()
                    .icon(Icons::Play)
                    .label("Present")
                    .dropdown_caret(true)
                    .tooltip(if cfg!(target_os = "macos") {
                        "Present (⌘↩ from this slide; ⇧⌘↩ from the start)"
                    } else {
                        "Present (Ctrl+Enter from this slide; F5 from the start)"
                    })
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let (here, start, rehearse) =
                            (editor.clone(), editor.clone(), editor.clone());
                        menu.item(PopupMenuItem::new("From this slide").on_click(
                            move |_, window, cx| {
                                _ = here.update(cx, |this, cx| {
                                    let current = this.current;
                                    this.present_from(current, window, cx)
                                });
                            },
                        ))
                        .item(PopupMenuItem::new("From the start").on_click(
                            move |_, window, cx| {
                                _ = start.update(cx, |this, cx| this.present_from(0, window, cx));
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Rehearse in the presenter's view").on_click(
                                move |_, window, cx| {
                                    _ = rehearse.update(cx, |this, cx| {
                                        this.finish_editing(window, cx);
                                        let current = this.current;
                                        present::rehearse(this.doc.clone(), current, cx);
                                    });
                                },
                            ),
                        )
                    })
            })
            .child(export)
            .into_any_element()
    }
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else if cfg!(target_os = "windows") {
        "Show in Explorer"
    } else {
        "Show in file manager"
    }
}

use gpui_kit::component::menu::PopupMenuItem;

impl Focusable for SlidesEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SlidesEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let canvas = self.render_canvas(window, cx);
        let rail = self.render_rail(window, cx);
        let toolbar = self.render_toolbar(cx);
        let bar = self.render_bar(window, cx);
        let side = self.render_side(window, cx);
        // Pictures replaced are freed on the GPU, in every window (the
        // presenting ones draw the same deck).
        let stale = self.doc.update(cx, |doc, _| doc.take_stale());
        for image in stale {
            cx.drop_image(image, Some(window));
        }
        let notes = self.notes_shown.then(|| {
            div()
                .flex_none()
                .border_t_1()
                .border_color(theme.border)
                .px_3()
                .py_2()
                .child(gpui_kit::component::input::Textarea::new(&self.notes).appearance(false))
        });
        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::duplicate))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::group))
            .on_action(cx.listener(Self::ungroup))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::cancel))
            .on_action(
                cx.listener(|this, _: &NudgeLeft, window, cx| this.nudge(-1., 0., window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeRight, window, cx| this.nudge(1., 0., window, cx)),
            )
            .on_action(cx.listener(|this, _: &NudgeUp, window, cx| this.nudge(0., -1., window, cx)))
            .on_action(
                cx.listener(|this, _: &NudgeDown, window, cx| this.nudge(0., 1., window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeLeftFar, window, cx| this.nudge(-10., 0., window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeRightFar, window, cx| this.nudge(10., 0., window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeUpFar, window, cx| this.nudge(0., -10., window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeDownFar, window, cx| this.nudge(0., 10., window, cx)),
            )
            .on_action(cx.listener(Self::edit_text))
            .on_action(
                cx.listener(|this, _: &NewSlide, window, cx| this.new_slide(None, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Present, window, cx| {
                let current = this.current;
                this.present_from(current, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PresentFromStart, window, cx| {
                this.present_from(0, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &BringForward, window, cx| this.order("forward", window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SendBackward, window, cx| {
                    this.order("backward", window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &BringToFront, window, cx| this.order("front", window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SendToBack, window, cx| this.order("back", window, cx)),
            )
            .on_action(cx.listener(|this, _: &PreviousSlide, window, cx| {
                let previous = this.current.saturating_sub(1);
                this.go_to(previous, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextSlide, window, cx| {
                let next = this.current + 1;
                this.go_to(next, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .bg(theme.background)
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(rail)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(div().flex_1().min_h_0().flex().child(canvas))
                            .child(bar)
                            .children(notes),
                    )
                    .children(side),
            )
    }
}
