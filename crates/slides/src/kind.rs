//! What a deck's address answers: the app's own page for it (the editor for
//! the owner, the viewer for a guest), and the calls that page makes under
//! `/_artifact/slides/`. The deck's files themselves (`deck.json`, the
//! slides, its media) are served as any artifact's are.
//!
//! | Call | Who | Does |
//! |---|---|---|
//! | `GET deck` | anyone | The deck's files and its version |
//! | `POST change` | owner | Writes files as one step |
//! | `POST undo`, `POST redo` | owner | Takes a step back, or forward again |
//! | `POST media?name=` | owner | Keeps a picture or video in `media/` |
//! | `GET`, `POST comments` | owner | The deck's comments |
//! | `POST tell` | owner | What the page tells the app: the selection, what does not fit, a wish (export, share) |
//! | `GET app/…` | anyone | The page's own scripts, styles and fonts |

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use ph_artifacts::library::SITE;
use ph_artifacts::{Body, Kind, Request, Response, Server, Visit};
use serde_json::{Value, json};

use crate::comments;
use crate::deck::{self, MEDIA};
use crate::live::{Applied, Change, Live, Refused};
use crate::looks::{LAYOUTS, LOOKS};

/// What the page's files are served under.
const CALLS: &str = "/_artifact/slides/";
/// Decks held open at once. One more than this and the one not asked for
/// longest is let go (it is read again when someone opens it).
const MAX_LIVE: usize = 24;
/// How long after a file changed the deck is read again: an agent writing a
/// file is then done with it, and a burst of writes is read once.
const SETTLE: Duration = Duration::from_millis(140);
/// A picture or a video, at most.
const MAX_MEDIA: usize = 200 * 1024 * 1024;

/// What a slide may show from `media/`.
const MEDIA_TYPES: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "mp4", "webm", "mov", "m4v", "mp3", "wav",
    "m4a", "ttf", "otf", "woff", "woff2",
];

const INDEX: &str = include_str!("page/index.html");
/// The page's files, by name.
const FILES: &[(&str, &str)] = &[
    ("app.css", include_str!("page/app.css")),
    ("slide.css", include_str!("page/slide.css")),
    ("core.js", include_str!("page/core.js")),
    ("store.js", include_str!("page/store.js")),
    ("editor.js", include_str!("page/editor.js")),
    ("edit.js", include_str!("page/edit.js")),
    ("panel.js", include_str!("page/panel.js")),
    ("comments.js", include_str!("page/comments.js")),
    ("present.js", include_str!("page/present.js")),
    ("check.js", include_str!("page/check.js")),
    ("measure.js", include_str!("page/measure.js")),
    ("viewer.js", include_str!("page/viewer.js")),
    ("render.js", include_str!("page/render.js")),
];

/// Something the deck's page told the app.
#[derive(Debug, Clone)]
pub struct Told {
    /// The deck's artifact.
    pub id: String,
    pub what: Value,
}

struct Open {
    live: Arc<Live>,
    used: u64,
    /// Watches the deck's folder for an agent's writes; gone with the deck.
    _watcher: Option<notify::RecommendedWatcher>,
}

/// The decks of the running app, as the server's threads and the app's own
/// both reach them.
pub struct Slides {
    server: Server,
    open: Mutex<HashMap<String, Open>>,
    clock: std::sync::atomic::AtomicU64,
    /// The app's colours and sizes as CSS variables, for the page to look
    /// like the window it is in.
    chrome: RwLock<String>,
    told: smol::channel::Sender<Told>,
    /// One writer of a deck's comments at a time.
    commenting: Mutex<()>,
}

impl Slides {
    /// The decks served by `server`; what their pages tell the app comes
    /// out of the receiver.
    pub fn new(server: Server) -> (Arc<Self>, smol::channel::Receiver<Told>) {
        let (told, heard) = smol::channel::unbounded();
        let slides = Arc::new(Self {
            server,
            open: Mutex::default(),
            clock: std::sync::atomic::AtomicU64::new(1),
            chrome: RwLock::default(),
            told,
            commenting: Mutex::default(),
        });
        (slides, heard)
    }

