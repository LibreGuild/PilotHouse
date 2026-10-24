//! A read-only viewer for files too big for the editor.
//!
//! Only the lines on screen are read, from the checkpoint before them (see
//! [`index`](super::index)), so opening a multi-gigabyte log is instant and
//! scrolling stays smooth while the line index is still being built. The
//! scroll position is a line number, not pixels: a list of 100 million rows
//! is beyond what `f32` pixel offsets can address, so this draws its own
//! rows and scrollbar.

use std::cell::Cell;
use std::fs::File;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollWheelEvent, SharedString,
    Styled as _, Subscription, Task, Window, canvas, div, px,
};

use super::index::{self, Line, LineIndex, Query};

/// Row height of the viewer's text, and the width of one character.
const ROW: f32 = 20.;
const CHAR: f32 = 7.8;
/// Most bytes copied at once.
const COPY_CAP: usize = 32 << 20;

pub struct LargeFileView {
    path: PathBuf,
    file: Option<Arc<File>>,
    len: u64,
    index: Arc<Mutex<LineIndex>>,
    /// Set to stop the running scan (the file shrank, or the view closed).
    cancel_scan: Arc<AtomicBool>,
    indexing: bool,
    /// First line on screen, and how many rows fit.
    top: u64,
    rows: usize,
    /// Pixels scrolled that do not yet add up to a row.
    scroll_rest: f32,
    /// Characters scrolled off the left (long lines), and the pixels that
    /// do not yet add up to one.
    left: usize,
    left_rest: f32,
    /// What was read for `(top, rows)`, kept until either changes.
    shown: Option<(u64, Vec<Line>)>,
    /// Selected lines (anchor, head).
    selection: Option<(u64, u64)>,
    /// The current search match: its line and byte offset.
    hit: Option<(u64, u64)>,
    /// Stay at the end as the file grows.
    follow: bool,
    search: Entity<InputState>,
    go_to: Entity<InputState>,
    status: Option<SharedString>,
    cancel_search: Arc<AtomicBool>,
    dragging_thumb: bool,
    view_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    track_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    focus_handle: FocusHandle,
    error: Option<SharedString>,
    _scan: Option<Task<()>>,
    _search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LargeFileView {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Find (Enter, ⇧Enter for previous)"));
        let go_to = cx.new(|cx| InputState::new(window, cx).placeholder("Line"));
        let subscriptions = vec![
            cx.subscribe_in(
                &search,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift, .. } = event {
                        let back = *shift || window.modifiers().shift;
                        let text = input.read(cx).value().to_string();
                        this.find(&text, back, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &go_to,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        let text = input.read(cx).value().trim().replace([',', '_'], "");
                        if let Ok(line) = text.parse::<u64>() {
                            this.reveal(line.saturating_sub(1), cx);
                            this.selection = Some((line.saturating_sub(1), line.saturating_sub(1)));
                            this.focus_handle.focus(window, cx);
                        }
                    }
                },
            ),
        ];
        let mut view = Self {
            path,
            file: None,
            len: 0,
            index: Arc::default(),
            cancel_scan: Arc::default(),
            indexing: false,
            top: 0,
            rows: 40,
            scroll_rest: 0.,
            left: 0,
            left_rest: 0.,
            shown: None,
            selection: None,
            hit: None,
            follow: false,
            search,
            go_to,
            status: None,
            cancel_search: Arc::default(),
            dragging_thumb: false,
            view_bounds: Rc::default(),
            track_bounds: Rc::default(),
            focus_handle: cx.focus_handle(),
            error: None,
            _scan: None,
            _search: None,
            _subscriptions: subscriptions,
        };
        view.refresh(cx);
        view
    }

    /// Reads the file's size again: a grown file is indexed on from where
    /// the scan stopped, a shrunk or replaced one from the start.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let opened = File::open(&self.path).and_then(|file| {
            let len = file.metadata()?.len();
            Ok((file, len))
        });
        let (file, len) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                self.error = Some(format!("Could not read the file: {err}").into());
                cx.notify();
                return;
            }
        };
        self.error = None;
        if len
            < self
                .index
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .scanned()
        {
            self.cancel_scan.store(true, Ordering::Relaxed);
            self.cancel_scan = Arc::default();
            self.index = Arc::default();
            self.top = 0;
            self.selection = None;
            self.hit = None;
        }
        let file = Arc::new(file);
        self.file = Some(file.clone());
        self.len = len;
        self.shown = None;
        self.start_scan(file, cx);
        cx.notify();
    }

    fn start_scan(&mut self, file: Arc<File>, cx: &mut Context<Self>) {
        let (index, cancel, len) = (self.index.clone(), self.cancel_scan.clone(), self.len);
        let (progress, updates) = smol::channel::bounded::<()>(1);
        self.indexing = true;
        let work = cx.background_spawn(async move {
            let mut last = 0;
            index::scan(&file, len, &index, &cancel, |offset| {
                // About ten redraws a second at disk speed.
                if offset - last >= 64 << 20 {
                    last = offset;
                    let _ = progress.try_send(());
                }
            })
        });
        self._scan = Some(cx.spawn(async move |this, cx| {
            let ticker = async {
                while updates.recv().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            };
            let (result, ()) = smol::future::zip(work, ticker).await;
            _ = this.update(cx, |this, cx| {
                this.indexing = false;
                if let Err(err) = result {
                    this.error = Some(format!("Could not read the file: {err}").into());
                }
                if this.follow {
                    this.scroll_to_end();
                }
                cx.notify();
            });
        }));
    }

    fn lines(&self) -> u64 {
        self.index.lock().unwrap_or_else(|e| e.into_inner()).lines()
    }

    fn max_top(&self) -> u64 {
        self.lines()
            .saturating_sub(self.rows.saturating_sub(1) as u64)
    }

    fn set_top(&mut self, top: u64, cx: &mut Context<Self>) {
        let top = top.min(self.max_top());
        if top != self.top {
            self.top = top;
            self.follow = top == self.max_top() && self.follow;
            cx.notify();
        }
    }

    fn scroll_to_end(&mut self) {
        self.top = self.max_top();
    }

    /// Scrolls so `line` is on screen, a third of the way down.
    fn reveal(&mut self, line: u64, cx: &mut Context<Self>) {
        if line < self.top || line >= self.top + self.rows as u64 {
            self.set_top(line.saturating_sub(self.rows as u64 / 3), cx);
        }
        cx.notify();
    }

    /// The lines on screen, read when the view moved (or more lines came).
    fn visible(&mut self) -> &[Line] {
        let stale = match &self.shown {
            Some((top, lines)) => {
                *top != self.top
                    || (lines.len() < self.rows && self.top + (lines.len() as u64) < self.lines())
            }
            None => true,
        };
        if stale {
            let lines = self.read(self.top, self.rows).unwrap_or_default();
            self.shown = Some((self.top, lines));
        }
        let lines = &self.shown.as_ref().expect("just read").1;
        &lines[..lines.len().min(self.rows)]
    }

    fn read(&self, first: u64, count: usize) -> Option<Vec<Line>> {
        let file = self.file.as_ref()?;
        let index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        // Read on from the lines already shown when scrolling down a little.
        let anchor = match &self.shown {
            Some((_, lines))
                if lines.first().is_some_and(|line| line.number <= first)
                    && first - lines[0].number < index::STRIDE =>
            {
                (lines[0].number, lines[0].start)
            }
            _ => index.anchor(first),
        };
        drop(index);
        index::read_lines(file, self.len, anchor, first, count).ok()
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pixels = event.delta.pixel_delta(px(ROW));
        // Sideways: a trackpad's x, or shift with a wheel.
        let sideways = if event.modifiers.shift {
            pixels.y
        } else {
            pixels.x
        };
        let dx = f32::from(sideways) + self.left_rest;
        let chars = (dx / CHAR).trunc();
        self.left_rest = dx - chars * CHAR;
        if chars != 0. {
            let left = (self.left as i64 - chars as i64).clamp(0, index::MAX_LINE_BYTES as i64);
            self.left = left as usize;
            cx.notify();
        }
        if event.modifiers.shift {
            return;
        }
        let delta = f32::from(pixels.y) + self.scroll_rest;
        let rows = (delta / ROW).trunc();
        self.scroll_rest = delta - rows * ROW;
        if rows != 0. {
            let rows = -rows as i64;
            let top = self.top.saturating_add_signed(rows);
            self.follow = rows > 0 && top >= self.max_top();
            self.set_top(top, cx);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.platform || keystroke.modifiers.control;
        let page = self.rows.saturating_sub(2).max(1) as i64;
        let moved = match keystroke.key.as_str() {
            "up" if command => Some(i64::MIN),
            "down" if command => Some(i64::MAX),
            "up" => Some(-1),
            "down" => Some(1),
            "pageup" => Some(-page),
            "pagedown" | "space" => Some(page),
            "home" => Some(i64::MIN),
            "end" => Some(i64::MAX),
            "c" if command => {
                self.copy(cx);
                None
            }
            "f" if command => {
                self.search.update(cx, |input, cx| input.focus(window, cx));
                None
            }
            "g" if command || keystroke.modifiers.control => {
                self.go_to.update(cx, |input, cx| input.focus(window, cx));
                None
            }
            "a" if command => {
                self.selection = Some((0, self.lines().saturating_sub(1)));
                cx.notify();
                None
            }
            _ => None,
        };
        if let Some(rows) = moved {
            let top = match rows {
                i64::MIN => 0,
                i64::MAX => self.max_top(),
                rows => self.top.saturating_add_signed(rows),
            };
            self.follow = rows == i64::MAX;
            self.set_top(top, cx);
            cx.stop_propagation();
        }
    }

    fn select(&mut self, line: u64, extend: bool, cx: &mut Context<Self>) {
        self.selection = match (self.selection, extend) {
            (Some((anchor, _)), true) => Some((anchor, line)),
            _ => Some((line, line)),
        };
        cx.notify();
    }

    /// Copies the selected lines (up to 32 MB).
    fn copy(&mut self, cx: &mut Context<Self>) {
        let (Some((a, b)), Some(file)) = (self.selection, self.file.clone()) else {
            return;
        };
        let (first, last) = (a.min(b), a.max(b));
        let ends = self
            .read(first, 1)
            .and_then(|start| Some((start.first()?.start, self.read(last, 1)?.first()?.end)));
        let Some((start, end)) = ends else { return };
        match index::read_range(&file, start, end, COPY_CAP) {
            Ok(bytes) => {
                let cut = (end - start) as usize > bytes.len();
                cx.write_to_clipboard(ClipboardItem::new_string(
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
                self.status = Some(if cut {
                    "Copied the first 32 MB of the selection.".into()
                } else {
                    format!(
                        "Copied {} line{}.",
                        last - first + 1,
                        if first == last { "" } else { "s" }
                    )
                    .into()
                });
            }
            Err(err) => self.status = Some(format!("Could not copy: {err}").into()),
        }
        cx.notify();
    }

    /// Finds the next (or previous) match after the current one, wrapping
    /// around once.
    fn find(&mut self, text: &str, back: bool, cx: &mut Context<Self>) {
        let (Some(query), Some(file)) = (Query::new(text), self.file.clone()) else {
            return;
        };
        self.cancel_search.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_search = cancel.clone();
        let len = self.len;
        // From the current match, else from the top of the screen.
        let from = match self.hit {
            Some((_, offset)) => offset,
            None => self
                .read(self.top, 1)
                .and_then(|lines| lines.first().map(|line| line.start))
                .unwrap_or(0),
        };
        let index = self.index.clone();
        self.status = Some("Searching…".into());
        cx.notify();
        let work = cx.background_spawn(async move {
            let found = if back {
                match index::find_prev(&file, &query, from, &cancel)? {
                    Some(found) => Some(found),
                    None => index::find_prev(&file, &query, len, &cancel)?,
                }
            } else {
                let start = if from == 0 { 0 } else { from + 1 };
                match index::find_next(&file, len, &query, start, &cancel)? {
                    Some(found) => Some(found),
                    None => index::find_next(&file, len, &query, 0, &cancel)?,
                }
            };
            let Some(offset) = found else {
                return std::io::Result::Ok(None);
            };
            // The line needs the index up to the match.
            while index.lock().unwrap_or_else(|e| e.into_inner()).scanned() < offset
                && !cancel.load(Ordering::Relaxed)
            {
                smol::Timer::after(Duration::from_millis(50)).await;
            }
            let snapshot = index.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Ok(Some((index::line_of(&file, &snapshot, offset)?, offset)))
        });
        self._search = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some((line, offset))) => {
                        this.hit = Some((line, offset));
                        this.selection = Some((line, line));
                        this.status = Some(format!("Match on line {}", line + 1).into());
                        this.reveal(line, cx);
                    }
                    Ok(None) => this.status = Some("No matches.".into()),
                    Err(err) => this.status = Some(format!("Search failed: {err}").into()),
                }
                cx.notify();
            });
        }));
    }

    /// Fits the row count to the measured height.
    fn fit(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let rows = ((f32::from(bounds.size.height) / ROW).floor() as usize).max(1);
        if rows != self.rows {
            self.rows = rows;
            self.top = self.top.min(self.max_top());
            cx.notify();
        }
    }

    /// Moves the view so the thumb sits under the pointer.
    fn drag_thumb(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some(track) = self.track_bounds.get() else {
            return;
        };
        let height = f32::from(track.size.height).max(1.);
        let fraction = (f32::from(y - track.origin.y) / height).clamp(0., 1.);
        let top = (fraction as f64 * self.max_top() as f64).round() as u64;
        self.follow = top >= self.max_top();
        self.set_top(top, cx);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = self.lines();
        let scanned = self
            .index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .scanned();
        let size = format_size(self.len);
        let summary = if self.indexing && self.len > 0 {
            format!(
                "{size} · counting lines… {}% ({} so far)",
                scanned * 100 / self.len.max(1),
                group(lines)
            )
        } else {
            format!("{size} · {} lines", group(lines))
        };
        h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(format!("Read-only · {summary}")),
            )
            .child(div().w(px(180.)).child(Input::new(&self.search).xsmall()))
            .child(
                Button::new("large-find-prev")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronUp)
                    .tooltip("Previous match (⇧Enter)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let text = this.search.read(cx).value().to_string();
                        this.find(&text, true, cx);
                    })),
            )
            .child(
                Button::new("large-find-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .tooltip("Next match (Enter)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let text = this.search.read(cx).value().to_string();
                        this.find(&text, false, cx);
                    })),
            )
            .child(div().w(px(72.)).child(Input::new(&self.go_to).xsmall()))
            .child(
                Button::new("large-follow")
                    .ghost()
                    .xsmall()
                    .selected(self.follow)
                    .icon(IconName::ArrowDown)
                    .tooltip("Follow the end as the file grows")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.follow = !this.follow;
                        if this.follow {
                            this.scroll_to_end();
                        }
                        cx.notify();
                    })),
            )
    }

    fn render_scrollbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = self.lines().max(1);
        let max_top = self.max_top();
        let visible = (self.rows as f64 / lines as f64).min(1.);
        let track_bounds = self.track_bounds.clone();
        let height = track_bounds
            .get()
            .map(|bounds| f32::from(bounds.size.height))
            .unwrap_or(400.);
        let thumb = (visible as f32 * height).max(24.).min(height);
        let offset = if max_top == 0 {
            0.
        } else {
            (self.top as f64 / max_top as f64) as f32 * (height - thumb)
        };
        div()
            .id("large-scrollbar")
            .relative()
            .flex_none()
            .w(px(10.))
            .h_full()
            .when(max_top > 0, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(offset))
                        .left(px(2.))
                        .w(px(6.))
                        .h(px(thumb))
                        .rounded(px(3.))
                        .bg(cx.theme().muted_foreground.opacity(if self.dragging_thumb {
                            0.6
                        } else {
                            0.35
                        })),
                )
            })
            .child(
                canvas(
                    move |bounds, _, _| track_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.dragging_thumb = true;
                    this.drag_thumb(event.position.y, cx);
                    cx.stop_propagation();
                }),
            )
    }
}

