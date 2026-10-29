//! Viewers for files that are more than lines of text: rendered Markdown
//! with Mermaid diagrams, images, JSON and YAML trees, tables, notebooks,
//! audio, video, PDF, databases, archives and fonts, and a hex view for the
//! rest.
//!
//! - [`Kind`]: what a path is, and the [`View`]s a text kind offers.
//! - [`TextPreview`]: the rendered side of a text file, fed the editor's
//!   text as it changes.
//! - [`FilePreview`]: a file shown only (media, documents, binaries), read
//!   from disk.
//!
//! Everything is drawn by PilotHouse itself; nothing here loads a web view.
//! Video plays through the system's own media framework, so no codec ships
//! with the app.

mod ansi;
mod archive;
mod audio;
pub mod diff;
mod env;
mod font;
mod hex;
mod image_view;
pub mod json;
mod kind;
mod markdown;
mod mermaid;
mod notebook;
mod pdf;
pub mod rich;
mod spreadsheet;
mod sqlite;
mod table;
mod tree;
mod video;

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window, div,
};

pub use kind::{Kind, View};
pub use markdown::markdown_view;

/// What a preview asks of the file view around it.
#[derive(Debug, Clone)]
pub enum PreviewEvent {
    /// Open this file (a link in the document), at a 0-based line.
    OpenPath(PathBuf, Option<u32>),
    /// Replace the file's text (a tick box, a formatted document).
    ReplaceText(String),
}

/// The rendered side of a text file.
pub struct TextPreview {
    kind: Kind,
    body: TextBody,
    _subscription: Option<Subscription>,
}

enum TextBody {
    Markdown(Entity<markdown::MarkdownPreview>),
    Mermaid(Entity<mermaid::MermaidPreview>),
    Svg(Entity<image_view::ImagePreview>),
    Tree(Entity<tree::TreePreview>),
    Table(Entity<table::TablePreview>),
    Env(Entity<env::EnvPreview>),
    Log(Entity<ansi::LogPreview>),
    Diff(Entity<diff::DiffPreview>),
    Notebook(Entity<notebook::NotebookPreview>),
    None,
}

impl EventEmitter<PreviewEvent> for TextPreview {}

