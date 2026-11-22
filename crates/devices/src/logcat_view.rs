//! The log itself: lines in columns (time, level, tag, message) that read
//! like text in an editor, after Android Studio's Logcat. Text is selected
//! by dragging over it, by double and triple clicks and with Shift, and
//! copied with the usual shortcut; a right click offers the line's own
//! commands. Long messages wrap (or not, as chosen), a stack frame opens its
//! file, and the list follows new lines until one scrolls back.
//!
//! Above it: the lowest level shown, the app (the open project's, one
//! installed, or all), the filter box (see [`crate::logcat`] for its
//! words; the two pickers write theirs into it) and the list's switches.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, KeyDownEvent, ListAlignment,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, Render, ScrollHandle, StatefulInteractiveElement as _, Styled as _, StyledText,
    Subscription, TextLayout, UnderlineStyle, WeakEntity, Window, div, list, px,
};
use ph_media::ToChat;
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::app_picker::{AppPicked, AppPicker};
use crate::logcat::{LogLine, Logcat, MINE, Priority, Query, Shown};

/// Lines sent to chat at most when none are selected (the newest shown).
const SEND_LINES: usize = 80;
/// Selected lines sent to chat at most.
const SELECTED_LINES: usize = 500;
/// The height of a line of text.
const ROW_HEIGHT: f32 = 20.0;
/// Rows drawn beyond the view, so scrolling shows no blank ones.
const OVERDRAW: f32 = 400.0;
/// How far above its end the list still counts as at its end.
const AT_END: f32 = 4.0;
/// What a row has beside its message, at the usual interface size: the
/// three columns, the gaps between the four and the row's padding.
const ROW_FIXED: f32 = TIME_WIDTH + LEVEL_WIDTH + TAG_WIDTH + 3. * 8. + 2. * 8.;
/// The longest message the unwrapped list makes room for, in characters:
/// one longer than this is read wrapped, not scrolled along.
const WIDEST: usize = 4000;
/// The widths of the time, level and tag columns.
const TIME_WIDTH: f32 = 86.0;
const LEVEL_WIDTH: f32 = 10.0;
const TAG_WIDTH: f32 = 150.0;

/// A place in the project's source that a stack frame names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    /// The file's name, without its folder: `Main.kt`.
    pub file: String,
    pub line: u32,
    /// The class the frame is in (`com.example.Main`), which tells files of
    /// one name apart.
    pub class: Option<String>,
}

pub enum LogcatViewEvent {
    /// A stack frame was clicked: show its file at its line.
    OpenSource(SourceRef),
    /// Long messages wrap, or not: the app keeps the choice.
    WrapChanged(bool),
}

/// A place in the lines shown: a line, by its number among all the lines
/// its list has shown (so it stays the same line as old ones go), and a
/// byte offset into the line's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Place {
    row: u64,
    at: usize,
}

/// Where a selection started and where it reaches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Selection {
    anchor: Place,
    head: Place,
}

impl Selection {
    /// Its two ends, the earlier first.
    fn ends(self) -> (Place, Place) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    /// The part of line `row` (of `len` bytes) that is selected.
    fn in_row(self, row: u64, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.ends();
        if self.is_empty() || row < start.row || row > end.row {
            return None;
        }
        let from = if row == start.row {
            start.at.min(len)
        } else {
            0
        };
        let to = if row == end.row { end.at.min(len) } else { len };
        // A line between the ends is selected though it is empty.
        (from < to || (row > start.row && row < end.row)).then_some(from..to)
    }
}

/// A line as the list shows it: one text, `time L tag: message`, and where
/// each column is in it. Selections are places in this text.
struct Cells {
    text: String,
    time: Range<usize>,
    level: Range<usize>,
    tag: Range<usize>,
    message: Range<usize>,
}

impl Cells {
    fn of(line: &LogLine) -> Self {
        // A line in no known shape is all message.
        if line.time.is_empty() {
            return Self {
                text: line.message.clone(),
                time: 0..0,
                level: 0..0,
                tag: 0..0,
                message: 0..line.message.len(),
            };
        }
        let mut text = String::with_capacity(line.text.len());
        let mut push = |part: &str, after: &str| {
            let start = text.len();
            text.push_str(part);
            let range = start..text.len();
            text.push_str(after);
            range
        };
        let time = push(&line.time, " ");
        let level = push(line.priority.letter(), " ");
        let tag = push(&line.tag, ": ");
        let message = push(&line.message, "");
        Self {
            text,
            time,
            level,
            tag,
            message,
        }
    }
}

/// What a press on a row needs to know of it: where its columns were drawn
/// and what they hold.
struct RowHit {
    row: u64,
    text: Rc<str>,
    /// Each column's layout and its place in `text`, left to right.
    columns: [(TextLayout, Range<usize>); 4],
    /// The stack frame's file in `text`, when the line names one.
    link: Option<(Range<usize>, SourceRef)>,
}

