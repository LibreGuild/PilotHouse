//! One presentation being edited: its deck and the deck's history, saved
//! to its space a moment after each change, and pictures of its slides,
//! drawn off the UI thread and kept until the slide changes.
//!
//! The editor asks for a slide's picture where it draws one
//! ([`DeckDocument::picture`]); the last picture is shown while a new one is
//! made, and nothing is drawn while nothing changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{AppContext as _, Context, RenderImage, Task};
use ph_media::Frame;

use super::model::{Deck, Slide};
use super::ops::{self, Op};
use super::space::{self, Space};
use super::svg::{self, Options};
use crate::history::History;

/// How long after the last change the deck is saved.
const SAVE_DELAY: Duration = Duration::from_millis(700);
/// The list's picture of a presentation, this wide.
const THUMBNAIL_WIDTH: u32 = 480;
/// Pictures drawn in one go off the UI thread (the canvas's goes alone).
const BATCH: usize = 6;

/// What a slide's picture is for: each is kept apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Purpose {
    /// The slide being edited.
    Canvas,
    /// Presenting: the slide shown.
    Present,
    /// Presenting: the slide with only the elements come in by this many
    /// builds.
    Step(u16),
    /// Presenting: an element alone (keyed by its id), coming in.
    Layer,
    /// Presenting: the next slide, in the presenter's view.
    Next,
    /// The rail's small picture.
    Rail,
}

/// What the user has selected, for the agent to know what "this" is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub slide: Option<String>,
    pub elements: Vec<String>,
    /// Text selected in the element being typed in.
    pub text: Option<String>,
}

/// A slide's picture as made.
struct Picture {
    /// The slide it shows (kept, so it is compared by address safely).
    slide: Arc<Slide>,
    looks: u64,
    width: u32,
    hide: Option<String>,
    hidden: Vec<String>,
    image: Arc<RenderImage>,
    /// A layer's place on the slide (points: left, top, width, height).
    bounds: [f32; 4],
}

/// A picture to make.
struct Job {
    /// The slide's id, or for a layer its element's.
    id: String,
    purpose: Purpose,
    slide: Arc<Slide>,
    looks: u64,
    width: u32,
    hide: Option<String>,
    /// Elements left out (those not come in yet).
    hidden: Vec<String>,
}

pub struct DeckDocument {
    space: Space,
    /// The presentation's space in the store; its agents edit it.
    project: Option<i64>,
    deck: Deck,
    history: History<Deck>,
    /// Bumped on every change.
    revision: u64,
    saved: u64,
    _save: Option<Task<()>>,
    save_error: Option<String>,
    /// Bumped when what every slide looks like changes (theme, size).
    looks: u64,
    pictures: HashMap<(String, Purpose), Picture>,
    jobs: Vec<Job>,
    drawing: bool,
    _draw: Option<Task<()>>,
    /// Pictures replaced, for the window to free on the GPU.
    stale: Vec<Arc<RenderImage>>,
    /// The slides the canvas showed last (at most two), oldest first.
    canvas_slides: Vec<String>,
    /// The first slide as last drawn for the list of presentations.
    thumbnail: Option<(Arc<Slide>, u64)>,
    pub selection: Selection,
    /// When the user or an agent last used it: agents edit the latest.
    pub(crate) used: Instant,
}

impl DeckDocument {
    pub fn new(space: Space, deck: Deck, project: Option<i64>, cx: &mut Context<Self>) -> Self {
        // Fonts load off the UI thread before the first slide needs them;
        // the list of presentations gets its picture if it has none.
        let missing =
            (!space.path.join(space::THUMBNAIL).exists()).then(|| (space.clone(), deck.clone()));
        cx.background_spawn(async move {
            let _ = super::fonts::fonts();
            if let Some((space, deck)) = missing {
                write_thumbnail(&space, &deck);
            }
        })
        .detach();
        let this = Self {
            space,
            project,
            deck,
            history: History::default(),
            revision: 0,
            saved: 0,
            _save: None,
            save_error: None,
            looks: 0,
            pictures: HashMap::new(),
            jobs: Vec::new(),
            drawing: false,
            _draw: None,
            stale: Vec::new(),
            canvas_slides: Vec::new(),
            thumbnail: None,
            selection: Selection::default(),
            used: Instant::now(),
        };
        super::control::register(cx.entity().downgrade(), cx);
        this
    }

