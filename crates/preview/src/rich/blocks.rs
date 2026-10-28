//! A Markdown document as blocks for the rich editor: headings, paragraphs,
//! list items, to-dos, quotes, code, dividers, tables and anything else as
//! raw Markdown.
//!
//! The Markdown text stays the truth. A block keeps the exact text it was
//! read from until it is edited, and the whitespace between blocks is kept
//! too, so editing one paragraph changes only that paragraph in the file.

use std::sync::atomic::{AtomicU64, Ordering};

use markdown::mdast::Node;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Paragraph,
    Heading(u8),
    Bullet {
        indent: String,
        marker: char,
    },
    Numbered {
        indent: String,
        number: u64,
        delimiter: char,
    },
    Todo {
        indent: String,
        marker: char,
        checked: bool,
    },
    Quote,
    Code {
        fence: String,
        lang: String,
    },
    Divider,
    /// Kept and edited as Markdown: tables, HTML, front matter, footnotes…
    Raw,
}

impl Kind {
    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Kind::Bullet { .. } | Kind::Numbered { .. } | Kind::Todo { .. }
        )
    }

    /// The indent of a list item (nesting), empty otherwise.
    pub fn indent(&self) -> &str {
        match self {
            Kind::Bullet { indent, .. }
            | Kind::Numbered { indent, .. }
            | Kind::Todo { indent, .. } => indent,
            _ => "",
        }
    }

    /// Whether the body is edited as text that keeps its line breaks and
    /// spacing exactly (code, tables, raw Markdown).
    pub fn is_literal(&self) -> bool {
        matches!(self, Kind::Code { .. } | Kind::Raw)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: u64,
    pub kind: Kind,
    /// The text without its Markdown marks (`# `, `- [ ] `, `> `, fences).
    pub body: String,
    /// The whitespace after the block, up to the next one.
    pub gap: String,
    /// The exact text the block was read from, while it is unchanged.
    original: Option<String>,
}

impl Block {
    pub fn new(kind: Kind, body: impl Into<String>) -> Self {
        Self {
            id: next_id(),
            kind,
            body: body.into(),
            gap: "\n\n".into(),
            original: None,
        }
    }

    /// Changes the text; the block is written out afresh from now on.
    pub fn set_body(&mut self, body: String) {
        if body != self.body {
            self.body = body;
            self.original = None;
        }
    }

    pub fn set_kind(&mut self, kind: Kind) {
        if kind != self.kind {
            self.kind = kind;
            self.original = None;
        }
    }

    /// The block as Markdown.
    pub fn source(&self) -> String {
        if let Some(original) = &self.original {
            return original.clone();
        }
        let body = &self.body;
        match &self.kind {
            Kind::Paragraph | Kind::Raw => body.clone(),
            Kind::Heading(level) => format!("{} {body}", "#".repeat((*level).clamp(1, 6) as usize)),
            Kind::Bullet { indent, marker } => format!("{indent}{marker} {body}"),
            Kind::Numbered {
                indent,
                number,
                delimiter,
            } => format!("{indent}{number}{delimiter} {body}"),
            Kind::Todo {
                indent,
                marker,
                checked,
            } => format!(
                "{indent}{marker} [{}] {body}",
                if *checked { "x" } else { " " }
            ),
            Kind::Quote => body
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Kind::Code { fence, lang } => format!("{fence}{lang}\n{body}\n{fence}"),
            Kind::Divider => "---".into(),
        }
    }
}

/// A document: what comes before the first block, and the blocks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub leading: String,
    pub blocks: Vec<Block>,
}