impl RowHit {
    /// The place in the row's text under `position`: in the column there,
    /// else at the start of the next one to the right.
    fn place(&self, position: Point<Pixels>) -> Place {
        let last = self.columns.len() - 1;
        let mut at = self.text.len();
        for (ix, (layout, range)) in self.columns.iter().enumerate() {
            let bounds = layout.bounds();
            if position.x < bounds.left() {
                at = range.start;
                break;
            }
            if position.x <= bounds.right() || ix == last {
                let within = layout
                    .index_for_position(position)
                    .unwrap_or_else(|nearest| nearest);
                at = range.start + within.min(range.len());
                break;
            }
        }
        Place { row: self.row, at }
    }

    fn link_at(&self, place: Place) -> Option<SourceRef> {
        self.link
            .as_ref()
            .filter(|(range, _)| range.contains(&place.at))
            .map(|(_, source)| source.clone())
    }
}

pub struct LogcatView {
    logcat: Option<Entity<Logcat>>,
    device: String,
    /// Picks the app whose lines show.
    app_picker: Entity<AppPicker>,
    app_picker_open: bool,
    /// The open project's application ids (`package:mine`).
    mine: Vec<String>,
    /// The filter was set to the project's apps once: cleared by the user,
    /// it stays cleared.
    defaulted: bool,
    filter: Entity<InputState>,
    /// Long messages wrap onto more lines; unwrapped, the list scrolls
    /// sideways.
    wrap: bool,
    hscroll: ScrollHandle,
    /// The longest message among the lines shown, in bytes: how wide the
    /// unwrapped list is.
    widest: usize,
    /// Keep the newest line in view. The view owns this, not the list: the
    /// list's own following starts again by itself while the view is at
    /// the end, where a line being read or selected must stay put.
    follow: bool,
    list: ListState,
    /// What `list` shows: the list's `epoch`, how many of its lines have
    /// gone from the front, and how many it has.
    synced: (u64, u64, usize),
    selection: Option<Selection>,
    /// The mouse is down, selecting.
    selecting: bool,
    /// Where the mouse went down, until it moves: a click, not a drag.
    pressed: Option<Place>,
    /// The line the context menu is for.
    context: Option<Rc<LogLine>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _logcat_subscription: Option<Subscription>,
}

impl EventEmitter<ToChat> for LogcatView {}
impl EventEmitter<LogcatViewEvent> for LogcatView {}

