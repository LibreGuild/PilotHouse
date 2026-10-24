//! Mappings, and paths from one side to the other.
//!
//! Paths are compared component by component. On Windows letters are
//! compared without case, `/` and `\` are the same, and the long forms
//! (`\\?\Z:\`, `\\?\UNC\server\share`) are the short ones. The rest of a
//! path keeps its case: the server's file system is case-sensitive.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// A folder on this computer and the same folder on a server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mapping {
    /// The folder here, as the user gave it (`Z:\`, `\\nas\code`,
    /// `/Volumes/code`).
    pub local: String,
    /// The same folder as the system resolves it (a mapped drive is its
    /// share, `\\nas\code`), so a path given either way matches. Filled in
    /// when the mapping is checked; empty until then. Paths PilotHouse makes
    /// use [`Mapping::local`].
    pub resolved: String,
    /// Where to connect: `user@host`, or a host from `~/.ssh/config`.
    pub host: String,
    /// The SSH port, when not the one SSH would use.
    pub port: Option<u16>,
    /// The same folder on the server (`/srv/code`).
    pub remote: String,
}

impl Mapping {
    /// What is wrong with it, for the form; `None` when it can be used.
    pub fn problem(&self) -> Option<&'static str> {
        if self.local.trim().is_empty() {
            return Some("Choose the folder on this computer.");
        }
        if !valid_host(&self.host) {
            return Some("Give the server as user@host, or a host from your SSH config.");
        }
        if !self.remote.starts_with('/') {
            return Some("Give the folder on the server as a full path, like /srv/code.");
        }
        if self.port == Some(0) {
            return Some("The port must be between 1 and 65535.");
        }
        None
    }

    /// `host`, or `host:port`, for showing.
    pub fn destination(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }
}

/// A host SSH can be given safely: no spaces or control characters, and not
/// starting with `-` (which `ssh` would read as an option).
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && c != '\'' && c != '"')
}

static MAPPINGS: RwLock<Vec<Mapping>> = RwLock::new(Vec::new());

/// Replaces the mappings in use (from Settings).
pub fn set_mappings(mappings: Vec<Mapping>) {
    *MAPPINGS.write().unwrap_or_else(|err| err.into_inner()) = mappings
        .into_iter()
        .filter(|mapping| mapping.problem().is_none())
        .collect();
}

/// The mappings in use.
pub fn mappings() -> Vec<Mapping> {
    MAPPINGS
        .read()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

/// A folder's place on a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub mapping: Mapping,
    /// The folder on the server.
    pub path: String,
}

/// Where `path` is on a server, if it is under a mapping.
pub fn remote_of(path: &Path) -> Option<Remote> {
    let mappings = MAPPINGS.read().unwrap_or_else(|err| err.into_inner());
    remote_in(&mappings, &path.to_string_lossy(), cfg!(windows))
}

/// The folder here that is `remote` on a server, if a mapping covers it.
pub fn to_local(remote: &str) -> Option<PathBuf> {
    let mappings = MAPPINGS.read().unwrap_or_else(|err| err.into_inner());
    local_in(&mappings, remote, cfg!(windows)).map(PathBuf::from)
}

/// [`remote_of`] over `mappings`, with Windows or Unix paths.
pub(crate) fn remote_in(mappings: &[Mapping], path: &str, windows: bool) -> Option<Remote> {
    if !is_absolute(path, windows) {
        return None;
    }
    let path = local_parts(path, windows);
    let mut best: Option<(usize, &Mapping)> = None;
    for mapping in mappings {
        for form in [&mapping.resolved, &mapping.local] {
            if form.is_empty() {
                continue;
            }
            let base = local_parts(form, windows);
            let matches =
                base.len() <= path.len() && base.iter().zip(&path).all(|(a, b)| a.key == b.key);
            if matches && best.is_none_or(|(len, _)| base.len() > len) {
                best = Some((base.len(), mapping));
            }
        }
    }
    let (len, mapping) = best?;
    let rest: Vec<&str> = path[len..].iter().map(|part| part.text.as_str()).collect();
    Some(Remote {
        mapping: mapping.clone(),
        path: join_remote(&mapping.remote, &rest),
    })
}

/// [`to_local`] over `mappings`.
pub(crate) fn local_in(mappings: &[Mapping], remote: &str, windows: bool) -> Option<String> {
    let path = remote_parts(remote);
    let mut best: Option<(usize, &Mapping)> = None;
    for mapping in mappings {
        let base = remote_parts(&mapping.remote);
        if base.len() <= path.len()
            && base.iter().zip(&path).all(|(a, b)| a == b)
            && best.is_none_or(|(len, _)| base.len() > len)
        {
            best = Some((base.len(), mapping));
        }
    }
    let (len, mapping) = best?;
    let local = if mapping.local.is_empty() {
        &mapping.resolved
    } else {
        &mapping.local
    };
    Some(join_local(local, &path[len..], windows))
}

/// `C:\…`, `\\server\…` (Windows) or `/…`: relative paths never match.
fn is_absolute(path: &str, windows: bool) -> bool {
    if windows {
        let bytes = path.as_bytes();
        path.starts_with(r"\\")
            || path.starts_with("//")
            || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    } else {
        path.starts_with('/')
    }
}