    /// The app's look changed: open pages take it on.
    pub fn set_chrome(&self, css: String) {
        let mut chrome = self.chrome.write().unwrap_or_else(PoisonError::into_inner);
        if *chrome == css {
            return;
        }
        *chrome = css.clone();
        drop(chrome);
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        for id in open.keys() {
            self.server.announce(id, "chrome", &json!({ "css": css }));
        }
    }

    /// The deck of artifact `id`, whose folder is `folder`: held open, and
    /// watched for what an agent writes into it.
    pub fn live(self: &Arc<Self>, id: &str, folder: &Path) -> Arc<Live> {
        use std::sync::atomic::Ordering;
        let site = folder.join(SITE);
        let now = self.clock.fetch_add(1, Ordering::Relaxed);
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = open.get_mut(id) {
            // Renamed since: its folder moved, and it is read where it is now.
            if held.live.site() == site {
                held.used = now;
                return held.live.clone();
            }
        }
        if open.len() >= MAX_LIVE
            && let Some(idle) = open
                .iter()
                .min_by_key(|(_, held)| held.used)
                .map(|(id, _)| id.clone())
        {
            open.remove(&idle);
        }
        let live = Arc::new(Live::open(id, &site));
        let watcher = self.watch(&live);
        open.insert(
            id.to_string(),
            Open {
                live: live.clone(),
                used: now,
                _watcher: watcher,
            },
        );
        live
    }

    /// The deck of artifact `id`, if it is held open.
    pub fn held(&self, id: &str) -> Option<Arc<Live>> {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(|held| held.live.clone())
    }

    /// Lets go of a deck (it was deleted).
    pub fn close(&self, id: &str) {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }

    /// Tells the deck's open pages which files read differently now.
    pub fn changed(&self, id: &str, applied: &Applied, by: &str) {
        if applied.files.is_empty() {
            return;
        }
        self.server.announce(
            id,
            "deck",
            &json!({ "version": applied.version, "files": applied.files, "by": by }),
        );
        self.tell(id, json!({ "changed": by }));
    }

    /// Asks the deck's open pages to do something (`command`: go to a
    /// slide, measure for an export).
    pub fn command(&self, id: &str, command: &Value) {
        self.server.announce(id, "command", command);
    }

    fn tell(&self, id: &str, what: Value) {
        let _ = self.told.try_send(Told {
            id: id.to_string(),
            what,
        });
    }

