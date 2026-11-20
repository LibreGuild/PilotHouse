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
                        "another {} is running with this data folder ({})",
                        ph_brand::NAME,
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
                "{} is not running (no control socket at {})",
                ph_brand::NAME,
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
    //! Both ends open the pipe for overlapped I/O, and each half of a
    //! [`Connection`] waits for its own operations only: on a synchronous
    //! handle Windows runs one operation at a time, so a write would wait for
    //! the read pending on the other half (the Chrome bridge always has one),
    //! which may be waiting for that very write's answer.

    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use std::sync::Arc;

    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError,
        HANDLE, INVALID_HANDLE_VALUE, LocalFree, TRUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile,
        WriteFile,
    };
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::CreateEventW;

    use super::*;

    /// Only the pipe's owner (this user) gets access; nothing is inherited.
    const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)";
    const BUFFER: u32 = 64 * 1024;

    pub struct Listener {
        name: Vec<u16>,
        /// The instance the next client connects to, made ahead of
        /// [`Listener::accept`] for the first one.
        next: Option<OwnedHandle>,
    }

    impl Listener {
        pub fn bind(endpoint: &Path) -> Result<Self> {
            // The file names a running app's pipe: that one is not ours.
            if connect(endpoint).is_ok() {
                anyhow::bail!(
                    "another {} is running with this data folder ({})",
                    ph_brand::NAME,
                    endpoint.display()
                );
            }
            let name = format!(r"\\.\pipe\pilothouse-{}", random_id());
            let wide_name = wide(&name);
            // The first instance exists before its name is written, so a
            // client that reads the name finds the pipe.
            let first = instance(&wide_name, true)
                .with_context(|| format!("creating the control pipe {name}"))?;
            std::fs::write(endpoint, &name)
                .with_context(|| format!("writing {}", endpoint.display()))?;
            Ok(Self {
                name: wide_name,
                next: Some(first),
            })
        }

        /// Waits for the next client on a fresh pipe instance.
        pub fn accept(&mut self) -> std::io::Result<Connection> {
            let pipe = match self.next.take() {
                Some(pipe) => pipe,
                None => instance(&self.name, false)?,
            };
            let handle: HANDLE = pipe.as_raw_handle();
            let event = Event::new()?;
            let mut overlapped = OVERLAPPED {
                hEvent: event.raw(),
                ..Default::default()
            };
            // SAFETY: `handle` is a valid pipe; `overlapped` and its event
            // outlive the operation, which is waited for below.
            if unsafe { ConnectNamedPipe(handle, &mut overlapped) } == 0 {
                // SAFETY: reads this thread's last error, nothing else.
                match unsafe { GetLastError() } {
                    ERROR_PIPE_CONNECTED => {}
                    ERROR_IO_PENDING => {
                        let mut done = 0;
                        // SAFETY: waits until the connect has finished with
                        // `overlapped`.
                        if unsafe { GetOverlappedResult(handle, &overlapped, &mut done, TRUE) } == 0
                        {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    error => return Err(std::io::Error::from_raw_os_error(error as i32)),
                }
            }
            halves(pipe)
        }
    }

    /// One instance of the pipe `name`, owner-only and overlapped. The first
    /// must be new: if another process already owns this name, fail rather
    /// than serve next to it.
    fn instance(name: &[u16], first: bool) -> std::io::Result<OwnedHandle> {
        let descriptor = OwnerOnly::new()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let mut flags = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            flags |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `name` is NUL-terminated UTF-16 and `attributes` points
        // at a descriptor that lives until after the call.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
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
        // SAFETY: `handle` is a valid pipe handle, owned from here on.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    pub fn connect(endpoint: &Path) -> Result<Connection> {
        let name = std::fs::read_to_string(endpoint)
            .with_context(|| format!("{} is not running (no control pipe)", ph_brand::NAME))?;
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
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(name)
            {
                Ok(pipe) => break pipe,
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!(
                            "{} is not running (cannot open its control pipe)",
                            ph_brand::NAME
                        )
                    });
                }
            }
        };
        Ok(halves(OwnedHandle::from(pipe))?)
    }

    /// The two halves of a connected overlapped pipe, each with its own
    /// event; the pipe closes when both are dropped.
    fn halves(pipe: OwnedHandle) -> std::io::Result<Connection> {
        let pipe = Arc::new(pipe);
        Ok(Connection {
            reader: Box::new(PipeReader {
                pipe: pipe.clone(),
                event: Event::new()?,
            }),
            writer: Box::new(PipeWriter {
                pipe,
                event: Event::new()?,
            }),
        })
    }

    struct PipeReader {
        pipe: Arc<OwnedHandle>,
        event: Event,
    }

    struct PipeWriter {
        pipe: Arc<OwnedHandle>,
        event: Event,
    }

    impl Read for PipeReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let len = buf.len().min(u32::MAX as usize) as u32;
            let data = buf.as_mut_ptr();
            // SAFETY: `buf` stays borrowed until `transfer` has waited for
            // the read to finish.
            let read = transfer(&self.pipe, &self.event, |handle, overlapped| unsafe {
                ReadFile(handle, data, len, std::ptr::null_mut(), overlapped)
            });
            match read {
                // The other end closed: the end of the input, as `File` has it.
                Err(err) if err.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
                read => read,
            }
        }
    }

    impl Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let len = buf.len().min(u32::MAX as usize) as u32;
            let data = buf.as_ptr();
            // SAFETY: `buf` stays borrowed until `transfer` has waited for
            // the write to finish.
            transfer(&self.pipe, &self.event, |handle, overlapped| unsafe {
                WriteFile(handle, data, len, std::ptr::null_mut(), overlapped)
            })
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Starts one operation on `pipe` with `start` and waits for its result
    /// alone, signalled on `event`.
    fn transfer(
        pipe: &OwnedHandle,
        event: &Event,
        start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> windows_sys::core::BOOL,
    ) -> std::io::Result<usize> {
        let handle: HANDLE = pipe.as_raw_handle();
        let mut overlapped = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        let pointer: *mut OVERLAPPED = &mut overlapped;
        if start(handle, pointer) == 0 {
            // SAFETY: reads this thread's last error, nothing else.
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(std::io::Error::from_raw_os_error(error as i32));
            }
        }
        let mut done = 0;
        // SAFETY: waits until the operation has finished with `overlapped`
        // and its buffer; both outlive the wait.
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut done, TRUE) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(done as usize)
    }

    /// A manual-reset event, as overlapped I/O needs (each operation resets
    /// it when it starts).
    struct Event(OwnedHandle);

    impl Event {
        fn new() -> std::io::Result<Self> {
            // SAFETY: no attributes and no name; the handle is checked below.
            let handle = unsafe { CreateEventW(std::ptr::null(), TRUE, 0, std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `handle` is a valid event handle, owned from here on.
            Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
        }

        fn raw(&self) -> HANDLE {
            self.0.as_raw_handle()
        }
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// A read waiting on one half must not hold up a write on the other: the
    /// Chrome bridge always has a read pending while it answers.
    #[test]
    fn a_pending_read_does_not_hold_up_a_write() {
        let dir = std::env::temp_dir().join(format!("ph-transport-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let endpoint = dir.join("c");
        let mut listener = Listener::bind(&endpoint).unwrap();
        let (lines, received) = mpsc::channel::<String>();
        let server_lines = lines.clone();
        let server = std::thread::spawn(move || {
            let Connection { reader, mut writer } = listener.accept().unwrap();
            let reading = std::thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(reader).read_line(&mut line).unwrap();
                _ = server_lines.send(format!("server: {line}"));
            });
            // The server's read is pending (or about to be): this must still go.
            writer.write_all(b"hello\n").unwrap();
            reading.join().unwrap();
        });
        let Connection { reader, mut writer } = connect(&endpoint).unwrap();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
                if lines.send(format!("client: {line}")).is_err() {
                    break;
                }
                line.clear();
            }
        });
        // A write that waits for the read stops here, not forever.
        let wait = Duration::from_secs(5);
        assert_eq!(received.recv_timeout(wait).unwrap(), "client: hello\n");
        // The client's reader is waiting again, for nothing that will come
        // before this write is through.
        std::thread::spawn(move || writer.write_all(b"ping\n"));
        assert_eq!(received.recv_timeout(wait).unwrap(), "server: ping\n");
        server.join().unwrap();
        _ = std::fs::remove_dir_all(&dir);
    }
}
