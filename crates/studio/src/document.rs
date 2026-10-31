//! One picture being edited: the original (read once), the edit and its
//! history, and the preview, made off the UI thread.
//!
//! The preview is made from a copy of the original no larger than
//! [`PREVIEW_SIDE`], so a change shows at once whatever the picture's size;
//! one is made at a time, and changes made meanwhile are folded into the
//! next. What the saved file would weigh is worked out a moment after the
//! changes stop, from the original itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use gpui_kit::{App, AppContext as _, Context, RenderImage, SharedString, Task};
use image::{ImageDecoder as _, ImageEncoder as _, RgbaImage, imageops};
use ph_media::Frame;

use crate::edit::{ImageEdit, MAX_PIXELS, MAX_SIDE};
use crate::encode::{self, Encoded, ExportSettings, Format};
use crate::history::History;
use crate::render::{Quality, render};

/// The longest side of the copy previews are made from: sharp on a large
/// high-density screen, small enough to redo on every change.
const PREVIEW_SIDE: u32 = 2048;
/// The largest file opened.
const MAX_FILE: u64 = 256 * 1024 * 1024;
/// How long after the last change the saved size is worked out.
const ESTIMATE_DELAY: Duration = Duration::from_millis(450);

/// The picture as read.
pub struct Original {
    pub full: Arc<RgbaImage>,
    /// `full`, at most [`PREVIEW_SIDE`] a side.
    pub small: Arc<RgbaImage>,
    /// `full`'s size.
    pub size: (u32, u32),
    /// The file's format, when one PilotHouse writes.
    pub format: Option<Format>,
    pub bytes: u64,
    /// The original as shown (for comparing).
    pub frame: Frame,
}

enum State {
    Loading,
    Failed(SharedString),
    Ready(Original),
}

/// What the preview shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The result.
    Result,
    /// The whole turned picture with its marks, to choose the crop on.
    Crop,
}

/// A preview made.
pub struct Preview {
    pub frame: Frame,
    /// Its pixels, for the color under the pointer.
    pub image: Arc<RgbaImage>,
    /// What it shows.
    pub view: View,
    /// The edit it was made from.
    pub edit: ImageEdit,
}

/// What the saved file would be.
#[derive(Debug, Clone)]
pub enum Estimate {
    Working,
    Done {
        bytes: usize,
        width: u32,
        height: u32,
        note: Option<String>,
    },
    Failed(String),
}

pub struct ImageDocument {
    path: PathBuf,
    /// The workspace the picture was opened from; its agents edit it.
    project: Option<i64>,
    state: State,
    edit: ImageEdit,
    /// The edit as last saved (none: the original).
    saved: ImageEdit,
    history: History<ImageEdit>,
    export: ExportSettings,
    view: View,
    preview: Option<Preview>,
    /// Previews replaced, for the window to free on the GPU.
    stale: Vec<Arc<RenderImage>>,
    rendering: bool,
    render_again: bool,
    _render: Option<Task<()>>,
    /// Bumped on every change, so late answers are dropped.
    revision: u64,
    estimate: Option<(u64, Estimate)>,
    _estimate: Option<Task<()>>,
    /// The saved size is wanted (Export is open): working it out renders
    /// and encodes the whole picture, so only then.
    estimating: bool,
    /// When the user or an agent last used it: agents edit the latest.
    pub(crate) used: Instant,
}

