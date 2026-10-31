//! A presentation's space: its folder in `Documents/PilotHouse
//! Presentations`, holding the deck (`deck.json`), its pictures
//! (`media/`), what the user gives it to work from (`references/`), what is
//! exported (`exports/`), and earlier versions of the deck (`.versions/`).
//! The folder is the agent's working directory, so it can read the
//! references and keep files of its own there.
//!
//! The deck is saved as it changes (written to a temporary file, then
//! moved over the old one), and a version is kept every few minutes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};

use super::model::Deck;

pub const DECK: &str = "deck.json";
pub const MEDIA: &str = "media";
pub const REFERENCES: &str = "references";
pub const EXPORTS: &str = "exports";
const VERSIONS: &str = ".versions";
/// A picture of the first slide, for the list of presentations.
pub const THUMBNAIL: &str = ".thumbnail.png";
/// Versions kept, and how far apart.
const KEEP: usize = 20;
const EVERY: Duration = Duration::from_secs(5 * 60);
/// Pictures longer than this on a side are made smaller when added.
const LARGEST: u32 = 3840;
/// Where small copies of large pictures are kept (in `media`), for the
/// slides' small pictures to draw quickly, and their longest side.
pub const SMALL: &str = ".small";
const SMALL_SIDE: u32 = 960;

/// Where presentations live: `PILOTHOUSE_PRESENTATIONS` when set (tests,
/// scratch profiles), else `PilotHouse Presentations` in the Documents
/// folder (or the home folder without one).
pub fn root() -> PathBuf {
    if let Some(dir) = std::env::var_os("PILOTHOUSE_PRESENTATIONS").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("PilotHouse Presentations")
}

