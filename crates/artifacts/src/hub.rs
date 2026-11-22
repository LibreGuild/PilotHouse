//! The artifacts as the running app holds them: the list read from their
//! folder, the server that serves them, and every change to them (publish,
//! rename, go back a version, delete), each made off the UI thread and
//! followed by a fresh read, so what the list shows is what is on disk.
//!
//! What only the window shell can do (find a workspace's folders, show a
//! page in its built-in browser, move a folder to the trash) it lends
//! through [`Host`], so this crate depends on none of them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task, Window};

use crate::library::{self, Artifact, Publish, Published};
use crate::server::Server;
use crate::sharing::{self, Network, Share};

/// Artifacts served from the start: the ones changed last, so a page left
/// open in a browser yesterday loads today. The others are served when
/// they are opened.
const SERVED_AT_START: usize = 12;

/// A workspace's name, and the folders an agent of it may publish from.
type WorkspaceOf = dyn Fn(i64, &App) -> Option<(String, Vec<PathBuf>)>;
/// Shows an address in a workspace's built-in browser; a PNG of the page.
type Show = dyn Fn(i64, String, &mut App) -> Task<Result<Vec<u8>>>;
type OpenInWorkspace = dyn Fn(i64, String, &mut Window, &mut App);
/// Makes an artifact of a kind and opens it; opens one there is.
type CreateKind = dyn Fn(&mut Window, &mut App);
type OpenKind = dyn Fn(&Artifact, &mut Window, &mut App);
/// Draws the picture of a kind's card, to fill the card's box, which is
/// as many pixels wide as said (at the default interface size) and 16:10.
type KindArt = dyn Fn(f32, &mut Window, &mut App) -> gpui_kit::AnyElement;

/// A kind of artifact with a window of its own (a deck): the list offers
/// to make one, and opens one there rather than in a browser.
#[derive(Clone)]
pub struct NewKind {
    /// What artifacts of it say they are (`artifact.json`).
    pub kind: &'static str,
    /// What the list calls it ("Slides"), and what making one is for.
    pub label: &'static str,
    pub hint: &'static str,
    pub icon: ph_ui::Icons,
    /// The picture on the card that offers a new one, at the head of the
    /// list: drawn by whoever knows the kind, at each drawing (so it
    /// follows the theme).
    pub art: Rc<KindArt>,
    /// Makes one and opens it.
    pub create: Rc<CreateKind>,
    /// Opens one there is.
    pub open: Rc<OpenKind>,
}

/// What the window shell lends.
#[derive(Clone)]
pub struct Host {
    /// A workspace's name, and the folders an agent of it may publish from.
    pub workspace: Rc<WorkspaceOf>,
    /// Shows an address in the workspace's built-in browser beside its
    /// chat, and answers with a picture of the page (a PNG).
    pub show: Rc<Show>,
    /// Brings the workspace in front with the address in its built-in
    /// browser (the user's own click).
    pub open_in_workspace: Rc<OpenInWorkspace>,
    /// Moves folders to the system's trash.
    pub trash: fn(&[PathBuf]) -> Result<()>,
    /// An artifact was renamed in the list; the path is its folder before
    /// (what is kept by folder elsewhere, a deck's chats, goes with it, and
    /// a deck takes the name as its own).
    pub renamed: fn(&Artifact, &Path, &mut App),
    /// The kinds with a window of their own.
    pub kinds: Vec<NewKind>,
}

struct Hub {
    artifacts: Entity<Artifacts>,
    host: Option<Host>,
}

impl Global for Hub {}

/// Starts the artifacts: reads the folder and serves the latest. Call once,
/// in a build that has the feature.
pub fn init(cx: &mut App) {
    start(library::root(), cx);
}

/// [`init`], with the artifacts kept in `root` (a test's own folder).
pub fn start(root: PathBuf, cx: &mut App) {
    let artifacts = cx.new(|cx| {
        let server = Server::default();
        // Who is here, and what is shared, changes on the server's own
        // threads: the list and the share dialog follow.
        let heard = server.news();
        let mut artifacts = Artifacts {
            root,
            server,
            list: Vec::new(),
            loaded: false,
            network: Network::default(),
            _news: cx.spawn(async move |this, cx| {
                while heard.recv().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }),
        };
        artifacts.reload(SERVED_AT_START, cx).detach();
        artifacts
    });
    cx.set_global(Hub {
        artifacts,
        host: None,
    });
}