impl ImageDocument {
    /// Opens `path`, read off the UI thread.
    pub fn new(path: PathBuf, project: Option<i64>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            project,
            state: State::Loading,
            edit: ImageEdit::default(),
            saved: ImageEdit::default(),
            history: History::default(),
            export: ExportSettings::default(),
            view: View::Result,
            preview: None,
            stale: Vec::new(),
            rendering: false,
            render_again: false,
            _render: None,
            revision: 0,
            estimate: None,
            _estimate: None,
            estimating: false,
            used: Instant::now(),
        };
        this.load(cx);
        crate::control::register(cx.entity().downgrade(), cx);
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let read = cx.background_spawn(async move { read(&path) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(original) => {
                        this.export =
                            ExportSettings::for_format(original.format.unwrap_or_default());
                        this.state = State::Ready(original);
                        this.changed(cx);
                    }
                    Err(err) => this.state = State::Failed(format!("{err:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn project(&self) -> Option<i64> {
        self.project
    }

    pub fn original(&self) -> Option<&Original> {
        match &self.state {
            State::Ready(original) => Some(original),
            _ => None,
        }
    }

    /// Why the picture could not be opened.
    pub fn error(&self) -> Option<&SharedString> {
        match &self.state {
            State::Failed(error) => Some(error),
            _ => None,
        }
    }

    pub fn edit(&self) -> &ImageEdit {
        &self.edit
    }

    pub fn export(&self) -> &ExportSettings {
        &self.export
    }

    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }

    pub fn view(&self) -> View {
        self.view
    }

    /// Works out what the saved file would weigh while `on`.
    pub fn set_estimating(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.estimating != on {
            self.estimating = on;
            if on {
                self.schedule_estimate(cx);
            } else {
                self._estimate = None;
            }
            cx.notify();
        }
    }

    pub fn estimate(&self) -> Option<&Estimate> {
        self.estimate
            .as_ref()
            .filter(|(revision, _)| *revision == self.revision)
            .map(|(_, estimate)| estimate)
    }

    /// Whether there are changes not saved anywhere.
    pub fn is_dirty(&self) -> bool {
        self.edit != self.saved
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The original's size (0 × 0 until read).
    pub fn size(&self) -> (u32, u32) {
        self.original().map_or((0, 0), |original| original.size)
    }

    /// Previews replaced since last asked: the window frees them.
    pub fn take_stale(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.stale)
    }

    /// Changes the edit; `what` names the change, and changes of the same
    /// thing in quick succession undo as one.
    pub fn update(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut ImageEdit, (u32, u32)),
        cx: &mut Context<Self>,
    ) {
        let size = self.size();
        let mut next = self.edit.clone();
        change(&mut next, size);
        if next.sanitize(size).is_err() || next == self.edit {
            return;
        }
        self.commit(what, next, cx);
    }

    /// Takes `next` as the edit (checked), as one undo step.
    pub(crate) fn commit(&mut self, what: &'static str, next: ImageEdit, cx: &mut Context<Self>) {
        let before = std::mem::replace(&mut self.edit, next);
        self.history.record(what, before);
        self.changed(cx);
    }

    /// Ends folding changes: the next one is its own undo step.
    pub fn settle(&mut self) {
        self.history.settle();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let current = std::mem::take(&mut self.edit);
        match self.history.undo(current) {
            Ok(previous) => {
                self.edit = previous;
                self.changed(cx);
            }
            Err(current) => self.edit = current,
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let current = std::mem::take(&mut self.edit);
        match self.history.redo(current) {
            Ok(next) => {
                self.edit = next;
                self.changed(cx);
            }
            Err(current) => self.edit = current,
        }
    }

    /// Back to the original (one step to undo).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if !self.edit.is_unchanged() {
            self.history.settle();
            self.commit("reset", ImageEdit::default(), cx);
        }
    }

