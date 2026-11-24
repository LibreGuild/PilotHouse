//! The running app's decks: the kind added to the artifacts' server, a new
//! deck made, and what a deck's page tells the app passed on to whoever
//! shows that deck (its window).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui_kit::component::Theme;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription,
    Task, WeakEntity,
};
use ph_artifacts::library::Source;
use ph_artifacts::{Artifact, Artifacts};
use serde_json::Value;

use crate::deck::{DECK, Deck, KIND};
use crate::kind::{Handler, Slides, Told};
use crate::live::Change;
use crate::looks;
use crate::view::DeckView;

/// A deck that changes is noted in the list this often at most (its place
/// there is by when it last changed, not by every letter typed).
const TOUCH: Duration = Duration::from_secs(20);

/// Something a deck's page told the app: `what` is as the page sent it
/// (`{"present": "full"}`, `{"export": "pdf"}`, `{"comment": {…}}`), or
/// `{"changed": "page" | "outside" | "undo" | "redo"}` when its files changed.
#[derive(Debug, Clone)]
pub struct DeckEvent {
    /// The deck's artifact.
    pub id: String,
    pub what: Value,
}

pub struct Decks {
    slides: Arc<Slides>,
    /// The deck windows that are open: where a picture or a PDF of a deck
    /// is taken.
    views: HashMap<String, (WeakEntity<DeckView>, AnyWindowHandle)>,
    /// Decks drawn out of sight, in a window of the app that is open
    /// anyway: for a deck whose own window is not (one made from a
    /// workspace's chat and looked at beside it). The newest last.
    hidden: Vec<(String, Entity<DeckView>, AnyWindowHandle)>,
    touched: HashMap<String, Instant>,
    /// Pictures for the Artifacts list being taken of decks drawn out of
    /// sight (a deck's own window takes its own).
    picturing: HashMap<String, Task<()>>,
    _heard: Task<()>,
    _theme: Subscription,
}

impl EventEmitter<DeckEvent> for Decks {}

struct Hub(Entity<Decks>);

impl Global for Hub {}

/// Starts the decks, where the artifacts were started. Call once, after
/// `ph_artifacts::init`.
pub fn init(cx: &mut App) {
    let Some(artifacts) = Artifacts::global(cx) else {
        return;
    };
    let server = artifacts.read(cx).server().clone();
    let (slides, heard) = Slides::new(server.clone());
    slides.set_chrome(chrome(cx));
    server.add_kind(KIND, Arc::new(Handler(slides.clone())));
    let decks = cx.new(|cx| Decks {
        slides,
        views: HashMap::new(),
        hidden: Vec::new(),
        touched: HashMap::new(),
        picturing: HashMap::new(),
        _heard: cx.spawn(async move |this, cx| {
            while let Ok(told) = heard.recv().await {
                if this
                    .update(cx, |this: &mut Decks, cx| this.heard(told, cx))
                    .is_err()
                {
                    break;
                }
            }
        }),
        // The app's look changed: the decks' pages take it on.
        _theme: cx.observe_global::<Theme>(|this: &mut Decks, cx| {
            this.slides.set_chrome(chrome(cx));
        }),
    });
    cx.set_global(Hub(decks));
}