impl TextPreview {
    /// A preview of `path`, a file of `kind`; it shows nothing until
    /// [`Self::set_text`].
    pub fn new(kind: Kind, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (body, subscription) = match kind {
            Kind::Markdown => {
                let view = cx.new(|_| markdown::MarkdownPreview::new(path));
                let subscription = forward(&view, cx);
                (TextBody::Markdown(view), Some(subscription))
            }
            Kind::Mermaid => (
                TextBody::Mermaid(cx.new(mermaid::MermaidPreview::new)),
                None,
            ),
            Kind::Svg => (
                TextBody::Svg(cx.new(|cx| image_view::ImagePreview::new(None, cx))),
                None,
            ),
            Kind::Json | Kind::JsonLines | Kind::Yaml | Kind::Toml => (
                TextBody::Tree(cx.new(|cx| tree::TreePreview::new(kind, cx))),
                None,
            ),
            Kind::Table { tabs } => (
                TextBody::Table(cx.new(|cx| table::TablePreview::new(tabs, window, cx))),
                None,
            ),
            Kind::Env => (TextBody::Env(cx.new(|_| env::EnvPreview::default())), None),
            Kind::Log => (TextBody::Log(cx.new(|_| ansi::LogPreview::default())), None),
            Kind::Diff => (
                TextBody::Diff(cx.new(|_| diff::DiffPreview::default())),
                None,
            ),
            Kind::Notebook => {
                let view = cx.new(|_| notebook::NotebookPreview::new(path));
                let subscription = forward(&view, cx);
                (TextBody::Notebook(view), Some(subscription))
            }
            _ => (TextBody::None, None),
        };
        Self {
            kind,
            body,
            _subscription: subscription,
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Shows `text`, the file's current text.
    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        match &self.body {
            TextBody::Markdown(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
            TextBody::Mermaid(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
            TextBody::Svg(view) => view.update(cx, |view, cx| view.set_svg(text, cx)),
            TextBody::Tree(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
            TextBody::Table(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
            TextBody::Env(view) => view.update(cx, |view, cx| view.set_text(&text, cx)),
            TextBody::Log(view) => view.update(cx, |view, cx| view.set_text(&text, cx)),
            TextBody::Diff(view) => view.update(cx, |view, cx| view.set_text(&text, cx)),
            TextBody::Notebook(view) => view.update(cx, |view, cx| view.set_text(text, cx)),
            TextBody::None => {}
        }
    }
}

/// Passes a view's events on as the preview's own.
fn forward<V: EventEmitter<PreviewEvent>>(
    view: &Entity<V>,
    cx: &mut Context<TextPreview>,
) -> Subscription {
    cx.subscribe(view, |_, _, event: &PreviewEvent, cx| {
        cx.emit(event.clone())
    })
}

impl Render for TextPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.body {
            TextBody::Markdown(view) => view.clone().into_any_element(),
            TextBody::Mermaid(view) => view.clone().into_any_element(),
            TextBody::Svg(view) => view.clone().into_any_element(),
            TextBody::Tree(view) => view.clone().into_any_element(),
            TextBody::Table(view) => view.clone().into_any_element(),
            TextBody::Env(view) => view.clone().into_any_element(),
            TextBody::Log(view) => view.clone().into_any_element(),
            TextBody::Diff(view) => view.clone().into_any_element(),
            TextBody::Notebook(view) => view.clone().into_any_element(),
            TextBody::None => div().into_any_element(),
        };
        div().size_full().child(body)
    }
}

/// A file shown only: media, documents, databases, archives, fonts, and a
/// hex view for other binaries.
pub struct FilePreview {
    path: PathBuf,
    body: FileBody,
}

enum FileBody {
    Image(Entity<image_view::ImagePreview>),
    Audio(Entity<audio::AudioPlayer>),
    Video(Entity<video::VideoPlayer>),
    Pdf(Entity<pdf::PdfView>),
    Sqlite(Entity<sqlite::SqliteView>),
    Spreadsheet(Entity<spreadsheet::SpreadsheetView>),
    Archive(Entity<archive::ArchiveView>),
    Font(Entity<font::FontPreview>),
    Binary(Entity<hex::HexView>),
}

impl FilePreview {
    /// `kind` is the file's [`Kind`]; a text kind (a binary file with a
    /// text name) gets the hex view.
    pub fn new(kind: Kind, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let body = match kind {
            Kind::Image => {
                FileBody::Image(cx.new(|cx| image_view::ImagePreview::new(Some(path.clone()), cx)))
            }
            Kind::Audio => FileBody::Audio(cx.new(|cx| audio::AudioPlayer::new(path.clone(), cx))),
            Kind::Video => {
                FileBody::Video(cx.new(|cx| video::VideoPlayer::new(path.clone(), window, cx)))
            }
            Kind::Pdf => FileBody::Pdf(cx.new(|cx| pdf::PdfView::new(path.clone(), cx))),
            Kind::Sqlite => {
                FileBody::Sqlite(cx.new(|cx| sqlite::SqliteView::new(path.clone(), window, cx)))
            }
            Kind::Spreadsheet => FileBody::Spreadsheet(
                cx.new(|cx| spreadsheet::SpreadsheetView::new(path.clone(), window, cx)),
            ),
            Kind::Archive => {
                FileBody::Archive(cx.new(|cx| archive::ArchiveView::new(path.clone(), window, cx)))
            }
            Kind::Font => FileBody::Font(cx.new(|cx| font::FontPreview::new(path.clone(), cx))),
            _ => FileBody::Binary(cx.new(|cx| hex::HexView::new(path.clone(), cx))),
        };
        Self { path, body }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The file changed on disk.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.body {
            FileBody::Image(view) => view.update(cx, |view, cx| view.reload(cx)),
            FileBody::Audio(view) => view.update(cx, |view, cx| view.reload(cx)),
            FileBody::Video(view) => view.update(cx, |view, cx| view.reload(window, cx)),
            FileBody::Pdf(view) => view.update(cx, |view, cx| view.reload(cx)),
            FileBody::Sqlite(view) => view.update(cx, |view, cx| view.reload(cx)),
            FileBody::Spreadsheet(view) => view.update(cx, |view, cx| view.reload(cx)),
            FileBody::Archive(view) => view.update(cx, |view, cx| view.reload(cx)),
            FileBody::Font(view) => view.update(cx, |view, cx| view.reload(cx)),
            FileBody::Binary(view) => view.update(cx, |view, cx| view.reload(cx)),
        }
    }

    /// Stops playback (the tab is closing or hidden).
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        match &self.body {
            FileBody::Audio(view) => view.update(cx, |view, cx| view.pause(cx)),
            FileBody::Video(view) => view.update(cx, |view, cx| view.pause(cx)),
            _ => {}
        }
    }
}

impl Render for FilePreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.body {
            FileBody::Image(view) => view.clone().into_any_element(),
            FileBody::Audio(view) => view.clone().into_any_element(),
            FileBody::Video(view) => view.clone().into_any_element(),
            FileBody::Pdf(view) => view.clone().into_any_element(),
            FileBody::Sqlite(view) => view.clone().into_any_element(),
            FileBody::Spreadsheet(view) => view.clone().into_any_element(),
            FileBody::Archive(view) => view.clone().into_any_element(),
            FileBody::Font(view) => view.clone().into_any_element(),
            FileBody::Binary(view) => view.clone().into_any_element(),
        };
        div().size_full().child(body)
    }
}

/// "245 KB", "1.2 MB".
pub(crate) fn byte_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} bytes"),
        _ if value >= 10. => format!("{value:.0} {}", UNITS[unit]),
        _ => format!("{value:.1} {}", UNITS[unit]),
    }
}

/// A message filling a viewer (loading, or why it cannot show the file).
pub(crate) fn notice(text: impl Into<gpui_kit::SharedString>, cx: &App) -> AnyElement {
    use gpui_kit::component::ActiveTheme as _;
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_6()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        // Wraps rather than running out of a narrow window.
        .child(
            div()
                .w_full()
                .max_w(gpui_kit::px(560.))
                .text_center()
                .child(text.into()),
        )
        .into_any_element()
}
