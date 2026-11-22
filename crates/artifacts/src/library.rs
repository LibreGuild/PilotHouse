//! The artifacts on disk: one folder each in `Documents/<app> Artifacts/`,
//! holding the page as it was published (`site/`), what the page stored
//! (`data/`), the sites it replaced (`.versions/`) and a small file saying
//! what it is (`artifact.json`).
//!
//! A folder of the user's, not the app's data folder: they can find it,
//! back it up, zip it for someone. Publishing copies; the files an agent
//! built stay where they are, and what is served never changes under a
//! reader until the next publish swaps the whole site in.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

pub const MANIFEST: &str = "artifact.json";
/// The published files, as the server serves them.
pub const SITE: &str = "site";
/// What the page stored (`storage.rs`): one file per key.
pub const DATA: &str = "data";
/// The sites earlier publishes put there, by their number.
pub const VERSIONS: &str = ".versions";
/// A small picture of the page, for the list.
pub const THUMBNAIL: &str = ".thumbnail.png";

/// Earlier sites kept: enough to go back after a bad publish, without a
/// large site filling the disk five times over for ever.
const KEPT_VERSIONS: usize = 5;
/// What one publish may copy. A whole repository given by mistake stops
/// here instead of filling the Documents folder.
pub const MAX_FILES: usize = 5_000;
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// The ports artifacts are served on, one each: below every system's range
/// for outgoing connections, so one is rarely taken when the app starts.
pub const PORTS: std::ops::RangeInclusive<u16> = 23_100..=23_999;

/// Where artifacts live: `PILOTHOUSE_ARTIFACTS` when set; in a data folder
/// of its own choosing (`PILOTHOUSE_DATA_DIR`: tests, scratch profiles),
/// inside that folder, so such a run never writes among the user's
/// documents nor serves their artifacts a second time; else `<app>
/// Artifacts` in the Documents folder (or the home folder without one).
pub fn root() -> PathBuf {
    let set = |name: &str| std::env::var_os(name).filter(|dir| !dir.is_empty());
    if let Some(dir) = set("PILOTHOUSE_ARTIFACTS") {
        return PathBuf::from(dir);
    }
    if let Some(data) = set("PILOTHOUSE_DATA_DIR") {
        return PathBuf::from(data).join("artifacts");
    }
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("{} Artifacts", ph_brand::NAME))
}

/// Where an artifact was published from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    /// The workspace's id, and its name then (it may be gone since).
    pub project: i64,
    pub workspace: String,
    /// The folder or file that was published.
    pub path: PathBuf,
}

/// `artifact.json`. Fields a build does not know are kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    /// Its own for good, whatever the folder is called.
    pub id: String,
    pub title: String,
    pub description: String,
    /// What it is besides a page of its own making: empty for a page an
    /// agent published; `slides` for a deck, whose page is the app's own
    /// (see [`crate::Kind`]) and whose `site/` holds what that page shows.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The page a visit to `/` shows, relative to `site/`.
    pub entry: String,
    /// The port it is served on. It stays the same, so its address does,
    /// and with it what a browser keeps for that address.
    pub port: u16,
    /// Seconds since 1970.
    pub created: u64,
    pub updated: u64,
    /// Which publish is live (1 for the first).
    pub version: u32,
    pub files: usize,
    pub bytes: u64,
    pub source: Option<Source>,
    /// The key of its link, once it was shared: the same link works each
    /// time it is shared again, until the owner asks for a new one.
    pub share_key: String,
    /// The port of its address on the network while it is shared (0 until
    /// it first was).
    pub share_port: u16,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// One artifact: its folder and what it says of itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Artifact {
    pub folder: PathBuf,
    pub manifest: Manifest,
    /// Whether a picture of its page is kept (it is once the page was
    /// shown in the built-in browser).
    pub picture: bool,
}

impl Artifact {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn site(&self) -> PathBuf {
        self.folder.join(SITE)
    }

