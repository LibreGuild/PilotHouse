//! The `studio.*` control methods behind the MCP bridge's `studio_*`
//! tools: an agent sees the picture or video open in Studio and changes it
//! as the user watches. Every change is one step the user can undo; nothing
//! is written until the agent (or the user) saves.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use gpui_kit::{App, AppContext as _, Entity, Task, WeakEntity};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::document::ImageDocument;
use crate::encode::{ExportSettings, Format};
use crate::video::document::VideoDocument;
use crate::video::{VideoExport, VideoFormat};

/// How positions and marks work, for the agent.
const GUIDE: &str = "\
Positions (crop, marks) are pixels of the original picture as turned (after any transform), whatever the crop, resize or frame; view \"original\" shows that picture with its scale, so divide what you see by the scale. \
Marks: rect, ellipse (x, y, width, height; color, stroke, fill, radius), arrow and line (from, to: [x, y]), pen (points), text (x, y = top left; text, size, color, background, bold, align: left, center or right), step (a numbered circle centered on x, y), highlight (see-through marker over an area), blur and pixelate (hide an area: emails, tokens, faces), spotlight (dims all but an area; dim 0-100). Sizes left out suit the picture. Colors are #rrggbb or #rrggbbaa. \
`set` changes: crop {x, y, width, height} or null; adjust {brightness, contrast, saturation: -100..100, grayscale, invert, blur 0-50, sharpen 0-100}; resize {width and/or height} (one keeps proportions) or null; frame {padding, background (#color, [#from, #to] gradient, or null for none), radius, shadow 0-100} or null. \
`transform`: rotate_left, rotate_right, flip_horizontal, flip_vertical. `export`: format (png, jpeg, webp, gif, bmp, tiff, ico), quality (JPEG, 1-100), max_kb (a size limit), palette (PNG of 256 colors).";

pub fn register_control_methods(cx: &mut App) {
    ControlServer::handle("studio.list", list, cx);
    ControlServer::handle_async("studio.get", get, cx);
    ControlServer::handle("studio.edit", edit, cx);
    ControlServer::handle("studio.undo", undo, cx);
    ControlServer::handle_async("studio.export", export, cx);
    crate::slides::control::register_control_methods(cx);
}

/// A document open in Studio.
#[derive(Clone)]
enum Open {
    Image(WeakEntity<ImageDocument>),
    Video(WeakEntity<VideoDocument>),
}

/// One found for an agent.
enum Found {
    Image(Entity<ImageDocument>),
    Video(Entity<VideoDocument>),
}

/// The documents open in Studio, for agents to find.
#[derive(Default)]
struct Documents(Vec<Open>);

impl gpui_kit::Global for Documents {}

impl Open {
    fn upgrade(&self) -> Option<Found> {
        match self {
            Open::Image(document) => document.upgrade().map(Found::Image),
            Open::Video(document) => document.upgrade().map(Found::Video),
        }
    }
}

impl Found {
    /// (project, path, last used)
    fn about(&self, cx: &App) -> (Option<i64>, std::path::PathBuf, std::time::Instant, bool) {
        match self {
            Found::Image(document) => {
                let document = document.read(cx);
                (
                    document.project(),
                    document.path().to_path_buf(),
                    document.used,
                    !document.edit().is_unchanged(),
                )
            }
            Found::Video(document) => {
                let document = document.read(cx);
                (
                    document.project(),
                    document.path().to_path_buf(),
                    document.used,
                    !document.edit().is_unchanged(),
                )
            }
        }
    }
}

fn keep(open: Open, cx: &mut App) {
    let documents = cx.default_global::<Documents>();
    documents.0.retain(|known| known.upgrade().is_some());
    documents.0.push(open);
}

/// Keeps `document` findable by agents while it is open.
pub(crate) fn register(document: WeakEntity<ImageDocument>, cx: &mut App) {
    keep(Open::Image(document), cx);
}

pub(crate) fn register_video(document: WeakEntity<VideoDocument>, cx: &mut App) {
    keep(Open::Video(document), cx);
}

/// The open documents of `project`.
fn open_in(project: i64, cx: &mut App) -> Vec<Found> {
    let documents = cx.default_global::<Documents>();
    documents.0.retain(|known| known.upgrade().is_some());
    let open: Vec<Found> = documents.0.iter().filter_map(Open::upgrade).collect();
    open.into_iter()
        .filter(|found| found.about(cx).0 == Some(project))
        .collect()
}

