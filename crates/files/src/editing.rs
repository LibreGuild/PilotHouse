//! Line editing commands of the code editor, as VS Code has them: toggle a
//! comment, move, copy or delete lines, open a line above or below, and
//! delete a word without running into the line before.
//!
//! Each command reads the text and the selection (byte offsets) and says
//! what to replace and what to select after, so it is tested without an
//! editor. They act on the main cursor or selection.

use std::ops::Range;

/// A change: replace `range` with `insert`, then select `select` (offsets
/// in the changed text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub insert: String,
    pub select: Range<usize>,
}

/// How a language comments out a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comment {
    /// `// line`
    Line(&'static str),
    /// `/* line */`
    Block(&'static str, &'static str),
}

/// The comment style for a file name, if the language has one.
pub fn comment_for(name: &str) -> Option<Comment> {
    let lower = name.to_ascii_lowercase();
    let extension = lower
        .rsplit_once('.')
        .map_or(lower.as_str(), |(_, ext)| ext);
    Some(match extension {
        "rs" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "cs" | "java" | "kt" | "kts"
        | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "go" | "swift"
        | "scala" | "dart" | "groovy" | "gradle" | "proto" | "zig" | "jsonc" | "m" | "mm"
        | "php" => Comment::Line("//"),
        "py" | "sh" | "bash" | "zsh" | "fish" | "rb" | "yaml" | "yml" | "toml" | "r" | "pl"
        | "ps1" | "makefile" | "dockerfile" | "cmake" | "conf" | "ini" | "properties"
        | "gitignore" | "env" | "tf" | "nix" | "ex" | "exs" | "jl" => Comment::Line("#"),
        "sql" | "lua" | "hs" | "elm" => Comment::Line("--"),
        "html" | "htm" | "xml" | "svg" | "md" | "markdown" | "vue" | "xaml" | "csproj"
        | "plist" => Comment::Block("<!--", "-->"),
        "css" | "scss" | "less" => Comment::Block("/*", "*/"),
        "bat" | "cmd" => Comment::Line("REM"),
        "vim" => Comment::Line("\""),
        "lisp" | "clj" | "el" | "scm" => Comment::Line(";"),
        _ => return None,
    })
}

/// Where the line holding `offset` starts.
fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |at| at + 1)
}

/// Where the line holding `offset` ends (before its `\n`, and a `\r`).
fn line_end(text: &str, offset: usize) -> usize {
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |at| offset + at);
    if end > offset && text.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// After the line ending that follows `offset` (or the text's end).
fn next_line_start(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map_or(text.len(), |at| offset + at + 1)
}

/// The whole lines a selection covers: a selection ending at the start of
/// a line does not take that line.
fn covered_lines(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = line_start(text, selection.start);
    let mut end = selection.end;
    if selection.end > selection.start && end == line_start(text, end) {
        end -= 1;
    }
    start..line_end(text, end.max(start))
}

/// What Ctrl+Backspace (⌥⌫) deletes, when it is not simply the word before
/// the cursor (`None`: the editor's own word deletion does that):
/// at a line's start, only the line break; with only spaces before the
/// cursor, the spaces; after a run of several spaces, the run.
pub fn delete_word_left(text: &str, cursor: usize) -> Option<Range<usize>> {
    let start = line_start(text, cursor);
    if cursor == start {
        if start == 0 {
            return None;
        }
        // The line break, \r\n included.
        let mut from = start - 1;
        if from > 0 && text.as_bytes()[from - 1] == b'\r' {
            from -= 1;
        }
        return Some(from..cursor);
    }
    let before = &text[start..cursor];
    if before.trim().is_empty() {
        return Some(start..cursor);
    }
    let spaces = before.len() - before.trim_end().len();
    (spaces > 1).then(|| cursor - spaces..cursor)
}

/// What Ctrl+Delete deletes, when it is not simply the word after the
/// cursor (see [`delete_word_left`]).
pub fn delete_word_right(text: &str, cursor: usize) -> Option<Range<usize>> {
    let end = line_end(text, cursor);
    if cursor == end {
        let next = next_line_start(text, cursor);
        return (next > cursor).then_some(cursor..next);
    }
    let after = &text[cursor..end];
    if after.trim().is_empty() {
        return Some(cursor..end);
    }
    let spaces = after.len() - after.trim_start().len();
    (spaces > 1).then(|| cursor..cursor + spaces)
}