    pub fn data(&self) -> PathBuf {
        self.folder.join(DATA)
    }

    pub fn thumbnail(&self) -> PathBuf {
        self.folder.join(THUMBNAIL)
    }

    /// Its address on this computer. `127.0.0.1`, not `localhost`: that
    /// name means the IPv6 loopback first on some systems, where nothing
    /// listens.
    pub fn url(&self) -> String {
        url_of(self.manifest.port)
    }

    fn save(&self) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.manifest)?;
        write_atomic(&self.folder.join(MANIFEST), &json)
    }
}

pub fn url_of(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// What to publish.
#[derive(Debug, Clone, Default)]
pub struct Publish {
    /// A folder with the page in it, or one HTML file. Already checked to
    /// be the caller's to give (`control.rs`).
    pub source: PathBuf,
    pub title: Option<String>,
    pub description: Option<String>,
    /// The page to show first, relative to a folder source, when it is not
    /// its `index.html`.
    pub entry: Option<String>,
    /// The artifact to replace (its id). Without one, the artifact last
    /// published from the same source is replaced, else a new one made.
    pub artifact: Option<String>,
    pub origin: Option<Source>,
}

/// How a publish went.
#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub artifact: Artifact,
    /// A new artifact, rather than a new version of one.
    pub created: bool,
    /// Entries of the source left out: hidden ones, `node_modules`, links.
    pub skipped: usize,
}

/// Every artifact in `root`, the one changed last first. Two folders with
/// one id (a folder copied by hand) are told apart: the copy gets an id and
/// a port of its own.
pub fn list(root: &Path) -> Vec<Artifact> {
    let mut found: Vec<Artifact> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| read(&entry.path()))
        .collect();
    found.sort_by(|a, b| (b.manifest.updated, &a.folder).cmp(&(a.manifest.updated, &b.folder)));
    let mut ids = HashSet::new();
    let mut ports = HashSet::new();
    // The older of two keeps the id: go through them oldest first.
    for artifact in found.iter_mut().rev() {
        let fresh_id = !ids.insert(artifact.manifest.id.clone());
        let fresh_port = artifact.manifest.port != 0 && !ports.insert(artifact.manifest.port);
        if fresh_id {
            artifact.manifest.id = new_id();
            ids.insert(artifact.manifest.id.clone());
            // Nor does a copy answer to the original's link.
            artifact.manifest.share_key.clear();
            artifact.manifest.share_port = 0;
        }
        if fresh_port {
            artifact.manifest.port = 0;
        }
        if (fresh_id || fresh_port)
            && let Err(err) = artifact.save()
        {
            tracing::warn!("could not rewrite {}: {err:#}", artifact.folder.display());
        }
    }
    found
}

/// The artifact in `folder`, if it is one.
pub fn read(folder: &Path) -> Option<Artifact> {
    let bytes = std::fs::read(folder.join(MANIFEST)).ok()?;
    let mut manifest: Manifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.id.is_empty() || !folder.join(SITE).is_dir() {
        return None;
    }
    if manifest.title.trim().is_empty() {
        manifest.title = folder.file_name()?.to_string_lossy().into_owned();
    }
    if manifest.entry.is_empty() {
        manifest.entry = "index.html".into();
    }
    Some(Artifact {
        picture: folder.join(THUMBNAIL).is_file(),
        folder: folder.to_path_buf(),
        manifest,
    })
}