    /// Watches the deck's files: whatever writes them (an agent with its
    /// own tools), the deck is read again and its pages follow.
    fn watch(self: &Arc<Self>, live: &Arc<Live>) -> Option<notify::RecommendedWatcher> {
        use notify::Watcher as _;
        let (changed, heard) = std::sync::mpsc::channel::<()>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok_and(|event| !event.kind.is_access()) {
                    let _ = changed.send(());
                }
            })
            .inspect_err(|err| tracing::warn!("a deck's folder cannot be watched: {err}"))
            .ok()?;
        watcher
            .watch(live.site(), notify::RecursiveMode::Recursive)
            .inspect_err(|err| tracing::warn!("a deck's folder cannot be watched: {err}"))
            .ok()?;
        // The thread holds the deck weakly: it ends with the watcher (its
        // channel closes), and never keeps a deck that was let go.
        let (slides, deck) = (Arc::downgrade(self), Arc::downgrade(live));
        let spawned = std::thread::Builder::new()
            .name("deck-watch".into())
            .spawn(move || {
                while heard.recv().is_ok() {
                    // Until it is quiet: one read for a burst of writes.
                    while heard.recv_timeout(SETTLE).is_ok() {}
                    let (Some(slides), Some(live)) = (slides.upgrade(), deck.upgrade()) else {
                        return;
                    };
                    if let Some(applied) = live.scan() {
                        slides.changed(&live.id, &applied, "outside");
                    }
                }
            });
        spawned
            .inspect_err(|err| tracing::warn!("a deck's folder cannot be watched: {err}"))
            .ok()?;
        Some(watcher)
    }

    fn page(&self, visit: &Visit<'_>) -> Response {
        let chrome = self
            .chrome
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let title = ph_artifacts::library::read(visit.folder)
            .map(|artifact| artifact.manifest.title)
            .unwrap_or_default();
        let audience = visit.request.query("audience").is_some();
        // Beside the chat of the workspace the deck was made from: the
        // slides alone, with the way to the deck's own window.
        let preview = visit.owner && visit.request.query("preview").is_some();
        let config = json!({
            "id": visit.id,
            "owner": visit.owner,
            "app": ph_brand::NAME,
            "title": title,
            // Shown in the app's own window (which goes full screen itself
            // to present), not in a browser.
            "embedded": visit.request.query("app").is_some(),
            "preview": preview,
        });
        let escaped = title
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let html = asset("index.html")
            .replace("__TITLE__", &escaped)
            .replace("__CHROME__", &chrome)
            // In a script: a title cannot close it.
            .replace("__CONFIG__", &config.to_string().replace("</", "<\\/"))
            .replace("__CLIENT__", if visit.owner { "" } else { "?guest" })
            // The editor for the owner; for a guest, and for the window the
            // audience sees while the owner presents, the slides alone.
            .replace(
                "__SCRIPT__",
                match (visit.owner, audience) {
                    // For the app itself, to take pictures and PDFs of.
                    (true, _) if visit.request.query("render").is_some() => "render.js",
                    (true, false) if preview => "viewer.js",
                    (true, false) => "editor.js",
                    _ => "viewer.js",
                },
            );
        Response::new(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Cache-Control", "no-store")
            .body(Body::Bytes(html.into_bytes()))
    }

    fn call(self: &Arc<Self>, visit: &Visit<'_>, call: &str) -> Response {
        let request = visit.request;
        let reading = matches!(request.method.as_str(), "GET" | "HEAD");
        if let Some(name) = call.strip_prefix("app/") {
            return if reading {
                app_file(name)
            } else {
                Response::error(405, "the page's files are only read")
            };
        }
        let live = self.live(visit.id, visit.folder);
        if call == "deck" && reading {
            return Response::json(200, state(&live, visit.owner).to_string().into_bytes());
        }
        if call == "kit" && reading {
            return Response::json(200, kit().to_string().into_bytes());
        }
        // Everything else changes the deck, or is the owner's own.
        if !visit.owner {
            return Response::error(403, "only the deck's owner changes it");
        }
        let posted = request.method == "POST";
        match call {
            "change" if posted => self.change(visit, &live),
            "undo" | "redo" if posted => {
                let stepped = if call == "undo" {
                    live.undo()
                } else {
                    live.redo()
                };
                match stepped {
                    Ok(Some(applied)) => {
                        self.changed(visit.id, &applied, call);
                        Response::json(200, state(&live, true).to_string().into_bytes())
                    }
                    Ok(None) => Response::json(200, state(&live, true).to_string().into_bytes()),
                    Err(err) => Response::error(500, &format!("{err:#}")),
                }
            }
            "media" if posted => media(live.site(), request),
            "import" if posted => match start_from(visit.folder, &live, request) {
                Ok(made) => {
                    if let Some(applied) = live.scan() {
                        self.changed(visit.id, &applied, "outside");
                    }
                    Response::json(200, json!({ "text": made }).to_string().into_bytes())
                }
                Err(err) => Response::error(422, &format!("{err:#}")),
            },
            "comments" if reading => Response::json(
                200,
                json!(comments::read(visit.folder)).to_string().into_bytes(),
            ),
            "comments" if posted => self.comment(visit),
            "tell" if posted => {
                let Ok(what) = serde_json::from_slice::<Value>(&request.body) else {
                    return Response::error(400, "what is told is JSON");
                };
                if what.get("rendered").is_some() {
                    // The render page's answer to the app; with it, when
                    // asked for, what does not fit on the slides.
                    if let Some(report) = what.get("report") {
                        live.set_report(report.clone());
                    }
                    self.tell(visit.id, what);
                } else if let Some(selection) = what.get("selection") {
                    live.set_selection((!selection.is_null()).then(|| selection.clone()));
                } else if let Some(report) = what.get("report") {
                    live.set_report(report.clone());
                    self.tell(visit.id, json!({ "reported": live.version() }));
                } else {
                    self.tell(visit.id, what);
                }
                Response::new(204)
            }
            _ => Response::error(404, "a deck has no such call"),
        }
    }

    fn change(&self, visit: &Visit<'_>, live: &Live) -> Response {
        let Ok(asked) = serde_json::from_slice::<Value>(&visit.request.body) else {
            return Response::error(400, "a change is JSON");
        };
        let changes: Vec<Change> = asked["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|change| {
                Some(Change {
                    path: change["path"].as_str()?.to_string(),
                    text: change["text"].as_str().map(str::to_string),
                })
            })
            .collect();
        let base: Vec<(String, String)> = asked["base"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(path, mark)| Some((path.clone(), mark.as_str()?.to_string())))
            .collect();
        let label = asked["label"].as_str().unwrap_or_default();
        match live.apply(&changes, &base, label) {
            Ok(applied) => {
                if !applied.files.is_empty() {
                    visit.announce(
                        "deck",
                        &json!({ "version": applied.version, "files": applied.files, "by": "page" }),
                        true,
                    );
                    self.tell(visit.id, json!({ "changed": "page" }));
                }
                let marks: serde_json::Map<String, Value> = applied
                    .files
                    .iter()
                    .map(|path| {
                        let mark = live.text(path).map(|text| deck::mark(&text));
                        (path.clone(), json!(mark))
                    })
                    .collect();
                let snapshot = live.snapshot();
                Response::json(
                    200,
                    json!({
                        "version": applied.version,
                        "marks": marks,
                        "canUndo": snapshot.can_undo,
                        "canRedo": snapshot.can_redo,
                    })
                    .to_string()
                    .into_bytes(),
                )
            }
            Err(Refused::Stale(files)) => {
                Response::json(409, json!({ "stale": files }).to_string().into_bytes())
            }
            Err(Refused::Failed(err)) => Response::error(400, &format!("{err:#}")),
        }
    }

    fn comment(&self, visit: &Visit<'_>) -> Response {
        let Ok(op) = serde_json::from_slice::<Value>(&visit.request.body) else {
            return Response::error(400, "a comment is JSON");
        };
        let _one = self
            .commenting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match comments::apply(visit.folder, &op, "You") {
            Ok(changed) => {
                visit.announce("comments", &json!({}), true);
                // One meant for the agent goes to the deck's chat.
                let for_agent = matches!(op["op"].as_str(), Some("add" | "reply"))
                    && op["agent"].as_bool().unwrap_or(false);
                if for_agent && let Some(comment) = &changed.touched {
                    self.tell(visit.id, json!({ "comment": comment }));
                }
                Response::json(200, json!(changed.all).to_string().into_bytes())
            }
            Err(err) => Response::error(400, &format!("{err:#}")),
        }
    }

    /// Changes the deck's comments from the app's side (an agent answered),
    /// and has open pages read them again.
    pub fn comment_as(
        &self,
        id: &str,
        folder: &Path,
        op: &Value,
        author: &str,
    ) -> anyhow::Result<comments::Changed> {
        let _one = self
            .commenting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = comments::apply(folder, op, author)?;
        self.server.announce(id, "comments", &json!({}));
        Ok(changed)
    }
}

