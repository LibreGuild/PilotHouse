//! The GitHub view's own clones of repositories, apart from any checkout
//! of the user's (which are never touched):
//!
//! ```text
//! <data dir>/github/<host>/<owner>/<name>/repo    the clone (default branch)
//! <data dir>/github/<host>/<owner>/<name>/pr-12   pull request 12's head
//! ```
//!
//! Git reaches the host over HTTPS with the account's token, passed to each
//! call in the environment as a header for that host only
//! (`http.https://HOST/.extraHeader` through `GIT_CONFIG_*`): it is never
//! on a command line, in a remote URL or in a config file. The clone is a
//! full one, so agents reading history in a review never need the network.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::hosting::Hosting;

/// A repository on a GitHub host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoId {
    pub fn of(hosting: &Hosting) -> Self {
        Self {
            host: hosting.account.host.clone(),
            owner: hosting.repo.owner.clone(),
            name: hosting.repo.name.clone(),
        }
    }

    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// `host/owner/name`, as the list of added repositories keeps it.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }

    /// The repository a [`Self::key`] names (checked like any name).
    pub fn parse(key: &str) -> Option<Self> {
        let mut parts = key.split('/');
        let (host, owner, name) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let host = ph_github::accounts::normalize_host(host).ok()?;
        let repo = ph_github::Repo::new(owner, name).ok()?;
        Some(Self {
            host,
            owner: repo.owner,
            name: repo.name,
        })
    }

    /// The repository whose clone is `clone` (a GitHub space's folder).
    pub fn from_clone_dir(clone: &Path) -> Option<Self> {
        let root = ph_store::paths::canonical(&root()).ok()?;
        let clone = ph_store::paths::canonical(clone).ok()?;
        Self::from_clone_in(&root, &clone)
    }

    /// [`Self::from_clone_dir`] under clones folder `root` (both resolved).
    fn from_clone_in(root: &Path, clone: &Path) -> Option<Self> {
        let rest = clone.strip_prefix(root).ok()?;
        let parts: Vec<String> = rest
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        match parts.as_slice() {
            // Folders hold `_` where the host has `:`; host names have no `_`.
            [host, owner, name, last] if last == "repo" => {
                Self::parse(&format!("{}/{owner}/{name}", host.replace('_', ":")))
            }
            _ => None,
        }
    }

    /// The repository's folder of clones. The host's port (`host:8443`)
    /// becomes `host_8443`, which every file system takes.
    pub fn folder(&self) -> PathBuf {
        root()
            .join(self.host.replace(':', "_"))
            .join(&self.owner)
            .join(&self.name)
    }

    /// The clone, on the default branch.
    pub fn clone_dir(&self) -> PathBuf {
        self.folder().join("repo")
    }

    /// Pull request `number`'s head, checked out for a review.
    pub fn pull_dir(&self, number: u64) -> PathBuf {
        self.folder().join(format!("pr-{number}"))
    }
}

/// Where every clone goes.
pub fn root() -> PathBuf {
    ph_store::paths::data_dir().join("github")
}

/// Git's settings for one call that authenticate it to `host`.
fn auth_env(host: &str, token: &str) -> Vec<(String, String)> {
    let header = format!(
        "Authorization: Basic {}",
        base64(format!("x-access-token:{token}").as_bytes())
    );
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        (
            "GIT_CONFIG_KEY_0".into(),
            format!("http.https://{host}/.extraHeader"),
        ),
        ("GIT_CONFIG_VALUE_0".into(), header),
    ]
}

/// One clone or fetch at a time: two chats getting the same repository
/// ready would otherwise clone it into one folder together.
static GIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The clone of `hosting`'s repository, made the first time and brought up
/// to date (every branch) after (blocking; network).
pub fn ensure(hosting: &Hosting) -> Result<PathBuf> {
    let _one = GIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_locked(hosting)
}

fn ensure_locked(hosting: &Hosting) -> Result<PathBuf> {
    let id = RepoId::of(hosting);
    let env = auth_env(&id.host, &hosting.account.token()?);
    let clone = id.clone_dir();
    if clone.join(".git").exists() {
        ph_git::git_env(&clone, &["fetch", "--quiet", "--prune", "origin"], &env)
            .context("updating the clone")?;
        return Ok(clone);
    }
    let folder = id.folder();
    ph_store::paths::ensure_private_dir(&folder)?;
    // A clone that stopped half way is started again.
    if clone.exists() {
        std::fs::remove_dir_all(&clone)?;
    }
    let url = format!("https://{}/{}/{}.git", id.host, id.owner, id.name);
    ph_git::git_env(&folder, &["clone", "--quiet", "--", &url, "repo"], &env)
        .with_context(|| format!("cloning {}", id.full_name()))?;
    Ok(clone)
}

/// What a pull request may carry that would act on the agent reading it,
/// not just be read: settings with hooks that run commands, MCP servers
/// that start programs, API addresses that take the keys, environment
/// files the agent loads. Removed from the checkout before an agent works
/// there (the next checkout puts tracked ones back).
pub const AGENT_SETTINGS: &[&str] = &[
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".claude/hooks",
    ".mcp.json",
    ".gemini",
    ".codex",
    ".cursor",
    ".agents",
    ".clinerules",
    ".vscode/mcp.json",
];

