//! Writing a deck as PPTX (Office Open XML) that PowerPoint, Keynote and
//! Google Slides open as an editable presentation: the theme's colours and
//! fonts as a PowerPoint theme, its layouts as slide layouts (New Slide
//! offers them), and every element as PowerPoint's own kind (shapes and
//! text boxes with real text, pictures, tables, charts). SVG graphics and
//! icons go in as SVG with a PNG beside them for older readers.
//!
//! The deck itself rides along as `pilothouse/deck.json`, so a PPTX made
//! here opens again exactly as it was.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;

use super::charts::{escape, series_color};
use super::model::*;
use super::text::{self, Context};

/// English Metric Units a point.
const EMU: f32 = 12_700.;
/// Where the deck's own JSON goes, and the relationship that finds it.
pub const DECK_PART: &str = "pilothouse/deck.json";
/// Legacy comments have no state: a resolved one says so first.
pub(crate) const RESOLVED: &str = "[Resolved] ";

/// What the slides' XML was when written: changed since (in PowerPoint),
/// the deck part is stale and the slides are read instead.
const WRITTEN_PART: &str = "pilothouse/written.json";

/// A fingerprint of the presentation's own parts (FNV-1a over the
/// presentation and its slides, in name order).
fn slides_fingerprint<'a>(parts: impl Iterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut named: Vec<(&str, &[u8])> = parts
        .filter(|(name, _)| {
            *name == "ppt/presentation.xml"
                || (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        })
        .collect();
    named.sort_by_key(|(name, _)| *name);
    for (name, bytes) in named {
        for byte in name.as_bytes().iter().chain(bytes) {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}
pub const DECK_RELATIONSHIP: &str = "https://schemas.libreguild.dev/pilothouse/2026/deck";

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn emu(points: f32) -> i64 {
    (points * EMU).round() as i64
}

/// Writes `deck` (its pictures in `media`) to `out` as PPTX.
pub fn write(deck: &Deck, media: Option<&Path>, out: &Path) -> Result<()> {
    let parts = package(deck, media)?;
    // Written beside it, then moved over it: a file being replaced (the
    // PowerPoint file a deck was opened from) is never left half written.
    let name = out
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "presentation.pptx".into());
    let temporary = out.with_file_name(format!(".{name}.{}.part", std::process::id()));
    let written = (|| -> Result<()> {
        let file = std::fs::File::create(&temporary)
            .with_context(|| format!("could not write {}", out.display()))?;
        let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // Content types first, as readers expect.
        let mut ordered: Vec<(&String, &Vec<u8>)> = parts.iter().collect();
        ordered.sort_by_key(|(name, _)| name.as_str() != "[Content_Types].xml");
        for (name, bytes) in ordered {
            zip.start_file(name.as_str(), options)?;
            zip.write_all(bytes)?;
        }
        let file = zip.finish()?.into_inner().map_err(|err| err.into_error())?;
        file.sync_all()?;
        Ok(())
    })();
    match written.and_then(|()| {
        std::fs::rename(&temporary, out)
            .with_context(|| format!("could not write {}", out.display()))
    }) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&temporary);
            Err(err)
        }
    }
}

/// Every part of the package, by name.
pub fn package(deck: &Deck, media: Option<&Path>) -> Result<BTreeMap<String, Vec<u8>>> {
    // A deck opened from PowerPoint, its theme as read: the file's own
    // masters and layouts go back as they were.
    let template = match &deck.source {
        Some(source)
            if source.theme.as_deref() == Some(&deck.theme)
                && super::space::safe_media_name(&source.file) =>
        {
            media
                .and_then(Path::parent)
                .map(|space| space.join(&source.file))
                .filter(|file| file.is_file())
                .and_then(|file| read_template(&file).ok())
        }
        _ => None,
    };
    let mut writer = Writer {
        deck,
        media,
        parts: BTreeMap::new(),
        overrides: Vec::new(),
        defaults: Vec::new(),
        media_names: BTreeMap::new(),
        charts: 0,
        images: 0,
        template,
        ids: HashMap::new(),
        authors: Vec::new(),
    };
    writer.write_all()?;
    Ok(writer.parts)
}

struct Writer<'a> {
    deck: &'a Deck,
    media: Option<&'a Path>,
    parts: BTreeMap<String, Vec<u8>>,
    /// Content type overrides: part name and type.
    overrides: Vec<(String, String)>,
    /// Content types by extension, besides the usual.
    defaults: Vec<(String, String)>,
    /// Media already in the package: deck name to part name.
    media_names: BTreeMap<String, String>,
    charts: usize,
    images: usize,
    /// The PowerPoint file the deck came from, when written back as it was.
    template: Option<Template>,
    /// The slide being written: each element's id in its shape tree.
    ids: HashMap<String, u32>,
    /// Who commented, in order (their index is their id).
    authors: Vec<String>,
}

/// A PowerPoint file's masters, layouts and themes, to copy as they are.
struct Template {
    parts: BTreeMap<String, Vec<u8>>,
    overrides: Vec<(String, String)>,
    defaults: Vec<(String, String)>,
    /// Each master's id in the presentation and its target (from `ppt/`).
    masters: Vec<(u64, String)>,
    /// The first master's theme (from `ppt/`).
    theme: Option<String>,
    default_text_style: Option<String>,
}

/// A part's folder: `ppt/slides` for `ppt/slides/slide1.xml`.
fn folder_of(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// `target` (relative to `folder`) as a part name.
pub(crate) fn resolve_target(folder: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        folder.split('/').filter(|part| !part.is_empty()).collect()
    };
    for piece in target.trim_start_matches('/').split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// `part` as a target from a part in `folder`.
fn relative_target(folder: &str, part: &str) -> String {
    let from: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
    let to: Vec<&str> = part.split('/').collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = vec!["..".to_string(); from.len() - common];
    out.extend(to[common..].iter().map(|piece| piece.to_string()));
    out.join("/")
}

/// The relationships of `part` in `zip`: id, type, target part (or the
/// link), external.
pub(crate) fn rels_of(
    files: &BTreeMap<String, Vec<u8>>,
    part: &str,
) -> Vec<(String, String, String, bool)> {
    let name = match part.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    };
    let Some(bytes) = files.get(&name) else {
        return Vec::new();
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let Ok(document) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    document
        .descendants()
        .filter(|node| node.has_tag_name("Relationship"))
        .map(|node| {
            let target = node.attribute("Target").unwrap_or_default();
            let external = node.attribute("TargetMode") == Some("External");
            (
                node.attribute("Id").unwrap_or_default().to_string(),
                node.attribute("Type").unwrap_or_default().to_string(),
                if external {
                    target.to_string()
                } else {
                    resolve_target(folder_of(part), target)
                },
                external,
            )
        })
        .collect()
}

/// Every file of a zip, by name.
pub(crate) fn unzip(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("{} is not a PowerPoint file", path.display()))?;
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name()?.to_string();
        // A file that unpacks to more than 1 GB is not a presentation (it
        // is all held in memory); what is read is counted, not what the
        // archive says (that can lie).
        const MOST: usize = 1 << 30;
        let left = MOST.saturating_sub(total);
        let mut bytes = Vec::with_capacity(entry.size().min(256 << 20) as usize);
        std::io::Read::read_to_end(
            &mut std::io::Read::take(&mut entry, left as u64 + 1),
            &mut bytes,
        )?;
        total += bytes.len();
        if total > MOST {
            bail!("{} unpacks to more than 1 GB", path.display());
        }
        files.insert(name, bytes);
    }
    Ok(files)
}

/// A PowerPoint file's masters, with everything they use (layouts,
/// themes, pictures), and how the presentation names them.
fn read_template(path: &Path) -> Result<Template> {
    let files = unzip(path)?;
    let text = |name: &str| -> Option<String> {
        files
            .get(name)
            .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
    };
    let presentation = text("ppt/presentation.xml").context("no presentation in it")?;
    let document = roxmltree::Document::parse(&presentation)?;
    let rels = rels_of(&files, "ppt/presentation.xml");
    let target_of = |id: &str| {
        rels.iter()
            .find(|(rel, ..)| rel == id)
            .map(|(_, _, target, _)| target.clone())
    };
    let mut masters = Vec::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("sldMasterId"))
    {
        let id = node
            .attribute("id")
            .and_then(|id| id.parse::<u64>().ok())
            .unwrap_or(2_147_483_648);
        let rel = node
            .attributes()
            .find(|attribute| attribute.name() == "id" && attribute.namespace().is_some())
            .map(|attribute| attribute.value())
            .unwrap_or_default();
        if let Some(target) = target_of(rel) {
            masters.push((id, target));
        }
    }
    if masters.is_empty() {
        bail!("it has no slide master");
    }
    let default_text_style = document
        .descendants()
        .find(|node| node.has_tag_name("defaultTextStyle"))
        .map(|node| presentation[node.range()].to_string());
    // Everything the masters reach, but slides and notes.
    let mut parts = BTreeMap::new();
    let mut queue: Vec<String> = masters.iter().map(|(_, target)| target.clone()).collect();
    let mut theme = None;
    while let Some(part) = queue.pop() {
        if parts.contains_key(&part) || !files.contains_key(&part) {
            continue;
        }
        if part.starts_with("ppt/slides/")
            || part.starts_with("ppt/notesSlides/")
            || part.starts_with("ppt/notesMasters/")
        {
            continue;
        }
        parts.insert(part.clone(), files[&part].clone());
        let rels_name = match part.rsplit_once('/') {
            Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
            None => format!("_rels/{part}.rels"),
        };
        if let Some(bytes) = files.get(&rels_name) {
            parts.insert(rels_name.clone(), bytes.clone());
        }
        for (_, kind, target, external) in rels_of(&files, &part) {
            if external {
                continue;
            }
            if theme.is_none() && kind.ends_with("/theme") && part.starts_with("ppt/slideMasters/")
            {
                theme = Some(target.trim_start_matches("ppt/").to_string());
            }
            queue.push(target);
        }
    }
    let masters = masters
        .into_iter()
        .map(|(id, target)| (id, target.trim_start_matches("ppt/").to_string()))
        .collect();
    // Their content types, as the file gives them.
    let mut overrides = Vec::new();
    let mut defaults = Vec::new();
    if let Some(types) = text("[Content_Types].xml")
        && let Ok(types) = roxmltree::Document::parse(&types)
    {
        for node in types.descendants() {
            match node.tag_name().name() {
                "Override" => {
                    let part = node.attribute("PartName").unwrap_or_default();
                    if parts.contains_key(part.trim_start_matches('/')) {
                        overrides.push((
                            part.to_string(),
                            node.attribute("ContentType")
                                .unwrap_or_default()
                                .to_string(),
                        ));
                    }
                }
                "Default" => defaults.push((
                    node.attribute("Extension").unwrap_or_default().to_string(),
                    node.attribute("ContentType")
                        .unwrap_or_default()
                        .to_string(),
                )),
                _ => {}
            }
        }
    }
    Ok(Template {
        parts,
        overrides,
        defaults,
        masters,
        theme,
        default_text_style,
    })
}

