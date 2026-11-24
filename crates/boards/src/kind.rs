//! What a board's address answers: the app's own page for it (to change it
//! for the owner, to look at for a guest), and the calls that page makes
//! under `/_artifact/whiteboard/`. Pictures put on a board (`media/`) are
//! served as any artifact's files are.
//!
//! | Call | Who | Does |
//! |---|---|---|
//! | `GET board` | anyone | The board's file and its mark |
//! | `POST save` | owner | Writes the board, if it is still as the page read it |
//! | `POST media?name=` | owner | Keeps a picture in `media/` |
//! | `POST picture?for=` | owner | The whole board as a PNG, drawn by the page: for the list (`thumbnail`) or a file (`export`) |
//! | `POST tell` | owner | What the page tells the app: the selection, a wish (share, its own window), a note for the agent |
//! | `GET app/…` | anyone | The page's own scripts and styles |
//!
//! "Owner" is whoever comes through this computer's own door, as for every
//! artifact: a page here, and any program here. What a page tells the app
//! is therefore taken for a wish, never for a fact: only the wishes a page
//! has are passed on ([`TOLD`]), each of a bounded size.
//!
//! The file is the truth. A page saves the whole board with the mark of
//! the board it changed; if the file reads differently by then (an agent
//! wrote it meanwhile) the save is refused with the board as it is, and
//! the page takes that. Whatever writes the file from outside is seen by a
//! watcher, and every open page reads the board again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use ph_artifacts::library::{SITE, write_atomic};
use ph_artifacts::{Body, Kind, Request, Response, Server, Visit};
use serde_json::{Value, json};

use crate::board::{self, BOARD, MAX_BYTES, MEDIA};

/// What the page's calls are under.
const CALLS: &str = "/_artifact/whiteboard/";
/// How long after the file changed it is read again: an agent writing it
/// is then done, and a burst of writes is read once.
const SETTLE: Duration = Duration::from_millis(140);
/// Boards watched at once; one more and the one not asked for longest is
/// let go (it is watched again when someone opens it).
const MAX_OPEN: usize = 24;
/// A picture, at most.
const MAX_MEDIA: usize = 40 * 1024 * 1024;
/// What a page tells the app, at most (a note for the agent is the longest
/// of it).
const MAX_TOLD: usize = 64 * 1024;
/// What a page may tell the app. Anything else it sends is dropped: what
/// the app says of a board itself (`changed`, `made`, `picture`) is not a
/// page's to say.
const TOLD: &[&str] = &["focus", "share", "open", "opened", "ask", "error", "at"];
/// How a PNG begins.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The pictures a page draws of its board: what they are wanted for.
const PICTURES: &[&str] = &["thumbnail", "export"];
/// What a board may show from `media/`.
const MEDIA_TYPES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg"];

const INDEX: &str = include_str!("page/index.html");
/// The page's files, by name.
const FILES: &[(&str, &str)] = &[
    ("board.css", include_str!("page/board.css")),
    ("main.js", include_str!("page/main.js")),
    ("model.js", include_str!("page/model.js")),
    ("draw.js", include_str!("page/draw.js")),
    ("shapes.js", include_str!("page/shapes.js")),
    ("tools.js", include_str!("page/tools.js")),
    ("bars.js", include_str!("page/bars.js")),
    ("extras.js", include_str!("page/extras.js")),
];

/// Something a board's page told the app, or that happened to its file.
#[derive(Debug, Clone)]
pub struct Told {
    /// The board's artifact.
    pub id: String,
    pub what: Value,
}

struct Open {
    site: PathBuf,
    /// The mark of the board as last read or written here: a change of the
    /// file that reads the same is the page's own save, not news.
    known: Arc<Mutex<String>>,
    used: u64,
    _watcher: Option<notify::RecommendedWatcher>,
}