/// Publishes `request` into `root`: copies the source beside the live site,
/// then swaps it in, so a reader sees the old site or the new one, never
/// half of each. The site it replaces is kept (`.versions/`).
/// Makes an artifact of a kind (a deck): its folder and an empty `site/`,
/// for the kind to fill. Nothing is published into it: what is in `site/`
/// is worked on in place.
pub fn create(root: &Path, title: &str, kind: &str) -> Result<Artifact> {
    std::fs::create_dir_all(root).with_context(|| format!("could not make {}", root.display()))?;
    let folder = free_folder(root, &folder_name(title), None);
    std::fs::create_dir_all(folder.join(SITE))?;
    let now = now();
    let artifact = Artifact {
        folder,
        manifest: Manifest {
            id: new_id(),
            title: title.to_string(),
            kind: kind.to_string(),
            entry: "index.html".into(),
            created: now,
            updated: now,
            version: 1,
            ..Manifest::default()
        },
        picture: false,
    };
    artifact.save()?;
    Ok(artifact)
}

/// Notes that the artifact changed now, so it leads the list. For kinds
/// worked on in place; a publish notes it itself.
pub fn touch(artifact: &Artifact) -> Result<()> {
    let mut artifact = read(&artifact.folder).context("the artifact is not there any more")?;
    artifact.manifest.updated = now();
    artifact.save()
}

pub fn publish(root: &Path, request: &Publish) -> Result<Published> {
    let source = &request.source;
    let plan = Plan::of(source, request.entry.as_deref())?;
    let existing = list(root);
    let target = match &request.artifact {
        Some(id) => Some(
            existing
                .iter()
                .find(|artifact| artifact.id() == id)
                .with_context(|| format!("there is no artifact with the id {id}"))?
                .clone(),
        ),
        // Published from here before: that one again, not a second one.
        None => existing
            .iter()
            .find(|artifact| {
                artifact
                    .manifest
                    .source
                    .as_ref()
                    .is_some_and(|known| known.path == *source)
            })
            .cloned(),
    };
    let created = target.is_none();
    let now = now();
    let mut artifact = match target {
        Some(artifact) => artifact,
        None => {
            let title = request
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .or_else(|| plan.page_title())
                .unwrap_or_else(|| plan.fallback_title());
            std::fs::create_dir_all(root)
                .with_context(|| format!("could not make {}", root.display()))?;
            let folder = free_folder(root, &folder_name(&title), None);
            std::fs::create_dir_all(&folder)?;
            Artifact {
                folder,
                manifest: Manifest {
                    id: new_id(),
                    title,
                    created: now,
                    ..Manifest::default()
                },
                picture: false,
            }
        }
    };

    let staging = artifact
        .folder
        .join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let copied = plan.copy_into(&staging);
    let (files, bytes) = match copied {
        Ok(counts) => counts,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            // A folder made for an artifact that never came to be.
            if created {
                let _ = std::fs::remove_dir_all(&artifact.folder);
            }
            return Err(err);
        }
    };

    let site = artifact.site();
    let next = next_version(&artifact);
    if site.is_dir() {
        let kept = artifact.folder.join(VERSIONS);
        std::fs::create_dir_all(&kept)?;
        std::fs::rename(
            &site,
            kept.join(artifact.manifest.version.max(1).to_string()),
        )
        .context("could not set the site before aside")?;
    }
    std::fs::rename(&staging, &site).context("could not put the new site in place")?;
    prune_versions(&artifact.folder);

    if let Some(title) = request
        .title
        .as_ref()
        .filter(|title| !title.trim().is_empty())
    {
        artifact.manifest.title = title.trim().to_string();
    }
    if let Some(description) = &request.description {
        artifact.manifest.description = description.trim().to_string();
    }
    artifact.manifest.entry = plan.entry.clone();
    artifact.manifest.version = next;
    artifact.manifest.updated = now;
    artifact.manifest.files = files;
    artifact.manifest.bytes = bytes;
    // Always noted, also without a workspace to name: the next publish
    // from here finds this artifact by it.
    artifact.manifest.source = Some(request.origin.clone().unwrap_or_else(|| Source {
        path: source.clone(),
        ..Source::default()
    }));
    artifact.save()?;
    Ok(Published {
        artifact,
        created,
        skipped: plan.skipped,
    })
}

