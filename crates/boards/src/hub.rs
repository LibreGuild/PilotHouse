//! The running app's boards: the kind added to the artifacts' server, a new
//! board made, and what a board's page tells the app passed on to whoever
//! shows that board (its window).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui_kit::component::Theme;
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use ph_artifacts::library::Source;
use ph_artifacts::{Artifact, Artifacts};
use serde_json::Value;

use crate::board::{self, BOARD, KIND};
use crate::kind::{Handler, Told, Whiteboards};

/// A board that changes is noted in the list this often at most (its place
/// there is by when it last changed, not by every note moved).
const TOUCH: Duration = Duration::from_secs(20);

/// Something a board's page told the app: `what` is as the page sent it
/// (`{"share": true}`, `{"ask": {…}}`, `{"focus": true}`, `{"open": true}`),
/// `{"changed": "page" | "outside"}` when its file changed, or
/// `{"made": {"project": …}}` when a workspace's agent made the board.
#[derive(Debug, Clone)]
pub struct BoardEvent {
    /// The board's artifact.
    pub id: String,
    pub what: Value,
}

pub struct Boards {
    whiteboards: Arc<Whiteboards>,
    touched: HashMap<String, Instant>,
    _heard: Task<()>,
    _theme: Subscription,
}

impl EventEmitter<BoardEvent> for Boards {}

struct Hub(Entity<Boards>);

impl Global for Hub {}

/// Starts the boards, where the artifacts were started. Call once, after
/// `ph_artifacts::init`.
pub fn init(cx: &mut App) {
    let Some(artifacts) = Artifacts::global(cx) else {
        return;
    };
    let server = artifacts.read(cx).server().clone();
    let (whiteboards, heard) = Whiteboards::new(server.clone());
    whiteboards.set_chrome(ph_artifacts::kit::chrome(cx));
    server.add_kind(KIND, Arc::new(Handler(whiteboards.clone())));
    let boards = cx.new(|cx| Boards {
        whiteboards,
        touched: HashMap::new(),
        _heard: cx.spawn(async move |this, cx| {
            while let Ok(told) = heard.recv().await {
                if this
                    .update(cx, |this: &mut Boards, cx| this.heard(told, cx))
                    .is_err()
                {
                    break;
                }
            }
        }),
        // The app's look changed: the boards' pages take it on.
        _theme: cx.observe_global::<Theme>(|this: &mut Boards, cx| {
            this.whiteboards.set_chrome(ph_artifacts::kit::chrome(cx));
        }),
    });
    cx.set_global(Hub(boards));
}

