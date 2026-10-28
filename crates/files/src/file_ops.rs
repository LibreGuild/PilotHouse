//! File and folder operations behind the Files tree's menu: new, rename,
//! duplicate, paste (copy or move) and move to the trash.
//!
//! Nothing here deletes for good: removing goes to the system trash, and a
//! name that is taken is refused (or, for a copy, numbered) rather than
//! overwritten.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// Checks a name typed for a new file or folder. `nested` allows
/// `a/b/c.txt`, whose folders are created too.
pub fn check_name(name: &str, nested: bool) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name.");
    }
    if !nested && (name.contains('/') || name.contains('\\')) {
        return Err("A name cannot contain / or \\.");
    }
    if name.contains('\0') {
        return Err("A name cannot contain that character.");
    }
    if cfg!(windows) && name.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return Err("A name cannot contain < > : \" | ? or *.");
    }
    // Only plain parts: no `..`, no leading `/`, no drive.
    let plain = Path::new(name)
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    if !plain || name.split(['/', '\\']).any(|part| part.is_empty()) {
        return Err("Use a plain name, without `..` or empty parts.");
    }
    Ok(())
}

/// Creates the empty file `name` in `dir` (with its folders, for a nested
/// name). Refuses a name that is taken.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = target(dir, name, true)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("creating {}", path.display()))?;
    Ok(path)
}

/// Creates the folder `name` in `dir` (with its parents, for a nested name).
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = target(dir, name, true)?;
    fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    Ok(path)
}

/// Renames `path` to `new_name` in the same folder. Changing only the case
/// works on file systems that ignore it.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    let dir = path.parent().context("cannot rename this")?;
    check_name(new_name, false).map_err(anyhow::Error::msg)?;
    let to = dir.join(new_name.trim());
    if to == path {
        return Ok(to);
    }
    if to.symlink_metadata().is_ok() && !same_file(path, &to) {
        bail!("{} already exists here", new_name.trim());
    }
    fs::rename(path, &to).with_context(|| format!("renaming {}", path.display()))?;
    Ok(to)
}

/// Copies `path` beside itself as "name copy.ext" (then "name copy 2.ext").
pub fn duplicate(path: &Path) -> Result<PathBuf> {
    let dir = path.parent().context("cannot duplicate this")?;
    let name = file_name(path)?;
    let to = dir.join(copy_name(dir, &name));
    copy_all(path, &to)?;
    Ok(to)
}

/// Copies (or, with `cut`, moves) `sources` into `dir`. A copy whose name is
/// taken gets a numbered "copy" name; a move onto a taken name is refused,
/// as is putting a folder inside itself. Returns the new paths.
pub fn paste(sources: &[PathBuf], dir: &Path, cut: bool) -> Result<Vec<PathBuf>> {
    let mut pasted = Vec::new();
    for source in sources {
        if dir.starts_with(source) {
            bail!("cannot put {} inside itself", source.display());
        }
        let name = file_name(source)?;
        let mut to = dir.join(&name);
        if cut {
            if to == *source {
                pasted.push(to);
                continue;
            }
            if to.symlink_metadata().is_ok() {
                bail!("{name} already exists in {}", dir.display());
            }
            move_path(source, &to)?;
        } else {
            if to.symlink_metadata().is_ok() {
                to = dir.join(copy_name(dir, &name));
            }
            copy_all(source, &to)?;
        }
        pasted.push(to);
    }
    Ok(pasted)
}

/// Moves `paths` to the system trash, where they can be put back.
pub fn trash(paths: &[PathBuf]) -> Result<()> {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    // The system call, not a script asking Finder: that needs permission
    // to control Finder, which macOS asks the user for.
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos as _};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context
        .delete_all(paths)
        .map_err(|err| anyhow::anyhow!("moving to the trash failed: {err}"))
}

/// "name copy.ext", or the first free "name copy N.ext", in `dir`.
pub fn copy_name(dir: &Path, name: &str) -> String {
    // A leading dot is part of the name (".env copy"), not an extension.
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            format!("{stem} copy{extension}")
        } else {
            format!("{stem} copy {n}{extension}")
        };
        if dir.join(&candidate).symlink_metadata().is_err() {
            return candidate;
        }
        n += 1;
    }
}

fn target(dir: &Path, name: &str, nested: bool) -> Result<PathBuf> {
    check_name(name, nested).map_err(anyhow::Error::msg)?;
    let path = dir.join(name.trim());
    if path.symlink_metadata().is_ok() {
        bail!("{} already exists", name.trim());
    }
    Ok(path)
}

