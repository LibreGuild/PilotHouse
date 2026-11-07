//! Rendered Markdown: GitHub-flavored, with Mermaid diagrams drawn in
//! place, local images, and links to other files opening them in the editor.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::AppContext as _;
use gpui_kit::base::text::{MarkdownNode, TextView, markdown_ast};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, EventEmitter, ImageSource, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, SharedUri,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, px,
};

use crate::PreviewEvent;
use crate::mermaid::{self, Diagram};

/// The widest a line of rendered text gets, for reading comfort.
pub const READING_WIDTH: f32 = 820.;
/// The widest a diagram is drawn in a document.
const DIAGRAM_WIDTH: f32 = READING_WIDTH - 64.;

/// A Markdown view of `text`, the file at `path`: images resolve beside
/// it, ```` ```mermaid ```` blocks become diagrams, and links to files call
/// `open` (web and mail links open in the browser; other schemes do
/// nothing, since a document may come from anyone).
pub fn markdown_view(
    id: impl Into<gpui_kit::ElementId>,
    text: SharedString,
    path: &Path,
    dark: bool,
    open: impl Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static,
) -> TextView {
    let dir: Arc<Path> = path.parent().unwrap_or(Path::new(".")).into();
    let images = dir.clone();
    TextView::markdown(id, text)
        .selectable(true)
        .code_block_actions(ph_ui::markdown::code_actions)
        .image_source(move |uri: &SharedUri| resolve_image(&images, uri))
        .on_link_click(move |url, _, window, cx| match local_target(&dir, url) {
            Some(path) => open(path, window, cx),
            None if ph_ui::platform::is_web_link(url) => cx.open_url(url),
            None => {}
        })
        .markdown_block_parser(move |node, _| {
            let markdown_ast::Node::Code(code) = node else {
                return None;
            };
            if code.lang.as_deref() != Some("mermaid") {
                return None;
            }
            let drawn = mermaid::draw(&code.value, dark);
            Some(
                MarkdownNode::new("mermaid", drawn)
                    .text(code.value.clone())
                    .markdown(format!("```mermaid\n{}\n```", code.value)),
            )
        })
        .markdown_block_renderer("mermaid", |node, _, cx| {
            let theme = cx.theme();
            match node.data::<Result<Diagram, String>>() {
                Some(Ok(diagram)) => {
                    // A document's blocks have no ids of their own: the
                    // diagram's source names it.
                    let source = node.as_text().to_string();
                    let key = {
                        use std::hash::{Hash as _, Hasher as _};
                        let mut hasher = std::hash::DefaultHasher::new();
                        source.hash(&mut hasher);
                        hasher.finish()
                    };
                    div()
                        .w_full()
                        .py_3()
                        .child(ph_ui::DiagramCard::new(
                            ("mermaid", key),
                            diagram.image.clone(),
                            (diagram.width, diagram.height),
                            source,
                            DIAGRAM_WIDTH,
                        ))
                        .into_any_element()
                }
                Some(Err(error)) => div()
                    .w_full()
                    .my_2()
                    .px_3()
                    .py_2()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.danger.opacity(0.4))
                    .bg(theme.danger.opacity(0.08))
                    .text_xs()
                    .child(
                        div()
                            .text_color(theme.danger)
                            .child(format!("Mermaid: {error}")),
                    )
                    .child(
                        div()
                            .pt_1()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(node.as_text().to_string()),
                    )
                    .into_any_element(),
                None => div().into_any_element(),
            }
        })
}

/// A document image: HTTPS and data URLs as they are, anything else a file
/// beside the document. Other web schemes and files on other computers are
/// not loaded (an image that does not load shows nothing).
fn resolve_image(dir: &Path, uri: &SharedUri) -> ImageSource {
    // Through a loader that refuses drawings that would read files.
    match image_target(dir, uri) {
        Some(resource) => ph_ui::images_safe::source(resource),
        None => refused_image(),
    }
}

