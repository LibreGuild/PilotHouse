//! Changes not saved yet, kept on disk so they survive a crash or a quit.
//!
//! A second after the last keystroke, a changed file's text goes to
//! `<data dir>/unsaved/<hash>.txt` (its first line is the file's path, so a
//! hash collision is noticed). Saving the file, or reloading it unchanged,
//! deletes the copy; opening the file again with a copy there offers the
//! copy back, still unsaved.

use std::path::{Path, PathBuf};

/// The folder of kept changes.
fn folder() -> PathBuf {
    ph_store::paths::data_dir().join("unsaved")
}

/// The kept changes of `file` (absolute) in `folder`.
fn copy_path(folder: &Path, file: &Path) -> PathBuf {
    // FNV-1a: stable across runs and Rust versions, unlike the std hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in file.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    folder.join(format!("{hash:016x}.txt"))
}

/// Keeps `text`, the unsaved contents of `file`.
pub fn keep(file: &Path, text: &str) -> std::io::Result<()> {
    keep_in(&folder(), file, text)
}

/// The kept unsaved contents of `file`, if any.
pub fn kept(file: &Path) -> Option<String> {
    kept_in(&folder(), file)
}

/// Forgets `file`'s kept changes (it was saved, or they were discarded).
pub fn forget(file: &Path) {
    let _ = std::fs::remove_file(copy_path(&folder(), file));
}

fn keep_in(folder: &Path, file: &Path, text: &str) -> std::io::Result<()> {
    ph_store::paths::ensure_private_dir(folder)?;
    let mut contents = file.to_string_lossy().into_owned();
    contents.push('\n');
    contents.push_str(text);
    std::fs::write(copy_path(folder, file), contents)
}

fn kept_in(folder: &Path, file: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(copy_path(folder, file)).ok()?;
    let (path, text) = contents.split_once('\n')?;
    (path == file.to_string_lossy()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_kept_until_forgotten() {
        let folder = std::env::temp_dir().join(format!("ph-unsaved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let file = Path::new("/work/app/src/main.rs");
        assert_eq!(kept_in(&folder, file), None);
        keep_in(&folder, file, "fn main() {\n    // new\n}\n").unwrap();
        assert_eq!(
            kept_in(&folder, file).as_deref(),
            Some("fn main() {\n    // new\n}\n")
        );
        assert_eq!(
            kept_in(&folder, Path::new("/work/app/src/lib.rs")),
            None,
            "another file has none"
        );
        let _ = std::fs::remove_file(copy_path(&folder, file));
        assert_eq!(kept_in(&folder, file), None);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