impl LogcatView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(
                "Filter: words, -word, tag:Name, package:mine, level:warn, is:crash, pid:123",
            )
        });
        let app_picker = cx.new(|cx| AppPicker::new(window, cx));
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW));
        let this = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            let this = this.clone();
            // The list is still borrowed while it reports the scroll.
            cx.defer(move |cx| {
                _ = this.update(cx, |this, cx| this.scrolled(cx));
            });
        });
        Self {
            logcat: None,
            device: String::new(),
            app_picker: app_picker.clone(),
            app_picker_open: false,
            mine: Vec::new(),
            defaulted: false,
            filter: filter.clone(),
            wrap: true,
            hscroll: ScrollHandle::new(),
            widest: 0,
            follow: true,
            list,
            synced: (0, 0, 0),
            selection: None,
            selecting: false,
            pressed: None,
            context: None,
            focus: cx.focus_handle(),
            _subscriptions: vec![
                cx.observe(&filter, |_, _, cx| cx.notify()),
                cx.subscribe_in(
                    &app_picker,
                    window,
                    |this, _, AppPicked(app): &AppPicked, window, cx| {
                        this.app_picker_open = false;
                        this.set_filter_token(
                            &["package:", "app:"],
                            app.as_ref().map(|app| format!("package:{app}")),
                            window,
                            cx,
                        );
                        cx.notify();
                    },
                ),
            ],
            _logcat_subscription: None,
        }
    }

    /// The device's running logcat (`None` when there is no device).
    pub fn set_logcat(
        &mut self,
        logcat: Option<Entity<Logcat>>,
        device: String,
        cx: &mut Context<Self>,
    ) {
        if self.logcat == logcat {
            return;
        }
        self._logcat_subscription = logcat
            .as_ref()
            .map(|logcat| cx.observe(logcat, |_, _, cx| cx.notify()));
        self.logcat = logcat;
        self.device = device;
        self.selection = None;
        self.context = None;
        cx.notify();
    }

    /// The apps installed, for the app picker.
    pub fn set_packages(&mut self, packages: Vec<String>, cx: &mut Context<Self>) {
        self.app_picker
            .update(cx, |picker, cx| picker.set_packages(packages, cx));
    }

    /// Whether long messages wrap, as saved.
    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.hscroll.set_offset(gpui_kit::point(px(0.), px(0.)));
            self.list.remeasure();
            cx.notify();
        }
    }

    /// The open project's application ids. The first time there are some,
    /// an empty filter becomes the project's apps: the log of one's own app
    /// is what one opens Logcat for.
    pub fn set_project_apps(
        &mut self,
        mine: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mine == mine {
            return;
        }
        self.mine = mine;
        let has_project = !self.mine.is_empty();
        self.app_picker
            .update(cx, |picker, cx| picker.set_has_project(has_project, cx));
        if !self.defaulted && !self.mine.is_empty() {
            self.defaulted = true;
            if self.filter.read(cx).value().trim().is_empty() {
                self.filter.update(cx, |input, cx| {
                    input.set_value(format!("package:{MINE}"), window, cx)
                });
            }
        }
        cx.notify();
    }

    /// Recent errors, for `@log`: the open project's apps' when it has some.
    pub fn recent_errors(&self, cx: &App) -> Option<String> {
        let logcat = self.logcat.as_ref()?.read(cx);
        let query = Query::parse(if self.mine.is_empty() {
            "level:error"
        } else {
            "level:error package:mine"
        });
        let mut lines: Vec<&str> = logcat
            .lines
            .iter()
            .rev()
            .filter(|line| query.matches(line, logcat.process_of(line), &self.mine))
            .take(40)
            .map(|line| line.text.as_str())
            .collect();
        if lines.is_empty() {
            return None;
        }
        lines.reverse();
        Some(format!("Logcat errors:\n```\n{}\n```", lines.join("\n")))
    }

    fn query(&self, cx: &App) -> Query {
        Query::parse(&self.filter.read(cx).value())
    }

    /// The lines the filter passes now.
    fn shown(&self, cx: &mut Context<Self>) -> Shown {
        let Some(logcat) = self.logcat.clone() else {
            return Shown {
                epoch: 0,
                dropped: 0,
                lines: Vec::new(),
            };
        };
        let query = self.query(cx);
        logcat.update(cx, |logcat, _| logcat.matching(&query, &self.mine))
    }

    /// Brings the list in step with `shown`: another list starts at its
    /// end; the same one loses lines at the front and gains them at the end.
    fn sync(&mut self, shown: &Shown) {
        let (epoch, dropped, len) = self.synced;
        if shown.epoch != epoch {
            self.list
                .reset_with_uniform_height(shown.lines.len(), px(ROW_HEIGHT));
            self.follow = true;
            self.selection = None;
            self.widest = longest(&shown.lines);
        } else {
            let gone = ((shown.dropped - dropped) as usize).min(len);
            if gone > 0 {
                self.list.splice(0..gone, 0);
            }
            let kept = len - gone;
            let added = shown.lines.len().saturating_sub(kept);
            if added > 0 {
                self.list.splice(kept..kept, added);
                // Lines that go from the front may leave it wider than it
                // need be until the next list: no harm.
                self.widest = self
                    .widest
                    .max(longest(&shown.lines[shown.lines.len() - added..]));
            }
        }
        self.synced = (shown.epoch, shown.dropped, shown.lines.len());
        if self.follow {
            self.list.scroll_to_end();
        }
    }

    /// How wide the unwrapped list is: the columns, and room for the
    /// longest message in the log's own type.
    fn unwrapped_width(&self, window: &Window, cx: &App) -> Pixels {
        let rem = window.rem_size();
        // The rows' type: the mono font at `text_xs`.
        let size = rem * 0.75;
        let text = window.text_system();
        let font = text.resolve_font(&gpui_kit::font(cx.theme().mono_font_family.clone()));
        let advance = text.ch_advance(font, size).unwrap_or(size * 0.6);
        ph_ui::rems_from_px(ROW_FIXED).to_pixels(rem)
            + advance * (self.widest.min(WIDEST) + 2) as f32
    }

    /// The list was scrolled: back from its end stops following new lines,
    /// to its end follows them again.
    fn scrolled(&mut self, cx: &mut Context<Self>) {
        // Both are measured from the rows drawn so far, which those in view
        // and near it are.
        let below =
            self.list.max_offset_for_scrollbar().y + self.list.scroll_px_offset_for_scrollbar().y;
        let at_end = below <= px(AT_END);
        if self.follow != at_end {
            self.follow = at_end;
            cx.notify();
        }
    }

    /// The selected text: whole lines as logcat printed them (with their
    /// date, pid and tid), and of a line selected in part, that part.
    fn selected_text(&self, shown: &Shown) -> Option<String> {
        let selection = self.selection.filter(|selection| !selection.is_empty())?;
        let (start, end) = selection.ends();
        let mut out = Vec::new();
        for row in start.row.max(shown.dropped)..=end.row {
            let Some(line) = shown.lines.get((row - shown.dropped) as usize) else {
                break;
            };
            let cells = Cells::of(line);
            let Some(range) = selection.in_row(row, cells.text.len()) else {
                continue;
            };
            if range == (0..cells.text.len()) {
                out.push(line.text.clone());
            } else {
                out.push(cells.text.get(range).unwrap_or_default().to_string());
            }
        }
        (!out.is_empty()).then(|| out.join("\n"))
    }

    /// The lines the selection touches, whole, for chat.
    fn selected_lines(&self, shown: &Shown) -> Vec<String> {
        let Some(selection) = self.selection.filter(|selection| !selection.is_empty()) else {
            return Vec::new();
        };
        let (start, end) = selection.ends();
        let from = start.row.saturating_sub(shown.dropped) as usize;
        let to = ((end.row + 1).saturating_sub(shown.dropped) as usize).min(shown.lines.len());
        let lines = shown.lines.get(from..to).unwrap_or_default();
        lines[lines.len().saturating_sub(SELECTED_LINES)..]
            .iter()
            .map(|line| line.text.clone())
            .collect()
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        if let Some(text) = self.selected_text(&shown) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        let Some(last) = shown.lines.last() else {
            return;
        };
        self.selection = Some(Selection {
            anchor: Place {
                row: shown.dropped,
                at: 0,
            },
            head: Place {
                row: shown.dropped + shown.lines.len() as u64 - 1,
                at: Cells::of(last).text.len(),
            },
        });
        cx.notify();
    }

    /// The selected lines to chat, or the newest ones shown when none are.
    fn send_to_chat(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        let mut lines = self.selected_lines(&shown);
        if lines.is_empty() {
            lines = shown.lines[shown.lines.len().saturating_sub(SEND_LINES)..]
                .iter()
                .map(|line| line.text.clone())
                .collect();
        }
        if lines.is_empty() {
            return;
        }
        cx.emit(ToChat::Text {
            label: "logcat".into(),
            text: format!(
                "Logcat from {}:\n```\n{}\n```",
                self.device,
                lines.join("\n")
            ),
        });
    }

    /// The mouse went down at `place` in a row whose text is `text`: once
    /// starts a selection there (with Shift, carries the one there is to
    /// it), twice selects the word, three times the line.
    fn press(
        &mut self,
        place: Place,
        text: &str,
        extend: bool,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        // Reading here: new lines must not scroll these away.
        self.follow = false;
        let at = |at: usize| Place { row: place.row, at };
        let (anchor, head) = match clicks {
            0 | 1 if extend => (self.selection.map_or(place, |held| held.anchor), place),
            0 | 1 => (place, place),
            2 => {
                let word = word_at(text, place.at);
                (at(word.start), at(word.end))
            }
            _ => (at(0), at(text.len())),
        };
        self.selection = Some(Selection { anchor, head });
        self.selecting = clicks <= 1;
        self.pressed = (clicks <= 1 && !extend).then_some(place);
        cx.notify();
    }

    /// The mouse moved to `place` with its button down.
    fn drag_to(&mut self, place: Place, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if self.pressed.is_some_and(|pressed| pressed != place) {
            self.pressed = None;
        }
        if let Some(selection) = &mut self.selection
            && selection.head != place
        {
            selection.head = place;
            cx.notify();
        }
    }

    /// The mouse came up at `place`: where it went down, on a stack
    /// frame's file, that is a click on the file.
    fn release(&mut self, place: Place, link: Option<SourceRef>, cx: &mut Context<Self>) {
        self.selecting = false;
        if self.pressed.take() == Some(place)
            && let Some(source) = link
        {
            cx.emit(LogcatViewEvent::OpenSource(source));
        }
    }

    /// A right click on `line`, in row `row` whose text is `len` bytes
    /// long: the menu is for the selection when the line is in it, else for
    /// this line, which is then what is selected.
    fn context_press(
        &mut self,
        line: Rc<LogLine>,
        row: u64,
        len: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        self.follow = false;
        let inside = self
            .selection
            .is_some_and(|selection| selection.in_row(row, len).is_some());
        if !inside {
            self.selection = Some(Selection {
                anchor: Place { row, at: 0 },
                head: Place { row, at: len },
            });
        }
        self.context = Some(line);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" && self.selection.take().is_some() {
            cx.stop_propagation();
            cx.notify();
        } else if ph_media::editing_shortcut(keystroke, cx) {
            match keystroke.key.as_str() {
                "c" => self.copy_selection(cx),
                "a" => self.select_all(cx),
                _ => return,
            }
            cx.stop_propagation();
        }
    }

    /// Puts `token` in the filter box in place of any that starts with one
    /// of `prefixes`.
    fn set_filter_token(
        &mut self,
        prefixes: &[&str],
        token: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.filter.read(cx).value().to_string();
        let mut words: Vec<String> = current
            .split_whitespace()
            .filter(|word| !prefixes.iter().any(|prefix| word.starts_with(prefix)))
            .map(str::to_string)
            .collect();
        words.extend(token);
        let text = words.join(" ");
        self.filter
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// Adds `token` to the filter box, once.
    fn add_filter_token(&mut self, token: String, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.filter.read(cx).value().to_string();
        if current.split_whitespace().any(|word| word == token) {
            return;
        }
        let text = format!("{current} {token}").trim().to_string();
        self.filter
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// The right-click menu: the selection's commands, then those of the
    /// line clicked.
    fn context_menu(&self, menu: PopupMenu, view: WeakEntity<Self>, cx: &App) -> PopupMenu {
        let Some(line) = self.context.clone() else {
            return menu;
        };
        let act = |action: fn(&mut Self, &mut Context<Self>)| {
            let view = view.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                _ = view.update(cx, action);
            }
        };
        let mut menu = menu
            .item(PopupMenuItem::new("Copy").on_click(act(|view, cx| view.copy_selection(cx))))
            .item(PopupMenuItem::new("Copy message").on_click({
                let message = line.message.clone();
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(message.clone()))
            }))
            .item(
                PopupMenuItem::new("Send to chat")
                    .icon(Icons::SendHorizontal)
                    .on_click(act(|view, cx| view.send_to_chat(cx))),
            )
            .separator();
        // A tag is one word in the filter; of a tag with spaces, its first.
        if let Some(tag) = line.tag.split_whitespace().next() {
            let filter = |token: String, replace: bool| {
                let view = view.clone();
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                    _ = view.update(cx, |view, cx| {
                        if replace {
                            view.set_filter_token(&["tag:"], Some(token.clone()), window, cx)
                        } else {
                            view.add_filter_token(token.clone(), window, cx)
                        }
                    });
                }
            };
            menu = menu
                .item(
                    PopupMenuItem::new(format!("Only tag {tag}"))
                        .on_click(filter(format!("tag:{tag}"), true)),
                )
                .item(
                    PopupMenuItem::new(format!("Hide tag {tag}"))
                        .on_click(filter(format!("-tag:{tag}"), false)),
                );
        }
        let process = self
            .logcat
            .as_ref()
            .and_then(|logcat| logcat.read(cx).process_of(&line).map(str::to_string));
        // An app's processes by its name; a process of no app by its number.
        let (label, token) = match process.filter(|name| crate::adb::is_package_name(name)) {
            Some(name) => (format!("Only {name}"), format!("package:{name}")),
            None => (
                format!("Only process {}", line.pid),
                format!("pid:{}", line.pid),
            ),
        };
        let view = view.clone();
        menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
            _ = view.update(cx, |view, cx| {
                view.set_filter_token(
                    &["package:", "app:", "pid:"],
                    Some(token.clone()),
                    window,
                    cx,
                )
            });
        }))
    }

    fn render_toolbar(&self, query: &Query, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let min = query.level.unwrap_or(Priority::Verbose);
        let level = Button::new("logcat-level")
            .ghost()
            .xsmall()
            .label(min.name())
            .dropdown_caret(true)
            .tooltip("Lowest level shown")
            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                Priority::ALL[..5].iter().fold(menu, |menu, &priority| {
                    let view = view.clone();
                    menu.item(
                        PopupMenuItem::new(format!("{} and above", priority.name()))
                            .checked(priority == min)
                            .on_click(move |_, window, cx| {
                                // Every level is the filter with no level in it.
                                let token = (priority != Priority::Verbose).then(|| {
                                    format!("level:{}", priority.name().to_ascii_lowercase())
                                });
                                _ = view.update(cx, |view, cx| {
                                    view.set_filter_token(&["level:"], token, window, cx)
                                });
                            }),
                    )
                })
            });
        let package = query.packages.first().cloned();
        let app_label = match package.as_deref() {
            None => "All apps".to_string(),
            Some(MINE) => "This project".to_string(),
            Some(package) => package.rsplit('.').next().unwrap_or(package).to_string(),
        };
        // A list to search: a phone has more apps than a menu shows.
        let picker = self.app_picker.clone();
        let app = Popover::new("logcat-app-picker")
            .offset(ph_ui::POPUP_GAP)
            .anchor(gpui_kit::Anchor::TopLeft)
            .open(self.app_picker_open)
            .on_open_change(cx.listener(move |this, open: &bool, window, cx| {
                this.app_picker_open = *open;
                if *open {
                    let current = package.clone();
                    this.app_picker
                        .update(cx, |picker, cx| picker.opened(current, window, cx));
                }
                cx.notify();
            }))
            .trigger(
                Button::new("logcat-app")
                    .ghost()
                    .xsmall()
                    .icon(Icons::Package)
                    .label(app_label)
                    .dropdown_caret(true)
                    .tooltip("Only one app's lines"),
            )
            .content(move |_, _, _| picker.clone());
        let following = self.follow;
        h_flex()
            .flex_none()
            .px_2()
            .py_1()
            .gap_1()
            .child(level)
            .child(app)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.filter).xsmall().cleanable(true)),
            )
            .child(
                Button::new("logcat-wrap")
                    .ghost()
                    .xsmall()
                    .icon(Icons::TextWrap)
                    .selected(self.wrap)
                    .tooltip(if self.wrap {
                        "Long messages wrap (off: one line each, scrolled sideways)"
                    } else {
                        "Wrap long messages"
                    })
                    .accessibility_label("Wrap long messages")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let wrap = !this.wrap;
                        this.set_wrap(wrap, cx);
                        cx.emit(LogcatViewEvent::WrapChanged(wrap));
                    })),
            )
            .child(
                Button::new("logcat-follow")
                    .ghost()
                    .xsmall()
                    .icon(Icons::ArrowDownToLine)
                    .selected(following)
                    .tooltip(if following {
                        "Following new lines (scroll up to stop)"
                    } else {
                        "Follow new lines"
                    })
                    .accessibility_label("Follow new lines")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.follow = !this.follow;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("logcat-send")
                    .ghost()
                    .xsmall()
                    .icon(Icons::SendHorizontal)
                    .tooltip("Send to chat: the selected lines, or the last 80 shown")
                    .accessibility_label("Send to chat")
                    .on_click(cx.listener(|this, _, _, cx| this.send_to_chat(cx))),
            )
            .child(
                Button::new("logcat-clear")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Delete)
                    .tooltip("Clear")
                    .accessibility_label("Clear logcat")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selection = None;
                        if let Some(logcat) = &this.logcat {
                            logcat.update(cx, |logcat, cx| logcat.clear(cx));
                        }
                    })),
            )
    }
}

