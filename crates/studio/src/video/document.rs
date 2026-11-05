//! One video being edited: what it is (read once, off the UI thread), its
//! frames' times, the edit and its history, small pictures for the
//! timeline, exact stills of the result, and exports under way.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, RenderImage, SharedString, Task};
use image::{RgbaImage, imageops};
use ph_media::Frame;

use super::compose::Composer;
use super::edit::{VideoEdit, VideoExport};
use super::export::{self, Exported, Progress};
use super::media::{self, Info};
use crate::history::History;

/// Stills of the result are made at most this many pixels a side.
const STILL_SIDE: u32 = 1600;
/// Timeline pictures are this tall.
pub const THUMB_HEIGHT: u32 = 56;
/// New timeline pictures show at most this often.
const THUMB_BATCH: std::time::Duration = std::time::Duration::from_millis(700);

enum State {
    Loading,
    Failed(SharedString),
    Ready(Info),
}

/// An export under way.
pub struct Job {
    pub progress: Arc<Progress>,
    pub out: PathBuf,
    _task: Task<()>,
}

pub struct VideoDocument {
    path: PathBuf,
    project: Option<i64>,
    state: State,
    /// When each frame starts, once read (for stepping and numbering).
    frames: Vec<f64>,
    edit: VideoEdit,
    saved: VideoEdit,
    history: History<VideoEdit>,
    export: VideoExport,
    /// Timeline pictures by source time (in quarter seconds).
    thumbs: HashMap<i64, Frame>,
    thumbs_wanted: Vec<i64>,
    _thumbs: Option<Task<()>>,
    /// An exact still of the result: source time, the edit it shows, the
    /// picture and its pixels.
    still: Option<(f64, VideoEdit, Frame, Arc<RgbaImage>)>,
    _still: Option<Task<()>>,
    job: Option<Job>,
    /// The last export's outcome, for the window to tell.
    pub(crate) finished: Option<Result<Exported, String>>,
    stale: Vec<Arc<RenderImage>>,
    pub(crate) used: Instant,
}