    pub fn set_export(&mut self, change: impl FnOnce(&mut ExportSettings), cx: &mut Context<Self>) {
        let mut next = self.export.clone();
        change(&mut next);
        next.sanitize();
        if next != self.export {
            self.export = next;
            self.revision += 1;
            self.schedule_estimate(cx);
            cx.notify();
        }
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            self.schedule_render(cx);
            cx.notify();
        }
    }

    /// Marks it used now (the user or an agent at work on it).
    pub fn touch(&mut self) {
        self.used = Instant::now();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.used = Instant::now();
        self.schedule_render(cx);
        self.schedule_estimate(cx);
        cx.notify();
    }

    /// The edit as the preview shows it.
    fn shown_edit(&self) -> ImageEdit {
        match self.view {
            View::Result => self.edit.clone(),
            View::Crop => ImageEdit {
                crop: None,
                resize: None,
                frame: None,
                ..self.edit.clone()
            },
        }
    }

    fn schedule_render(&mut self, cx: &mut Context<Self>) {
        if self.rendering {
            self.render_again = true;
        } else {
            self.start_render(cx);
        }
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        let Some(original) = self.original() else {
            return;
        };
        let (small, size) = (original.small.clone(), original.size);
        let (edit, view) = (self.shown_edit(), self.view);
        self.rendering = true;
        self.render_again = false;
        let made = cx.background_spawn({
            let edit = edit.clone();
            async move {
                let image = render(&small, size, &edit, Quality::Preview);
                let frame = Frame::from_rgba_slice(image.width(), image.height(), image.as_raw());
                (Arc::new(image), frame)
            }
        });
        self._render = Some(cx.spawn(async move |this, cx| {
            let (image, frame) = made.await;
            _ = this.update(cx, |this, cx| {
                let preview = Preview {
                    frame,
                    image,
                    view,
                    edit,
                };
                if let Some(old) = this.preview.replace(preview) {
                    this.stale.push(old.frame.render_image());
                }
                this.rendering = false;
                if this.render_again {
                    this.start_render(cx);
                }
                cx.notify();
            });
        }));
    }

    /// Works out the saved file a moment after the changes stop.
    fn schedule_estimate(&mut self, cx: &mut Context<Self>) {
        if !self.estimating {
            return;
        }
        let Some(original) = self.original() else {
            return;
        };
        let (full, size) = (original.full.clone(), original.size);
        let (edit, settings, revision) = (self.edit.clone(), self.export.clone(), self.revision);
        self.estimate = Some((revision, Estimate::Working));
        self._estimate = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ESTIMATE_DELAY).await;
            let encoded = cx
                .background_spawn(async move {
                    let image = render(&full, size, &edit, Quality::Full);
                    encode::encode(&image, &settings)
                })
                .await;
            _ = this.update(cx, |this, cx| {
                let estimate = match encoded {
                    Ok(encoded) => Estimate::Done {
                        bytes: encoded.bytes.len(),
                        width: encoded.width,
                        height: encoded.height,
                        note: encoded.note,
                    },
                    Err(err) => Estimate::Failed(format!("{err:#}")),
                };
                this.estimate = Some((revision, estimate));
                cx.notify();
            });
        }));
    }

    /// The result at full size, made off the UI thread.
    pub fn result(&self, cx: &App) -> Task<Result<RgbaImage>> {
        let Some(original) = self.original() else {
            return Task::ready(Err(anyhow::anyhow!("the picture is not open yet")));
        };
        let (full, size, edit) = (original.full.clone(), original.size, self.edit.clone());
        cx.background_spawn(async move { Ok(render(&full, size, &edit, Quality::Full)) })
    }

    /// A small PNG of the result (or, `original`, the turned original), at
    /// most `side` a side, for an agent to look at; with its scale.
    pub fn snapshot(&self, original: bool, side: u32, cx: &App) -> Task<Result<(Vec<u8>, f32)>> {
        let Some(read) = self.original() else {
            return Task::ready(Err(anyhow::anyhow!("the picture is not open yet")));
        };
        let (small, size) = (read.small.clone(), read.size);
        let edit = if original {
            ImageEdit {
                orientation: self.edit.orientation,
                ..Default::default()
            }
        } else {
            self.edit.clone()
        };
        cx.background_spawn(async move {
            let image = render(&small, size, &edit, Quality::Preview);
            let full = if original {
                edit.turned_size(size)
            } else {
                edit.result_size(size)
            };
            let fitted = fit_within(image.dimensions(), side);
            let image = if fitted != image.dimensions() {
                imageops::thumbnail(&image, fitted.0, fitted.1)
            } else {
                image
            };
            let scale = image.width() as f32 / full.0.max(1) as f32;
            let mut bytes = Vec::new();
            image::codecs::png::PngEncoder::new(&mut bytes).write_image(
                &image,
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )?;
            Ok((bytes, scale))
        })
    }

    /// Writes the result to `path` as `settings` say. An existing file is
    /// replaced only when `replace` (the save panel asked already).
    pub fn save(
        &mut self,
        path: PathBuf,
        settings: ExportSettings,
        replace: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Encoded>> {
        let made = self.result(cx);
        let edit = self.edit.clone();
        let writing = cx.background_spawn(async move {
            let image = made.await?;
            let encoded = encode::encode(&image, &settings)?;
            write(&path, &encoded.bytes, replace)?;
            anyhow::Ok(encoded)
        });
        cx.spawn(async move |this, cx| {
            let encoded = writing.await?;
            _ = this.update(cx, |this, cx| {
                this.saved = edit;
                cx.notify();
            });
            Ok(encoded)
        })
    }

    /// Writes the result over the original, in its own format, and starts
    /// again from it (the edit is now in its pixels).
    pub fn replace_original(&mut self, cx: &mut Context<Self>) -> Task<Result<Encoded>> {
        let format = self
            .original()
            .and_then(|original| original.format)
            .or_else(|| {
                self.path
                    .extension()
                    .and_then(|extension| Format::from_extension(&extension.to_string_lossy()))
            });
        let Some(format) = format else {
            return Task::ready(Err(anyhow::anyhow!(
                "PilotHouse cannot write this file's format; save a copy instead"
            )));
        };
        let settings = ExportSettings {
            format,
            ..self.export.clone()
        };
        let saving = self.save(self.path.clone(), settings, true, cx);
        cx.spawn(async move |this, cx| {
            let encoded = saving.await?;
            this.update(cx, |this, cx| {
                this.edit = ImageEdit::default();
                this.saved = ImageEdit::default();
                this.history.clear();
                this.state = State::Loading;
                this.load(cx);
                cx.notify();
            })?;
            Ok(encoded)
        })
    }
}

