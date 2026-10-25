//! Which GitHub repository and account a project belongs to.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use ph_github::{Account, Client, Repo, accounts};
use ph_store::Store;

/// A project's GitHub repository with a ready client.
#[derive(Clone)]
pub struct Hosting {
    pub account: Account,
    pub repo: Repo,
    /// The git remote it came from (`origin`).
    pub remote: String,
    pub client: Arc<Client>,
}

impl Hosting {
    /// `https://HOST/OWNER/NAME`.
    pub fn web_url(&self) -> String {
        format!(
            "https://{}/{}/{}",
            self.account.host, self.repo.owner, self.repo.name
        )
    }
}

/// Why a project has no hosting yet, shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub enum Missing {
    NotRepository,
    NoRemote,
    /// The remote is on `host`, which has no account yet.
    NoAccount {
        host: String,
    },
    Failed(String),
}

impl Missing {
    /// Why, in a sentence for the user.
    pub fn explain(&self) -> String {
        match self {
            Missing::NotRepository => "Not a Git repository.".into(),
            Missing::NoRemote => "No GitHub remote.".into(),
            Missing::NoAccount { host } => {
                format!("No GitHub account for {host} (Settings → GitHub).")
            }
            Missing::Failed(reason) => reason.clone(),
        }
    }
}

/// Reads the remote and finds its account (blocking; run off the UI thread).
pub fn resolve(root: &Path, store: &Store) -> Result<Hosting, Missing> {
    if !ph_git::is_repository(root) {
        return Err(Missing::NotRepository);
    }
    let hosted = ph_git::remote::hosted_repo(root).ok_or(Missing::NoRemote)?;
    let account = accounts::for_host(store, &hosted.host).ok_or_else(|| Missing::NoAccount {
        host: hosted.host.clone(),
    })?;
    let repo =
        Repo::new(&hosted.owner, &hosted.name).map_err(|err| Missing::Failed(err.to_string()))?;
    let client = account
        .client()
        .map_err(|err| Missing::Failed(format!("{err:#}")))?;
    Ok(Hosting {
        account,
        repo,
        remote: hosted.remote,
        client: Arc::new(client),
    })
}

/// The repository `id` on its host, for the GitHub view: no checkout of
/// the user's is involved (the remote named is the managed clone's, see
/// [`crate::clones`]).
pub fn for_repo(store: &Store, id: &crate::clones::RepoId) -> Result<Hosting, Missing> {
    let account = accounts::for_host(store, &id.host).ok_or_else(|| Missing::NoAccount {
        host: id.host.clone(),
    })?;
    let repo = Repo::new(&id.owner, &id.name).map_err(|err| Missing::Failed(err.to_string()))?;
    let client = account
        .client()
        .map_err(|err| Missing::Failed(format!("{err:#}")))?;
    Ok(Hosting {
        account,
        repo,
        remote: "origin".into(),
        client: Arc::new(client),
    })
}

/// Fetches PR `number`'s head into `refs/pilothouse/pr/<number>` and returns
/// the commit (for checking out or reviewing in a worktree).
pub fn fetch_head(root: &Path, hosting: &Hosting, number: u64) -> Result<String> {
    let local = format!("refs/pilothouse/pr/{number}");
    ph_git::worktree::fetch_ref(
        root,
        &hosting.remote,
        &format!("pull/{number}/head:{local}"),
    )?;
    let sha = ph_git::git(root, &["rev-parse", &local])?;
    let sha = sha.trim().to_string();
    if sha.is_empty() {
        return Err(anyhow!("could not read the fetched head"));
    }
    Ok(sha)
}
