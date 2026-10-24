//! Checkpoints: snapshots of the working tree taken before each agent turn,
//! so the files can be put back as they were.
//!
//! A checkpoint is an ordinary commit of every tracked and untracked file
//! (ignored files are left out), kept under `refs/pilothouse/checkpoints/`.
//! It is built in a temporary index, so HEAD, the real index, branches and
//! the stash are never touched, and it appears in no branch's history.
//! Restoring writes the checkpoint's files over the working tree (the index
//! still untouched) after taking one more checkpoint, so a restore can be
//! undone the same way.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail};

use crate::run::command;

/// Refs under this prefix hold the checkpoints.
const REFS: &str = "refs/pilothouse/checkpoints";
/// Newest checkpoints kept per repository; older refs are deleted (their
/// objects go with the next `git gc`).
const KEEP: usize = 200;
/// Untracked files bigger than this are left out of a checkpoint (a stray
/// dump or build artifact would bloat the repository).
const MAX_UNTRACKED_BYTES: u64 = 20 << 20;
/// More untracked files than this (an unignored `node_modules`) are left
/// out too.
const MAX_UNTRACKED_FILES: usize = 20_000;

/// The author of checkpoint commits, so they work without a configured
/// `user.name`.
const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "PilotHouse"),
    ("GIT_AUTHOR_EMAIL", "checkpoints@pilothouse.invalid"),
    ("GIT_COMMITTER_NAME", "PilotHouse"),
    ("GIT_COMMITTER_EMAIL", "checkpoints@pilothouse.invalid"),
];

/// What a restore did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// Files written back from the checkpoint (changed or deleted since).
    pub restored: usize,
    /// Files created since the checkpoint, now removed.
    pub removed: usize,
    /// Files not in the checkpoint that stay: it could not hold every
    /// untracked file (too many, or too large), so they may be the user's.
    pub kept: usize,
    /// The checkpoint taken just before, to undo the restore.
    pub undo: String,
}

/// Marks a checkpoint that left some untracked files out.
const PARTIAL: &str = "PilotHouse-Untracked: partial";