/// How Studio finds a workspace's folders: it does not know the store, so
/// the app says how (see [`set_workspace_folders`]).
struct Folders(Rc<FolderLookup>);

/// A workspace's folders by its id.
type FolderLookup = dyn Fn(i64, &App) -> Vec<PathBuf>;

impl gpui_kit::Global for Folders {}

/// Tells Studio how to find the folders of a workspace (by its id). An
/// agent's export may be written in them, as well as in the folder of the
/// file it works on; until this is set, only there.
pub fn set_workspace_folders(lookup: impl Fn(i64, &App) -> Vec<PathBuf> + 'static, cx: &mut App) {
    cx.set_global(Folders(Rc::new(lookup)));
}

/// The folders of workspace `project`, as they are now.
pub(crate) fn workspace_folders(project: Option<i64>, cx: &App) -> Vec<PathBuf> {
    let lookup = cx.try_global::<Folders>().map(|folders| folders.0.clone());
    project
        .zip(lookup)
        .map(|(project, lookup)| lookup(project, cx))
        .unwrap_or_default()
}

/// Where an agent asked for an export to be written. The path is the
/// agent's (or anything's on the control socket), so it is checked before
/// a byte is written: see [`ExportTo::checked`].
pub(crate) struct ExportTo {
    /// The path as given.
    pub given: PathBuf,
    /// Where a relative path starts.
    pub base: PathBuf,
    /// The file being worked on: its folder may be written in, the file
    /// itself is never written over this way.
    pub source: PathBuf,
    /// The workspace's folders, which may be written in too.
    pub folders: Vec<PathBuf>,
    /// Whether a file already there may be replaced.
    pub overwrite: bool,
    /// The answer when the path leads to `source` itself.
    pub is_source: &'static str,
}

impl ExportTo {
    /// The file to write, or why not. It is inside the folder of the file
    /// being worked on or one of the workspace's folders, whatever `..`,
    /// absolute path or link the given path goes through: folders are
    /// compared as the disk has them, not as they are spelled. Reads the
    /// disk, so this is for a background thread.
    pub(crate) fn checked(&self) -> Result<PathBuf> {
        // The folders that may be written in, and those for the refusal.
        let allowed = || {
            self.source
                .parent()
                .into_iter()
                .chain(self.folders.iter().map(PathBuf::as_path))
        };
        let may = || {
            let folders: Vec<String> = allowed()
                .map(|folder| folder.display().to_string())
                .collect();
            format!(
                "an agent may save only in {} (the user can save anywhere with Export)",
                folders.join(", ")
            )
        };
        // Before anything looks at the disk: on Windows, merely asking
        // about a path on another computer sends the user's sign-in there.
        if ph_ui::platform::is_network_path(&self.given.to_string_lossy()) {
            bail!("{} is on another computer; {}", self.given.display(), may());
        }
        let path = self.base.join(&self.given);
        let name = path
            .file_name()
            .with_context(|| format!("{} names no file", path.display()))?;
        // Folders that exist are resolved (links and `..` followed). Ones
        // still to be made are plain names below them, so they are where
        // they say.
        let mut folder = path
            .parent()
            .with_context(|| format!("{} has no folder", path.display()))?;
        let mut to_make = Vec::new();
        let mut target = loop {
            match dunce::canonicalize(folder) {
                Ok(real) => break real,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let (Some(name), Some(parent)) = (folder.file_name(), folder.parent()) else {
                        bail!("{} does not exist", folder.display());
                    };
                    to_make.push(name);
                    folder = parent;
                }
                Err(err) => bail!("could not look at {}: {err}", folder.display()),
            }
        };
        target.extend(to_make.into_iter().rev());
        target.push(name);
        // A file already there may be a link: what would be written over
        // is the file it leads to.
        let exists = std::fs::symlink_metadata(&target).is_ok();
        if exists {
            target = dunce::canonicalize(&target)
                .with_context(|| format!("{} is a link that leads nowhere", target.display()))?;
            if dunce::canonicalize(&self.source).is_ok_and(|source| source == target) {
                bail!("{}", self.is_source);
            }
        }
        let inside = allowed()
            .filter_map(|folder| dunce::canonicalize(folder).ok())
            .any(|folder| target.starts_with(folder));
        if !inside {
            bail!("not saved to {}: {}", target.display(), may());
        }
        if exists && target.is_dir() {
            bail!("{} is a folder", target.display());
        }
        if exists && !self.overwrite {
            bail!(
                "{} exists already; choose another name, or overwrite: true if the user asked",
                target.display()
            );
        }
        Ok(target)
    }
}

