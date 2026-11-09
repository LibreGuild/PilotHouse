//! Things put in the chat without typing: an image pasted from the
//! clipboard (a screenshot), files dragged from the Files tree, and files
//! or images dragged from the system. Each becomes an attachment of the
//! next message, for the user to send.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{ClipboardEntry, ExternalPaths, ImageFormat};
use ph_files::roots::Roots;

use super::*;

/// The largest text file attached by its content (others by their path).
const MAX_TEXT_FILE: u64 = 256 * 1024;
/// The most files one drop or paste attaches: each is read from disk and
/// becomes a chip, and a whole folder's worth is never what was meant.
const MAX_FILES: usize = 20;
/// The image kinds agents take as images; other images go as files.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

impl ChatPanel {
    /// ⌘V with an image on the clipboard and no text (a screenshot, an
    /// image copied in a browser): attached, saved in PilotHouse's
    /// captures. Returns whether it was an image (else the text pastes).
    pub(super) fn paste_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        // Files copied in Finder or Explorer: attached as dropped. Looked
        // for before text, since the clipboard holds their names as text too.
        if let Some(paths) = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
            _ => None,
        }) {
            self.attach_paths(&paths, window, cx);
            return true;
        }
        let has_text = item.entries().iter().any(
            |entry| matches!(entry, ClipboardEntry::String(text) if !text.text().trim().is_empty()),
        );
        if has_text {
            return false;
        }
        let Some((format, bytes)) = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some((image.format, image.bytes.clone())),
            _ => None,
        }) else {
            return false;
        };
        if format == ImageFormat::Svg {
            window.push_notification(
                Notification::info(
                    "A pasted SVG cannot go to an agent as an image. Save it as a file and drop the file here.",
                ),
                cx,
            );
            return true;
        }
        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_spawn(async move { save_pasted(format, bytes) })
                .await;
            _ = this.update_in(cx, |this, window, cx| match saved {
                Ok(path) => this.attach(
                    Attachment::Image {
                        label: "Pasted image".into(),
                        path,
                    },
                    window,
                    cx,
                ),
                Err(err) => window.push_notification(
                    Notification::error(format!("Could not attach the pasted image: {err:#}")),
                    cx,
                ),
            });
        })
        .detach();
        true
    }

    /// Rows dragged from the Files tree: attached as the workspace's files.
    pub(super) fn attach_tree_files(
        &mut self,
        dragged: &ph_files::DraggedFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for path in dragged.paths.iter() {
            self.attach(Attachment::File(path.clone()), window, cx);
        }
    }

    /// Files dragged from the system (or copied there): images as images,
    /// the workspace's files as its files, other text files by their content
    /// (when small), anything else by its path. Read off the UI thread.
    pub(super) fn attach_paths(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let roots = self.project.roots();
        let left_out = paths.len().saturating_sub(MAX_FILES);
        let paths: Vec<PathBuf> = paths.iter().take(MAX_FILES).cloned().collect();
        cx.spawn_in(window, async move |this, cx| {
            let attachments = cx
                .background_spawn(async move {
                    paths
                        .iter()
                        .map(|path| dropped(path, &roots))
                        .collect::<Vec<_>>()
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| {
                for attachment in attachments {
                    this.attach(attachment, window, cx);
                }
                if left_out > 0 {
                    window.push_notification(
                        Notification::info(format!(
                            "Attached the first {MAX_FILES} files; {left_out} more were left out."
                        )),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(super) fn drop_external(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.attach_paths(paths.paths(), window, cx);
    }
}

/// What a dropped `path` attaches as. Reads the disk.
fn dropped(path: &Path, roots: &Roots) -> Attachment {
    if is_image(path) {
        Attachment::Image {
            label: file_name(path),
            path: path.to_path_buf(),
        }
    } else if let Some(tree_path) = roots.path_of(path) {
        Attachment::File(tree_path)
    } else {
        Attachment::Text {
            label: file_name(path),
            text: outside_file(path),
        }
    }
}

/// Saves a pasted image in PilotHouse's captures: as it is when agents take
/// its kind, else as PNG (macOS often hands over TIFF).
fn save_pasted(format: ImageFormat, bytes: Vec<u8>) -> anyhow::Result<PathBuf> {
    let (extension, bytes) = match format {
        ImageFormat::Png => ("png", bytes),
        ImageFormat::Jpeg => ("jpg", bytes),
        ImageFormat::Webp => ("webp", bytes),
        ImageFormat::Gif => ("gif", bytes),
        _ => ("png", to_png(&bytes)?),
    };
    let dir = ph_store::paths::data_dir().join("captures");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis())
        .unwrap_or_default();
    let path = dir.join(format!("pasted-{stamp}.{extension}"));
    std::fs::write(&path, bytes).with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}

fn to_png(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let image = image::load_from_memory(bytes).context("reading the image")?;
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .context("making a PNG of it")?;
    Ok(png.into_inner())
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A file outside the workspace, for the agent: its text when small and
/// readable as text, else where it is.
fn outside_file(path: &Path) -> String {
    let small =
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_TEXT_FILE);
    if small && let Ok(text) = std::fs::read_to_string(path) {
        return format!("{}:\n\n{text}", path.display());
    }
    if path.is_dir() {
        format!("A folder the user dropped: {}", path.display())
    } else {
        format!("A file the user dropped: {}", path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_told_by_their_extension() {
        assert!(is_image(Path::new("/a/Shot.PNG")));
        assert!(!is_image(Path::new("/a/notes.md")));
        // Kinds agents do not take go as files.
        assert!(!is_image(Path::new("/a/scan.tiff")));
        let dir = std::env::temp_dir().join(format!("ph-dropped-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.txt");
        std::fs::write(&note, "hello").unwrap();
        assert!(outside_file(&note).ends_with("hello"));
        assert!(outside_file(&dir).starts_with("A folder"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_image_kinds_become_png() {
        let mut bmp = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(2, 2)
            .write_to(&mut bmp, image::ImageFormat::Bmp)
            .unwrap();
        let png = to_png(bmp.get_ref()).unwrap();
        assert_eq!(
            crate::attachments::image_media_type(&png),
            Some("image/png")
        );
        assert!(to_png(b"not an image").is_err());
    }
}