/// The folder of the user's own themes.
pub fn themes_dir() -> PathBuf {
    root().join("Themes")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Space {
    pub path: PathBuf,
}

/// A presentation in the list.
#[derive(Debug, Clone)]
pub struct Listed {
    pub space: Space,
    pub title: String,
    pub slides: usize,
    pub modified: SystemTime,
}

impl Space {
    /// A new space for `deck`, its folder named after the title (made
    /// unique), in `root`.
    pub fn create(root: &Path, deck: &Deck) -> Result<Space> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("could not make {}", root.display()))?;
        let base = folder_name(&deck.title);
        let mut path = root.join(&base);
        let mut n = 2;
        while path.exists() {
            path = root.join(format!("{base} {n}"));
            n += 1;
        }
        for dir in [MEDIA, REFERENCES, EXPORTS] {
            std::fs::create_dir_all(path.join(dir))?;
        }
        let space = Space { path };
        space.save(deck)?;
        Ok(space)
    }

    /// The space at `path` and its deck.
    pub fn open(path: &Path) -> Result<(Space, Deck)> {
        let file = path.join(DECK);
        let json = std::fs::read_to_string(&file)
            .with_context(|| format!("{} has no presentation", path.display()))?;
        let deck: Deck = serde_json::from_str(&json)
            .with_context(|| format!("{} could not be read", file.display()))?;
        for dir in [MEDIA, REFERENCES, EXPORTS] {
            let _ = std::fs::create_dir_all(path.join(dir));
        }
        Ok((
            Space {
                path: path.to_path_buf(),
            },
            deck,
        ))
    }

    pub fn media(&self) -> PathBuf {
        self.path.join(MEDIA)
    }

    pub fn exports(&self) -> PathBuf {
        self.path.join(EXPORTS)
    }

    /// Writes `deck` (and now and then keeps the one it replaces).
    pub fn save(&self, deck: &Deck) -> Result<()> {
        let file = self.path.join(DECK);
        let json = serde_json::to_vec_pretty(deck)?;
        if file.exists() && self.version_due() {
            let versions = self.path.join(VERSIONS);
            std::fs::create_dir_all(&versions)?;
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let _ = std::fs::copy(&file, versions.join(format!("{stamp}.json")));
            self.prune_versions();
        }
        // A name of its own per write: two saves at once (an autosave still
        // writing as the window closes) never share it.
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temporary = self
            .path
            .join(format!(".{DECK}.{}.{write}", std::process::id()));
        std::fs::write(&temporary, json)
            .with_context(|| format!("could not save in {}", self.path.display()))?;
        std::fs::rename(&temporary, &file).map_err(|err| {
            let _ = std::fs::remove_file(&temporary);
            anyhow::anyhow!("could not save {}: {err}", file.display())
        })
    }

    fn version_due(&self) -> bool {
        let latest = std::fs::read_dir(self.path.join(VERSIONS))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.metadata().ok()?.modified().ok())
            .max();
        latest.is_none_or(|at| at.elapsed().unwrap_or_default() >= EVERY)
    }

    fn prune_versions(&self) {
        let mut versions: Vec<PathBuf> = std::fs::read_dir(self.path.join(VERSIONS))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        versions.sort();
        while versions.len() > KEEP {
            let _ = std::fs::remove_file(versions.remove(0));
        }
    }

    /// The versions kept, newest first: path and when.
    pub fn versions(&self) -> Vec<(PathBuf, SystemTime)> {
        let mut versions: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(self.path.join(VERSIONS))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Some((entry.path(), entry.metadata().ok()?.modified().ok()?)))
            .collect();
        versions.sort_by_key(|version| std::cmp::Reverse(version.1));
        versions
    }

    /// Copies a picture (or SVG) into `media`, made smaller when very
    /// large: its name there.
    pub fn add_media(&self, source: &Path) -> Result<String> {
        let extension = source
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let known = [
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "svg",
        ];
        if !known.contains(&extension.as_str()) {
            bail!("{} is not a picture PilotHouse takes", source.display());
        }
        let bytes = std::fs::read(source)
            .with_context(|| format!("could not read {}", source.display()))?;
        self.add_media_bytes(&bytes, &extension)
    }

    /// A picture's bytes into `media`: its name there (the same picture
    /// added twice is kept once).
    pub fn add_media_bytes(&self, bytes: &[u8], extension: &str) -> Result<String> {
        let media = self.media();
        std::fs::create_dir_all(&media)?;
        let mut bytes = bytes.to_vec();
        let mut extension = extension.to_lowercase();
        if extension == "jpeg" {
            extension = "jpg".into();
        }
        // Huge photos would make every slide slow to draw and the PPTX big.
        if extension != "svg"
            && extension != "gif"
            && let Ok(image) = image::load_from_memory(&bytes)
            && image.width().max(image.height()) > LARGEST
        {
            let smaller = image.resize(LARGEST, LARGEST, image::imageops::FilterType::Lanczos3);
            let format = if extension == "jpg" {
                image::ImageFormat::Jpeg
            } else {
                extension = "png".into();
                image::ImageFormat::Png
            };
            let mut out = Vec::new();
            smaller.write_to(&mut std::io::Cursor::new(&mut out), format)?;
            bytes = out;
        }
        let name = format!("{:016x}.{extension}", fingerprint(&bytes));
        let path = media.join(&name);
        if !path.exists() {
            std::fs::write(&path, &bytes)?;
            // A small copy for thumbnails (a failure only makes them slower).
            if extension != "svg"
                && extension != "gif"
                && let Ok(image) = image::load_from_memory(&bytes)
                && image.width().max(image.height()) > SMALL_SIDE
            {
                let small = media.join(SMALL);
                let _ = std::fs::create_dir_all(&small);
                let _ = image
                    .resize(
                        SMALL_SIDE,
                        SMALL_SIDE,
                        image::imageops::FilterType::Triangle,
                    )
                    .save(small.join(&name));
            }
        }
        Ok(name)
    }

    /// Renames the space's folder for `title`, if no other has that name:
    /// where it is now.
    pub fn rename(&mut self, title: &str) -> Result<()> {
        let Some(parent) = self.path.parent() else {
            return Ok(());
        };
        let target = parent.join(folder_name(title));
        if target == self.path || target.exists() {
            return Ok(());
        }
        std::fs::rename(&self.path, &target)?;
        self.path = target;
        Ok(())
    }
}