impl Render for LogcatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query(cx);
        let shown = self.shown(cx);
        self.sync(&shown);
        let body: AnyElement = if shown.lines.is_empty() {
            let text = if self.logcat.is_none() {
                "Logcat starts when a device is chosen."
            } else if query.wants_mine() && self.mine.is_empty() {
                "No app of this project is known yet. Run it once, or pick an app."
            } else {
                "No lines match."
            };
            div()
                .flex_1()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element()
        } else {
            let rows = Rows {
                view: cx.entity().downgrade(),
                lines: Rc::new(shown.lines),
                dropped: shown.dropped,
                words: Rc::new(query.words.clone()),
                selection: self.selection,
                wrap: self.wrap,
            };
            let view = cx.entity().downgrade();
            div()
                .id("logcat-lines")
                .track_focus(&self.focus)
                .flex_1()
                .min_h_0()
                .min_w_0()
                .cursor_text()
                .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
                .map(|this| {
                    let rows = list(self.list.clone(), move |ix, _, cx| rows.render(ix, cx))
                        .h_full()
                        .min_h_0();
                    if self.wrap {
                        return this.child(rows.w_full());
                    }
                    // Unwrapped, the rows are as wide as the longest of
                    // them and the list scrolls sideways under the pane.
                    let width = self.unwrapped_width(window, cx);
                    this.child(
                        div()
                            .id("logcat-sideways")
                            .size_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.hscroll)
                            .child(rows.w(width).min_w_full()),
                    )
                    .horizontal_scrollbar(&self.hscroll)
                })
                .vertical_scrollbar(&self.list)
                .context_menu(move |menu, _, cx| match view.upgrade() {
                    Some(this) => this.read(cx).context_menu(menu, view.clone(), cx),
                    None => menu,
                })
                .into_any_element()
        };
        v_flex()
            .size_full()
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .child(self.render_toolbar(&query, cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
    }
}

