//! Where PilotHouse keeps its files on disk.

use std::path::{Path, PathBuf};

/// Overrides the data directory. Tests and the automation harness use it to run
/// against a throwaway profile instead of the user's real one.
pub const DATA_DIR_ENV: &str = "PILOTHOUSE_DATA_DIR";

/// `~/Library/Application Support/PilotHouse` on macOS, the XDG data dir on
/// Linux and `%APPDATA%\PilotHouse` on Windows.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("PilotHouse")
}

/// The absolute, symlink-free form of `path`. Unlike `fs::canonicalize`, it
/// gives `C:\\work\\app` on Windows, not `\\\\?\\C:\\work\\app`, which many
/// programs refuse as a working directory.
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Where workspaces without folders keep their scratch folders.
pub fn scratch_root() -> PathBuf {
    data_dir().join("workspaces")
}

pub fn database_path() -> PathBuf {
    data_dir().join("pilothouse.db")
}

/// Where the running app listens for the MCP bridge and automation.
///
/// On Unix this is the socket itself. Socket paths are limited to ~104
/// bytes on macOS, so a deep data directory (common for test profiles)
/// moves the socket into a private folder in the temp dir, keyed by a hash
/// of the data directory so two profiles never share a socket. The folder is
/// never shared with other users (see [`ensure_private_dir`]), so nobody
/// else can plant a socket there.
///
/// On Windows it is a file in the data folder holding the name of the
/// app's named pipe (random for each run).
pub fn control_socket_path() -> PathBuf {
    let dir = data_dir();
    if cfg!(windows) {
        return dir.join("control.pipe");
    }
    const MAX_SOCKET_PATH: usize = 100;
    let preferred = dir.join("control.sock");
    if preferred.as_os_str().len() < MAX_SOCKET_PATH {
        return preferred;
    }
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut hasher);
    private_temp_dir().join(format!("pilothouse-{:016x}.sock", hasher.finish()))
}

/// `$XDG_RUNTIME_DIR` when set (already private), else a per-user folder in
/// the temp dir. On Linux `/tmp` is shared by every user.
fn private_temp_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("pilothouse");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("pilothouse-{user}"))
}

/// Creates `dir` (and parents) readable only by this user, and tightens it if
/// it already exists. The data folder holds chats, browser logins and
/// captures, so other accounts on the machine must not read it.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let metadata = std::fs::metadata(dir)?;
        if metadata.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Checks that the folder holding `path` belongs to this user and is closed
/// to others, so a socket in it was made by us. Fails with a reason otherwise.
pub fn verify_private_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let parent = path.parent().unwrap_or(Path::new("/"));
        let metadata = std::fs::metadata(parent)?;
        // Our own data folder tells us who we are, without libc.
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let me = std::fs::metadata(&home)
            .map(|home| home.uid())
            .unwrap_or(metadata.uid());
        if metadata.uid() != me || metadata.mode() & 0o022 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{} is not private to this user", parent.display()),
            ));
        }
    }
    let _ = path;
    Ok(())
}
