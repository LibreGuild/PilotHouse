//! The text of a tooltip, kept narrow.
//!
//! A tooltip is one line as wide as its text: a path, or a sentence with a
//! path in it, ran across the whole window and off its edge. Tooltips grow
//! downwards instead: their text is broken into lines of at most
//! [`COLUMNS`] characters, at spaces where there are any, and inside a word
//! that is longer than a line (a path, a URL, an id), after a separator
//! where one is near.
//!
//! The lines are broken in the text itself, since the toolkit lays a
//! tooltip's text out as it comes and gives no width to wrap it at. Every
//! tooltip whose text is made up as the app runs (a title, a path, a
//! command, a state) goes through [`tip`]; so do the few fixed ones longer
//! than a line.

use gpui_kit::SharedString;

/// The most characters on a tooltip's line: about 340 pixels of the
/// interface's small text.
pub const COLUMNS: usize = 52;

/// Where a word too long for a line is best broken: after one of these.
const SEPARATORS: [char; 8] = ['/', '\\', '-', '_', '.', ':', ',', '='];

/// `text` as a tooltip shows it: in lines of at most [`COLUMNS`]
/// characters. Line ends already in it are kept.
pub fn tip(text: impl Into<SharedString>) -> SharedString {
    let text: SharedString = text.into();
    if text.lines().all(|line| line.chars().count() <= COLUMNS) {
        return text;
    }
    let mut out = String::with_capacity(text.len() + text.len() / COLUMNS + 1);
    for (n, line) in text.lines().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        wrap_line(line, &mut out);
    }
    out.into()
}

/// Adds `line` to `out`, broken into lines of at most [`COLUMNS`].
fn wrap_line(line: &str, out: &mut String) {
    // The characters on the line being filled.
    let mut width = 0;
    for word in line.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        // A space before the word, unless it starts a line.
        if width > 0 {
            let fits = width + 1 + word.len() <= COLUMNS;
            // A word too long for any line goes on from here when there is
            // real room left, rather than leaving most of a line empty.
            let goes_on = word.len() > COLUMNS && COLUMNS - width > 12;
            if fits || goes_on {
                out.push(' ');
                width += 1;
            } else {
                out.push('\n');
                width = 0;
            }
        }
        // Longer than what is left of the line: broken, each piece ending
        // after a separator where one is near the line's end.
        while word.len() > COLUMNS - width {
            let room = COLUMNS - width;
            if room == 0 {
                out.push('\n');
                width = 0;
                continue;
            }
            let cut = (room / 2..room)
                .rev()
                .find(|at| SEPARATORS.contains(&word[*at]))
                .map_or(room, |at| at + 1);
            out.extend(word.drain(..cut));
            out.push('\n');
            width = 0;
        }
        width += word.len();
        out.extend(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widest(text: &str) -> usize {
        text.lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn a_short_tooltip_is_as_it_was() {
        assert_eq!(tip("Stop").as_ref(), "Stop");
        assert_eq!(
            tip("Mode: Auto review (⇧Tab switches)").as_ref(),
            "Mode: Auto review (⇧Tab switches)"
        );
        assert_eq!(tip("two\nlines").as_ref(), "two\nlines");
        assert_eq!(tip("").as_ref(), "");
    }

    #[test]
    fn a_sentence_grows_downwards() {
        let said = tip(
            "Each runs the agent's own updater in a terminal of the workspace in front, where it is seen to run.",
        );
        assert!(widest(&said) <= COLUMNS, "{said}");
        assert!(said.lines().count() >= 2);
        // Nothing is lost or added but the line ends.
        assert_eq!(
            said.replace('\n', " "),
            "Each runs the agent's own updater in a terminal of the workspace in front, where it is seen to run."
        );
        // Words are not cut where a space would do.
        for line in said.lines() {
            assert!(!line.starts_with(' ') && !line.ends_with(' '), "{line:?}");
        }
    }

    /// What is left of `text` without spaces and line ends: wrapping
    /// changes nothing else.
    fn letters(text: &str) -> String {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn a_long_path_is_broken_after_its_separators() {
        let path = "/private/tmp/claude-501/-Users-someone-Tech-Project/b8d79a31-7117-4bf6-ad52-9d67cd2fac6b/scratchpad/explain/notes-api";
        let whole = format!("2+3 · Waits for a look · Codex · in {path}");
        let said = tip(whole.clone());
        assert!(widest(&said) <= COLUMNS, "{said}");
        assert_eq!(letters(&said), letters(&whole), "all of it, in order");
        // A piece of the path ends after a separator, not mid-name, where
        // one was near.
        let pieces: Vec<&str> = said.lines().collect();
        assert!(pieces.len() >= 3, "{said}");
        assert!(
            pieces[1..pieces.len() - 1]
                .iter()
                .all(|piece| piece.ends_with(SEPARATORS)),
            "{said}"
        );
    }

    #[test]
    fn a_string_with_nowhere_to_break_is_cut_at_the_line() {
        let said = tip("x".repeat(130));
        assert_eq!(widest(&said), COLUMNS);
        assert_eq!(said.replace('\n', ""), "x".repeat(130));
        // Wide characters count as one each; nothing is cut inside one.
        let said = tip("é".repeat(60));
        assert_eq!(said.replace('\n', ""), "é".repeat(60));
        assert!(widest(&said) <= COLUMNS);
    }
}