/// Git's empty tree, as a source of no attributes.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Runs git with the given index file and the checkpoint identity.
///
/// Line-ending conversion is off (`core.autocrlf`, `.gitattributes`): a
/// checkpoint stores the files' exact bytes and a restore writes them back
/// exactly, or a Windows user's LF files would come back as CRLF.
fn git_in(root: &Path, index: &Path, args: &[&str], input: Option<&[u8]>) -> Result<String> {
    let no_attributes = crate::null_device(root);
    let mut command = command(root);
    command
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.safecrlf=false",
            "-c",
            &format!("core.attributesFile={no_attributes}"),
        ])
        .args(args)
        .env("GIT_ATTR_SOURCE", EMPTY_TREE)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_INDEX_FILE", index)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .envs(IDENTITY);
    let mut command = crate::run::ready(command);
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("running git (is it installed?)")?;
    if let Some(input) = input {
        use std::io::Write as _;
        child.stdin.take().context("git stdin")?.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "{}",
            crate::run::failure(root, args.first().unwrap_or(&""), &output)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A temporary index file in the repository's git folder, removed on drop.
struct TempIndex(PathBuf);

impl TempIndex {
    fn new(git_dir: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "pilothouse-checkpoint-{}-{}.index",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = git_dir.join(name);
        // Start from the real index: its file stats let `git add` skip
        // hashing every unchanged file.
        let real = git_dir.join("index");
        if real.is_file() {
            std::fs::copy(&real, &path).context("copying the index")?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("index.lock"));
    }
}

/// Snapshots the working tree of the repository at `root`. Returns the
/// checkpoint's commit, the newest one again when nothing changed since, or
/// `None` when `root` is not in a Git repository.
pub fn create(root: &Path, label: &str) -> Result<Option<String>> {
    let Ok(git_dir) = crate::git(root, &["rev-parse", "--absolute-git-dir"]) else {
        return Ok(None);
    };
    let top = crate::local_path(root, &crate::git(root, &["rev-parse", "--show-toplevel"])?);
    let index = TempIndex::new(&crate::local_path(root, &git_dir))?;
    let index = index.0.as_path();

    // Tracked files: changes and deletions.
    git_in(&top, index, &["add", "--update", "--", "."], None)?;
    // Untracked files, within reason.
    let untracked = crate::git(&top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut wanted: Vec<&str> = untracked
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    let all_untracked = wanted.len();
    if wanted.len() > MAX_UNTRACKED_FILES {
        tracing::info!(
            "checkpoint: {} untracked files, leaving them out",
            wanted.len()
        );
        wanted.clear();
    }
    wanted.retain(|path| {
        std::fs::symlink_metadata(top.join(path))
            .is_ok_and(|meta| meta.len() <= MAX_UNTRACKED_BYTES)
    });
    // Some untracked files were left out: restoring cannot tell them from
    // files made later, so it must keep both.
    let partial = wanted.len() < all_untracked;
    if !wanted.is_empty() {
        let list = wanted.join("\0");
        git_in(
            &top,
            index,
            &["add", "--pathspec-from-file=-", "--pathspec-file-nul"],
            Some(list.as_bytes()),
        )?;
    }
    let tree = git_in(&top, index, &["write-tree"], None)?
        .trim()
        .to_string();

    // Nothing changed since the newest checkpoint: reuse it.
    if !partial
        && let Some(newest) = list(&top)?.into_iter().next()
        && crate::git(&top, &["rev-parse", &format!("{newest}^{{tree}}")])
            .is_ok_and(|newest_tree| newest_tree.trim() == tree)
    {
        return Ok(Some(newest));
    }

    let head = crate::git(&top, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .ok()
        .map(|head| head.trim().to_string());
    let mut message = format!("PilotHouse checkpoint\n\n{}", label.trim());
    if partial {
        message.push_str(&format!("\n\n{PARTIAL}"));
    }
    let mut args = vec!["commit-tree", tree.as_str(), "-m", message.as_str()];
    if let Some(head) = &head {
        args.extend(["-p", head.as_str()]);
    }
    let commit = git_in(&top, index, &args, None)?.trim().to_string();
    let name = format!("{REFS}/{}", ref_name());
    crate::git(&top, &["update-ref", &name, &commit])?;
    prune(&top)?;
    Ok(Some(commit))
}

/// A name that sorts by time: zero-padded milliseconds, then a counter for
/// checkpoints taken within the same millisecond.
fn ref_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    format!(
        "{millis:015}-{:06}",
        NEXT.fetch_add(1, Ordering::Relaxed) % 1_000_000
    )
}

/// Checkpoint commits, newest first.
pub fn list(root: &Path) -> Result<Vec<String>> {
    let out = crate::git(
        root,
        &[
            "for-each-ref",
            "--sort=-refname",
            "--format=%(objectname)",
            REFS,
        ],
    )?;
    Ok(out.lines().map(str::to_string).collect())
}

/// Keeps only the newest [`KEEP`] checkpoint refs.
fn prune(root: &Path) -> Result<()> {
    let out = crate::git(
        root,
        &[
            "for-each-ref",
            "--sort=-refname",
            "--format=%(refname)",
            REFS,
        ],
    )?;
    for old in out.lines().skip(KEEP) {
        crate::git(root, &["update-ref", "-d", old])?;
    }
    Ok(())
}

/// Puts the working tree back as it was at `checkpoint`: files changed or
/// deleted since are written back, files created since are removed. The
/// index and HEAD are left alone. Takes a checkpoint first (see
/// [`Restored::undo`]).
pub fn restore(root: &Path, checkpoint: &str) -> Result<Restored> {
    crate::check_name("checkpoint", checkpoint)?;
    let top = crate::local_path(root, &crate::git(root, &["rev-parse", "--show-toplevel"])?);
    crate::git(
        &top,
        &["cat-file", "-e", &format!("{checkpoint}^{{commit}}")],
    )
    .context("that checkpoint is gone")?;
    let undo = create(&top, "Before restoring a checkpoint")?.context("not a Git repository")?;

    let diff = crate::git(
        &top,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            "-z",
            checkpoint,
            &undo,
        ],
    )?;
    let mut parts = diff.split('\0').filter(|part| !part.is_empty());
    let (mut write_back, mut remove) = (Vec::new(), Vec::new());
    while let (Some(status), Some(path)) = (parts.next(), parts.next()) {
        match status {
            "A" => remove.push(path.to_string()),
            _ => write_back.push(path.to_string()),
        }
    }
    // A checkpoint that left untracked files out cannot say which files are
    // new: removing them could delete the user's own.
    let message = crate::git(&top, &["log", "-1", "--format=%B", checkpoint])?;
    let kept = if message.lines().any(|line| line.trim() == PARTIAL) {
        std::mem::take(&mut remove).len()
    } else {
        0
    };
    for path in &remove {
        let file = top.join(path);
        if file.is_file() || file.is_symlink() {
            std::fs::remove_file(&file).with_context(|| format!("removing {path}"))?;
        }
    }
    if !write_back.is_empty() {
        let list = write_back.join("\0");
        let index = TempIndex::new(&crate::local_path(
            &top,
            &crate::git(&top, &["rev-parse", "--absolute-git-dir"])?,
        ))?;
        git_in(
            &top,
            &index.0,
            &[
                "restore",
                &format!("--source={checkpoint}"),
                "--worktree",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            Some(list.as_bytes()),
        )?;
    }
    Ok(Restored {
        restored: write_back.len(),
        removed: remove.len(),
        kept,
        undo,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_repo;

    /// When a checkpoint could not keep every untracked file, restoring it
    /// keeps files it does not know, as they may be the user's.
    #[test]
    fn a_partial_checkpoint_never_removes_unknown_files() {
        let root = temp_repo("partial");
        std::fs::write(
            root.join("big.bin"),
            vec![0u8; (MAX_UNTRACKED_BYTES + 1) as usize],
        )
        .unwrap();
        let checkpoint = create(&root, "before").unwrap().unwrap();
        // The big file shrinks under the limit (the undo checkpoint keeps
        // it), and the agent makes a file.
        std::fs::write(root.join("big.bin"), b"small").unwrap();
        std::fs::write(root.join("new.txt"), b"new").unwrap();
        let restored = restore(&root, &checkpoint).unwrap();
        assert_eq!((restored.removed, restored.kept), (0, 2));
        assert!(root.join("big.bin").exists() && root.join("new.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Git set to convert line endings must not change a restored file.
    #[test]
    fn restores_files_byte_for_byte() {
        let root = temp_repo("crlf");
        let run = |args: &[&str]| crate::git(&root, args).unwrap();
        run(&["config", "core.autocrlf", "true"]);
        std::fs::write(root.join("unix.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("dos.txt"), "one\r\ntwo\r\n").unwrap();
        let checkpoint = create(&root, "before").unwrap().unwrap();
        std::fs::write(root.join("unix.txt"), "changed\n").unwrap();
        std::fs::write(root.join("dos.txt"), "changed\r\n").unwrap();
        restore(&root, &checkpoint).unwrap();
        assert_eq!(
            std::fs::read(root.join("unix.txt")).unwrap(),
            b"one\ntwo\n",
            "LF stays LF"
        );
        assert_eq!(
            std::fs::read(root.join("dos.txt")).unwrap(),
            b"one\r\ntwo\r\n",
            "CRLF stays CRLF"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restores_tracked_untracked_and_removed_files() {
        let root = &temp_repo("checkpoint");
        std::fs::write(root.join("notes.txt"), "draft\n").unwrap();
        let before = crate::git(root, &["status", "--porcelain"]).unwrap();
        let index_before = std::fs::read(root.join(".git/index")).unwrap();

        let checkpoint = create(root, "Before the agent").unwrap().unwrap();
        assert_eq!(
            create(root, "Again").unwrap().unwrap(),
            checkpoint,
            "nothing changed: the same checkpoint"
        );
        assert_eq!(
            crate::git(root, &["status", "--porcelain"]).unwrap(),
            before,
            "taking a checkpoint changes nothing"
        );
        assert_eq!(
            std::fs::read(root.join(".git/index")).unwrap(),
            index_before
        );

        // The "agent" edits a file, deletes one, adds one.
        std::fs::write(root.join("README.md"), "rewritten\n").unwrap();
        std::fs::remove_file(root.join("notes.txt")).unwrap();
        std::fs::write(root.join("new.rs"), "fn x() {}\n").unwrap();

        let restored = restore(root, &checkpoint).unwrap();
        assert_eq!((restored.restored, restored.removed), (2, 1));
        assert_eq!(
            std::fs::read_to_string(root.join("notes.txt")).unwrap(),
            "draft\n"
        );
        assert!(!root.join("new.rs").exists());
        assert_eq!(
            crate::git(root, &["status", "--porcelain"]).unwrap(),
            before
        );

        // And the restore can be undone.
        restore(root, &restored.undo).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("README.md")).unwrap(),
            "rewritten\n"
        );
        assert!(root.join("new.rs").exists());
        assert!(!root.join("notes.txt").exists());
        assert_eq!(
            list(root).unwrap().len(),
            3,
            "the restores took checkpoints too"
        );
    }

    #[test]
    fn outside_a_repository_there_is_nothing_to_take() {
        let dir = std::env::temp_dir().join(format!("ph-no-repo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(create(&dir, "x").unwrap(), None);
    }
}
