//! Whether a folder is on a network drive, and which: to offer running its
//! work on the server, with the server and share filled in.

use std::path::Path;

/// A network share a folder is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkDrive {
    /// Where it is on this computer: `Z:\`, `\\nas\code`, `/Volumes/code`.
    pub root: String,
    /// The server sharing it.
    pub server: String,
    /// The share's name (Samba), or the exported path (NFS).
    pub share: String,
}

/// The network drive `path` is on, if it is on one.
pub fn network_drive_of(path: &Path) -> Option<NetworkDrive> {
    imp::network_drive_of(path)
}

/// `//user@server/share` (SMB) or `server:/export` (NFS) as server and share.
#[cfg_attr(windows, allow(dead_code))] // Windows asks the system instead.
fn split_source(source: &str) -> Option<(String, String)> {
    if let Some(rest) = source.strip_prefix("//") {
        let (host, share) = rest.split_once('/')?;
        let server = host.rsplit_once('@').map_or(host, |(_, server)| server);
        let share = share.trim_end_matches('/');
        return (!server.is_empty() && !share.is_empty())
            .then(|| (server.to_string(), share.to_string()));
    }
    let (server, export) = source.split_once(":/")?;
    (!server.is_empty()).then(|| (server.to_string(), format!("/{export}")))
}

/// File systems that are another computer's folders.
#[cfg_attr(windows, allow(dead_code))] // Windows asks the system instead.
const NETWORK_TYPES: &[&str] = &[
    "smbfs",
    "cifs",
    "smb3",
    "nfs",
    "nfs4",
    "afpfs",
    "webdav",
    "fuse.sshfs",
    "macfuse",
];

/// The mount holding `path` from `(source, mount point, type)` rows.
#[cfg_attr(windows, allow(dead_code))] // Windows asks the system instead.
fn mount_holding(path: &str, mounts: &[(String, String, String)]) -> Option<NetworkDrive> {
    mounts
        .iter()
        .filter(|(_, point, kind)| {
            NETWORK_TYPES.contains(&kind.as_str())
                && (path == point || path.starts_with(&format!("{}/", point.trim_end_matches('/'))))
        })
        .max_by_key(|(_, point, _)| point.len())
        .and_then(|(source, point, _)| {
            let (server, share) = split_source(source)?;
            Some(NetworkDrive {
                root: point.clone(),
                server,
                share,
            })
        })
}

/// macOS `mount` output: `//dev@nas/code on /Volumes/code (smbfs, nodev…)`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_mount(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter_map(|line| {
            let (source, rest) = line.split_once(" on ")?;
            let (point, options) = rest.rsplit_once(" (")?;
            let kind = options.split([',', ')']).next()?.trim();
            Some((source.to_string(), point.to_string(), kind.to_string()))
        })
        .collect()
}

/// Linux `/proc/mounts`: `//nas/code /mnt/code cifs rw,… 0 0`, with spaces
/// written `\040`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_mounts(text: &str) -> Vec<(String, String, String)> {
    let unescape = |field: &str| {
        field
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\134", "\\")
    };
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            Some((
                unescape(fields.next()?),
                unescape(fields.next()?),
                fields.next()?.to_string(),
            ))
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn network_drive_of(path: &Path) -> Option<NetworkDrive> {
        let output = ph_env::command("/sbin/mount").output().ok()?;
        let mounts = parse_mount(&String::from_utf8_lossy(&output.stdout));
        mount_holding(&path.to_string_lossy(), &mounts)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub fn network_drive_of(path: &Path) -> Option<NetworkDrive> {
        let text = std::fs::read_to_string("/proc/mounts").ok()?;
        mount_holding(&path.to_string_lossy(), &parse_proc_mounts(&text))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    /// `DRIVE_REMOTE` from `GetDriveTypeW`.
    const DRIVE_REMOTE: u32 = 4;

    pub fn network_drive_of(path: &Path) -> Option<NetworkDrive> {
        let text = path.to_string_lossy().replace('/', "\\");
        let text = text
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!(r"\\{rest}"))
            .unwrap_or(text);
        if let Some(rest) = text.strip_prefix(r"\\") {
            let mut parts = rest.split('\\');
            let (server, share) = (parts.next()?, parts.next()?);
            return Some(NetworkDrive {
                root: format!(r"\\{server}\{share}"),
                server: server.to_string(),
                share: share.to_string(),
            });
        }
        let letter = text.chars().next().filter(char::is_ascii_alphabetic)?;
        let root: Vec<u16> = format!("{letter}:\\\0").encode_utf16().collect();
        // SAFETY: `root` is a NUL-terminated UTF-16 string.
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_REMOTE {
            return None;
        }
        let device: Vec<u16> = format!("{letter}:\0").encode_utf16().collect();
        let mut buffer = vec![0u16; 1024];
        let mut length = buffer.len() as u32;
        // SAFETY: `device` is NUL-terminated and `buffer` holds `length` u16s.
        let status =
            unsafe { WNetGetConnectionW(device.as_ptr(), buffer.as_mut_ptr(), &mut length) };
        if status != 0 {
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        let unc = String::from_utf16_lossy(&buffer[..end]);
        let mut parts = unc.trim_start_matches('\\').split('\\');
        let (server, share) = (parts.next()?, parts.next()?);
        Some(NetworkDrive {
            root: format!("{}:\\", letter.to_ascii_uppercase()),
            server: server.to_string(),
            share: share.to_string(),
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use super::*;

    pub fn network_drive_of(_: &Path) -> Option<NetworkDrive> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_mounts_name_the_server_and_share() {
        let text = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only)\n//dev@nas.local/code on /Volumes/code (smbfs, nodev, nosuid, mounted by dev)\nnas:/export/home on /Volumes/home (nfs)\n";
        let mounts = parse_mount(text);
        let drive = mount_holding("/Volumes/code/app", &mounts).unwrap();
        assert_eq!(
            drive,
            NetworkDrive {
                root: "/Volumes/code".into(),
                server: "nas.local".into(),
                share: "code".into(),
            }
        );
        let nfs = mount_holding("/Volumes/home/x", &mounts).unwrap();
        assert_eq!(
            (nfs.server.as_str(), nfs.share.as_str()),
            ("nas", "/export/home")
        );
        assert!(mount_holding("/Users/dev/app", &mounts).is_none());
        assert!(mount_holding("/Volumes/codex", &mounts).is_none());
    }

    #[test]
    fn linux_mounts_too() {
        let text = "sysfs /sys sysfs rw 0 0\n//nas/team\\040code /mnt/team\\040code cifs rw,vers=3.0 0 0\n";
        let drive = mount_holding("/mnt/team code/app", &parse_proc_mounts(text)).unwrap();
        assert_eq!(
            (drive.server.as_str(), drive.share.as_str()),
            ("nas", "team code")
        );
        assert_eq!(drive.root, "/mnt/team code");
    }
}