/// Where a document picture is read from, if it may be.
fn image_target(dir: &Path, uri: &SharedUri) -> Option<gpui_kit::Resource> {
    let text = uri.as_ref();
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("data:") {
        return Some(gpui_kit::Resource::Uri(uri.clone()));
    }
    local_path(dir, text).map(|path| gpui_kit::Resource::Path(path.into()))
}

/// An image that is not loaded: it shows nothing, quietly.
fn refused_image() -> ImageSource {
    ImageSource::Custom(std::sync::Arc::new(|_, _| {
        Some(Err(gpui_kit::ImageCacheError::Asset(
            "not loaded from a document".into(),
        )))
    }))
}

/// The file a link points at, when it is a relative or `file:` link to
/// something that exists (any `#fragment` dropped).
fn local_target(dir: &Path, url: &str) -> Option<PathBuf> {
    local_path(dir, url).filter(|path| path.exists())
}

/// `text` as a path beside the document: relative, or a `file:` URL;
/// never one on another computer (reading those sends the user's sign-in
/// there, on Windows).
fn local_path(dir: &Path, text: &str) -> Option<PathBuf> {
    if text.starts_with('#') {
        return None;
    }
    let text = match text.strip_prefix("file://") {
        Some(rest) => rest,
        // Any other scheme (`https:`, `mailto:`, `C:` is a drive).
        None if text
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.len() > 1) =>
        {
            return None;
        }
        None => text,
    };
    let path = percent_decode(text.split(['#', '?']).next()?);
    if path.is_empty() || ph_ui::platform::is_network_path(&path) {
        return None;
    }
    let path = tidy(&dir.join(path));
    (!ph_ui::platform::is_network_path(&path.to_string_lossy())).then_some(path)
}

/// `path` without `.` and `..` parts (`docs/../README.md` is `README.md`),
/// so it matches the workspace's own paths.
fn tidy(path: &Path) -> PathBuf {
    let mut tidy = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
                if matches!(
                    tidy.components().next_back(),
                    Some(std::path::Component::Normal(_))
                ) =>
            {
                tidy.pop();
            }
            part => tidy.push(part),
        }
    }
    tidy
}

pub(crate) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        // By bytes: `%` before a multi-byte character is not an escape (and
        // slicing the text there would split the character).
        let hex = |byte: u8| (byte as char).to_digit(16);
        if bytes[ix] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(ix + 1).copied().and_then(hex),
                bytes.get(ix + 2).copied().and_then(hex),
            )
        {
            let byte = (high * 16 + low) as u8;
            out.push(byte);
            ix += 3;
            continue;
        }
        out.push(bytes[ix]);
        ix += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A long document cut into pieces that render alone as they do in the
/// whole: a list or a quote stays one piece (its items keep their spacing
/// and nesting), and a piece using `[text][label]` links or `[^note]`s
/// gets the definitions it uses.
fn chunks(text: &str) -> Vec<String> {
    use crate::rich::blocks::{Document, Kind};
    let document = Document::parse(text);
    let mut chunks: Vec<String> = Vec::new();
    if !document.leading.trim().is_empty() {
        chunks.push(document.leading.clone());
    }
    let family = |kind: &Kind| {
        if kind.is_list() {
            1
        } else if *kind == Kind::Quote {
            2
        } else {
            0
        }
    };
    let mut last_family = 0;
    let mut gap = String::new();
    for block in &document.blocks {
        let this = family(&block.kind);
        match chunks.last_mut() {
            Some(chunk) if this != 0 && this == last_family => {
                chunk.push_str(&gap);
                chunk.push_str(&block.source());
            }
            _ => chunks.push(block.source()),
        }
        last_family = this;
        gap = block.gap.clone();
    }
    // Definitions (`[label]: url`, `[^note]: text`) wherever they are used.
    let definitions: Vec<(String, &str)> = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix('[')?;
            let (label, after) = rest.split_once("]:")?;
            (!label.is_empty() && !after.trim().is_empty())
                .then(|| (format!("[{}]", label.to_lowercase()), line))
        })
        .collect();
    if !definitions.is_empty() {
        for chunk in &mut chunks {
            let lower = chunk.to_lowercase();
            let used: Vec<&str> = definitions
                .iter()
                .filter(|(label, line)| lower.contains(label.as_str()) && !chunk.contains(line))
                .map(|(_, line)| *line)
                .collect();
            if !used.is_empty() {
                chunk.push_str("\n\n");
                chunk.push_str(&used.join("\n"));
            }
        }
    }
    chunks
}