fn project(params: &Value) -> Result<i64> {
    params
        .get("project_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing project_id"))
}

/// The document an agent means: the one named `file` (a path or a file
/// name), else the one used last.
fn find(params: &Value, cx: &mut App) -> Result<Found> {
    let file = params.get("file").and_then(Value::as_str);
    let named = |path: &std::path::Path| match file {
        None => true,
        Some(file) => {
            path == std::path::Path::new(file)
                || path.ends_with(file)
                || path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy() == file)
        }
    };
    let found = open_in(project(params)?, cx)
        .into_iter()
        .filter(|found| named(&found.about(cx).1))
        .max_by_key(|found| found.about(cx).2)
        .ok_or_else(|| match file {
            Some(file) => anyhow!("{file} is not open in Studio"),
            None => anyhow!(
                "nothing is open in Studio for this workspace; ask the user to open a picture or video (Edit on it)"
            ),
        })?;
    match &found {
        Found::Image(document) => document.update(cx, |document, _| document.touch()),
        Found::Video(document) => document.update(cx, |document, _| document.touch()),
    }
    Ok(found)
}

/// What the picture is now, in words.
fn describe(document: &ImageDocument) -> Value {
    let size = document.size();
    let edit = document.edit();
    let (width, height) = edit.result_size(size);
    let (turned_w, turned_h) = edit.turned_size(size);
    json!({
        "file": document.path().to_string_lossy(),
        "original": { "width": turned_w, "height": turned_h },
        "result": { "width": width, "height": height },
        "edit": edit,
        "export": document.export(),
        "can_undo": document.can_undo(),
    })
}

fn list(params: Value, cx: &mut App) -> Result<Value> {
    let project = project(&params)?;
    let open: Vec<Value> = open_in(project, cx)
        .iter()
        .map(|found| {
            let (_, path, _, edited) = found.about(cx);
            let kind = match found {
                Found::Image(_) => "picture",
                Found::Video(_) => "video",
            };
            json!({ "file": path.to_string_lossy(), "kind": kind, "edited": edited })
        })
        .collect();
    let text = if open.is_empty() {
        "Nothing is open in Studio for this workspace.".to_string()
    } else {
        serde_json::to_string_pretty(&open)?
    };
    Ok(json!({ "text": text, "open": open }))
}

fn get(params: Value, cx: &mut App) -> Task<Result<Value>> {
    match find(&params, cx) {
        Ok(Found::Image(document)) => image_get(document, params, cx),
        Ok(Found::Video(document)) => video_get(document, params, cx),
        Err(err) => Task::ready(Err(err)),
    }
}

fn image_get(document: Entity<ImageDocument>, params: Value, cx: &mut App) -> Task<Result<Value>> {
    let original = params.get("view").and_then(Value::as_str) == Some("original");
    let side = params
        .get("max_side")
        .and_then(Value::as_u64)
        .map_or(1280, |side| side.clamp(256, 2048) as u32);
    let state = describe(document.read(cx));
    let snapshot = document.read(cx).snapshot(original, side, cx);
    cx.spawn(async move |_| {
        let (png, scale) = snapshot.await?;
        let shown = if original {
            "the original as turned"
        } else {
            "the result"
        };
        let text = format!(
            "{}\n\nThe image attached is {shown} at scale {scale:.4} (its pixels × {:.4} are the picture's).\n\n{GUIDE}",
            serde_json::to_string_pretty(&state)?,
            1. / scale.max(0.0001),
        );
        Ok(json!({
            "text": text,
            "images": [{ "data": base64::engine::general_purpose::STANDARD.encode(&png), "mimeType": "image/png" }],
        }))
    })
}

fn edit(params: Value, cx: &mut App) -> Result<Value> {
    match find(&params, cx)? {
        Found::Image(document) => image_edit(document, params, cx),
        Found::Video(document) => video_edit(document, params, cx),
    }
}