impl Document {
    pub fn parse(text: &str) -> Self {
        let mut options = markdown::ParseOptions::gfm();
        options.constructs.frontmatter = true;
        let Ok(root) = markdown::to_mdast(text, &options) else {
            return Self::raw(text);
        };
        let Some(children) = root.children() else {
            return Self::raw(text);
        };
        // Where each top-level block (or list item) starts and ends.
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for child in children {
            match child {
                Node::List(list) => {
                    for item in &list.children {
                        if let Some(position) = item.position() {
                            spans.push((position.start.offset, position.end.offset));
                        }
                    }
                }
                other => {
                    if let Some(position) = other.position() {
                        spans.push((position.start.offset, position.end.offset));
                    }
                }
            }
        }
        // A block's trailing line breaks belong to the gap after it.
        for (start, end) in &mut spans {
            *end = *start + text[*start..*end].trim_end_matches(['\n', '\r']).len();
        }
        if spans.is_empty() {
            return Self {
                leading: text.to_string(),
                blocks: Vec::new(),
            };
        }
        let leading = text[..spans[0].0].to_string();
        let blocks = spans
            .iter()
            .enumerate()
            .map(|(ix, &(start, end))| {
                let next = spans.get(ix + 1).map(|span| span.0).unwrap_or(text.len());
                let source = &text[start..end];
                let (kind, body) = classify(source);
                Block {
                    id: next_id(),
                    kind,
                    body,
                    gap: text[end..next.max(end)].to_string(),
                    original: Some(source.to_string()),
                }
            })
            .collect();
        Self { leading, blocks }
    }

    fn raw(text: &str) -> Self {
        Self {
            leading: String::new(),
            blocks: vec![Block {
                id: next_id(),
                kind: Kind::Raw,
                body: text.to_string(),
                gap: String::new(),
                original: Some(text.to_string()),
            }],
        }
    }

    pub fn text(&self) -> String {
        let mut out = self.leading.clone();
        for block in &self.blocks {
            out.push_str(&block.source());
            out.push_str(&block.gap);
        }
        out
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.blocks.iter().position(|block| block.id == id)
    }

    /// Puts `block` after block `ix` (at the start for `None`). A list item
    /// after an item of a tight list joins it on the next line; anything
    /// else is set off by a blank line.
    pub fn insert_after(&mut self, ix: Option<usize>, mut block: Block) -> usize {
        let at = ix.map_or(0, |ix| ix + 1);
        let Some(ix) = ix.filter(|ix| *ix < self.blocks.len()) else {
            block.gap = if self.blocks.is_empty() { "\n" } else { "\n\n" }.into();
            self.blocks.insert(0, block);
            return 0;
        };
        let before = &self.blocks[ix];
        let in_tight_list = before.kind.is_list()
            && (before.gap == "\n"
                || (ix > 0
                    && self.blocks[ix - 1].kind.is_list()
                    && self.blocks[ix - 1].gap == "\n"));
        let tight = in_tight_list && block.kind.is_list();
        // The new block ends where the one before used to.
        block.gap = std::mem::replace(
            &mut self.blocks[ix].gap,
            if tight { "\n" } else { "\n\n" }.into(),
        );
        if !tight && at < self.blocks.len() && !block.gap.contains("\n\n") {
            block.gap = "\n\n".into();
        }
        if block.gap.is_empty() && at < self.blocks.len() {
            block.gap = "\n\n".into();
        }
        self.blocks.insert(at, block);
        at
    }

    pub fn remove(&mut self, ix: usize) -> Block {
        let removed = self.blocks.remove(ix);
        // The block before ends where the removed one did, when that kept
        // the blocks apart (or ended the file).
        if ix > 0 && (ix == self.blocks.len() || removed.gap.contains("\n\n")) {
            self.blocks[ix - 1].gap = removed.gap.clone();
        }
        removed
    }

    /// Renumbers the numbered items of the list that block `ix` is in.
    pub fn renumber(&mut self, ix: usize) {
        let Some(Kind::Numbered { indent, .. }) =
            self.blocks.get(ix).map(|block| block.kind.clone())
        else {
            return;
        };
        let mut start = ix;
        while start > 0
            && matches!(&self.blocks[start - 1].kind, Kind::Numbered { indent: i, .. } if *i == indent)
        {
            start -= 1;
        }
        let mut number = match &self.blocks[start].kind {
            Kind::Numbered { number, .. } => *number,
            _ => 1,
        };
        let mut at = start;
        while let Some(block) = self.blocks.get_mut(at) {
            match &mut block.kind {
                Kind::Numbered {
                    indent: i,
                    number: n,
                    ..
                } if *i == indent => {
                    if *n != number {
                        *n = number;
                        block.original = None;
                    }
                }
                _ => break,
            }
            number += 1;
            at += 1;
        }
    }
}

