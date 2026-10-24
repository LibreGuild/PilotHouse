//! Secrets (access tokens) in the operating system's credential store:
//! the Keychain on macOS, Credential Manager on Windows. Linux desktops vary
//! too much; there the token is a file only the user can read, in
//! PilotHouse's private data folder (like `gh` does).
//!
//! Debug builds use that private file too: every rebuild is a new binary to
//! the OS, which would ask the developer for keychain access each time.
//!
//! Secrets never go to the database, logs or the transcript.

use anyhow::Result;

/// The service name secrets are filed under in the OS store.
#[cfg(any(target_vendor = "apple", windows))]
const SERVICE: &str = "PilotHouse";

/// The OS store in release builds on macOS and Windows.
const OS_STORE: bool = !cfg!(debug_assertions) && (cfg!(target_vendor = "apple") || cfg!(windows));

/// Stores `secret` under `name` (replacing any old one).
pub fn set(name: &str, secret: &str) -> Result<()> {
    if OS_STORE {
        os::set(name, secret)
    } else {
        file::set(name, secret)
    }
}

/// The secret under `name`, if there is one.
pub fn get(name: &str) -> Result<Option<String>> {
    if OS_STORE {
        os::get(name)
    } else {
        file::get(name)
    }
}

pub fn delete(name: &str) -> Result<()> {
    if OS_STORE {
        os::delete(name)
    } else {
        file::delete(name)
    }
}

#[cfg(target_vendor = "apple")]
mod os {
    use anyhow::{Context as _, Result};
    use security_framework::passwords;

    use super::SERVICE;

    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25300;

    pub fn set(name: &str, secret: &str) -> Result<()> {
        passwords::set_generic_password(SERVICE, name, secret.as_bytes())
            .context("could not save to the Keychain")
    }

    pub fn get(name: &str) -> Result<Option<String>> {
        match passwords::get_generic_password(SERVICE, name) {
            Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
            Err(err) if err.code() == NOT_FOUND => Ok(None),
            Err(err) => Err(err).context("could not read the Keychain"),
        }
    }

    pub fn delete(name: &str) -> Result<()> {
        match passwords::delete_generic_password(SERVICE, name) {
            Err(err) if err.code() != NOT_FOUND => {
                Err(err).context("could not delete from the Keychain")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod os {
    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };

    use super::SERVICE;

    fn target(name: &str) -> Vec<u16> {
        format!("{SERVICE}:{name}")
            .encode_utf16()
            .chain([0])
            .collect()
    }

    pub fn set(name: &str, secret: &str) -> Result<()> {
        let mut target = target(name);
        let mut blob = secret.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: unsafe { std::mem::zeroed() },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            // Per user, kept across sign-ins on this machine.
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: std::ptr::null_mut(),
        };
        // SAFETY: every pointer refers to a live local buffer for the call.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            bail!("could not save to Credential Manager (error {})", unsafe {
                GetLastError()
            });
        }
        Ok(())
    }

    pub fn get(name: &str) -> Result<Option<String>> {
        let target = target(name);
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `target` is NUL-terminated; `found` is freed with CredFree.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) } == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_NOT_FOUND {
                return Ok(None);
            }
            bail!("could not read Credential Manager (error {error})");
        }
        // SAFETY: CredReadW succeeded, so `found` points at a credential
        // whose blob has `CredentialBlobSize` bytes.
        let secret = unsafe {
            let credential = &*found;
            let bytes = std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            );
            let text = String::from_utf8(bytes.to_vec());
            CredFree(found.cast());
            text
        }?;
        Ok(Some(secret))
    }

    pub fn delete(name: &str) -> Result<()> {
        let target = target(name);
        // SAFETY: `target` is NUL-terminated.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_NOT_FOUND {
                bail!("could not delete from Credential Manager (error {error})");
            }
        }
        Ok(())
    }
}

/// No OS store on this platform: the file store stands in.
#[cfg(not(any(target_vendor = "apple", windows)))]
use file as os;

mod file {
    use std::path::PathBuf;

    use anyhow::Result;

    fn path(name: &str) -> Result<PathBuf> {
        let dir = ph_store::paths::data_dir().join("secrets");
        ph_store::paths::ensure_private_dir(&dir)?;
        // Names are ours (`github:<id>`), but keep them to safe characters.
        let file: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Ok(dir.join(file))
    }

    pub fn set(name: &str, secret: &str) -> Result<()> {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path(name)?)?;
        file.write_all(secret.as_bytes())?;
        Ok(())
    }

    pub fn get(name: &str) -> Result<Option<String>> {
        match std::fs::read_to_string(path(name)?) {
            Ok(secret) => Ok(Some(secret)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub fn delete(name: &str) -> Result<()> {
        match std::fs::remove_file(path(name)?) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips() {
        let dir = std::env::temp_dir().join(format!("ph-secrets-{}", std::process::id()));
        // SAFETY: tests in this crate do not read the variable concurrently.
        unsafe { std::env::set_var(ph_store::paths::DATA_DIR_ENV, &dir) };
        file::set("github:test", "s3cret").unwrap();
        assert_eq!(file::get("github:test").unwrap().as_deref(), Some("s3cret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("secrets/github_test"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only the user may read it");
        }
        file::delete("github:test").unwrap();
        assert_eq!(file::get("github:test").unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