/// A Markdown file, rendered.
pub struct MarkdownPreview {
    path: PathBuf,
    text: SharedString,
    scroll: ScrollHandle,
    /// A long document, shown a block at a time as a virtual list (its
    /// top-level blocks, each rendered on its own): laying out every
    /// paragraph of a book on each frame froze the app.
    long: Option<LongDocument>,
    _parse: Option<gpui_kit::Task<()>>,
}

/// The blocks of a long document and the list showing them.
struct LongDocument {
    blocks: Vec<SharedString>,
    list: gpui_kit::ListState,
}

/// Past this size a document is shown as a list of blocks.
const LONG: usize = 64 * 1024;

impl EventEmitter<PreviewEvent> for MarkdownPreview {}

impl MarkdownPreview {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            text: SharedString::default(),
            scroll: ScrollHandle::new(),
            long: None,
            _parse: None,
        }
    }

    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        crate::mermaid::release_evicted(cx);
        if text.len() <= LONG {
            // Each line of a paragraph on its own line, as typed.
            self.text = ph_ui::markdown::keep_line_breaks(&text).into();
            self.long = None;
            self._parse = None;
            cx.notify();
            return;
        }
        // Split into blocks off the UI thread.
        let parse = cx.background_spawn(async move {
            chunks(&text)
                .into_iter()
                .map(|chunk| ph_ui::markdown::keep_line_breaks(&chunk).into())
                .collect::<Vec<SharedString>>()
        });
        self._parse = Some(cx.spawn(async move |this, cx| {
            let blocks = parse.await;
            _ = this.update(cx, |this, cx| {
                this.text = SharedString::default();
                match &mut this.long {
                    Some(long) => {
                        // Only the blocks that changed are measured again.
                        let same_start = long
                            .blocks
                            .iter()
                            .zip(&blocks)
                            .take_while(|(a, b)| a == b)
                            .count();
                        let (old, new) = (long.blocks.len(), blocks.len());
                        let same_end = long
                            .blocks
                            .iter()
                            .rev()
                            .zip(blocks.iter().rev())
                            .take(old.min(new) - same_start)
                            .take_while(|(a, b)| a == b)
                            .count();
                        long.list
                            .splice(same_start..old - same_end, new - same_end - same_start);
                        long.blocks = blocks;
                    }
                    None => {
                        let list = gpui_kit::ListState::new(
                            blocks.len(),
                            gpui_kit::ListAlignment::Top,
                            px(1000.),
                        );
                        this.long = Some(LongDocument { blocks, list });
                    }
                }
                cx.notify();
            });
        }));
    }

    /// A long document: its blocks in a virtual list.
    fn render_long(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let long = self.long.as_ref()?;
        let dark = cx.theme().is_dark();
        Some(
            gpui_kit::list(
                long.list.clone(),
                cx.processor(move |this, ix: usize, _, cx| {
                    let Some(block) = this
                        .long
                        .as_ref()
                        .and_then(|long| long.blocks.get(ix).cloned())
                    else {
                        return div().into_any_element();
                    };
                    let view = markdown_view(
                        ("markdown-block", ix),
                        block,
                        &this.path,
                        dark,
                        opener(cx.weak_entity()),
                    );
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(div().w_full().max_w(px(READING_WIDTH)).px_8().child(view))
                        .into_any_element()
                }),
            )
            .size_full()
            .pt_6()
            .pb_6()
            .bg(cx.theme().background)
            .into_any_element(),
        )
    }
}