impl Focusable for LargeFileView {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for LargeFileView {
    fn drop(&mut self) {
        self.cancel_scan.store(true, Ordering::Relaxed);
        self.cancel_search.store(true, Ordering::Relaxed);
    }
}

impl Render for LargeFileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.follow && !self.indexing {
            self.scroll_to_end();
        }
        let theme = cx.theme();
        let (active, warning, muted, danger, border, mono) = (
            theme.list_active,
            theme.warning,
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.mono_font_family.clone(),
        );
        let selection = self.selection.map(|(a, b)| (a.min(b), a.max(b)));
        let hit = self.hit.map(|(line, _)| line);
        let gutter = px(16. + CHAR * group_digits(self.lines()) as f32);
        let left = self.left;
        let rows: Vec<_> = self
            .visible()
            .iter()
            .map(|line| {
                let number = line.number;
                let selected = selection.is_some_and(|(a, b)| (a..=b).contains(&number));
                let cut = line.is_cut();
                h_flex()
                    .id(("large-line", number as usize))
                    .h(px(ROW))
                    .flex_none()
                    .w_full()
                    .whitespace_nowrap()
                    .when(selected, |this| this.bg(active))
                    .when(hit == Some(number), |this| this.bg(warning.opacity(0.22)))
                    .child(
                        div()
                            .flex_none()
                            .w(gutter)
                            .pr_3()
                            .text_right()
                            .text_color(muted.opacity(0.7))
                            .child((number + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(if left == 0 {
                                line.text.clone()
                            } else {
                                line.text.chars().skip(left).collect()
                            }),
                    )
                    .when(cut, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .px_2()
                                .text_color(muted)
                                .child(format!("… {}", format_size(line.end - line.start))),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.focus_handle.focus(window, cx);
                            this.select(number, event.modifiers.shift, cx);
                        }),
                    )
            })
            .collect();

