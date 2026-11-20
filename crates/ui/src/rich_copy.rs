//! Copying rendered Markdown as rich text: the clipboard gets the selection
//! as plain text and as HTML, so pasting into a wiki, a mail or a document
//! keeps its headings, tables, lists and code, as copying from a browser
//! or an editor's Markdown preview does.
//!
//! A Markdown view hands over its selection as Markdown
//! (`SelectionFormat::Source`); [`on`] takes the copy from it and writes
//! both forms. GPUI's clipboard has no HTML form, so that one is added
//! with the system's own calls on macOS and Windows; elsewhere the plain
//! text alone is copied.

use gpui_kit::base::TextSelection;
use gpui_kit::{App, ClipboardItem, InteractiveElement};
use markdown::mdast::Node;

/// Makes copying inside `element` (a Markdown view whose selection is its
/// source) write plain text and HTML. Put it right around the view: it
/// answers any copy that starts inside it.
pub fn on<E: InteractiveElement>(element: E) -> E {
    element.capture_action(|_: &gpui_kit::base::input::Copy, window, cx| {
        let source = TextSelection::selected_text(window, cx);
        if source.trim().is_empty() {
            return;
        }
        write(source.trim(), cx);
        cx.stop_propagation();
    })
}

/// Whether the HTML form is written: only in the app itself. It goes to
/// the system's clipboard directly, which a test (whose GPUI clipboard is
/// its own) must never change.
static SYSTEM_CLIPBOARD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called once by the app at start: copies then carry HTML too.
pub fn use_system_clipboard() {
    SYSTEM_CLIPBOARD.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Puts `markdown` on the clipboard as it reads (plain text) and as HTML.
pub fn write(markdown: &str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(plain(markdown)));
    add_html(markdown);
}

/// Puts `markdown` on the clipboard as it is written and as HTML: for a
/// view that edits Markdown, where pasted text should stay Markdown.
pub fn write_source(markdown: &str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(markdown.to_string()));
    add_html(markdown);
}

/// Adds the HTML of `markdown` to what was just copied (in the app only).
fn add_html(markdown: &str) {
    if SYSTEM_CLIPBOARD.load(std::sync::atomic::Ordering::Relaxed)
        && let Some(html) = html(markdown)
    {
        system::add_html(&html);
    }
}

/// `markdown` as HTML (GitHub's flavour: tables, task lists, strikethrough);
/// HTML written in it is shown as text, not passed on.
pub fn html(markdown: &str) -> Option<String> {
    // A selection of text shown plain because it nests too deeply is that
    // text again: it is not parsed here either.
    if crate::markdown::nests_too_deep(markdown) {
        return None;
    }
    markdown::to_html_with_options(markdown, &markdown::Options::gfm()).ok()
}

/// `markdown` as a reader sees it: no marks, a list's markers kept, a
/// table's cells apart by tabs (as a spreadsheet takes them).
pub fn plain(markdown: &str) -> String {
    if crate::markdown::nests_too_deep(markdown) {
        return markdown.to_string();
    }
    let Ok(root) = markdown::to_mdast(markdown, &markdown::ParseOptions::gfm()) else {
        return markdown.to_string();
    };
    let mut parts = Vec::new();
    blocks(std::slice::from_ref(&root), &mut parts);
    parts.join("\n\n")
}