/// What a publish will copy, worked out (and refused) before a byte is.
struct Plan {
    /// Each file: where it is, and where in the site it goes.
    files: Vec<(PathBuf, PathBuf)>,
    entry: String,
    skipped: usize,
    source: PathBuf,
}

impl Plan {
    fn of(source: &Path, entry: Option<&str>) -> Result<Self> {
        let meta = std::fs::symlink_metadata(source)
            .with_context(|| format!("{} is not there", source.display()))?;
        if meta.is_file() {
            if !is_html(source) {
                bail!(
                    "{} is not an HTML file: publish the page (.html), or the folder it is in",
                    source.display()
                );
            }
            // Alone, under the name a visit to `/` finds.
            return Ok(Self {
                files: vec![(source.to_path_buf(), PathBuf::from("index.html"))],
                entry: "index.html".into(),
                skipped: 0,
                source: source.to_path_buf(),
            });
        }
        if !meta.is_dir() {
            bail!("{} is neither a folder nor a file", source.display());
        }
        let mut plan = Self {
            files: Vec::new(),
            entry: String::new(),
            skipped: 0,
            source: source.to_path_buf(),
        };
        let mut bytes = 0;
        plan.walk(source, Path::new(""), &mut bytes)?;
        plan.entry = plan.entry_of(entry)?;
        Ok(plan)
    }

    fn walk(&mut self, dir: &Path, within: &Path, bytes: &mut u64) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("could not read {}", dir.display()))?
            .flatten()
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let text = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // Hidden files hold secrets and tools' own things (`.env`,
            // `.git`); a link may lead out of what the agent may give.
            if text.starts_with('.') || text == "node_modules" || kind.is_symlink() {
                self.skipped += 1;
                continue;
            }
            let to = within.join(&name);
            if kind.is_dir() {
                self.walk(&entry.path(), &to, bytes)?;
            } else if kind.is_file() {
                *bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                self.files.push((entry.path(), to));
                if self.files.len() > MAX_FILES || *bytes > MAX_BYTES {
                    bail!(
                        "{} is too much for one artifact (at most {MAX_FILES} files and {} MB): publish the folder with the built page only, not the whole project",
                        self.source.display(),
                        MAX_BYTES / (1024 * 1024)
                    );
                }
            }
        }
        Ok(())
    }

    /// The page shown first: the one asked for, `index.html`, or the only
    /// page at the top.
    fn entry_of(&self, asked: Option<&str>) -> Result<String> {
        let has = |path: &Path| self.files.iter().any(|(_, to)| to == path);
        if let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) {
            let path = relative(asked).with_context(|| {
                format!("`entry` must be a path inside the folder, not {asked}")
            })?;
            if !has(&path) {
                bail!("the folder has no {asked}");
            }
            return Ok(slashed(&path));
        }
        if has(Path::new("index.html")) {
            return Ok("index.html".into());
        }
        let pages: Vec<&PathBuf> = self
            .files
            .iter()
            .map(|(_, to)| to)
            .filter(|to| is_html(to))
            .collect();
        let top: Vec<&&PathBuf> = pages
            .iter()
            .filter(|to| to.components().count() == 1)
            .collect();
        match (top.as_slice(), pages.as_slice()) {
            ([only], _) => Ok(slashed(only)),
            (_, []) => bail!(
                "{} has no HTML page: publish a folder with an index.html (the built site, not its sources)",
                self.source.display()
            ),
            _ => {
                let names: Vec<String> = pages.iter().take(8).map(|page| slashed(page)).collect();
                bail!(
                    "{} has no index.html: say which page opens first with `entry` ({})",
                    self.source.display(),
                    names.join(", ")
                )
            }
        }
    }

    /// The page's own `<title>`, when it has one.
    fn page_title(&self) -> Option<String> {
        let (from, _) = self
            .files
            .iter()
            .find(|(_, to)| slashed(to) == self.entry)?;
        let mut head = vec![0; 64 * 1024];
        let read = {
            use std::io::Read as _;
            std::fs::File::open(from).ok()?.read(&mut head).ok()?
        };
        let head = String::from_utf8_lossy(&head[..read]).into_owned();
        let lower = head.to_ascii_lowercase();
        let open = lower.find("<title")?;
        let start = open + lower[open..].find('>')? + 1;
        let end = start + lower[start..].find("</title")?;
        let title = head[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!title.is_empty()).then(|| title.chars().take(120).collect())
    }

    /// A name from the source's own: the folder, or the file without `.html`.
    fn fallback_title(&self) -> String {
        let name = if self.source.is_file() {
            self.source.file_stem()
        } else {
            // `dist` and its like say nothing: the project's folder does.
            match self.source.file_name().and_then(|name| name.to_str()) {
                Some("dist" | "build" | "out" | "public" | "site" | "www") => self
                    .source
                    .parent()
                    .and_then(Path::file_name)
                    .or(self.source.file_name()),
                _ => self.source.file_name(),
            }
        };
        name.map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Untitled artifact".into())
    }

    fn copy_into(&self, site: &Path) -> Result<(usize, u64)> {
        let mut bytes = 0;
        for (from, to) in &self.files {
            let to = site.join(to);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            bytes += std::fs::copy(from, &to)
                .with_context(|| format!("could not copy {}", from.display()))?;
        }
        std::fs::create_dir_all(site)?;
        Ok((self.files.len(), bytes))
    }
}