    /// Opens the presentation in `folder`.
    pub fn open(folder: &Path, project: Option<i64>, cx: &mut Context<Self>) -> Result<Self> {
        let (space, deck) = Space::open(folder)?;
        Ok(Self::new(space, deck, project, cx))
    }

    pub fn deck(&self) -> &Deck {
        &self.deck
    }

    pub fn space(&self) -> &Space {
        &self.space
    }

    pub fn folder(&self) -> &Path {
        &self.space.path
    }

    pub fn media(&self) -> PathBuf {
        self.space.media()
    }

    pub fn project(&self) -> Option<i64> {
        self.project
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Why the last save failed, if it did.
    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    /// Whether changes are waiting to be saved.
    pub fn is_dirty(&self) -> bool {
        self.saved != self.revision
    }

    /// Marks it used now (the user or an agent at work on it).
    pub fn touch(&mut self) {
        self.used = Instant::now();
    }

    /// Applies `ops` as one undo step (all or none): what each made or
    /// said.
    pub fn apply(
        &mut self,
        what: &'static str,
        ops: Vec<Op>,
        cx: &mut Context<Self>,
    ) -> Result<Vec<Option<String>>> {
        let mut next = self.deck.clone();
        let mut results = Vec::new();
        for (ix, op) in ops.into_iter().enumerate() {
            results.push(
                ops::apply(&mut next, op).map_err(|err| anyhow!("change {}: {err:#}", ix + 1))?,
            );
        }
        self.commit(what, next, cx);
        Ok(results)
    }

    /// Changes the deck directly (`what` names the change: changes of the
    /// same thing in quick succession, a drag, undo as one).
    pub fn update(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut Deck),
        cx: &mut Context<Self>,
    ) {
        let mut next = self.deck.clone();
        change(&mut next);
        self.commit(what, next, cx);
    }

    /// Takes `next` as the deck, as one undo step.
    pub fn commit(&mut self, what: &'static str, mut next: Deck, cx: &mut Context<Self>) {
        // Connectors go with the elements they are fixed to.
        super::connectors::follow(&mut next, Some(&self.deck));
        super::connectors::keep_finite(&mut next, Some(&self.deck));
        if next == self.deck {
            return;
        }
        let restyled = looks_differ(&self.deck, &next);
        let before = std::mem::replace(&mut self.deck, next);
        self.history.record(what, before);
        self.changed(restyled, cx);
    }