/// The text of each block of `nodes`, in order.
fn blocks(nodes: &[Node], out: &mut Vec<String>) {
    for node in nodes {
        match node {
            Node::Paragraph(_) | Node::Heading(_) => out.push(inline(node)),
            Node::Code(code) => out.push(code.value.clone()),
            Node::Math(math) => out.push(math.value.clone()),
            Node::Html(html) => out.push(html.value.clone()),
            Node::ThematicBreak(_) => out.push("---".to_string()),
            Node::Table(table) => {
                let rows = table.children.iter().map(|row| {
                    let cells = row.children().map(Vec::as_slice).unwrap_or_default();
                    cells.iter().map(inline).collect::<Vec<_>>().join("\t")
                });
                out.push(rows.collect::<Vec<_>>().join("\n"));
            }
            Node::List(list) => {
                let items = list.children.iter().enumerate().map(|(ix, item)| {
                    let mut inner = Vec::new();
                    blocks(std::slice::from_ref(item), &mut inner);
                    let marker = match (list.ordered, list.start) {
                        (true, start) => format!("{}. ", start.unwrap_or(1) as usize + ix),
                        (false, _) => "- ".to_string(),
                    };
                    let checked = match item {
                        Node::ListItem(item) => item.checked,
                        _ => None,
                    };
                    let checked = match checked {
                        Some(true) => "[x] ",
                        Some(false) => "[ ] ",
                        None => "",
                    };
                    // Lines after the first sit under the item's text.
                    let indent = " ".repeat(marker.len());
                    let text = inner.join("\n").replace('\n', &format!("\n{indent}"));
                    format!("{marker}{checked}{text}")
                });
                out.push(items.collect::<Vec<_>>().join("\n"));
            }
            // Link definitions show nothing.
            Node::Definition(_) => {}
            other => match other.children() {
                Some(children) => blocks(children, out),
                None => {
                    let text = inline(other);
                    if !text.is_empty() {
                        out.push(text);
                    }
                }
            },
        }
    }
}

/// The text of `node` and what is inside it, without marks.
fn inline(node: &Node) -> String {
    match node {
        Node::Text(text) => text.value.clone(),
        Node::InlineCode(code) => code.value.clone(),
        Node::InlineMath(math) => math.value.clone(),
        Node::Html(html) => html.value.clone(),
        Node::Break(_) => "\n".to_string(),
        Node::Image(image) => image.alt.clone(),
        other => other
            .children()
            .map(|children| children.iter().map(inline).collect())
            .unwrap_or_default(),
    }
}

#[cfg(target_os = "macos")]
mod system {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML};
    use objc2_foundation::{NSArray, NSString};

    /// Adds `html` to what the clipboard holds (the plain text GPUI just
    /// wrote stays).
    pub fn add_html(html: &str) {
        add_html_to(&NSPasteboard::generalPasteboard(), html);
    }

    pub(super) fn add_html_to(pasteboard: &NSPasteboard, html: &str) {
        // SAFETY: the constant is a static string of AppKit's, and both
        // calls take objects owned here; they run on the main thread, as
        // every copy does.
        unsafe {
            let kind = NSPasteboardTypeHTML;
            pasteboard.addTypes_owner(&NSArray::from_slice(&[kind]), None);
            pasteboard.setString_forType(&NSString::from_str(html), kind);
        }
    }

    #[cfg(test)]
    mod tests {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
        use objc2_foundation::NSString;

        /// On a pasteboard of the test's own, never the user's.
        #[test]
        fn html_joins_the_plain_text_already_copied() {
            // SAFETY: as in `add_html_to`.
            unsafe {
                let pasteboard = NSPasteboard::pasteboardWithUniqueName();
                pasteboard.clearContents();
                pasteboard.setString_forType(&NSString::from_str("Title"), NSPasteboardTypeString);
                super::add_html_to(&pasteboard, "<h1>Title</h1>");
                let read = |kind| pasteboard.stringForType(kind).map(|text| text.to_string());
                assert_eq!(read(NSPasteboardTypeString).as_deref(), Some("Title"));
                assert_eq!(
                    read(NSPasteboardTypeHTML).as_deref(),
                    Some("<h1>Title</h1>")
                );
                pasteboard.clearContents();
            }
        }
    }
}