fn is_html(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
        })
}

/// `text` as a path that stays inside a folder: plain names only.
fn relative(text: &str) -> Option<PathBuf> {
    let path = Path::new(text.trim_start_matches("./"));
    let plain = path
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    (plain && path.components().count() > 0).then(|| path.to_path_buf())
}

/// A relative path as an address writes it.
fn slashed(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The earlier sites of `artifact`, newest first: their number and when
/// they were published.
pub fn versions(artifact: &Artifact) -> Vec<(u32, SystemTime)> {
    let mut found: Vec<(u32, SystemTime)> = std::fs::read_dir(artifact.folder.join(VERSIONS))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let number = entry.file_name().to_string_lossy().parse().ok()?;
            let when = entry.metadata().ok()?.modified().ok()?;
            entry.path().is_dir().then_some((number, when))
        })
        .collect();
    found.sort_by_key(|(number, _)| std::cmp::Reverse(*number));
    found
}

/// The number the next publish gets: after every one there was.
fn next_version(artifact: &Artifact) -> u32 {
    versions(artifact)
        .first()
        .map_or(0, |(number, _)| *number)
        .max(artifact.manifest.version)
        + 1
}

fn prune_versions(folder: &Path) {
    let artifact = Artifact {
        folder: folder.to_path_buf(),
        manifest: Manifest::default(),
        picture: false,
    };
    for (number, _) in versions(&artifact).into_iter().skip(KEPT_VERSIONS) {
        let _ = std::fs::remove_dir_all(folder.join(VERSIONS).join(number.to_string()));
    }
}

/// Puts the site of publish `version` back in place; the live one takes its
/// place among the earlier ones, so going back can itself be undone.
pub fn restore(artifact: &Artifact, version: u32) -> Result<Artifact> {
    let kept = artifact.folder.join(VERSIONS);
    let wanted = kept.join(version.to_string());
    if !wanted.is_dir() {
        bail!("version {version} is not kept any more");
    }
    let site = artifact.site();
    let aside = kept.join(artifact.manifest.version.max(1).to_string());
    std::fs::rename(&site, &aside).context("could not set the live site aside")?;
    if let Err(err) = std::fs::rename(&wanted, &site) {
        // Back as it was, rather than an artifact with no site.
        let _ = std::fs::rename(&aside, &site);
        return Err(err).context("could not put the earlier site in place");
    }
    let mut restored = artifact.clone();
    restored.manifest.version = version;
    restored.manifest.updated = now();
    restored.manifest.entry = entry_in(&site, &restored.manifest.entry);
    restored.save()?;
    Ok(restored)
}

