//! Text selected in the formatted view, copied as the Markdown it shows.
//!
//! The view copies what it shows: plain text. Found in the document's text
//! as shown, the selection is mapped back to the Markdown it came from, so
//! "Some bold words" copies as `Some **bold** words`. Formatting the
//! selection cuts through (half of a bold run, a link) is taken whole; a
//! whole heading, list item or quote brings its marker (`# `, `- `, `> `);
//! a selection across table cells takes the table.

use ::markdown::mdast::Node;

/// The Markdown in `source` that `copied` (text copied from its formatted
/// view) shows, or `None` when it is not found or has no formatting.
pub fn as_markdown(source: &str, copied: &str) -> Option<String> {
    let tree = ::markdown::to_mdast(source, &::markdown::ParseOptions::gfm()).ok()?;
    let mut shown = Shown::default();
    shown.walk(&tree, source);
    let (start, end) = shown.find(copied)?;
    let (mut start, mut end) = shown.source_range(start, end)?;
    // Formatting the selection cuts through is taken whole.
    loop {
        let before = (start, end);
        for &(a, b) in &shown.inlines {
            let cuts = (start < a && end > a && end < b) || (start > a && start < b && end > b);
            if cuts {
                start = start.min(a);
                end = end.max(b);
            }
        }
        if (start, end) == before {
            break;
        }
    }
    // A block selected whole brings its marker.
    for block in &shown.blocks {
        if start >= block.start && start <= block.first && end >= block.last {
            start = block.start;
        }
    }
    // Across table cells: the table.
    for (a, b, cells) in &shown.tables {
        let (a, b) = (*a, *b);
        let touched = cells.iter().filter(|&&(c, d)| start < d && end > c).count();
        if touched > 1 {
            start = start.min(a);
            end = end.max(b);
        }
    }
    let markdown = source.get(start..end)?.to_string();
    (markdown != copied.trim()).then_some(markdown)
}

/// A block with a marker before its text.
struct Block {
    start: usize,
    /// Where its text starts and ends.
    first: usize,
    last: usize,
}

/// The document as the view shows it, each character with where it is in
/// the Markdown.
#[derive(Default)]
struct Shown {
    chars: Vec<char>,
    /// Byte offset in the source, per character (`None`: a break between
    /// blocks).
    at: Vec<Option<usize>>,
    inlines: Vec<(usize, usize)>,
    blocks: Vec<Block>,
    tables: Vec<Table>,
}

/// A table's range and its cells'.
type Table = (usize, usize, Vec<(usize, usize)>);

impl Shown {
    fn walk(&mut self, node: &Node, source: &str) {
        let range = node
            .position()
            .map(|position| (position.start.offset, position.end.offset));
        match node {
            Node::Text(text) => self.text(&text.value, range, source),
            Node::InlineCode(code) => self.text(&code.value, range, source),
            Node::Code(code) => self.text(&code.value, range, source),
            Node::Break(_) => self.gap(),
            _ => {}
        }
        if let (
            Node::Strong(_)
            | Node::Emphasis(_)
            | Node::Delete(_)
            | Node::Link(_)
            | Node::InlineCode(_)
            | Node::Image(_),
            Some(range),
        ) = (node, range)
        {
            self.inlines.push(range);
        }
        let from = self.chars.len();
        if let Node::Table(_) = node
            && let Some((a, b)) = range
        {
            self.tables.push((a, b, Vec::new()));
        }
        for child in node.children().into_iter().flatten() {
            self.walk(child, source);
        }
        // Tables do not nest: a cell is the last table's.
        if let (Node::TableCell(_), Some(range)) = (node, range)
            && let Some(table) = self.tables.last_mut()
        {
            table.2.push(range);
        }
        if let (Node::Heading(_) | Node::ListItem(_) | Node::Blockquote(_), Some((start, _))) =
            (node, range)
        {
            let shown = &self.at[from..];
            let first = shown.iter().flatten().next().copied();
            let last = shown.iter().flatten().last().copied();
            if let (Some(first), Some(last)) = (first, last) {
                // `last` is the start of the last character: to its end.
                let end = last + source[last..].chars().next().map_or(0, char::len_utf8);
                self.blocks.push(Block {
                    start,
                    first,
                    last: end,
                });
            }
        }
        if matches!(
            node,
            Node::Paragraph(_) | Node::Heading(_) | Node::Code(_) | Node::TableCell(_)
        ) {
            self.gap();
        }
    }