/// [`AGENT_SETTINGS`] and the root's `.env` files, as present in `folder`.
fn agent_files(folder: &Path) -> Vec<String> {
    let env_files = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name == ".env" || name.starts_with(".env."))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    AGENT_SETTINGS
        .iter()
        .map(|path| path.to_string())
        .chain(env_files)
        .filter(|path| std::fs::symlink_metadata(folder.join(path)).is_ok())
        .collect()
}

/// Removes [`AGENT_SETTINGS`] and the root's `.env` files from checkout
/// `folder`; returns what was removed.
pub fn sanitize(folder: &Path) -> Vec<String> {
    remove(folder, agent_files(folder))
}

fn remove(folder: &Path, paths: Vec<String>) -> Vec<String> {
    let mut removed = Vec::new();
    for relative in paths {
        let path = folder.join(&relative);
        // A link is removed itself, never what it points to.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let gone = if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if gone.is_ok() {
            removed.push(relative);
        }
    }
    removed
}

/// [`sanitize`] for a checkout someone will commit in (a task fixing
/// another's pull request): what is taken out is also hidden from git
/// (`--skip-worktree`), so it is never committed as deleted.
pub fn sanitize_worktree(folder: &Path) -> Vec<String> {
    // Only what the pull request carries: files the task copied in from
    // the user's own checkout (an `.env`) are theirs and stay.
    let mut tracked = Vec::new();
    let mut theirs = Vec::new();
    for path in agent_files(folder) {
        let files = ph_git::git(folder, &["ls-files", "--", &path]).unwrap_or_default();
        if !files.trim().is_empty() {
            tracked.extend(files.lines().map(str::to_string));
            theirs.push(path);
        }
    }
    let removed = remove(folder, theirs);
    if !tracked.is_empty() {
        let mut args = vec!["update-index", "--skip-worktree", "--"];
        args.extend(tracked.iter().map(String::as_str));
        if let Err(err) = ph_git::git(folder, &args) {
            tracing::warn!("hiding the agent settings taken out from git failed: {err:#}");
        }
    }
    removed
}

/// Fetches pull request `number`'s head and checks it out (detached) in its
/// own folder beside the clone, which is made or updated first; then takes
/// out what would act on an agent ([`sanitize`]). Returns the folder, the
/// commit and what was taken out (blocking; network).
pub fn check_out_pull(hosting: &Hosting, number: u64) -> Result<(PathBuf, String, Vec<String>)> {
    let _one = GIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let clone = ensure_locked(hosting)?;
    let id = RepoId::of(hosting);
    let env = auth_env(&id.host, &hosting.account.token()?);
    let local = format!("refs/pilothouse/pr/{number}");
    ph_git::git_env(
        &clone,
        &[
            "fetch",
            "--quiet",
            "--force",
            "--",
            "origin",
            &format!("pull/{number}/head:{local}"),
        ],
        &env,
    )
    .with_context(|| format!("fetching #{number}"))?;
    let sha = ph_git::git(&clone, &["rev-parse", "--verify", &local])?
        .trim()
        .to_string();
    if sha.is_empty() {
        bail!("could not read #{number}'s head");
    }
    let folder = id.pull_dir(number);
    if folder.join(".git").exists() {
        // The copy is PilotHouse's: anything changed in it gives way.
        ph_git::git(
            &folder,
            &["checkout", "--quiet", "--force", "--detach", &sha],
        )?;
        ph_git::git(&folder, &["clean", "--quiet", "-d", "--force"])?;
    } else {
        if folder.exists() {
            std::fs::remove_dir_all(&folder)?;
        }
        ph_git::worktree::add_detached(&clone, &folder, &sha)?;
    }
    // The reviewed commit stays reachable for the next round's line map,
    // even after a force-push.
    let _ = ph_git::git(
        &clone,
        &[
            "update-ref",
            &format!("refs/pilothouse/reviewed/{number}/{sha}"),
            &sha,
        ],
    );
    let removed = sanitize(&folder);
    Ok((folder, sha, removed))
}

/// The repository's notes for agents on branch `base` of the clone
/// (`AGENTS.md`, `CLAUDE.md`, GitHub Copilot's instructions), by file:
/// context for reviews, from the base, never the pull request's head.
pub fn agent_notes(clone: &Path, base: &str) -> Vec<(String, String)> {
    if ph_git::check_ref(base).is_err() {
        return Vec::new();
    }
    ["AGENTS.md", "CLAUDE.md", ".github/copilot-instructions.md"]
        .iter()
        .filter_map(|file| {
            let text = ph_git::git(
                clone,
                &["show", "--no-textconv", &format!("origin/{base}:{file}")],
            )
            .ok()?;
            (!text.trim().is_empty())
                .then(|| (file.to_string(), text.chars().take(8_000).collect()))
        })
        .collect()
}