#[cfg(windows)]
mod system {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };

    /// Adds `html` to what the clipboard holds (the plain text GPUI just
    /// wrote stays), in Windows' "HTML Format".
    pub fn add_html(html: &str) {
        let data = super::cf_html(html);
        let name: Vec<u16> = "HTML Format\0".encode_utf16().collect();
        // SAFETY: `name` is NUL-terminated; the block is allocated with
        // room for `data` and its NUL, written while locked, and either
        // handed to the clipboard (which then owns it) or freed.
        unsafe {
            let format = RegisterClipboardFormatW(name.as_ptr());
            if format == 0 {
                return;
            }
            // Another program may hold the clipboard for a moment.
            let opened = (0..5).any(|attempt| {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                OpenClipboard(std::ptr::null_mut()) != 0
            });
            if !opened {
                return;
            }
            let block = GlobalAlloc(GMEM_MOVEABLE, data.len() + 1);
            if !block.is_null() {
                let bytes = GlobalLock(block).cast::<u8>();
                if bytes.is_null() {
                    GlobalFree(block);
                } else {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), bytes, data.len());
                    *bytes.add(data.len()) = 0;
                    GlobalUnlock(block);
                    if SetClipboardData(format, block).is_null() {
                        GlobalFree(block);
                    }
                }
            }
            CloseClipboard();
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod system {
    /// No HTML form here: the plain text is what was copied.
    pub fn add_html(_: &str) {}
}

/// `html` as Windows' "HTML Format" wants it: a header of byte offsets
/// (ten digits each, so its own length is known before they are), then the
/// document with the copied part marked.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn cf_html(html: &str) -> Vec<u8> {
    const OPEN: &str = "<html><body>\r\n<!--StartFragment-->";
    const CLOSE: &str = "<!--EndFragment-->\r\n</body></html>";
    let header = |start_html: usize, end_html: usize, start: usize, end: usize| {
        format!(
            "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
             StartFragment:{start:010}\r\nEndFragment:{end:010}\r\n"
        )
    };
    let start_html = header(0, 0, 0, 0).len();
    let start = start_html + OPEN.len();
    let end = start + html.len();
    let end_html = end + CLOSE.len();
    format!(
        "{}{OPEN}{html}{CLOSE}",
        header(start_html, end_html, start, end)
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Title\n\nSome **bold** and `code` with a [link](https://example.com).\n\n\
        | Name | Count |\n|---|---|\n| a | 1 |\n| b | 2 |\n\n\
        1. first\n2. second\n   - nested\n\n- [x] done\n- [ ] open\n\n\
        ```rust\nfn main() {}\n```\n";

    #[test]
    fn markdown_copies_as_html() {
        let html = html(DOC).unwrap();
        for part in [
            "<h1>Title</h1>",
            "<strong>bold</strong>",
            "<code>code</code>",
            "<a href=\"https://example.com\">link</a>",
            "<table>",
            "<th>Name</th>",
            "<td>b</td>",
            "<ol>",
            "<pre><code class=\"language-rust\">fn main() {}",
        ] {
            assert!(html.contains(part), "{part} in {html}");
        }
        // HTML in the text is not passed on as HTML.
        let unsafe_html = super::html("a <script>alert(1)</script> b").unwrap();
        assert!(!unsafe_html.contains("<script>"), "{unsafe_html}");
    }

    #[test]
    fn markdown_copies_as_the_text_read() {
        assert_eq!(
            plain(DOC),
            "Title\n\nSome bold and code with a link.\n\n\
             Name\tCount\na\t1\nb\t2\n\n\
             1. first\n2. second\n   - nested\n\n- [x] done\n- [ ] open\n\n\
             fn main() {}"
        );
        assert_eq!(plain("just *one* word"), "just one word");
        assert_eq!(plain("line one\\\nline two"), "line one\nline two");
        assert_eq!(plain("> quoted\n> text"), "quoted\ntext");
    }

    #[test]
    fn windows_html_marks_the_copied_part() {
        let html = "<p>héllo</p>";
        let data = cf_html(html);
        let text = String::from_utf8(data.clone()).unwrap();
        let offset = |name: &str| -> usize {
            let at = text.find(name).unwrap() + name.len();
            text[at..at + 10].parse().unwrap()
        };
        let (start, end) = (offset("StartFragment:"), offset("EndFragment:"));
        assert_eq!(&data[start..end], html.as_bytes());
        assert!(text[offset("StartHTML:")..].starts_with("<html>"));
        assert_eq!(offset("EndHTML:"), data.len());
    }
}