/// The boards of the running app, as the server's threads and the app's
/// own both reach them.
pub struct Whiteboards {
    server: Server,
    open: Mutex<HashMap<String, Open>>,
    clock: std::sync::atomic::AtomicU64,
    /// The app's colours and sizes as CSS variables.
    chrome: RwLock<String>,
    /// What is selected on each board, as its page last said.
    selection: Mutex<HashMap<String, Value>>,
    /// What the user changed by hand on each board since an agent was last
    /// told: how many saves.
    by_hand: Mutex<HashMap<String, usize>>,
    told: smol::channel::Sender<Told>,
    /// One writer of a board's file at a time.
    writing: Mutex<()>,
    /// The picture (PNG) each board's page last drew of the whole board,
    /// by the board and what it is wanted for, until the app takes it (for
    /// the list, or to save as a file).
    pictures: Mutex<HashMap<(String, &'static str), Vec<u8>>>,
}

impl Whiteboards {
    /// The boards served by `server`; what their pages tell the app comes
    /// out of the receiver.
    pub fn new(server: Server) -> (Arc<Self>, smol::channel::Receiver<Told>) {
        let (told, heard) = smol::channel::unbounded();
        let boards = Arc::new(Self {
            server,
            open: Mutex::default(),
            clock: std::sync::atomic::AtomicU64::new(1),
            chrome: RwLock::default(),
            selection: Mutex::default(),
            by_hand: Mutex::default(),
            told,
            writing: Mutex::default(),
            pictures: Mutex::default(),
        });
        (boards, heard)
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

    /// What is selected on board `id`, as its page last said.
    pub fn selection(&self, id: &str) -> Option<Value> {
        self.selection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// How many times the user changed board `id` by hand since this was
    /// last asked (an agent is told, and reads the file again).
    pub fn take_by_hand(&self, id: &str) -> usize {
        self.by_hand
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .unwrap_or(0)
    }

    /// Asks the board's open pages to do something (`command`: a note at
    /// the foot, a clean view for a picture).
    pub fn command(&self, id: &str, command: &Value) {
        self.server.announce(id, "command", command);
    }

    /// The picture board `id`'s page last drew of it for `wanted`
    /// (`thumbnail`, `export`), once.
    pub fn take_picture(&self, id: &str, wanted: &str) -> Option<Vec<u8>> {
        let wanted = PICTURES.iter().find(|known| **known == wanted)?;
        self.pictures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(id.to_string(), *wanted))
    }

    /// Board `id` was renamed, and its folder took the new name: it is
    /// watched there, and its open pages read it again (a watcher of the
    /// old name hears nothing more).
    pub fn renamed(self: &Arc<Self>, id: &str, folder: &Path) {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        let known = self.known(id, folder);
        let mark = known.lock().unwrap_or_else(PoisonError::into_inner).clone();
        self.server
            .announce(id, "board", &json!({ "mark": mark, "by": "outside" }));
    }

    fn tell(&self, id: &str, what: Value) {
        let _ = self.told.try_send(Told {
            id: id.to_string(),
            what,
        });
    }

    /// Board `id` in `folder`, watched for what is written into it from
    /// outside: the mark it is known by.
    fn known(self: &Arc<Self>, id: &str, folder: &Path) -> Arc<Mutex<String>> {
        use std::sync::atomic::Ordering;
        let site = folder.join(SITE);
        let now = self.clock.fetch_add(1, Ordering::Relaxed);
        if let Some(held) = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(id)
            // Renamed since: its folder moved, and it is watched there.
            && held.site == site
        {
            held.used = now;
            return held.known.clone();
        }
        // Read and watched without the lock held: the app's own thread
        // takes it at every change of the look, and a disk may be slow.
        let known = Arc::new(Mutex::new(board::mark(&board::read(&site))));
        let watcher = self.watch(id, &site, &known);
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = open.get_mut(id)
            && held.site == site
        {
            // Another request got here first: its watcher is the one.
            held.used = now;
            return held.known.clone();
        }
        if open.len() >= MAX_OPEN
            && !open.contains_key(id)
            && let Some(idle) = open
                .iter()
                .min_by_key(|(_, held)| held.used)
                .map(|(id, _)| id.clone())
        {
            open.remove(&idle);
        }
        open.insert(
            id.to_string(),
            Open {
                site,
                known: known.clone(),
                used: now,
                _watcher: watcher,
            },
        );
        known
    }

    /// Watches the board's folder: whatever writes its file (an agent with
    /// its own tools), the open pages read the board again.
    fn watch(
        self: &Arc<Self>,
        id: &str,
        site: &Path,
        known: &Arc<Mutex<String>>,
    ) -> Option<notify::RecommendedWatcher> {
        use notify::Watcher as _;
        let (changed, heard) = std::sync::mpsc::channel::<()>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok_and(|event| !event.kind.is_access()) {
                    let _ = changed.send(());
                }
            })
            .inspect_err(|err| tracing::warn!("a board's folder cannot be watched: {err}"))
            .ok()?;
        let _ = std::fs::create_dir_all(site);
        watcher
            .watch(site, notify::RecursiveMode::NonRecursive)
            .inspect_err(|err| tracing::warn!("a board's folder cannot be watched: {err}"))
            .ok()?;
        // The thread holds the boards weakly: it ends with the watcher (its
        // channel closes).
        let (boards, known) = (Arc::downgrade(self), Arc::downgrade(known));
        let (id, site) = (id.to_string(), site.to_path_buf());
        std::thread::Builder::new()
            .name("board-watch".into())
            .spawn(move || {
                while heard.recv().is_ok() {
                    // Until it is quiet: one read for a burst of writes.
                    while heard.recv_timeout(SETTLE).is_ok() {}
                    let (Some(boards), Some(known)) = (boards.upgrade(), known.upgrade()) else {
                        return;
                    };
                    let mark = board::mark(&board::read(&site));
                    let mut known = known.lock().unwrap_or_else(PoisonError::into_inner);
                    if *known == mark {
                        continue;
                    }
                    *known = mark.clone();
                    drop(known);
                    boards
                        .server
                        .announce(&id, "board", &json!({ "mark": mark, "by": "outside" }));
                    boards.tell(&id, json!({ "changed": "outside" }));
                }
            })
            .inspect_err(|err| tracing::warn!("a board's folder cannot be watched: {err}"))
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
        // Beside the chat of the workspace the board was made from: the
        // board to look at, with the way to its own window.
        let preview = visit.owner && visit.request.query("preview").is_some();
        let config = json!({
            "id": visit.id,
            "owner": visit.owner,
            "app": ph_brand::NAME,
            "title": title,
            // Shown in the app's own window, not in a browser.
            "embedded": visit.request.query("app").is_some(),
            "preview": preview,
        });
        let escaped = title
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        // What the board's title is in goes in last: a title that reads as
        // one of the other marks is then not filled in as one.
        let html = asset("index.html")
            .replace("__CHROME__", &chrome)
            .replace("__CLIENT__", if visit.owner { "" } else { "?guest" })
            .replace("__TITLE__", &escaped)
            // In a script: nothing of a title opens or closes a tag there.
            .replace("__CONFIG__", &config.to_string().replace('<', "\\u003c"));
        Response::new(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Cache-Control", "no-store")
            // A board's pictures are its own: the file is written by
            // agents, and an address in it is not to be called.
            .header("Content-Security-Policy", "img-src 'self' data: blob:")
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
        let known = self.known(visit.id, visit.folder);
        let site = visit.folder.join(SITE);
        if call == "board" && reading {
            return match state(&site) {
                Ok(state) => Response::json(200, state.to_string().into_bytes()),
                Err(why) => Response::error(500, &why),
            };
        }
        // Everything else changes the board, or is the owner's own.
        if !visit.owner {
            return Response::error(403, "only the board's owner changes it");
        }
        if request.method != "POST" {
            return Response::error(404, "a board has no such call");
        }
        match call {
            "save" => self.save(visit, &site, &known),
            "media" => media(&site, request),
            // The whole board as a picture, drawn by the page (which knows
            // how a board looks): `for` says what it is wanted for.
            "picture" => {
                let wanted = request.query("for").unwrap_or_default();
                let Some(wanted) = PICTURES.iter().find(|known| **known == wanted) else {
                    return Response::error(400, "a picture is for the thumbnail or for export");
                };
                if request.body.len() > MAX_MEDIA {
                    return Response::error(413, "the picture is too large");
                }
                if !request.body.starts_with(PNG) {
                    return Response::error(415, "the picture of a board is a PNG");
                }
                self.pictures
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert((visit.id.to_string(), *wanted), request.body.clone());
                self.tell(visit.id, json!({ "picture": wanted }));
                Response::new(204)
            }
            "tell" => {
                if request.body.len() > MAX_TOLD {
                    return Response::error(413, "that is too much to tell");
                }
                let Ok(Value::Object(what)) = serde_json::from_slice::<Value>(&request.body) else {
                    return Response::error(400, "what is told is a JSON object");
                };
                if let Some(selection) = what.get("selection") {
                    let mut kept = self
                        .selection
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    if selection.is_null() {
                        kept.remove(visit.id);
                    } else {
                        kept.insert(visit.id.to_string(), selection.clone());
                    }
                    return Response::new(204);
                }
                let wish: serde_json::Map<String, Value> = what
                    .into_iter()
                    .filter(|(key, _)| TOLD.contains(&key.as_str()))
                    .collect();
                if !wish.is_empty() {
                    self.tell(visit.id, Value::Object(wish));
                }
                Response::new(204)
            }
            _ => Response::error(404, "a board has no such call"),
        }
    }

