//! Serves artifacts to browsers: each at an address of its own on this
//! computer, `http://127.0.0.1:<port>/`, and, while its owner shares it, at
//! a second address on the network for guests who have its key.
//!
//! **A port each**, not one server with a path each: the artifact is then
//! the root of its address, so a built site's `/assets/app.js` loads as it
//! did where it was made; and a browser keeps each artifact's own storage
//! and permissions apart from the others'. The port is the artifact's for
//! good (`artifact.json`), so its address never changes.
//!
//! **Two doors.** The owner's is bound to the loopback interface and is
//! there for as long as the artifact is served. The guests' is bound to
//! every interface, on a port of its own, only while the artifact is
//! shared (`sharing.rs`). Which door a connection came through says who it
//! is: nothing a request claims about itself does. A guest gets the page,
//! the shared storage and the room; the owner's own storage never leaves
//! through the guests' door.
//!
//! Beside the files, an artifact's address answers a few calls of the app's
//! own under `/_artifact/`: the script every page gets (`client.js`), what
//! the page stores (`storage.rs`), and the stream and calls of its room
//! (`room.rs`): which publish is live, who is here, what they say.
//!
//! Plain threads over blocking sockets: a handful of requests from a few
//! people's browsers, and nothing to wake while nobody has a page open.

use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::http::{self, Body, ReadError, Request, Response};
use crate::library::{Artifact, DATA, PORTS, SITE};
use crate::room::{self, Next, Person, Room};
use crate::sharing::{self, INVITE, Share};
use crate::storage;

/// Artifacts served at once. Each holds a socket and a parked thread; the
/// one asked for longest ago makes room (it is served again when opened).
const MAX_SERVED: usize = 32;
/// Connections at once, over every artifact: a browser opens six to an
/// address, and each open page holds one more for its room.
const MAX_CONNECTIONS: usize = 512;
/// How long a connection may say nothing before it is closed.
const IDLE: Duration = Duration::from_secs(15);
/// How often an open page's stream looks whether its browser is still
/// there: so often, at the latest, the others learn that someone left.
const LOOK: Duration = Duration::from_secs(2);
/// How often a stream with nothing to say shows that the line holds.
const PING: Duration = Duration::from_secs(20);
/// A page larger than this is served as it is, without the app's script.
const MAX_PAGE: u64 = 32 * 1024 * 1024;
/// One call to the room: a batch of messages.
const MAX_SAID: usize = 512 * 1024;
/// What the owner's own page may send in one call: a picture or a video
/// for a deck. Held in memory while it is read, so not without end.
const MAX_UPLOAD: usize = 256 * 1024 * 1024;
/// Where what everyone shares is kept, inside the artifact's `data/`.
const SHARED: &str = "shared";

const CLIENT: &str = include_str!("client.js");
/// The script a page gets; a guest's says so, in a way no inline script is
/// needed for (a page may forbid those).
const SCRIPT_TAG: &[u8] = b"<script src=\"/_artifact/client.js\"></script>";
const GUEST_SCRIPT_TAG: &[u8] = b"<script src=\"/_artifact/client.js?guest\"></script>";

static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
/// Counts requests, to tell which artifact was asked for longest ago.
static CLOCK: AtomicU64 = AtomicU64::new(1);

/// Who a connection is, by the door it came through.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Visitor {
    /// On this computer.
    Owner,
    /// Through the shared link.
    Guest { take_part: bool },
}

impl Visitor {
    fn guest(self) -> bool {
        matches!(self, Visitor::Guest { .. })
    }

    /// May send to the room and write what is shared.
    fn may_change(self) -> bool {
        !matches!(self, Visitor::Guest { take_part: false })
    }
}

/// A kind of artifact whose page is the app's own (a deck's editor), not
/// one an agent published: the kind answers for the page and for the calls
/// that page makes, and the artifact's `site/` holds what the page shows.
/// Kinds are added once, by name ([`Server::add_kind`]); an artifact names
/// its kind in `artifact.json`.
pub trait Kind: Send + Sync {
    /// Answers what is the kind's own: its page at `/`, and its calls under
    /// `/_artifact/<kind>/`. `None` leaves the request to the artifact's
    /// files and to the calls every artifact has.
    fn respond(&self, visit: &Visit<'_>) -> Option<Response>;
}

/// One request to an artifact of a kind.
pub struct Visit<'a> {
    /// The artifact's id and folder.
    pub id: &'a str,
    pub folder: &'a Path,
    pub request: &'a Request,
    /// Came through the owner's door (this computer), not the shared link.
    pub owner: bool,
    /// May change what is shared (the owner, or a guest let to take part).
    pub may_change: bool,
    /// The open page the request says it comes from.
    pub page: Option<u64>,
    room: &'a Room,
}

impl Visit<'_> {
    /// Tells every open page of the artifact, or all but the asking one.
    pub fn announce(&self, event: &str, data: &Value, others_only: bool) {
        self.room
            .announce(event, data, self.page.filter(|_| others_only));
    }
}

/// The kinds, by the name artifacts call them.
type Kinds = Arc<RwLock<HashMap<String, Arc<dyn Kind>>>>;

/// The guests' door of an artifact that is shared.
struct Door {
    share: Share,
    port: u16,
    /// Set when sharing ends: the thread waiting at the door leaves.
    closed: Arc<AtomicBool>,
}

/// One artifact being served.
struct Site {
    id: String,
    port: u16,
    /// Its folder and first page, as they are now (a rename moves the
    /// folder; a publish may change the page).
    folder: RwLock<PathBuf>,
    entry: RwLock<String>,
    /// The kind it says it is (empty: a page of its own), and the kinds
    /// there are. Looked up at each request: a kind may be added after the
    /// first artifacts are served.
    kind: RwLock<String>,
    kinds: Kinds,
    /// Which publish is live.
    version: Mutex<u64>,
    /// The guests' door, while it is shared.
    door: RwLock<Option<Door>>,
    room: Room,
    stopped: Arc<AtomicBool>,
    used: AtomicU64,
    /// One writer at a time checks the storage limits and writes.
    writing: Mutex<()>,
    /// Tells the app that who is here, or what is shared, changed.
    news: smol::channel::Sender<()>,
}