    fn text(&mut self, value: &str, range: Option<(usize, usize)>, source: &str) {
        let Some((start, end)) = range else {
            return;
        };
        // Where the text sits in its node's Markdown (escapes and entities
        // aside, it is there as is).
        let base = source
            .get(start..end)
            .and_then(|slice| slice.find(value))
            .map(|offset| start + offset);
        for (offset, ch) in value.char_indices() {
            self.chars.push(ch);
            self.at.push(Some(match base {
                Some(base) => base + offset,
                None => start + (offset * (end - start)) / value.len().max(1),
            }));
        }
    }

    fn gap(&mut self) {
        self.chars.push('\n');
        self.at.push(None);
    }

    /// Where `copied` is in the shown text (indices of its first and last
    /// character), whitespace compared loosely.
    fn find(&self, copied: &str) -> Option<(usize, usize)> {
        let (shown, index) = loosen(self.chars.iter().copied());
        let (wanted, _) = loosen(copied.chars());
        let wanted: Vec<char> = trim(&wanted);
        if wanted.is_empty() || wanted.len() > shown.len() {
            return None;
        }
        let at = shown
            .windows(wanted.len())
            .position(|window| window == wanted.as_slice())?;
        Some((index[at], index[at + wanted.len() - 1]))
    }

    /// The source bytes from the first to the last of shown characters
    /// `start..=end`.
    fn source_range(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        let first = self.at[start..=end].iter().flatten().next().copied()?;
        let (last_ix, last) = self.at[start..=end]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(ix, at)| at.map(|at| (ix, at)))?;
        let ch = self.chars[start + last_ix];
        Some((first, last + ch.len_utf8()))
    }
}

/// Whitespace runs as one space; with the index each character came from.
fn loosen(chars: impl Iterator<Item = char>) -> (Vec<char>, Vec<usize>) {
    let (mut out, mut index) = (Vec::new(), Vec::new());
    let mut space = false;
    for (ix, ch) in chars.enumerate() {
        if ch.is_whitespace() {
            if !space && !out.is_empty() {
                out.push(' ');
                index.push(ix);
            }
            space = true;
        } else {
            out.push(ch);
            index.push(ix);
            space = false;
        }
    }
    (out, index)
}

fn trim(chars: &[char]) -> Vec<char> {
    let mut chars = chars.to_vec();
    while chars.last() == Some(&' ') {
        chars.pop();
    }
    chars
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Release notes\n\nSome **bold** words and a [link](https://example.com) to copy.\n\n| Name | Price |\n| ---- | ----- |\n| Tea  | 3     |\n\n- One\n- Two\n";

    #[test]
    fn formatting_comes_with_the_text() {
        assert_eq!(
            as_markdown(DOC, "Some bold words").as_deref(),
            Some("Some **bold** words")
        );
        // A link cut through is taken whole.
        assert_eq!(
            as_markdown(DOC, "a lin").as_deref(),
            Some("a [link](https://example.com)")
        );
        // Inside one run, the text is the text.
        assert_eq!(as_markdown(DOC, "ol"), None);
    }

    #[test]
    fn whole_blocks_bring_their_markers() {
        assert_eq!(
            as_markdown(DOC, "Release notes").as_deref(),
            Some("# Release notes")
        );
        assert_eq!(
            as_markdown(DOC, "Release notes\nSome bold").as_deref(),
            Some("# Release notes\n\nSome **bold**")
        );
        assert_eq!(
            as_markdown(DOC, "One\nTwo").as_deref(),
            Some("- One\n- Two")
        );
        // Part of a heading is just text.
        assert_eq!(as_markdown(DOC, "Release"), None);
    }

    #[test]
    fn across_cells_takes_the_table() {
        let table = as_markdown(DOC, "Tea 3").unwrap();
        assert!(table.starts_with("| Name | Price |"), "{table}");
        assert!(table.ends_with("| Tea  | 3     |"), "{table}");
        // Not from this document: left as copied.
        assert_eq!(as_markdown(DOC, "something else"), None);
    }
}