/// The page to show first in `site`: `wanted` while it is there, else its
/// `index.html`, else its first page.
fn entry_in(site: &Path, wanted: &str) -> String {
    if site.join(wanted).is_file() || !site.is_dir() {
        return wanted.to_string();
    }
    if site.join("index.html").is_file() {
        return "index.html".into();
    }
    std::fs::read_dir(site)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_file() && is_html(path))
        .and_then(|path| Some(path.file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| wanted.to_string())
}

/// Gives `artifact` another name: in what it says of itself, and its
/// folder's (kept apart from the others').
pub fn rename(artifact: &Artifact, title: &str) -> Result<Artifact> {
    let title = title.trim();
    if title.is_empty() {
        bail!("an artifact needs a name");
    }
    let mut renamed = artifact.clone();
    renamed.manifest.title = title.to_string();
    if let Some(root) = artifact.folder.parent() {
        let folder = free_folder(root, &folder_name(title), Some(&artifact.folder));
        if folder != artifact.folder {
            std::fs::rename(&artifact.folder, &folder).context("could not rename the folder")?;
            renamed.folder = folder;
        }
    }
    renamed.save()?;
    Ok(renamed)
}

/// Gives `artifact` another name where it says its own, leaving its folder
/// where it is: for a kind that is being worked in (an agent's working
/// folder must not move under it).
pub fn retitle(artifact: &Artifact, title: &str) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        bail!("an artifact needs a name");
    }
    let mut renamed = read(&artifact.folder).context("the artifact is not there any more")?;
    renamed.manifest.title = title.to_string();
    renamed.manifest.updated = now();
    renamed.save()
}

/// Notes the workspace `artifact` belongs to (`None`: none any more). For a
/// kind worked on beside a workspace's chat; a publish notes its own.
pub fn set_source(artifact: &Artifact, source: Option<Source>) -> Result<()> {
    let mut changed = read(&artifact.folder).context("the artifact is not there any more")?;
    changed.manifest.source = source;
    changed.save()
}

/// Notes the key and the port `artifact` is shared with.
pub fn set_share(artifact: &Artifact, key: &str, port: u16) -> Result<()> {
    let mut changed = artifact.clone();
    changed.manifest.share_key = key.to_string();
    changed.manifest.share_port = port;
    changed.save()
}

/// Notes the port `artifact` is served on.
pub fn set_port(artifact: &Artifact, port: u16) -> Result<()> {
    let mut changed = artifact.clone();
    changed.manifest.port = port;
    changed.save()
}

/// Keeps `png` as the artifact's picture, made small: the list draws many
/// of them, and a picture of a whole pane each would take hundreds of
/// megabytes.
pub fn set_thumbnail(artifact: &Artifact, png: &[u8]) -> Result<()> {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 400;
    let picture = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .context("the picture of the page could not be read")?;
    // The top of the page at the list's proportions, whatever the pane's.
    let width = picture.width().max(1);
    let height =
        (width as u64 * HEIGHT as u64 / WIDTH as u64).clamp(1, picture.height().max(1) as u64);
    let small = picture.crop_imm(0, 0, width, height as u32).resize(
        WIDTH,
        HEIGHT,
        image::imageops::FilterType::Triangle,
    );
    let mut out = std::io::Cursor::new(Vec::new());
    small.write_to(&mut out, image::ImageFormat::Png)?;
    write_atomic(&artifact.thumbnail(), out.get_ref())
}

