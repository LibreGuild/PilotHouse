//! Any other binary file, as hex and text, sixteen bytes a row, with what
//! its first bytes say it is.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, UniformListScrollHandle, Window, div, uniform_list,
};

use crate::byte_size;

/// Bytes read for the view; a larger file shows its start.
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const PER_ROW: usize = 16;

struct Loaded {
    bytes: Arc<Vec<u8>>,
    size: u64,
}

pub struct HexView {
    path: PathBuf,
    loaded: Option<Result<Loaded, SharedString>>,
    scroll: UniformListScrollHandle,
    _load: Option<Task<()>>,
}

impl HexView {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            loaded: None,
            scroll: UniformListScrollHandle::new(),
            _load: None,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let read = cx.background_spawn(async move {
            use std::io::Read as _;
            let file = std::fs::File::open(&path)?;
            let size = file.metadata()?.len();
            let mut bytes = Vec::new();
            file.take(MAX_BYTES).read_to_end(&mut bytes)?;
            std::io::Result::Ok(Loaded {
                bytes: Arc::new(bytes),
                size,
            })
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                this.loaded = Some(result.map_err(|err| err.to_string().into()));
                cx.notify();
            });
        }));
    }
}

/// What a file's first bytes say it is, when they say.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    const SIGNATURES: &[(&[u8], &str)] = &[
        (b"\x7fELF", "ELF executable"),
        (b"\xcf\xfa\xed\xfe", "Mach-O executable"),
        (b"\xce\xfa\xed\xfe", "Mach-O executable"),
        (b"\xca\xfe\xba\xbe", "Mach-O universal binary or Java class"),
        (b"MZ", "Windows executable"),
        (b"\0asm", "WebAssembly module"),
        (b"PK\x03\x04", "ZIP archive"),
        (b"\x1f\x8b", "gzip data"),
        (b"%PDF", "PDF document"),
        (b"SQLite format 3\0", "SQLite database"),
        (b"\x89PNG", "PNG image"),
        (b"\xff\xd8\xff", "JPEG image"),
        (b"GIF8", "GIF image"),
        (b"7z\xbc\xaf\x27\x1c", "7-Zip archive"),
        (b"Rar!", "RAR archive"),
        (b"\x28\xb5\x2f\xfd", "Zstandard data"),
        (b"BZh", "bzip2 data"),
        (b"\xfd7zXZ\0", "XZ data"),
        (b"OggS", "Ogg media"),
        (b"fLaC", "FLAC audio"),
        (b"ID3", "MP3 audio"),
        (b"RIFF", "RIFF media (WAV, AVI or WebP)"),
        (b"\x1a\x45\xdf\xa3", "Matroska or WebM video"),
        (b"wOFF", "WOFF font"),
        (b"wOF2", "WOFF2 font"),
        (b"bplist", "Binary property list"),
        (b"dex\n", "Android DEX file"),
    ];
    if bytes.len() > 8 && &bytes[4..8] == b"ftyp" {
        return Some("MP4 / QuickTime media");
    }
    SIGNATURES
        .iter()
        .find(|(magic, _)| bytes.starts_with(magic))
        .map(|(_, name)| *name)
}

/// One row: offset, the bytes in hex, and as text.
pub fn row_text(bytes: &[u8], row: usize) -> (String, String, String) {
    let start = row * PER_ROW;
    let chunk = &bytes[start.min(bytes.len())..(start + PER_ROW).min(bytes.len())];
    let mut hex = String::with_capacity(PER_ROW * 3 + 1);
    for (ix, byte) in chunk.iter().enumerate() {
        if ix == PER_ROW / 2 {
            hex.push(' ');
        }
        hex.push_str(&format!("{byte:02x} "));
    }
    let text = chunk
        .iter()
        .map(|&byte| {
            if (0x20..0x7f).contains(&byte) {
                byte as char
            } else {
                '·'
            }
        })
        .collect();
    (format!("{start:08x}"), hex, text)
}

impl Render for HexView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loaded = match &self.loaded {
            None => return crate::notice("Reading…", cx),
            Some(Err(err)) => return crate::notice(format!("Could not read the file: {err}"), cx),
            Some(Ok(loaded)) => loaded,
        };
        let theme = cx.theme();
        let bytes = loaded.bytes.clone();
        let rows = bytes.len().div_ceil(PER_ROW);
        let mut summary = format!("Binary file · {}", byte_size(loaded.size));
        if let Some(kind) = sniff(&bytes) {
            summary = format!("{kind} · {}", byte_size(loaded.size));
        }
        if loaded.size > bytes.len() as u64 {
            summary.push_str(&format!(
                " · showing the first {}",
                byte_size(bytes.len() as u64)
            ));
        }
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_none()
                    .h_8()
                    .px_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(summary),
            )
            .child(
                uniform_list(
                    "hex-rows",
                    rows,
                    move |range: Range<usize>, _, cx: &mut App| {
                        let theme = cx.theme();
                        range
                            .map(|row| {
                                let (offset, hex, text) = row_text(&bytes, row);
                                h_flex()
                                    .px_3()
                                    .gap_4()
                                    .whitespace_nowrap()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_sm()
                                    .child(div().text_color(theme.muted_foreground).child(offset))
                                    .child(div().child(hex))
                                    .child(div().text_color(theme.muted_foreground).child(text))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0()
                .py_1(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_kinds() {
        let bytes: Vec<u8> = (0u8..20).chain(*b"Hi!").collect();
        let (offset, hex, text) = row_text(&bytes, 1);
        assert_eq!(offset, "00000010");
        assert_eq!(hex, "10 11 12 13 48 69 21 ");
        assert_eq!(text, "····Hi!");
        assert_eq!(sniff(b"\x7fELF\x02"), Some("ELF executable"));
        assert_eq!(sniff(b"\0\0\0\x18ftypmp42"), Some("MP4 / QuickTime media"));
        assert_eq!(sniff(b"hello"), None);
    }
}
