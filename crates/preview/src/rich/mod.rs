//! Markdown edited as formatted text, a block at a time, like Notion: every
//! block shows formatted until clicked; the block being edited shows its
//! text, with its look (a heading stays large, code stays code).
//!
//! - Enter starts a new block (a new item in a list), Shift+Enter a new line.
//! - Backspace at the start turns a heading, list item or quote back into
//!   text, or joins a paragraph to the one above.
//! - Typing `# `, `- `, `1. `, `[] `, `> ` or ```` ``` ```` at the start
//!   makes that kind of block; `/` opens a menu of kinds.
//! - Tab and Shift+Tab move list items in and out; tick boxes tick with a
//!   click; blocks move by their handle; an image pasted is saved beside the
//!   document and shown.
//! - ⌘B, ⌘I, ⌘E and ⌘K wrap the selection in bold, italic, code or a link.
//!
//! The file's Markdown stays the truth (see [`blocks`]): a block not edited
//! keeps its exact text, so editing here changes only what was edited.

pub mod blocks;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardEntry, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    Subscription, Task, Window, div, px,
};

use crate::PreviewEvent;
use crate::markdown::{READING_WIDTH, markdown_view};
use blocks::{Block, Document, Kind};

/// How long after the last edit the file's text is updated.
const SAVE_DELAY: Duration = Duration::from_millis(250);
const UNDO_DEPTH: usize = 100;
/// The left margin with each block's handle.
const GUTTER: f32 = 44.;

/// The block being edited.
struct Active {
    id: u64,
    input: Entity<TextareaState>,
    /// The text when editing began (an unchanged block undoes as a whole).
    started: String,
    _subscription: Subscription,
}

/// A kind of block the slash menu and toolbar make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Make {
    Text,
    Heading(u8),
    Bullet,
    Numbered,
    Todo,
    Quote,
    Code,
    Mermaid,
    Table,
    Divider,
}

impl Make {
    const ALL: [Make; 12] = [
        Make::Text,
        Make::Heading(1),
        Make::Heading(2),
        Make::Heading(3),
        Make::Bullet,
        Make::Numbered,
        Make::Todo,
        Make::Quote,
        Make::Code,
        Make::Mermaid,
        Make::Table,
        Make::Divider,
    ];

    fn label(self) -> &'static str {
        match self {
            Make::Text => "Text",
            Make::Heading(1) => "Heading 1",
            Make::Heading(2) => "Heading 2",
            Make::Heading(_) => "Heading 3",
            Make::Bullet => "Bulleted list",
            Make::Numbered => "Numbered list",
            Make::Todo => "To-do list",
            Make::Quote => "Quote",
            Make::Code => "Code",
            Make::Mermaid => "Mermaid diagram",
            Make::Table => "Table",
            Make::Divider => "Divider",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Make::Text => "Plain text",
            Make::Heading(1) => "#",
            Make::Heading(2) => "##",
            Make::Heading(_) => "###",
            Make::Bullet => "-",
            Make::Numbered => "1.",
            Make::Todo => "[]",
            Make::Quote => ">",
            Make::Code => "```",
            Make::Mermaid => "flowchart, sequence…",
            Make::Table => "Rows and columns",
            Make::Divider => "---",
        }
    }

    /// The block's kind, and its starting text when it has one.
    fn kind(self, indent: &str) -> (Kind, Option<&'static str>) {
        let indent = indent.to_string();
        match self {
            Make::Text => (Kind::Paragraph, None),
            Make::Heading(level) => (Kind::Heading(level), None),
            Make::Bullet => (
                Kind::Bullet {
                    indent,
                    marker: '-',
                },
                None,
            ),
            Make::Numbered => (
                Kind::Numbered {
                    indent,
                    number: 1,
                    delimiter: '.',
                },
                None,
            ),
            Make::Todo => (
                Kind::Todo {
                    indent,
                    marker: '-',
                    checked: false,
                },
                None,
            ),
            Make::Quote => (Kind::Quote, None),
            Make::Code => (
                Kind::Code {
                    fence: "```".into(),
                    lang: String::new(),
                },
                None,
            ),
            Make::Mermaid => (
                Kind::Code {
                    fence: "```".into(),
                    lang: "mermaid".into(),
                },
                Some("flowchart LR\n  A[Start] --> B[Finish]"),
            ),
            Make::Table => (
                Kind::Raw,
                Some("| Column | Column |\n| ------ | ------ |\n|        |        |"),
            ),
            Make::Divider => (Kind::Divider, None),
        }
    }
}