fn image_edit(document: Entity<ImageDocument>, params: Value, cx: &mut App) -> Result<Value> {
    let reply = document.update(cx, |document, cx| -> Result<String> {
        if document.original().is_none() {
            bail!("the picture is still opening; try again");
        }
        let mut done = Vec::new();
        if params.get("reset").and_then(Value::as_bool) == Some(true) {
            document.reset(cx);
            done.push("reset to the original".to_string());
        }
        let edits = ["set", "transform", "add", "update", "remove"];
        if edits.iter().any(|key| params.get(*key).is_some()) {
            let mut next = document.edit().clone();
            done.extend(next.apply(&params, document.size())?);
            document.settle();
            document.commit("agent", next, cx);
            document.settle();
        }
        if let Some(export) = params.get("export").filter(|export| !export.is_null()) {
            let mut value = serde_json::to_value(document.export())?;
            crate::edit::merge(&mut value, export);
            let settings: ExportSettings =
                serde_json::from_value(value).map_err(|err| anyhow!("export: {err}"))?;
            document.set_export(|current| *current = settings, cx);
            done.push("changed how it saves".into());
        }
        if done.is_empty() {
            bail!("nothing to change: give set, transform, add, update, remove, export or reset");
        }
        Ok(done.join("; "))
    })?;
    let state = describe(document.read(cx));
    Ok(json!({
        "text": format!("Done: {reply}. The user sees it now.\n{}", serde_json::to_string_pretty(&state)?),
    }))
}

fn undo(params: Value, cx: &mut App) -> Result<Value> {
    let redo = params.get("redo").and_then(Value::as_bool) == Some(true);
    let changed = match find(&params, cx)? {
        Found::Image(document) => document.update(cx, |document, cx| {
            let can = if redo {
                document.can_redo()
            } else {
                document.can_undo()
            };
            if redo {
                document.redo(cx)
            } else {
                document.undo(cx)
            }
            can
        }),
        Found::Video(document) => document.update(cx, |document, cx| {
            let can = if redo {
                document.can_redo()
            } else {
                document.can_undo()
            };
            if redo {
                document.redo(cx)
            } else {
                document.undo(cx)
            }
            can
        }),
    };
    let text = match (changed, redo) {
        (true, false) => "Undone.",
        (true, true) => "Redone.",
        (false, false) => "Nothing to undo.",
        (false, true) => "Nothing to redo.",
    };
    Ok(json!({ "text": text }))
}

fn export(params: Value, cx: &mut App) -> Task<Result<Value>> {
    match find(&params, cx) {
        Ok(Found::Image(document)) => image_export(document, params, cx),
        Ok(Found::Video(document)) => video_export(document, params, cx),
        Err(err) => Task::ready(Err(err)),
    }
}

fn image_export(
    document: Entity<ImageDocument>,
    params: Value,
    cx: &mut App,
) -> Task<Result<Value>> {
    if params.get("replace_original").and_then(Value::as_bool) == Some(true) {
        let saving = document.update(cx, |document, cx| document.replace_original(cx));
        return cx.spawn(async move |_| {
            let replaced = saving.await?;
            let size = kilobytes(replaced.encoded.bytes.len());
            let text = if replaced.changed_since {
                format!(
                    "Replaced the original ({size}). The picture was changed while the file was written: those changes are not in the file. They are still in Studio, not saved; replace again to write them."
                )
            } else {
                format!("Replaced the original ({size}). Studio starts again from it.")
            };
            Ok(json!({ "text": text }))
        });
    }
    let doc = document.read(cx);
    let source = doc.path().to_path_buf();
    let mut settings = doc.export().clone();
    let Some(path) = params.get("path").and_then(Value::as_str) else {
        return Task::ready(Err(anyhow!(
            "give `path` (a new file; relative to the picture's folder) or replace_original: true"
        )));
    };
    let mut path = PathBuf::from(path);
    match path
        .extension()
        .and_then(|extension| Format::from_extension(&extension.to_string_lossy()))
    {
        Some(format) => settings.format = format,
        None => {
            path.set_extension(settings.format.extension());
        }
    }
    let replace = params.get("overwrite").and_then(Value::as_bool) == Some(true);
    let wanted = ExportTo {
        given: path,
        base: source.parent().map(PathBuf::from).unwrap_or_default(),
        folders: workspace_folders(doc.project(), cx),
        overwrite: replace,
        is_source: "that is the original; use replace_original: true if the user asked to overwrite it",
        source,
    };
    cx.spawn(async move |cx| {
        let path = cx.background_spawn(async move { wanted.checked() }).await?;
        let saving = document.update(cx, |document, cx| {
            document.save(path.clone(), settings, replace, cx)
        });
        let encoded = saving.await?;
        let note = encoded
            .note
            .map(|note| format!(" To fit: {note}."))
            .unwrap_or_default();
        Ok(json!({
            "text": format!(
                "Saved {} ({} × {}, {}).{note}",
                path.display(),
                encoded.width,
                encoded.height,
                kilobytes(encoded.bytes.len())
            ),
        }))
    })
}