/// A media type for a file kept in the package.
fn media_type(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "emf" => "image/x-emf",
        "wmf" => "image/x-wmf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "bin" => "application/vnd.openxmlformats-officedocument.oleObject",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "json" => "application/json",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}

/// A part's relationships as it is written.
#[derive(Default)]
struct Rels {
    list: Vec<(String, String, String, bool)>,
}

impl Rels {
    /// Adds one: its id.
    fn add(&mut self, kind: &str, target: &str) -> String {
        if let Some((id, ..)) = self
            .list
            .iter()
            .find(|(_, k, t, _)| k == kind && t == target)
        {
            return id.clone();
        }
        let id = format!("rId{}", self.list.len() + 1);
        self.list
            .push((id.clone(), kind.to_string(), target.to_string(), false));
        id
    }

    fn external(&mut self, kind: &str, target: &str) -> String {
        let id = format!("rId{}", self.list.len() + 1);
        self.list
            .push((id.clone(), kind.to_string(), target.to_string(), true));
        id
    }

    fn xml(&self) -> String {
        let mut out = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (id, kind, target, external) in &self.list {
            let _ = write!(
                out,
                r#"<Relationship Id="{id}" Type="{}" Target="{}"{}/>"#,
                kind_url(kind),
                escape(target),
                if *external {
                    r#" TargetMode="External""#
                } else {
                    ""
                }
            );
        }
        out.push_str("</Relationships>");
        out
    }
}

fn kind_url(kind: &str) -> String {
    if kind.starts_with("http") {
        kind.to_string()
    } else {
        format!("{REL}/{kind}")
    }
}

const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;

impl<'a> Writer<'a> {
    /// The deck, borrowed apart from the writer.
    fn deck_ref(&self) -> &'a Deck {
        self.deck
    }

    fn put(&mut self, name: &str, content: String) {
        self.parts.insert(name.to_string(), content.into_bytes());
    }

    /// A part's content type.
    fn content(&mut self, part: impl Into<String>, kind: impl Into<String>) {
        self.overrides.push((part.into(), kind.into()));
    }

    fn write_all(&mut self) -> Result<()> {
        let deck = self.deck;
        let has_notes = deck
            .slides
            .iter()
            .any(|slide| !slide.notes.trim().is_empty());
        let mut presentation = Rels::default();
        // The masters: the file's own (copied as they are), else ours.
        let masters: Vec<(u64, String)> = match &self.template {
            Some(template) => template.masters.clone(),
            None => vec![(2_147_483_648, "slideMasters/slideMaster1.xml".into())],
        };
        let master_ids: Vec<(u64, String)> = masters
            .iter()
            .map(|(id, target)| (*id, presentation.add("slideMaster", target)))
            .collect();
        let mut slide_rels = Vec::new();
        for index in 0..deck.slides.len() {
            slide_rels.push(presentation.add("slide", &format!("slides/slide{}.xml", index + 1)));
        }
        let notes_master =
            has_notes.then(|| presentation.add("notesMaster", "notesMasters/notesMaster1.xml"));
        let theme_target = self
            .template
            .as_ref()
            .and_then(|template| template.theme.clone())
            .unwrap_or_else(|| "theme/theme1.xml".into());
        presentation.add("theme", &theme_target);
        presentation.add("presProps", "presProps.xml");
        presentation.add("viewProps", "viewProps.xml");
        presentation.add("tableStyles", "tableStyles.xml");
        let commented = deck.slides.iter().any(|slide| !slide.comments.is_empty());
        if commented {
            presentation.add("commentAuthors", "commentAuthors.xml");
        }

        // The presentation.
        let mut xml = format!(r#"{XML}<p:presentation {NS} saveSubsetFonts="1">"#);
        xml.push_str("<p:sldMasterIdLst>");
        for (id, rel) in &master_ids {
            let _ = write!(xml, r#"<p:sldMasterId id="{id}" r:id="{rel}"/>"#);
        }
        xml.push_str("</p:sldMasterIdLst>");
        if let Some(notes) = &notes_master {
            let _ = write!(
                xml,
                r#"<p:notesMasterIdLst><p:notesMasterId r:id="{notes}"/></p:notesMasterIdLst>"#
            );
        }
        xml.push_str("<p:sldIdLst>");
        for (index, id) in slide_rels.iter().enumerate() {
            let _ = write!(xml, r#"<p:sldId id="{}" r:id="{id}"/>"#, 256 + index);
        }
        let _ = write!(
            xml,
            r#"</p:sldIdLst><p:sldSz cx="{}" cy="{}"/><p:notesSz cx="6858000" cy="9144000"/>"#,
            emu(deck.size.width).clamp(914_400, 51_206_400),
            emu(deck.size.height).clamp(914_400, 51_206_400)
        );
        let default_style = self
            .template
            .as_ref()
            .and_then(|template| template.default_text_style.clone())
            .unwrap_or_else(|| default_text_style(&deck.theme));
        xml.push_str(&default_style);
        xml.push_str(&sections(deck));
        xml.push_str("</p:presentation>");
        self.put("ppt/presentation.xml", xml);
        self.put("ppt/_rels/presentation.xml.rels", presentation.xml());
        self.content(
            "/ppt/presentation.xml",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
        );
        self.put(
            "ppt/presProps.xml",
            format!(r#"{XML}<p:presentationPr {NS}/>"#),
        );
        self.put(
            "ppt/viewProps.xml",
            format!(
                r#"{XML}<p:viewPr {NS}><p:normalViewPr><p:restoredLeft sz="15620"/><p:restoredTop sz="94660"/></p:normalViewPr><p:gridSpacing cx="76200" cy="76200"/></p:viewPr>"#
            ),
        );
        self.put(
            "ppt/tableStyles.xml",
            format!(
                r#"{XML}<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#
            ),
        );
        for (part, kind) in [
            (
                "/ppt/presProps.xml",
                "application/vnd.openxmlformats-officedocument.presentationml.presProps+xml",
            ),
            (
                "/ppt/viewProps.xml",
                "application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml",
            ),
            (
                "/ppt/tableStyles.xml",
                "application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml",
            ),
        ] {
            self.content(part, kind);
        }

        match self.template.take() {
            Some(template) => {
                // The file's masters, layouts and themes, byte for byte.
                for (name, bytes) in &template.parts {
                    self.parts.insert(name.clone(), bytes.clone());
                }
                for (part, kind) in &template.overrides {
                    self.content(part.clone(), kind.clone());
                }
                self.defaults.extend(template.defaults.iter().cloned());
                self.template = Some(template);
            }
            None => {
                self.put("ppt/theme/theme1.xml", theme_xml(&deck.theme, "PilotHouse"));
                self.content(
                    "/ppt/theme/theme1.xml",
                    "application/vnd.openxmlformats-officedocument.theme+xml",
                );
                self.write_master()?;
            }
        }
        for index in 0..deck.slides.len() {
            self.write_slide(index, has_notes)?;
        }
        if has_notes {
            self.write_notes_master();
        }
        if commented {
            self.write_comment_authors();
        }

        // The deck itself, for opening again here, and what its slides
        // were written as.
        let json = serde_json::to_string_pretty(deck)?;
        self.put(DECK_PART, json);
        let fingerprint = slides_fingerprint(
            self.parts
                .iter()
                .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
        );
        self.put(WRITTEN_PART, format!(r#"{{"slides": "{fingerprint}"}}"#));

        // Package relationships and properties.
        let mut root = Rels::default();
        root.add("officeDocument", "ppt/presentation.xml");
        root.add(
            "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties",
            "docProps/core.xml",
        );
        root.add("extended-properties", "docProps/app.xml");
        root.add(DECK_RELATIONSHIP, DECK_PART);
        self.put("_rels/.rels", root.xml());
        let now = time_now();
        self.put(
            "docProps/core.xml",
            format!(
                r#"{XML}<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>PilotHouse</dc:creator><dcterms:created xsi:type="dcterms:W3CDTF">{now}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{now}</dcterms:modified></cp:coreProperties>"#,
                escape(&deck.title)
            ),
        );
        self.put(
            "docProps/app.xml",
            format!(
                r#"{XML}<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Application>PilotHouse</Application><Slides>{}</Slides><PresentationFormat>Custom</PresentationFormat></Properties>"#,
                deck.slides.len()
            ),
        );
        self.content(
            "/docProps/core.xml",
            "application/vnd.openxmlformats-package.core-properties+xml",
        );
        self.content(
            "/docProps/app.xml",
            "application/vnd.openxmlformats-officedocument.extended-properties+xml",
        );
        // A type for every file's extension (pictures, objects kept).
        let mut defaults: BTreeMap<String, String> = [
            (
                "rels",
                "application/vnd.openxmlformats-package.relationships+xml",
            ),
            ("xml", "application/xml"),
            ("json", "application/json"),
        ]
        .into_iter()
        .map(|(ext, kind)| (ext.to_string(), kind.to_string()))
        .collect();
        for (ext, kind) in std::mem::take(&mut self.defaults) {
            defaults.entry(ext.to_lowercase()).or_insert(kind);
        }
        let overridden: std::collections::HashSet<String> = self
            .overrides
            .iter()
            .map(|(part, _)| part.clone())
            .collect();
        for name in self.parts.keys() {
            if overridden.contains(&format!("/{name}")) || name == "[Content_Types].xml" {
                continue;
            }
            if let Some((_, ext)) = name.rsplit_once('.') {
                let ext = ext.to_lowercase();
                if !defaults.contains_key(&ext) {
                    defaults.insert(ext.clone(), media_type(&ext).to_string());
                }
            }
        }
        let mut types = format!(
            r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#
        );
        for (ext, kind) in &defaults {
            let _ = write!(
                types,
                r#"<Default Extension="{}" ContentType="{}"/>"#,
                escape(ext),
                escape(kind)
            );
        }
        let mut seen = std::collections::HashSet::new();
        for (part, kind) in &self.overrides {
            if seen.insert(part.clone()) {
                let _ = write!(
                    types,
                    r#"<Override PartName="{}" ContentType="{}"/>"#,
                    escape(part),
                    escape(kind)
                );
            }
        }
        types.push_str("</Types>");
        self.put("[Content_Types].xml", types);
        Ok(())
    }

    fn write_master(&mut self) -> Result<()> {
        let deck = self.deck;
        let theme = &deck.theme;
        let mut rels = Rels::default();
        let mut layout_ids = String::new();
        for index in 0..theme.layouts.len() {
            let id = rels.add(
                "slideLayout",
                &format!("../slideLayouts/slideLayout{}.xml", index + 1),
            );
            let _ = write!(
                layout_ids,
                r#"<p:sldLayoutId id="{}" r:id="{id}"/>"#,
                2_147_483_649u64 + index as u64
            );
        }
        rels.add("theme", "../theme/theme1.xml");
        let mut tree = Tree::default();
        let decorations = self.elements(&theme.decorations, &mut rels, &mut tree, None)?;
        let background = background_xml(&theme.background, theme, self, &mut rels)?;
        let xml = format!(
            r#"{XML}<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr>{background}<a:effectLst/></p:bgPr></p:bg><p:spTree>{}{decorations}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst>{layout_ids}</p:sldLayoutIdLst><p:txStyles>{}</p:txStyles></p:sldMaster>"#,
            GROUP_HEADER,
            text_styles(theme),
        );
        self.put("ppt/slideMasters/slideMaster1.xml", xml);
        self.put("ppt/slideMasters/_rels/slideMaster1.xml.rels", rels.xml());
        self.content(
            "/ppt/slideMasters/slideMaster1.xml",
            "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml",
        );
        for (index, layout) in theme.layouts.iter().enumerate() {
            self.write_layout(index, layout)?;
        }
        Ok(())
    }

    fn write_layout(&mut self, index: usize, layout: &Layout) -> Result<()> {
        let theme = &self.deck_ref().theme;
        let mut rels = Rels::default();
        rels.add("slideMaster", "../slideMasters/slideMaster1.xml");
        let mut tree = Tree::default();
        let decorations = self.elements(&layout.decorations, &mut rels, &mut tree, None)?;
        let mut places = String::new();
        for place in &layout.placeholders {
            let id = tree.next();
            let (kind, idx) = placeholder_attrs(&layout.id, place);
            let base = if place.kind.is_title() {
                &theme.title
            } else {
                &theme.body
            };
            let size = place.size.unwrap_or(base.size);
            let color = place.color.as_ref().unwrap_or(&base.color);
            let bold = place.bold.unwrap_or(base.bold);
            let align = place.align.unwrap_or(base.align);
            let bullet = if place.bullets { "" } else { "<a:buNone/>" };
            let level = format!(
                r#"<a:lvl1pPr algn="{}">{bullet}<a:defRPr sz="{}" b="{}">{}</a:defRPr></a:lvl1pPr>"#,
                align_attr(align),
                (size * 100.).round() as i64,
                bold as u8,
                solid(color, theme, 1.)
            );
            let _ = write!(
                places,
                r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{} {id}"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph{kind}{idx}/></p:nvPr></p:nvSpPr><p:spPr>{}</p:spPr><p:txBody><a:bodyPr anchor="{}">{}</a:bodyPr><a:lstStyle>{level}</a:lstStyle><a:p><a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>"#,
                escape(place.kind.label()),
                xfrm(&place.frame),
                anchor_attr(place.anchor),
                if place.autofit == Autofit::Shrink {
                    "<a:normAutofit/>"
                } else {
                    ""
                },
                escape(&place.prompt)
            );
        }
        let background = match &layout.background {
            Some(fill) => format!(
                "<p:bg><p:bgPr>{}<a:effectLst/></p:bgPr></p:bg>",
                background_xml(fill, theme, self, &mut rels)?
            ),
            None => String::new(),
        };
        let xml = format!(
            r#"{XML}<p:sldLayout {NS} type="{}" preserve="1"><p:cSld name="{}">{background}<p:spTree>{GROUP_HEADER}{decorations}{places}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#,
            layout_type(&layout.id),
            escape(&layout.name)
        );
        let name = format!("ppt/slideLayouts/slideLayout{}.xml", index + 1);
        self.put(&name, xml);
        self.put(
            &format!("ppt/slideLayouts/_rels/slideLayout{}.xml.rels", index + 1),
            rels.xml(),
        );
        self.content(
            format!("/{name}"),
            "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml",
        );
        Ok(())
    }

    fn write_slide(&mut self, index: usize, has_notes: bool) -> Result<()> {
        let deck = self.deck;
        let slide = &deck.slides[index];
        let mut rels = Rels::default();
        let layout_index = deck
            .theme
            .layouts
            .iter()
            .position(|layout| layout.id == slide.layout)
            .unwrap_or(deck.theme.layouts.len().saturating_sub(1));
        let layout_target = match (&self.template, deck.theme.layouts.get(layout_index)) {
            (Some(_), Some(layout)) => layout
                .part
                .clone()
                .or_else(|| {
                    deck.theme
                        .layouts
                        .iter()
                        .find_map(|layout| layout.part.clone())
                })
                .map(|part| relative_target("ppt/slides", &part)),
            _ => None,
        }
        .unwrap_or_else(|| format!("../slideLayouts/slideLayout{}.xml", layout_index + 1));
        rels.add("slideLayout", &layout_target);
        // Each element's id first, so connectors and animations can name
        // elements written after them.
        self.ids = tree_ids(&slide.elements);
        let mut tree = Tree::default();
        let elements = self.elements(&slide.elements, &mut rels, &mut tree, Some(&slide.layout))?;
        let background = match &slide.background {
            Some(fill) => format!(
                "<p:bg><p:bgPr>{}<a:effectLst/></p:bgPr></p:bg>",
                background_xml(fill, &deck.theme, self, &mut rels)?
            ),
            None => String::new(),
        };
        let transition = transition_xml(slide);
        let timing = timing_xml(slide, &self.ids);
        if has_notes {
            rels.add(
                "notesSlide",
                &format!("../notesSlides/notesSlide{}.xml", index + 1),
            );
            self.write_notes(index);
        }
        if !slide.comments.is_empty() {
            rels.add("comments", &format!("../comments/comment{}.xml", index + 1));
            self.write_comments(index);
        }
        let xml = format!(
            r#"{XML}<p:sld {NS}{}><p:cSld>{background}<p:spTree>{GROUP_HEADER}{elements}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>{transition}{timing}</p:sld>"#,
            if slide.hidden { r#" show="0""# } else { "" }
        );
        let name = format!("ppt/slides/slide{}.xml", index + 1);
        self.put(&name, xml);
        self.put(
            &format!("ppt/slides/_rels/slide{}.xml.rels", index + 1),
            rels.xml(),
        );
        self.content(
            format!("/{name}"),
            "application/vnd.openxmlformats-officedocument.presentationml.slide+xml",
        );
        Ok(())
    }

    /// The slide's comments (PowerPoint's comment parts).
    fn write_comments(&mut self, index: usize) {
        let slide = &self.deck.slides[index];
        let mut xml = format!(r#"{XML}<p:cmLst {NS}>"#);
        // Each author's comments are numbered from 1 (PowerPoint's idx).
        let mut counts: HashMap<usize, usize> = HashMap::new();
        for comment in &slide.comments {
            // On the element it is about, where that is now.
            let (x, y) = comment
                .element
                .as_ref()
                .and_then(|id| slide.elements.iter().find(|element| &element.id == id))
                .map(|element| {
                    let [_, top, right, _] = super::editor::geometry::bounds(&element.frame);
                    (right, top)
                })
                .unwrap_or((comment.x, comment.y));
            // Replies name the comment they answer (PowerPoint 2013's
            // threads in legacy comments).
            let mut write = |author: &str,
                             text: &str,
                             at: u64,
                             parent: Option<(usize, usize)>,
                             this: &mut Self| {
                let author = this.author(author);
                let idx = {
                    let count = counts.entry(author).or_insert(0);
                    *count += 1;
                    *count
                };
                let thread = parent
                    .map(|(parent_author, parent_idx)| {
                        format!(
                            r#"<p:extLst><p:ext uri="{{C676402C-5697-4E1C-873F-D02D1690AC5C}}"><p15:threadingInfo xmlns:p15="http://schemas.microsoft.com/office/powerpoint/2012/main" timeZoneBias="0"><p15:parentCm authorId="{parent_author}" idx="{parent_idx}"/></p15:threadingInfo></p:ext></p:extLst>"#
                        )
                    })
                    .unwrap_or_default();
                let _ = write!(
                    xml,
                    r#"<p:cm authorId="{author}" dt="{}" idx="{idx}"><p:pos x="{}" y="{}"/><p:text>{}</p:text>{thread}</p:cm>"#,
                    w3c_time(at),
                    (x * 8.).round().max(0.) as i64,
                    (y * 8.).round().max(0.) as i64,
                    escape(text)
                );
                (author, idx)
            };
            let text = if comment.resolved {
                format!("{RESOLVED}{}", comment.text)
            } else {
                comment.text.clone()
            };
            let parent = write(&comment.author, &text, comment.at, None, self);
            for reply in &comment.replies {
                write(&reply.author, &reply.text, reply.at, Some(parent), self);
            }
        }
        xml.push_str("</p:cmLst>");
        let name = format!("ppt/comments/comment{}.xml", index + 1);
        self.put(&name, xml);
        self.content(
            format!("/{name}"),
            "application/vnd.openxmlformats-officedocument.presentationml.comments+xml",
        );
    }

    /// An author's id (added the first time).
    fn author(&mut self, name: &str) -> usize {
        match self.authors.iter().position(|known| known == name) {
            Some(ix) => ix,
            None => {
                self.authors.push(name.to_string());
                self.authors.len() - 1
            }
        }
    }

    fn write_comment_authors(&mut self) {
        let mut xml = format!(r#"{XML}<p:cmAuthorLst {NS}>"#);
        for (id, name) in self.authors.iter().enumerate() {
            let initials: String = name
                .split_whitespace()
                .filter_map(|word| word.chars().next())
                .take(3)
                .collect();
            let _ = write!(
                xml,
                r#"<p:cmAuthor id="{id}" name="{}" initials="{}" lastIdx="1000" clrIdx="{}"/>"#,
                escape(name),
                escape(&initials),
                id % 8
            );
        }
        xml.push_str("</p:cmAuthorLst>");
        self.put("ppt/commentAuthors.xml", xml);
        self.content(
            "/ppt/commentAuthors.xml",
            "application/vnd.openxmlformats-officedocument.presentationml.commentAuthors+xml",
        );
    }

    fn write_notes(&mut self, index: usize) {
        let notes = &self.deck.slides[index].notes;
        let paragraphs: String = notes
            .split('\n')
            .map(|line| {
                format!(
                    r#"<a:p><a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p>"#,
                    escape(line)
                )
            })
            .collect();
        let xml = format!(
            r#"{XML}<p:notes {NS}><p:cSld><p:spTree>{GROUP_HEADER}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide image"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#
        );
        let name = format!("ppt/notesSlides/notesSlide{}.xml", index + 1);
        self.put(&name, xml);
        let mut rels = Rels::default();
        rels.add("notesMaster", "../notesMasters/notesMaster1.xml");
        rels.add("slide", &format!("../slides/slide{}.xml", index + 1));
        self.put(
            &format!("ppt/notesSlides/_rels/notesSlide{}.xml.rels", index + 1),
            rels.xml(),
        );
        self.content(
            format!("/{name}"),
            "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml",
        );
    }

    fn write_notes_master(&mut self) {
        let xml = format!(
            r#"{XML}<p:notesMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{GROUP_HEADER}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide image"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg" idx="2"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="381000" y="685800"/><a:ext cx="6096000" cy="3429000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" sz="quarter" idx="3"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="685800" y="4400550"/><a:ext cx="5486400" cy="3600450"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:notesStyle><a:lvl1pPr marL="0" algn="l"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:notesStyle></p:notesMaster>"#
        );
        self.put("ppt/notesMasters/notesMaster1.xml", xml);
        let mut rels = Rels::default();
        rels.add("theme", "../theme/themeNotesPilotHouse.xml");
        self.put("ppt/notesMasters/_rels/notesMaster1.xml.rels", rels.xml());
        self.put(
            "ppt/theme/themeNotesPilotHouse.xml",
            theme_xml(&self.deck.theme, "Notes"),
        );
        self.content(
            "/ppt/notesMasters/notesMaster1.xml",
            "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml",
        );
        self.content(
            "/ppt/theme/themeNotesPilotHouse.xml",
            "application/vnd.openxmlformats-officedocument.theme+xml",
        );
    }

    /// A media file into the package (once): its target from a slide.
    fn add_media(&mut self, name: &str) -> Option<String> {
        if let Some(part) = self.media_names.get(name) {
            return Some(format!("../media/{part}"));
        }
        if !super::space::safe_media_name(name) {
            return None;
        }
        let bytes = std::fs::read(self.media?.join(name)).ok()?;
        // Its own name, so the deck's JSON finds it again on opening.
        self.parts.insert(format!("ppt/media/{name}"), bytes);
        self.media_names.insert(name.to_string(), name.to_string());
        Some(format!("../media/{name}"))
    }

    /// Bytes made here (a PNG of an SVG) into the package: their target.
    fn add_bytes(&mut self, extension: &str, bytes: Vec<u8>) -> String {
        self.images += 1;
        let name = format!("pilothouse{}.{extension}", self.images);
        self.parts.insert(format!("ppt/media/{name}"), bytes);
        format!("../media/{name}")
    }

    /// The XML of `elements` (filling `layout`'s placeholders when given).
    fn elements(
        &mut self,
        elements: &[Element],
        rels: &mut Rels,
        tree: &mut Tree,
        layout: Option<&str>,
    ) -> Result<String> {
        let mut out = String::new();
        for element in elements {
            if element.placeholder.is_some() && super::ops::is_empty(element) {
                continue;
            }
            out.push_str(&self.element(element, rels, tree, layout)?);
        }
        Ok(out)
    }

    fn element(
        &mut self,
        element: &Element,
        rels: &mut Rels,
        tree: &mut Tree,
        layout: Option<&str>,
    ) -> Result<String> {
        let theme = &self.deck_ref().theme;
        let id = tree.next();
        let name = escape(if element.name.is_empty() {
            element.kind.label()
        } else {
            &element.name
        });
        let opacity = element.opacity.clamp(0., 1.);
        let place = element.placeholder.and_then(|kind| {
            let layout = theme.layout(layout?)?;
            layout
                .placeholder(kind)
                .map(|place| (layout.id.as_str(), place))
        });
        let ph = place
            .map(|(layout, place)| {
                let (kind, idx) = placeholder_attrs(layout, place);
                format!("<p:ph{kind}{idx}/>")
            })
            .unwrap_or_default();
        Ok(match &element.kind {
            Kind::Shape(shape) if shape.geometry == Geometry::Line && shape.connect.is_some() => {
                // A connector: PowerPoint keeps its ends on their shapes.
                let line = line_xml(&shape.line, theme, opacity, true, &shape.fill);
                let connect = shape.connect.as_ref().expect("a connector");
                let end = |tag: &str, end: &Option<(String, u8)>| {
                    end.as_ref()
                        .and_then(|(target, side)| {
                            let id = self.ids.get(target)?;
                            // PowerPoint numbers a box's sites anticlockwise from the top.
                            let site = [0, 3, 2, 1][(*side as usize) % 4];
                            Some(format!(r#"<a:{tag} id="{id}" idx="{site}"/>"#))
                        })
                        .unwrap_or_default()
                };
                format!(
                    r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvCxnSpPr>{}{}</p:cNvCxnSpPr><p:nvPr/></p:nvCxnSpPr><p:spPr>{}<a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom><a:noFill/>{line}</p:spPr></p:cxnSp>"#,
                    end("stCxn", &connect.start),
                    end("endCxn", &connect.end),
                    xfrm(&element.frame)
                )
            }
            Kind::Raw(raw) => self.raw_xml(id, raw, rels),
            Kind::Shape(shape) => {
                let geometry = geometry_xml(shape, element.frame.w, element.frame.h);
                let fill = if shape.geometry == Geometry::Line {
                    "<a:noFill/>".to_string()
                } else {
                    self.fill_xml(&shape.fill, rels, opacity)?
                };
                let line = line_xml(
                    &shape.line,
                    theme,
                    opacity,
                    shape.geometry == Geometry::Line,
                    &shape.fill,
                );
                let effects = shadow_xml(shape.shadow.as_ref(), theme);
                let body = match &shape.text {
                    Some(body) => {
                        let on = text_on(&shape.fill, theme);
                        self.text_body(body, None, on, element, rels)
                    }
                    None => String::new(),
                };
                format!(
                    r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{}{geometry}{fill}{line}{effects}</p:spPr>{body}</p:sp>"#,
                    xfrm(&element.frame)
                )
            }
            Kind::Text(text_box) => {
                let fill = self.fill_xml(&text_box.fill, rels, opacity)?;
                let line = line_xml(&text_box.line, theme, opacity, false, &text_box.fill);
                let on = text_on(&text_box.fill, theme);
                let body = self.text_body(
                    &text_box.text,
                    place.map(|(_, place)| place),
                    on,
                    element,
                    rels,
                );
                format!(
                    r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr txBox="{}"/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{fill}{line}</p:spPr>{body}</p:sp>"#,
                    (element.placeholder.is_none()) as u8,
                    xfrm(&element.frame)
                )
            }
            Kind::Picture(picture) => {
                let Some(target) = self.add_media(&picture.media) else {
                    return Ok(String::new());
                };
                let blip = rels.add("image", &target);
                self.picture_xml(id, &name, &ph, element, picture, &blip, None, opacity)
            }
            Kind::Icon(icon) => {
                let Some(source) = super::icons::svg(&icon.icon) else {
                    return Ok(String::new());
                };
                let color = theme.resolve(&icon.color);
                let svg = String::from_utf8_lossy(&source)
                    .replace("currentColor", &format!("#{}", color.hex()))
                    .replace(
                        r#"stroke-width="2""#,
                        &format!(r#"stroke-width="{}""#, icon.stroke.clamp(0.25, 6.)),
                    );
                self.svg_picture(id, &name, element, &svg, rels, opacity)?
            }
            Kind::Svg(art) => self.svg_picture(id, &name, element, &art.source, rels, opacity)?,
            Kind::Table(table) => self.table_xml(id, &name, element, table, rels),
            Kind::Chart(chart) => {
                self.charts += 1;
                let number = self.charts;
                let part = format!("ppt/charts/chart{number}.xml");
                // Its data as a workbook, so PowerPoint's Edit Data opens it.
                let book = format!("ppt/embeddings/Microsoft_Excel_Worksheet{number}.xlsx");
                self.parts
                    .insert(book.clone(), super::xlsx::chart_workbook(chart)?);
                let mut chart_rels = Rels::default();
                let data = chart_rels.add(
                    "package",
                    &format!("../embeddings/Microsoft_Excel_Worksheet{number}.xlsx"),
                );
                self.put(
                    &format!("ppt/charts/_rels/chart{number}.xml.rels"),
                    chart_rels.xml(),
                );
                self.put(&part, chart_xml(chart, theme, &data));
                self.content(
                    format!("/{part}"),
                    "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
                );
                let rel = rels.add("chart", &format!("../charts/chart{number}.xml"));
                format!(
                    r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="{name}"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="{rel}"/></a:graphicData></a:graphic></p:graphicFrame>"#,
                    emu(element.frame.x),
                    emu(element.frame.y),
                    emu(element.frame.w.max(1.)),
                    emu(element.frame.h.max(1.))
                )
            }
            Kind::Code(code) => self.code_xml(id, &name, element, code),
            Kind::Group(group) => {
                let bounds = element.frame;
                let mut children = String::new();
                for child in &group.children {
                    children.push_str(&self.element(child, rels, tree, None)?);
                }
                let off = format!(
                    r#"<a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/>"#,
                    emu(bounds.x),
                    emu(bounds.y),
                    emu(bounds.w.max(1.)),
                    emu(bounds.h.max(1.))
                );
                format!(
                    r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm{}>{off}{}</a:xfrm></p:grpSpPr>{children}</p:grpSp>"#,
                    rotation_attr(bounds.rotation),
                    off.replace("a:off", "a:chOff").replace("a:ext", "a:chExt")
                )
            }
        })
    }

    /// An object kept from a PowerPoint file: its XML with its files put
    /// back and its relationships and id renumbered for this slide.
    fn raw_xml(&mut self, id: u32, raw: &Raw, rels: &mut Rels) -> String {
        let mut xml = raw.xml.clone();
        // Through markers, so one renumbering never meets another.
        let mut renamed = Vec::new();
        for (ix, rel) in raw.rels.iter().enumerate() {
            let new = if rel.external {
                rels.external(&rel.kind, &rel.target)
            } else {
                match self.add_media(&rel.target) {
                    Some(target) => rels.add(&rel.kind, &target),
                    None => continue,
                }
            };
            let marker = format!("\u{1}{ix}\u{1}");
            xml = xml.replace(&format!("\"{}\"", rel.id), &format!("\"{marker}\""));
            renamed.push((marker, new));
        }
        for (marker, new) in renamed {
            xml = xml.replace(&marker, &new);
        }
        // Its own id among the slide's shapes.
        if let Some(at) = xml.find("cNvPr")
            && let Some(start) = xml[at..].find(" id=\"").map(|offset| at + offset + 5)
            && let Some(end) = xml[start..].find('"').map(|offset| start + offset)
        {
            xml.replace_range(start..end, &id.to_string());
        }
        xml
    }

    #[allow(clippy::too_many_arguments)]
    fn picture_xml(
        &mut self,
        id: u32,
        name: &str,
        ph: &str,
        element: &Element,
        picture: &Picture,
        blip: &str,
        svg: Option<&str>,
        opacity: f32,
    ) -> String {
        let theme = &self.deck_ref().theme;
        let mut frame = element.frame;
        let mut crop = picture.crop;
        // PowerPoint stretches the (cropped) picture to its frame: cover
        // crops more, contain makes the frame the picture's shape.
        if let Some((iw, ih)) = self.picture_size(&picture.media) {
            let (vw, vh) = (
                iw * (1. - crop[0] - crop[2]).max(0.01),
                ih * (1. - crop[1] - crop[3]).max(0.01),
            );
            let source = vw / vh;
            let target = frame.w / frame.h.max(0.01);
            match picture.fit {
                Fit::Cover if (source - target).abs() > 0.001 => {
                    if source > target {
                        let keep = target / source;
                        let cut = (1. - crop[0] - crop[2]) * (1. - keep) / 2.;
                        crop[0] += cut;
                        crop[2] += cut;
                    } else {
                        let keep = source / target;
                        let cut = (1. - crop[1] - crop[3]) * (1. - keep) / 2.;
                        crop[1] += cut;
                        crop[3] += cut;
                    }
                }
                Fit::Contain if (source - target).abs() > 0.001 => {
                    if source > target {
                        let h = frame.w / source;
                        frame.y += (frame.h - h) / 2.;
                        frame.h = h;
                    } else {
                        let w = frame.h * source;
                        frame.x += (frame.w - w) / 2.;
                        frame.w = w;
                    }
                }
                _ => {}
            }
        }
        let src_rect = if crop.iter().any(|side| *side > 0.0001) {
            format!(
                r#"<a:srcRect l="{}" t="{}" r="{}" b="{}"/>"#,
                (crop[0] * 100_000.).round() as i64,
                (crop[1] * 100_000.).round() as i64,
                (crop[2] * 100_000.).round() as i64,
                (crop[3] * 100_000.).round() as i64
            )
        } else {
            String::new()
        };
        let alpha = if opacity < 0.999 {
            format!(
                r#"<a:alphaModFix amt="{}"/>"#,
                (opacity * 100_000.).round() as i64
            )
        } else {
            String::new()
        };
        let svg_ext = svg
            .map(|svg| {
                format!(
                    r#"<a:extLst><a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}"><asvg:svgBlip xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main" r:embed="{svg}"/></a:ext></a:extLst>"#
                )
            })
            .unwrap_or_default();
        let geometry = if picture.radius > 0. {
            let adj = (picture.radius / frame.w.min(frame.h).max(0.01) * 100_000.).min(50_000.);
            format!(
                r#"<a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val {}"/></a:avLst></a:prstGeom>"#,
                adj.round() as i64
            )
        } else {
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#.into()
        };
        let line = line_xml(&picture.line, theme, 1., false, &Fill::None);
        let effects = shadow_xml(picture.shadow.as_ref(), theme);
        format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="{name}" descr="{}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr>{ph}</p:nvPr></p:nvPicPr><p:blipFill><a:blip r:embed="{blip}">{alpha}{svg_ext}</a:blip>{src_rect}<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}{geometry}{line}{effects}</p:spPr></p:pic>"#,
            escape(&picture.alt),
            xfrm(&frame)
        )
    }

    fn picture_size(&self, name: &str) -> Option<(f32, f32)> {
        let path = self.media?.join(name);
        image::image_dimensions(path)
            .ok()
            .map(|(w, h)| (w.max(1) as f32, h.max(1) as f32))
    }

    /// An SVG graphic as a picture: the SVG, and a PNG of it for readers
    /// that do not take SVG.
    fn svg_picture(
        &mut self,
        id: u32,
        name: &str,
        element: &Element,
        source: &str,
        rels: &mut Rels,
        opacity: f32,
    ) -> Result<String> {
        if source.trim().is_empty() {
            return Ok(String::new());
        }
        let frame = element.frame;
        let scale = 2.;
        let width = (frame.w * scale).round().clamp(1., 4096.) as u32;
        let wrapper = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><image href="data:image/svg+xml;base64,{}" width="{w}" height="{h}" preserveAspectRatio="xMidYMid meet"/></svg>"#,
            base64::engine::general_purpose::STANDARD.encode(source.as_bytes()),
            w = super::shapes::n(frame.w.max(1.)),
            h = super::shapes::n(frame.h.max(1.)),
        );
        let png = super::raster::rasterize(&wrapper, width)
            .ok()
            .and_then(|image| {
                let mut bytes = Vec::new();
                image
                    .write_to(
                        &mut std::io::Cursor::new(&mut bytes),
                        image::ImageFormat::Png,
                    )
                    .ok()?;
                Some(bytes)
            })
            .context("an SVG graphic could not be drawn")?;
        let png_target = self.add_bytes("png", png);
        let svg_target = self.add_bytes("svg", wrapper.into_bytes());
        let png_rel = rels.add("image", &png_target);
        let svg_rel = rels.add("image", &svg_target);
        let picture = Picture::default();
        Ok(self.picture_xml(
            id,
            name,
            "",
            element,
            &picture,
            &png_rel,
            Some(&svg_rel),
            opacity,
        ))
    }

    fn fill_xml(&mut self, fill: &Fill, rels: &mut Rels, opacity: f32) -> Result<String> {
        let theme = &self.deck_ref().theme;
        Ok(match fill {
            Fill::None => "<a:noFill/>".into(),
            Fill::Solid {
                color,
                opacity: alpha,
            } => solid(color, theme, alpha * opacity),
            Fill::Gradient {
                stops,
                angle,
                radial,
            } => {
                let mut list = String::new();
                for stop in stops {
                    let _ = write!(
                        list,
                        r#"<a:gs pos="{}">{}</a:gs>"#,
                        (stop.at.clamp(0., 1.) * 100_000.).round() as i64,
                        color_xml(&stop.color, theme, opacity)
                    );
                }
                let shade = if *radial {
                    r#"<a:path path="circle"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path>"#.to_string()
                } else {
                    format!(
                        r#"<a:lin ang="{}" scaled="0"/>"#,
                        ((angle.rem_euclid(360.)) * 60_000.).round() as i64
                    )
                };
                format!(
                    r#"<a:gradFill rotWithShape="1"><a:gsLst>{list}</a:gsLst>{shade}</a:gradFill>"#
                )
            }
            Fill::Picture { media, .. } => match self.add_media(media) {
                Some(target) => {
                    let blip = rels.add("image", &target);
                    format!(
                        r#"<a:blipFill rotWithShape="1"><a:blip r:embed="{blip}"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>"#
                    )
                }
                None => "<a:noFill/>".into(),
            },
        })
    }

    /// A text body: real text, styled as drawn here.
    fn text_body(
        &mut self,
        body: &TextBody,
        place: Option<&Placeholder>,
        on: Option<Rgba>,
        element: &Element,
        rels: &mut Rels,
    ) -> String {
        let theme = &self.deck_ref().theme;
        let paras = text::paragraphs(
            body,
            &Context {
                theme,
                place,
                color: on,
                size: None,
                bold: None,
            },
        );
        let [left, top, right, bottom] = body.insets;
        // The shrink worked out here, so PowerPoint shows the same.
        let autofit = match body.autofit {
            Autofit::Shrink => {
                let laid = text::lay_out(
                    &paras,
                    (element.frame.w - left - right).max(1.),
                    body.wrap,
                    Some((element.frame.h - top - bottom).max(1.)),
                );
                if laid.scale < 0.999 {
                    format!(
                        r#"<a:normAutofit fontScale="{}" lnSpcReduction="{}"/>"#,
                        (laid.scale * 100_000.).round() as i64,
                        (((1. - laid.scale) * 0.5).min(0.2) * 100_000.).round() as i64
                    )
                } else {
                    "<a:normAutofit/>".into()
                }
            }
            Autofit::Grow => "<a:spAutoFit/>".into(),
            Autofit::None => "<a:noAutofit/>".into(),
        };
        let mut out = format!(
            r#"<p:txBody><a:bodyPr wrap="{}" lIns="{}" tIns="{}" rIns="{}" bIns="{}" anchor="{}" rtlCol="0">{autofit}</a:bodyPr><a:lstStyle/>"#,
            if body.wrap { "square" } else { "none" },
            emu(left),
            emu(top),
            emu(right),
            emu(bottom),
            anchor_attr(body.anchor)
        );
        for (para, source) in paras.iter().zip(&body.paragraphs) {
            let size = para.style.size;
            let indent = para.level as f32 * size * 1.6;
            let hang = if para.bullet.is_some() {
                size * 1.1
            } else {
                0.
            };
            let bullet = match (&para.bullet, &source.bullet) {
                (None, _) => "<a:buNone/>".to_string(),
                (Some(_), Some(Bullet::Number)) => {
                    r#"<a:buFont typeface="+mj-lt"/><a:buAutoNum type="arabicPeriod"/>"#.to_string()
                }
                (Some((text, _)), _) => format!(
                    r#"<a:buFont typeface="Arial"/><a:buChar char="{}"/>"#,
                    escape(text)
                ),
            };
            let _ = write!(
                out,
                r#"<a:p><a:pPr marL="{}" indent="{}" algn="{}"{}><a:lnSpc><a:spcPct val="{}"/></a:lnSpc><a:spcBef><a:spcPts val="{}"/></a:spcBef><a:spcAft><a:spcPts val="{}"/></a:spcAft>{bullet}</a:pPr>"#,
                emu(indent + hang),
                -emu(hang),
                align_attr(para.align),
                if para.level > 0 {
                    format!(r#" lvl="{}""#, para.level.min(8))
                } else {
                    String::new()
                },
                (para.line_spacing * 100_000.).round() as i64,
                (para.space_before * 100.).round() as i64,
                (para.space_after * 100.).round() as i64,
            );
            let mut wrote = false;
            for ((text, style), run) in para.runs.iter().zip(&source.runs) {
                if text.is_empty() {
                    continue;
                }
                wrote = true;
                let link = run.link.as_ref().map(|url| {
                    let id = rels.external("hyperlink", url);
                    format!(r#"<a:hlinkClick r:id="{id}"/>"#)
                });
                // Line breaks within the paragraph.
                let props = run_props(style, run, theme, link.as_deref());
                for (piece_ix, piece) in text.split('\n').enumerate() {
                    if piece_ix > 0 {
                        let _ = write!(out, "<a:br>{}</a:br>", props);
                    }
                    if !piece.is_empty() {
                        let _ = write!(out, r#"<a:r>{props}<a:t>{}</a:t></a:r>"#, escape(piece));
                    }
                }
            }
            let _ = write!(
                out,
                r#"<a:endParaRPr lang="en-US" sz="{}" dirty="0"/></a:p>"#,
                (para.style.size * 100.).round().max(100.) as i64
            );
            let _ = wrote;
        }
        if body.paragraphs.is_empty() {
            out.push_str(r#"<a:p><a:endParaRPr lang="en-US"/></a:p>"#);
        }
        out.push_str("</p:txBody>");
        out
    }

    fn table_xml(
        &mut self,
        id: u32,
        name: &str,
        element: &Element,
        table: &Table,
        rels: &mut Rels,
    ) -> String {
        let theme = &self.deck_ref().theme;
        let frame = element.frame;
        let rows = table.rows.len();
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if rows == 0 || columns == 0 {
            return String::new();
        }
        let weights: Vec<f32> = if table.columns.len() == columns {
            table.columns.iter().map(|w| w.max(0.01)).collect()
        } else {
            vec![1.; columns]
        };
        let total: f32 = weights.iter().sum();
        let widths: Vec<i64> = weights.iter().map(|w| emu(w / total * frame.w)).collect();
        let row_h = emu(frame.h / rows as f32);
        let accent = table.accent.clone().unwrap_or(Color::from("accent1"));
        let border = format!(
            r#"<a:ln w="9525"><a:solidFill><a:srgbClr val="{}"/></a:solidFill></a:ln>"#,
            theme.resolve(&table.border).hex()
        );
        let dark = theme.resolve(&theme.background_color()).luminance() < 0.4;
        let band = if dark {
            Color::from("#FFFFFF1E")
        } else {
            Color(format!("#{}", theme.resolve(&accent).shade(0.9).hex()))
        };
        let mut grid = String::new();
        for width in &widths {
            let _ = write!(grid, r#"<a:gridCol w="{width}"/>"#);
        }
        let pad = emu(table.font_size * 0.45);
        let mut covered = vec![vec![(false, false); columns]; rows];
        let mut body = String::new();
        for (r, cells) in table.rows.iter().enumerate() {
            let _ = write!(body, r#"<a:tr h="{row_h}">"#);
            for c in 0..columns {
                let cell = cells.get(c).cloned().unwrap_or_default();
                let (h_merge, v_merge) = covered[r][c];
                let mut spans = String::new();
                if !h_merge && !v_merge {
                    let (span_c, span_r) = cell
                        .span
                        .map(|(a, b)| (a.max(1) as usize, b.max(1) as usize))
                        .unwrap_or((1, 1));
                    let span_c = span_c.min(columns - c);
                    let span_r = span_r.min(rows - r);
                    for (rr, line) in covered.iter_mut().enumerate().skip(r).take(span_r) {
                        for (cc, spot) in line.iter_mut().enumerate().skip(c).take(span_c) {
                            if (rr, cc) != (r, c) {
                                *spot = (cc > c, rr > r);
                            }
                        }
                    }
                    if span_c > 1 {
                        let _ = write!(spans, r#" gridSpan="{span_c}""#);
                    }
                    if span_r > 1 {
                        let _ = write!(spans, r#" rowSpan="{span_r}""#);
                    }
                } else {
                    if h_merge {
                        spans.push_str(r#" hMerge="1""#);
                    }
                    if v_merge {
                        spans.push_str(r#" vMerge="1""#);
                    }
                }
                let header = table.header && r == 0;
                let fill = match &cell.fill {
                    Some(fill) => Some(fill.clone()),
                    None if header => Some(Fill::Solid {
                        color: accent.clone(),
                        opacity: 1.,
                    }),
                    None if table.banded && r % 2 == (table.header as usize + 1) % 2 => {
                        Some(Fill::Solid {
                            color: band.clone(),
                            opacity: 1.,
                        })
                    }
                    None => None,
                };
                let fill_xml = match &fill {
                    Some(fill) => self.fill_xml(fill, rels, 1.).unwrap_or_default(),
                    None => "<a:noFill/>".into(),
                };
                let on = fill.as_ref().and_then(|fill| text_on(fill, theme));
                let paras = text::paragraphs(
                    &cell.text,
                    &Context {
                        theme,
                        place: None,
                        color: if header { on.or(Some(Rgba::WHITE)) } else { on },
                        size: Some(table.font_size),
                        bold: Some(header),
                    },
                );
                let mut text = String::new();
                for (para, source) in paras.iter().zip(&cell.text.paragraphs) {
                    let _ = write!(text, r#"<a:p><a:pPr algn="{}"/>"#, align_attr(para.align));
                    for ((run_text, style), run) in para.runs.iter().zip(&source.runs) {
                        if run_text.is_empty() {
                            continue;
                        }
                        let _ = write!(
                            text,
                            r#"<a:r>{}<a:t>{}</a:t></a:r>"#,
                            run_props(style, run, theme, None),
                            escape(run_text)
                        );
                    }
                    let _ = write!(
                        text,
                        r#"<a:endParaRPr lang="en-US" sz="{}"/></a:p>"#,
                        (table.font_size * 100.).round() as i64
                    );
                }
                if cell.text.paragraphs.is_empty() {
                    let _ = write!(
                        text,
                        r#"<a:p><a:endParaRPr lang="en-US" sz="{}"/></a:p>"#,
                        (table.font_size * 100.).round() as i64
                    );
                }
                let _ = write!(
                    body,
                    r#"<a:tc{spans}><a:txBody><a:bodyPr/><a:lstStyle/>{text}</a:txBody><a:tcPr marL="{pad}" marR="{pad}" marT="{pad}" marB="{pad}" anchor="ctr">{l}{r}{t}{b}{fill_xml}</a:tcPr></a:tc>"#,
                    l = border.replace("a:ln", "a:lnL"),
                    r = border.replace("a:ln", "a:lnR"),
                    t = border.replace("a:ln", "a:lnT"),
                    b = border.replace("a:ln", "a:lnB"),
                );
            }
            body.push_str("</a:tr>");
        }
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="{name}"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="{}" bandRow="{}"/><a:tblGrid>{grid}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
            emu(frame.x),
            emu(frame.y),
            emu(frame.w.max(1.)),
            emu(frame.h.max(1.)),
            table.header as u8,
            table.banded as u8,
        )
    }

    fn code_xml(&mut self, id: u32, name: &str, element: &Element, code: &Code) -> String {
        let background = super::code::background(code.dark);
        let lines = super::code::highlight(&code.code, &code.language, code.dark);
        let font = self.deck.theme.fonts.code.clone();
        let size = (code.font_size * 100.).round() as i64;
        let pad = emu(code.font_size * 0.9);
        let mut text = String::new();
        for (ix, tokens) in lines.iter().enumerate() {
            text.push_str(
                r#"<a:p><a:pPr><a:lnSpc><a:spcPct val="115000"/></a:lnSpc><a:buNone/></a:pPr>"#,
            );
            if code.line_numbers {
                let width = lines.len().to_string().len();
                let _ = write!(
                    text,
                    r#"<a:r><a:rPr lang="en-US" sz="{size}"><a:solidFill><a:srgbClr val="8C959F"/></a:solidFill><a:latin typeface="{f}"/><a:cs typeface="{f}"/></a:rPr><a:t>{:>width$} </a:t></a:r>"#,
                    ix + 1,
                    f = escape(&font)
                );
            }
            for (token, color, bold, italic) in tokens {
                let _ = write!(
                    text,
                    r#"<a:r><a:rPr lang="en-US" sz="{size}" b="{}" i="{}"><a:solidFill><a:srgbClr val="{}"/></a:solidFill><a:latin typeface="{f}"/><a:cs typeface="{f}"/></a:rPr><a:t>{}</a:t></a:r>"#,
                    *bold as u8,
                    *italic as u8,
                    color.hex(),
                    escape(token),
                    f = escape(&font)
                );
            }
            let _ = write!(text, r#"<a:endParaRPr lang="en-US" sz="{size}"/></a:p>"#);
        }
        let adj = ((code.font_size * 0.5) / element.frame.w.min(element.frame.h).max(1.)
            * 100_000.)
            .min(50_000.);
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val {}"/></a:avLst></a:prstGeom><a:solidFill><a:srgbClr val="{}"/></a:solidFill></p:spPr><p:txBody><a:bodyPr wrap="none" lIns="{pad}" tIns="{pad}" rIns="{pad}" bIns="{pad}" anchor="t"><a:normAutofit/></a:bodyPr><a:lstStyle/>{text}</p:txBody></p:sp>"#,
            xfrm(&element.frame),
            adj.round() as i64,
            background.hex()
        )
    }
}

/// The ids of a shape tree's shapes (1 is the tree itself).
struct Tree {
    next: u32,
}

impl Default for Tree {
    fn default() -> Self {
        Tree { next: 2 }
    }
}

impl Tree {
    fn next(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }
}

const GROUP_HEADER: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

fn xfrm(frame: &Frame) -> String {
    format!(
        r#"<a:xfrm{}{}{}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm>"#,
        rotation_attr(frame.rotation),
        if frame.flip_h { r#" flipH="1""# } else { "" },
        if frame.flip_v { r#" flipV="1""# } else { "" },
        emu(frame.x),
        emu(frame.y),
        emu(frame.w.max(0.)),
        emu(frame.h.max(0.))
    )
}

fn rotation_attr(rotation: f32) -> String {
    let degrees = rotation.rem_euclid(360.);
    if degrees.abs() < 0.001 {
        String::new()
    } else {
        format!(r#" rot="{}""#, (degrees * 60_000.).round() as i64)
    }
}

fn geometry_xml(shape: &Shape, w: f32, h: f32) -> String {
    // A PowerPoint outline PilotHouse draws as its nearest: written back as
    // it was, while it is still that.
    if let Some((preset, adjust)) = &shape.preset
        && super::pptx_read::nearest_geometry(preset) == shape.geometry
    {
        let mut gds = String::new();
        for (name, formula) in adjust {
            let _ = write!(
                gds,
                r#"<a:gd name="{}" fmla="{}"/>"#,
                escape(name),
                escape(formula)
            );
        }
        return format!(
            r#"<a:prstGeom prst="{}"><a:avLst>{gds}</a:avLst></a:prstGeom>"#,
            escape(preset)
        );
    }
    if shape.geometry == Geometry::Path {
        return custom_geometry(shape.path.as_deref().unwrap_or_default(), w, h);
    }
    let preset = super::shapes::preset_name(shape.geometry).unwrap_or("rect");
    let radius = match shape.geometry {
        Geometry::RoundRect => Some(
            shape
                .radius
                .unwrap_or_else(|| super::shapes::default_radius(w, h)),
        ),
        Geometry::Rect => shape.radius.filter(|radius| *radius > 0.),
        _ => None,
    };
    match radius {
        Some(radius) => format!(
            r#"<a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val {}"/></a:avLst></a:prstGeom>"#,
            (radius / w.min(h).max(0.01) * 100_000.)
                .min(50_000.)
                .round() as i64
        ),
        None => format!(r#"<a:prstGeom prst="{preset}"><a:avLst/></a:prstGeom>"#),
    }
}

/// An SVG path as PowerPoint's own path (in the frame's points as EMU).
fn custom_geometry(d: &str, w: f32, h: f32) -> String {
    use svgtypes::{SimplePathSegment, SimplifyingPathParser};
    let point = |x: f64, y: f64| format!(r#"<a:pt x="{}" y="{}"/>"#, emu(x as f32), emu(y as f32));
    let mut path = String::new();
    for segment in SimplifyingPathParser::from(d).flatten() {
        match segment {
            SimplePathSegment::MoveTo { x, y } => {
                let _ = write!(path, "<a:moveTo>{}</a:moveTo>", point(x, y));
            }
            SimplePathSegment::LineTo { x, y } => {
                let _ = write!(path, "<a:lnTo>{}</a:lnTo>", point(x, y));
            }
            SimplePathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let _ = write!(
                    path,
                    "<a:cubicBezTo>{}{}{}</a:cubicBezTo>",
                    point(x1, y1),
                    point(x2, y2),
                    point(x, y)
                );
            }
            SimplePathSegment::Quadratic { x1, y1, x, y } => {
                let _ = write!(
                    path,
                    "<a:quadBezTo>{}{}</a:quadBezTo>",
                    point(x1, y1),
                    point(x, y)
                );
            }
            SimplePathSegment::ClosePath => path.push_str("<a:close/>"),
        }
    }
    format!(
        r#"<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst><a:path w="{}" h="{}">{path}</a:path></a:pathLst></a:custGeom>"#,
        emu(w.max(0.01)),
        emu(h.max(0.01))
    )
}

fn line_xml(line: &Line, theme: &Theme, opacity: f32, always: bool, fill: &Fill) -> String {
    let (width, color) = if line.width > 0. {
        (
            line.width,
            line.color.clone().unwrap_or(Color::from("dark1")),
        )
    } else if always {
        let color = match fill {
            Fill::Solid { color, .. } => color.clone(),
            _ => Color::from("dark1"),
        };
        (2., line.color.clone().unwrap_or(color))
    } else {
        return "<a:ln><a:noFill/></a:ln>".into();
    };
    let dash = match line.dash {
        Dash::Solid => "solid",
        Dash::Dash => "dash",
        Dash::Dot => "sysDot",
        Dash::DashDot => "dashDot",
    };
    let head = if line.head {
        r#"<a:headEnd type="triangle"/>"#
    } else {
        ""
    };
    let tail = if line.tail {
        r#"<a:tailEnd type="triangle"/>"#
    } else {
        ""
    };
    format!(
        r#"<a:ln w="{}" cap="rnd">{}<a:prstDash val="{dash}"/><a:round/>{head}{tail}</a:ln>"#,
        emu(width),
        solid(&color, theme, opacity)
    )
}

fn shadow_xml(shadow: Option<&Shadow>, theme: &Theme) -> String {
    let Some(shadow) = shadow else {
        return String::new();
    };
    let color = theme.resolve(&shadow.color);
    format!(
        r#"<a:effectLst><a:outerShdw blurRad="{}" dist="{}" dir="{}" algn="ctr" rotWithShape="0"><a:srgbClr val="{}"><a:alpha val="{}"/></a:srgbClr></a:outerShdw></a:effectLst>"#,
        emu(shadow.blur),
        emu(shadow.distance),
        (shadow.angle.rem_euclid(360.) * 60_000.).round() as i64,
        color.hex(),
        (shadow.opacity.clamp(0., 1.) * 100_000.).round() as i64
    )
}

/// A colour as PowerPoint writes it: a theme slot as itself (so a new
/// theme in PowerPoint recolours it), anything else as its RGB.
fn color_xml(color: &Color, theme: &Theme, opacity: f32) -> String {
    let rgba = theme.resolve(color);
    let alpha = rgba.a as f32 / 255. * opacity.clamp(0., 1.);
    let alpha_xml = if alpha < 0.999 {
        format!(r#"<a:alpha val="{}"/>"#, (alpha * 100_000.).round() as i64)
    } else {
        String::new()
    };
    match super::model::parse_slot(&color.0) {
        Some((slot, 0.)) => {
            let name = match slot {
                "dark1" => "dk1",
                "light1" => "lt1",
                "dark2" => "dk2",
                "light2" => "lt2",
                "link" => "hlink",
                "followed_link" => "folHlink",
                other => other,
            };
            format!(r#"<a:schemeClr val="{name}">{alpha_xml}</a:schemeClr>"#)
        }
        _ => format!(r#"<a:srgbClr val="{}">{alpha_xml}</a:srgbClr>"#, rgba.hex()),
    }
}

fn solid(color: &Color, theme: &Theme, opacity: f32) -> String {
    format!(
        "<a:solidFill>{}</a:solidFill>",
        color_xml(color, theme, opacity)
    )
}

/// The colour text takes on `fill` when nothing sets it.
fn text_on(fill: &Fill, theme: &Theme) -> Option<Rgba> {
    match fill {
        Fill::Solid { color, opacity } => {
            let rgba = theme.resolve(color);
            (rgba.a as f32 / 255. * opacity > 0.5).then(|| super::svg::contrasting(rgba, theme))
        }
        Fill::Gradient { stops, .. } if !stops.is_empty() => Some(super::svg::contrasting(
            theme.resolve(&stops[0].color),
            theme,
        )),
        _ => None,
    }
}

fn run_props(style: &text::Style, run: &Run, theme: &Theme, link: Option<&str>) -> String {
    let mut attrs = format!(
        r#" lang="en-US" sz="{}" b="{}" i="{}""#,
        (style.size * 100.).round().clamp(100., 400_000.) as i64,
        style.bold as u8,
        style.italic as u8
    );
    if style.underline {
        attrs.push_str(r#" u="sng""#);
    }
    if style.strike {
        attrs.push_str(r#" strike="sngStrike""#);
    }
    if style.baseline != 0 {
        let _ = write!(attrs, r#" baseline="{}""#, style.baseline as i64 * 1000);
    }
    // Colours the run sets keep their theme slot; others are as drawn.
    let color = match &run.color {
        Some(color) => solid(color, theme, 1.),
        None => format!(
            r#"<a:solidFill><a:srgbClr val="{}"/></a:solidFill>"#,
            style.color.hex()
        ),
    };
    let highlight = style
        .highlight
        .map(|rgba| {
            format!(
                r#"<a:highlight><a:srgbClr val="{}"/></a:highlight>"#,
                rgba.hex()
            )
        })
        .unwrap_or_default();
    let font = escape(&style.font);
    format!(
        r#"<a:rPr{attrs} dirty="0">{color}{highlight}<a:latin typeface="{font}"/><a:cs typeface="{font}"/>{}</a:rPr>"#,
        link.unwrap_or_default()
    )
}

fn background_xml(
    fill: &Fill,
    theme: &Theme,
    writer: &mut Writer,
    rels: &mut Rels,
) -> Result<String> {
    let _ = theme;
    let xml = writer.fill_xml(fill, rels, 1.)?;
    Ok(if xml == "<a:noFill/>" {
        solid(&Color::from("light1"), &writer.deck.theme, 1.)
    } else {
        xml
    })
}

fn align_attr(align: Align) -> &'static str {
    match align {
        Align::Left => "l",
        Align::Center => "ctr",
        Align::Right => "r",
        Align::Justify => "just",
    }
}

fn anchor_attr(anchor: Anchor) -> &'static str {
    match anchor {
        Anchor::Top => "t",
        Anchor::Middle => "ctr",
        Anchor::Bottom => "b",
    }
}

fn layout_type(id: &str) -> &'static str {
    match id {
        "title" => "title",
        "title_content" | "agenda" => "obj",
        "section" => "secHead",
        "two_content" => "twoObj",
        "comparison" => "twoTxTwoObj",
        "title_only" => "titleOnly",
        "blank" => "blank",
        "picture_caption" => "picTx",
        _ => "cust",
    }
}

/// A placeholder's `type` and `idx` attributes: as its PowerPoint file had
/// them, else for its kind in layout `layout`.
fn placeholder_attrs(layout: &str, place: &Placeholder) -> (String, String) {
    match &place.ph {
        Some((kind, idx)) => (
            if kind.is_empty() {
                String::new()
            } else {
                format!(r#" type="{}""#, escape(kind))
            },
            if *idx == 0 && !kind.is_empty() {
                String::new()
            } else {
                format!(r#" idx="{idx}""#)
            },
        ),
        None => placeholder_type(layout, place.kind),
    }
}

/// A placeholder's `type` and `idx` attributes in layout `layout`.
fn placeholder_type(layout: &str, kind: PlaceholderKind) -> (String, String) {
    let typed = |kind: &str| format!(r#" type="{kind}""#);
    let idx = |idx: u32| format!(r#" idx="{idx}""#);
    match kind {
        PlaceholderKind::Title if layout == "title" => (typed("ctrTitle"), String::new()),
        PlaceholderKind::Title => (typed("title"), String::new()),
        PlaceholderKind::Subtitle if layout == "title" => (typed("subTitle"), idx(1)),
        PlaceholderKind::Subtitle => (typed("body"), idx(1)),
        PlaceholderKind::Body => (String::new(), idx(1)),
        PlaceholderKind::Body2 => (String::new(), idx(2)),
        PlaceholderKind::Heading => (typed("body"), idx(3)),
        PlaceholderKind::Heading2 => (typed("body"), idx(4)),
        PlaceholderKind::Picture => (typed("pic"), idx(5)),
        PlaceholderKind::Statement => (typed("body"), idx(6)),
        PlaceholderKind::Caption => (typed("body"), idx(7)),
        PlaceholderKind::Footer => (typed("ftr"), idx(10)),
        PlaceholderKind::SlideNumber => (typed("sldNum"), idx(11)),
    }
}

fn text_styles(theme: &Theme) -> String {
    let style = |text: &TextStyle, bullets: bool| {
        let mut levels = String::new();
        for level in 0..9 {
            let size = (text.size * if level == 0 { 1. } else { 0.9 }).max(8.);
            let indent = level as f32 * size * 1.6;
            let hang = if bullets { size * 1.1 } else { 0. };
            let bullet = if bullets {
                let char = match text.bullets.get(level) {
                    Some(Bullet::Char(char)) => char.clone(),
                    _ => "•".into(),
                };
                format!(
                    r#"<a:buFont typeface="Arial"/><a:buChar char="{}"/>"#,
                    escape(&char)
                )
            } else {
                "<a:buNone/>".into()
            };
            let _ = write!(
                levels,
                r#"<a:lvl{n}pPr marL="{}" indent="{}" algn="{}"><a:lnSpc><a:spcPct val="{}"/></a:lnSpc><a:spcAft><a:spcPts val="{}"/></a:spcAft>{bullet}<a:defRPr sz="{}" b="{}" i="{}">{}<a:latin typeface="{}"/><a:cs typeface="{}"/></a:defRPr></a:lvl{n}pPr>"#,
                emu(indent + hang),
                -emu(hang),
                align_attr(text.align),
                (text.line_spacing * 100_000.).round() as i64,
                (text.space_after * 100.).round() as i64,
                (size * 100.).round() as i64,
                text.bold as u8,
                text.italic as u8,
                solid(&text.color, theme, 1.),
                escape(theme.font(&text.font)),
                escape(theme.font(&text.font)),
                n = level + 1,
            );
        }
        levels
    };
    format!(
        "<p:titleStyle>{}</p:titleStyle><p:bodyStyle>{}</p:bodyStyle><p:otherStyle>{}</p:otherStyle>",
        style(&theme.title, false),
        style(&theme.body, true),
        style(&theme.body, false)
    )
}

fn default_text_style(theme: &Theme) -> String {
    format!(
        r#"<p:defaultTextStyle><a:lvl1pPr marL="0" algn="l"><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="{}"/></a:defRPr></a:lvl1pPr></p:defaultTextStyle>"#,
        escape(&theme.fonts.body)
    )
}

fn sections(deck: &Deck) -> String {
    if deck.sections.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        r#"<p:extLst><p:ext uri="{521415D9-36F7-43E2-AB2F-B90AF26B5E84}"><p14:sectionLst xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main">"#,
    );
    let mut owned: Vec<(usize, String)> = deck
        .sections
        .iter()
        .filter_map(|section| {
            deck.slides
                .iter()
                .position(|slide| slide.id == section.slide)
                .map(|at| (at, section.name.clone()))
        })
        .collect();
    owned.sort_by_key(|(at, _)| *at);
    // Slides before the first section go in a section of their own.
    if owned.first().is_some_and(|(at, _)| *at > 0) {
        owned.insert(0, (0, String::new()));
    }
    for (ix, (at, name)) in owned.iter().enumerate() {
        let end = owned
            .get(ix + 1)
            .map_or(deck.slides.len(), |(next, _)| *next);
        let _ = write!(
            out,
            r#"<p14:section name="{}" id="{{{}}}"><p14:sldIdLst>"#,
            escape(if name.is_empty() {
                "Default Section"
            } else {
                name
            }),
            guid(ix as u32)
        );
        for slide in *at..end {
            let _ = write!(out, r#"<p14:sldId id="{}"/>"#, 256 + slide);
        }
        out.push_str("</p14:sldIdLst></p14:section>");
    }
    out.push_str("</p14:sectionLst></p:ext></p:extLst>");
    out
}

fn guid(n: u32) -> String {
    format!("5C9F6A3B-0000-4000-8000-{:012X}", n as u64 + 1)
}

fn theme_xml(theme: &Theme, name: &str) -> String {
    let slot = |slot: &str| {
        theme
            .slot(slot)
            .map(|rgba| rgba.hex())
            .unwrap_or_else(|| "000000".into())
    };
    let font = |face: &str| {
        format!(
            r#"<a:latin typeface="{}"/><a:ea typeface=""/><a:cs typeface=""/>"#,
            escape(face)
        )
    };
    format!(
        r#"{XML}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="{}"><a:themeElements><a:clrScheme name="{}"><a:dk1><a:srgbClr val="{}"/></a:dk1><a:lt1><a:srgbClr val="{}"/></a:lt1><a:dk2><a:srgbClr val="{}"/></a:dk2><a:lt2><a:srgbClr val="{}"/></a:lt2><a:accent1><a:srgbClr val="{}"/></a:accent1><a:accent2><a:srgbClr val="{}"/></a:accent2><a:accent3><a:srgbClr val="{}"/></a:accent3><a:accent4><a:srgbClr val="{}"/></a:accent4><a:accent5><a:srgbClr val="{}"/></a:accent5><a:accent6><a:srgbClr val="{}"/></a:accent6><a:hlink><a:srgbClr val="{}"/></a:hlink><a:folHlink><a:srgbClr val="{}"/></a:folHlink></a:clrScheme><a:fontScheme name="{}"><a:majorFont>{}</a:majorFont><a:minorFont>{}</a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:shade val="80000"/></a:schemeClr></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst><a:outerShdw blurRad="57150" dist="19050" dir="5400000" algn="ctr" rotWithShape="0"><a:srgbClr val="000000"><a:alpha val="63000"/></a:srgbClr></a:outerShdw></a:effectLst></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:shade val="90000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#,
        escape(name),
        escape(&theme.name),
        slot("dark1"),
        slot("light1"),
        slot("dark2"),
        slot("light2"),
        slot("accent1"),
        slot("accent2"),
        slot("accent3"),
        slot("accent4"),
        slot("accent5"),
        slot("accent6"),
        slot("link"),
        slot("followed_link"),
        escape(&theme.name),
        font(&theme.fonts.heading),
        font(&theme.fonts.body),
    )
}

/// A chart as DrawingML (its data written in, no workbook).
/// A chart as DrawingML, its data both written in and in the workbook
/// `data` (a relationship) names.
fn chart_xml(chart: &Chart, theme: &Theme, data: &str) -> String {
    let ns = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    let text_color = theme.resolve(&theme.body.color).hex();
    let font = escape(&theme.fonts.body);
    let size = (chart.font_size * 100.).round() as i64;
    let tx_pr = format!(
        r#"<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="{size}"><a:solidFill><a:srgbClr val="{text_color}"/></a:solidFill><a:latin typeface="{font}"/></a:defRPr></a:pPr><a:endParaRPr lang="en-US"/></a:p></c:txPr>"#
    );
    // Where each value is in the workbook (see `xlsx::chart_workbook`).
    let range = |column: usize, rows: usize| {
        let column = super::xlsx::column(column);
        format!("Sheet1!${column}$2:${column}${}", rows.max(1) + 1)
    };
    let categories = |count: usize| {
        let mut out = format!(
            r#"<c:cat><c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val="{count}"/>"#,
            range(0, count)
        );
        for ix in 0..count {
            let name = chart.categories.get(ix).cloned().unwrap_or_default();
            let _ = write!(
                out,
                r#"<c:pt idx="{ix}"><c:v>{}</c:v></c:pt>"#,
                escape(&name)
            );
        }
        out.push_str("</c:strCache></c:strRef></c:cat>");
        out
    };
    let numbers = |tag: &str, column: usize, values: &[f64]| {
        let mut out = format!(
            r#"<c:{tag}><c:numRef><c:f>{}</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="{}"/>"#,
            range(column, values.len()),
            values.len()
        );
        for (ix, value) in values.iter().enumerate() {
            let _ = write!(out, r#"<c:pt idx="{ix}"><c:v>{value}</c:v></c:pt>"#);
        }
        let _ = write!(out, "</c:numCache></c:numRef></c:{tag}>");
        out
    };
    let name_of = |column: usize, name: &str| {
        format!(
            r#"<c:tx><c:strRef><c:f>Sheet1!${}$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{}</c:v></c:pt></c:strCache></c:strRef></c:tx>"#,
            super::xlsx::column(column),
            escape(name)
        )
    };
    let labels = if chart.labels {
        r#"<c:dLbls><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="0"/><c:showBubbleSize val="0"/></c:dLbls>"#
    } else {
        ""
    };
    let pie = matches!(chart.chart, ChartKind::Pie | ChartKind::Doughnut);
    let count = chart.categories.len().max(
        chart
            .series
            .iter()
            .map(|series| series.values.len())
            .max()
            .unwrap_or(0),
    );
    let mut series_xml = String::new();
    for (ix, series) in chart.series.iter().enumerate() {
        if pie && ix > 0 {
            break;
        }
        let color = series_color(chart, theme, ix).hex();
        let paint = match chart.chart {
            ChartKind::Line | ChartKind::Scatter => format!(
                r#"<c:spPr><a:ln w="28575" cap="rnd"><a:solidFill><a:srgbClr val="{color}"/></a:solidFill><a:round/></a:ln></c:spPr>"#
            ),
            _ => {
                format!(r#"<c:spPr><a:solidFill><a:srgbClr val="{color}"/></a:solidFill></c:spPr>"#)
            }
        };
        let points = if pie {
            (0..series.values.len())
                .map(|point| {
                    format!(
                        r#"<c:dPt><c:idx val="{point}"/><c:bubble3D val="0"/><c:spPr><a:solidFill><a:srgbClr val="{}"/></a:solidFill><a:ln w="19050"><a:solidFill><a:srgbClr val="{}"/></a:solidFill></a:ln></c:spPr></c:dPt>"#,
                        series_color(chart, theme, point).hex(),
                        theme.resolve(&Color::from("light1")).hex()
                    )
                })
                .collect()
        } else {
            String::new()
        };
        let marker = match chart.chart {
            ChartKind::Line => format!(
                r#"<c:marker><c:symbol val="circle"/><c:size val="6"/><c:spPr><a:solidFill><a:srgbClr val="{color}"/></a:solidFill></c:spPr></c:marker>"#
            ),
            ChartKind::Scatter => format!(
                r#"<c:marker><c:symbol val="circle"/><c:size val="7"/><c:spPr><a:solidFill><a:srgbClr val="{color}"/></a:solidFill></c:spPr></c:marker>"#
            ),
            _ => String::new(),
        };
        let data = if chart.chart == ChartKind::Scatter {
            let xs: Vec<f64> = if series.x.len() == series.values.len() {
                series.x.clone()
            } else {
                (1..=series.values.len()).map(|x| x as f64).collect()
            };
            format!(
                "{}{}",
                numbers("xVal", ix * 2, &xs),
                numbers("yVal", ix * 2 + 1, &series.values)
            )
        } else {
            format!(
                "{}{}",
                categories(count),
                numbers("val", ix + 1, &series.values)
            )
        };
        let tx = name_of(
            if chart.chart == ChartKind::Scatter {
                ix * 2 + 1
            } else {
                ix + 1
            },
            &series.name,
        );
        let scatter_line = if chart.chart == ChartKind::Scatter {
            r#"<c:spPr><a:ln w="19050"><a:noFill/></a:ln></c:spPr>"#.to_string()
        } else {
            paint
        };
        let _ = write!(
            series_xml,
            r#"<c:ser><c:idx val="{ix}"/><c:order val="{ix}"/>{tx}{scatter_line}{marker}{points}{labels}{data}{}</c:ser>"#,
            if chart.chart == ChartKind::Line || chart.chart == ChartKind::Scatter {
                r#"<c:smooth val="0"/>"#
            } else {
                ""
            }
        );
    }
    let axes_ids = r#"<c:axId val="111111111"/><c:axId val="222222222"/>"#;
    let plot = match chart.chart {
        ChartKind::Column | ChartKind::Bar | ChartKind::StackedColumn => format!(
            r#"<c:barChart><c:barDir val="{}"/><c:grouping val="{}"/><c:varyColors val="0"/>{series_xml}<c:gapWidth val="80"/>{}{axes_ids}</c:barChart>"#,
            if chart.chart == ChartKind::Bar {
                "bar"
            } else {
                "col"
            },
            if chart.chart == ChartKind::StackedColumn {
                "stacked"
            } else {
                "clustered"
            },
            if chart.chart == ChartKind::StackedColumn {
                r#"<c:overlap val="100"/>"#
            } else {
                ""
            }
        ),
        ChartKind::Line => format!(
            r#"<c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/>{series_xml}<c:marker val="1"/>{axes_ids}</c:lineChart>"#
        ),
        ChartKind::Area => format!(
            r#"<c:areaChart><c:grouping val="standard"/><c:varyColors val="0"/>{series_xml}{axes_ids}</c:areaChart>"#
        ),
        ChartKind::Pie => format!(
            r#"<c:pieChart><c:varyColors val="1"/>{series_xml}<c:firstSliceAng val="0"/></c:pieChart>"#
        ),
        ChartKind::Doughnut => format!(
            r#"<c:doughnutChart><c:varyColors val="1"/>{series_xml}<c:firstSliceAng val="0"/><c:holeSize val="55"/></c:doughnutChart>"#
        ),
        ChartKind::Scatter => format!(
            r#"<c:scatterChart><c:scatterStyle val="lineMarker"/><c:varyColors val="0"/>{series_xml}{axes_ids}</c:scatterChart>"#
        ),
    };
    let grid = format!(
        r#"<c:majorGridlines><c:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="{text_color}"><a:alpha val="25000"/></a:srgbClr></a:solidFill></a:ln></c:spPr></c:majorGridlines>"#
    );
    let axis_title = |title: &str| {
        if title.is_empty() {
            String::new()
        } else {
            format!(
                r#"<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title>"#,
                escape(title)
            )
        }
    };
    let horizontal = chart.chart == ChartKind::Bar;
    let axes = if pie {
        String::new()
    } else if chart.chart == ChartKind::Scatter {
        format!(
            r#"<c:valAx><c:axId val="111111111"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/>{}<c:numFmt formatCode="General" sourceLinked="0"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="222222222"/><c:crosses val="autoZero"/><c:crossBetween val="midCat"/></c:valAx><c:valAx><c:axId val="222222222"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/>{grid}{}<c:numFmt formatCode="General" sourceLinked="0"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="111111111"/><c:crosses val="autoZero"/><c:crossBetween val="midCat"/></c:valAx>"#,
            axis_title(&chart.x_title),
            axis_title(&chart.y_title)
        )
    } else {
        format!(
            r#"<c:catAx><c:axId val="111111111"/><c:scaling><c:orientation val="{}"/></c:scaling><c:delete val="0"/><c:axPos val="{}"/>{}<c:numFmt formatCode="General" sourceLinked="0"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="222222222"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx><c:valAx><c:axId val="222222222"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="{}"/>{grid}{}<c:numFmt formatCode="General" sourceLinked="0"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="111111111"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx>"#,
            if horizontal { "maxMin" } else { "minMax" },
            if horizontal { "l" } else { "b" },
            axis_title(&chart.x_title),
            if horizontal { "b" } else { "l" },
            axis_title(&chart.y_title)
        )
    };
    let title = if chart.title.is_empty() {
        r#"<c:autoTitleDeleted val="1"/>"#.to_string()
    } else {
        format!(
            r#"<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="{}" b="1"/></a:pPr><a:r><a:rPr lang="en-US" sz="{}" b="1"/><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/>"#,
            (chart.font_size * 125.).round() as i64,
            (chart.font_size * 125.).round() as i64,
            escape(&chart.title)
        )
    };
    let legend = if chart.legend {
        r#"<c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend>"#
    } else {
        ""
    };
    format!(
        r#"{XML}<c:chartSpace {ns}><c:roundedCorners val="0"/><c:chart>{title}<c:plotArea><c:layout/>{plot}{axes}<c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr></c:plotArea>{legend}<c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart><c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr>{tx_pr}<c:externalData r:id="{data}"><c:autoUpdate val="0"/></c:externalData></c:chartSpace>"#
    )
}

/// The shape-tree id each element of a slide gets, in the order they are
/// written (empty placeholders are not).
fn tree_ids(elements: &[Element]) -> HashMap<String, u32> {
    fn walk(element: &Element, next: &mut u32, ids: &mut HashMap<String, u32>) {
        ids.insert(element.id.clone(), *next);
        *next += 1;
        if let Kind::Group(group) = &element.kind {
            for child in &group.children {
                walk(child, next, ids);
            }
        }
    }
    let mut ids = HashMap::new();
    let mut next = Tree::default().next;
    for element in elements {
        if element.placeholder.is_some() && super::ops::is_empty(element) {
            continue;
        }
        walk(element, &mut next, &mut ids);
    }
    ids
}

/// How the slide comes in, as PowerPoint writes it.
fn transition_xml(slide: &Slide) -> String {
    let speed = match slide.transition_seconds {
        seconds if seconds < 0.5 => "fast",
        seconds if seconds < 1. => "med",
        _ => "slow",
    };
    // Push, wipe and cover name where the slide moves to (from the right:
    // leftwards).
    let towards = match slide.transition_from {
        Direction::Left => "r",
        Direction::Right => "l",
        Direction::Top => "d",
        Direction::Bottom => "u",
    };
    let effect = match slide.transition {
        Transition::None => return String::new(),
        Transition::Fade => "<p:fade/>".to_string(),
        Transition::Push => format!(r#"<p:push dir="{towards}"/>"#),
        Transition::Wipe => format!(r#"<p:wipe dir="{towards}"/>"#),
        Transition::Cover => format!(r#"<p:cover dir="{towards}"/>"#),
        Transition::Zoom => r#"<p:zoom dir="in"/>"#.to_string(),
        Transition::Dissolve => "<p:dissolve/>".to_string(),
    };
    format!(r#"<p:transition spd="{speed}">{effect}</p:transition>"#)
}

/// The slide's animations as PowerPoint's timing tree: each click starts a
/// group; in it, effects with the previous share a start, effects after
/// the previous start when it ends.
fn timing_xml(slide: &Slide, ids: &HashMap<String, u32>) -> String {
    let animations: Vec<(&Animation, u32)> = slide
        .animations
        .iter()
        .filter_map(|animation| ids.get(&animation.id).map(|id| (animation, *id)))
        .collect();
    if animations.is_empty() {
        return String::new();
    }
    // Clicks, each a list of moments, each a list of effects.
    let mut clicks: Vec<Vec<Vec<(&Animation, u32)>>> = Vec::new();
    let mut first_auto = false;
    for (ix, (animation, id)) in animations.iter().enumerate() {
        match animation.start {
            Start::OnClick => clicks.push(vec![vec![(animation, *id)]]),
            _ if clicks.is_empty() => {
                first_auto = ix == 0;
                clicks.push(vec![vec![(animation, *id)]]);
            }
            Start::WithPrevious => clicks
                .last_mut()
                .and_then(|click| click.last_mut())
                .expect("a moment")
                .push((animation, *id)),
            Start::AfterPrevious => clicks
                .last_mut()
                .expect("a click")
                .push(vec![(animation, *id)]),
        }
    }
    let mut next_id = 3u32;
    let mut take = || {
        let id = next_id;
        next_id += 1;
        id
    };
    let ms = |seconds: f32| (seconds.max(0.) * 1000.).round() as i64;
    let mut body = String::new();
    for (click_ix, click) in clicks.iter().enumerate() {
        let click_id = take();
        let start = if click_ix == 0 && first_auto {
            r#"<p:cond delay="indefinite"/><p:cond evt="onBegin" delay="0"><p:tn val="2"/></p:cond>"#
        } else {
            r#"<p:cond delay="indefinite"/>"#
        };
        let _ = write!(
            body,
            r#"<p:par><p:cTn id="{click_id}" fill="hold"><p:stCondLst>{start}</p:stCondLst><p:childTnLst>"#
        );
        let mut at = 0f32;
        for (moment_ix, moment) in click.iter().enumerate() {
            let moment_id = take();
            let _ = write!(
                body,
                r#"<p:par><p:cTn id="{moment_id}" fill="hold"><p:stCondLst><p:cond delay="{}"/></p:stCondLst><p:childTnLst>"#,
                ms(at)
            );
            let mut longest = 0f32;
            for (effect_ix, (animation, shape)) in moment.iter().enumerate() {
                longest = longest.max(animation.delay + animation.seconds);
                let node = if moment_ix == 0 && effect_ix == 0 {
                    if click_ix == 0 && first_auto {
                        "withEffect"
                    } else {
                        "clickEffect"
                    }
                } else if effect_ix == 0 {
                    "afterEffect"
                } else {
                    "withEffect"
                };
                let (preset, subtype) = match animation.effect {
                    Effect::Appear => (1, 0),
                    Effect::Fade => (10, 0),
                    Effect::FlyIn => (2, direction_subtype(animation.from)),
                    Effect::Zoom => (53, 16),
                    Effect::Wipe => (22, direction_subtype(animation.from)),
                };
                let effect_id = take();
                let set_id = take();
                let dur = ms(animation.seconds).max(1);
                let show = format!(
                    r#"<p:set><p:cBhvr><p:cTn id="{set_id}" dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set>"#
                );
                let motion = match animation.effect {
                    Effect::Appear => String::new(),
                    Effect::Fade => {
                        let id = take();
                        format!(
                            r#"<p:animEffect transition="in" filter="fade"><p:cBhvr><p:cTn id="{id}" dur="{dur}"/><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl></p:cBhvr></p:animEffect>"#
                        )
                    }
                    Effect::Wipe => {
                        let id = take();
                        let side = match animation.from {
                            Direction::Left => "left",
                            Direction::Right => "right",
                            Direction::Top => "up",
                            Direction::Bottom => "down",
                        };
                        format!(
                            r#"<p:animEffect transition="in" filter="wipe({side})"><p:cBhvr><p:cTn id="{id}" dur="{dur}"/><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl></p:cBhvr></p:animEffect>"#
                        )
                    }
                    Effect::Zoom => {
                        let (a, b) = (take(), take());
                        format!(
                            r##"<p:anim calcmode="lin" valueType="num"><p:cBhvr><p:cTn id="{a}" dur="{dur}" fill="hold"/><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl><p:attrNameLst><p:attrName>ppt_w</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst><p:tav tm="0"><p:val><p:fltVal val="0"/></p:val></p:tav><p:tav tm="100000"><p:val><p:strVal val="#ppt_w"/></p:val></p:tav></p:tavLst></p:anim><p:anim calcmode="lin" valueType="num"><p:cBhvr><p:cTn id="{b}" dur="{dur}" fill="hold"/><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl><p:attrNameLst><p:attrName>ppt_h</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst><p:tav tm="0"><p:val><p:fltVal val="0"/></p:val></p:tav><p:tav tm="100000"><p:val><p:strVal val="#ppt_h"/></p:val></p:tav></p:tavLst></p:anim>"##
                        )
                    }
                    Effect::FlyIn => {
                        let (a, b) = (take(), take());
                        let (x, y) = match animation.from {
                            Direction::Left => ("0-#ppt_w/2", "#ppt_y"),
                            Direction::Right => ("1+#ppt_w/2", "#ppt_y"),
                            Direction::Top => ("#ppt_x", "0-#ppt_h/2"),
                            Direction::Bottom => ("#ppt_x", "1+#ppt_h/2"),
                        };
                        format!(
                            r##"<p:anim calcmode="lin" valueType="num"><p:cBhvr additive="base"><p:cTn id="{a}" dur="{dur}" fill="hold"/><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl><p:attrNameLst><p:attrName>ppt_x</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst><p:tav tm="0"><p:val><p:strVal val="{x}"/></p:val></p:tav><p:tav tm="100000"><p:val><p:strVal val="#ppt_x"/></p:val></p:tav></p:tavLst></p:anim><p:anim calcmode="lin" valueType="num"><p:cBhvr additive="base"><p:cTn id="{b}" dur="{dur}" fill="hold"/><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl><p:attrNameLst><p:attrName>ppt_y</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst><p:tav tm="0"><p:val><p:strVal val="{y}"/></p:val></p:tav><p:tav tm="100000"><p:val><p:strVal val="#ppt_y"/></p:val></p:tav></p:tavLst></p:anim>"##
                        )
                    }
                };
                let _ = write!(
                    body,
                    r#"<p:par><p:cTn id="{effect_id}" presetID="{preset}" presetClass="entr" presetSubtype="{subtype}" fill="hold" grpId="0" nodeType="{node}"><p:stCondLst><p:cond delay="{}"/></p:stCondLst><p:childTnLst>{show}{motion}</p:childTnLst></p:cTn></p:par>"#,
                    ms(animation.delay)
                );
            }
            body.push_str("</p:childTnLst></p:cTn></p:par>");
            at += longest;
        }
        body.push_str("</p:childTnLst></p:cTn></p:par>");
    }
    let mut built = String::new();
    let mut seen = std::collections::HashSet::new();
    for (_, shape) in &animations {
        if seen.insert(*shape) {
            let _ = write!(built, r#"<p:bldP spid="{shape}" grpId="0"/>"#);
        }
    }
    format!(
        r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>{body}</p:childTnLst></p:cTn><p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst><p:bldLst>{built}</p:bldLst></p:timing>"#
    )
}

/// PowerPoint's subtype for a side an effect comes from.
fn direction_subtype(from: Direction) -> u32 {
    match from {
        Direction::Top => 1,
        Direction::Right => 2,
        Direction::Bottom => 4,
        Direction::Left => 8,
    }
}

/// Seconds since 1970 as PowerPoint writes a comment's time.
fn w3c_time(seconds: u64) -> String {
    let full = civil(seconds as i64);
    format!("{full}.000")
}

/// Now, as W3C date and time (UTC).
fn time_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0) as i64;
    format!("{}Z", civil(seconds))
}

/// Seconds since 1970 as `YYYY-MM-DDTHH:MM:SS` (UTC).
fn civil(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// The deck inside a PPTX made here, if it is one.
pub fn read_deck(path: &Path) -> Result<Option<Deck>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    let Ok(mut part) = archive.by_name(DECK_PART) else {
        return Ok(None);
    };
    // Each part read up to a limit: a crafted file could unpack to any size.
    const LIMIT: u64 = 256 * 1024 * 1024;
    let mut json = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(&mut part, LIMIT), &mut json)?;
    drop(part);
    // Saved by another program since: its slides, not the deck kept.
    let written = match archive.by_name(WRITTEN_PART) {
        Ok(mut written) => {
            let mut text = String::new();
            std::io::Read::read_to_string(
                &mut std::io::Read::take(&mut written, LIMIT),
                &mut text,
            )?;
            Some(text)
        }
        Err(_) => None,
    };
    if let Some(text) = written {
        let wanted = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("slides")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        let names: Vec<String> = archive
            .file_names()
            .filter_map(|name| name.ok().map(|name| name.into_owned()))
            .filter(|name| {
                name == "ppt/presentation.xml"
                    || (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
            })
            .collect();
        let mut parts = Vec::new();
        for name in names {
            let mut entry = archive.by_name(&name)?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(&mut entry, LIMIT), &mut bytes)?;
            parts.push((name, bytes));
        }
        let now = slides_fingerprint(
            parts
                .iter()
                .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
        );
        if wanted.as_deref() != Some(now.as_str()) {
            return Ok(None);
        }
    }
    Ok(Some(serde_json::from_str(&json)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_package_that_opens_again() {
        let deck = super::super::raster::tests::sample("harbor");
        let dir = std::env::temp_dir().join(format!("pilothouse-pptx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("deck.pptx");
        write(&deck, None, &out).unwrap();
        let parts = package(&deck, None).unwrap();
        for name in [
            "[Content_Types].xml",
            "ppt/presentation.xml",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideLayouts/slideLayout12.xml",
            "ppt/slides/slide4.xml",
            "ppt/charts/chart2.xml",
            "ppt/theme/theme1.xml",
            DECK_PART,
        ] {
            assert!(parts.contains_key(name), "{name}");
        }
        // Every XML part is well-formed.
        for (name, bytes) in &parts {
            if name.ends_with(".xml") || name.ends_with(".rels") {
                let text = std::str::from_utf8(bytes).unwrap();
                roxmltree::Document::parse(text).unwrap_or_else(|err| panic!("{name}: {err}"));
            }
        }
        let back = read_deck(&out).unwrap().unwrap();
        assert_eq!(back, deck);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Writes sample decks for a look in PowerPoint or Keynote:
/// `SLIDES_PPTX=/some/dir cargo test -p ph-studio slides_pptx -- --ignored`.
#[cfg(test)]
mod look {
    #[test]
    #[ignore]
    fn slides_pptx() {
        let Ok(dir) = std::env::var("SLIDES_PPTX") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let media = dir.join("media");
        std::fs::create_dir_all(&media).unwrap();
        // A picture to put in.
        let picture = image::RgbaImage::from_fn(640, 400, |x, y| {
            image::Rgba([(x * 255 / 640) as u8, (y * 255 / 400) as u8, 180, 255])
        });
        picture.save(media.join("gradient.png")).unwrap();
        for theme in ["clean", "midnight", "paper"] {
            let mut deck = super::super::raster::tests::sample(theme);
            let op: super::super::ops::Op = serde_json::from_value(serde_json::json!({
                "op": "add_slide", "layout": "picture_caption", "title": "A picture", "subtitle": "With a caption under it",
                "notes": "Say what the picture shows.\nThen move on."
            }))
            .unwrap();
            let id = super::super::ops::apply(&mut deck, op).unwrap().unwrap();
            let slide = deck.slides.iter_mut().find(|slide| slide.id == id).unwrap();
            for element in &mut std::sync::Arc::make_mut(slide).elements {
                if let super::Kind::Picture(picture) = &mut element.kind {
                    picture.media = "gradient.png".into();
                }
            }
            deck.sections.push(super::Section {
                name: "Intro".into(),
                slide: deck.slides[0].id.clone(),
            });
            super::write(&deck, Some(&media), &dir.join(format!("{theme}.pptx"))).unwrap();
        }
    }
}