/// `size` made to fit in a `side` square, keeping its proportions.
fn fit_within(size: (u32, u32), side: u32) -> (u32, u32) {
    let longest = size.0.max(size.1);
    if longest <= side {
        return size;
    }
    let scale = side as f32 / longest as f32;
    (
        ((size.0 as f32 * scale).round() as u32).max(1),
        ((size.1 as f32 * scale).round() as u32).max(1),
    )
}

/// Writes `bytes` to `path` through a file beside it, so a failed write
/// never leaves half a picture.
fn write(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    if !replace && path.exists() {
        bail!("{} exists already", path.display());
    }
    let folder = path.parent().context("the file has no folder")?;
    let name = path
        .file_name()
        .context("the file has no name")?
        .to_string_lossy();
    let temporary = folder.join(format!(".{name}.pilothouse-{}", std::process::id()));
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("could not write in {}", folder.display()))?;
    std::fs::rename(&temporary, path).map_err(|err| {
        let _ = std::fs::remove_file(&temporary);
        anyhow::anyhow!("could not save {}: {err}", path.display())
    })
}

/// Reads the picture at `path`, turned as its camera recorded.
fn read(path: &Path) -> Result<Original> {
    let bytes_len = std::fs::metadata(path)
        .with_context(|| format!("could not read {}", path.display()))?
        .len();
    if bytes_len > MAX_FILE {
        bail!("this picture is too large to edit here");
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .context("could not read the picture")?;
    let format = reader.format().and_then(|format| match format {
        image::ImageFormat::Png => Some(Format::Png),
        image::ImageFormat::Jpeg => Some(Format::Jpeg),
        image::ImageFormat::WebP => Some(Format::Webp),
        image::ImageFormat::Gif => Some(Format::Gif),
        image::ImageFormat::Bmp => Some(Format::Bmp),
        image::ImageFormat::Tiff => Some(Format::Tiff),
        image::ImageFormat::Ico => Some(Format::Ico),
        _ => None,
    });
    let mut decoder = reader
        .into_decoder()
        .context("this picture's format cannot be edited")?;
    let (width, height) = decoder.dimensions();
    if width > MAX_SIDE || height > MAX_SIDE || width as u64 * height as u64 > MAX_PIXELS {
        bail!("this picture is too large to edit here ({width} × {height} pixels)");
    }
    let orientation = decoder.orientation().ok();
    let mut picture =
        image::DynamicImage::from_decoder(decoder).context("could not read the picture")?;
    if let Some(orientation) = orientation {
        picture.apply_orientation(orientation);
    }
    let full = picture.into_rgba8();
    let size = full.dimensions();
    let small = if size.0.max(size.1) > PREVIEW_SIDE {
        let (width, height) = fit_within(size, PREVIEW_SIDE);
        imageops::thumbnail(&full, width, height)
    } else {
        full.clone()
    };
    let frame = Frame::from_rgba_slice(small.width(), small.height(), small.as_raw());
    Ok(Original {
        full: Arc::new(full),
        small: Arc::new(small),
        size,
        format,
        bytes: bytes_len,
        frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_keeps_proportions() {
        assert_eq!(fit_within((4000, 3000), 2048), (2048, 1536));
        assert_eq!(fit_within((100, 50), 2048), (100, 50));
        assert_eq!(fit_within((10, 5000), 1000), (2, 1000));
    }

    #[test]
    fn writes_never_replace_unless_asked() {
        let dir = std::env::temp_dir().join(format!("ph-studio-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        write(&path, b"one", false).unwrap();
        assert!(write(&path, b"two", false).is_err());
        write(&path, b"two", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no leftovers");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