/// A new space from a PowerPoint file: a deck PilotHouse wrote comes back
/// exactly; any other is read as well as it can be (what was left out is
/// said), and remembered, so its masters and layouts go back as they were.
pub fn import_pptx(root: &Path, pptx: &Path) -> Result<(Space, Vec<String>)> {
    if let Some(mut deck) = super::pptx::read_deck(pptx)? {
        if deck.title.trim().is_empty()
            && let Some(stem) = pptx.file_stem()
        {
            deck.title = stem.to_string_lossy().into_owned();
        }
        // Where it came from is this file, whatever the deck inside says
        // (a crafted one could name another file to save over, or another
        // deck's template to copy from).
        deck.source = Some(super::model::Source {
            file: String::new(),
            original: Some(pptx.to_path_buf()),
            theme: None,
            stamp: modified_secs(pptx),
        });
        let space = Space::create(root, &deck)?;
        let file = std::fs::File::open(pptx)?;
        let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))?;
        let media = space.media();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let Some(name) = entry.name()?.strip_prefix("ppt/media/").map(str::to_string) else {
                continue;
            };
            // The deck's own pictures only (not the PNGs made for SVG).
            if !safe_media_name(&name) || name.starts_with("pilothouse") {
                continue;
            }
            let mut bytes = Vec::new();
            // At most what a picture can be (the declared size is not
            // trusted).
            std::io::Read::read_to_end(
                &mut std::io::Read::take(&mut entry, 512 << 20),
                &mut bytes,
            )?;
            std::fs::write(media.join(&name), bytes)?;
        }
        return Ok((space, Vec::new()));
    }
    let files = super::pptx::unzip(pptx)?;
    let title = pptx
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Presentation".into());
    let placeholder = Deck {
        title: title.clone(),
        ..Deck::default()
    };
    let space = Space::create(root, &placeholder)?;
    let read = match super::pptx_read::read(&files, &space, &title) {
        Ok(read) => read,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&space.path);
            return Err(err);
        }
    };
    let mut deck = read.deck;
    std::fs::copy(pptx, space.path.join(SOURCE))?;
    deck.source = Some(super::model::Source {
        file: SOURCE.into(),
        original: Some(pptx.to_path_buf()),
        theme: Some(Box::new(deck.theme.clone())),
        stamp: modified_secs(pptx),
    });
    space.save(&deck)?;
    Ok((space, read.notes))
}

/// The copy of a PowerPoint file a deck was opened from.
pub const SOURCE: &str = "source.pptx";

/// When `path` was last changed, in seconds since 1970.
pub fn modified_secs(path: &Path) -> Option<u64> {
    let modified = path.metadata().ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    )
}

/// The presentation in `root` opened from PowerPoint file `pptx` and not
/// changed there since, if one.
pub fn find_imported(root: &Path, pptx: &Path) -> Option<Space> {
    let stamp = modified_secs(pptx)?;
    let wanted = pptx.canonicalize().unwrap_or_else(|_| pptx.to_path_buf());
    list(root).into_iter().find_map(|listed| {
        // Only the source is read (decks can be large).
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Head {
            source: Option<super::model::Source>,
        }
        let bytes = std::fs::read(listed.space.path.join(DECK)).ok()?;
        let head: Head = serde_json::from_slice(&bytes).ok()?;
        let source = head.source?;
        let original = source.original?;
        let same = original.canonicalize().unwrap_or(original) == wanted;
        (same && source.stamp == Some(stamp)).then_some(listed.space)
    })
}

/// Whether `path` is a PowerPoint file PilotHouse opens.
pub fn is_powerpoint(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| matches!(extension.as_str(), "pptx" | "potx"))
}

/// A theme the user saved, with the slide size its layouts were made for.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavedTheme {
    pub theme: super::model::Theme,
    pub size: super::model::SlideSize,
}

/// The user's saved themes in `dir`: name and file, by name.
pub fn saved_themes(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut themes: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| Some((path.file_stem()?.to_string_lossy().into_owned(), path)))
        .collect();
    themes.sort_by_key(|(name, _)| name.to_lowercase());
    themes
}

/// Saved theme `name` from `dir`, its layouts made for `size`.
pub fn load_theme(
    dir: &Path,
    name: &str,
    size: &super::model::SlideSize,
) -> Result<super::model::Theme> {
    let (_, path) = saved_themes(dir)
        .into_iter()
        .find(|(saved, _)| saved.eq_ignore_ascii_case(name))
        .with_context(|| format!("no saved theme `{name}`"))?;
    let saved: SavedTheme = serde_json::from_slice(&std::fs::read(&path)?)
        .with_context(|| format!("{} could not be read", path.display()))?;
    let mut theme = super::theme::resized(&saved.theme, &saved.size, size);
    theme.name = name.to_string();
    Ok(theme)
}

/// Saves `theme` (made for `size`) in `dir` as `name`, replacing one of
/// that name: its file.
pub fn save_theme(
    dir: &Path,
    name: &str,
    theme: &super::model::Theme,
    size: &super::model::SlideSize,
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = folder_name(name);
    let mut theme = theme.clone();
    theme.name = name.clone();
    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_vec_pretty(&SavedTheme {
        theme,
        size: size.clone(),
    })?;
    std::fs::write(&path, json).with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

/// Every presentation in `root`, most recently changed first.
pub fn list(root: &Path) -> Vec<Listed> {
    let mut listed: Vec<Listed> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join(DECK).is_file())
        .filter_map(|entry| {
            let path = entry.path();
            let file = path.join(DECK);
            let modified = file.metadata().ok()?.modified().ok()?;
            // Only what the list shows is read.
            #[derive(serde::Deserialize, Default)]
            #[serde(default)]
            struct Head {
                title: String,
                slides: Vec<serde::de::IgnoredAny>,
            }
            let head: Head = std::fs::read(&file)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            let title = if head.title.trim().is_empty() {
                path.file_name()?.to_string_lossy().into_owned()
            } else {
                head.title
            };
            Some(Listed {
                space: Space { path },
                title,
                slides: head.slides.len(),
                modified,
            })
        })
        .collect();
    listed.sort_by_key(|listed| std::cmp::Reverse(listed.modified));
    listed
}