/// Says what the shell lends (see [`Host`]).
pub fn set_host(host: Host, cx: &mut App) {
    if cx.has_global::<Hub>() {
        cx.global_mut::<Hub>().host = Some(host);
    }
}

pub(crate) fn host(cx: &App) -> Option<Host> {
    cx.try_global::<Hub>()?.host.clone()
}

/// The folders an agent of workspace (or space) `project` works in: what a
/// kind's own tools find its artifact by (a deck's chat works in the
/// deck's folder).
pub fn folders_of(project: i64, cx: &App) -> Vec<PathBuf> {
    host(cx)
        .and_then(|host| (host.workspace)(project, cx))
        .map(|(_, folders)| folders)
        .unwrap_or_default()
}

/// Workspace (or space) `project`'s name and the folders an agent of it
/// works in, while there is such a workspace.
pub fn workspace_of(project: i64, cx: &App) -> Option<(String, Vec<PathBuf>)> {
    host(cx).and_then(|host| (host.workspace)(project, cx))
}

/// How an artifact is shared with people on the same network.
#[derive(Debug, Clone, PartialEq)]
pub struct Sharing {
    /// The link to give them; `None` while this computer is on no network.
    pub link: Option<String>,
    /// Whether they may take part, or only look.
    pub take_part: bool,
    /// How many of them have the page open now.
    pub guests: usize,
}

impl Artifacts {
    /// Whether a VPN is on here, which may keep colleagues from reaching
    /// this computer whatever link they have.
    pub fn behind_a_tunnel(&self) -> bool {
        self.network.tunnel
    }
}

pub struct Artifacts {
    root: PathBuf,
    server: Server,
    list: Vec<Artifact>,
    /// The folder was read at least once.
    loaded: bool,
    /// Where this computer stood on the network when last asked (a link
    /// carries its address).
    network: Network,
    _news: Task<()>,
}