/// A block being dragged by its handle.
#[derive(Clone)]
struct DraggedBlock {
    id: u64,
    label: SharedString,
}

struct DragGhost(SharedString);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .max_w(px(320.))
            .truncate()
            .rounded(cx.theme().radius)
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_md()
            .text_sm()
            .child(self.0.clone())
    }
}

pub struct RichEditor {
    path: PathBuf,
    document: Document,
    /// The text last read or written, to tell our own edits coming back.
    text: String,
    active: Option<Active>,
    /// The slash menu: its highlighted row.
    menu: Option<usize>,
    undo: Vec<String>,
    redo: Vec<String>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _save: Option<Task<()>>,
}

impl EventEmitter<PreviewEvent> for RichEditor {}

impl Focusable for RichEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RichEditor {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            path,
            document: Document::default(),
            text: String::new(),
            active: None,
            menu: None,
            undo: Vec::new(),
            redo: Vec::new(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _save: None,
        }
    }

    /// The file's text changed (loaded, or edited in the source view).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text == self.text {
            return;
        }
        self.text = text.to_string();
        self.document = Document::parse(text);
        self.active = None;
        self.menu = None;
        cx.notify();
    }

    /// The document as it is now (for saving before the timer).
    pub fn text(&mut self, cx: &mut Context<Self>) -> String {
        self.take_input(cx);
        self.document.text()
    }

    // ---- Editing ---------------------------------------------------------------

    /// Copies the input's text into its block.
    fn take_input(&mut self, cx: &App) {
        let Some(active) = &self.active else {
            return;
        };
        let value = active.input.read(cx).value().to_string();
        if let Some(ix) = self.document.index_of(active.id) {
            self.document.blocks[ix].set_body(value);
        }
    }

    /// The document changed: the file follows shortly.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self._save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            _ = this.update(cx, |this, cx| this.write(cx));
        }));
        cx.notify();
    }

    fn write(&mut self, cx: &mut Context<Self>) {
        self.take_input(cx);
        let text = self.document.text();
        if text != self.text {
            self.text = text.clone();
            cx.emit(PreviewEvent::ReplaceText(text));
        }
    }

    /// Keeps the document as it is, before a change to its blocks.
    fn checkpoint(&mut self, cx: &mut Context<Self>) {
        self.take_input(cx);
        let text = self.document.text();
        if self.undo.last() != Some(&text) {
            self.undo.push(text);
            if self.undo.len() > UNDO_DEPTH {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    fn undo(&mut self, redo: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.take_input(cx);
        let current = self.document.text();
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let Some(text) = from.pop() else {
            return;
        };
        to.push(current);
        self.document = Document::parse(&text);
        self.active = None;
        self.menu = None;
        window.focus(&self.focus, cx);
        self.changed(cx);
    }

    /// Starts editing block `id`, the cursor at `cursor` (bytes; the end
    /// when `None`).
    fn activate(
        &mut self,
        id: u64,
        cursor: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active.as_ref().is_some_and(|active| active.id == id) {
            return;
        }
        self.take_input(cx);
        self.menu = None;
        let Some(ix) = self.document.index_of(id) else {
            return;
        };
        let block = &self.document.blocks[ix];
        if block.kind == Kind::Divider {
            self.active = None;
            cx.notify();
            return;
        }
        let body = block.body.clone();
        let literal = block.kind.is_literal();
        let placeholder = match &block.kind {
            Kind::Paragraph => "Type / for blocks, or just write",
            Kind::Heading(_) => "Heading",
            Kind::Code { .. } => "Code",
            Kind::Quote => "Quote",
            _ if block.kind.is_list() => "List item",
            _ => "",
        };
        let input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx)
                .auto_grow(1, 400)
                .submit_on_enter(!literal)
                .placeholder(placeholder);
            input.set_value(body.clone(), window, cx);
            let at = cursor.unwrap_or(body.len()).min(body.len());
            input.set_selected_range(at..at, cx);
            input
        });
        let subscription = cx.subscribe_in(&input, window, Self::on_input);
        // Focused now and again once drawn: a block made from another's
        // key press would otherwise lose the focus to the block that ended.
        input.update(cx, |input, cx| input.focus(window, cx));
        let later = input.downgrade();
        window.defer(cx, move |window, cx| {
            _ = later.update(cx, |input, cx| input.focus(window, cx));
        });
        self.active = Some(Active {
            id,
            input,
            started: body,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn deactivate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.take_input(cx);
        self.active = None;
        self.menu = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn active_ix(&self) -> Option<usize> {
        self.active
            .as_ref()
            .and_then(|active| self.document.index_of(active.id))
    }

    fn on_input(
        &mut self,
        input: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                let Some(ix) = self.active_ix() else {
                    return;
                };
                // Markdown typed at the start of plain text makes its block.
                if self.document.blocks[ix].kind == Kind::Paragraph
                    && let Some((kind, rest)) = blocks::shortcut(&value)
                {
                    self.checkpoint(cx);
                    let block = &mut self.document.blocks[ix];
                    block.set_kind(kind.clone());
                    block.set_body(rest);
                    let id = block.id;
                    self.active = None;
                    if kind == Kind::Divider {
                        let next = self
                            .document
                            .insert_after(Some(ix), Block::new(Kind::Paragraph, ""));
                        let next = self.document.blocks[next].id;
                        self.activate(next, None, window, cx);
                    } else {
                        self.activate(id, Some(0), window, cx);
                    }
                    self.changed(cx);
                    return;
                }
                self.menu = (self.document.blocks[ix].kind == Kind::Paragraph
                    && value.starts_with('/')
                    && !value.contains('\n'))
                .then(|| self.menu.unwrap_or(0));
                self.document.blocks[ix].set_body(value);
                self.changed(cx);
            }
            InputEvent::PressEnter {
                shift: false,
                secondary,
            } => {
                // After the key's own handling: the block that takes the
                // focus must not be made while the old one still has it.
                let secondary = *secondary;
                cx.defer_in(window, move |this, window, cx| {
                    if this.menu.is_some() {
                        this.pick_from_menu(window, cx);
                    } else if secondary {
                        this.toggle_active_todo(cx);
                    } else {
                        this.split(window, cx);
                    }
                });
            }
            _ => {}
        }
    }

    /// Enter: the text after the cursor goes to a new block below (the next
    /// item, in a list); an empty list item leaves the list.
    fn split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ix), Some(active)) = (self.active_ix(), &self.active) else {
            return;
        };
        let input = active.input.read(cx);
        let value = input.value().to_string();
        let cursor = input.selected_range().start.min(value.len());
        let kind = self.document.blocks[ix].kind.clone();
        self.checkpoint(cx);
        if kind.is_list() && value.trim().is_empty() {
            self.document.blocks[ix].set_kind(Kind::Paragraph);
            let id = self.document.blocks[ix].id;
            self.active = None;
            self.activate(id, Some(0), window, cx);
            self.changed(cx);
            return;
        }
        let (before, after) = value.split_at(cursor);
        self.document.blocks[ix].set_body(before.to_string());
        let next_kind = match &kind {
            Kind::Todo { indent, marker, .. } => Kind::Todo {
                indent: indent.clone(),
                marker: *marker,
                checked: false,
            },
            Kind::Bullet { .. } | Kind::Numbered { .. } => kind.clone(),
            _ => Kind::Paragraph,
        };
        let at = self
            .document
            .insert_after(Some(ix), Block::new(next_kind, after.to_string()));
        self.document.renumber(at);
        let id = self.document.blocks[at].id;
        self.active = None;
        self.activate(id, Some(0), window, cx);
        self.changed(cx);
    }

    /// Backspace at the very start of a block.
    fn backspace_at_start(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.active_ix() else {
            return false;
        };
        let kind = self.document.blocks[ix].kind.clone();
        self.take_input(cx);
        if matches!(kind, Kind::Heading(_) | Kind::Quote) || kind.is_list() {
            self.checkpoint(cx);
            self.document.blocks[ix].set_kind(Kind::Paragraph);
            let id = self.document.blocks[ix].id;
            self.active = None;
            self.activate(id, Some(0), window, cx);
            self.changed(cx);
            return true;
        }
        if kind != Kind::Paragraph || ix == 0 {
            return false;
        }
        let body = self.document.blocks[ix].body.clone();
        let previous = &self.document.blocks[ix - 1];
        let joins = matches!(
            previous.kind,
            Kind::Paragraph | Kind::Heading(_) | Kind::Quote
        ) || previous.kind.is_list();
        if !joins && !body.is_empty() {
            return false;
        }
        self.checkpoint(cx);
        self.document.remove(ix);
        let previous = &mut self.document.blocks[ix - 1];
        let (id, at) = if joins {
            let at = previous.body.len();
            let joined = format!("{}{body}", previous.body);
            previous.set_body(joined);
            (previous.id, Some(at))
        } else {
            (previous.id, None)
        };
        self.active = None;
        self.activate(id, at, window, cx);
        self.changed(cx);
        true
    }

    fn toggle_active_todo(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.active.as_ref().map(|active| active.id) {
            self.toggle_todo(id, cx);
        }
    }

    fn toggle_todo(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.document.index_of(id) else {
            return;
        };
        self.checkpoint(cx);
        let block = &mut self.document.blocks[ix];
        if let Kind::Todo { checked, .. } = &block.kind {
            let mut kind = block.kind.clone();
            if let Kind::Todo { checked: c, .. } = &mut kind {
                *c = !*checked;
            }
            block.set_kind(kind);
        }
        self.changed(cx);
    }

    /// Tab and Shift+Tab: a list item in or out a level.
    fn indent(&mut self, out: bool, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.active_ix() else {
            return false;
        };
        let block = &self.document.blocks[ix];
        let step = match &block.kind {
            Kind::Numbered { .. } => 3,
            Kind::Bullet { .. } | Kind::Todo { .. } => 2,
            _ => return false,
        };
        self.checkpoint(cx);
        let block = &mut self.document.blocks[ix];
        let mut kind = block.kind.clone();
        let indent = match &mut kind {
            Kind::Bullet { indent, .. }
            | Kind::Numbered { indent, .. }
            | Kind::Todo { indent, .. } => indent,
            _ => return false,
        };
        if out {
            let remove = indent.len().min(step);
            indent.drain(..remove);
        } else {
            indent.push_str(&" ".repeat(step));
        }
        block.set_kind(kind);
        self.document.renumber(ix);
        self.changed(cx);
        true
    }

    /// Makes block `ix` (or a new block after it, for a divider) a `make`.
    fn make(&mut self, make: Make, window: &mut Window, cx: &mut Context<Self>) {
        self.take_input(cx);
        let ix = match self.active_ix() {
            Some(ix) => ix,
            None => self.document.insert_after(
                self.document.blocks.len().checked_sub(1),
                Block::new(Kind::Paragraph, ""),
            ),
        };
        self.checkpoint(cx);
        let indent = self.document.blocks[ix].kind.indent().to_string();
        let (kind, template) = make.kind(&indent);
        let block = &mut self.document.blocks[ix];
        // A slash command's text goes; other text stays.
        if block.body.starts_with('/') && !block.body.contains('\n') {
            block.set_body(String::new());
        }
        if let Some(template) = template
            && block.body.trim().is_empty()
        {
            block.set_body(template.to_string());
        }
        block.set_kind(kind.clone());
        let id = block.id;
        self.active = None;
        self.menu = None;
        if kind == Kind::Divider {
            let next = self
                .document
                .insert_after(Some(ix), Block::new(Kind::Paragraph, ""));
            let next = self.document.blocks[next].id;
            self.activate(next, None, window, cx);
        } else {
            self.document.renumber(ix);
            self.activate(id, None, window, cx);
        }
        self.changed(cx);
    }

    fn menu_items(&self, cx: &App) -> Vec<Make> {
        let query = self
            .active
            .as_ref()
            .map(|active| active.input.read(cx).value().to_lowercase())
            .unwrap_or_default();
        let query = query.trim_start_matches('/').trim();
        Make::ALL
            .into_iter()
            .filter(|make| query.is_empty() || make.label().to_lowercase().contains(query))
            .collect()
    }

    fn pick_from_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.menu_items(cx);
        match items.get(self.menu.unwrap_or(0)).copied() {
            Some(make) => self.make(make, window, cx),
            None => {
                self.menu = None;
                cx.notify();
            }
        }
    }

    /// Wraps the selection in `open` and `close` (or puts the pair at the
    /// cursor).
    fn wrap(&mut self, open: &str, close: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = &self.active else {
            return;
        };
        let input = active.input.clone();
        input.update(cx, |input, cx| {
            let value = input.value().to_string();
            let range = input.selected_range();
            let (start, end) = (range.start.min(value.len()), range.end.min(value.len()));
            let text = format!(
                "{}{open}{}{close}{}",
                &value[..start],
                &value[start..end],
                &value[end..]
            );
            input.set_value(text, window, cx);
            let from = start + open.len();
            input.set_selected_range(from..from + (end - start), cx);
        });
    }

    /// Up at the start of a block goes to the one above (Down at the end,
    /// below); a divider is passed over.
    fn step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.active_ix() else {
            return false;
        };
        let mut target = ix;
        loop {
            target = match (down, target) {
                (true, target) if target + 1 < self.document.blocks.len() => target + 1,
                (false, target) if target > 0 => target - 1,
                _ => return false,
            };
            if self.document.blocks[target].kind != Kind::Divider {
                break;
            }
        }
        let id = self.document.blocks[target].id;
        let cursor = if down { Some(0) } else { None };
        self.activate(id, cursor, window, cx);
        true
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let key = keystroke.key.as_str();
        if modifiers.secondary() && !modifiers.alt {
            let handled = match (key, modifiers.shift) {
                ("z", shift) => {
                    // Typing in the block undoes itself first.
                    let typed = self.active.as_ref().is_some_and(|active| {
                        active.input.read(cx).value().as_ref() != active.started
                    });
                    if typed && !shift {
                        false
                    } else {
                        self.undo(shift, window, cx);
                        true
                    }
                }
                ("b", false) => {
                    self.wrap("**", "**", window, cx);
                    true
                }
                ("i", false) => {
                    self.wrap("_", "_", window, cx);
                    true
                }
                ("e", false) => {
                    self.wrap("`", "`", window, cx);
                    true
                }
                ("k", false) => {
                    self.wrap("[", "](https://)", window, cx);
                    true
                }
                ("v", false) => self.paste_image(window, cx),
                _ => false,
            };
            if handled {
                cx.stop_propagation();
            }
            return;
        }
        let Some(active) = &self.active else {
            return;
        };
        let input = active.input.read(cx);
        let value = input.value();
        let range = input.selected_range();
        let at_start = range.start == 0 && range.end == 0;
        let at_end = range.start >= value.len();
        let one_line = !value.contains('\n') && value.chars().count() < 60;
        let handled = match key {
            "escape" if self.menu.is_some() => {
                self.menu = None;
                cx.notify();
                true
            }
            "escape" => {
                self.deactivate(window, cx);
                true
            }
            "up" if self.menu.is_some() => {
                let count = self.menu_items(cx).len().max(1);
                self.menu = self.menu.map(|ix| (ix + count - 1) % count);
                cx.notify();
                true
            }
            "down" if self.menu.is_some() => {
                let count = self.menu_items(cx).len().max(1);
                self.menu = self.menu.map(|ix| (ix + 1) % count);
                cx.notify();
                true
            }
            "up" if at_start || (one_line && !modifiers.shift) => self.step(false, window, cx),
            "down" if at_end || (one_line && !modifiers.shift) => self.step(true, window, cx),
            "backspace" if at_start => self.backspace_at_start(window, cx),
            "tab" => self.indent(modifiers.shift, cx),
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    /// ⌘V with an image: saves it beside the document (`images/`) and puts
    /// it in a block below. Text pastes as usual.
    fn paste_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        // Text copied with a picture of itself pastes as text.
        if clipboard
            .entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::String(_)))
        {
            return false;
        }
        let Some(image) = clipboard.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image.clone()),
            _ => None,
        }) else {
            return false;
        };
        let extension = match image.format {
            gpui_kit::ImageFormat::Jpeg => "jpg",
            gpui_kit::ImageFormat::Gif => "gif",
            gpui_kit::ImageFormat::Webp => "webp",
            gpui_kit::ImageFormat::Svg => "svg",
            _ => "png",
        };
        let dir = self.path.parent().unwrap_or(Path::new(".")).join("images");
        let name = format!(
            "pasted-{}.{extension}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_millis())
                .unwrap_or_default()
        );
        // A new file only, never over one already there.
        let saved = std::fs::create_dir_all(&dir).and_then(|_| {
            use std::io::Write as _;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(&name))?
                .write_all(&image.bytes)
        });
        if let Err(err) = saved {
            tracing::warn!("saving a pasted image failed: {err}");
            return false;
        }
        self.take_input(cx);
        self.checkpoint(cx);
        let ix = self
            .active_ix()
            .or(self.document.blocks.len().checked_sub(1));
        let at = self.document.insert_after(
            ix,
            Block::new(Kind::Paragraph, format!("![{name}](images/{name})")),
        );
        let next = self
            .document
            .insert_after(Some(at), Block::new(Kind::Paragraph, ""));
        let id = self.document.blocks[next].id;
        self.active = None;
        self.activate(id, None, window, cx);
        self.changed(cx);
        true
    }

    /// A new empty block after `ix` (at the end for `None`), being edited.
    fn add_after(
        &mut self,
        ix: Option<usize>,
        slash: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.take_input(cx);
        self.checkpoint(cx);
        let ix = ix.or(self.document.blocks.len().checked_sub(1));
        let at = self.document.insert_after(
            ix,
            Block::new(Kind::Paragraph, if slash { "/" } else { "" }),
        );
        let id = self.document.blocks[at].id;
        self.active = None;
        self.activate(id, None, window, cx);
        if slash {
            self.menu = Some(0);
        }
        self.changed(cx);
    }

    fn move_block(&mut self, id: u64, before: u64, cx: &mut Context<Self>) {
        if id == before {
            return;
        }
        let (Some(from), Some(_)) = (self.document.index_of(id), self.document.index_of(before))
        else {
            return;
        };
        self.take_input(cx);
        self.checkpoint(cx);
        let mut block = self.document.remove(from);
        let Some(to) = self.document.index_of(before) else {
            return;
        };
        block.gap = if block.kind.is_list() && self.document.blocks[to].kind.is_list() {
            "\n".into()
        } else {
            "\n\n".into()
        };
        self.document.blocks.insert(to, block);
        if to > 0 && self.document.blocks[to - 1].gap.is_empty() {
            self.document.blocks[to - 1].gap = "\n\n".into();
        }
        self.document.renumber(to);
        self.changed(cx);
    }
}