/// What drawing a row takes: the lines shown and how they are shown.
struct Rows {
    view: WeakEntity<LogcatView>,
    lines: Rc<Vec<Rc<LogLine>>>,
    /// How many lines went from the front of the list before `lines[0]`.
    dropped: u64,
    /// The filter's words, marked where they match.
    words: Rc<Vec<String>>,
    selection: Option<Selection>,
    wrap: bool,
}

impl Rows {
    fn render(&self, ix: usize, cx: &App) -> AnyElement {
        let Some(line) = self.lines.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let row = self.dropped + ix as u64;
        let cells = Cells::of(&line);
        let selected = self
            .selection
            .and_then(|selection| selection.in_row(row, cells.text.len()));
        // A line selected whole is marked across the pane, as in an editor.
        let whole = selected == Some(0..cells.text.len());
        let link = source_link(&line.message)
            .map(|(range, source)| (shifted(&range, cells.message.start), source));
        let mark = HighlightStyle {
            background_color: Some(theme.warning.opacity(0.3)),
            ..Default::default()
        };
        let select = HighlightStyle {
            background_color: Some(theme.selection),
            ..Default::default()
        };
        let linked = HighlightStyle {
            color: Some(theme.link),
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme.link.opacity(0.6)),
                wavy: false,
            }),
            ..Default::default()
        };
        // One column's text, with what marks it: the filter's words, the
        // stack frame's file and the selection.
        let column = |range: &Range<usize>, marks_words: bool| {
            let text = &cells.text[range.clone()];
            let mut layers: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
            if marks_words {
                layers.extend(
                    match_ranges(text, &self.words)
                        .into_iter()
                        .map(|found| (found, mark)),
                );
            }
            if let Some((link, _)) = &link
                && let Some(within) = intersection(link, range)
            {
                layers.push((within, linked));
            }
            if !whole
                && let Some(selected) = &selected
                && let Some(within) = intersection(selected, range)
            {
                layers.push((within, select));
            }
            let styled =
                StyledText::new(text.to_string()).with_highlights(flatten(text.len(), &layers));
            let layout = styled.layout().clone();
            (styled, layout)
        };
        let (time, time_layout) = column(&cells.time, false);
        let (level, level_layout) = column(&cells.level, false);
        let (tag, tag_layout) = column(&cells.tag, true);
        let (message, message_layout) = column(&cells.message, true);
        let hit = Rc::new(RowHit {
            row,
            text: cells.text.as_str().into(),
            columns: [
                (time_layout, cells.time.clone()),
                (level_layout, cells.level.clone()),
                (tag_layout, cells.tag.clone()),
                (message_layout, cells.message.clone()),
            ],
            link,
        });
        // Warnings and errors are told by their colour; the rest is plain
        // text, the lowest level a little quieter.
        let (level_color, text_color) = match line.priority {
            Priority::Verbose => (theme.muted_foreground, Some(theme.muted_foreground)),
            Priority::Debug => (theme.muted_foreground, None),
            Priority::Info => (theme.foreground, None),
            Priority::Warn => (theme.warning, Some(theme.warning)),
            Priority::Error | Priority::Fatal => (theme.danger, Some(theme.danger)),
        };
        let text_len = cells.text.len();
        let (down, moved, up, menu) = (hit.clone(), hit.clone(), hit.clone(), self.view.clone());
        let (press, drag, release) = (self.view.clone(), self.view.clone(), self.view.clone());
        h_flex()
            .id(("logcat-row", row as usize))
            .w_full()
            .min_h(ph_ui::rems_from_px(ROW_HEIGHT))
            // The log's own type is each row's: the menu over the rows is
            // in the app's.
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .line_height(ph_ui::rems_from_px(ROW_HEIGHT))
            .items_start()
            .px_2()
            .gap_2()
            .when(whole, |this| this.bg(theme.selection))
            .child(
                div()
                    .flex_none()
                    .w(ph_ui::rems_from_px(TIME_WIDTH))
                    .whitespace_nowrap()
                    .text_color(theme.muted_foreground)
                    .child(time),
            )
            .child(
                div()
                    .flex_none()
                    .w(ph_ui::rems_from_px(LEVEL_WIDTH))
                    .whitespace_nowrap()
                    .font_weight(FontWeight::BOLD)
                    .text_color(level_color)
                    .child(level),
            )
            .child(
                div()
                    .flex_none()
                    .w(ph_ui::rems_from_px(TAG_WIDTH))
                    .truncate()
                    .when_some(text_color, |this, color| this.text_color(color))
                    .child(tag),
            )
            .child(
                div()
                    // Wrapped it takes what is left of the row; unwrapped it
                    // is as long as it is, and the list scrolls.
                    .map(|this| {
                        if self.wrap {
                            this.flex_1().min_w_0()
                        } else {
                            this.flex_none().whitespace_nowrap()
                        }
                    })
                    .when_some(text_color, |this, color| this.text_color(color))
                    .child(message),
            )
            .on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window, cx| {
                    let place = down.place(event.position);
                    _ = press.update(cx, |view, cx| {
                        view.press(
                            place,
                            &down.text,
                            event.modifiers.shift,
                            event.click_count,
                            window,
                            cx,
                        )
                    });
                },
            )
            .on_mouse_down(MouseButton::Right, move |_: &MouseDownEvent, window, cx| {
                let line = line.clone();
                _ = menu.update(cx, |view, cx| {
                    view.context_press(line, row, text_len, window, cx)
                });
            })
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let place = moved.place(event.position);
                _ = drag.update(cx, |view, cx| view.drag_to(place, cx));
            })
            .on_mouse_up(MouseButton::Left, move |event: &MouseUpEvent, _, cx| {
                let place = up.place(event.position);
                let link = up.link_at(place);
                _ = release.update(cx, |view, cx| view.release(place, link, cx));
            })
            .into_any_element()
    }
}