fn file_name(path: &Path) -> Result<String> {
    Ok(path
        .file_name()
        .with_context(|| format!("{} has no name", path.display()))?
        .to_string_lossy()
        .into_owned())
}

/// Whether two paths are one file (a case-only rename on macOS or Windows).
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Renames, or copies then removes across file systems.
fn move_path(from: &Path, to: &Path) -> Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            copy_all(from, to)?;
            if from.is_dir() {
                fs::remove_dir_all(from)
            } else {
                fs::remove_file(from)
            }
            .with_context(|| format!("removing {} after copying it", from.display()))
        }
        Err(err) => Err(err).with_context(|| format!("moving {}", from.display())),
    }
}

/// Copies a file, a link (as a link) or a whole folder.
fn copy_all(from: &Path, to: &Path) -> Result<()> {
    let meta = from
        .symlink_metadata()
        .with_context(|| format!("reading {}", from.display()))?;
    if meta.is_symlink() {
        let link = fs::read_link(from)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link, to)?;
        #[cfg(windows)]
        {
            if from.is_dir() {
                std::os::windows::fs::symlink_dir(&link, to)?;
            } else {
                std::os::windows::fs::symlink_file(&link, to)?;
            }
        }
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir(to).with_context(|| format!("creating {}", to.display()))?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ph-file-ops-{name}-{}", std::process::id()));
        _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_must_be_plain() {
        assert!(check_name("main.rs", false).is_ok());
        assert!(check_name("src/new/mod.rs", true).is_ok());
        assert!(check_name("src/mod.rs", false).is_err());
        assert!(check_name("  ", false).is_err());
        assert!(check_name("..", true).is_err());
        assert!(check_name("a/../b", true).is_err());
        assert!(check_name("/etc/passwd", true).is_err());
        assert!(check_name("a//b", true).is_err());
    }

    #[test]
    fn creates_nested_files_and_refuses_taken_names() {
        let dir = temp("create");
        let file = create_file(&dir, "src/lib.rs").unwrap();
        assert!(file.is_file());
        assert!(create_file(&dir, "src/lib.rs").is_err());
        assert!(create_dir(&dir, "src").is_err());
        assert!(create_dir(&dir, "docs/api").unwrap().is_dir());
    }

    #[test]
    fn copies_get_free_names() {
        let dir = temp("copy-name");
        assert_eq!(copy_name(&dir, "a.txt"), "a copy.txt");
        fs::write(dir.join("a copy.txt"), "").unwrap();
        assert_eq!(copy_name(&dir, "a.txt"), "a copy 2.txt");
        assert_eq!(copy_name(&dir, ".env"), ".env copy");
        assert_eq!(copy_name(&dir, "Makefile"), "Makefile copy");
    }

    #[test]
    fn duplicates_folders_with_their_contents() {
        let dir = temp("duplicate");
        create_file(&dir, "pkg/inner/x.txt").unwrap();
        let copy = duplicate(&dir.join("pkg")).unwrap();
        assert_eq!(copy, dir.join("pkg copy"));
        assert!(copy.join("inner/x.txt").is_file());
    }

    #[test]
    fn renames_and_refuses_taken_names() {
        let dir = temp("rename");
        let a = create_file(&dir, "a.txt").unwrap();
        create_file(&dir, "b.txt").unwrap();
        assert!(rename(&a, "b.txt").is_err());
        assert!(rename(&a, "sub/c.txt").is_err());
        let c = rename(&a, "c.txt").unwrap();
        assert!(c.is_file() && !a.exists());
    }

    #[test]
    fn pastes_copies_and_moves() {
        let dir = temp("paste");
        let file = create_file(&dir, "one.txt").unwrap();
        let folder = create_dir(&dir, "into").unwrap();
        // A copy into its own folder is numbered.
        let copied = paste(std::slice::from_ref(&file), &dir, false).unwrap();
        assert_eq!(copied, [dir.join("one copy.txt")]);
        let moved = paste(std::slice::from_ref(&file), &folder, true).unwrap();
        assert_eq!(moved, [folder.join("one.txt")]);
        assert!(!file.exists());
        // A folder never goes inside itself.
        assert!(paste(std::slice::from_ref(&dir), &folder, false).is_err());
        // A move onto a taken name is refused.
        let again = create_file(&dir, "one.txt").unwrap();
        assert!(paste(&[again], &folder, true).is_err());
    }
}
