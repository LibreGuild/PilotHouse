//! Finding the file a stack frame names (`Main.kt`, in class
//! `com.example.ui.Main`) among a workspace's folders.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use ph_devices::SourceRef;

/// The file named `source.file` under `folders`: the one in the folders of
/// the frame's package (`com/example/ui/`) when several have the name, else
/// the first found. Ignored files (build output) are not looked at.
pub(super) fn find_source(folders: &[PathBuf], source: &SourceRef) -> Option<PathBuf> {
    let package = package_path(source);
    let mut first = None;
    for folder in folders {
        let walker = WalkBuilder::new(folder)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build();
        for entry in walker.flatten() {
            if entry.file_name().to_string_lossy() != source.file.as_str()
                || !entry.file_type().is_some_and(|kind| kind.is_file())
            {
                continue;
            }
            let path = entry.into_path();
            if package
                .as_deref()
                .is_some_and(|package| in_package(&path, package))
            {
                return Some(path);
            }
            first.get_or_insert(path);
        }
    }
    first
}

/// The folders of the frame's class: `com/example/ui` for
/// `com.example.ui.Main$onCreate$1`.
fn package_path(source: &SourceRef) -> Option<String> {
    let class = source.class.as_deref()?;
    let (package, _) = class.rsplit_once('.')?;
    Some(package.replace('.', "/"))
}

/// Whether `path`'s folder ends with the folders `package` names.
fn in_package(path: &Path, package: &str) -> bool {
    path.parent().is_some_and(|folder| {
        folder
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with(package)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_in_the_frames_package_wins() {
        let root = std::env::temp_dir().join(format!("ph-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ui = root.join("app/src/main/java/com/example/ui");
        let other = root.join("app/src/main/java/com/example/a");
        std::fs::create_dir_all(&ui).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("Main.kt"), "").unwrap();
        std::fs::write(ui.join("Main.kt"), "").unwrap();
        let frame = |class: Option<&str>, file: &str| SourceRef {
            file: file.into(),
            line: 1,
            class: class.map(str::to_string),
        };
        let folders = [root.clone()];
        assert_eq!(
            find_source(
                &folders,
                &frame(Some("com.example.ui.Main$run$1"), "Main.kt")
            ),
            Some(ui.join("Main.kt"))
        );
        // No class, or one of another package: a file of that name still.
        assert!(find_source(&folders, &frame(None, "Main.kt")).is_some());
        assert!(find_source(&folders, &frame(Some("x.y.Main"), "Main.kt")).is_some());
        assert_eq!(find_source(&folders, &frame(None, "Looper.java")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