/// Comments out the selected lines, or uncomments them when every line
/// with text is commented already. Blank lines are left alone; line
/// comments go at the lines' shared indentation, as VS Code puts them.
pub fn toggle_comment(text: &str, selection: Range<usize>, comment: Comment) -> Edit {
    let lines = covered_lines(text, &selection);
    let block = &text[lines.clone()];
    let rows: Vec<&str> = block.split('\n').collect();
    let has_text = |row: &str| !row.trim().is_empty();
    let indent = rows
        .iter()
        .filter(|row| has_text(row))
        .map(|row| row.len() - row.trim_start().len())
        .min()
        .unwrap_or(0);
    let commented = |row: &str| {
        let body = row.trim_start();
        match comment {
            Comment::Line(token) => body.starts_with(token),
            Comment::Block(open, close) => {
                body.starts_with(open) && body.trim_end_matches('\r').trim_end().ends_with(close)
            }
        }
    };
    let remove = rows
        .iter()
        .filter(|row| has_text(row))
        .all(|row| commented(row))
        && rows.iter().any(|row| has_text(row));
    let changed: Vec<String> = rows
        .iter()
        .map(|row| {
            if !has_text(row) {
                return row.to_string();
            }
            let (row, cr) = match row.strip_suffix('\r') {
                Some(row) => (row, "\r"),
                None => (*row, ""),
            };
            let lead = row.len() - row.trim_start().len();
            let body = &row[lead..];
            let changed = match (comment, remove) {
                (Comment::Line(token), true) => {
                    let rest = &body[token.len()..];
                    format!("{}{}", &row[..lead], rest.strip_prefix(' ').unwrap_or(rest))
                }
                (Comment::Line(token), false) => {
                    format!("{}{token} {}", &row[..indent], &row[indent..])
                }
                (Comment::Block(open, close), true) => {
                    let inner = body[open.len()..].trim_end();
                    let inner = &inner[..inner.len() - close.len()];
                    let inner = inner.strip_prefix(' ').unwrap_or(inner);
                    format!(
                        "{}{}",
                        &row[..lead],
                        inner.strip_suffix(' ').unwrap_or(inner)
                    )
                }
                (Comment::Block(open, close), false) => {
                    format!("{}{open} {body} {close}", &row[..lead])
                }
            };
            format!("{changed}{cr}")
        })
        .collect();
    let insert = changed.join("\n");
    let select = if selection.is_empty() {
        // The cursor keeps its place in the line's text.
        let column = selection.start - lines.start;
        let first = rows.first().copied().unwrap_or_default();
        let shift = changed.first().map_or(0, |row| row.len()) as isize - first.len() as isize;
        let at = if column >= indent {
            (column as isize + shift).max(indent as isize) as usize
        } else {
            column
        };
        let at = lines.start + at.min(changed.first().map_or(0, |row| row.len()));
        at..at
    } else {
        lines.start..lines.start + insert.len()
    };
    Edit {
        range: lines,
        insert,
        select,
    }
}

/// Moves the selected lines one line up or down (⌥↑ / ⌥↓), keeping them
/// selected. `None` at the top or bottom.
pub fn move_lines(text: &str, selection: Range<usize>, up: bool) -> Option<Edit> {
    let lines = covered_lines(text, &selection);
    let block = &text[lines.clone()];
    if up {
        if lines.start == 0 {
            return None;
        }
        let above_start = line_start(text, lines.start - 1);
        let above = &text[above_start..line_end(text, above_start)];
        let newline = &text[above_start + above.len()..lines.start];
        let insert = format!("{block}{newline}{above}");
        let shift = lines.start - above_start;
        Some(Edit {
            range: above_start..lines.end,
            insert,
            select: selection.start - shift..selection.end - shift,
        })
    } else {
        let next = next_line_start(text, lines.end);
        if next >= text.len() && next == lines.end {
            return None;
        }
        if lines.end == text.len() {
            return None;
        }
        let below = &text[next..line_end(text, next)];
        let newline = &text[lines.end..next];
        let insert = format!("{below}{newline}{block}");
        let shift = below.len() + newline.len();
        Some(Edit {
            range: lines.start..next + below.len(),
            insert,
            select: selection.start + shift..selection.end + shift,
        })
    }
}

/// Copies the selected lines below them (`down`, the copy selected) or
/// above them (⇧⌥↓ / ⇧⌥↑).
pub fn copy_lines(text: &str, selection: Range<usize>, down: bool) -> Edit {
    let lines = covered_lines(text, &selection);
    let block = &text[lines.clone()];
    let newline = if text[..lines.start].ends_with("\r\n") || text[lines.end..].starts_with("\r\n")
    {
        "\r\n"
    } else {
        "\n"
    };
    let shift = if down { block.len() + newline.len() } else { 0 };
    Edit {
        range: lines.end..lines.end,
        insert: format!("{newline}{block}"),
        select: selection.start + shift..selection.end + shift,
    }
}

