//! "Open with PilotHouse" in the system file manager, for the current user
//! only (no administrator rights, nothing outside the user's own settings).
//!
//! - **Windows**: Explorer verbs under `HKCU\Software\Classes` for folders,
//!   a folder's background and every file. They run `pilothouse "<path>"`,
//!   which hands the path to the running app. (On Windows 11 they are under
//!   "Show more options".)
//! - **Linux**: a `.desktop` entry in `~/.local/share/applications` that
//!   file managers list under "Open With" for folders and text files.
//! - **macOS**: Finder's "Open With" comes from the app bundle's document
//!   types, so there is nothing to switch here.

use anyhow::Result;

/// Whether this platform has a per-user switch for it.
pub const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));

/// What the switch is called where it lives.
pub fn label() -> &'static str {
    if cfg!(windows) {
        "Show “Open with PilotHouse” in Explorer"
    } else {
        "List PilotHouse under “Open With” in the file manager"
    }
}

pub fn is_installed() -> bool {
    imp::is_installed()
}

/// Adds the menu entries, pointing at this executable.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe()?;
    imp::install(&exe)
}

pub fn uninstall() -> Result<()> {
    imp::uninstall()
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
    };

    /// The verb's key, and the placeholder Explorer fills with the path.
    const VERBS: [(&str, &str); 3] = [
        (r"Software\Classes\Directory\shell\PilotHouse", "%1"),
        (
            r"Software\Classes\Directory\Background\shell\PilotHouse",
            "%V",
        ),
        (r"Software\Classes\*\shell\PilotHouse", "%1"),
    ];

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// Creates (or opens) `path` and sets its values; `None` names the
    /// default value.
    fn write_key(path: &str, values: &[(Option<&str>, &str)]) -> Result<()> {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(path);
        // SAFETY: valid, NUL-terminated strings and an out pointer for the key.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            bail!("could not write the registry (error {status})");
        }
        let mut result = Ok(());
        for (name, value) in values {
            let name = name.map(wide);
            let data = wide(value);
            // SAFETY: `key` is open; the data is a NUL-terminated UTF-16
            // string and its length in bytes includes the NUL.
            let status = unsafe {
                RegSetValueExW(
                    key,
                    name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
                    0,
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            if status != ERROR_SUCCESS {
                result = Err(anyhow::anyhow!(
                    "could not write the registry (error {status})"
                ));
                break;
            }
        }
        // SAFETY: closing the key opened above.
        unsafe { RegCloseKey(key) };
        result
    }

    pub fn is_installed() -> bool {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(&format!(r"{}\command", VERBS[0].0));
        // SAFETY: valid string and out pointer; the key is closed below.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) };
        if status == ERROR_SUCCESS {
            // SAFETY: the key was opened above.
            unsafe { RegCloseKey(key) };
            return true;
        }
        false
    }

    pub fn install(exe: &Path) -> Result<()> {
        let exe = exe.display().to_string();
        let icon = format!("\"{exe}\",0");
        for (verb, placeholder) in VERBS {
            write_key(
                verb,
                &[(None, "Open with PilotHouse"), (Some("Icon"), &icon)],
            )?;
            let command = format!("\"{exe}\" \"{placeholder}\"");
            write_key(&format!(r"{verb}\command"), &[(None, &command)])?;
        }
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        for (verb, _) in VERBS {
            let path = wide(verb);
            // SAFETY: valid NUL-terminated key path under HKCU.
            let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                bail!("could not remove the menu entry (error {status})");
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::{Path, PathBuf};

    use anyhow::{Context as _, Result};

    fn entry_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })?;
        Some(data.join("applications/pilothouse.desktop"))
    }

    pub fn is_installed() -> bool {
        entry_path().is_some_and(|path| path.is_file())
    }

    pub fn install(exe: &Path) -> Result<()> {
        let path = entry_path().context("no home folder")?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&path, super::desktop_entry(exe))
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn uninstall() -> Result<()> {
        match entry_path() {
            Some(path) if path.exists() => Ok(std::fs::remove_file(path)?),
            _ => Ok(()),
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use std::path::Path;

    use anyhow::{Result, bail};

    pub fn is_installed() -> bool {
        false
    }

    pub fn install(_: &Path) -> Result<()> {
        bail!("Finder lists PilotHouse under Open With once the app is installed")
    }

    pub fn uninstall() -> Result<()> {
        Ok(())
    }
}

/// A freedesktop entry that opens folders and text files with `exe`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop_entry(exe: &std::path::Path) -> String {
    // Exec arguments are quoted, with `"`, `` ` ``, `$` and `\` escaped.
    let exe = exe
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=PilotHouse\n\
         Comment=Coding agents, editor, Git and more, on your machine\n\
         Exec=\"{exe}\" %F\n\
         Terminal=false\n\
         Categories=Development;IDE;TextEditor;\n\
         MimeType=inode/directory;text/plain;text/x-log;application/json;application/x-yaml;\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_the_program() {
        let entry = desktop_entry(std::path::Path::new("/opt/my apps/pilot$house"));
        assert!(entry.contains("Exec=\"/opt/my apps/pilot\\$house\" %F\n"));
        assert!(entry.contains("MimeType=inode/directory;"));
    }
}