/// The decks as the artifacts' server asks them (the kind `slides`).
pub struct Handler(pub Arc<Slides>);

impl Kind for Handler {
    fn respond(&self, visit: &Visit<'_>) -> Option<Response> {
        let path = visit.request.path.as_str();
        if let Some(call) = path.strip_prefix(CALLS) {
            return Some(self.0.call(visit, call));
        }
        let reading = matches!(visit.request.method.as_str(), "GET" | "HEAD");
        (reading && matches!(path, "/" | "/index.html")).then(|| self.0.page(visit))
    }
}

/// The deck's files as the page reads them.
fn state(live: &Live, owner: bool) -> Value {
    let snapshot = live.snapshot();
    let files: serde_json::Map<String, Value> = snapshot
        .files
        .iter()
        .map(|(path, text)| {
            (
                path.clone(),
                json!({ "text": &**text, "mark": deck::mark(text) }),
            )
        })
        .collect();
    let mut state = json!({ "version": snapshot.version, "files": files });
    if owner {
        state["canUndo"] = json!(snapshot.can_undo);
        state["canRedo"] = json!(snapshot.can_redo);
    }
    state
}

/// What the page offers to start from: the looks, and the layouts of a new
/// slide.
fn kit() -> Value {
    json!({
        "looks": LOOKS.iter().map(|look| json!({
            "id": look.id, "name": look.name, "hint": look.hint, "css": look.css(),
        })).collect::<Vec<_>>(),
        "layouts": LAYOUTS.iter().map(|layout| json!({
            "id": layout.id, "name": layout.name, "html": layout.html,
        })).collect::<Vec<_>>(),
    })
}