    /// Ends folding changes: the next one is its own undo step.
    pub fn settle(&mut self) {
        self.history.settle();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let looked = (self.deck.theme.clone(), self.deck.size.clone());
        let current = std::mem::take(&mut self.deck);
        match self.history.undo(current) {
            Ok(previous) => {
                let restyled = previous.theme != looked.0 || previous.size != looked.1;
                self.deck = previous;
                self.changed(restyled, cx);
            }
            Err(current) => self.deck = current,
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let looked = (self.deck.theme.clone(), self.deck.size.clone());
        let current = std::mem::take(&mut self.deck);
        match self.history.redo(current) {
            Ok(next) => {
                let restyled = next.theme != looked.0 || next.size != looked.1;
                self.deck = next;
                self.changed(restyled, cx);
            }
            Err(current) => self.deck = current,
        }
    }

    /// Remembers that the PowerPoint file the deck was opened from now
    /// holds it (as of `stamp`, its change time), so opening that file
    /// again reopens this deck.
    pub fn saved_original(&mut self, stamp: Option<u64>, cx: &mut Context<Self>) {
        if let Some(source) = self.deck.source.as_mut() {
            source.stamp = stamp;
            self.changed(false, cx);
        }
    }

    fn changed(&mut self, restyled: bool, cx: &mut Context<Self>) {
        if restyled {
            self.looks += 1;
        }
        self.revision += 1;
        self.used = Instant::now();
        // Pictures of slides that are gone are let go.
        let ids: std::collections::HashSet<&str> = self
            .deck
            .slides
            .iter()
            .map(|slide| slide.id.as_str())
            .collect();
        let gone: Vec<(String, Purpose)> = self
            .pictures
            .keys()
            // Layers are an element's, let go when presenting ends.
            .filter(|(id, purpose)| *purpose != Purpose::Layer && !ids.contains(id.as_str()))
            .cloned()
            .collect();
        for key in gone {
            if let Some(picture) = self.pictures.remove(&key) {
                self.stale.push(picture.image);
            }
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let revision = self.revision;
        self._save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Ok((space, deck, thumbnail, stamp)) = this.update(cx, |this, _| {
                let first = this.deck.slides.first().cloned();
                let thumbnail = first.filter(|first| {
                    this.thumbnail.as_ref().is_none_or(|(slide, looks)| {
                        !Arc::ptr_eq(slide, first) || *looks != this.looks
                    })
                });
                let stamp = super::space::snapshot_stamp();
                (this.space.clone(), this.deck.clone(), thumbnail, stamp)
            }) else {
                return;
            };
            let redraw = thumbnail.is_some();
            let written = cx
                .background_spawn(async move {
                    space.save_stamped(&deck, stamp)?;
                    if redraw {
                        write_thumbnail(&space, &deck);
                    }
                    anyhow::Ok(())
                })
                .await;
            _ = this.update(cx, |this, cx| {
                match written {
                    Ok(()) => {
                        this.saved = revision.max(this.saved);
                        this.save_error = None;
                        if let Some(slide) = thumbnail {
                            this.thumbnail = Some((slide, this.looks));
                        }
                    }
                    Err(err) => this.save_error = Some(format!("{err:#}")),
                }
                cx.notify();
            });
        }));
    }

    /// Saves now, on this thread (the window closing).
    pub fn save_now(&mut self) -> Result<()> {
        if !self.is_dirty() {
            return Ok(());
        }
        self.space
            .save_stamped(&self.deck, super::space::snapshot_stamp())?;
        write_thumbnail(&self.space, &self.deck);
        self.saved = self.revision;
        self._save = None;
        Ok(())
    }

    /// Renames the presentation (its title, and its folder when free).
    pub fn rename(&mut self, title: &str, cx: &mut Context<Self>) {
        let title = title.trim();
        if title.is_empty() || title == self.deck.title {
            return;
        }
        self.update("title", |deck| deck.title = title.to_string(), cx);
    }

    /// The picture of slide `index` for `purpose`, `width` pixels wide
    /// (with the text of element `hide` left out): the last one made, and
    /// a new one is made if that is out of date.
    pub fn picture(
        &mut self,
        index: usize,
        purpose: Purpose,
        width: u32,
        hide: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let slide = self.deck.slides.get(index)?.clone();
        let width = width.clamp(16, 7680);
        if purpose == Purpose::Canvas {
            self.keep_canvas(&slide.id);
        }
        let key = (slide.id.clone(), purpose);
        let current = self.pictures.get(&key);
        let fresh = current.is_some_and(|picture| {
            drawn_alike(&picture.slide, &slide)
                && picture.looks == self.looks
                && picture.hide.as_deref() == hide
                && picture.hidden.is_empty()
                // Small changes of size keep the picture (a window
                // dragged wider redraws once it settles a little).
                && (picture.width as f32 / width as f32 - 1.).abs() < 0.12
        });
        if !fresh {
            self.want(
                Job {
                    id: slide.id.clone(),
                    purpose,
                    slide,
                    looks: self.looks,
                    width,
                    hide: hide.map(str::to_string),
                    hidden: Vec::new(),
                },
                cx,
            );
        }
        self.pictures.get(&key).map(|picture| picture.image.clone())
    }

    /// Presenting: slide `index` after `step` clicks, `hidden` (those not
    /// come in yet) left out.
    pub fn step_picture(
        &mut self,
        index: usize,
        step: u16,
        width: u32,
        hidden: &[String],
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let slide = self.deck.slides.get(index)?.clone();
        let width = width.clamp(16, 7680);
        let key = (slide.id.clone(), Purpose::Step(step));
        let fresh = self.pictures.get(&key).is_some_and(|picture| {
            drawn_alike(&picture.slide, &slide)
                && picture.looks == self.looks
                && picture.hidden == hidden
                && (picture.width as f32 / width as f32 - 1.).abs() < 0.12
        });
        if !fresh {
            self.want(
                Job {
                    id: slide.id.clone(),
                    purpose: Purpose::Step(step),
                    slide,
                    looks: self.looks,
                    width,
                    hide: None,
                    hidden: hidden.to_vec(),
                },
                cx,
            );
        }
        self.pictures.get(&key).map(|picture| picture.image.clone())
    }

    /// Presenting: element `id` of slide `index` alone, `scale` pixels a
    /// point, and where it goes (points).
    pub fn layer(
        &mut self,
        index: usize,
        id: &str,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Option<(Arc<RenderImage>, [f32; 4])> {
        let slide = self.deck.slides.get(index)?.clone();
        let element = slide.elements.iter().find(|element| element.id == id)?;
        let [l, _, r, _] = super::editor::geometry::bounds(&element.frame);
        let width = (((r - l).max(1.) + 48.) * scale).round().clamp(4., 7680.) as u32;
        let key = (id.to_string(), Purpose::Layer);
        let fresh = self.pictures.get(&key).is_some_and(|picture| {
            drawn_alike(&picture.slide, &slide)
                && picture.looks == self.looks
                && (picture.width as f32 / width as f32 - 1.).abs() < 0.12
        });
        if !fresh {
            self.want(
                Job {
                    id: id.to_string(),
                    purpose: Purpose::Layer,
                    slide,
                    looks: self.looks,
                    width,
                    hide: None,
                    hidden: Vec::new(),
                },
                cx,
            );
        }
        self.pictures
            .get(&key)
            .map(|picture| (picture.image.clone(), picture.bounds))
    }

    fn want(&mut self, job: Job, cx: &mut Context<Self>) {
        if let Some(queued) = self
            .jobs
            .iter_mut()
            .find(|queued| queued.id == job.id && queued.purpose == job.purpose)
        {
            *queued = job;
        } else {
            self.jobs.push(job);
        }
        if !self.drawing {
            self.draw(cx);
        }
    }

    /// Pictures of slides that are not shown are let go (`keep` are the
    /// rail's slides near the view).
    pub fn forget_rail(&mut self, keep: &dyn Fn(&str) -> bool) {
        let drop: Vec<(String, Purpose)> = self
            .pictures
            .keys()
            .filter(|(id, purpose)| *purpose == Purpose::Rail && !keep(id))
            .cloned()
            .collect();
        for key in drop {
            if let Some(picture) = self.pictures.remove(&key) {
                self.stale.push(picture.image);
            }
        }
        self.jobs
            .retain(|job| job.purpose != Purpose::Rail || keep(&job.id));
    }

    /// Lets go of the pictures for presenting that `keep` (given the
    /// slide's id, for a layer the id of the slide its element is on) does
    /// not want.
    pub fn keep_presenting(&mut self, keep: impl Fn(&str, Purpose) -> bool) {
        let slide_of = |element: &str| {
            self.deck
                .slides
                .iter()
                .find(|slide| slide.elements.iter().any(|known| known.id == element))
                .map(|slide| slide.id.clone())
        };
        let drop: Vec<(String, Purpose)> = self
            .pictures
            .keys()
            .filter(|(id, purpose)| match purpose {
                Purpose::Present | Purpose::Next | Purpose::Step(_) => !keep(id, *purpose),
                Purpose::Layer => !slide_of(id).is_some_and(|slide| keep(&slide, *purpose)),
                _ => false,
            })
            .cloned()
            .collect();
        for key in drop {
            if let Some(picture) = self.pictures.remove(&key) {
                self.stale.push(picture.image);
            }
        }
    }

    /// Lets go of the pictures for presenting.
    pub fn forget_presenting(&mut self) {
        let drop: Vec<(String, Purpose)> = self
            .pictures
            .keys()
            .filter(|(_, purpose)| {
                matches!(
                    purpose,
                    Purpose::Present | Purpose::Next | Purpose::Step(_) | Purpose::Layer
                )
            })
            .cloned()
            .collect();
        for key in drop {
            if let Some(picture) = self.pictures.remove(&key) {
                self.stale.push(picture.image);
            }
        }
    }

    /// The pictures let go since last asked; the caller frees them on the
    /// GPU in every window (`cx.drop_image(image, Some(window))`), as any
    /// window showing the deck may have drawn them.
    pub fn take_stale(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.stale)
    }

    /// The canvas shows one slide: pictures of it are kept for that slide
    /// and the one before (going back is instant), the others let go (one
    /// can be 130 MB zoomed in).
    fn keep_canvas(&mut self, id: &str) {
        if self.canvas_slides.last().map(String::as_str) == Some(id) {
            return;
        }
        self.canvas_slides.retain(|known| known != id);
        self.canvas_slides.push(id.to_string());
        while self.canvas_slides.len() > 2 {
            let gone = self.canvas_slides.remove(0);
            if let Some(picture) = self.pictures.remove(&(gone, Purpose::Canvas)) {
                self.stale.push(picture.image);
            }
        }
    }

    /// Frees pictures let go while no window drew (and so took them), but
    /// the last few, which a window may still show.
    fn trim_stale(&mut self, cx: &mut Context<Self>) {
        const KEEP: usize = 6;
        if self.stale.len() > KEEP {
            let extra = self.stale.len() - KEEP;
            for image in self.stale.drain(..extra) {
                ph_ui::images::release_render_image(image, cx);
            }
        }
    }

    /// Makes the wanted pictures, most wanted first, off the UI thread.
    fn draw(&mut self, cx: &mut Context<Self>) {
        if self.jobs.is_empty() {
            self.drawing = false;
            return;
        }
        self.jobs.sort_by_key(|job| job.purpose);
        let take = if self.jobs[0].purpose == Purpose::Rail {
            BATCH.min(self.jobs.len())
        } else {
            1
        };
        let batch: Vec<Job> = self.jobs.drain(..take).collect();
        self.drawing = true;
        // The deck as it is, sharing its slides: each job draws its own
        // slide (as it was asked for) with today's theme.
        let mut deck = self.deck.clone();
        let media = self.media();
        let made = cx.background_spawn(async move {
            batch
                .into_iter()
                .map(|job| {
                    deck.slides = vec![job.slide.clone()];
                    let options = Options {
                        media: Some(&media),
                        editing: job.purpose == Purpose::Canvas,
                        hide_text: job.hide.as_deref(),
                        small: job.purpose == Purpose::Rail,
                        hidden: &job.hidden,
                    };
                    let (svg, bounds) = if job.purpose == Purpose::Layer {
                        match svg::element_layer(&deck, 0, &job.id, &options) {
                            Some(layer) => layer,
                            None => return (job, None, [0.; 4]),
                        }
                    } else {
                        (
                            svg::slide(&deck, 0, &options),
                            [0., 0., deck.size.width, deck.size.height],
                        )
                    };
                    let image = super::raster::rasterize(&svg, job.width).ok().map(|image| {
                        let (width, height) = image.dimensions();
                        Frame::from_rgba(width, height, image.into_raw()).render_image()
                    });
                    (job, image, bounds)
                })
                .collect::<Vec<_>>()
        });
        self._draw = Some(cx.spawn(async move |this, cx| {
            let made = made.await;
            _ = this.update(cx, |this, cx| {
                for (job, image, bounds) in made {
                    let Some(image) = image else {
                        continue;
                    };
                    // A slide gone meanwhile is not kept.
                    if !this
                        .deck
                        .slides
                        .iter()
                        .any(|slide| slide.id == job.slide.id)
                    {
                        this.stale.push(image);
                        continue;
                    }
                    let picture = Picture {
                        slide: job.slide,
                        looks: job.looks,
                        width: job.width,
                        hide: job.hide,
                        hidden: job.hidden,
                        image,
                        bounds,
                    };
                    if let Some(old) = this.pictures.insert((job.id, job.purpose), picture) {
                        this.stale.push(old.image);
                    }
                }
                this.trim_stale(cx);
                this.draw(cx);
                cx.notify();
            });
        }));
    }

    /// Where exports go by default: `exports/<title>.<extension>`.
    pub fn export_path(&self, extension: &str) -> PathBuf {
        let name = space::folder_name(&self.deck.title);
        self.space.exports().join(format!("{name}.{extension}"))
    }

    /// Writes the deck as `format` to `path` off the UI thread: `slides`
    /// (indexes; all when empty), `notes` pages (PDF), `scale` (PNG: the
    /// width is 1920 × scale). PNGs of several slides go in a folder.
    pub fn export(
        &self,
        format: ExportFormat,
        path: PathBuf,
        slides: Vec<usize>,
        notes: bool,
        width: u32,
        cx: &mut Context<Self>,
    ) -> Task<Result<PathBuf>> {
        let deck = self.deck.clone();
        let media = self.media();
        cx.background_spawn(async move {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match format {
                ExportFormat::Pptx => super::pptx::write(&deck, Some(&media), &path)?,
                ExportFormat::Pdf => super::pdf::write(&deck, Some(&media), &slides, notes, &path)?,
                ExportFormat::Png => {
                    let chosen: Vec<usize> = if slides.is_empty() {
                        (0..deck.slides.len()).collect()
                    } else {
                        slides
                    };
                    if chosen.len() == 1 {
                        super::raster::slide(&deck, chosen[0], width, Some(&media), false)?
                            .save(&path)
                            .with_context(|| format!("could not write {}", path.display()))?;
                    } else {
                        // Several: a folder named for the file, a picture a slide.
                        let folder = path.with_extension("");
                        std::fs::create_dir_all(&folder)?;
                        for index in chosen {
                            super::raster::slide(&deck, index, width, Some(&media), false)?
                                .save(folder.join(format!("Slide {}.png", index + 1)))?;
                        }
                        return Ok(folder);
                    }
                }
            }
            Ok(path)
        })
    }
}

