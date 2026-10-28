//! Logs with their terminal colours: ANSI escape codes become coloured
//! text instead of `^[[31m` noise. The view can follow the end as the file
//! grows.

use std::ops::Range;

use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    App, Context, FontWeight, HighlightStyle, Hsla, IntoElement, ParentElement as _, Render,
    ScrollStrategy, SharedString, Styled as _, StyledText, UniformListScrollHandle, Window, div,
    px, rgb, uniform_list,
};

/// A run of a line in one style.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    /// One of the 16 terminal colours.
    Basic(u8),
    Rgb(u8, u8, u8),
}

/// A line's text without escapes, and its styled ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub runs: Vec<(Range<usize>, Style)>,
}

/// Splits `text` into lines, reading SGR codes and dropping other escapes.
/// The style carries on from line to line, as in a terminal.
pub fn parse(text: &str) -> Vec<Line> {
    let mut style = Style::default();
    text.lines()
        .map(|line| {
            let mut out = String::with_capacity(line.len());
            let mut runs = Vec::new();
            let mut start = 0;
            let mut chars = line.char_indices().peekable();
            while let Some((_, c)) = chars.next() {
                if c != '\u{1b}' {
                    if c != '\r' {
                        out.push(c);
                    }
                    continue;
                }
                // ESC ] … BEL (or ESC \\): a title or a link's address, not
                // text.
                if chars.peek().map(|(_, c)| *c) == Some(']') {
                    let mut escape = false;
                    for (_, c) in chars.by_ref() {
                        if c == '\u{7}' || (escape && c == '\\') {
                            break;
                        }
                        escape = c == '\u{1b}';
                    }
                    continue;
                }
                // ESC [ params letter
                if chars.peek().map(|(_, c)| *c) != Some('[') {
                    chars.next();
                    continue;
                }
                chars.next();
                let mut params = String::new();
                let mut letter = None;
                for (_, c) in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        letter = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if letter == Some('m') {
                    if out.len() > start && style != Style::default() {
                        runs.push((start..out.len(), style));
                    }
                    apply(&mut style, &params);
                    start = out.len();
                }
            }
            if out.len() > start && style != Style::default() {
                runs.push((start..out.len(), style));
            }
            Line { text: out, runs }
        })
        .collect()
}