/// A folder name from a title: what file systems take, never empty.
pub fn folder_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned: String = cleaned
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(80)
        .collect();
    // Cut short, it may end in a space or a dot again (Windows refuses
    // those), and Windows keeps a few names for devices.
    let cleaned = cleaned
        .trim_end()
        .trim_end_matches('.')
        .trim_end()
        .to_string();
    let stem = cleaned
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if cleaned.is_empty() {
        "Untitled artifact".into()
    } else if reserved {
        format!("{cleaned} artifact")
    } else {
        cleaned
    }
}

/// `root/base`, or the first `root/base N` that is free (`own` counts as
/// free: it is the folder being renamed).
fn free_folder(root: &Path, base: &str, own: Option<&Path>) -> PathBuf {
    let mut path = root.join(base);
    let mut n = 2;
    while path.symlink_metadata().is_ok() && Some(path.as_path()) != own {
        path = root.join(format!("{base} {n}"));
        n += 1;
    }
    path
}

/// An id of its own: eight letters and digits, from the clock and a count,
/// so two made in the same instant differ.
fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|now| now.as_nanos() as u64)
        .unwrap_or_default();
    let mut value = nanos
        ^ (u64::from(std::process::id()) << 32)
        ^ COUNT
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    // Mixed, so ids made one after another share no prefix.
    value ^= value >> 33;
    value = value.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    value ^= value >> 33;
    const LETTERS: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    (0..8)
        .map(|ix| LETTERS[((value >> (ix * 5)) & 31) as usize] as char)
        .collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|now| now.as_secs())
        .unwrap_or_default()
}

