//! Rendered Markdown: GitHub-flavored, with Mermaid diagrams drawn in
//! place, local images, and links to other files opening them in the editor.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::base::text::{MarkdownNode, TextView, markdown_ast};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, EventEmitter, ImageSource, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, SharedUri,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, WeakEntity, Window, div, img,
    px,
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
                    // As wide as it is drawn, or the column when that is
                    // narrower; the frame keeps the diagram's shape.
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .py_3()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(diagram.width.min(DIAGRAM_WIDTH)))
                                .aspect_ratio(diagram.width / diagram.height)
                                .child(
                                    img(diagram.image.clone())
                                        .size_full()
                                        .object_fit(gpui_kit::ObjectFit::Contain),
                                ),
                        )
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
    let text = uri.as_ref();
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("data:") {
        return ImageSource::from(uri.clone());
    }
    match local_path(dir, text) {
        Some(path) => ImageSource::from(path),
        None => refused_image(),
    }
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

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%'
            && ix + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&text[ix + 1..ix + 3], 16)
        {
            out.push(byte);
            ix += 3;
            continue;
        }
        out.push(bytes[ix]);
        ix += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A Markdown file, rendered.
pub struct MarkdownPreview {
    path: PathBuf,
    text: SharedString,
    scroll: ScrollHandle,
}

impl EventEmitter<PreviewEvent> for MarkdownPreview {}

impl MarkdownPreview {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            text: SharedString::default(),
            scroll: ScrollHandle::new(),
        }
    }

    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        self.text = text.to_string().into();
        cx.notify();
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
        let dark = cx.theme().is_dark();
        let empty = self.text.trim().is_empty();
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let refused = |uri: &str| {
            matches!(
                resolve_image(dir, &SharedUri::from(uri.to_string())),
                ImageSource::Custom(_)
            )
        };
        assert!(refused("http://tracker.example/pixel.gif"));
        assert!(refused(r"\\server\share\a.png"));
        assert!(!refused("img/a.png"));
    }

    #[test]
    fn images_resolve_beside_the_document() {
        let dir = Path::new("/work/docs");
        let local = resolve_image(dir, &SharedUri::from("img/a%20b.png"));
        assert!(matches!(local, ImageSource::Resource(_)));
        assert_eq!(percent_decode("a%20b"), "a b");
    }
}