impl Decks {
    /// The app's decks, where the build has them and they were started.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Hub>().map(|hub| hub.0.clone())
    }

    pub fn slides(&self) -> &Arc<Slides> {
        &self.slides
    }

    /// Notes the window deck `id` is open in (its editor, not a window
    /// the audience sees).
    pub fn opened(&mut self, id: &str, view: WeakEntity<DeckView>, window: AnyWindowHandle) {
        self.views.retain(|_, (view, _)| view.upgrade().is_some());
        self.views.insert(id.to_string(), (view, window));
        // Its window draws it from here on.
        self.hidden.retain(|(hidden, ..)| hidden != id);
    }

    /// Where deck `id` is drawn for a picture, a PDF or a look at what
    /// does not fit: its own window when that is open, else out of sight
    /// in another window of the app (made on first use, and kept for the
    /// few decks drawn last).
    pub fn drawer(
        this: &Entity<Self>,
        id: &str,
        cx: &mut App,
    ) -> Task<Result<(Entity<DeckView>, AnyWindowHandle)>> {
        /// Decks kept drawn out of sight: each is a web view.
        const KEPT: usize = 2;
        if let Some(open) = this.read(cx).view(id) {
            return Task::ready(Ok(open));
        }
        // One made before, while the window it is in is still there.
        let kept = this
            .read(cx)
            .hidden
            .iter()
            .find(|(hidden, ..)| hidden == id)
            .map(|(_, view, window)| (view.clone(), *window));
        if let Some((view, window)) = kept {
            if window.update(cx, |_, _, _| ()).is_ok() {
                return Task::ready(Ok((view, window)));
            }
            this.update(cx, |this, _| {
                this.hidden.retain(|(hidden, ..)| hidden != id)
            });
        }
        if !ph_webview::supported() {
            return Task::ready(Err(anyhow::anyhow!(
                "on this system the deck is drawn in your browser, where the app cannot take a picture of it"
            )));
        }
        let Some(artifacts) = Artifacts::global(cx) else {
            return Task::ready(Err(anyhow::anyhow!("this build has no artifacts")));
        };
        // A deck deleted since is drawn nowhere any more.
        let gone: Vec<String> = this
            .read(cx)
            .hidden
            .iter()
            .filter(|(hidden, ..)| artifacts.read(cx).find(hidden).is_none())
            .map(|(hidden, ..)| hidden.clone())
            .collect();
        if !gone.is_empty() {
            this.update(cx, |this, _| {
                this.hidden.retain(|(hidden, ..)| !gone.contains(hidden))
            });
        }
        // Any window that is not a deck's own: those close with their deck.
        let decks: Vec<AnyWindowHandle> = this
            .read(cx)
            .views
            .values()
            .map(|(_, window)| *window)
            .collect();
        let free = |window: &AnyWindowHandle| !decks.contains(window);
        let window = cx
            .active_window()
            .filter(free)
            .or_else(|| cx.windows().into_iter().find(free));
        let Some(window) = window else {
            return Task::ready(Err(anyhow::anyhow!(
                "no window is open to draw the deck in"
            )));
        };
        let address = artifacts.update(cx, |artifacts, cx| artifacts.address(id, cx));
        let (this, id) = (this.clone(), id.to_string());
        cx.spawn(async move |cx| {
            let address = address.await?;
            Ok(cx.update(|cx| {
                // Opened meanwhile: its window draws it.
                if let Some(open) = this.read(cx).view(&id) {
                    return open;
                }
                let view = cx.new(|cx| DeckView::hidden(&id, &address, cx));
                this.update(cx, |this, _| {
                    this.hidden.retain(|(hidden, ..)| *hidden != id);
                    this.hidden.push((id.clone(), view.clone(), window));
                    let extra = this.hidden.len().saturating_sub(KEPT);
                    this.hidden.drain(..extra);
                });
                (view, window)
            }))
        })
    }

    /// Takes deck `id`'s picture for the Artifacts list (its first slide,
    /// or the one the user chose), once it has been quiet for a moment.
    /// For a deck drawn out of sight: one with a window open has its
    /// window take it, and one drawn nowhere waits until it is.
    pub fn picture_soon(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.view(id).is_some() || !self.hidden.iter().any(|(hidden, ..)| hidden == id) {
            return;
        }
        let Some(artifacts) = Artifacts::global(cx) else {
            return;
        };
        let deck = id.to_string();
        let taking = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            // Which slide, and where it is drawn. The drawing itself is
            // asked for after this: it reads the decks, which cannot be
            // read while they are being changed.
            let found = this.update(cx, |this, _| {
                let (_, view, window) = this
                    .hidden
                    .iter()
                    .find(|(hidden, ..)| *hidden == deck)?
                    .clone();
                let read = this.slides.held(&deck)?.deck();
                let chosen = read.other.get("thumbnail").and_then(|file| file.as_str());
                let file = chosen
                    .filter(|file| read.slides.iter().any(|slide| slide.file == *file))
                    .or_else(|| read.slides.first().map(|slide| slide.file.as_str()))?
                    .to_string();
                Some((view, window, file))
            });
            let Ok(Some((view, window, file))) = found else {
                return;
            };
            let asked = window
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| view.picture(&file, window, cx))
                })
                .map(Some);
            let Ok(Some(picture)) = asked else {
                return;
            };
            let Ok(png) = picture.await else {
                return;
            };
            _ = artifacts
                .update(cx, |artifacts, cx| artifacts.set_thumbnail(&deck, png, cx))
                .await;
        });
        self.picturing.insert(id.to_string(), taking);
    }

    /// The decks made from workspace `project`, the one changed last first.
    pub fn of_workspace(project: i64, cx: &App) -> Vec<Artifact> {
        let Some(artifacts) = Artifacts::global(cx) else {
            return Vec::new();
        };
        let mut decks: Vec<Artifact> = artifacts
            .read(cx)
            .all()
            .iter()
            .filter(|artifact| {
                artifact.manifest.kind == KIND
                    && artifact
                        .manifest
                        .source
                        .as_ref()
                        .is_some_and(|source| source.project == project)
            })
            .cloned()
            .collect();
        decks.sort_by_key(|deck| std::cmp::Reverse(deck.manifest.updated));
        decks
    }

    /// Says something about deck `id` to whoever listens to the decks (the
    /// window shell: a deck was made from a workspace's chat).
    pub fn announce(&mut self, id: &str, what: Value, cx: &mut Context<Self>) {
        cx.emit(DeckEvent {
            id: id.to_string(),
            what,
        });
    }

    /// The open window of deck `id`.
    pub fn view(&self, id: &str) -> Option<(Entity<DeckView>, AnyWindowHandle)> {
        let (view, window) = self.views.get(id)?;
        Some((view.upgrade()?, *window))
    }

    /// Makes a deck named `title` in the look `look`: an artifact with a
    /// `deck.json` of no slides yet and the look's `deck.css`.
    pub fn create(
        &mut self,
        title: String,
        look: &'static str,
        cx: &mut Context<Self>,
    ) -> Task<Result<Artifact>> {
        self.create_for(title, look, None, cx)
    }

    /// [`Self::create`], for a deck made from a workspace's chat (`from`):
    /// it belongs to that workspace, whose chats work on it.
    pub fn create_for(
        &mut self,
        title: String,
        look: &'static str,
        from: Option<Source>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Artifact>> {
        let Some(artifacts) = Artifacts::global(cx) else {
            return Task::ready(Err(anyhow::anyhow!("this build has no artifacts")));
        };
        let made = artifacts.update(cx, |artifacts, cx| {
            artifacts.create(KIND, title.clone(), cx)
        });
        cx.spawn(async move |_, cx| {
            let artifact = made.await?;
            let site = artifact.site();
            cx.background_spawn(async move {
                for Change { path, text } in looks::new_deck(&title, look) {
                    let text = text.unwrap_or_default();
                    ph_artifacts::library::write_atomic(&site.join(path), text.as_bytes())?;
                }
                anyhow::Ok(())
            })
            .await
            .context("the new deck could not be written")?;
            let Some(from) = from else {
                return Ok(artifact);
            };
            artifacts
                .update(cx, |artifacts, cx| {
                    artifacts.set_source(artifact.id(), Some(from), cx)
                })
                .await?;
            // As it is listed now, with its workspace.
            artifacts
                .read_with(cx, |artifacts, _| artifacts.find(artifact.id()).cloned())
                .context("the new deck is not listed")
        })
    }

    /// Makes a deck from the PowerPoint file at `path`: its slides, in the
    /// file's own size, colours and fonts, under the file's name.
    pub fn create_from(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Task<Result<Artifact>> {
        let title = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().trim().to_string())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| "Untitled deck".into());
        let made = self.create(title, "clean", cx);
        let slides = self.slides.clone();
        cx.spawn(async move |_, cx| {
            let artifact = made.await?;
            let live = slides.live(artifact.id(), &artifact.folder);
            let imported = cx
                .background_spawn({
                    let (slides, id) = (slides.clone(), artifact.id().to_string());
                    async move {
                        crate::import::powerpoint(&path, &live, true)?;
                        if let Some(applied) = live.scan() {
                            slides.changed(&id, &applied, "outside");
                        }
                        anyhow::Ok(())
                    }
                })
                .await;
            if let Err(err) = imported {
                // Nothing was made of the file: the deck begun for it goes.
                slides.close(artifact.id());
                if let Some(artifacts) = cx.update(|cx| Artifacts::global(cx)) {
                    let _ = artifacts
                        .update(cx, |artifacts, cx| artifacts.delete(artifact.id(), cx))
                        .await;
                }
                return Err(err);
            }
            Ok(artifact)
        })
    }

    fn heard(&mut self, told: Told, cx: &mut Context<Self>) {
        if told.what.get("changed").is_some() {
            self.changed(&told.id, cx);
            self.picture_soon(&told.id, cx);
        }
        if let Some(error) = told.what.get("error") {
            // The page's own failure: said where a developer looks.
            tracing::warn!(
                "a deck's page: {error} {}",
                told.what["at"].as_str().unwrap_or_default()
            );
            return;
        }
        cx.emit(DeckEvent {
            id: told.id,
            what: told.what,
        });
    }

    /// The deck's files changed: its artifact takes the deck's name, and
    /// its place at the head of the list.
    fn changed(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(artifacts) = Artifacts::global(cx) else {
            return;
        };
        let Some(artifact) = artifacts.read(cx).find(id).cloned() else {
            return;
        };
        let title = self
            .slides
            .held(id)
            .and_then(|live| live.text(DECK))
            .and_then(|text| Deck::parse(&text).ok())
            .map(|deck| deck.title.trim().to_string())
            .filter(|title| !title.is_empty() && *title != artifact.manifest.title);
        if let Some(title) = title {
            // Its folder stays where it is: an agent may be at work in it.
            artifacts
                .update(cx, |artifacts, cx| artifacts.retitle(id, title, cx))
                .detach();
            self.touched.insert(id.to_string(), Instant::now());
            return;
        }
        let due = self
            .touched
            .get(id)
            .is_none_or(|last| last.elapsed() >= TOUCH);
        if due {
            self.touched.insert(id.to_string(), Instant::now());
            artifacts.update(cx, |artifacts, cx| artifacts.touch(id, cx));
        }
    }

    /// The artifact was given another name in the list: the deck says the
    /// same of itself.
    pub fn renamed(&self, artifact: &Artifact) {
        let live = self.slides.live(artifact.id(), &artifact.folder);
        let mut deck = live.deck();
        if deck.title == artifact.manifest.title {
            return;
        }
        deck.title = artifact.manifest.title.clone();
        let change = Change {
            path: DECK.into(),
            text: Some(deck.to_text()),
        };
        match live.apply(&[change], &[], "") {
            Ok(applied) => self.slides.changed(artifact.id(), &applied, "page"),
            Err(err) => tracing::warn!("a deck could not take its new name: {err:?}"),
        }
    }
}

/// The app's colours and sizes as CSS variables: what makes a deck's page
/// look like the window it is in.
fn chrome(cx: &App) -> String {
    ph_artifacts::kit::chrome(cx)
}