pub(crate) fn kilobytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    } else {
        format!("{} KB", (bytes as f64 / 1024.).ceil() as u64)
    }
}

/// How videos work, for the agent.
const VIDEO_GUIDE: &str = "\
Times are seconds of the source video (`at` in studio_get, marks' start and end, cut/trim/speed ranges), so they stay on the same moments whatever is cut; the timeline shows where each kept part lands in the result. \
Positions are pixels of the source frame (upright), whatever the crop or size. \
studio_edit for a video: `cut` [{from, to}] removes parts; `trim` {from, to} keeps only that; `speed` [{from, to, speed}] plays a part faster (sped-up parts are silent); `split` [t]; `reset_timeline`. \
Marks are `add`ed with `start`, `end` and optional `fade` (seconds) plus any picture mark: rect, ellipse, arrow, line, pen, text, step, highlight, blur and pixelate (hide emails, tokens, faces), spotlight. `update` and `remove` by id. \
`set`: crop {x, y, width, height} or null; resize {width and/or height} or null; mute. `export`: format (mp4, hevc, gif, webm), max_mb (a size limit, e.g. 50 for Confluence), fps, gif_width. \
Look at frames with studio_get `at`: [seconds] (up to 6) to find where things happen before cutting or marking.";

fn describe_video(document: &VideoDocument) -> Value {
    let Some(info) = document.info() else {
        return json!({ "file": document.path().to_string_lossy(), "state": "opening" });
    };
    let edit = document.edit();
    let mut at = 0.;
    let parts: Vec<Value> = edit
        .pieces(info.duration)
        .iter()
        .map(|piece| {
            let part = json!({
                "from": round(piece.start), "to": round(piece.end), "speed": piece.speed,
                "result_from": round(at), "result_to": round(at + piece.length()),
            });
            at += piece.length();
            part
        })
        .collect();
    let (width, height) = edit.output_size(info.size);
    json!({
        "file": document.path().to_string_lossy(),
        "source": {
            "seconds": round(info.duration), "width": info.size.0, "height": info.size.1,
            "fps": round(info.fps), "sound": info.audio, "frames": document.frames().len(),
        },
        "result": { "seconds": round(edit.output_duration(info.duration)), "width": width, "height": height },
        "kept": parts,
        "crop": edit.crop,
        "resize": edit.resize,
        "mute": edit.mute,
        "marks": edit.overlays,
        "export": document.export(),
        "can_undo": document.can_undo(),
    })
}

fn round(value: f64) -> f64 {
    (value * 1000.).round() / 1000.
}