/// Deletes the selected lines (⌘⇧K), the cursor going to the line after.
pub fn delete_lines(text: &str, selection: Range<usize>) -> Edit {
    let lines = covered_lines(text, &selection);
    let column = selection.start - lines.start;
    let (range, after) = if lines.end < text.len() {
        // The lines and the break after them.
        (lines.start..next_line_start(text, lines.end), lines.start)
    } else if lines.start > 0 {
        // The last line: the break before it.
        let mut from = lines.start - 1;
        if from > 0 && text.as_bytes()[from - 1] == b'\r' {
            from -= 1;
        }
        (from..lines.end, line_start(text, from))
    } else {
        (lines.clone(), 0)
    };
    // The same column on the line that takes their place.
    let rest = format!("{}{}", &text[..range.start], &text[range.end..]);
    let line_length = line_end(&rest, after.min(rest.len())) - after.min(rest.len());
    let at = after + column.min(line_length);
    Edit {
        range,
        insert: String::new(),
        select: at..at,
    }
}

/// Opens an empty line below (⌘↵) or above (⌘⇧↵) the cursor's, indented
/// like it, wherever the cursor is in the line.
pub fn open_line(text: &str, cursor: usize, below: bool) -> Edit {
    let start = line_start(text, cursor);
    let end = line_end(text, cursor);
    let line = &text[start..end];
    let indent = &line[..line.len() - line.trim_start().len()];
    if below {
        let insert = format!("\n{indent}");
        let at = end + insert.len();
        Edit {
            range: end..end,
            insert,
            select: at..at,
        }
    } else {
        let at = start + indent.len();
        Edit {
            range: start..start,
            insert: format!("{indent}\n"),
            select: at..at,
        }
    }
}

/// The next place `needle` appears after `from`, wrapping around to the
/// start; `None` when it appears nowhere else.
pub fn next_occurrence(
    text: &str,
    needle: &str,
    from: usize,
    skip: &Range<usize>,
) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let after = text[from..]
        .match_indices(needle)
        .map(|(at, _)| from + at)
        .chain(text[..from].match_indices(needle).map(|(at, _)| at))
        .map(|at| at..at + needle.len())
        .find(|range| range != skip)?;
    Some(after)
}