impl Boards {
    /// The app's boards, where the build has them and they were started.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Hub>().map(|hub| hub.0.clone())
    }

    /// What is selected on board `id`, as its page last said.
    pub fn selection(&self, id: &str) -> Option<Value> {
        self.whiteboards.selection(id)
    }

    /// How many times the user changed board `id` by hand since this was
    /// last asked.
    pub fn take_by_hand(&self, id: &str) -> usize {
        self.whiteboards.take_by_hand(id)
    }

    /// Board `artifact` was renamed, and its folder took the new name.
    pub fn renamed(&self, artifact: &Artifact) {
        self.whiteboards.renamed(artifact.id(), &artifact.folder);
    }

    /// Asks board `id`'s open pages to do something.
    pub fn command(&self, id: &str, command: &Value) {
        self.whiteboards.command(id, command);
    }

    /// The boards made from workspace `project`, the one changed last first.
    pub fn of_workspace(project: i64, cx: &App) -> Vec<Artifact> {
        let Some(artifacts) = Artifacts::global(cx) else {
            return Vec::new();
        };
        let mut boards: Vec<Artifact> = artifacts
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
        boards.sort_by_key(|board| std::cmp::Reverse(board.manifest.updated));
        boards
    }

    /// Says something about board `id` to whoever listens to the boards
    /// (the window shell: a board was made from a workspace's chat).
    pub fn announce(&mut self, id: &str, what: Value, cx: &mut Context<Self>) {
        cx.emit(BoardEvent {
            id: id.to_string(),
            what,
        });
    }

    /// Makes a board named `title` and answers with its artifact.
    pub fn create(&mut self, title: String, cx: &mut Context<Self>) -> Task<Result<Artifact>> {
        self.create_for(title, None, cx)
    }

    /// [`Self::create`], for a board made from a workspace's chat (`from`):
    /// it belongs to that workspace, whose chats work on it.
    pub fn create_for(
        &mut self,
        title: String,
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
            let file = artifact.site().join(BOARD);
            cx.background_spawn(async move {
                ph_artifacts::library::write_atomic(&file, board::empty(&title).as_bytes())
            })
            .await
            .context("the new board could not be written")?;
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
                .context("the new board is not listed")
        })
    }

    /// What a board's page told the app, or what happened to its file:
    /// passed on, and a change noted in the list.
    fn heard(&mut self, told: Told, cx: &mut Context<Self>) {
        if told.what.get("changed").is_some() {
            let due = self
                .touched
                .get(&told.id)
                .is_none_or(|at| at.elapsed() >= TOUCH);
            if due && let Some(artifacts) = Artifacts::global(cx) {
                self.touched.insert(told.id.clone(), Instant::now());
                artifacts.update(cx, |artifacts, cx| artifacts.touch(&told.id, cx));
            }
        }
        if let Some(wanted) = told.what.get("picture") {
            // The page drew the whole board: for the list, or as a file.
            let wanted = wanted.as_str().unwrap_or_default();
            if let Some(png) = self.whiteboards.take_picture(&told.id, wanted) {
                self.picture(&told.id, wanted == "export", png, cx);
            }
            return;
        }
        cx.emit(BoardEvent {
            id: told.id,
            what: told.what,
        });
    }

    /// A picture the page drew of board `id`: the board's picture in the
    /// list, or (`export`) a file in the board's `exports` folder, shown
    /// there once written.
    fn picture(&mut self, id: &str, export: bool, png: Vec<u8>, cx: &mut Context<Self>) {
        let Some(artifacts) = Artifacts::global(cx) else {
            return;
        };
        if !export {
            artifacts
                .update(cx, |artifacts, cx| artifacts.set_thumbnail(id, png, cx))
                .detach();
            return;
        }
        let Some(artifact) = artifacts.read(cx).find(id).cloned() else {
            return;
        };
        let id = id.to_string();
        let whiteboards = self.whiteboards.clone();
        let written = cx.background_spawn(async move {
            let folder = artifact.folder.join("exports");
            std::fs::create_dir_all(&folder)?;
            let name = ph_artifacts::library::folder_name(&artifact.manifest.title);
            // Never over a file that is there: the next free number, taken
            // by making the file (two exports at once get two names).
            use std::io::Write as _;
            let mut number = 1;
            let (path, mut file) = loop {
                let path = match number {
                    1 => folder.join(format!("{name}.png")),
                    n => folder.join(format!("{name} {n}.png")),
                };
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(file) => break (path, file),
                    Err(err)
                        if err.kind() == std::io::ErrorKind::AlreadyExists && number < 10_000 =>
                    {
                        number += 1;
                    }
                    Err(err) => return Err(err).context("no free name for the picture"),
                }
            };
            file.write_all(&png)?;
            anyhow::Ok(path)
        });
        cx.spawn(async move |_, cx| {
            let written = written.await;
            cx.update(|cx| match written {
                Ok(path) => {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let said = format!("Saved {name} in the board's exports folder");
                    whiteboards.command(&id, &serde_json::json!({ "toast": said }));
                    ph_ui::platform::reveal(&path, cx);
                }
                Err(err) => whiteboards.command(
                    &id,
                    &serde_json::json!({ "toast": format!("The picture could not be saved: {err:#}"), "error": true }),
                ),
            });
        })
        .detach();
    }
}