    /// Writes the board the page sent, if the file still reads as the page
    /// read it; else answers with the board as it is (409).
    fn save(&self, visit: &Visit<'_>, site: &Path, known: &Mutex<String>) -> Response {
        // Before it is read as JSON: the board as text within text, at
        // most twice as long, and a little around it.
        if visit.request.body.len() > 2 * MAX_BYTES + 1024 {
            return Response::error(413, "the board is too large to keep");
        }
        let Ok(asked) = serde_json::from_slice::<Value>(&visit.request.body) else {
            return Response::error(400, "a save is JSON");
        };
        let (Some(text), Some(base)) = (asked["text"].as_str(), asked["base"].as_str()) else {
            return Response::error(400, "a save has `text` and `base`");
        };
        if text.len() > MAX_BYTES {
            return Response::error(413, "the board is too large to keep");
        }
        if let Err(why) = board::check(text) {
            return Response::error(400, &why);
        }
        let _one = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let now = match board::load(site) {
            Ok(now) => now,
            // Not written over what cannot be read: it may be a board.
            Err(why) => return Response::error(500, &why),
        };
        let held = board::mark(&now);
        if held != base {
            let state = json!({ "mark": held, "text": now });
            return Response::json(409, state.to_string().into_bytes());
        }
        // The board as it is: nothing to write, and nothing the user did.
        if now == text {
            return Response::json(200, json!({ "mark": held }).to_string().into_bytes());
        }
        let mark = board::mark(text);
        // Known before it is written: the watcher then takes the write for
        // what it is, the page's own.
        *known.lock().unwrap_or_else(PoisonError::into_inner) = mark.clone();
        if let Err(err) = write_atomic(&site.join(BOARD), text.as_bytes()) {
            return Response::error(500, &format!("{err:#}"));
        }
        visit.announce("board", &json!({ "mark": mark, "by": "page" }), true);
        *self
            .by_hand
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(visit.id.to_string())
            .or_default() += 1;
        self.tell(visit.id, json!({ "changed": "page" }));
        Response::json(200, json!({ "mark": mark }).to_string().into_bytes())
    }
}

/// The boards as the artifacts' server asks them (the kind `whiteboard`).
pub struct Handler(pub Arc<Whiteboards>);

impl Kind for Handler {
    fn respond(&self, visit: &Visit<'_>) -> Option<Response> {
        let path = visit.request.path.as_str();
        if let Some(call) = path.strip_prefix(CALLS) {
            return Some(self.0.call(visit, call));
        }
        let reading = matches!(visit.request.method.as_str(), "GET" | "HEAD");
        if !reading {
            return None;
        }
        if let Some(name) = path.strip_prefix("/media/")
            && name.to_lowercase().ends_with(".svg")
        {
            return Some(drawing(&visit.folder.join(SITE).join(MEDIA), name));
        }
        matches!(path, "/" | "/index.html").then(|| self.0.page(visit))
    }
}

/// A drawing (SVG) from a board's `media/`. It is shown as a picture, but
/// opened by its own address it would be a page of the board's, script and
/// all: it is served as one that may run none.
fn drawing(media: &Path, name: &str) -> Response {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.contains("..");
    let Some(bytes) = plain
        .then(|| std::fs::read(media.join(name)).ok())
        .flatten()
    else {
        return Response::text(404, "The board has no such picture.");
    };
    Response::new(200)
        .header("Content-Type", "image/svg+xml")
        .header(
            "Content-Security-Policy",
            "sandbox; default-src 'none'; style-src 'unsafe-inline'",
        )
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-cache")
        .body(Body::Bytes(bytes))
}

/// The board as the page reads it: its text and the text's mark.
fn state(site: &Path) -> Result<Value, String> {
    let text = board::load(site)?;
    Ok(json!({ "mark": board::mark(&text), "text": text }))
}

/// A file of the page, by name: as built in, or, in a debug build, as it
/// is in the folder `PILOTHOUSE_BOARD_PAGE` names (a change shows on
/// reload).
fn asset(name: &str) -> String {
    if cfg!(debug_assertions)
        && let Some(folder) = std::env::var_os("PILOTHOUSE_BOARD_PAGE")
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
    if !FILES.iter().any(|(known, _)| *known == name) {
        return Response::text(404, "The page has no such file.");
    }
    let kind = if name.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else {
        "text/css; charset=utf-8"
    };
    Response::new(200)
        .header("Content-Type", kind)
        // Asked for again each time: an app that was updated serves another
        // page under the same name.
        .header("Cache-Control", "no-cache")
        .body(Body::Bytes(asset(name).into_bytes()))
}

/// Keeps what the page sent as a picture in `media/`, named after what it
/// was called and what it holds: the same picture added twice is kept once.
fn media(site: &Path, request: &Request) -> Response {
    let name = request.query("name").unwrap_or_default();
    let extension = Path::new(&name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !MEDIA_TYPES.contains(&extension.as_str()) {
        return Response::error(415, "a board takes pictures (png, jpg, gif, webp, svg)");
    }
    if request.body.is_empty() || request.body.len() > MAX_MEDIA {
        return Response::error(413, "the picture is empty or too large");
    }
    let stem: String = Path::new(&name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    let stem = if stem.is_empty() {
        "picture".into()
    } else {
        stem
    };
    // By what it holds, so one picture is one file.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in &request.body {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let file = format!("{stem}-{:08x}.{extension}", hash as u32);
    let folder = site.join(MEDIA);
    if let Err(err) = std::fs::create_dir_all(&folder)
        .map_err(anyhow::Error::from)
        .and_then(|()| write_atomic(&folder.join(&file), &request.body))
    {
        return Response::error(500, &format!("{err:#}"));
    }
    Response::json(
        200,
        json!({ "src": format!("{MEDIA}/{file}") })
            .to_string()
            .into_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pages_files_are_all_there() {
        for (name, text) in FILES {
            assert!(!text.is_empty(), "{name}");
            assert_eq!(app_file(name).status, 200, "{name}");
        }
        assert_eq!(app_file("nothing.js").status, 404);
        // Every module a script of the page imports is one the page has (or
        // the kit's).
        for (name, text) in FILES.iter().filter(|(name, _)| name.ends_with(".js")) {
            for line in text.lines().filter(|line| line.contains(" from \"")) {
                let from = line.rsplit(" from \"").next().unwrap_or_default();
                let module = from.trim_end_matches(['"', ';']);
                if module == "/_artifact/kit/ui.js" {
                    continue;
                }
                let file = module.trim_start_matches("./");
                assert!(
                    FILES.iter().any(|(known, _)| *known == file),
                    "{name} imports {module}"
                );
            }
        }
        assert!(INDEX.contains("/_artifact/kit/kit.css"));
    }

    /// One request to a board's own door: the status, the headers
    /// (lowercase) and the body.
    fn ask(port: u16, method: &str, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
        use std::io::{Read as _, Write as _};
        let mut stream =
            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        let split = answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("an answer with headers");
        let head = String::from_utf8_lossy(&answer[..split]).to_lowercase();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, head, answer[split + 4..].to_vec())
    }

    /// The calls a board's page makes, as the page makes them: the board
    /// read, saved on the state it was read in and refused on another,
    /// pictures kept and taken, and only a page's own wishes passed on.
    #[test]
    fn a_page_reads_saves_and_tells_through_its_calls() {
        let root = std::env::temp_dir().join(format!("ph-board-calls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let artifact =
            ph_artifacts::library::create(&root, "Plan <b>", crate::board::KIND).unwrap();
        let site = artifact.site();
        std::fs::create_dir_all(&site).unwrap();
        let first = board::empty("Plan");
        std::fs::write(site.join(BOARD), &first).unwrap();
        let server = Server::default();
        let (boards, heard) = Whiteboards::new(server.clone());
        server.add_kind(crate::board::KIND, Arc::new(Handler(boards.clone())));
        let port = server.serve(&artifact, &Default::default()).unwrap();
        let call = |method: &str, call: &str, body: &[u8]| {
            ask(port, method, &format!("{CALLS}{call}"), body)
        };
        let json = |body: &[u8]| serde_json::from_slice::<Value>(body).unwrap();
        let said = || {
            let mut all = Vec::new();
            while let Ok(told) = heard.try_recv() {
                all.push(told.what);
            }
            all
        };

        // The page: its title cannot open a tag, in the page or in the
        // script that carries it; pictures come from the board alone.
        let (status, head, page) = ask(port, "GET", "/", b"");
        let page = String::from_utf8(page).unwrap();
        assert_eq!(status, 200);
        assert!(
            head.contains("content-security-policy: img-src 'self'"),
            "{head}"
        );
        assert!(
            page.contains("Plan &lt;b&gt;") && !page.contains("Plan <b>"),
            "{page}"
        );
        assert!(page.contains("Plan \\u003cb>"), "{page}");

        // Read, then saved on what was read.
        let read = json(&call("GET", "board", b"").2);
        assert_eq!(read["text"], first);
        let mark = read["mark"].as_str().unwrap().to_string();
        let second =
            r#"{"version":1,"title":"Plan","elements":[{"id":"a","type":"sticky","text":"One"}]}"#;
        let save = |text: &str, base: &str| {
            call(
                "POST",
                "save",
                json!({ "text": text, "base": base }).to_string().as_bytes(),
            )
        };
        let (status, _, answer) = save(second, &mark);
        assert_eq!(status, 200);
        let mark2 = json(&answer)["mark"].as_str().unwrap().to_string();
        assert_ne!(mark, mark2);
        assert_eq!(std::fs::read_to_string(site.join(BOARD)).unwrap(), second);
        assert_eq!(boards.take_by_hand(artifact.id()), 1);
        assert_eq!(said(), [json!({ "changed": "page" })]);
        // The same again: nothing written, and nothing the user did.
        assert_eq!(save(second, &mark2).0, 200);
        assert_eq!(boards.take_by_hand(artifact.id()), 0);
        assert!(said().is_empty());
        // On a state the file has left: refused, with the file as it is.
        let (status, _, answer) = save(&first, &mark);
        assert_eq!(status, 409);
        assert_eq!(json(&answer)["text"], second);
        assert_eq!(std::fs::read_to_string(site.join(BOARD)).unwrap(), second);
        // What is no board is not written.
        assert_eq!(save("{}", &mark2).0, 400);
        assert_eq!(save("not json", &mark2).0, 400);

        // Told: the selection is kept; a wish is passed on; what only the
        // app says of a board is not a page's to say.
        let tell = |what: Value| call("POST", "tell", what.to_string().as_bytes()).0;
        assert_eq!(tell(json!({ "selection": { "ids": ["a"] } })), 204);
        assert_eq!(boards.selection(artifact.id()).unwrap()["ids"][0], "a");
        assert_eq!(tell(json!({ "selection": null })), 204);
        assert!(boards.selection(artifact.id()).is_none());
        assert_eq!(
            tell(json!({ "share": true, "made": { "project": 3 } })),
            204
        );
        assert_eq!(
            tell(json!({ "changed": "outside", "picture": "export" })),
            204
        );
        assert_eq!(said(), [json!({ "share": true })]);
        assert_eq!(call("POST", "tell", b"[1]").0, 400);
        assert_eq!(call("POST", "tell", &vec![b' '; MAX_TOLD + 1]).0, 413);

        // A picture of the board: a PNG, for one of two uses, taken once.
        let png = [PNG, b"rest"].concat();
        assert_eq!(call("POST", "picture?for=export", b"<svg/>").0, 415);
        assert_eq!(call("POST", "picture?for=else", &png).0, 400);
        assert_eq!(call("POST", "picture?for=thumbnail", &png).0, 204);
        assert_eq!(call("POST", "picture?for=export", &png).0, 204);
        assert_eq!(said().len(), 2);
        assert_eq!(
            boards.take_picture(artifact.id(), "export"),
            Some(png.clone())
        );
        assert_eq!(boards.take_picture(artifact.id(), "export"), None);
        assert_eq!(boards.take_picture(artifact.id(), "thumbnail"), Some(png));

        // Pictures put on the board: named by what they hold, of the kinds
        // a board shows; a drawing is served as a page that runs nothing.
        let (status, _, answer) = call("POST", "media?name=../../My%20logo.SVG", b"<svg/>");
        assert_eq!(status, 200);
        let src = json(&answer)["src"].as_str().unwrap().to_string();
        assert!(
            src.starts_with("media/Mylogo-") && src.ends_with(".svg"),
            "{src}"
        );
        assert!(site.join(&src).is_file());
        assert_eq!(
            json(&call("POST", "media?name=logo.svg", b"<svg/>").2)["src"]
                .as_str()
                .unwrap()
                .len(),
            src.len() - 2
        );
        assert_eq!(call("POST", "media?name=run.html", b"<p>").0, 415);
        let (status, head, body) = ask(port, "GET", &format!("/{src}"), b"");
        assert_eq!((status, body.as_slice()), (200, b"<svg/>".as_slice()));
        assert!(head.contains("content-security-policy: sandbox"), "{head}");

        // A file that cannot be a board's is not read as an empty one, and
        // is not written over.
        std::fs::write(site.join(BOARD), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(call("GET", "board", b"").0, 500);
        assert_eq!(save(&first, &board::mark("")).0, 500);

        server.stop(artifact.id());
        let _ = std::fs::remove_dir_all(&root);
    }
}