/// A file of the page's. In a debug build, `PILOTHOUSE_SLIDES_PAGE` names a
/// folder to read them from instead, so the page is worked on without
/// building the app again.
fn asset(name: &str) -> String {
    if cfg!(debug_assertions)
        && let Some(folder) = std::env::var_os("PILOTHOUSE_SLIDES_PAGE")
        && let Ok(text) = std::fs::read_to_string(PathBuf::from(folder).join(name))
    {
        return text;
    }
    if name == "index.html" {
        return INDEX.to_string();
    }
    FILES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, text)| text.to_string())
        .unwrap_or_default()
}

fn app_file(name: &str) -> Response {
    let answer = |kind: &'static str, body: Vec<u8>| {
        Response::new(200)
            .header("Content-Type", kind)
            // Asked for again each time: an app that was updated serves
            // another page under the same name.
            .header("Cache-Control", "no-cache")
            .body(Body::Bytes(body))
    };
    if name == "fonts.css" {
        return answer(
            "text/css; charset=utf-8",
            fonts_css("/_artifact/slides/app/font/").into_bytes(),
        );
    }
    if let Some(file) = name.strip_prefix("font/") {
        return match ph_theme::fonts::WEB_FACES
            .iter()
            .find(|face| face.file == file)
        {
            Some(face) => Response::new(200)
                .header("Content-Type", "font/ttf")
                .header("Cache-Control", "max-age=86400")
                .body(Body::Bytes(face.bytes.to_vec())),
            None => Response::text(404, "The app has no such font."),
        };
    }
    let known = FILES.iter().any(|(known, _)| *known == name);
    if !known {
        return Response::text(404, "The page has no such file.");
    }
    let kind = if name.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else {
        "text/css; charset=utf-8"
    };
    answer(kind, asset(name).into_bytes())
}

/// The fonts the app brings, declared for a page; their files are under
/// `base`.
pub fn fonts_css(base: &str) -> String {
    ph_artifacts::kit::fonts_css(base)
}