        let view_bounds = self.view_bounds.clone();
        let entity = cx.weak_entity();
        let measure = canvas(
            move |bounds, _, cx| {
                if view_bounds.get() != Some(bounds) {
                    view_bounds.set(Some(bounds));
                    cx.defer(move |cx| {
                        _ = entity.update(cx, |this, cx| this.fit(bounds, cx));
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        v_flex()
            .id("large-file")
            .key_context("LargeFileView")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .child(self.render_header(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(danger)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .font_family(mono)
                            .text_size(px(13.))
                            .line_height(px(ROW))
                            .on_scroll_wheel(cx.listener(Self::scroll))
                            .child(measure)
                            .child(v_flex().children(rows)),
                    )
                    .child(self.render_scrollbar(cx)),
            )
            .when_some(self.status.clone(), |this, status| {
                this.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_0p5()
                        .border_t_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(muted)
                        .child(status),
                )
            })
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.dragging_thumb {
                    if event.pressed_button == Some(MouseButton::Left) {
                        this.drag_thumb(event.position.y, cx);
                    } else {
                        this.dragging_thumb = false;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging_thumb {
                        this.dragging_thumb = false;
                        cx.notify();
                    }
                }),
            )
    }
}

/// `1.2 GB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `12,345,678`.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Digits in the largest line number.
fn group_digits(lines: u64) -> usize {
    lines.max(1).to_string().len().max(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_counts_read_well() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(5 << 20), "5.0 MB");
        assert_eq!(format_size(3 << 30), "3.0 GB");
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_234_567), "1,234,567");
    }
}