// ---- Rendering --------------------------------------------------------------

impl RichEditor {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let inline = |id: &'static str,
                      label: &'static str,
                      tip: &'static str,
                      open: &'static str,
                      close: &'static str,
                      cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .xsmall()
                .label(label)
                .tooltip(tip)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.wrap(open, close, window, cx)),
                )
        };
        let block = |id: &'static str, label: &'static str, make: Make, cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .xsmall()
                .label(label)
                .tooltip(make.label())
                .on_click(cx.listener(move |this, _, window, cx| this.make(make, window, cx)))
        };
        let separator = || div().w_px().h_4().mx_1().bg(border);
        h_flex()
            .flex_none()
            .px_3()
            .py_1()
            .gap_0p5()
            .flex_wrap()
            .border_b_1()
            .border_color(border)
            .child(
                inline("rich-bold", "B", "Bold (⌘B)", "**", "**", cx)
                    .font_weight(gpui_kit::FontWeight::BOLD),
            )
            .child(inline("rich-italic", "I", "Italic (⌘I)", "_", "_", cx).italic())
            .child(inline("rich-strike", "S", "Strikethrough", "~~", "~~", cx).line_through())
            .child(inline("rich-code", "</>", "Code (⌘E)", "`", "`", cx))
            .child(inline(
                "rich-link",
                "Link",
                "Link (⌘K)",
                "[",
                "](https://)",
                cx,
            ))
            .child(separator())
            .child(block("rich-text", "Text", Make::Text, cx))
            .child(block("rich-h1", "H1", Make::Heading(1), cx))
            .child(block("rich-h2", "H2", Make::Heading(2), cx))
            .child(block("rich-h3", "H3", Make::Heading(3), cx))
            .child(separator())
            .child(block("rich-bullet", "• List", Make::Bullet, cx))
            .child(block("rich-numbered", "1. List", Make::Numbered, cx))
            .child(block("rich-todo", "☐ To-do", Make::Todo, cx))
            .child(block("rich-quote", "Quote", Make::Quote, cx))
            .child(block("rich-code-block", "Code", Make::Code, cx))
            .child(separator())
            .child(block("rich-table", "Table", Make::Table, cx))
            .child(block("rich-mermaid", "Diagram", Make::Mermaid, cx))
            .child(block("rich-divider", "Divider", Make::Divider, cx))
            .into_any_element()
    }

    fn render_block(
        &self,
        ix: usize,
        block: &Block,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = block.id;
        let dark = cx.theme().is_dark();
        let indent = block.kind.indent().chars().count() as f32 * 9.;
        let content: AnyElement = match self.active.as_ref().filter(|active| active.id == id) {
            Some(active) => self.render_input(block, active, cx),
            None => self.render_shown(block, dark, window, cx),
        };
        let theme = cx.theme();
        let marker: Option<AnyElement> = match &block.kind {
            Kind::Bullet { .. } => Some(
                div()
                    .w_5()
                    .flex_none()
                    .pt(px(1.))
                    .text_color(theme.muted_foreground)
                    .child("•")
                    .into_any_element(),
            ),
            Kind::Numbered {
                number, delimiter, ..
            } => Some(
                div()
                    .min_w_5()
                    .flex_none()
                    .pr_1()
                    .text_color(theme.muted_foreground)
                    .child(format!("{number}{delimiter}"))
                    .into_any_element(),
            ),
            Kind::Todo { checked, .. } => {
                let checked = *checked;
                Some(
                    div()
                        .id(("rich-check", id))
                        .flex_none()
                        .mt(px(3.))
                        .mr_2()
                        .size(px(16.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(if checked {
                            theme.info
                        } else {
                            theme.muted_foreground
                        })
                        .when(checked, |this| this.bg(theme.info))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(checked, |this| {
                            this.child(
                                Icon::new(IconName::Check)
                                    .xsmall()
                                    .text_color(theme.button_info_foreground),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_todo(id, cx);
                        }))
                        .into_any_element(),
                )
            }
            _ => None,
        };
        let label: SharedString = block
            .body
            .lines()
            .next()
            .unwrap_or("Block")
            .to_string()
            .into();
        let row = h_flex()
            .id(("rich-block", id))
            .group("rich-block")
            .items_start()
            .w_full()
            .pl(px(indent))
            .child(
                h_flex()
                    .w(px(GUTTER))
                    .flex_none()
                    .justify_end()
                    .gap_0p5()
                    .pr_1()
                    .invisible()
                    .group_hover("rich-block", |this| this.visible())
                    .child(
                        Button::new(("rich-add", id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("Add a block below")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let ix = this.document.index_of(id);
                                this.add_after(ix, true, window, cx)
                            })),
                    )
                    .child(
                        div()
                            .id(("rich-handle", id))
                            .px_0p5()
                            .py_1()
                            .rounded(theme.radius)
                            .cursor_grab()
                            .hover(|this| this.bg(theme.list_hover))
                            .child(
                                Icon::new(ph_ui::Icons::GripVertical)
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                            )
                            .on_drag(DraggedBlock { id, label }, |dragged, _, _, cx| {
                                cx.new(|_| DragGhost(dragged.label.clone()))
                            }),
                    ),
            )
            .children(marker)
            .child(div().flex_1().min_w_0().child(content))
            .drag_over::<DraggedBlock>(move |style, _, _, cx| {
                style.border_t_2().border_color(cx.theme().drag_border)
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedBlock, _, cx| {
                this.move_block(dragged.id, id, cx)
            }));
        // Space above a block, by what it is.
        let space = match (&block.kind, ix) {
            (_, 0) => 0.,
            (Kind::Heading(1), _) => 18.,
            (Kind::Heading(_), _) => 12.,
            (kind, _) if kind.is_list() => 2.,
            _ => 6.,
        };
        div().pt(px(space)).child(row).into_any_element()
    }

    /// A block not being edited, formatted.
    fn render_shown(
        &self,
        block: &Block,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = block.id;
        let shown: AnyElement = match &block.kind {
            Kind::Divider => div()
                .py_3()
                .child(div().h_px().w_full().bg(theme.border))
                .into_any_element(),
            Kind::Code { lang, .. } if lang == "mermaid" => {
                match crate::mermaid::draw(&block.body, dark) {
                    Ok(diagram) => div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .py_2()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(diagram.width.min(READING_WIDTH - 64.)))
                                .aspect_ratio(diagram.width / diagram.height)
                                .child(
                                    gpui_kit::img(diagram.image.clone())
                                        .size_full()
                                        .object_fit(gpui_kit::ObjectFit::Contain),
                                ),
                        )
                        .into_any_element(),
                    Err(err) => div()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(format!("Mermaid: {err}"))
                        .into_any_element(),
                }
            }
            kind if kind.is_list() && block.body.trim().is_empty() => {
                div().h(px(22.)).into_any_element()
            }
            Kind::Paragraph if block.body.trim().is_empty() => div().h(px(22.)).into_any_element(),
            kind => {
                let source: SharedString = if kind.is_list() {
                    block.body.clone().into()
                } else {
                    block.source().into()
                };
                let muted = matches!(kind, Kind::Todo { checked: true, .. });
                let view = self.view_for(id, source, dark, cx);
                div()
                    .when(muted, |this| this.opacity(0.55).line_through())
                    .child(view)
                    .into_any_element()
            }
        };
        let _ = window;
        div()
            .id(("rich-shown", id))
            .w_full()
            .min_h(px(22.))
            .cursor_text()
            .child(shown)
            .on_click(cx.listener(move |this, _, window, cx| this.activate(id, None, window, cx)))
            .into_any_element()
    }

    fn view_for(
        &self,
        id: u64,
        source: SharedString,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        markdown_view(
            ("rich-text", id),
            source,
            &self.path,
            dark,
            crate::markdown::opener(cx.weak_entity()),
        )
        .into_any_element()
    }

    /// The block being edited: its text, looking like the block.
    fn render_input(&self, block: &Block, active: &Active, cx: &mut Context<Self>) -> AnyElement {
        let menu = self.menu.map(|highlight| self.render_menu(highlight, cx));
        let theme = cx.theme();
        let input = Textarea::new(&active.input)
            .appearance(false)
            .bordered(false);
        let editor = div().w_full().child(input);
        let editor = match &block.kind {
            Kind::Heading(level) => {
                editor
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_size(px(match level {
                        1 => 28.,
                        2 => 22.,
                        3 => 18.,
                        _ => 16.,
                    }))
            }
            Kind::Quote => editor
                .pl_3()
                .border_l_2()
                .border_color(theme.border)
                .text_color(theme.muted_foreground),
            Kind::Code { .. } | Kind::Raw => editor
                .px_3()
                .py_2()
                .rounded(theme.radius)
                .bg(theme.secondary)
                .font_family(theme.mono_font_family.clone())
                .text_sm(),
            _ => editor,
        };
        let lang = match &block.kind {
            Kind::Code { lang, .. } if !lang.is_empty() => Some(lang.clone()),
            _ => None,
        };
        let live = match &block.kind {
            Kind::Code { lang, .. } if lang == "mermaid" => {
                crate::mermaid::draw(&active.input.read(cx).value(), theme.is_dark()).ok()
            }
            _ => None,
        };
        v_flex()
            .w_full()
            .relative()
            .rounded(theme.radius)
            .bg(theme.list_hover.opacity(0.35))
            .children(lang.map(|lang| {
                div()
                    .px_3()
                    .pt_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(lang)
            }))
            .child(editor)
            .children(live.map(|diagram| {
                div().w_full().flex().justify_center().py_2().child(
                    div()
                        .w_full()
                        .max_w(px(diagram.width.min(READING_WIDTH - 64.)))
                        .aspect_ratio(diagram.width / diagram.height)
                        .child(
                            gpui_kit::img(diagram.image.clone())
                                .size_full()
                                .object_fit(gpui_kit::ObjectFit::Contain),
                        ),
                )
            }))
            .children(menu)
            .into_any_element()
    }

    fn render_menu(&self, highlight: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let items = self.menu_items(cx);
        let rows: Vec<AnyElement> = if items.is_empty() {
            vec![
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No such block")
                    .into_any_element(),
            ]
        } else {
            items
                .iter()
                .enumerate()
                .map(|(ix, make)| {
                    let make = *make;
                    h_flex()
                        .id(("rich-menu", ix))
                        .px_3()
                        .py_1()
                        .gap_3()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .when(ix == highlight, |this| this.bg(theme.list_active))
                        .hover(|this| this.bg(theme.list_hover))
                        .child(div().flex_1().text_sm().child(make.label()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(make.hint()),
                        )
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.make(make, window, cx)),
                        )
                        .into_any_element()
                })
                .collect()
        };
        let list = div()
            .w(px(280.))
            .p_1()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .occlude()
            .children(rows);
        // Drawn after the blocks below it, so it stays on top.
        div()
            .absolute()
            .top_full()
            .left_0()
            .mt_1()
            .child(gpui_kit::deferred(list).with_priority(1))
            .into_any_element()
    }
}