/// Opens `path` through `view`'s events (links are followed from the
/// renderer, off the view's update).
pub(crate) fn opener<V: EventEmitter<PreviewEvent>>(
    view: WeakEntity<V>,
) -> impl Fn(PathBuf, &mut Window, &mut App) + Send + Sync + 'static {
    let view = std::sync::Mutex::new(view);
    move |path, _, cx| {
        let Ok(view) = view.lock().map(|view| view.clone()) else {
            return;
        };
        _ = view.update(cx, |_, cx| cx.emit(PreviewEvent::OpenPath(path, None)));
    }
}

impl Render for MarkdownPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(long) = self.render_long(cx) {
            return long;
        }
        let dark = cx.theme().is_dark();
        let empty = self.text.trim().is_empty();
        if empty && self._parse.is_some() {
            return crate::notice("Loading…", cx).into_any_element();
        }
        let view = markdown_view(
            "markdown-preview",
            self.text.clone(),
            &self.path,
            dark,
            opener(cx.weak_entity()),
        );
        div()
            .id("markdown-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(cx.theme().background)
            .child(
                div()
                    .w_full()
                    .max_w(px(READING_WIDTH))
                    .mx_auto()
                    .px_8()
                    .py_6()
                    .when(empty, |this| {
                        this.text_color(cx.theme().muted_foreground)
                            .child("Nothing written yet.")
                    })
                    .when(!empty, |this| this.child(view)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_documents_cut_where_pieces_stand_alone() {
        let text = "# Title\n\n- a\n  - nested\n- b\n\n> q1\n\nSee [docs][d].\n\n[d]: https://example.com\n";
        let pieces = chunks(text);
        assert_eq!(pieces[0], "# Title");
        assert_eq!(pieces[1], "- a\n  - nested\n- b");
        assert!(
            pieces
                .iter()
                .any(|piece| piece.starts_with("See [docs][d].")
                    && piece.contains("[d]: https://example.com"))
        );
    }

    #[test]
    fn links_to_files_open_them() {
        let dir = std::env::temp_dir().join(format!("ph-md-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/My Guide.md"), "").unwrap();
        assert_eq!(
            local_target(&dir, "docs/My%20Guide.md#setup"),
            Some(dir.join("docs/My Guide.md"))
        );
        assert_eq!(local_target(&dir, "https://example.com/a.md"), None);
        assert_eq!(local_target(&dir, "#heading"), None);
        assert_eq!(local_target(&dir, "missing.md"), None);
        assert_eq!(local_target(&dir, "mailto:a@b.c"), None);
        assert_eq!(local_target(&dir, "x-apple.systempreferences:a"), None);
        assert_eq!(
            local_target(&dir.join("docs"), "./../docs/My%20Guide.md"),
            Some(dir.join("docs/My Guide.md"))
        );
    }

    #[test]
    fn nothing_loads_from_other_computers() {
        let dir = Path::new("/work/docs");
        assert_eq!(local_path(dir, r"\\server\share\a.png"), None);
        assert_eq!(local_path(dir, "//server/share/a.png"), None);
        assert_eq!(local_path(dir, "file:////server/share/a.png"), None);
        assert_eq!(local_path(dir, "%5C%5Cserver%5Cshare%5Ca.png"), None);
        let refused = |uri: &str| image_target(dir, &SharedUri::from(uri.to_string())).is_none();
        assert!(refused("http://tracker.example/pixel.gif"));
        assert!(refused(r"\\server\share\a.png"));
        assert!(!refused("img/a.png"));
    }

    #[test]
    fn images_resolve_beside_the_document() {
        let dir = Path::new("/work/docs");
        let local = image_target(dir, &SharedUri::from("img/a%20b.png"));
        assert!(matches!(local, Some(gpui_kit::Resource::Path(_))));
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%aé.png"), "%aé.png");
        assert_eq!(percent_decode("end%41"), "endA");
    }
}