/// One component of a local path: as written, and as compared.
#[derive(Debug, Clone)]
struct Part {
    text: String,
    key: String,
}

fn local_parts(path: &str, windows: bool) -> Vec<Part> {
    if !windows {
        return path
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(|part| Part {
                text: part.to_string(),
                key: part.to_string(),
            })
            .collect();
    }
    let mut path = path.replace('/', "\\");
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        path = format!(r"\\{rest}");
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        path = rest.to_string();
    }
    let unc = path.starts_with(r"\\");
    let mut parts: Vec<Part> = path
        .split('\\')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| Part {
            text: part.to_string(),
            key: part.to_lowercase(),
        })
        .collect();
    // `\\server` is not a folder named `server`.
    if unc && let Some(first) = parts.first_mut() {
        first.key = format!(r"\\{}", first.key);
    }
    parts
}

fn remote_parts(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

fn join_remote(base: &str, rest: &[&str]) -> String {
    let mut out = base.trim_end_matches('/').to_string();
    for part in rest {
        out.push('/');
        out.push_str(part);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn join_local(base: &str, rest: &[&str], windows: bool) -> String {
    let separator = if windows { '\\' } else { '/' };
    let mut out = base.to_string();
    if windows {
        out = out.replace('/', "\\");
    }
    for part in rest {
        if !out.ends_with(separator) {
            out.push(separator);
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive() -> Mapping {
        Mapping {
            local: r"Z:\".into(),
            resolved: r"\\nas\code".into(),
            host: "dev@nas".into(),
            port: None,
            remote: "/srv/code".into(),
        }
    }

    #[test]
    fn windows_paths_reach_the_server_in_every_spelling() {
        let mappings = [drive()];
        for path in [
            r"Z:\app\src",
            r"z:/app/src",
            r"\\nas\code\app\src",
            r"\\NAS\Code\app\src",
            r"\\?\UNC\nas\code\app\src",
            r"\\?\Z:\app\src",
        ] {
            let remote = remote_in(&mappings, path, true).unwrap_or_else(|| panic!("{path}"));
            assert_eq!(remote.path, "/srv/code/app/src", "{path}");
        }
        assert_eq!(
            remote_in(&mappings, r"Z:\", true).unwrap().path,
            "/srv/code"
        );
        // The rest keeps its case, the server's names are case-sensitive.
        assert_eq!(
            remote_in(&mappings, r"Z:\App\Src", true).unwrap().path,
            "/srv/code/App/Src"
        );
        assert!(remote_in(&mappings, r"C:\code\app", true).is_none());
        assert!(remote_in(&mappings, r"\\nas\other\app", true).is_none());
        // A folder named like the server is not the share.
        assert!(remote_in(&mappings, r"C:\nas\code", true).is_none());
        assert!(remote_in(&mappings, r"app\src", true).is_none(), "relative");
    }

    #[test]
    fn server_paths_come_back_as_the_user_gave_the_folder() {
        let mappings = [drive()];
        assert_eq!(
            local_in(&mappings, "/srv/code/app/.git", true).unwrap(),
            r"Z:\app\.git"
        );
        assert_eq!(local_in(&mappings, "/srv/code", true).unwrap(), r"Z:\");
        assert!(local_in(&mappings, "/srv/codex/app", true).is_none());
        let share = Mapping {
            local: r"\\nas\code".into(),
            ..drive()
        };
        assert_eq!(
            local_in(&[share], "/srv/code/app", true).unwrap(),
            r"\\nas\code\app"
        );
    }

    #[test]
    fn unix_mounts_and_the_longest_mapping_win() {
        let mappings = [
            Mapping {
                local: "/Volumes/code".into(),
                host: "nas".into(),
                remote: "/srv/code".into(),
                ..Default::default()
            },
            Mapping {
                local: "/Volumes/code/big".into(),
                host: "build".into(),
                remote: "/data/big".into(),
                ..Default::default()
            },
        ];
        let remote = remote_in(&mappings, "/Volumes/code/app/", false).unwrap();
        assert_eq!(
            (remote.mapping.host.as_str(), remote.path.as_str()),
            ("nas", "/srv/code/app")
        );
        let remote = remote_in(&mappings, "/Volumes/code/big/x", false).unwrap();
        assert_eq!(
            (remote.mapping.host.as_str(), remote.path.as_str()),
            ("build", "/data/big/x")
        );
        assert!(remote_in(&mappings, "/Volumes/codex", false).is_none());
        assert!(
            remote_in(&mappings, "Volumes/code/app", false).is_none(),
            "relative"
        );
        assert_eq!(
            local_in(&mappings, "/data/big/x/y", false).unwrap(),
            "/Volumes/code/big/x/y"
        );
    }

    #[test]
    fn hosts_that_could_be_options_are_refused() {
        assert!(valid_host("dev@nas.local"));
        assert!(valid_host("devbox"));
        for host in ["", "-oProxyCommand=x", "a b", "a'b", "a\nb"] {
            assert!(!valid_host(host), "{host:?}");
        }
        let mapping = Mapping {
            remote: "srv/code".into(),
            ..drive()
        };
        assert!(mapping.problem().is_some());
        assert!(drive().problem().is_none());
    }
}