impl Site {
    fn folder(&self) -> PathBuf {
        self.folder
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn entry(&self) -> String {
        self.entry
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn version(&self) -> u64 {
        *self.version.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What answers for its kind, when it has one that is known here.
    fn kind(&self) -> Option<Arc<dyn Kind>> {
        let name = self.kind.read().unwrap_or_else(PoisonError::into_inner);
        if name.is_empty() {
            return None;
        }
        self.kinds
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name.as_str())
            .cloned()
    }

    /// Takes on what `artifact` says now; open pages hear of a new publish.
    fn set(&self, artifact: &Artifact) {
        *self.folder.write().unwrap_or_else(PoisonError::into_inner) = artifact.folder.clone();
        *self.entry.write().unwrap_or_else(PoisonError::into_inner) =
            artifact.manifest.entry.clone();
        *self.kind.write().unwrap_or_else(PoisonError::into_inner) = artifact.manifest.kind.clone();
        let live = u64::from(artifact.manifest.version);
        let mut version = self.version.lock().unwrap_or_else(PoisonError::into_inner);
        if *version != live {
            *version = live;
            drop(version);
            self.room.announce("version", &json!(live), None);
        }
    }

    fn tell(&self) {
        // One note waiting is enough: the app reads everything anew.
        let _ = self.news.try_send(());
    }

    /// Ends sharing; says whether it was shared.
    fn close_door(&self) -> bool {
        let door = self
            .door
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(door) = door else {
            return false;
        };
        door.closed.store(true, Ordering::SeqCst);
        wake(door.port);
        self.room.close_guests();
        true
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.close_door();
        self.room.close();
        wake(self.port);
    }
}

/// A thread waiting for a connection on `port` wakes on one, and sees that
/// it is to leave.
fn wake(port: u16) {
    let _ = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(200),
    );
}

/// How an artifact is shared now.
#[derive(Debug, Clone, PartialEq)]
pub struct Shared {
    pub share: Share,
    /// The port of its address on the network.
    pub port: u16,
    /// People here through the link.
    pub guests: usize,
}

/// The artifacts being served, by id.
#[derive(Clone)]
pub struct Server {
    sites: Arc<Mutex<HashMap<String, Arc<Site>>>>,
    kinds: Kinds,
    news: smol::channel::Sender<()>,
    heard: smol::channel::Receiver<()>,
}

impl Default for Server {
    fn default() -> Self {
        let (news, heard) = smol::channel::bounded(1);
        Self {
            sites: Arc::default(),
            kinds: Arc::default(),
            news,
            heard,
        }
    }
}

impl Server {
    fn sites(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Site>>> {
        self.sites.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Serves `artifact` (or, already served, takes on what changed) and
    /// says on which port. `taken` are the other artifacts' ports, never
    /// used for this one. The port it asks for is used unless another
    /// program holds it now; then the next free one, which the caller notes.
    pub fn serve(&self, artifact: &Artifact, taken: &HashSet<u16>) -> Result<u16> {
        let mut sites = self.sites();
        if let Some(site) = sites.get(artifact.id()) {
            site.set(artifact);
            site.used
                .store(CLOCK.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            return Ok(site.port);
        }
        // Never one that is shared: its guests would be cut off.
        let shared = |site: &Site| site.door.read().is_ok_and(|door| door.is_some());
        if sites.len() >= MAX_SERVED
            && let Some(idle) = sites
                .iter()
                .filter(|(_, site)| !shared(site))
                .min_by_key(|(_, site)| site.used.load(Ordering::Relaxed))
                .map(|(id, _)| id.clone())
            && let Some(site) = sites.remove(&idle)
        {
            site.stop();
        }
        let (listener, port) = bind(Ipv4Addr::LOCALHOST, artifact.manifest.port, taken)?;
        let site = Arc::new(Site {
            id: artifact.id().to_string(),
            port,
            folder: RwLock::new(artifact.folder.clone()),
            entry: RwLock::new(artifact.manifest.entry.clone()),
            kind: RwLock::new(artifact.manifest.kind.clone()),
            kinds: self.kinds.clone(),
            version: Mutex::new(u64::from(artifact.manifest.version)),
            door: RwLock::new(None),
            room: Room::default(),
            stopped: Arc::new(AtomicBool::new(false)),
            used: AtomicU64::new(CLOCK.fetch_add(1, Ordering::Relaxed)),
            writing: Mutex::new(()),
            news: self.news.clone(),
        });
        let (at_door, closed) = (site.clone(), site.stopped.clone());
        std::thread::Builder::new()
            .name(format!("artifact-{port}"))
            .spawn(move || accept(listener, at_door, closed, false))
            .context("could not start the artifact's server")?;
        sites.insert(artifact.id().to_string(), site);
        Ok(port)
    }

    /// Has artifacts that say they are of kind `name` answered by `kind`.
    pub fn add_kind(&self, name: &str, kind: Arc<dyn Kind>) {
        self.kinds
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_string(), kind);
    }

    /// Tells every open page of artifact `id` (a kind's news: a file of
    /// its deck changed).
    pub fn announce(&self, id: &str, event: &str, data: &Value) {
        if let Some(site) = self.site(id) {
            site.room.announce(event, data, None);
        }
    }

    /// How many pages of artifact `id` are open now, and how many of them
    /// came through the shared link.
    pub fn here(&self, id: &str) -> (usize, usize) {
        self.site(id)
            .map(|site| site.room.here())
            .unwrap_or_default()
    }

    fn site(&self, id: &str) -> Option<Arc<Site>> {
        self.sites().get(id).cloned()
    }

    /// The port `id` is served on now, if it is.
    pub fn port(&self, id: &str) -> Option<u16> {
        self.site(id).map(|site| site.port)
    }

    /// The ids being served.
    pub fn served(&self) -> Vec<String> {
        self.sites().keys().cloned().collect()
    }

    /// Stops serving `id` (it was deleted).
    pub fn stop(&self, id: &str) {
        let site = self.sites().remove(id);
        if let Some(site) = site {
            site.stop();
            site.tell();
        }
    }

    /// Opens `artifact` (which is served) to the network as `share` says,
    /// and answers with the port of its address there: the one it asks for
    /// (`share_port`) unless that is taken (`taken`: every other port of
    /// the artifacts, and this one's own). Shared already, it takes on what
    /// changed: another key lets the guests go, who come back with the new
    /// link.
    pub fn share(&self, artifact: &Artifact, share: Share, taken: &HashSet<u16>) -> Result<u16> {
        let site = self
            .site(artifact.id())
            .context("the artifact is not served")?;
        let mut door = site.door.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(open) = door.as_mut() {
            let (rekeyed, changed) = (
                open.share.key != share.key,
                open.share.take_part != share.take_part,
            );
            open.share = share.clone();
            let port = open.port;
            drop(door);
            if rekeyed {
                site.room.close_guests();
            } else if changed {
                site.room.guests_may_change(share.take_part);
            }
            site.tell();
            return Ok(port);
        }
        let (listener, port) = bind(Ipv4Addr::UNSPECIFIED, artifact.manifest.share_port, taken)
            .context("could not open the artifact to the network")?;
        let closed = Arc::new(AtomicBool::new(false));
        let (at_door, leaving) = (site.clone(), closed.clone());
        std::thread::Builder::new()
            .name(format!("artifact-{port}-shared"))
            .spawn(move || accept(listener, at_door, leaving, true))
            .context("could not open the artifact to the network")?;
        *door = Some(Door {
            share,
            port,
            closed,
        });
        drop(door);
        site.tell();
        Ok(port)
    }

    /// Ends the sharing of `id`: its address on the network closes, and its
    /// guests are let go.
    pub fn unshare(&self, id: &str) {
        if let Some(site) = self.site(id)
            && site.close_door()
        {
            site.tell();
        }
    }

    /// How `id` is shared now, if it is.
    pub fn shared(&self, id: &str) -> Option<Shared> {
        let site = self.site(id)?;
        let door = site.door.read().unwrap_or_else(PoisonError::into_inner);
        let door = door.as_ref()?;
        Some(Shared {
            share: door.share.clone(),
            port: door.port,
            guests: site.room.here().1,
        })
    }

    /// Whether any artifact is open to the network.
    pub fn any_shared(&self) -> bool {
        self.sites()
            .values()
            .any(|site| site.door.read().is_ok_and(|door| door.is_some()))
    }

    /// Hears when who is here, or what is shared, changed (one note for
    /// any number of changes).
    pub fn news(&self) -> smol::channel::Receiver<()> {
        self.heard.clone()
    }
}

/// A listener at `address` on `wanted`, or on the first free port no
/// artifact has.
fn bind(address: Ipv4Addr, wanted: u16, taken: &HashSet<u16>) -> Result<(TcpListener, u16)> {
    let listen = |port: u16| {
        // A door on every interface opens, on some systems, although
        // another program has the port on this computer alone; whoever
        // came to it from here would then land there. So the port must be
        // free here as well.
        if address.is_unspecified() && TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err() {
            return None;
        }
        TcpListener::bind((address, port)).ok()
    };
    if wanted != 0
        && !taken.contains(&wanted)
        && let Some(listener) = listen(wanted)
    {
        return Ok((listener, wanted));
    }
    for port in PORTS {
        if port != wanted
            && !taken.contains(&port)
            && let Some(listener) = listen(port)
        {
            return Ok((listener, port));
        }
    }
    bail!(
        "no port between {} and {} is free to serve the artifact on",
        PORTS.start(),
        PORTS.end()
    )
}

/// Waits at a door for connections until it is `closed`. `guests`: the
/// door on the network, whoever comes through it is a guest.
fn accept(listener: TcpListener, site: Arc<Site>, closed: Arc<AtomicBool>, guests: bool) {
    for stream in listener.incoming() {
        if closed.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else {
            // Out of sockets, most likely: not in a tight loop.
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        if CONNECTIONS.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            continue;
        }
        let (site, closed) = (site.clone(), closed.clone());
        CONNECTIONS.fetch_add(1, Ordering::SeqCst);
        let spawned = std::thread::Builder::new()
            .name(format!("artifact-{}-request", site.port))
            .spawn(move || {
                connection(stream, &site, &closed, guests);
                CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Answers the requests of one connection until it closes, falls silent,
/// or its door is closed.
fn connection(stream: TcpStream, site: &Site, closed: &AtomicBool, guests: bool) {
    let _ = stream.set_read_timeout(Some(IDLE));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_nodelay(true);
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut out = stream;
    loop {
        // A guest sends stored values at most; the owner's own page may also
        // hand over a picture or a video (a kind's call).
        let most = if guests {
            storage::MAX_VALUE
        } else {
            MAX_UPLOAD
        };
        let request = match http::read_request(&mut reader, most) {
            Ok(request) => request,
            Err(ReadError::Closed) => return,
            Err(ReadError::Bad(why)) => {
                let _ = Response::text(400, why).write(&mut out, false, false);
                return;
            }
            Err(ReadError::TooLarge) => {
                let _ = Response::error(413, "that is more than can be taken at once")
                    .write(&mut out, false, false);
                return;
            }
        };
        if closed.load(Ordering::SeqCst) {
            return;
        }
        site.used
            .store(CLOCK.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
        let head_only = request.method == "HEAD";
        let keep = request.keeps_alive;
        let visitor = if guests {
            match admit(site, &request) {
                Ok(visitor) => visitor,
                // The key just came with the link: on to the plain address.
                Err(answer) if answer.status == 302 => {
                    if answer.write(&mut out, head_only, keep).is_err() || !keep {
                        return;
                    }
                    continue;
                }
                Err(answer) => {
                    let _ = answer.write(&mut out, head_only, false);
                    return;
                }
            }
        } else if host_is_ours(site.port, request.header("host")) {
            Visitor::Owner
        } else {
            let _ = Response::text(403, "This address is not the artifact's own.")
                .write(&mut out, false, false);
            return;
        };
        if request.path == "/_artifact/events" && request.method == "GET" {
            // The connection is the stream's from here on.
            return events(out, site, &request, visitor);
        }
        if respond(site, &request, visitor)
            .write(&mut out, head_only, keep)
            .is_err()
            || !keep
        {
            return;
        }
    }
}

/// Only the artifact's own address is answered at the owner's door. A page
/// elsewhere that had its own name point at this computer (DNS rebinding)
/// is refused, since it would otherwise read the artifact as its own.
fn host_is_ours(port: u16, host: Option<&str>) -> bool {
    host.is_some_and(|host| {
        host == format!("127.0.0.1:{port}")
            || host.eq_ignore_ascii_case(&format!("localhost:{port}"))
    })
}

/// Lets a guest in who has the key: kept in their cookie, or come with the
/// link, in which case the answer (`Err`, a redirect) leaves it in the
/// cookie and sends them on to the same address without it. Anyone else
/// gets a page saying what to ask for.
fn admit(site: &Site, request: &Request) -> Result<Visitor, Response> {
    let door = site.door.read().unwrap_or_else(PoisonError::into_inner);
    let Some(door) = door.as_ref() else {
        return Err(refusal("This page is not shared any more."));
    };
    let name = sharing::cookie_name(&site.id);
    let visitor = Visitor::Guest {
        take_part: door.share.take_part,
    };
    if sharing::cookie(request.header("cookie"), &name)
        .is_some_and(|kept| sharing::opens(&door.share, kept))
    {
        return Ok(visitor);
    }
    match request.query(INVITE) {
        Some(invite) if sharing::opens(&door.share, &invite) => Err(redirect(302, &request.path)
            .header(
                "Set-Cookie",
                // Not for scripts to read, and not sent along from other sites.
                format!(
                    "{name}={}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax",
                    door.share.key
                ),
            )
            .header("Cache-Control", "no-store")),
        _ => Err(refusal(
            "This page is shared by a link. Ask the person who shared it for the link.",
        )),
    }
}

/// What someone without the key sees.
fn refusal(why: &str) -> Response {
    let page = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Shared with {name}</title><body style=\"font: 16px system-ui, sans-serif; margin: 15vh auto; max-width: 28em; padding: 0 1.5em; color-scheme: light dark\"><p>{why}</p>",
        name = ph_brand::NAME
    );
    Response::new(403)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(Body::Bytes(page.into_bytes()))
}

fn respond(site: &Site, request: &Request, visitor: Visitor) -> Response {
    if let Some(kind) = site.kind() {
        // A kind's calls change what it keeps: like every call here, only
        // the artifact's own page may make them.
        if request.path.starts_with("/_artifact/")
            && request.path != "/_artifact/client.js"
            && !from_own_page(request)
        {
            return Response::error(403, "only the artifact's own page may ask this");
        }
        let folder = site.folder();
        let visit = Visit {
            id: &site.id,
            folder: &folder,
            request,
            owner: !visitor.guest(),
            may_change: visitor.may_change(),
            page: page_of(request),
            room: &site.room,
        };
        if let Some(answer) = kind.respond(&visit) {
            return answer;
        }
    }
    if let Some(call) = request.path.strip_prefix("/_artifact/") {
        return service(site, request, call, visitor);
    }
    match request.method.as_str() {
        "GET" | "HEAD" => file(site, request, visitor.guest()),
        _ => Response::text(405, "An artifact's files can only be read.")
            .header("Allow", "GET, HEAD"),
    }
}

/// The app's own calls at an artifact's address.
fn service(site: &Site, request: &Request, call: &str, visitor: Visitor) -> Response {
    let reading = matches!(request.method.as_str(), "GET" | "HEAD");
    if call == "client.js" && reading {
        return Response::new(200)
            .header("Content-Type", "text/javascript; charset=utf-8")
            .header("Cache-Control", "no-cache")
            .body(Body::Bytes(CLIENT.as_bytes().to_vec()));
    }
    // What the app's own pages are drawn with (a deck's, a board's).
    if reading
        && let Some(name) = call.strip_prefix("kit/")
        && let Some(answer) = crate::kit::respond(name)
    {
        return answer;
    }
    if !matches!(call, "storage" | "shared" | "room") {
        return Response::text(404, "The app has no such call.");
    }
    // What a page stores and says is the page's alone: a request another
    // site's script made is refused, whatever it asks.
    if !from_own_page(request) {
        return Response::error(403, "only the artifact's own page may ask this");
    }
    let look_only = || Response::error(403, "this artifact is shared to look at only");
    let data = site.folder().join(DATA);
    match call {
        // What the owner stored for themselves never leaves through the
        // guests' door (a guest's page keeps its own in its browser).
        "storage" if visitor.guest() => {
            Response::error(403, "a guest's own storage is kept in their browser")
        }
        "storage" => stored(site, &data, request).0,
        "shared" if !reading && !visitor.may_change() => look_only(),
        "shared" => {
            let (answer, changed) = stored(site, &data.join(SHARED), request);
            if let Some(key) = changed {
                // Every other open page reads it anew.
                site.room
                    .announce("shared", &json!({ "key": key }), page_of(request));
            }
            answer
        }
        _ if request.method != "POST" => {
            Response::error(405, "the room takes POST").header("Allow", "POST")
        }
        _ if !visitor.may_change() => look_only(),
        _ => said(site, request, visitor),
    }
}

/// The open page a call says it comes from (the number its stream was
/// given).
fn page_of(request: &Request) -> Option<u64> {
    request.header("x-artifact-page")?.parse().ok()
}

/// Whether the browser says the request comes from the artifact's own page
/// (or from the address bar). Browsers send both headers themselves; a
/// script cannot set them.
fn from_own_page(request: &Request) -> bool {
    let own = request.header("host").map(|host| format!("http://{host}"));
    let origin = request
        .header("origin")
        .is_none_or(|origin| Some(origin) == own.as_deref());
    let site = request
        .header("sec-fetch-site")
        .is_none_or(|site| matches!(site, "same-origin" | "none"));
    origin && site
}

/// Reads or writes a value in `dir`; with the answer, the key that changed.
fn stored(site: &Site, dir: &Path, request: &Request) -> (Response, Option<String>) {
    let key = request.query("key");
    let failed = |err: anyhow::Error| Response::error(400, &format!("{err:#}"));
    let unchanged = |answer: Response| (answer, None);
    match (request.method.as_str(), key) {
        ("GET" | "HEAD", None) => {
            let keys: Vec<Value> = storage::keys(dir)
                .into_iter()
                .map(|(key, bytes)| json!({ "key": key, "bytes": bytes }))
                .collect();
            unchanged(Response::json(
                200,
                json!({ "keys": keys }).to_string().into_bytes(),
            ))
        }
        ("GET" | "HEAD", Some(key)) => unchanged(match storage::get(dir, &key) {
            Ok(Some(value)) => Response::json(200, value),
            // Not an error: a page's first read finds nothing, and a 404
            // would stand in its console as a failure for an agent to chase.
            Ok(None) => Response::json(200, b"null".to_vec()),
            Err(err) => failed(err),
        }),
        ("PUT", Some(key)) => {
            let _one = site.writing.lock().unwrap_or_else(PoisonError::into_inner);
            match storage::set(dir, &key, &request.body) {
                Ok(()) => (Response::new(204), Some(key)),
                Err(err) => unchanged(failed(err)),
            }
        }
        ("DELETE", Some(key)) => {
            let _one = site.writing.lock().unwrap_or_else(PoisonError::into_inner);
            match storage::remove(dir, &key) {
                Ok(true) => (Response::new(204), Some(key)),
                Ok(false) => unchanged(Response::new(204)),
                Err(err) => unchanged(failed(err)),
            }
        }
        (_, None) => unchanged(Response::error(400, "say which key: ?key=…")),
        _ => unchanged(
            Response::error(405, "storage takes GET, PUT and DELETE")
                .header("Allow", "GET, PUT, DELETE"),
        ),
    }
}

/// A page speaks in the room: `{"page": 3, "name": "Maya", "messages":
/// [{"to": null, "data": …}]}`. `page` is the number its stream was given;
/// a page speaks only through the door its stream came through.
fn said(site: &Site, request: &Request, visitor: Visitor) -> Response {
    if request.body.len() > MAX_SAID {
        return Response::error(413, "too much said at once");
    }
    let Ok(call) = serde_json::from_slice::<Value>(&request.body) else {
        return Response::error(400, "the room takes JSON");
    };
    let page = call["page"].as_u64();
    let Some(page) = page.filter(|page| site.room.is_guest(*page) == Some(visitor.guest())) else {
        // Its stream broke; the page connects again by itself and is given
        // another number.
        return Response::error(409, "this page is not connected to the room right now");
    };
    if let Some(name) = call["name"].as_str() {
        site.room.rename(page, name);
        site.tell();
    }
    for message in call["messages"].as_array().into_iter().flatten() {
        let data = &message["data"];
        if data.to_string().len() > room::MAX_MESSAGE {
            return Response::error(413, "a message can be 64 KB at most");
        }
        site.room.say(page, message["to"].as_str(), data);
    }
    Response::new(204)
}

/// An open page's stream: which publish is live, who is here, and whatever
/// its room passes on, until the page goes away.
fn events(mut out: TcpStream, site: &Site, request: &Request, visitor: Visitor) {
    let person = request.query("who").and_then(|who| {
        let name = request.query("name").unwrap_or_default();
        Person::new(&who, &name, !visitor.guest())
    });
    let Some(person) = person else {
        let _ = Response::text(400, "say who you are: ?who=…").write(&mut out, false, false);
        return;
    };
    let joined = site
        .room
        .join(person, visitor.guest(), visitor.may_change());
    let Some((page, outbox)) = joined else {
        let _ = Response::text(503, "Too many pages are open on this artifact.")
            .write(&mut out, false, false);
        return;
    };
    site.tell();
    let opening = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nretry: 1500\n\nevent: version\ndata: {}\n\n",
        site.version()
    );
    let send =
        |text: &str, out: &mut TcpStream| out.write_all(text.as_bytes()).and_then(|()| out.flush());
    // From here nothing waits on this browser: a page that takes in less
    // than the others send is let go rather than hold a thread.
    let streaming = send(&opening, &mut out).is_ok() && out.set_nonblocking(true).is_ok();
    let mut quiet = Duration::ZERO;
    while streaming && !site.stopped.load(Ordering::SeqCst) {
        match outbox.next(LOOK) {
            Next::Send(text) => {
                if send(&text, &mut out).is_err() {
                    break;
                }
                quiet = Duration::ZERO;
            }
            Next::Quiet => {
                // A browser that closed the page has closed the line.
                match out.read(&mut [0; 64]) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
                quiet += LOOK;
                if quiet >= PING {
                    // A comment: it only shows that the line still holds.
                    if send(": still here\n\n", &mut out).is_err() {
                        break;
                    }
                    quiet = Duration::ZERO;
                }
            }
            Next::Closed => break,
        }
    }
    site.room.leave(page);
    site.tell();
}

/// The file a path asks for, from the artifact's site.
fn file(site: &Site, request: &Request, guest: bool) -> Response {
    let root = site.folder().join(SITE);
    let entry = site.entry();
    let mut wanted = PathBuf::new();
    for part in request.path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        // No way up, no hidden files, and nothing a file system reads as
        // more than a name.
        if part.starts_with('.') || part.contains(['\\', ':', '\0']) {
            return not_found();
        }
        wanted.push(part);
    }
    if wanted.as_os_str().is_empty() {
        // A first page inside a folder is shown at its own address, so
        // what it links to beside it is found.
        return if entry.contains('/') {
            redirect(302, &format!("/{entry}"))
        } else {
            send_file(&root, &root.join(&entry), request, guest)
        };
    }
    let path = root.join(&wanted);
    if path.is_dir() {
        if !request.path.ends_with('/') {
            return redirect(301, &format!("{}/", request.path));
        }
        let index = path.join("index.html");
        if index.is_file() {
            return send_file(&root, &index, request, guest);
        }
    } else if path.is_file() {
        return send_file(&root, &path, request, guest);
    }
    // A path that names no file is a route of the page's own (`/settings`)
    // when it has no extension: the first page answers and shows it.
    let route = wanted
        .file_name()
        .is_some_and(|name| !name.to_string_lossy().contains('.'));
    if route {
        send_file(&root, &root.join(&entry), request, guest)
    } else if request.path == "/favicon.ico" {
        // Browsers ask for one unasked. A page without one has none, which
        // is no failure to report in its console.
        Response::new(204)
    } else {
        not_found()
    }
}

fn not_found() -> Response {
    Response::text(404, "The artifact has no such file.")
}

fn redirect(status: u16, to: &str) -> Response {
    // Each part written as an address takes it; the slashes stay.
    let location: String = to.split('/').map(encode_part).collect::<Vec<_>>().join("/");
    Response::new(status).header("Location", location)
}

fn encode_part(part: &str) -> String {
    part.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Sends `path`, once it is known to be inside `root` with every link
/// followed (a link put into the site by hand must not lead out of it).
fn send_file(root: &Path, path: &Path, request: &Request, guest: bool) -> Response {
    let (Ok(real_root), Ok(real)) = (dunce::canonicalize(root), dunce::canonicalize(path)) else {
        return not_found();
    };
    if !real.starts_with(&real_root) {
        return not_found();
    }
    let Ok(file) = std::fs::File::open(&real) else {
        return not_found();
    };
    let Ok(meta) = file.metadata() else {
        return not_found();
    };
    if !meta.is_file() {
        return not_found();
    }
    let len = meta.len();
    let kind = http::content_type(&real);
    let page = kind.starts_with("text/html") && len <= MAX_PAGE;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    // A page's tag also says which of the app's scripts it carries.
    let carries = match (page, guest) {
        (false, _) => "",
        (true, false) => "-p",
        (true, true) => "-g",
    };
    let tag = format!("\"{len:x}-{modified:x}{carries}\"");
    let shared = |response: Response| {
        response
            .header("Content-Type", kind)
            .header("ETag", tag.clone())
            // Asked again each time (and answered "not changed"), so a new
            // publish shows at once.
            .header("Cache-Control", "no-cache")
            .header("X-Content-Type-Options", "nosniff")
    };
    if request.header("if-none-match") == Some(tag.as_str()) {
        return shared(Response::new(304));
    }
    if page {
        return match std::fs::read(&real) {
            Ok(html) => shared(Response::new(200)).body(Body::Bytes(with_client(&html, guest))),
            Err(_) => not_found(),
        };
    }
    let response = shared(Response::new(200)).header("Accept-Ranges", "bytes");
    match request
        .header("range")
        .and_then(|range| http::range(range, len))
    {
        Some((first, last)) => {
            let mut partial = response
                .header("Content-Range", format!("bytes {first}-{last}/{len}"))
                .body(Body::File {
                    file,
                    start: first,
                    len: last - first + 1,
                });
            partial.status = 206;
            partial
        }
        None => response.body(Body::File {
            file,
            start: 0,
            len,
        }),
    }
}

/// `html` with the app's script as the first thing in its head, so that
/// `window.artifact` is there before any script of the page runs.
fn with_client(html: &[u8], guest: bool) -> Vec<u8> {
    // Only the start is searched: a page's head is there or nowhere.
    let start = &html[..html.len().min(64 * 1024)];
    let at = after_tag(start, b"head")
        .or_else(|| after_tag(start, b"html"))
        // Never before the doctype, which would put the page in quirks mode.
        .or_else(|| after_tag(start, b"!doctype"))
        .unwrap_or(0);
    let tag = if guest { GUEST_SCRIPT_TAG } else { SCRIPT_TAG };
    let mut out = Vec::with_capacity(html.len() + tag.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(tag);
    out.extend_from_slice(&html[at..]);
    out
}

/// Where the opening tag `name` ends in `html`, whatever its case.
fn after_tag(html: &[u8], name: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(open) = html[from..].iter().position(|byte| *byte == b'<') {
        let start = from + open + 1;
        let end = start + name.len();
        let named = html
            .get(start..end)
            .is_some_and(|found| found.eq_ignore_ascii_case(name));
        // `<head>` and `<head lang>`, not `<header>`.
        let whole = html
            .get(end)
            .is_some_and(|next| *next == b'>' || next.is_ascii_whitespace() || *next == b'/');
        if named && whole {
            return html[end..]
                .iter()
                .position(|byte| *byte == b'>')
                .map(|close| end + close + 1);
        }
        from = start;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{self, Publish};

    fn scratch(name: &str) -> PathBuf {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ph-artifact-server-{name}-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    /// A published page with a script, a film and a second page in a folder.
    fn published() -> (PathBuf, Artifact) {
        let (work, root) = (scratch("work"), scratch("root"));
        std::fs::create_dir_all(work.join("assets")).unwrap();
        std::fs::create_dir_all(work.join("docs")).unwrap();
        std::fs::write(
            work.join("index.html"),
            "<!DOCTYPE html>\n<html>\n<header></header><HEAD lang=\"en\"><title>Planner</title></HEAD><body>one</body></html>",
        )
        .unwrap();
        std::fs::write(work.join("assets/app.js"), "let a = 1;").unwrap();
        std::fs::write(work.join("assets/film.mp4"), b"0123456789").unwrap();
        std::fs::write(work.join("docs/index.html"), "<p>docs</p>").unwrap();
        let mut artifact = library::publish(
            &root,
            &Publish {
                source: work.clone(),
                ..Publish::default()
            },
        )
        .unwrap()
        .artifact;
        // Ports of its own, for both doors: tests run side by side.
        artifact.manifest.port = unused_port();
        artifact.manifest.share_port = unused_port();
        (work, artifact)
    }

    fn unused_port() -> u16 {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        *PORTS.end() - 200 + NEXT.fetch_add(1, Ordering::Relaxed) as u16
    }

    /// One request on a connection of its own: the status line's code, the
    /// headers (lowercase) and the body.
    fn ask(port: u16, request: &str) -> (u16, String, Vec<u8>) {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
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

    fn get(port: u16, path: &str) -> (u16, String, Vec<u8>) {
        ask(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
        )
    }

    #[test]
    fn serves_the_site_at_the_root_of_its_own_address() {
        let (_, artifact) = published();
        let server = Server::default();
        let port = server.serve(&artifact, &HashSet::new()).unwrap();
        assert!(PORTS.contains(&port));
        assert_eq!(server.port(artifact.id()), Some(port));

        // The page, with the app's script first in its head (not in the
        // `<header>` before it).
        let (status, head, body) = get(port, "/");
        let body = String::from_utf8(body).unwrap();
        assert_eq!(status, 200);
        assert!(head.contains("content-type: text/html"), "{head}");
        assert!(
            body.contains(
                "<HEAD lang=\"en\"><script src=\"/_artifact/client.js\"></script><title>"
            ),
            "{body}"
        );
        // A built site's absolute paths, a folder's own page, and a route
        // of the page's own.
        assert_eq!(get(port, "/assets/app.js").2, b"let a = 1;");
        let (status, head, _) = get(port, "/docs");
        assert_eq!(status, 301);
        assert!(head.contains("location: /docs/"));
        assert!(
            String::from_utf8(get(port, "/docs/").2)
                .unwrap()
                .contains("<p>docs</p>")
        );
        assert!(
            String::from_utf8(get(port, "/settings/profile").2)
                .unwrap()
                .contains("one")
        );
        assert_eq!(get(port, "/assets/missing.png").0, 404);
        assert_eq!(get(port, "/favicon.ico").0, 204);
        // Nothing above the site, and nothing hidden.
        assert_eq!(get(port, "/../artifact.json").0, 404);
        assert_eq!(get(port, "/%2e%2e/artifact.json").0, 404);
        assert_eq!(get(port, "/.env").0, 404);

        // Part of a file, for a film's seeking.
        let (status, head, body) = ask(
            port,
            &format!(
                "GET /assets/film.mp4 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nRange: bytes=2-5\r\nConnection: close\r\n\r\n"
            ),
        );
        assert_eq!((status, body.as_slice()), (206, &b"2345"[..]));
        assert!(head.contains("content-range: bytes 2-5/10"));

        // Not changed since: no body again.
        let tag = get(port, "/assets/app.js")
            .1
            .lines()
            .find_map(|line| line.strip_prefix("etag: ").map(str::to_string))
            .unwrap();
        let (status, _, body) = ask(
            port,
            &format!(
                "GET /assets/app.js HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nIf-None-Match: {tag}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert_eq!((status, body.len()), (304, 0));

        // Another name pointed at this computer is not the artifact's.
        let (status, ..) = ask(
            port,
            &format!("GET / HTTP/1.1\r\nHost: evil.example:{port}\r\nConnection: close\r\n\r\n"),
        );
        assert_eq!(status, 403);
        assert_eq!(
            ask(port, &format!("POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")).0,
            405
        );

        server.stop(artifact.id());
        assert_eq!(server.port(artifact.id()), None);
    }

    #[test]
    fn a_page_stores_values_and_only_its_own_page_reads_them() {
        let (_, artifact) = published();
        let server = Server::default();
        let port = server.serve(&artifact, &HashSet::new()).unwrap();
        let call = |method: &str, query: &str, body: &str, extra: &str| {
            ask(
                port,
                &format!(
                    "{method} /_artifact/storage{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
            )
        };
        let nothing = call("GET", "?key=todos", "", "");
        assert_eq!((nothing.0, nothing.2.as_slice()), (200, &b"null"[..]));
        assert_eq!(call("PUT", "?key=todos", "[1,2]", "").0, 204);
        assert_eq!(call("PUT", "?key=..%2Fup", "{}", "").0, 204);
        assert_eq!(call("GET", "?key=todos", "", "").2, b"[1,2]");
        let keys: serde_json::Value = serde_json::from_slice(&call("GET", "", "", "").2).unwrap();
        assert_eq!(
            keys["keys"][1],
            serde_json::json!({ "key": "todos", "bytes": 5 })
        );
        assert!(artifact.data().join("todos.json").is_file());
        assert_eq!(call("PUT", "?key=bad", "{oops", "").0, 400);
        assert_eq!(call("PUT", "", "1", "").0, 400);
        assert_eq!(call("DELETE", "?key=todos", "", "").0, 204);
        assert_eq!(call("GET", "?key=todos", "", "").2, b"null");

        // Another site's script, as the browser marks its requests.
        let foreign = "Origin: https://evil.example\r\n";
        assert_eq!(call("GET", "", "", foreign).0, 403);
        assert_eq!(
            call("PUT", "?key=x", "1", "Sec-Fetch-Site: cross-site\r\n").0,
            403
        );
        let own = format!("Origin: http://127.0.0.1:{port}\r\nSec-Fetch-Site: same-origin\r\n");
        assert_eq!(call("PUT", "?key=x", "1", &own).0, 204);
        server.stop(artifact.id());
    }

    #[test]
    fn an_open_page_hears_of_a_new_publish() {
        let (work, artifact) = published();
        let server = Server::default();
        let port = server.serve(&artifact, &HashSet::new()).unwrap();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .write_all(
                format!("GET /_artifact/events?who=p1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut heard = String::new();
        let mut until = |wanted: &str, stream: &mut TcpStream| {
            let mut buffer = [0; 512];
            while !heard.contains(wanted) {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "the stream ended before {wanted:?}: {heard}");
                heard.push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        };
        until("event: version\ndata: 1\n\n", &mut stream);

        std::fs::write(work.join("index.html"), "<body>two</body>").unwrap();
        let again = library::publish(
            artifact.folder.parent().unwrap(),
            &Publish {
                source: work,
                ..Publish::default()
            },
        )
        .unwrap()
        .artifact;
        assert_eq!(server.serve(&again, &HashSet::new()).unwrap(), port);
        until("event: version\ndata: 2\n\n", &mut stream);
        assert!(String::from_utf8(get(port, "/").2).unwrap().contains("two"));
        server.stop(artifact.id());
    }

    /// A page's stream, opened at `port` as `who` (with `cookie`, for a
    /// guest), and what was read of it so far.
    struct Stream {
        stream: TcpStream,
        heard: String,
    }

    impl Stream {
        fn open(port: u16, who: &str, cookie: &str) -> Self {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            write!(
                stream,
                "GET /_artifact/events?who={who}&name={who} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{cookie}\r\n"
            )
            .unwrap();
            Self {
                stream,
                heard: String::new(),
            }
        }

        /// Reads on until `wanted` was heard; answers with the JSON of the
        /// event's data that follows it.
        fn until(&mut self, wanted: &str) -> Value {
            let mut buffer = [0; 1024];
            loop {
                if let Some(at) = self.heard.find(wanted) {
                    let rest = self.heard[at + wanted.len()..].to_string();
                    if let Some(end) = rest.find("\n\n") {
                        self.heard = rest[end..].to_string();
                        return serde_json::from_str(&rest[..end]).unwrap();
                    }
                }
                let read = self.stream.read(&mut buffer).unwrap();
                assert!(
                    read > 0,
                    "the stream ended before {wanted:?}: {}",
                    self.heard
                );
                self.heard
                    .push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        }
    }

    fn share(key: &str, take_part: bool) -> Share {
        Share {
            key: key.into(),
            take_part,
        }
    }

    #[test]
    fn a_guest_needs_the_key_and_never_gets_the_owners_own_storage() {
        let (_, artifact) = published();
        let server = Server::default();
        let own = server.serve(&artifact, &HashSet::new()).unwrap();
        let id = artifact.id();
        let put = |port: u16, call: &str, body: &str, extra: &str| {
            ask(
                port,
                &format!(
                    "PUT /_artifact/{call} HTTP/1.1\r\nHost: 192.168.1.20:{port}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
            )
            .0
        };
        // The owner's own values, and the door to the network.
        let kept = ask(
            own,
            &format!(
                "PUT /_artifact/storage?key=salary HTTP/1.1\r\nHost: 127.0.0.1:{own}\r\nContent-Length: 4\r\nConnection: close\r\n\r\n9000"
            ),
        );
        assert_eq!(kept.0, 204);
        assert_eq!(server.shared(id), None);
        let door = server
            .share(&artifact, share("k3y", true), &HashSet::from([own]))
            .unwrap();
        assert_ne!(door, own);
        assert!(server.any_shared());
        let guest = |path: &str, extra: &str| {
            ask(
                door,
                &format!(
                    "GET {path} HTTP/1.1\r\nHost: 192.168.1.20:{door}\r\n{extra}Connection: close\r\n\r\n"
                ),
            )
        };

        // Without the key, and with a wrong one: a page that says what to
        // ask for, and none of the artifact.
        let (status, _, body) = guest("/", "");
        assert_eq!(status, 403);
        assert!(
            String::from_utf8(body)
                .unwrap()
                .contains("shared by a link")
        );
        assert_eq!(guest("/?invite=nope", "").0, 403);
        assert_eq!(
            guest("/assets/app.js", "Cookie: artifact_other=k3y\r\n").0,
            403
        );

        // With the link: the key goes into a cookie, and the address is
        // plain again.
        let (status, head, _) = guest("/docs/?invite=k3y", "");
        assert_eq!(status, 302);
        assert!(
            head.contains(&format!("set-cookie: artifact_{id}=k3y; path=/")),
            "{head}"
        );
        assert!(
            head.contains("httponly") && head.contains("location: /docs/\r\n"),
            "{head}"
        );
        let cookie = format!("Cookie: theme=dark; artifact_{id}=k3y\r\n");
        let (status, _, body) = guest("/", &cookie);
        assert_eq!(status, 200);
        let body = String::from_utf8(body).unwrap();
        assert!(body.contains("/_artifact/client.js?guest"), "{body}");
        assert_eq!(guest("/assets/app.js", &cookie).2, b"let a = 1;");
        // The owner's page is not a guest's.
        assert!(
            !String::from_utf8(get(own, "/").2)
                .unwrap()
                .contains("?guest")
        );

        // What the owner stored for themselves stays with the owner.
        assert_eq!(guest("/_artifact/storage?key=salary", &cookie).0, 403);
        assert_eq!(guest("/_artifact/storage", &cookie).0, 403);
        // What is shared, everyone reads and writes.
        assert_eq!(put(door, "shared?key=score", "12", &cookie), 204);
        assert_eq!(get(own, "/_artifact/shared?key=score").2, b"12");
        assert!(artifact.data().join("shared/score.json").is_file());
        assert_eq!(
            get(own, "/_artifact/storage?key=score").2,
            b"null",
            "kept apart"
        );

        // Shared to look at only: a guest still reads, and writes nothing.
        let same_door = server
            .share(&artifact, share("k3y", false), &HashSet::from([own]))
            .unwrap();
        assert_eq!(same_door, door);
        assert_eq!(put(door, "shared?key=score", "13", &cookie), 403);
        assert_eq!(guest("/_artifact/shared?key=score", &cookie).2, b"12");
        assert!(!server.shared(id).unwrap().share.take_part);

        // A new link: the old key opens nothing.
        server
            .share(&artifact, share("n3w", false), &HashSet::from([own]))
            .unwrap();
        assert_eq!(guest("/", &cookie).0, 403);
        assert_eq!(guest("/?invite=n3w", "").0, 302);

        // Sharing ends: nobody comes in, and the owner's door still stands.
        server.unshare(id);
        assert_eq!(server.shared(id), None);
        assert!(!server.any_shared());
        std::thread::sleep(Duration::from_millis(100));
        let after = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, door)),
            Duration::from_millis(500),
        )
        .ok()
        .map(|mut stream| {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = write!(
                stream,
                "GET /?invite=n3w HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
            );
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            answer
        });
        assert!(
            after
                .as_deref()
                .is_none_or(|answer| !answer.contains(" 200 ") && !answer.contains(" 302 ")),
            "{after:?}"
        );
        assert_eq!(get(own, "/").0, 200);
        server.stop(id);
    }

    #[test]
    fn people_with_the_page_open_hear_each_other() {
        let (_, artifact) = published();
        let server = Server::default();
        let own = server.serve(&artifact, &HashSet::new()).unwrap();
        let id = artifact.id();
        let door = server
            .share(&artifact, share("k3y", true), &HashSet::from([own]))
            .unwrap();
        let cookie = format!("Cookie: artifact_{id}=k3y\r\n");
        let say = |port: u16, body: String, extra: &str| {
            ask(
                port,
                &format!(
                    "POST /_artifact/room HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
            )
            .0
        };

        let mut maya = Stream::open(own, "maya", "");
        let hello = maya.until("event: hello\ndata: ");
        let maya_page = hello["page"].as_u64().unwrap();
        assert_eq!(
            hello["you"],
            json!({ "id": "maya", "name": "maya", "host": true })
        );
        assert_eq!(hello["change"], json!(true));

        let mut chen = Stream::open(door, "chen", &cookie);
        let hello = chen.until("event: hello\ndata: ");
        let chen_page = hello["page"].as_u64().unwrap();
        assert_eq!(hello["you"]["host"], json!(false));
        assert_eq!(hello["people"].as_array().unwrap().len(), 2);
        let people = maya.until("event: people\ndata: ");
        assert_eq!(people[1]["id"], json!("chen"));
        assert_eq!(server.shared(id).unwrap().guests, 1);

        // Maya moves; Chen hears it, and answers her alone.
        let moved = json!({ "page": maya_page, "messages": [{ "data": { "move": 3 } }] });
        assert_eq!(say(own, moved.to_string(), ""), 204);
        assert_eq!(
            chen.until("event: message\ndata: "),
            json!({ "from": "maya", "data": { "move": 3 } })
        );
        let answer = json!({ "page": chen_page, "messages": [{ "to": "maya", "data": "ok" }] });
        assert_eq!(say(door, answer.to_string(), &cookie), 204);
        assert_eq!(maya.until("event: message\ndata: ")["data"], json!("ok"));
        // Nobody speaks as a page that is not theirs, nor from elsewhere.
        let forged = json!({ "page": maya_page, "messages": [{ "data": 1 }] });
        assert_eq!(say(door, forged.to_string(), &cookie), 409);
        assert_eq!(
            say(own, moved.to_string(), "Origin: https://evil.example\r\n"),
            403
        );

        // What one writes into the shared storage, the other is told of.
        let written = ask(
            own,
            &format!(
                "PUT /_artifact/shared?key=board HTTP/1.1\r\nHost: 127.0.0.1:{own}\r\nX-Artifact-Page: {maya_page}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]"
            ),
        );
        assert_eq!(written.0, 204);
        assert_eq!(
            chen.until("event: shared\ndata: "),
            json!({ "key": "board" })
        );

        // Chen closes the page: Maya is told soon after.
        drop(chen);
        let people = maya.until("event: people\ndata: ");
        assert_eq!(people.as_array().unwrap().len(), 1);
        assert_eq!(server.shared(id).unwrap().guests, 0);

        // A guest's stream ends with the sharing.
        let mut dana = Stream::open(door, "dana", &cookie);
        dana.until("event: hello\ndata: ");
        server.unshare(id);
        let mut rest = Vec::new();
        dana.stream.read_to_end(&mut rest).unwrap();
        server.stop(id);
    }

    #[test]
    fn the_script_goes_where_a_page_has_room_for_it() {
        let tag = std::str::from_utf8(SCRIPT_TAG).unwrap();
        let with = |html: &str| String::from_utf8(with_client(html.as_bytes(), false)).unwrap();
        assert_eq!(with("<html><body>x"), format!("<html>{tag}<body>x"));
        assert_eq!(
            with("<!doctype html><p>x"),
            format!("<!doctype html>{tag}<p>x")
        );
        assert_eq!(with("<p>x"), format!("{tag}<p>x"));
        assert_eq!(with(""), tag);
    }

    #[test]
    fn a_port_another_artifact_has_is_never_taken() {
        let (_, artifact) = published();
        let server = Server::default();
        let first = server.serve(&artifact, &HashSet::new()).unwrap();
        // A second artifact asking for the same port gets another.
        let mut other = artifact.clone();
        other.manifest.id = "other".into();
        other.manifest.port = first;
        let second = server.serve(&other, &HashSet::from([first])).unwrap();
        assert_ne!(first, second);
        server.stop(artifact.id());
        server.stop("other");
    }
}