fn video_get(document: Entity<VideoDocument>, params: Value, cx: &mut App) -> Task<Result<Value>> {
    let original = params.get("view").and_then(Value::as_str) == Some("original");
    let side = params
        .get("max_side")
        .and_then(Value::as_u64)
        .map_or(768, |side| side.clamp(256, 1600) as u32);
    let doc = document.read(cx);
    let Some(info) = doc.info().cloned() else {
        return Task::ready(Err(anyhow!("the video is still opening; try again")));
    };
    // The moments asked for, else four across the result.
    let times: Vec<f64> = match params.get("at").and_then(Value::as_array) {
        Some(times) => times.iter().filter_map(Value::as_f64).take(6).collect(),
        None => {
            let length = doc.edit().output_duration(info.duration);
            (0..4)
                .map(|ix| {
                    doc.edit()
                        .to_source(info.duration, length * (ix as f64 + 0.5) / 4.)
                        .0
                })
                .collect()
        }
    };
    let state = describe_video(doc);
    let shots: Vec<_> = times
        .iter()
        .map(|t| (*t, doc.snapshot(*t, original, side, cx)))
        .collect();
    cx.spawn(async move |_| {
        let mut images = Vec::new();
        let mut captions = Vec::new();
        for (t, shot) in shots {
            let png = shot.await?;
            captions.push(format!("{t:.3}s"));
            images.push(json!({ "data": base64::engine::general_purpose::STANDARD.encode(&png), "mimeType": "image/png" }));
        }
        let shown = if original { "the source" } else { "the result" };
        let text = format!(
            "{}\n\nThe images are {shown} at source times {}, in order.\n\n{VIDEO_GUIDE}",
            serde_json::to_string_pretty(&state)?,
            captions.join(", ")
        );
        Ok(json!({ "text": text, "images": images }))
    })
}

fn video_edit(document: Entity<VideoDocument>, params: Value, cx: &mut App) -> Result<Value> {
    let reply = document.update(cx, |document, cx| -> Result<String> {
        let Some(info) = document.info().cloned() else {
            bail!("the video is still opening; try again");
        };
        let mut done = Vec::new();
        if params.get("reset").and_then(Value::as_bool) == Some(true) {
            document.reset(cx);
            done.push("back to the source".to_string());
        }
        let edits = ["set", "cut", "trim", "speed", "split", "reset_timeline", "add", "update", "remove"];
        if edits.iter().any(|key| params.get(*key).is_some()) {
            let mut next = document.edit().clone();
            done.extend(next.apply(&params, info.duration, info.size)?);
            document.settle();
            document.commit("agent", next, cx);
            document.settle();
        }
        if let Some(export) = params.get("export").filter(|export| !export.is_null()) {
            let mut value = serde_json::to_value(document.export())?;
            crate::edit::merge(&mut value, export);
            let settings: VideoExport =
                serde_json::from_value(value).map_err(|err| anyhow!("export: {err}"))?;
            if !crate::video::export::writable(settings.format) {
                bail!(
                    "{} cannot be written on this system (ffmpeg, which could, is not installed); use mp4",
                    settings.format.label()
                );
            }
            document.set_export(|current| *current = settings, cx);
            done.push("changed how it saves".into());
        }
        if done.is_empty() {
            bail!("nothing to change: give cut, trim, speed, split, set, add, update, remove, export or reset");
        }
        Ok(done.join("; "))
    })?;
    let state = describe_video(document.read(cx));
    Ok(json!({
        "text": format!("Done: {reply}. The user sees it now.\n{}", serde_json::to_string_pretty(&state)?),
    }))
}