/// What a deck is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Pptx,
    Pdf,
    Png,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Pptx => "pptx",
            ExportFormat::Pdf => "pdf",
            ExportFormat::Png => "png",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "pptx" => Some(ExportFormat::Pptx),
            "pdf" => Some(ExportFormat::Pdf),
            "png" => Some(ExportFormat::Png),
            _ => None,
        }
    }
}

/// The picture of the first slide the list of presentations shows.
fn write_thumbnail(space: &Space, deck: &Deck) {
    if deck.slides.is_empty() {
        let _ = std::fs::remove_file(space.path.join(space::THUMBNAIL));
        return;
    }
    let media = space.media();
    let svg = svg::slide(
        deck,
        0,
        &Options {
            media: Some(&media),
            editing: false,
            hide_text: None,
            small: true,
            hidden: &[],
        },
    );
    if let Ok(image) = super::raster::rasterize(&svg, THUMBNAIL_WIDTH) {
        let _ = image.save(space.path.join(space::THUMBNAIL));
    }
}

/// Whether every slide of `b` looks otherwise than in `a`.
fn looks_differ(a: &Deck, b: &Deck) -> bool {
    a.theme != b.theme || a.size != b.size
}

/// Whether two versions of a slide look the same (what is drawn: not its
/// notes, comments or animations), so a picture of one shows the other.
fn drawn_alike(a: &Arc<Slide>, b: &Arc<Slide>) -> bool {
    Arc::ptr_eq(a, b)
        || (a.id == b.id
            && a.layout == b.layout
            && a.background == b.background
            && a.elements == b.elements)
}