/// The length of the longest message in `lines`, in bytes.
fn longest(lines: &[Rc<LogLine>]) -> usize {
    lines
        .iter()
        .map(|line| line.message.len())
        .max()
        .unwrap_or(0)
}

/// `range` moved right by `by`.
fn shifted(range: &Range<usize>, by: usize) -> Range<usize> {
    range.start + by..range.end + by
}

/// The part of `range` inside `within`, counted from where `within` starts.
fn intersection(range: &Range<usize>, within: &Range<usize>) -> Option<Range<usize>> {
    let (start, end) = (range.start.max(within.start), range.end.min(within.end));
    (start < end).then(|| start - within.start..end - within.start)
}

/// One list of styles for a text of `len` bytes from layers that may
/// overlap: where they do, a later layer adds to an earlier one and wins.
fn flatten(
    len: usize,
    layers: &[(Range<usize>, HighlightStyle)],
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = layers
        .iter()
        .flat_map(|(range, _)| [range.start.min(len), range.end.min(len)])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter_map(|pair| {
            let (from, to) = (pair[0], pair[1]);
            let mut style: Option<HighlightStyle> = None;
            for (range, layer) in layers {
                if range.start <= from && to <= range.end {
                    let style = style.get_or_insert_with(HighlightStyle::default);
                    style.color = layer.color.or(style.color);
                    style.background_color = layer.background_color.or(style.background_color);
                    style.underline = layer.underline.or(style.underline);
                }
            }
            style.map(|style| (from..to, style))
        })
        .collect()
}