/// A 1-based `line` or `line:column` as typed in Go to Line, as 0-based
/// numbers.
pub fn parse_line_column(typed: &str) -> Option<(u32, u32)> {
    let mut parts = typed.trim().splitn(2, [':', ',']);
    let line: u32 = parts.next()?.trim().parse().ok()?;
    let column: u32 = match parts.next() {
        Some(column) if !column.trim().is_empty() => column.trim().parse().ok()?,
        _ => 1,
    };
    Some((line.max(1) - 1, column.max(1) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edit: &Edit) -> String {
        format!(
            "{}{}{}",
            &text[..edit.range.start],
            edit.insert,
            &text[edit.range.end..]
        )
    }

    #[test]
    fn word_deletion_stays_on_its_line() {
        let text = "fn a() {\n    \n    let x = 1;   \n}";
        // At a line's start: only the break.
        let third = text.find("    let").unwrap();
        assert_eq!(delete_word_left(text, third), Some(third - 1..third));
        // Only spaces before the cursor: the spaces, not the line above.
        let blank_end = text.find("\n    let").unwrap();
        let blank_start = text.find("\n    \n").unwrap() + 1;
        assert_eq!(
            delete_word_left(text, blank_end),
            Some(blank_start..blank_end)
        );
        // Several spaces after a word: the spaces.
        let after_spaces = text.find(";   ").unwrap() + 4;
        assert_eq!(
            delete_word_left(text, after_spaces),
            Some(after_spaces - 3..after_spaces)
        );
        // A word right before the cursor: the editor's own deletion.
        let after_word = text.find(" = 1").unwrap();
        assert_eq!(delete_word_left(text, after_word), None);
        assert_eq!(delete_word_left(text, 0), None);
        // Windows line breaks go whole.
        assert_eq!(delete_word_left("a\r\nb", 3), Some(1..3));
        // Forward: at the line's end, the break; trailing spaces; a word.
        let end_of_first = text.find('\n').unwrap();
        assert_eq!(
            delete_word_right(text, end_of_first),
            Some(end_of_first..end_of_first + 1)
        );
        assert_eq!(
            delete_word_right(text, after_spaces - 3),
            Some(after_spaces - 3..after_spaces)
        );
        assert_eq!(delete_word_right(text, third + 4), None);
        assert_eq!(delete_word_right(text, text.len()), None);
    }

    #[test]
    fn comments_toggle_at_the_shared_indent() {
        let text = "fn a() {\n    let x = 1;\n\n        y();\n}\n";
        let start = text.find("    let").unwrap();
        let end = text.find("y();").unwrap() + 4;
        let edit = toggle_comment(text, start..end, Comment::Line("//"));
        let commented = apply(text, &edit);
        assert_eq!(
            commented,
            "fn a() {\n    // let x = 1;\n\n    //     y();\n}\n"
        );
        // Again: back as it was.
        let again = toggle_comment(&commented, edit.select.clone(), Comment::Line("//"));
        assert_eq!(apply(&commented, &again), text);
        // One line, cursor inside: it keeps its place in the text.
        let cursor = text.find("x = 1").unwrap();
        let edit = toggle_comment(text, cursor..cursor, Comment::Line("//"));
        let changed = apply(text, &edit);
        assert_eq!(&changed[edit.select.start..edit.select.start + 5], "x = 1");
        // Block comments wrap each line.
        let css = "a {\n  color: red;\n}";
        let at = css.find("color").unwrap();
        let edit = toggle_comment(css, at..at, Comment::Block("/*", "*/"));
        let changed = apply(css, &edit);
        assert_eq!(changed, "a {\n  /* color: red; */\n}");
        let back = toggle_comment(&changed, at..at, Comment::Block("/*", "*/"));
        assert_eq!(apply(&changed, &back), css);
    }

    #[test]
    fn lines_move_copy_and_go() {
        let text = "one\ntwo\nthree";
        let two = text.find("two").unwrap();
        let up = move_lines(text, two + 1..two + 1, true).unwrap();
        assert_eq!(apply(text, &up), "two\none\nthree");
        assert_eq!(up.select, 1..1);
        let down = move_lines(text, two..two, false).unwrap();
        assert_eq!(apply(text, &down), "one\nthree\ntwo");
        assert_eq!(&apply(text, &down)[down.select.start..], "two");
        assert!(move_lines(text, 0..0, true).is_none());
        assert!(move_lines(text, text.len()..text.len(), false).is_none());

        let copy = copy_lines(text, two..two, true);
        let copied = apply(text, &copy);
        assert_eq!(copied, "one\ntwo\ntwo\nthree");
        assert_eq!(copy.select.start, "one\ntwo\n".len());

        let gone = delete_lines(text, two + 2..two + 2);
        assert_eq!(apply(text, &gone), "one\nthree");
        assert_eq!(gone.select, 6..6);
        let last = delete_lines(text, text.len()..text.len());
        assert_eq!(apply(text, &last), "one\ntwo");
        assert_eq!(apply("solo", &delete_lines("solo", 0..0)), "");
    }

    #[test]
    fn new_lines_keep_the_indent() {
        let text = "fn a() {\n    call();\n}";
        let inside = text.find("call").unwrap() + 2;
        let below = open_line(text, inside, true);
        assert_eq!(apply(text, &below), "fn a() {\n    call();\n    \n}");
        let above = open_line(text, inside, false);
        let changed = apply(text, &above);
        assert_eq!(changed, "fn a() {\n    \n    call();\n}");
        assert_eq!(above.select.start, "fn a() {\n    ".len());
    }

    #[test]
    fn occurrences_wrap_around() {
        let text = "ab ab ab";
        assert_eq!(next_occurrence(text, "ab", 2, &(0..2)), Some(3..5));
        assert_eq!(next_occurrence(text, "ab", 8, &(6..8)), Some(0..2));
        assert_eq!(next_occurrence("ab", "ab", 2, &(0..2)), None);
    }

    #[test]
    fn line_and_column() {
        assert_eq!(parse_line_column("12"), Some((11, 0)));
        assert_eq!(parse_line_column(" 12:5 "), Some((11, 4)));
        assert_eq!(parse_line_column("0"), Some((0, 0)));
        assert_eq!(parse_line_column("x"), None);
        assert_eq!(comment_for("main.RS"), Some(Comment::Line("//")));
        assert_eq!(comment_for("Dockerfile"), Some(Comment::Line("#")));
        assert_eq!(comment_for("notes.txt"), None);
    }
}