fn video_export(
    document: Entity<VideoDocument>,
    params: Value,
    cx: &mut App,
) -> Task<Result<Value>> {
    if params.get("replace_original").and_then(Value::as_bool) == Some(true) {
        return Task::ready(Err(anyhow!(
            "a video is never written over; give `path` for a new file"
        )));
    }
    let source = document.read(cx).path().to_path_buf();
    let Some(path) = params.get("path").and_then(Value::as_str) else {
        return Task::ready(Err(anyhow!(
            "give `path`: a new file, relative to the video's folder"
        )));
    };
    let mut path = PathBuf::from(path);
    let format = document.read(cx).export().format;
    match path
        .extension()
        .and_then(|extension| VideoFormat::from_extension(&extension.to_string_lossy()))
    {
        Some(VideoFormat::Gif) if format != VideoFormat::Gif => document
            .update(cx, |document, cx| {
                document.set_export(|e| e.format = VideoFormat::Gif, cx)
            }),
        Some(VideoFormat::Webm) if format != VideoFormat::Webm => document
            .update(cx, |document, cx| {
                document.set_export(|e| e.format = VideoFormat::Webm, cx)
            }),
        Some(VideoFormat::Mp4) if matches!(format, VideoFormat::Gif | VideoFormat::Webm) => {
            document.update(cx, |document, cx| {
                document.set_export(|e| e.format = VideoFormat::Mp4, cx)
            })
        }
        Some(_) => {}
        None => {
            path.set_extension(format.extension());
        }
    }
    let wanted = ExportTo {
        given: path,
        base: source.parent().map(PathBuf::from).unwrap_or_default(),
        folders: workspace_folders(document.read(cx).project(), cx),
        overwrite: params.get("overwrite").and_then(Value::as_bool) == Some(true),
        is_source: "that is the source; a video is never written over",
        source,
    };
    cx.spawn(async move |cx| {
        let path = cx.background_spawn(async move { wanted.checked() }).await?;
        let exporting = document.update(cx, |document, cx| document.start_export(path, cx));
        let done = exporting.await.map_err(|err| anyhow!(err))?;
        Ok(json!({
            "text": format!(
                "Saved {} ({} × {}, {:.1} s, {}).",
                done.path.display(),
                done.size.0,
                done.size.1,
                done.seconds,
                kilobytes(done.bytes as usize)
            ),
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own for one test: `work/shots/photo.png` (the file
    /// being edited), `work/docs` and `elsewhere/secret.png`.
    fn folders(test: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ph-studio-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("work/shots")).unwrap();
        std::fs::create_dir_all(root.join("work/docs")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::fs::write(root.join("work/shots/photo.png"), b"original").unwrap();
        std::fs::write(root.join("elsewhere/secret.png"), b"secret").unwrap();
        dunce::canonicalize(&root).unwrap()
    }

    fn export(
        root: &std::path::Path,
        given: &str,
        workspace: bool,
        overwrite: bool,
    ) -> Result<PathBuf> {
        ExportTo {
            given: PathBuf::from(given),
            base: root.join("work/shots"),
            source: root.join("work/shots/photo.png"),
            folders: if workspace {
                vec![root.join("work")]
            } else {
                Vec::new()
            },
            overwrite,
            is_source: "that is the original",
        }
        .checked()
    }

    #[test]
    fn an_export_stays_in_the_files_folder_or_the_workspace() {
        let root = folders("export-inside");
        let shots = root.join("work/shots");
        assert_eq!(
            export(&root, "copy.png", false, false).unwrap(),
            shots.join("copy.png")
        );
        // A folder still to be made, below one that is allowed.
        assert_eq!(
            export(&root, "out/small/copy.png", false, false).unwrap(),
            shots.join("out/small/copy.png")
        );
        // Out of the picture's folder: only where the workspace reaches.
        assert!(export(&root, "../docs/copy.png", false, false).is_err());
        assert_eq!(
            export(&root, "../docs/copy.png", true, false).unwrap(),
            root.join("work/docs/copy.png")
        );
        let outside = root.join("elsewhere/new.png");
        for workspace in [false, true] {
            assert!(export(&root, "../../elsewhere/new.png", workspace, true).is_err());
            assert!(export(&root, &outside.to_string_lossy(), workspace, true).is_err());
            assert!(export(&root, "missing/../../../elsewhere/new.png", workspace, true).is_err());
        }
        let refused = export(&root, "//server/share/copy.png", true, true).unwrap_err();
        assert!(
            refused.to_string().contains("another computer"),
            "{refused}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_export_never_writes_over_the_file_being_edited() {
        let root = folders("export-source");
        for given in ["photo.png", "out/../photo.png", "../shots/photo.png"] {
            std::fs::create_dir_all(root.join("work/shots/out")).unwrap();
            let refused = export(&root, given, true, true).unwrap_err();
            assert_eq!(refused.to_string(), "that is the original", "{given}");
        }
        // Another file there is replaced only when asked.
        std::fs::write(root.join("work/shots/copy.png"), b"copy").unwrap();
        assert!(export(&root, "copy.png", false, false).is_err());
        assert!(export(&root, "copy.png", false, true).is_ok());
        assert!(export(&root, "out", false, true).is_err(), "a folder");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_export_does_not_leave_through_a_link() {
        let root = folders("export-link");
        let shots = root.join("work/shots");
        // A folder that is a link out, and a file that is one.
        std::os::unix::fs::symlink(root.join("elsewhere"), shots.join("away")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere/secret.png"), shots.join("alias.png"))
            .unwrap();
        assert!(export(&root, "away/new.png", true, true).is_err());
        assert!(export(&root, "alias.png", true, true).is_err());
        assert_eq!(
            std::fs::read(root.join("elsewhere/secret.png")).unwrap(),
            b"secret"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