impl VideoDocument {
    pub fn new(path: PathBuf, project: Option<i64>, cx: &mut Context<Self>) -> Self {
        let this = Self {
            path: path.clone(),
            project,
            state: State::Loading,
            frames: Vec::new(),
            edit: VideoEdit::default(),
            saved: VideoEdit::default(),
            history: History::default(),
            export: VideoExport::default(),
            thumbs: HashMap::new(),
            thumbs_wanted: Vec::new(),
            _thumbs: None,
            still: None,
            _still: None,
            job: None,
            finished: None,
            stale: Vec::new(),
            used: Instant::now(),
        };
        cx.spawn(async move |this, cx| {
            let probe_path = path.clone();
            let info = cx
                .background_spawn(async move { media::probe(&probe_path) })
                .await;
            let frames = match &info {
                Ok(info) => {
                    let (path, info) = (path.clone(), info.clone());
                    cx.background_spawn(async move { media::frame_times(&path, &info) })
                        .await
                        .unwrap_or_default()
                }
                Err(_) => Vec::new(),
            };
            _ = this.update(cx, |this, cx| {
                this.state = match info {
                    Ok(info) => State::Ready(info),
                    Err(err) => State::Failed(format!("{err:#}").into()),
                };
                this.frames = frames;
                cx.notify();
            });
        })
        .detach();
        crate::control::register_video(cx.entity().downgrade(), cx);
        // Its window closed: an export under way stops (it would go on
        // using the processor, and write a file no one is told about).
        cx.on_release(|this, _| this.cancel_export()).detach();
        this
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn project(&self) -> Option<i64> {
        self.project
    }

    pub fn info(&self) -> Option<&Info> {
        match &self.state {
            State::Ready(info) => Some(info),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&SharedString> {
        match &self.state {
            State::Failed(error) => Some(error),
            _ => None,
        }
    }

    pub fn edit(&self) -> &VideoEdit {
        &self.edit
    }

    pub fn export(&self) -> &VideoExport {
        &self.export
    }

    pub fn frames(&self) -> &[f64] {
        &self.frames
    }

    pub fn is_dirty(&self) -> bool {
        self.edit != self.saved
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn job(&self) -> Option<&Job> {
        self.job.as_ref()
    }

    pub fn touch(&mut self) {
        self.used = Instant::now();
    }

    /// The source's length (0 until read).
    pub fn duration(&self) -> f64 {
        self.info().map_or(0., |info| info.duration)
    }

    pub fn size(&self) -> (u32, u32) {
        self.info().map_or((0, 0), |info| info.size)
    }

    /// The frame starting at or before source time `t`: its index and
    /// start (none while the times are not read).
    pub fn frame_at(&self, t: f64) -> Option<(usize, f64)> {
        if self.frames.is_empty() {
            return None;
        }
        let ix = self
            .frames
            .partition_point(|start| *start <= t + 1e-6)
            .saturating_sub(1);
        Some((ix, self.frames[ix]))
    }

    /// Changes the edit (checked); changes of the same `what` in quick
    /// succession undo as one.
    pub fn update(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut VideoEdit, f64, (u32, u32)),
        cx: &mut Context<Self>,
    ) {
        let (duration, size) = (self.duration(), self.size());
        let mut next = self.edit.clone();
        change(&mut next, duration, size);
        if next.sanitize(duration, size).is_err() || next == self.edit {
            return;
        }
        self.commit(what, next, cx);
    }

    pub(crate) fn commit(&mut self, what: &'static str, next: VideoEdit, cx: &mut Context<Self>) {
        let before = std::mem::replace(&mut self.edit, next);
        self.history.record(what, before);
        self.changed(cx);
    }

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

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if !self.edit.is_unchanged() {
            self.history.settle();
            self.commit("reset", VideoEdit::default(), cx);
        }
    }

    pub fn set_export(&mut self, change: impl FnOnce(&mut VideoExport), cx: &mut Context<Self>) {
        let mut next = self.export.clone();
        change(&mut next);
        next.sanitize();
        if next != self.export {
            self.export = next;
            cx.notify();
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.used = Instant::now();
        cx.notify();
    }

    /// Pictures replaced, for the window to free on the GPU.
    pub fn take_stale(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.stale)
    }

    // Timeline pictures.

    /// The timeline picture nearest source time `t`, if made.
    pub fn thumb(&self, t: f64) -> Option<&Frame> {
        let key = (t * 4.).round() as i64;
        self.thumbs.get(&key).or_else(|| {
            self.thumbs
                .iter()
                .min_by_key(|(at, _)| (**at - key).abs())
                .filter(|(at, _)| (**at - key).abs() <= 8)
                .map(|(_, frame)| frame)
        })
    }

    /// Keeps the timeline pictures to a few hundred (zooming and trimming
    /// asks for new times): those far from the ones last wanted go, freed
    /// on the GPU.
    fn trim_thumbs(&mut self, cx: &mut Context<Self>) {
        const MOST: usize = 300;
        if self.thumbs.len() <= MOST {
            return;
        }
        let wanted = &self.thumbs_wanted;
        let distance = |key: i64| {
            wanted
                .iter()
                .map(|want| (want - key).abs())
                .min()
                .unwrap_or(i64::MAX)
        };
        let mut keys: Vec<i64> = self.thumbs.keys().copied().collect();
        keys.sort_by_key(|key| std::cmp::Reverse(distance(*key)));
        let extra = self.thumbs.len() - MOST;
        for key in keys.into_iter().take(extra) {
            if let Some(frame) = self.thumbs.remove(&key) {
                ph_ui::images::release_render_image(frame.render_image(), cx);
            }
        }
    }

    /// Makes timeline pictures for source times `times` (those not made
    /// yet), off the UI thread, one batch at a time.
    pub fn want_thumbs(&mut self, times: &[f64], cx: &mut Context<Self>) {
        let Some(info) = self.info().cloned() else {
            return;
        };
        let keys: Vec<i64> = times
            .iter()
            .map(|t| (t * 4.).round() as i64)
            .filter(|key| !self.thumbs.contains_key(key))
            .collect();
        if keys.is_empty() || keys == self.thumbs_wanted {
            return;
        }
        self.thumbs_wanted = keys.clone();
        let path = self.path.clone();
        self._thumbs = Some(cx.spawn(async move |this, cx| {
            // Shown in batches: a redraw per picture would keep the window
            // busy (and its tooltips from ever showing) while they come.
            let mut made = Vec::new();
            let mut shown_at = Instant::now();
            let count = keys.len();
            for (ix, key) in keys.into_iter().enumerate() {
                let (path, info) = (path.clone(), info.clone());
                let thumb = cx
                    .background_spawn(async move {
                        let frame = media::frame_at(&path, &info, key as f64 / 4.).ok()?;
                        let width = (frame.width() as f64 * THUMB_HEIGHT as f64 * 2.
                            / frame.height().max(1) as f64)
                            .round() as u32;
                        let small = imageops::thumbnail(&frame, width.max(1), THUMB_HEIGHT * 2);
                        Some(Frame::from_rgba_slice(
                            small.width(),
                            small.height(),
                            small.as_raw(),
                        ))
                    })
                    .await;
                made.extend(thumb.map(|frame| (key, frame)));
                let last = ix + 1 == count;
                if made.is_empty() || !(last || shown_at.elapsed() >= THUMB_BATCH) {
                    continue;
                }
                shown_at = Instant::now();
                let batch = std::mem::take(&mut made);
                let alive = this
                    .update(cx, |this, cx| {
                        this.thumbs.extend(batch);
                        this.trim_thumbs(cx);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    // Exact stills.

    /// The exact still at source time `t` for the edit as it is, if made.
    pub fn still(&self, t: f64) -> Option<&Frame> {
        self.still
            .as_ref()
            .filter(|(at, edit, ..)| (*at - t).abs() < 1e-4 && *edit == self.edit)
            .map(|(_, _, frame, _)| frame)
    }

    /// Makes the still at source time `t` (the one asked for last wins).
    pub fn want_still(&mut self, t: f64, cx: &mut Context<Self>) {
        if self.still(t).is_some() {
            return;
        }
        let Some(info) = self.info().cloned() else {
            return;
        };
        let (path, edit) = (self.path.clone(), self.edit.clone());
        self._still = Some(cx.spawn(async move |this, cx| {
            let made_edit = edit.clone();
            let made = cx
                .background_spawn(async move { still(&path, &info, &edit, t) })
                .await;
            _ = this.update(cx, |this, cx| {
                if let Ok(image) = made {
                    let frame =
                        Frame::from_rgba_slice(image.width(), image.height(), image.as_raw());
                    if let Some((.., old, _)) =
                        this.still.replace((t, made_edit, frame, Arc::new(image)))
                    {
                        this.stale.push(old.render_image());
                    }
                    cx.notify();
                }
            });
        }));
    }

    /// The result's frame at source time `t`, for an agent to look at or
    /// to copy: at most `side` pixels a side, as PNG.
    pub fn snapshot(&self, t: f64, original: bool, side: u32, cx: &App) -> Task<Result<Vec<u8>>> {
        let Some(info) = self.info().cloned() else {
            return Task::ready(Err(anyhow::anyhow!("the video is not open yet")));
        };
        let path = self.path.clone();
        let edit = if original {
            VideoEdit::default()
        } else {
            self.edit.clone()
        };
        cx.background_spawn(async move {
            let image = still(&path, &info, &edit, t)?;
            let image = if image.width().max(image.height()) > side {
                let scale = side as f32 / image.width().max(image.height()) as f32;
                imageops::thumbnail(
                    &image,
                    ((image.width() as f32 * scale) as u32).max(1),
                    ((image.height() as f32 * scale) as u32).max(1),
                )
            } else {
                image
            };
            let mut bytes = Vec::new();
            image::ImageEncoder::write_image(
                image::codecs::png::PngEncoder::new(&mut bytes),
                &image,
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )?;
            Ok(bytes)
        })
    }

    // Exporting.

    /// Writes the result to `out`; the window shows how far it has got.
    pub fn start_export(
        &mut self,
        out: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<Result<Exported, String>> {
        let Some(info) = self.info().cloned() else {
            return Task::ready(Err("the video is not open yet".into()));
        };
        if self.job.is_some() {
            return Task::ready(Err("an export is under way".into()));
        }
        let progress = Arc::new(Progress::default());
        let (path, edit, settings) = (self.path.clone(), self.edit.clone(), self.export.clone());
        let (thread_progress, thread_out) = (progress.clone(), out.clone());
        // A thread of its own: AVFoundation's readers and writers stay on
        // the thread that made them, and an export takes a while.
        let (done, result) = futures::channel::oneshot::channel();
        std::thread::Builder::new()
            .name("studio-export".into())
            .spawn(move || {
                let outcome = export::export(
                    &path,
                    &info,
                    &edit,
                    &settings,
                    &thread_out,
                    &thread_progress,
                )
                .map_err(|err| format!("{err:#}"));
                let _ = done.send(outcome);
            })
            .ok();
        let saved_edit = self.edit.clone();
        let (finished, wait) = futures::channel::oneshot::channel();
        let task = cx.spawn(async move |this, cx| {
            let outcome = result
                .await
                .unwrap_or_else(|_| Err("the export stopped".into()));
            _ = this.update(cx, |this, cx| {
                this.job = None;
                if outcome.is_ok() {
                    this.saved = saved_edit;
                }
                this.finished = Some(outcome.clone());
                cx.notify();
            });
            let _ = finished.send(outcome);
        });
        self.job = Some(Job {
            progress,
            out,
            _task: task,
        });
        cx.notify();
        cx.spawn(async move |_, _| {
            wait.await
                .unwrap_or_else(|_| Err("the export stopped".into()))
        })
    }

    pub fn cancel_export(&mut self) {
        if let Some(job) = &self.job {
            job.progress.cancel();
        }
    }
}

/// The result's frame at source time `t`, at most [`STILL_SIDE`] a side.
fn still(path: &Path, info: &Info, edit: &VideoEdit, t: f64) -> Result<RgbaImage> {
    let frame = media::frame_at(path, info, t)?;
    let (width, height) = edit.output_size(info.size);
    let scale = (STILL_SIDE as f32 / width.max(height) as f32).min(1.);
    let target = (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    );
    Ok(Composer::new(edit.clone(), info.size).frame(&frame, t, target))
}