/// Keeps what the page sent as a file in `media/`, named after what it was
/// called and what it holds: the same picture added twice is kept once.
fn media(site: &Path, request: &Request) -> Response {
    let name = request.query("name").unwrap_or_default();
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem, extension.to_ascii_lowercase()),
        None => (name.as_str(), String::new()),
    };
    if !MEDIA_TYPES.contains(&extension.as_str()) {
        return Response::error(
            415,
            "a slide shows pictures (PNG, JPEG, GIF, WebP, SVG), videos (MP4, WebM, MOV) and sound",
        );
    }
    if request.body.is_empty() || request.body.len() > MAX_MEDIA {
        return Response::error(413, "that file is empty, or larger than 200 MB");
    }
    let mut plain: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while plain.contains("--") {
        plain = plain.replace("--", "-");
    }
    let plain: String = plain.trim_matches('-').chars().take(40).collect();
    let plain = if plain.is_empty() {
        "file".to_string()
    } else {
        plain
    };
    let path = format!(
        "{MEDIA}/{plain}-{}.{extension}",
        &mark_bytes(&request.body)[..8]
    );
    let file = site.join(&path);
    let kept = file
        .metadata()
        .is_ok_and(|meta| meta.len() == request.body.len() as u64);
    if !kept {
        let written = std::fs::create_dir_all(site.join(MEDIA))
            .map_err(anyhow::Error::from)
            .and_then(|()| ph_artifacts::library::write_atomic(&file, &request.body));
        if let Err(err) = written {
            return Response::error(500, &format!("{err:#}"));
        }
    }
    Response::json(200, json!({ "path": path }).to_string().into_bytes())
}

/// Makes slides from the PowerPoint file the page sent: the file is kept
/// with the deck's other files to work from, and its slides follow the
/// ones the deck has (an empty deck takes the file's look too).
fn start_from(folder: &Path, live: &Live, request: &Request) -> anyhow::Result<String> {
    use anyhow::Context as _;
    let name = request.query("name").unwrap_or_default();
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
    let extension = extension.to_ascii_lowercase();
    anyhow::ensure!(
        crate::import::EXTENSIONS.contains(&extension.as_str()),
        "slides are made from a PowerPoint file (.pptx)"
    );
    anyhow::ensure!(
        !request.body.is_empty() && request.body.len() <= MAX_MEDIA,
        "that file is empty, or larger than 200 MB"
    );
    // The name becomes the slides' names: plain letters only.
    let plain: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(60)
        .collect();
    let plain = if plain.is_empty() {
        "deck".to_string()
    } else {
        plain
    };
    let references = folder.join("references");
    std::fs::create_dir_all(&references).context("the file could not be kept")?;
    let file = references.join(format!("{plain}.{extension}"));
    ph_artifacts::library::write_atomic(&file, &request.body)
        .context("the file could not be kept")?;
    crate::import::powerpoint(&file, live, live.deck().slides.is_empty())
}

fn mark_bytes(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pages_files_are_all_there() {
        for (name, text) in FILES {
            assert!(!text.trim().is_empty(), "{name}");
            assert_eq!(app_file(name).status, 200, "{name}");
        }
        assert_eq!(app_file("fonts.css").status, 200);
        assert_eq!(app_file("font/Inter-Regular.ttf").status, 200);
        assert_eq!(app_file("font/../../secret").status, 404);
        assert_eq!(app_file("nothing.js").status, 404);
        for placeholder in [
            "__TITLE__",
            "__CHROME__",
            "__CONFIG__",
            "__CLIENT__",
            "__SCRIPT__",
        ] {
            assert!(INDEX.contains(placeholder), "{placeholder}");
        }
        let fonts = fonts_css("/f/");
        assert_eq!(
            fonts.matches("@font-face").count(),
            ph_theme::fonts::WEB_FACES.len()
        );
        assert!(fonts.contains("url(\"/f/Inter-Bold.ttf\")"));
    }

    #[test]
    fn the_kit_offers_looks_and_layouts() {
        let kit = kit();
        assert_eq!(kit["looks"].as_array().unwrap().len(), LOOKS.len());
        assert!(
            kit["looks"][0]["css"]
                .as_str()
                .unwrap()
                .contains("--accent")
        );
        assert_eq!(kit["layouts"].as_array().unwrap().len(), LAYOUTS.len());
    }
}