/// What a block's text is, and its body without the marks.
pub fn classify(source: &str) -> (Kind, String) {
    let trimmed = source.trim_start();
    // Fenced code.
    for fence in ["```", "~~~"] {
        if trimmed.starts_with(fence) && source.trim_end().len() > 3 {
            let (first, rest) = source.split_once('\n').unwrap_or((source, ""));
            let marks = first.trim_start();
            let fence_len = marks
                .chars()
                .take_while(|c| *c == fence.as_bytes()[0] as char)
                .count();
            let fence_text: String = marks.chars().take(fence_len).collect();
            let lang = marks[fence_len..].trim().to_string();
            let body = rest
                .strip_suffix(fence_text.as_str())
                .or_else(|| rest.trim_end().strip_suffix(fence_text.as_str()))
                .unwrap_or(rest);
            let body = body.strip_suffix('\n').unwrap_or(body);
            return (
                Kind::Code {
                    fence: fence_text,
                    lang,
                },
                body.to_string(),
            );
        }
    }
    // ATX headings.
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes)
        && !source.contains('\n')
        && trimmed[hashes..].starts_with([' ', '\t'])
    {
        let body = trimmed[hashes..].trim().trim_end_matches('#').trim_end();
        return (Kind::Heading(hashes as u8), body.to_string());
    }
    if matches!(
        trimmed.trim_end(),
        "---" | "***" | "___" | "- - -" | "* * *"
    ) {
        return (Kind::Divider, String::new());
    }
    if trimmed.starts_with('>') {
        let body = source
            .lines()
            .map(|line| {
                let line = line.trim_start();
                let line = line.strip_prefix('>').unwrap_or(line);
                line.strip_prefix(' ').unwrap_or(line)
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (Kind::Quote, body);
    }
    if let Some((kind, body)) = list_item(source) {
        return (kind, body);
    }
    if trimmed.starts_with('|') || trimmed.starts_with('<') || source.starts_with("---\n") {
        return (Kind::Raw, source.to_string());
    }
    // Setext headings and other forms stay Markdown.
    if source.lines().count() == 2
        && source.lines().nth(1).is_some_and(|line| {
            !line.trim().is_empty() && line.trim().chars().all(|c| c == '=' || c == '-')
        })
    {
        return (Kind::Raw, source.to_string());
    }
    (Kind::Paragraph, source.to_string())
}

/// A list item: its marker and the rest.
fn list_item(source: &str) -> Option<(Kind, String)> {
    let indent_len = source.len() - source.trim_start_matches([' ', '\t']).len();
    let indent = source[..indent_len].to_string();
    let rest = &source[indent_len..];
    let first = rest.chars().next()?;
    let (marker_kind, after) = if matches!(first, '-' | '*' | '+') {
        (None, &rest[1..])
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        let delimiter = rest[digits..].chars().next()?;
        if !matches!(delimiter, '.' | ')') {
            return None;
        }
        (
            Some((rest[..digits].parse::<u64>().ok()?, delimiter)),
            &rest[digits + 1..],
        )
    };
    if !(after.is_empty() || after.starts_with([' ', '\t', '\n'])) {
        return None;
    }
    let after = after.strip_prefix(' ').unwrap_or(after);
    if marker_kind.is_none() {
        for (open, checked) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(body) = after.strip_prefix(open) {
                return Some((
                    Kind::Todo {
                        indent,
                        marker: first,
                        checked,
                    },
                    body.to_string(),
                ));
            }
        }
        if matches!(after, "[ ]" | "[x]" | "[X]") {
            return Some((
                Kind::Todo {
                    indent,
                    marker: first,
                    checked: after != "[ ]",
                },
                String::new(),
            ));
        }
        return Some((
            Kind::Bullet {
                indent,
                marker: first,
            },
            after.to_string(),
        ));
    }
    let (number, delimiter) = marker_kind?;
    Some((
        Kind::Numbered {
            indent,
            number,
            delimiter,
        },
        after.to_string(),
    ))
}

