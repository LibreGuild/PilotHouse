//! Review notes on changed lines: written on the diff in the Git tab, then
//! sent to a chat together, each anchored to its file and line, so the agent
//! gets one list of fixes instead of a message per remark.

use std::path::Path;

/// A note on one line of a changed file.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Project-relative path.
    pub path: String,
    /// 1-based line in the file as it is now.
    pub line: u32,
    /// The line's text (with its `+`/` ` mark), for context.
    pub code: String,
    pub text: String,
    /// Written by an agent's review of the changes (replaced by its next).
    pub from_review: bool,
}

/// The new-file line number of each line of a hunk (`None` for removed
/// lines), from its `@@ -a,b +c,d @@` header.
pub fn new_line_numbers(header: &str, lines: &[String]) -> Vec<Option<u32>> {
    let start = header
        .split_whitespace()
        .find_map(|part| part.strip_prefix('+'))
        .and_then(|range| range.split(',').next())
        .and_then(|first| first.parse::<u32>().ok())
        .unwrap_or(1);
    let mut next = start;
    lines
        .iter()
        .map(|line| {
            if line.starts_with('-') {
                None
            } else {
                let number = next;
                next += 1;
                Some(number)
            }
        })
        .collect()
}

/// The notes as one message for the agent.
pub fn prompt(notes: &[Note], root: &Path) -> String {
    let mut text = format!(
        "Review notes on the uncommitted changes in {}. Address each one, then say what you changed:\n",
        root.display()
    );
    for (ix, note) in notes.iter().enumerate() {
        text.push_str(&format!(
            "\n{}. {}:{}\n   `{}`\n   {}\n",
            ix + 1,
            note.path,
            note.line,
            note.code.trim_end(),
            note.text.trim()
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_lines_on_the_new_side() {
        let lines: Vec<String> = [" a", "-b", "+c", "+d", " e"]
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(
            new_line_numbers("@@ -10,4 +12,5 @@ fn main()", &lines),
            vec![Some(12), None, Some(13), Some(14), Some(15)]
        );
    }

    #[test]
    fn anchors_each_note() {
        let notes = vec![Note {
            path: "src/app.rs".into(),
            line: 7,
            code: "+    let x = 1;".into(),
            text: "Name this better".into(),
            from_review: false,
        }];
        let text = prompt(&notes, Path::new("/repo"));
        assert!(text.contains("1. src/app.rs:7"));
        assert!(text.contains("Name this better"));
    }
}
