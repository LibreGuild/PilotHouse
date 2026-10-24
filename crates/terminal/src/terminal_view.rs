//! Draws one [`Terminal`] as styled text lines and turns keys, the mouse
//! and the clipboard into input. The terminal itself (shell and screen)
//! lives outside the view, so closing the view leaves the shell running.

use std::cell::Cell;
use std::rc::Rc;

use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, Font,
    FontStyle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, Pixels, Render, ScrollWheelEvent, StrikethroughStyle, Styled as _,
    StyledText, Subscription, TextRun, UnderlineStyle, Window, canvas, div, font, px,
};

use crate::palette::Palette;
use crate::terminal::{Listener, Terminal};
use crate::{TerminalAsk, TerminalCopy, TerminalPaste, keys};

pub const TERMINAL_CONTEXT: &str = "Terminal";

#[derive(Debug, Clone)]
pub enum TerminalViewEvent {
    /// Ask the agent about this text (the selection, or the last screenful).
    Ask(String),
}

pub struct TerminalView {
    terminal: Entity<Terminal>,
    focus_handle: FocusHandle,
    /// Last laid-out bounds, written during prepaint, so a resize can be
    /// detected without measuring in `render`.
    last_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// A cell's width and height as last drawn, to map the mouse to cells.
    cell: Cell<(f32, f32)>,
    /// A mouse drag is selecting text.
    selecting: bool,
    _observe: Subscription,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl TerminalView {
    pub fn new(terminal: Entity<Terminal>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&terminal, |_, _, cx| cx.notify()),
            terminal,
            focus_handle: cx.focus_handle(),
            last_bounds: Rc::default(),
            cell: Cell::new((0., 0.)),
            selecting: false,
        }
    }

    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.send_keystroke(&event.keystroke, cx);
    }

    fn key_action(&mut self, action: &crate::TerminalKey, _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(keystroke) = gpui_kit::Keystroke::parse(&action.key) {
            self.send_keystroke(&keystroke, cx);
        }
    }

    /// Sends what `keystroke` types to the shell.
    fn send_keystroke(&mut self, keystroke: &gpui_kit::Keystroke, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let app_cursor = terminal.term().lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = keys::to_bytes(keystroke, app_cursor) {
            // Typing jumps back to the live screen, like every terminal does,
            // and drops the selection.
            let mut term = terminal.term().lock();
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
            drop(term);
            terminal.write(bytes);
            cx.stop_propagation();
        }
    }

    /// The cell under `position`, and which half of it.
    fn cell_at(&self, position: gpui_kit::Point<Pixels>, cx: &App) -> Option<(Point, Side)> {
        let bounds = self.last_bounds.get()?;
        let (width, height) = self.cell.get();
        if width <= 0. || height <= 0. {
            return None;
        }
        let terminal = self.terminal.read(cx);
        let (columns, rows) = terminal.size();
        let x = (f32::from(position.x - bounds.origin.x) / width).max(0.);
        let y = (f32::from(position.y - bounds.origin.y) / height).max(0.);
        let column = (x.floor() as usize).min(columns.saturating_sub(1));
        let row = (y.floor() as usize).min(rows.saturating_sub(1));
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let offset = terminal.term().lock().grid().display_offset() as i32;
        Some((Point::new(Line(row as i32 - offset), Column(column)), side))
    }

    fn mouse_down(
        &mut self,
        event: &gpui_kit::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        let Some((point, side)) = self.cell_at(event.position, cx) else {
            return;
        };
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.terminal.read(cx).term().lock().selection = Some(Selection::new(kind, point, side));
        self.selecting = true;
        cx.notify();
    }

    fn mouse_move(
        &mut self,
        event: &gpui_kit::MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some((point, side)) = self.cell_at(event.position, cx) else {
            return;
        };
        let updated = match self.terminal.read(cx).term().lock().selection.as_mut() {
            Some(selection) => {
                selection.update(point, side);
                true
            }
            None => false,
        };
        if updated {
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &gpui_kit::MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut term = self.terminal.read(cx).term().lock();
        // A click without a drag selects nothing.
        if term.selection.as_ref().is_some_and(Selection::is_empty) {
            term.selection = None;
        }
        drop(term);
        cx.notify();
    }

    fn copy(&mut self, _: &TerminalCopy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal.read(cx).selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// The selection, else what is on screen (the end of it), for the chat.
    fn ask(&mut self, _: &TerminalAsk, _: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let text = terminal
            .selected_text()
            .unwrap_or_else(|| terminal.visible_text());
        let text = text.trim().to_string();
        if !text.is_empty() {
            cx.emit(TerminalViewEvent::Ask(text));
        }
    }

    fn paste(&mut self, _: &TerminalPaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.terminal.read(cx).paste(&text);
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line_height =
            line_height(px(ph_theme::preferences::preferences(cx).terminal_font_size));
        let delta = event.delta.pixel_delta(line_height).y;
        let lines = (delta / line_height).round() as i32;
        if lines != 0 {
            self.terminal
                .read(cx)
                .term()
                .lock()
                .scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// Resizes the grid and the PTY to fit `bounds`.
    fn fit(
        &mut self,
        bounds: Bounds<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        let columns = ((bounds.size.width / cell_width).floor() as usize).max(2);
        let rows = ((bounds.size.height / line_height).floor() as usize).max(1);
        let cell = (f32::from(cell_width) as u16, f32::from(line_height) as u16);
        self.terminal
            .update(cx, |terminal, _| terminal.resize(columns, rows, cell));
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn line_height(font_size: Pixels) -> Pixels {
    (font_size * 1.35).round()
}

/// The visible screen as one styled line per row.
struct ScreenLine {
    text: String,
    runs: Vec<TextRun>,
}

fn screen_lines(
    term: &Term<Listener>,
    palette: &Palette,
    base: &Font,
    focused: bool,
) -> Vec<ScreenLine> {
    let content = term.renderable_content();
    let cursor = content.cursor.point;
    let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
    let rows = term.screen_lines();
    let offset = content.display_offset as i32;
    let mut lines: Vec<ScreenLine> = (0..rows)
        .map(|_| ScreenLine {
            text: String::new(),
            runs: Vec::new(),
        })
        .collect();

    for indexed in content.display_iter {
        let cell = &indexed.cell;
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let row = (indexed.point.line.0 + offset) as usize;
        let Some(line) = lines.get_mut(row) else {
            continue;
        };

        let mut fg = palette.resolve(cell.fg, content.colors);
        let mut bg = palette.resolve(cell.bg, content.colors);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::DIM) {
            fg = fg.opacity(0.66);
        }
        let at_cursor = show_cursor && indexed.point == cursor && content.display_offset == 0;
        let selected = content
            .selection
            .is_some_and(|range| range.contains(indexed.point));
        let background = if selected {
            Some(palette.selection)
        } else if at_cursor && focused {
            fg = palette.background;
            Some(palette.cursor)
        } else if at_cursor {
            Some(palette.cursor.opacity(0.35))
        } else if bg != palette.background {
            Some(bg)
        } else {
            None
        };

        let ch = if cell.flags.contains(Flags::HIDDEN) || cell.c == '\0' {
            ' '
        } else {
            cell.c
        };
        let run = TextRun {
            len: ch.len_utf8(),
            font: Font {
                weight: if cell.flags.contains(Flags::BOLD) {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                style: if cell.flags.contains(Flags::ITALIC) {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
                ..base.clone()
            },
            color: fg,
            background_color: background,
            underline: cell
                .flags
                .intersects(Flags::ALL_UNDERLINES)
                .then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(fg),
                    wavy: cell.flags.contains(Flags::UNDERCURL),
                }),
            strikethrough: cell
                .flags
                .contains(Flags::STRIKEOUT)
                .then(|| StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(fg),
                }),
        };
        line.text.push(ch);
        push_run(&mut line.runs, run);
    }
    lines
}

/// Appends `run`, merging it into the previous one when the style matches.
fn push_run(runs: &mut Vec<TextRun>, run: TextRun) {
    if let Some(last) = runs.last_mut()
        && last.font == run.font
        && last.color == run.color
        && last.background_color == run.background_color
        && last.underline == run.underline
        && last.strikethrough == run.strikethrough
    {
        last.len += run.len;
        return;
    }
    runs.push(run);
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let palette = Palette::from_theme(theme);
        let font_size = px(ph_theme::preferences::preferences(cx).terminal_font_size);
        let line_height = line_height(font_size);
        let base = font(theme.mono_font_family.clone());
        let font_id = window.text_system().resolve_font(&base);
        let cell_width = window
            .text_system()
            .advance(font_id, font_size, 'm')
            .map(|size| size.width)
            .unwrap_or(font_size * 0.6);

        self.cell
            .set((f32::from(cell_width), f32::from(line_height)));
        let focused = self.focus_handle.is_focused(window);
        let lines = screen_lines(
            &self.terminal.read(cx).term().lock(),
            &palette,
            &base,
            focused,
        );
        let background: Hsla = palette.background;

        // Report our bounds after layout; resize on the next frame if needed.
        let entity = cx.weak_entity();
        let last_bounds = self.last_bounds.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                if last_bounds.get() != Some(bounds) {
                    last_bounds.set(Some(bounds));
                    cx.defer(move |cx| {
                        _ = entity.update(cx, |this, cx| {
                            this.fit(bounds, cell_width, line_height, cx);
                            cx.notify();
                        });
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        div()
            .id("terminal")
            .key_context(TERMINAL_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::key_action))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::ask))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .relative()
            .size_full()
            .px_2()
            .py_1()
            .bg(background)
            .overflow_hidden()
            .font_family(base.family.clone())
            .text_size(font_size)
            .line_height(line_height)
            .child(
                div()
                    .relative()
                    .size_full()
                    .child(measure)
                    .children(lines.into_iter().map(|line| {
                        div()
                            .h(line_height)
                            .whitespace_nowrap()
                            .child(StyledText::new(line.text).with_runs(line.runs))
                    })),
            )
    }
}