/// The word of `text` at byte `at`: what a double click selects. A word
/// runs to the next space, bracket, quote or comma, so a class name, a
/// path or an address is one.
fn word_at(text: &str, at: usize) -> Range<usize> {
    let ends = |c: char| c.is_whitespace() || "()[]{}<>,;\"'=".contains(c);
    let at = at.min(text.len());
    if !text.is_char_boundary(at) {
        return at..at;
    }
    let start = text[..at]
        .char_indices()
        .rev()
        .find(|(_, c)| ends(*c))
        .map_or(0, |(ix, c)| ix + c.len_utf8());
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| ends(*c))
        .map_or(text.len(), |(ix, _)| at + ix);
    start..end
}

/// The source a stack frame in `message` names, and where in the message
/// its file and line stand: `at com.example.Main.onCreate(Main.kt:42)`.
fn source_link(message: &str) -> Option<(Range<usize>, SourceRef)> {
    let close = message.rfind(')')?;
    let open = message[..close].rfind('(')?;
    let (file, line) = message[open + 1..close].rsplit_once(':')?;
    let line: u32 = line.parse().ok()?;
    let source = [".kt", ".java", ".kts"]
        .iter()
        .any(|extension| file.ends_with(extension));
    if !source || file.contains(['/', ' ']) {
        return None;
    }
    // `com.example.Main.onCreate` before the bracket: the class, then the
    // method.
    let class = message[..open]
        .rsplit(char::is_whitespace)
        .next()
        .and_then(|qualified| qualified.rsplit_once('.'))
        .map(|(class, _)| class.to_string())
        .filter(|class| !class.is_empty());
    Some((
        open + 1..close,
        SourceRef {
            file: file.to_string(),
            line,
            class,
        },
    ))
}

