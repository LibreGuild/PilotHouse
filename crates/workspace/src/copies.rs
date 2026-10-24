//! Where copies made for agents go (a pull request checked out for review,
//! parallel-run lanes): the data folder, or for a repository on a mapped
//! network drive, the share (`<share>/.pilothouse/<install>/worktrees`), so
//! Git on the server can write them and agents here can reach them.

use std::path::{Path, PathBuf};

use anyhow::Result;
use ph_store::Store;

/// The folder for copies of the repository at `root`, made if missing.
pub fn folder(store: &Store, root: &Path) -> Result<PathBuf> {
    if let Some(home) = ph_git::worktree::home(root, &store.install_id()) {
        std::fs::create_dir_all(&home)?;
        return Ok(home);
    }
    let folder = ph_store::paths::data_dir().join("worktrees");
    ph_store::paths::ensure_private_dir(&folder)?;
    Ok(folder)
}

/// Every folder copies of this install can be in.
pub fn all_folders(store: &Store) -> Vec<PathBuf> {
    let owner = store.install_id();
    std::iter::once(ph_store::paths::data_dir().join("worktrees"))
        .chain(
            ph_remote::mappings()
                .iter()
                .map(|mapping| ph_git::worktree::share_home(mapping, &owner)),
        )
        .collect()
}