/// What Enter does at the end of `line` in Markdown source.
#[derive(Debug, Clone, PartialEq)]
pub enum Continue {
    /// Start the next item with this text (`- `, `3. `, `- [ ] `).
    Item(String),
    /// The item is empty: end the list (take the marker away).
    End,
}

/// Whether `line` (up to the cursor) is a list item, and how Enter goes on.
pub fn continue_list(line: &str) -> Option<Continue> {
    let (kind, body) = list_item(line)?;
    if body.trim().is_empty() {
        return Some(Continue::End);
    }
    Some(Continue::Item(match kind {
        Kind::Bullet { indent, marker } => format!("{indent}{marker} "),
        Kind::Todo { indent, marker, .. } => format!("{indent}{marker} [ ] "),
        Kind::Numbered {
            indent,
            number,
            delimiter,
        } => format!("{indent}{}{delimiter} ", number + 1),
        _ => return None,
    }))
}

/// A Markdown shortcut typed at the start of a paragraph (`# `, `- `,
/// `1. `, `[] `, `> `, ```` ``` ````): the kind it makes and the text left.
pub fn shortcut(body: &str) -> Option<(Kind, String)> {
    let shortcuts: [(&str, Kind); 12] = [
        ("# ", Kind::Heading(1)),
        ("## ", Kind::Heading(2)),
        ("### ", Kind::Heading(3)),
        (
            "- ",
            Kind::Bullet {
                indent: String::new(),
                marker: '-',
            },
        ),
        (
            "* ",
            Kind::Bullet {
                indent: String::new(),
                marker: '-',
            },
        ),
        (
            "1. ",
            Kind::Numbered {
                indent: String::new(),
                number: 1,
                delimiter: '.',
            },
        ),
        (
            "[] ",
            Kind::Todo {
                indent: String::new(),
                marker: '-',
                checked: false,
            },
        ),
        (
            "[ ] ",
            Kind::Todo {
                indent: String::new(),
                marker: '-',
                checked: false,
            },
        ),
        (
            "- [ ] ",
            Kind::Todo {
                indent: String::new(),
                marker: '-',
                checked: false,
            },
        ),
        ("> ", Kind::Quote),
        (
            "```",
            Kind::Code {
                fence: "```".into(),
                lang: String::new(),
            },
        ),
        ("---", Kind::Divider),
    ];
    if body == "---" {
        return Some((Kind::Divider, String::new()));
    }
    if body.starts_with("```") && !body.contains('\n') {
        let lang = body.trim_start_matches('`').trim().to_string();
        if body.ends_with(' ') || body == "```" {
            return Some((
                Kind::Code {
                    fence: "```".into(),
                    lang,
                },
                String::new(),
            ));
        }
        return None;
    }
    // Longest prefix first: `- [ ] ` before `- `.
    let mut matches: Vec<&(&str, Kind)> = shortcuts
        .iter()
        .filter(|(prefix, _)| *prefix != "---" && *prefix != "```" && body.starts_with(prefix))
        .collect();
    matches.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    matches
        .first()
        .map(|(prefix, kind)| (kind.clone(), body[prefix.len()..].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\ntitle: x\n---\n\n# Title\n\nSome *text*\non two lines.\n\n- [x] done\n- [ ] todo\n  - nested\n\n1. one\n2. two\n\n> quote\n> more\n\n```rust\nfn main() {}\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n***\n\nSetext\n======\n";

    #[test]
    fn round_trips_exactly() {
        let document = Document::parse(SAMPLE);
        assert_eq!(document.text(), SAMPLE);
        let kinds: Vec<&Kind> = document.blocks.iter().map(|block| &block.kind).collect();
        assert!(matches!(kinds[0], Kind::Raw));
        assert!(matches!(kinds[1], Kind::Heading(1)));
        assert!(matches!(kinds[2], Kind::Paragraph));
        assert!(matches!(kinds[3], Kind::Todo { checked: true, .. }));
        assert!(matches!(kinds[4], Kind::Todo { checked: false, .. }));
        assert!(matches!(kinds[5], Kind::Numbered { number: 1, .. }));
        assert!(matches!(kinds[7], Kind::Quote));
        assert!(matches!(kinds[8], Kind::Code { lang, .. } if lang == "rust"));
        assert!(matches!(kinds[9], Kind::Raw));
        assert!(matches!(kinds[10], Kind::Divider));
        assert_eq!(document.blocks[4].body, "todo\n  - nested");
        assert_eq!(document.blocks[7].body, "quote\nmore");
        assert_eq!(document.blocks[8].body, "fn main() {}");
    }

    #[test]
    fn edits_change_only_their_block() {
        let mut document = Document::parse(SAMPLE);
        document.blocks[1].set_body("New title".into());
        if let Kind::Todo { checked, .. } = &mut document.blocks[4].kind {
            *checked = true;
        }
        document.blocks[4].original = None;
        let text = document.text();
        assert!(text.contains("# New title\n\nSome *text*\non two lines."));
        assert!(text.contains("- [x] done\n- [x] todo\n  - nested\n"));
        assert_eq!(text.len(), SAMPLE.len() + 4);
        document.blocks[7].set_body("one\n\ntwo".into());
        assert!(document.text().contains("> one\n>\n> two"));
    }

    #[test]
    fn inserts_keep_lists_together() {
        let mut document = Document::parse("- a\n- b\n\nText\n");
        let at = document.insert_after(
            Some(1),
            Block::new(
                Kind::Bullet {
                    indent: String::new(),
                    marker: '-',
                },
                "c",
            ),
        );
        assert_eq!(at, 2);
        assert_eq!(document.text(), "- a\n- b\n- c\n\nText\n");
        document.insert_after(Some(3), Block::new(Kind::Paragraph, "End"));
        assert_eq!(document.text(), "- a\n- b\n- c\n\nText\n\nEnd\n");
        document.remove(4);
        assert_eq!(document.text(), "- a\n- b\n- c\n\nText\n");
        let mut empty = Document::parse("");
        empty.insert_after(None, Block::new(Kind::Paragraph, "Hello"));
        assert_eq!(empty.text(), "Hello\n");
    }

    #[test]
    fn renumbers_lists_and_reads_shortcuts() {
        let mut document = Document::parse("1. a\n1. b\n1. c\n");
        document.renumber(0);
        assert_eq!(document.text(), "1. a\n2. b\n3. c\n");
        assert!(matches!(shortcut("## Hi"), Some((Kind::Heading(2), body)) if body == "Hi"));
        assert!(matches!(shortcut("- [ ] Buy"), Some((Kind::Todo { .. }, body)) if body == "Buy"));
        assert!(matches!(shortcut("- item"), Some((Kind::Bullet { .. }, body)) if body == "item"));
        assert!(
            matches!(shortcut("```rust "), Some((Kind::Code { lang, .. }, _)) if lang == "rust")
        );
        assert!(shortcut("plain text").is_none());
        assert_eq!(
            continue_list("  - item"),
            Some(Continue::Item("  - ".into()))
        );
        assert_eq!(
            continue_list("9. ninth"),
            Some(Continue::Item("10. ".into()))
        );
        assert_eq!(
            continue_list("* [x] done"),
            Some(Continue::Item("* [ ] ".into()))
        );
        assert_eq!(continue_list("- "), Some(Continue::End));
        assert_eq!(continue_list("plain"), None);
        assert!(shortcut("```rust").is_none());
    }
}