/// A folder (or file) name from a title: what file systems take, never
/// empty.
pub fn folder_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    let cleaned: String = cleaned.chars().take(80).collect();
    // Cut short, it may end in a space or a dot again (Windows refuses
    // those), and Windows keeps a few names for devices.
    let cleaned = cleaned
        .trim_end()
        .trim_end_matches('.')
        .trim_end()
        .to_string();
    let reserved = {
        let stem = cleaned
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
    };
    if cleaned.is_empty() {
        "Untitled presentation".into()
    } else if reserved {
        format!("{cleaned} presentation")
    } else {
        cleaned
    }
}

/// Whether `name` is a plain media file name (letters, digits, `.`, `_`,
/// `-`; no folders, drives or hidden files), so joining it to the media
/// folder never leads out of it, on any system.
pub fn safe_media_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A fingerprint of `bytes` (FNV-1a), to name media by what they hold.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slides::model::SlideSize;
    use crate::slides::ops::new_deck;

    #[test]
    fn spaces_save_list_and_keep_media() {
        let root = std::env::temp_dir().join(format!("pilothouse-spaces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let deck = new_deck("Board: Q3 / plan", "clean", SlideSize::default());
        let space = Space::create(&root, &deck).unwrap();
        assert_eq!(space.path.file_name().unwrap(), "Board- Q3 - plan");
        let again = Space::create(&root, &deck).unwrap();
        assert!(again.path.ends_with("Board- Q3 - plan 2"));
        let (_, read) = Space::open(&space.path).unwrap();
        assert_eq!(read, deck);
        let listed = list(&root);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "Board: Q3 / plan");
        assert_eq!(listed[0].slides, 1);

        let picture = root.join("pic.png");
        image::RgbaImage::new(4, 4).save(&picture).unwrap();
        let name = space.add_media(&picture).unwrap();
        assert!(space.media().join(&name).is_file());
        assert_eq!(space.add_media(&picture).unwrap(), name, "kept once");
        assert!(space.add_media(&root.join("x.txt")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod import {
    use super::*;
    use crate::slides::model::SlideSize;
    use crate::slides::ops::{Op, apply, new_deck};

    #[test]
    fn a_pptx_made_here_comes_back() {
        let root = std::env::temp_dir().join(format!("pilothouse-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut deck = new_deck("Round trip", "harbor", SlideSize::default());
        let space = Space::create(&root, &deck).unwrap();
        let picture = root.join("pic.png");
        image::RgbaImage::from_pixel(8, 6, image::Rgba([10, 20, 30, 255]))
            .save(&picture)
            .unwrap();
        let name = space.add_media(&picture).unwrap();
        let op: Op = serde_json::from_value(serde_json::json!({
            "op": "add_slide", "layout": "blank", "elements": [
                {"type": "picture", "media": name, "frame": {"x": 10, "y": 10, "w": 80, "h": 60}},
                {"type": "icon", "icon": "rocket", "frame": {"x": 100, "y": 10, "w": 40, "h": 40}}
            ]
        }))
        .unwrap();
        apply(&mut deck, op).unwrap();
        let pptx = root.join("out.pptx");
        crate::slides::pptx::write(&deck, Some(&space.media()), &pptx).unwrap();
        let (back, _) = import_pptx(&root, &pptx).unwrap();
        let (_, mut read) = Space::open(&back.path).unwrap();
        // It remembers the file it came from (not what the deck inside says).
        let source = read.source.take().unwrap();
        assert_eq!(source.original.as_deref(), Some(pptx.as_path()));
        assert!(source.file.is_empty() && source.theme.is_none());
        assert_eq!(read, deck);
        assert!(back.media().join(&name).is_file(), "its picture came too");
        // Only the deck's own pictures (not those made for the icon).
        let files: Vec<_> = std::fs::read_dir(back.media())
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .collect();
        assert_eq!(files.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