/// Byte ranges of `words` in `text`, any case, in order and not overlapping.
fn match_ranges(text: &str, words: &[String]) -> Vec<Range<usize>> {
    // Lowercasing keeps byte offsets only for ASCII; otherwise mark nothing.
    if words.is_empty() || !text.is_ascii() {
        return Vec::new();
    }
    let lower = text.to_ascii_lowercase();
    let mut ranges: Vec<Range<usize>> = words
        .iter()
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            lower
                .match_indices(word.as_str())
                .map(|(at, found)| at..at + found.len())
                .collect::<Vec<_>>()
        })
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_filter_words_once() {
        let words = vec!["api".to_string(), "pi c".to_string()];
        assert_eq!(match_ranges("API call to api", &words), vec![0..5, 12..15]);
        assert!(match_ranges("naïve api", &words).is_empty());
        assert!(match_ranges("text", &[]).is_empty());
    }

    #[test]
    fn a_line_is_one_text_with_its_columns() {
        let line = LogLine::parse("10-24 10:00:00.200  1234  1250 W OkHttp: slow request");
        let cells = Cells::of(&line);
        assert_eq!(cells.text, "10:00:00.200 W OkHttp: slow request");
        assert_eq!(&cells.text[cells.time.clone()], "10:00:00.200");
        assert_eq!(&cells.text[cells.level.clone()], "W");
        assert_eq!(&cells.text[cells.tag.clone()], "OkHttp");
        assert_eq!(&cells.text[cells.message.clone()], "slow request");
        // A line in no known shape is all message.
        let odd = Cells::of(&LogLine::parse("not a log line"));
        assert_eq!(odd.message, 0..odd.text.len());
    }

    #[test]
    fn a_selection_covers_part_of_its_end_lines_and_all_between() {
        let at = |row, at| Place { row, at };
        // Dragged upwards: the ends are put in order.
        let selection = Selection {
            anchor: at(7, 3),
            head: at(5, 4),
        };
        assert_eq!(selection.in_row(4, 10), None);
        assert_eq!(selection.in_row(5, 10), Some(4..10));
        assert_eq!(selection.in_row(6, 10), Some(0..10));
        assert_eq!(selection.in_row(6, 0), Some(0..0), "an empty line between");
        assert_eq!(selection.in_row(7, 10), Some(0..3));
        assert_eq!(selection.in_row(8, 10), None);
        // Within one line, and nothing when it is only a caret.
        let word = Selection {
            anchor: at(2, 2),
            head: at(2, 6),
        };
        assert_eq!(word.in_row(2, 10), Some(2..6));
        let caret = Selection {
            anchor: at(2, 2),
            head: at(2, 2),
        };
        assert_eq!(caret.in_row(2, 10), None);
    }

    #[test]
    fn a_double_click_takes_a_whole_name() {
        let text = "at com.example.Main.onCreate(Main.kt:42) for user=sam";
        assert_eq!(&text[word_at(text, 5)], "com.example.Main.onCreate");
        assert_eq!(&text[word_at(text, 31)], "Main.kt:42");
        assert_eq!(&text[word_at(text, text.len())], "sam");
        assert_eq!(&text[word_at(text, 0)], "at");
        assert_eq!(word_at("", 0), 0..0);
    }

    #[test]
    fn a_stack_frame_names_its_source() {
        let message = "\tat com.example.ui.Main$onCreate$1.invoke(Main.kt:42)";
        let (range, source) = source_link(message).unwrap();
        assert_eq!(&message[range], "Main.kt:42");
        assert_eq!(
            source,
            SourceRef {
                file: "Main.kt".into(),
                line: 42,
                class: Some("com.example.ui.Main$onCreate$1".into()),
            }
        );
        assert!(source_link("at android.os.Looper.loop(Looper.java:288)").is_some());
        // Not source: no line, a library with no file, plain brackets.
        assert_eq!(source_link("at a.b.C.d(Native Method)"), None);
        assert_eq!(source_link("at a.b.C.d(Unknown Source:12)"), None);
        assert_eq!(source_link("took 12 ms (cold start: 3)"), None);
        assert_eq!(source_link("no brackets"), None);
    }

    #[test]
    fn layers_that_overlap_become_one_list_of_styles() {
        let red = HighlightStyle {
            color: Some(gpui_kit::red()),
            ..Default::default()
        };
        let marked = HighlightStyle {
            background_color: Some(gpui_kit::blue()),
            ..Default::default()
        };
        let styles = flatten(10, &[(0..6, red), (4..20, marked)]);
        let ranges: Vec<Range<usize>> = styles.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, [0..4, 4..6, 6..10]);
        assert_eq!(styles[0].1, red);
        // Both where they overlap.
        assert_eq!(
            (styles[1].1.color, styles[1].1.background_color),
            (red.color, marked.background_color)
        );
        assert_eq!(styles[2].1, marked);
        assert!(flatten(10, &[]).is_empty());
    }
}
