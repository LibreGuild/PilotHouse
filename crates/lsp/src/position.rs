//! Columns: the editor counts characters, language servers count UTF-16
//! code units (LSP's default, which every server supports). They differ
//! after a character outside the Basic Multilingual Plane, like an emoji,
//! so every position is converted on its way to or from a server.

use gpui_kit::component::input::{Rope, RopeExt as _};
use lsp_types::{Position, Range};

/// The server's position for the editor's `position` in `text`.
pub fn to_server(text: &Rope, position: Position) -> Position {
    let line = text.slice_line(position.line as usize);
    let units: usize = line
        .chars()
        .take(position.character as usize)
        .map(char::len_utf16)
        .sum();
    Position::new(position.line, units as u32)
}

/// The editor's position for the server's `position` in `text`.
pub fn to_editor(text: &Rope, position: Position) -> Position {
    let line = text.slice_line(position.line as usize);
    let (mut units, mut chars) = (0, 0);
    for c in line.chars() {
        if units >= position.character as usize {
            break;
        }
        units += c.len_utf16();
        chars += 1;
    }
    Position::new(position.line, chars)
}

/// [`to_editor`] for both ends of `range`.
pub fn range_to_editor(text: &Rope, range: Range) -> Range {
    Range::new(to_editor(text, range.start), to_editor(text, range.end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_utf16_units_on_the_server() {
        let text = Rope::from("let 🎉 = x;\nok");
        // `=` is the 7th character, after the two-unit emoji.
        let editor = Position::new(0, 6);
        let server = to_server(&text, editor);
        assert_eq!(server, Position::new(0, 7));
        assert_eq!(to_editor(&text, server), editor);
        assert_eq!(to_server(&text, Position::new(1, 1)), Position::new(1, 1));
        // Past the end stays at the end.
        assert_eq!(to_editor(&text, Position::new(1, 9)), Position::new(1, 2));
    }
}