/// Writes beside `path` and moves the file over it, so a reader never sees
/// half of it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .context("no file name")?
        .to_string_lossy()
        .into_owned();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, bytes).with_context(|| format!("could not write {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ph-artifacts-{name}-{}-{}",
            std::process::id(),
            new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dunce::canonicalize(&dir).unwrap()
    }

    fn page(dir: &Path, title: &str) {
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            format!(
                "<!doctype html><html><head><title> {title} </title></head><body></body></html>"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("assets/app.js"), "console.log(1)").unwrap();
    }

    #[test]
    fn publishing_copies_the_page_and_again_replaces_it() {
        let (work, root) = (scratch("work"), scratch("root"));
        page(&work, "Budget  planner");
        std::fs::write(work.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(work.join("node_modules/x")).unwrap();
        let request = Publish {
            source: work.clone(),
            ..Publish::default()
        };

        let first = publish(&root, &request).unwrap();
        assert!(first.created);
        assert_eq!(first.skipped, 2, "the hidden file and node_modules");
        let artifact = &first.artifact;
        assert_eq!(
            artifact.manifest.title, "Budget planner",
            "the page's own title"
        );
        assert_eq!(artifact.folder, root.join("Budget planner"));
        assert_eq!(artifact.manifest.entry, "index.html");
        assert_eq!((artifact.manifest.version, artifact.manifest.files), (1, 2));
        assert!(artifact.site().join("assets/app.js").is_file());
        assert!(!artifact.site().join(".env").exists());

        // Changed in the workspace and published again: the same artifact,
        // and the site before it kept.
        std::fs::write(work.join("assets/app.js"), "console.log(2)").unwrap();
        let second = publish(&root, &request).unwrap();
        assert!(!second.created);
        assert_eq!(second.artifact.id(), artifact.id());
        assert_eq!(second.artifact.manifest.version, 2);
        assert_eq!(list(&root).len(), 1);
        let live = std::fs::read_to_string(second.artifact.site().join("assets/app.js")).unwrap();
        assert_eq!(live, "console.log(2)");
        assert_eq!(versions(&second.artifact).len(), 1);

        // Back to the first, and the second is the one kept.
        let back = restore(&second.artifact, 1).unwrap();
        let live = std::fs::read_to_string(back.site().join("assets/app.js")).unwrap();
        assert_eq!(live, "console.log(1)");
        assert_eq!(versions(&back)[0].0, 2);
        // The next publish is the third, not a second "2".
        assert_eq!(
            publish(&root, &request).unwrap().artifact.manifest.version,
            3
        );
    }

    #[test]
    fn one_file_is_published_as_the_page() {
        let (work, root) = (scratch("file"), scratch("root"));
        std::fs::write(work.join("game.html"), "<h1>Snake</h1>").unwrap();
        let published = publish(
            &root,
            &Publish {
                source: work.join("game.html"),
                ..Publish::default()
            },
        )
        .unwrap();
        assert_eq!(published.artifact.manifest.title, "game");
        assert!(published.artifact.site().join("index.html").is_file());
        let refused = publish(
            &root,
            &Publish {
                source: work.join("notes.txt"),
                ..Publish::default()
            },
        );
        assert!(refused.is_err());
    }

    #[test]
    fn a_folder_without_an_index_needs_to_be_told_its_page() {
        let (work, root) = (scratch("entry"), scratch("root"));
        std::fs::create_dir_all(work.join("pages")).unwrap();
        std::fs::write(work.join("a.html"), "a").unwrap();
        std::fs::write(work.join("b.html"), "b").unwrap();
        std::fs::write(work.join("pages/c.html"), "c").unwrap();
        let mut request = Publish {
            source: work.clone(),
            ..Publish::default()
        };
        let err = publish(&root, &request).unwrap_err().to_string();
        assert!(err.contains("`entry`") && err.contains("a.html"), "{err}");
        assert!(
            list(&root).is_empty(),
            "nothing is left of a publish refused"
        );

        request.entry = Some("../outside.html".into());
        assert!(publish(&root, &request).is_err());
        request.entry = Some("pages/c.html".into());
        let published = publish(&root, &request).unwrap();
        assert_eq!(published.artifact.manifest.entry, "pages/c.html");
        // A folder named for what it builds says nothing: the project does.
        let dist = work.join("dist");
        page(&dist, "");
        let titled = publish(
            &root,
            &Publish {
                source: dist,
                ..Publish::default()
            },
        )
        .unwrap();
        assert_eq!(
            Some(titled.artifact.manifest.title.as_str()),
            work.file_name().and_then(|name| name.to_str())
        );
    }

    #[test]
    fn renaming_moves_the_folder_and_a_copied_folder_gets_its_own_id() {
        let (work, root) = (scratch("rename"), scratch("root"));
        page(&work, "One");
        let made = publish(
            &root,
            &Publish {
                source: work.clone(),
                ..Publish::default()
            },
        )
        .unwrap()
        .artifact;
        set_port(&made, 23_150).unwrap();
        let made = read(&made.folder).unwrap();
        let renamed = rename(&made, "Two: the sequel").unwrap();
        assert_eq!(renamed.folder, root.join("Two- the sequel"));
        assert!(!made.folder.exists());
        assert_eq!(
            read(&renamed.folder).unwrap().manifest.title,
            "Two: the sequel"
        );

        // Copied by hand: two folders say they are the same artifact.
        let copy = root.join("Copy");
        std::fs::create_dir_all(copy.join(SITE)).unwrap();
        std::fs::copy(renamed.folder.join(MANIFEST), copy.join(MANIFEST)).unwrap();
        let listed = list(&root);
        assert_eq!(listed.len(), 2);
        assert_ne!(listed[0].id(), listed[1].id());
        let ports: Vec<u16> = listed
            .iter()
            .map(|artifact| artifact.manifest.port)
            .collect();
        assert!(ports.contains(&23_150) && ports.contains(&0), "{ports:?}");
    }

    #[test]
    fn folder_names_are_what_file_systems_take() {
        assert_eq!(folder_name("  a/b: c?  "), "a-b- c-");
        assert_eq!(folder_name("..."), "Untitled artifact");
        assert_eq!(folder_name("con"), "con artifact");
        assert_eq!(new_id().len(), 8);
        assert_ne!(new_id(), new_id());
    }
}
