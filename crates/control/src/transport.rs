//! The local channel between the app and its clients, per platform.
//!
//! - **Unix**: a Unix domain socket at the endpoint path, owner-only, in a
//!   folder private to the user.
//! - **Windows**: a named pipe that only the current user may open
//!   (`D:P(A;;GA;;;OW)`), refusing remote clients. Its name is random for
//!   each run and written to the endpoint path, a file in the user's private
//!   data folder, so another account can neither find it nor create it first.
//!
//! Both sides speak the same newline-delimited JSON over a [`Connection`].

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context as _, Result};

/// One client connection: its two halves, usable from different threads.
pub struct Connection {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
}

#[cfg(unix)]
pub use unix::{Listener, connect};
#[cfg(windows)]
pub use windows::{Listener, connect};

#[cfg(unix)]
mod unix {
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::*;

    pub struct Listener(UnixListener);

    impl Listener {
        pub fn bind(endpoint: &Path) -> Result<Self> {
            use std::os::unix::fs::PermissionsExt as _;
            // A socket file left by a crashed run would make bind fail; one
            // a running app answers on is not ours to take.
            if endpoint.exists() {
                if UnixStream::connect(endpoint).is_ok() {
                    anyhow::bail!(
                        "another PilotHouse is running with this data folder ({})",
                        endpoint.display()
                    );
                }
                std::fs::remove_file(endpoint)
                    .with_context(|| format!("removing stale socket {}", endpoint.display()))?;
            }
            let listener = UnixListener::bind(endpoint)
                .with_context(|| format!("binding control socket {}", endpoint.display()))?;
            std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self(listener))
        }

        pub fn accept(&mut self) -> std::io::Result<Connection> {
            let (stream, _) = self.0.accept()?;
            Ok(Connection {
                reader: Box::new(stream.try_clone()?),
                writer: Box::new(stream),
            })
        }
    }

    pub fn connect(endpoint: &Path) -> Result<Connection> {
        // Never talk to a socket someone else could have put there.
        ph_store::paths::verify_private_parent(endpoint)
            .with_context(|| format!("refusing the control socket at {}", endpoint.display()))?;
        let stream = UnixStream::connect(endpoint).with_context(|| {
            format!(
                "PilotHouse is not running (no control socket at {})",
                endpoint.display()
            )
        })?;
        Ok(Connection {
            reader: Box::new(stream.try_clone()?),
            writer: Box::new(stream),
        })
    }
}

#[cfg(windows)]
mod windows {
    use std::fs::File;
    use std::os::windows::io::FromRawHandle as _;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
        LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    use super::*;

    /// Only the pipe's owner (this user) gets access; nothing is inherited.
    const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)";
    const BUFFER: u32 = 64 * 1024;

    pub struct Listener {
        name: Vec<u16>,
        first: bool,
    }

    impl Listener {
        pub fn bind(endpoint: &Path) -> Result<Self> {
            // The file names a running app's pipe: that one is not ours.
            if connect(endpoint).is_ok() {
                anyhow::bail!(
                    "another PilotHouse is running with this data folder ({})",
                    endpoint.display()
                );
            }
            let name = format!(r"\\.\pipe\pilothouse-{}", random_id());
            std::fs::write(endpoint, &name)
                .with_context(|| format!("writing {}", endpoint.display()))?;
            Ok(Self {
                name: wide(&name),
                first: true,
            })
        }

        /// Waits for the next client on a fresh pipe instance.
        pub fn accept(&mut self) -> std::io::Result<Connection> {
            let descriptor = OwnerOnly::new()?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: 0,
            };
            // The first instance must be new: if another process already owns
            // this name, fail rather than serve next to it.
            let mut flags = PIPE_ACCESS_DUPLEX;
            if self.first {
                flags |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            // SAFETY: `name` is NUL-terminated UTF-16 and `attributes` points
            // at a descriptor that lives until after the call.
            let handle = unsafe {
                CreateNamedPipeW(
                    self.name.as_ptr(),
                    flags,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    BUFFER,
                    BUFFER,
                    0,
                    &attributes,
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            self.first = false;
            // SAFETY: `handle` is a valid pipe handle owned here.
            let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !connected {
                let error = std::io::Error::last_os_error();
                // SAFETY: closing the handle created above.
                unsafe { CloseHandle(handle) };
                return Err(error);
            }
            // SAFETY: the handle is valid and ownership moves into the File.
            let pipe = unsafe { File::from_raw_handle(handle as _) };
            Ok(Connection {
                reader: Box::new(pipe.try_clone()?),
                writer: Box::new(pipe),
            })
        }
    }

    pub fn connect(endpoint: &Path) -> Result<Connection> {
        let name = std::fs::read_to_string(endpoint)
            .with_context(|| "PilotHouse is not running (no control pipe)")?;
        let name = name.trim();
        anyhow::ensure!(
            name.starts_with(r"\\.\pipe\pilothouse-"),
            "unexpected control pipe name in {}",
            endpoint.display()
        );
        // All instances can be busy for a moment while the app opens the next.
        let mut attempts = 0;
        let pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
            {
                Ok(pipe) => break pipe,
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(err) => {
                    return Err(err)
                        .context("PilotHouse is not running (cannot open its control pipe)");
                }
            }
        };
        Ok(Connection {
            reader: Box::new(pipe.try_clone()?),
            writer: Box::new(pipe),
        })
    }

    /// A security descriptor from [`OWNER_ONLY`], freed on drop.
    struct OwnerOnly(*mut core::ffi::c_void);

    impl OwnerOnly {
        fn new() -> std::io::Result<Self> {
            let sddl = wide(OWNER_ONLY);
            let mut descriptor = std::ptr::null_mut();
            // SAFETY: `sddl` is NUL-terminated; the descriptor is written to
            // `descriptor` and freed with LocalFree in Drop.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self(descriptor))
        }
    }

    impl Drop for OwnerOnly {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptor…, freed once.
            unsafe { LocalFree(self.0 as _) };
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 128 random bits from the OS-seeded hasher keys.
    fn random_id() -> String {
        use std::hash::{BuildHasher as _, Hasher as _};
        let mut id = String::new();
        for _ in 0..2 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            );
            hasher.write_u32(std::process::id());
            id.push_str(&format!("{:016x}", hasher.finish()));
        }
        id
    }
}