impl Artifacts {
    /// The app's artifacts, where the build has them and they were started.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Hub>().map(|hub| hub.artifacts.clone())
    }

    /// Every artifact, the one changed last first.
    pub fn all(&self) -> &[Artifact] {
        &self.list
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Where they are kept.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The server they are served by: where a kind is added, and through
    /// which it tells its open pages.
    pub fn server(&self) -> &Server {
        &self.server
    }

    /// Makes an artifact of `kind` named `title`, served when this is done.
    pub fn create(
        &mut self,
        kind: &'static str,
        title: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<Artifact>> {
        let (sender, receiver) = smol::channel::bounded(1);
        let changed = self.change(cx, move |root| {
            let artifact = library::create(root, &title, kind)?;
            let id = artifact.id().to_string();
            let _ = sender.try_send(id.clone());
            Ok((vec![id], 0))
        });
        cx.spawn(async move |this, cx| {
            changed.await?;
            let id = receiver.recv().await.context("the new artifact was lost")?;
            // As it is listed now: served, so its port is known.
            this.read_with(cx, |this, _| this.find(&id).cloned())?
                .context("the new artifact is not listed")
        })
    }

    /// Notes that artifact `id` changed now (a kind worked on in place), so
    /// it leads the list.
    pub fn touch(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(artifact) = self.find(id).cloned() else {
            return;
        };
        let noted = self.change(cx, move |_| {
            library::touch(&artifact)?;
            Ok((Vec::new(), 0))
        });
        cx.background_spawn(async move {
            if let Err(err) = noted.await {
                tracing::warn!("could not note an artifact's change: {err:#}");
            }
        })
        .detach();
    }

    /// The artifact with this id, or (for an agent that names it) this
    /// name.
    pub fn find(&self, wanted: &str) -> Option<&Artifact> {
        let wanted = wanted.trim();
        self.list
            .iter()
            .find(|artifact| artifact.id() == wanted)
            .or_else(|| {
                self.list
                    .iter()
                    .find(|artifact| artifact.manifest.title.eq_ignore_ascii_case(wanted))
            })
    }

    /// How artifact `id` is shared now, if it is.
    pub fn sharing(&self, id: &str) -> Option<Sharing> {
        let shared = self.server.shared(id)?;
        Some(Sharing {
            link: self
                .network
                .address
                .map(|address| sharing::link(address, shared.port, &shared.share.key)),
            take_part: shared.share.take_part,
            guests: shared.guests,
        })
    }

    /// Whether any artifact is open to the network now.
    pub fn any_shared(&self) -> bool {
        self.server.any_shared()
    }

    /// Opens artifact `id` to people on the same network who have its link
    /// (or, shared already, changes how): `take_part` lets them send and
    /// write what is shared, and `new_link` gives it another key, so the
    /// link given before stops working. Its address there is open until
    /// [`Self::unshare`], or until the app quits: nothing is shared again
    /// by itself.
    pub fn share(
        &mut self,
        id: &str,
        take_part: bool,
        new_link: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let (server, id) = (self.server.clone(), id.to_string());
        let shared = self.change(cx, move |root| {
            // Served first: its door on the network stands beside the one
            // on this computer.
            let list = load(root, &server, std::slice::from_ref(&id), 0);
            let artifact = list
                .iter()
                .find(|artifact| artifact.id() == id)
                .context("the artifact is not there any more")?;
            let manifest = &artifact.manifest;
            let key = if new_link || manifest.share_key.is_empty() {
                sharing::new_key()?
            } else {
                manifest.share_key.clone()
            };
            // Every port an artifact has, but the one it was shared on.
            let mut taken: HashSet<u16> = list
                .iter()
                .flat_map(|artifact| [artifact.manifest.port, artifact.manifest.share_port])
                .filter(|port| *port != 0)
                .collect();
            if manifest.share_port != manifest.port {
                taken.remove(&manifest.share_port);
            }
            let share = Share {
                key: key.clone(),
                take_part,
            };
            let port = server.share(artifact, share, &taken)?;
            if key != manifest.share_key || port != manifest.share_port {
                library::set_share(artifact, &key, port)?;
            }
            Ok((vec![id], 0))
        });
        cx.spawn(async move |this, cx| {
            shared.await?;
            let network = cx.background_spawn(async { sharing::network() }).await;
            this.update(cx, |this, cx| {
                this.network = network;
                cx.notify();
            })
        })
    }

    /// Ends the sharing of artifact `id`: its address on the network
    /// closes, and those who had it open are let go.
    pub fn unshare(&mut self, id: &str, cx: &mut Context<Self>) {
        self.server.unshare(id);
        cx.notify();
    }

    /// Asks the system anew which address this computer has on the network
    /// (it changes with the network).
    pub fn refresh_address(&mut self, cx: &mut Context<Self>) {
        let found = cx.background_spawn(async { sharing::network() });
        cx.spawn(async move |this, cx| {
            let network = found.await;
            _ = this.update(cx, |this, cx| {
                if this.network != network {
                    this.network = network;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reads the folder again (it is the user's: they may have changed it
    /// by hand).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload(0, cx).detach();
    }

    fn reload(&mut self, recent: usize, cx: &mut Context<Self>) -> Task<Result<()>> {
        self.change(cx, move |_| Ok((Vec::new(), recent)))
    }

    /// Runs `work` off the UI thread, then reads the folder again and has
    /// the server take on what changed. `work` answers with the ids that
    /// must be served afterwards, and how many of the latest besides.
    fn change(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Path) -> Result<(Vec<String>, usize)> + Send + 'static,
    ) -> Task<Result<()>> {
        let (root, server) = (self.root.clone(), self.server.clone());
        let done = cx.background_spawn(async move {
            let worked = work(&root);
            let (serve, recent) = worked.as_ref().cloned().unwrap_or_default();
            // Read again also after a failure: half of it may have happened.
            (worked.map(|_| ()), load(&root, &server, &serve, recent))
        });
        cx.spawn(async move |this, cx| {
            let (result, list) = done.await;
            this.update(cx, |this, cx| {
                this.list = list;
                this.loaded = true;
                cx.notify();
            })?;
            result
        })
    }

    /// Publishes `request`; the artifact is served when this is done.
    pub fn publish(&mut self, request: Publish, cx: &mut Context<Self>) -> Task<Result<Published>> {
        let (sender, receiver) = smol::channel::bounded(1);
        let changed = self.change(cx, move |root| {
            let published = library::publish(root, &request)?;
            let id = published.artifact.id().to_string();
            let _ = sender.try_send(published);
            Ok((vec![id], 0))
        });
        cx.spawn(async move |this, cx| {
            changed.await?;
            let mut published = receiver.recv().await.context("the publish was lost")?;
            // As it is listed now: served, so its port is known.
            this.read_with(cx, |this, _| {
                if let Some(listed) = this.find(published.artifact.id()) {
                    published.artifact = listed.clone();
                }
            })?;
            Ok(published)
        })
    }

    /// The artifact's address, once it is served (it is, unless it was not
    /// among the latest when the app started).
    pub fn address(&mut self, id: &str, cx: &mut Context<Self>) -> Task<Result<String>> {
        if let Some(port) = self.server.port(id) {
            return Task::ready(Ok(library::url_of(port)));
        }
        let wanted = id.to_string();
        let served = self.change(cx, {
            let wanted = wanted.clone();
            move |_| Ok((vec![wanted], 0))
        });
        cx.spawn(async move |this, cx| {
            served.await?;
            this.read_with(cx, |this, _| this.server.port(&wanted))?
                .map(library::url_of)
                .context("the artifact could not be served")
        })
    }

    pub fn rename(&mut self, id: &str, title: String, cx: &mut Context<Self>) -> Task<Result<()>> {
        let Some(artifact) = self.find(id).cloned() else {
            return Task::ready(Ok(()));
        };
        let (id, before) = (id.to_string(), artifact.folder.clone());
        let renamed = self.change(cx, move |_| {
            library::rename(&artifact, &title)?;
            Ok((Vec::new(), 0))
        });
        cx.spawn(async move |this, cx| {
            renamed.await?;
            // Its folder took the new name: who keeps things by folder hears.
            let after = this.read_with(cx, |this, _| this.find(&id).cloned())?;
            if let Some(after) = after {
                cx.update(|cx| {
                    if let Some(host) = host(cx) {
                        (host.renamed)(&after, &before, cx);
                    }
                });
            }
            Ok(())
        })
    }

    /// Gives artifact `id` another name without moving its folder (see
    /// [`library::retitle`]).
    pub fn retitle(&mut self, id: &str, title: String, cx: &mut Context<Self>) -> Task<Result<()>> {
        let Some(artifact) = self.find(id).cloned() else {
            return Task::ready(Ok(()));
        };
        self.change(cx, move |_| {
            library::retitle(&artifact, &title)?;
            Ok((Vec::new(), 0))
        })
    }

    /// Says which workspace artifact `id` belongs to (see
    /// [`library::set_source`]).
    pub fn set_source(
        &mut self,
        id: &str,
        source: Option<library::Source>,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(artifact) = self.find(id).cloned() else {
            return Task::ready(Err(anyhow::anyhow!("the artifact is not there any more")));
        };
        self.change(cx, move |_| {
            library::set_source(&artifact, source)?;
            Ok((Vec::new(), 0))
        })
    }

    /// Puts an earlier publish back in place (see [`library::restore`]).
    pub fn restore(&mut self, id: &str, version: u32, cx: &mut Context<Self>) -> Task<Result<()>> {
        let Some(artifact) = self.find(id).cloned() else {
            return Task::ready(Ok(()));
        };
        self.change(cx, move |_| {
            library::restore(&artifact, version)?;
            Ok((Vec::new(), 0))
        })
    }

    /// Moves the artifact, with what it stored, to the system's trash.
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) -> Task<Result<()>> {
        let (Some(artifact), Some(host)) = (self.find(id).cloned(), host(cx)) else {
            return Task::ready(Ok(()));
        };
        // Its files are let go of first: Windows moves no folder a file of
        // which is being sent.
        self.server.stop(artifact.id());
        let trash = host.trash;
        self.change(cx, move |_| {
            trash(std::slice::from_ref(&artifact.folder))?;
            Ok((Vec::new(), 0))
        })
    }

    /// Keeps `png` as the artifact's picture in the list.
    pub fn set_thumbnail(
        &mut self,
        id: &str,
        png: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(artifact) = self.find(id).cloned() else {
            return Task::ready(Ok(()));
        };
        let picture = artifact.thumbnail();
        let written = self.change(cx, move |_| {
            library::set_thumbnail(&artifact, &png)?;
            Ok((Vec::new(), 0))
        });
        cx.spawn(async move |this, cx| {
            written.await?;
            // The picture drawn before, kept by its path, is read again.
            this.update(cx, |_, cx| {
                gpui_kit::ImageSource::from(picture).remove_asset(cx);
                cx.notify();
            })
        })
    }
}

/// The artifacts in `root`, with the server brought up to them: those it
/// serves take on their folder, page and version as they are now, `serve`
/// and the `recent` latest are served too, and what is gone is let go.
/// A port the server had to change is noted in the artifact's own file.
fn load(root: &Path, server: &Server, serve: &[String], recent: usize) -> Vec<Artifact> {
    let mut list = library::list(root);
    // Every port an artifact has: the one it is served on, and the one it
    // was shared on.
    let mut ports: HashSet<u16> = list
        .iter()
        .flat_map(|artifact| [artifact.manifest.port, artifact.manifest.share_port])
        .filter(|port| *port != 0)
        .collect();
    for (ix, artifact) in list.iter_mut().enumerate() {
        let wanted = ix < recent
            || serve.iter().any(|id| id == artifact.id())
            || server.port(artifact.id()).is_some();
        if !wanted {
            continue;
        }
        let mut others = ports.clone();
        others.remove(&artifact.manifest.port);
        match server.serve(artifact, &others) {
            Ok(port) if port != artifact.manifest.port => {
                if let Err(err) = library::set_port(artifact, port) {
                    tracing::warn!("could not note an artifact's port: {err:#}");
                }
                ports.insert(port);
                artifact.manifest.port = port;
            }
            Ok(_) => {}
            Err(err) => tracing::warn!("could not serve {}: {err:#}", artifact.manifest.title),
        }
    }
    for id in server.served() {
        if !list.iter().any(|artifact| artifact.id() == id) {
            server.stop(&id);
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_serves_what_is_asked_for_and_notes_its_port() {
        let base = std::env::temp_dir().join(format!("ph-artifact-hub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (work, root) = (base.join("work"), base.join("root"));
        std::fs::create_dir_all(&work).unwrap();
        let mut ids = Vec::new();
        for name in ["one", "two"] {
            std::fs::write(work.join(format!("{name}.html")), name).unwrap();
            let published = library::publish(
                &root,
                &Publish {
                    source: work.join(format!("{name}.html")),
                    ..Publish::default()
                },
            )
            .unwrap();
            ids.push(published.artifact.id().to_string());
        }
        let server = Server::default();
        // Nothing asked for: nothing served.
        assert!(
            load(&root, &server, &[], 0)
                .iter()
                .all(|a| a.manifest.port == 0)
        );
        let listed = load(&root, &server, &[ids[0].clone()], 0);
        let port = server.port(&ids[0]).expect("served");
        let one = listed.iter().find(|a| a.id() == ids[0]).unwrap();
        assert_eq!(one.manifest.port, port);
        assert_eq!(
            library::read(&one.folder).unwrap().manifest.port,
            port,
            "noted on disk"
        );
        assert_eq!(server.port(&ids[1]), None);

        // Its folder gone: it is let go.
        std::fs::remove_dir_all(&one.folder).unwrap();
        assert_eq!(load(&root, &server, &[], 0).len(), 1);
        assert_eq!(server.port(&ids[0]), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