/// The repository's own review rules ([`ph_review::templates::TEAM_RULES`])
/// on branch `base` of the clone, as fetched last; `None` when it has none.
/// Read from the base, never the pull request's head, which its author
/// controls.
pub fn team_rules(clone: &Path, base: &str) -> Option<String> {
    ph_git::check_ref(base).ok()?;
    let spec = format!("origin/{base}:{}", ph_review::templates::TEAM_RULES);
    let text = ph_git::git(clone, &["show", "--no-textconv", &spec]).ok()?;
    (!text.trim().is_empty()).then(|| text.chars().take(20_000).collect())
}

/// Whether `path` is an existing file or folder inside the clones (links
/// and `..` resolved, so nothing outside passes).
pub fn is_managed(path: &Path) -> bool {
    match (
        ph_store::paths::canonical(&root()),
        ph_store::paths::canonical(path),
    ) {
        (Ok(root), Ok(path)) => path.starts_with(root),
        _ => false,
    }
}

/// Standard base64, for the header.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (ix, byte)| n | u32::from(*byte) << (16 - 8 * ix));
        for ix in 0..4 {
            if ix <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * ix) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"x-access-token:abc"), "eC1hY2Nlc3MtdG9rZW46YWJj");
    }

    #[test]
    fn the_token_goes_to_its_host_only() {
        let env = auth_env("ghe.example.net:8443", "secret");
        assert_eq!(env[1].1, "http.https://ghe.example.net:8443/.extraHeader");
        assert!(env[2].1.starts_with("Authorization: Basic "));
        assert!(!env.iter().any(|(_, value)| value.contains("secret")));
    }

    #[test]
    fn folders_per_repository_and_pull_request() {
        let id = RepoId {
            host: "ghe.example.net:8443".into(),
            owner: "team".into(),
            name: "svc".into(),
        };
        let folder = id.folder();
        assert!(folder.ends_with("github/ghe.example.net_8443/team/svc"));
        assert_eq!(id.clone_dir(), folder.join("repo"));
        assert_eq!(id.pull_dir(12), folder.join("pr-12"));
        assert_eq!(id.full_name(), "team/svc");
        assert_eq!(RepoId::parse(&id.key()), Some(id.clone()));
        assert_eq!(RepoId::parse("github.com/../etc"), None);
        assert_eq!(RepoId::parse("github.com/acme"), None);
    }

    #[test]
    fn a_clone_folder_names_its_repository() {
        let root = Path::new("/data/github");
        assert_eq!(
            RepoId::from_clone_in(
                root,
                Path::new("/data/github/ghe.example.net_8443/team/svc/repo")
            ),
            Some(RepoId {
                host: "ghe.example.net:8443".into(),
                owner: "team".into(),
                name: "svc".into(),
            })
        );
        assert_eq!(
            RepoId::from_clone_in(root, Path::new("/data/github/github.com/team/svc/pr-3")),
            None
        );
        assert_eq!(
            RepoId::from_clone_in(root, Path::new("/elsewhere/a/b/c/repo")),
            None
        );
    }

    #[test]
    fn takes_out_what_would_act_on_an_agent() {
        let folder = std::env::temp_dir().join(format!("ph-sanitize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join(".claude/hooks")).unwrap();
        std::fs::create_dir_all(folder.join(".vscode")).unwrap();
        std::fs::create_dir_all(folder.join("src")).unwrap();
        for file in [
            ".claude/settings.json",
            ".mcp.json",
            ".env",
            ".env.local",
            ".vscode/mcp.json",
            ".vscode/extensions.json",
            "src/main.rs",
            "AGENTS.md",
        ] {
            std::fs::write(folder.join(file), "x").unwrap();
        }
        let mut removed = sanitize(&folder);
        removed.sort();
        assert_eq!(
            removed,
            [
                ".claude/hooks",
                ".claude/settings.json",
                ".env",
                ".env.local",
                ".mcp.json",
                ".vscode/mcp.json"
            ]
        );
        assert!(folder.join("src/main.rs").exists() && folder.join("AGENTS.md").exists());
        assert!(folder.join(".vscode/extensions.json").exists());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_task_keeps_its_own_files_and_never_commits_the_removal() {
        let folder = std::env::temp_dir().join(format!("ph-sanitize-task-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=T",
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&folder)
                .output()
                .unwrap();
            String::from_utf8_lossy(&ok.stdout).into_owned()
        };
        git(&["init", "-q"]);
        std::fs::write(folder.join(".mcp.json"), "{}").unwrap();
        std::fs::write(folder.join("main.rs"), "fn main() {}").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "pr"]);
        // Copied in from the user's checkout, not part of the pull request.
        std::fs::write(folder.join(".env"), "MINE=1").unwrap();
        assert_eq!(sanitize_worktree(&folder), [".mcp.json"]);
        assert!(folder.join(".env").exists() && !folder.join(".mcp.json").exists());
        assert_eq!(git(&["status", "--porcelain", "--untracked-files=no"]), "");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn only_real_paths_inside_count_as_managed() {
        // Nothing exists here: never managed, whatever the spelling.
        let fake = root().join("github.com/../../../../etc/passwd");
        assert!(!is_managed(&fake));
    }
}
