//! Markdown as PilotHouse shows it: each line of a paragraph on its own
//! line (as typed, like GitHub comments and chat apps), and a copy button
//! on every code block ([`code_actions`]).

use gpui_kit::base::text::CodeBlock;
use gpui_kit::component::{ActiveTheme as _, clipboard::Clipboard, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, IntoElement, ParentElement as _, Styled as _, Window};

/// Turns the line endings inside paragraphs into hard breaks, so a single
/// newline shows as a new line instead of joining the two lines. Code
/// blocks keep their text as it is. Fed in pieces (a streaming reply), it
/// remembers whether it is inside a code block.
#[derive(Debug, Default, Clone)]
pub struct LineBreaks {
    /// The open fence: its character and length.
    fence: Option<(char, usize)>,
    /// The line read so far (up to its newline).
    line: String,
    /// A carriage return the line ended with, not written yet (its
    /// newline may come in the next piece).
    held_cr: bool,
    /// The previous line was blank (an indented code block may start).
    after_blank: bool,
    /// Inside a code block made by indenting four spaces.
    indented_code: bool,
}

impl LineBreaks {
    /// Appends `chunk`, with its breaks kept, to `out`.
    pub fn push(&mut self, chunk: &str, out: &mut String) {
        for piece in chunk.split_inclusive('\n') {
            let (body, newline) = match piece.strip_suffix('\n') {
                Some(body) => (body, true),
                None => (piece, false),
            };
            // A carriage return waits: before a newline it ends the line.
            if self.held_cr && !body.is_empty() {
                out.push('\r');
                self.line.push('\r');
                self.held_cr = false;
            }
            let (body, cr) = match body.strip_suffix('\r') {
                Some(body) => (body, true),
                None => (body, false),
            };
            self.line.push_str(body);
            out.push_str(body);
            if cr {
                self.held_cr = true;
            }
            if !newline {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let in_code = self.fence.is_some();
            self.track_fence(&line);
            let blank = line.trim().is_empty();
            let indented = line.starts_with("    ") || line.starts_with('\t');
            if !in_code && self.fence.is_none() {
                self.indented_code = indented && (self.indented_code || self.after_blank)
                    || (self.indented_code && blank);
            }
            self.after_blank = blank;
            let hard = !in_code
                && self.fence.is_none()
                && !self.indented_code
                && !blank
                && !line.ends_with("  ")
                && !line.ends_with('\\');
            if hard {
                out.push_str("  ");
            }
            if std::mem::take(&mut self.held_cr) {
                out.push('\r');
            }
            out.push('\n');
        }
    }

    /// Opens or closes a fenced code block on `line` (also inside a quote
    /// or a list item: `> ```` and `- ````).
    fn track_fence(&mut self, line: &str) {
        let mut text = line.trim_start();
        loop {
            let before = text;
            text = text.trim_start_matches('>').trim_start();
            for marker in ["- ", "* ", "+ "] {
                if let Some(rest) = text.strip_prefix(marker) {
                    text = rest.trim_start();
                }
            }
            let digits = text.chars().take_while(char::is_ascii_digit).count();
            if digits > 0
                && let Some(rest) = text[digits..]
                    .strip_prefix(". ")
                    .or_else(|| text[digits..].strip_prefix(") "))
            {
                text = rest.trim_start();
            }
            if text == before {
                break;
            }
        }
        let Some(mark) = text.chars().next().filter(|c| *c == '`' || *c == '~') else {
            return;
        };
        let count = text.chars().take_while(|c| *c == mark).count();
        if count < 3 {
            return;
        }
        match self.fence {
            None => self.fence = Some((mark, count)),
            Some((open, len))
                if open == mark && count >= len && text[count..].trim().is_empty() =>
            {
                self.fence = None
            }
            Some(_) => {}
        }
    }
}

/// `text` with its line breaks kept (see [`LineBreaks`]).
pub fn keep_line_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    LineBreaks::default().push(text, &mut out);
    out
}

/// The corner of a code block: its language and a copy button (a tick for
/// a moment once copied). For `TextView::code_block_actions`.
pub fn code_actions(block: &CodeBlock, _: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    h_flex()
        .gap_1()
        .items_center()
        .when_some(
            block.lang().filter(|lang| !lang.is_empty()),
            |this, lang| {
                this.child(
                    gpui_kit::div()
                        .pl_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(lang),
                )
            },
        )
        .child(
            Clipboard::new("copy-code")
                .value(block.code())
                .tooltip("Copy")
                .accessibility_label("Copy the code"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_newlines_become_breaks() {
        assert_eq!(keep_line_breaks("one\ntwo\n"), "one  \ntwo  \n");
        assert_eq!(keep_line_breaks("one\n\ntwo"), "one  \n\ntwo");
        assert_eq!(keep_line_breaks("a  \nb\\\nc"), "a  \nb\\\nc");
        assert_eq!(keep_line_breaks("a\r\nb"), "a  \r\nb");
    }

    #[test]
    fn code_is_left_alone() {
        let text = "x\n```rust\nlet a = 1;\nlet b = 2;\n```\ny\n";
        assert_eq!(
            keep_line_breaks(text),
            "x  \n```rust\nlet a = 1;\nlet b = 2;\n```\ny  \n"
        );
        // A shorter or different fence does not close it.
        let text = "````\n```\nin\n````\nout\n";
        assert_eq!(keep_line_breaks(text), "````\n```\nin\n````\nout  \n");
    }

    #[test]
    fn carriage_returns_split_from_their_newline() {
        let mut breaks = LineBreaks::default();
        let mut out = String::new();
        for piece in ["a\r", "\nb\r\n"] {
            breaks.push(piece, &mut out);
        }
        assert_eq!(out, keep_line_breaks("a\r\nb\r\n"));
        assert_eq!(out, "a  \r\nb  \r\n");
    }

    #[test]
    fn code_in_quotes_lists_and_indents_is_left_alone() {
        assert_eq!(
            keep_line_breaks("> ```\n> a\n> ```\nx\n"),
            "> ```\n> a\n> ```\nx  \n"
        );
        assert_eq!(
            keep_line_breaks("- ```\n  a\n  ```\n"),
            "- ```\n  a\n  ```\n"
        );
        assert_eq!(
            keep_line_breaks("text\n\n    code\n    more\n\nafter\n"),
            "text  \n\n    code\n    more\n\nafter  \n"
        );
    }

    #[test]
    fn pieces_read_as_one() {
        let text = "a\nb\n```\ncode\n```\nc\n";
        let mut breaks = LineBreaks::default();
        let mut out = String::new();
        for piece in ["a", "\nb\n``", "`\nco", "de\n```", "\nc\n"] {
            breaks.push(piece, &mut out);
        }
        assert_eq!(out, keep_line_breaks(text));
    }
}