fn apply(style: &mut Style, params: &str) {
    let codes: Vec<u16> = params
        .split(';')
        .map(|code| code.parse().unwrap_or(0))
        .collect();
    let mut ix = 0;
    while ix < codes.len() {
        match codes[ix] {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => (style.bold, style.dim) = (false, false),
            23 => style.italic = false,
            24 => style.underline = false,
            code @ 30..=37 => style.fg = Some(Color::Basic(code as u8 - 30)),
            code @ 90..=97 => style.fg = Some(Color::Basic(code as u8 - 90 + 8)),
            39 => style.fg = None,
            code @ 40..=47 => style.bg = Some(Color::Basic(code as u8 - 40)),
            code @ 100..=107 => style.bg = Some(Color::Basic(code as u8 - 100 + 8)),
            49 => style.bg = None,
            code @ (38 | 48) => {
                let color = match codes.get(ix + 1) {
                    Some(5) => {
                        ix += 2;
                        codes.get(ix).map(|n| palette_256(*n as u8))
                    }
                    Some(2) => {
                        ix += 4;
                        match codes.get(ix - 2..=ix) {
                            Some([r, g, b]) => Some(Color::Rgb(*r as u8, *g as u8, *b as u8)),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if code == 38 {
                    style.fg = color;
                } else {
                    style.bg = color;
                }
            }
            _ => {}
        }
        ix += 1;
    }
}

fn palette_256(n: u8) -> Color {
    match n {
        0..=15 => Color::Basic(n),
        16..=231 => {
            let n = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color::Rgb(level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let gray = 8 + (n - 232) * 10;
            Color::Rgb(gray, gray, gray)
        }
    }
}

/// The 16 colours, readable on the app's own background.
fn to_hsla(color: Color, dark: bool) -> Hsla {
    let hex = match color {
        Color::Rgb(r, g, b) => {
            return rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into();
        }
        Color::Basic(n) => match (n, dark) {
            (0, _) => 0x4b5563,
            (1, true) => 0xf87171,
            (1, false) => 0xc0262d,
            (2, true) => 0x4ade80,
            (2, false) => 0x178a3f,
            (3, true) => 0xfacc15,
            (3, false) => 0x9a6700,
            (4, true) => 0x60a5fa,
            (4, false) => 0x1d4ed8,
            (5, true) => 0xe879f9,
            (5, false) => 0xa21caf,
            (6, true) => 0x22d3ee,
            (6, false) => 0x0e7490,
            (7, _) => 0x9ca3af,
            (8, _) => 0x6b7280,
            (9, _) => 0xef4444,
            (10, _) => 0x22c55e,
            (11, _) => 0xeab308,
            (12, _) => 0x3b82f6,
            (13, _) => 0xd946ef,
            (14, _) => 0x06b6d4,
            _ => 0xe5e7eb,
        },
    };
    rgb(hex).into()
}

/// The styled text of `line`.
pub fn styled(line: &Line, dark: bool) -> StyledText {
    let highlights = line.runs.iter().map(|(range, style)| {
        (
            range.clone(),
            HighlightStyle {
                color: style.fg.map(|color| to_hsla(color, dark)),
                background_color: style.bg.map(|color| to_hsla(color, dark).opacity(0.35)),
                font_weight: style.bold.then_some(FontWeight::BOLD),
                font_style: style.italic.then_some(gpui_kit::FontStyle::Italic),
                fade_out: style.dim.then_some(0.4),
                underline: style.underline.then(|| gpui_kit::UnderlineStyle {
                    thickness: px(1.),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
    });
    StyledText::new(SharedString::from(line.text.clone())).with_highlights(highlights)
}

/// A log file, coloured, optionally following its end.
pub struct LogPreview {
    lines: std::sync::Arc<Vec<Line>>,
    follow: bool,
    scroll: UniformListScrollHandle,
}

impl Default for LogPreview {
    fn default() -> Self {
        Self {
            lines: Default::default(),
            follow: true,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

impl LogPreview {
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.lines = std::sync::Arc::new(parse(text));
        if self.follow {
            let last = self.lines.len().saturating_sub(1);
            self.scroll.scroll_to_item(last, ScrollStrategy::Bottom);
        }
        cx.notify();
    }
}

impl Render for LogPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let dark = theme.is_dark();
        let lines = self.lines.clone();
        let count = lines.len();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_none()
                    .h_8()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child(format!("{count} lines")))
                    .child(
                        Button::new("log-follow")
                            .ghost()
                            .xsmall()
                            .label("Follow the end")
                            .selected(self.follow)
                            .tooltip("Keep the newest lines in view as the file grows")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.follow = !this.follow;
                                if this.follow {
                                    let last = this.lines.len().saturating_sub(1);
                                    this.scroll.scroll_to_item(last, ScrollStrategy::Bottom);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                uniform_list(
                    "log-lines",
                    count,
                    move |range: Range<usize>, _, cx: &mut App| {
                        let theme = cx.theme();
                        range
                            .map(|ix| {
                                div()
                                    .px_3()
                                    .whitespace_nowrap()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_sm()
                                    .child(styled(&lines[ix], dark))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0()
                .py_1(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_colours_and_drops_other_escapes() {
        let lines = parse("\u{1b}[1;31mERROR\u{1b}[0m done\u{1b}[2K\nplain");
        assert_eq!(lines[0].text, "ERROR done");
        assert_eq!(lines[0].runs.len(), 1);
        let (range, style) = &lines[0].runs[0];
        assert_eq!(range.clone(), 0..5);
        assert!(style.bold);
        assert_eq!(style.fg, Some(Color::Basic(1)));
        assert_eq!(lines[1].text, "plain");
        assert!(lines[1].runs.is_empty());
        // A link (OSC 8) shows its text only; a title (OSC 0) nothing.
        let lines =
            parse("\u{1b}]8;;https://x.dev\u{1b}\\docs\u{1b}]8;;\u{1b}\\ \u{1b}]0;title\u{7}end");
        assert_eq!(lines[0].text, "docs end");
    }

    #[test]
    fn reads_256_and_true_colour() {
        let lines = parse("\u{1b}[38;5;196mred\u{1b}[48;2;1;2;3mbg\u{1b}[m");
        assert_eq!(lines[0].runs[0].1.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(lines[0].runs[1].1.bg, Some(Color::Rgb(1, 2, 3)));
    }
}
