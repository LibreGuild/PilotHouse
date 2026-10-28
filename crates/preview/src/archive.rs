//! What is inside an archive (ZIP and its kin: JAR, APK, wheel…; tar,
//! .tar.gz), listed as a table. Nothing is extracted.

use std::io::{Read, Seek as _};
use std::path::{Path, PathBuf};

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div,
};

use crate::byte_size;
use crate::table::{TableData, TableGrid};

/// Entries listed at most; a bigger archive says how many it has.
const MAX_ENTRIES: usize = 100_000;
/// How much of a compressed tar is unpacked to list it: listing reads past
/// every file's contents, and a small file can unpack to a huge one.
const MAX_UNPACKED: u64 = 2 * 1024 * 1024 * 1024;

pub struct ArchiveView {
    path: PathBuf,
    grid: Entity<TableGrid>,
    error: Option<SharedString>,
    loaded: bool,
    _load: Option<Task<()>>,
}

impl ArchiveView {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            grid: cx.new(|cx| TableGrid::new(window, cx)),
            error: None,
            loaded: false,
            _load: None,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let read = cx.background_spawn(async move { list(&path) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                this.loaded = true;
                match result {
                    Ok((data, note)) => {
                        this.error = None;
                        this.grid
                            .update(cx, |grid, cx| grid.set_data(data, note.map(Into::into), cx));
                    }
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }
}

/// The archive's entries, and a note when not all are listed.
pub fn list(path: &Path) -> anyhow::Result<(TableData, Option<String>)> {
    let mut magic = [0u8; 4];
    let read = std::fs::File::open(path)?.read(&mut magic)?;
    let magic = &magic[..read];
    if magic.starts_with(b"PK") {
        return list_zip(path);
    }
    let file = std::fs::File::open(path)?;
    if magic.starts_with(&[0x1f, 0x8b]) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".crate") {
            return list_tar(flate2::read::GzDecoder::new(file).take(MAX_UNPACKED));
        }
        // A single compressed file: its size is in the last four bytes (as
        // the remainder over 4 GB), so nothing is unpacked.
        let mut file = file;
        let mut trailer = [0u8; 4];
        file.seek(std::io::SeekFrom::End(-4))?;
        file.read_exact(&mut trailer)?;
        let shown = name.trim_end_matches(".gz").to_string();
        let size = byte_size(u32::from_le_bytes(trailer) as u64);
        return Ok((
            TableData {
                columns: vec!["Name".into(), "Size".into()],
                rows: vec![vec![shown.into(), size.into()]],
            },
            None,
        ));
    }
    list_tar(file)
}

fn list_zip(path: &Path) -> anyhow::Result<(TableData, Option<String>)> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let total = archive.len();
    let mut rows = Vec::new();
    for ix in 0..total.min(MAX_ENTRIES) {
        let entry = archive.by_index_raw(ix)?;
        let modified = entry
            .last_modified()
            .map(|time| {
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}",
                    time.year(),
                    time.month(),
                    time.day(),
                    time.hour(),
                    time.minute()
                )
            })
            .unwrap_or_default();
        rows.push(vec![
            String::from_utf8_lossy(entry.name_raw())
                .into_owned()
                .into(),
            if entry.is_dir() {
                "folder".into()
            } else {
                byte_size(entry.size()).into()
            },
            byte_size(entry.compressed_size()).into(),
            modified.into(),
        ]);
    }
    let note = (total > MAX_ENTRIES).then(|| format!("first {MAX_ENTRIES} of {total} entries"));
    Ok((
        TableData {
            columns: vec![
                "Name".into(),
                "Size".into(),
                "Packed".into(),
                "Modified".into(),
            ],
            rows,
        },
        note,
    ))
}

fn list_tar(reader: impl Read) -> anyhow::Result<(TableData, Option<String>)> {
    let mut archive = tar::Archive::new(reader);
    let mut rows = Vec::new();
    let mut more = false;
    for entry in archive.entries()? {
        if rows.len() >= MAX_ENTRIES {
            more = true;
            break;
        }
        // Cut short (past the unpacking limit, or a damaged end): what was
        // read is listed.
        let Ok(entry) = entry else {
            if rows.is_empty() {
                entry?;
            }
            more = true;
            break;
        };
        let header = entry.header();
        let name = entry.path()?.to_string_lossy().into_owned();
        let size = if header.entry_type().is_dir() {
            "folder".to_string()
        } else {
            byte_size(header.size().unwrap_or(0))
        };
        rows.push(vec![name.into(), size.into()]);
    }
    let note = more.then(|| format!("the first {} entries", rows.len()));
    Ok((
        TableData {
            columns: vec!["Name".into(), "Size".into()],
            rows,
        },
        note,
    ))
}

impl Render for ArchiveView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match (&self.error, self.loaded) {
            (Some(error), _) => crate::notice(format!("Could not read the archive: {error}"), cx),
            (None, false) => crate::notice("Reading the archive…", cx),
            (None, true) => div()
                .size_full()
                .child(self.grid.clone())
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn lists_zip_and_tar_gz() {
        let dir = std::env::temp_dir().join(format!("ph-archive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("a.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        writer
            .start_file("docs/readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"hello").unwrap();
        writer.finish().unwrap();
        let (data, note) = list(&zip_path).unwrap();
        assert_eq!(data.rows[0][0], "docs/readme.txt");
        assert_eq!(data.rows[0][1], "5 bytes");
        assert!(note.is_none());

        let tgz_path = dir.join("b.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&tgz_path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_cksum();
        builder
            .append_data(&mut header, "x.txt", &b"abc"[..])
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let (data, _) = list(&tgz_path).unwrap();
        assert_eq!(data.rows[0], ["x.txt", "3 bytes"]);

        let gz_path = dir.join("notes.txt.gz");
        let mut gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&gz_path).unwrap(),
            flate2::Compression::fast(),
        );
        gz.write_all(&[b'a'; 1000]).unwrap();
        gz.finish().unwrap();
        let (data, _) = list(&gz_path).unwrap();
        assert_eq!(data.rows[0], ["notes.txt", "1000 bytes"]);
    }
}