impl Render for RichEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let blocks: Vec<AnyElement> = self
            .document
            .blocks
            .clone()
            .iter()
            .enumerate()
            .map(|(ix, block)| self.render_block(ix, block, window, cx))
            .collect();
        let toolbar = self.render_toolbar(cx);
        let theme = cx.theme();
        let empty = self.document.blocks.is_empty();
        v_flex()
            .id("rich-editor")
            .key_context("RichEditor")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_key))
            .size_full()
            .bg(theme.background)
            .child(toolbar)
            .child(
                div()
                    .id("rich-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(READING_WIDTH + GUTTER))
                            .mx_auto()
                            .pr_8()
                            .pt_6()
                            .pb(px(160.))
                            .children(blocks)
                            .child(
                                // The space under the last block adds one.
                                div()
                                    .id("rich-end")
                                    .ml(px(GUTTER))
                                    .h(px(120.))
                                    .cursor_text()
                                    .when(empty, |this| {
                                        this.text_color(theme.muted_foreground)
                                            .child("Click to start writing. Type / for headings, lists, code, tables and diagrams.")
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let last = this.document.blocks.last();
                                        let reuse = last.filter(|block| block.kind == Kind::Paragraph && block.body.is_empty()).map(|block| block.id);
                                        match reuse {
                                            Some(id) => this.activate(id, None, window, cx),
                                            None => this.add_after(None, false, window, cx),
                                        }
                                    })),
                            ),
                    ),
            )
    }
}
