//! PilotHouse's Chrome extension, which lets agents use the user's own
//! Chrome (see [`crate::AgentChrome`]), and its native messaging host.
//!
//! - **The extension** (`crates/browser/extension/`) is built into the app and
//!   written to `<data dir>/chrome-extension/`, from where the user loads it
//!   unpacked (Developer mode → Load unpacked). Its manifest carries a fixed
//!   public `key`, so its id ([`EXTENSION_ID`]) is the same wherever it is
//!   loaded from.
//! - **Set up** ([`set_up`], only when the user asks in Settings) registers
//!   the native messaging host `com.libreguild.pilothouse` with Chrome,
//!   Chromium, Edge and Brave: a host manifest that only this extension may use
//!   (`allowed_origins`), pointing at a small launcher script that runs this
//!   program as `pilothouse chrome-host` for this data folder. [`remove`] takes
//!   back exactly what it wrote.
//! - **The host** ([`run_host`]) is what Chrome starts: it passes Chrome's
//!   messages (stdio, a 32-bit native-endian length then JSON) to the running
//!   app over a local endpoint (`<data dir>/chrome-bridge.sock`, a named pipe
//!   on Windows; owner-only, never a network port), and the app's answers back,
//!   cutting those over Chrome's 1 MB limit into chunks the extension joins.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

/// The extension's id, from the public key in its manifest.
pub const EXTENSION_ID: &str = "ldjgjondfmfmfdkbbbghciidipnanjgn";
/// The native messaging host's name.
pub const HOST_NAME: &str = "com.libreguild.pilothouse";
/// The tab group agents work in.
pub const GROUP_TITLE: &str = "PilotHouse";

/// The only caller the host serves: this extension.
pub fn origin() -> String {
    format!("chrome-extension://{EXTENSION_ID}/")
}

/// The extension's files (paths in `extension/`), built into the app.
macro_rules! extension_files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../extension/", $name)) as &[u8])),*]
    };
}

/// The extension's files, as built into the app.
const FILES: &[(&str, &[u8])] = extension_files![
    "manifest.json",
    "background.js",
    "lib/link.js",
    "lib/page.js",
    "lib/settings.js",
    "lib/tabs.js",
    "features/record.js",
    "features/send.js",
    "features/status.js",
    "page/area.js",
    "page/clip.js",
    "page/picker.js",
    "page/recorder.js",
    "offscreen.html",
    "offscreen.js",
    "sidepanel.html",
    "sidepanel.css",
    "sidepanel.js",
    "icons/icon16.png",
    "icons/icon32.png",
    "icons/icon48.png",
    "icons/icon128.png",
];

/// Where the extension is written for the user to load.
pub fn extension_dir() -> PathBuf {
    ph_store::paths::data_dir().join("chrome-extension")
}

/// Writes the extension to [`extension_dir`] (each file only when it
/// changed, so Chrome sees no change when there is none). Returns the folder.
pub fn write_extension() -> Result<PathBuf> {
    let dir = extension_dir();
    write_files(&dir)?;
    Ok(dir)
}

fn write_files(dir: &Path) -> Result<()> {
    for (name, bytes) in FILES {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() == Some(*bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

// ---- Registering the host -------------------------------------------------

/// Where things go: the user's home, the data folder, this program. Real
/// values in the app; temporary folders in tests.
#[derive(Debug, Clone)]
pub struct Layout {
    pub home: PathBuf,
    pub data: PathBuf,
    pub program: PathBuf,
}

impl Layout {
    pub fn current() -> Result<Self> {
        let home = ph_env::var("HOME")
            .or_else(|| ph_env::var("USERPROFILE"))
            .map(PathBuf::from)
            .context("no home folder")?;
        // An AppImage runs from a mount that is named anew each start: the
        // launcher names the AppImage itself.
        let appimage = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|path| cfg!(target_os = "linux") && path.is_file());
        let program = match appimage {
            Some(appimage) => appimage,
            None => std::env::current_exe().context("finding PilotHouse's program")?,
        };
        Ok(Self {
            home,
            data: ph_store::paths::data_dir(),
            program,
        })
    }

    fn host_dir(&self) -> PathBuf {
        self.data.join("chrome-host")
    }

    /// The launcher Chrome runs.
    pub fn launcher(&self) -> PathBuf {
        let name = if cfg!(windows) {
            "pilothouse-chrome-host.bat"
        } else {
            "pilothouse-chrome-host"
        };
        self.host_dir().join(name)
    }

    /// The browsers PilotHouse registers with, each with the folder for its
    /// host manifests (on Windows, the registry key under HKCU instead).
    fn browsers(&self) -> Vec<(&'static str, PathBuf)> {
        let list: &[(&str, &str)] = if cfg!(target_os = "macos") {
            &[
                ("Google Chrome", "Library/Application Support/Google/Chrome"),
                ("Chromium", "Library/Application Support/Chromium"),
                (
                    "Microsoft Edge",
                    "Library/Application Support/Microsoft Edge",
                ),
                (
                    "Brave",
                    "Library/Application Support/BraveSoftware/Brave-Browser",
                ),
            ]
        } else if cfg!(windows) {
            &[
                ("Google Chrome", r"Software\Google\Chrome"),
                ("Chromium", r"Software\Chromium"),
                ("Microsoft Edge", r"Software\Microsoft\Edge"),
                ("Brave", r"Software\BraveSoftware\Brave-Browser"),
            ]
        } else {
            &[
                ("Google Chrome", ".config/google-chrome"),
                ("Chromium", ".config/chromium"),
                ("Microsoft Edge", ".config/microsoft-edge"),
                ("Brave", ".config/BraveSoftware/Brave-Browser"),
            ]
        };
        list.iter()
            .map(|(name, place)| {
                let place = if cfg!(windows) {
                    PathBuf::from(place)
                } else {
                    self.home.join(place)
                };
                (*name, place)
            })
            .collect()
    }
}

/// What the host's manifest says.
fn host_manifest(launcher: &Path) -> Value {
    json!({
        "name": HOST_NAME,
        "description": "PilotHouse: lets its Chrome extension reach the app",
        "path": launcher,
        "type": "stdio",
        "allowed_origins": [origin()],
    })
}

/// The launcher script: this program as the host, for this data folder (so
/// a test profile's app is the one reached).
fn launcher_script(layout: &Layout) -> String {
    if cfg!(windows) {
        // In a batch file `%` starts a variable; doubled, it is itself.
        let escape = |path: &Path| path.display().to_string().replace('%', "%%");
        format!(
            "@echo off\r\nrem Written by PilotHouse: Chrome runs this to reach the PilotHouse app.\r\nset \"{}={}\"\r\n\"{}\" chrome-host %*\r\n",
            ph_store::paths::DATA_DIR_ENV,
            escape(&layout.data),
            escape(&layout.program),
        )
    } else {
        let quote =
            |path: &Path| format!("'{}'", path.display().to_string().replace('\'', r"'\''"));
        format!(
            "#!/bin/sh\n# Written by PilotHouse: Chrome runs this to reach the PilotHouse app.\n{}={}\nexport {}\nexec {} chrome-host \"$@\"\n",
            ph_store::paths::DATA_DIR_ENV,
            quote(&layout.data),
            ph_store::paths::DATA_DIR_ENV,
            quote(&layout.program),
        )
    }
}

/// Registers the host with every browser found; returns their names.
pub fn set_up(layout: &Layout) -> Result<Vec<&'static str>> {
    let host_dir = layout.host_dir();
    ph_store::paths::ensure_private_dir(&host_dir)?;
    let launcher = layout.launcher();
    std::fs::write(&launcher, launcher_script(layout))
        .with_context(|| format!("writing {}", launcher.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700))?;
    }
    let manifest = serde_json::to_string_pretty(&host_manifest(&launcher))?;
    let mut done = Vec::new();
    if cfg!(windows) {
        let file = host_dir.join(format!("{HOST_NAME}.json"));
        std::fs::write(&file, &manifest)?;
        for (name, key) in layout.browsers() {
            registry::set(&key, &file)?;
            done.push(name);
        }
    } else {
        for (name, profile) in layout.browsers() {
            // A browser that was never run here has no folder: skipped.
            if !profile.is_dir() {
                continue;
            }
            let hosts = profile.join("NativeMessagingHosts");
            std::fs::create_dir_all(&hosts)?;
            let file = hosts.join(format!("{HOST_NAME}.json"));
            std::fs::write(&file, &manifest)
                .with_context(|| format!("writing {}", file.display()))?;
            done.push(name);
        }
    }
    if done.is_empty() {
        bail!(
            "no Chrome, Chromium, Edge or Brave was found for this user: open the browser once, then set up again"
        );
    }
    Ok(done)
}

/// Takes back what [`set_up`] wrote: host manifests that point at this data
/// folder's launcher, and the launcher. Ones another PilotHouse data folder
/// registered are left alone.
pub fn remove(layout: &Layout) -> Result<()> {
    let launcher = layout.launcher();
    if cfg!(windows) {
        let file = layout.host_dir().join(format!("{HOST_NAME}.json"));
        for (_, key) in layout.browsers() {
            if registry::get(&key).is_some_and(|known| known == file) {
                registry::delete(&key)?;
            }
        }
        remove_file(&file)?;
    } else {
        for (_, profile) in layout.browsers() {
            let file = profile
                .join("NativeMessagingHosts")
                .join(format!("{HOST_NAME}.json"));
            if points_at(&file, &launcher) {
                remove_file(&file)?;
            }
        }
    }
    remove_file(&launcher)
}

fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(err).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Whether the host manifest `file` is PilotHouse's and runs `launcher`.
fn points_at(file: &Path, launcher: &Path) -> bool {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|manifest| {
            manifest["name"] == HOST_NAME && manifest["path"].as_str() == launcher.to_str()
        })
}

/// Where the host is registered for this data folder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registration {
    /// Browsers whose host manifest runs this data folder's launcher.
    pub browsers: Vec<&'static str>,
    /// A browser runs another PilotHouse's launcher (another data folder).
    pub elsewhere: bool,
}

pub fn registration(layout: &Layout) -> Registration {
    let launcher = layout.launcher();
    let mut found = Registration::default();
    if !launcher.is_file() {
        return found;
    }
    for (name, place) in layout.browsers() {
        let file = if cfg!(windows) {
            match registry::get(&place) {
                Some(file) => file,
                None => continue,
            }
        } else {
            place
                .join("NativeMessagingHosts")
                .join(format!("{HOST_NAME}.json"))
        };
        if points_at(&file, &launcher) {
            found.browsers.push(name);
        } else if file.is_file() {
            found.elsewhere = true;
        }
    }
    found
}

#[cfg(windows)]
mod registry {
    //! `HKCU\<browser key>\NativeMessagingHosts\com.libreguild.pilothouse`,
    //! whose default value names the host manifest.
    use std::path::{Path, PathBuf};

    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn key_name(browser: &Path) -> Vec<u16> {
        wide(&format!(
            r"{}\NativeMessagingHosts\{}",
            browser.display(),
            super::HOST_NAME
        ))
    }

    pub fn set(browser: &Path, manifest: &Path) -> Result<()> {
        let name = key_name(browser);
        let value = wide(&manifest.display().to_string());
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: the strings are NUL-terminated and live through the calls;
        // the key is closed after use.
        unsafe {
            let created = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                name.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if created != ERROR_SUCCESS {
                bail!("could not write the registry ({created})");
            }
            let written = RegSetValueExW(
                key,
                std::ptr::null(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            );
            RegCloseKey(key);
            if written != ERROR_SUCCESS {
                bail!("could not write the registry ({written})");
            }
        }
        Ok(())
    }

    pub fn get(browser: &Path) -> Option<PathBuf> {
        let name = key_name(browser);
        let mut buffer = vec![0u16; 2048];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: `buffer` holds `size` bytes; the result is NUL-terminated.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                name.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if read != ERROR_SUCCESS {
            return None;
        }
        let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
    }

    pub fn delete(browser: &Path) -> Result<()> {
        let name = key_name(browser);
        // SAFETY: `name` is NUL-terminated; the key and what it holds go.
        let deleted = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, name.as_ptr()) };
        if deleted != ERROR_SUCCESS && deleted != ERROR_FILE_NOT_FOUND {
            bail!("could not change the registry ({deleted})");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod registry {
    use std::path::{Path, PathBuf};

    use anyhow::Result;

    pub fn set(_: &Path, _: &Path) -> Result<()> {
        Ok(())
    }

    pub fn get(_: &Path) -> Option<PathBuf> {
        None
    }

    pub fn delete(_: &Path) -> Result<()> {
        Ok(())
    }
}

// ---- The host -------------------------------------------------------------

/// Most Chrome sends the host in one message (Chrome's own limit).
const MAX_FROM_CHROME: usize = 64 << 20;
/// Most the host sends Chrome in one message (Chrome's limit is 1 MB).
const MAX_TO_CHROME: usize = 1 << 20;
/// A chunk's share of a large message, small enough that escaping it as a
/// JSON string (at most doubling it) keeps it under [`MAX_TO_CHROME`].
const CHUNK: usize = 256 * 1024;
/// Most the app sends in one message.
const MAX_FROM_APP: usize = 32 << 20;

/// `pilothouse chrome-host <origin> [--parent-window=…]`: what Chrome starts
/// for the extension. Serves only PilotHouse's extension, and only while the
/// app runs.
pub fn run_host(args: &[String]) -> i32 {
    let Some(caller) = args.first() else {
        eprintln!("pilothouse chrome-host is started by Chrome for PilotHouse's extension");
        return 2;
    };
    if *caller != origin()
        || args[1..]
            .iter()
            .any(|arg| !arg.starts_with("--parent-window="))
    {
        eprintln!(
            "pilothouse chrome-host: refusing {caller}: only PilotHouse's extension may use it"
        );
        return 1;
    }
    let mut stdout = std::io::stdout().lock();
    let connection = match ph_control::transport::connect(&ph_store::paths::chrome_bridge_path()) {
        Ok(connection) => connection,
        Err(_) => {
            // The extension tries again later.
            let _ = write_message(
                &mut stdout,
                &json!({ "method": "PilotHouse.unavailable", "params": { "message": "PilotHouse is not running" } })
                    .to_string(),
                0,
            );
            return 0;
        }
    };
    let ph_control::transport::Connection { reader, mut writer } = connection;
    // Chrome → app. When Chrome closes the host's input (the extension let
    // go, or Chrome quit), the host is done.
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        while let Ok(Some(mut message)) = read_frame(&mut stdin, MAX_FROM_CHROME) {
            if !is_object(&message) {
                continue;
            }
            message.push(b'\n');
            if writer
                .write_all(&message)
                .and_then(|_| writer.flush())
                .is_err()
            {
                break;
            }
        }
        std::process::exit(0);
    });
    // App → Chrome.
    let mut reader = BufReader::new(reader);
    let mut id = 0u64;
    while let Ok(Some(line)) = read_line(&mut reader, MAX_FROM_APP) {
        if !is_object(&line) {
            continue;
        }
        id += 1;
        let Ok(text) = String::from_utf8(line) else {
            continue;
        };
        if write_message(&mut stdout, &text, id).is_err() {
            break;
        }
    }
    0
}

fn is_object(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok_and(|value| value.is_object())
}

/// One message from Chrome: `None` at the end of input.
fn read_frame(input: &mut impl Read, max: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut length = [0u8; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let length = u32::from_ne_bytes(length) as usize;
    if length > max {
        return Err(std::io::Error::other("message too large"));
    }
    let mut message = vec![0; length];
    input.read_exact(&mut message)?;
    Ok(Some(message))
}

fn write_frame(output: &mut impl Write, message: &[u8]) -> std::io::Result<()> {
    output.write_all(&(message.len() as u32).to_ne_bytes())?;
    output.write_all(message)?;
    output.flush()
}

/// Sends `text` (a JSON object) to Chrome, in chunks when it is too large
/// for one message; `id` names the chunks' message.
fn write_message(output: &mut impl Write, text: &str, id: u64) -> std::io::Result<()> {
    if text.len() < MAX_TO_CHROME {
        return write_frame(output, text.as_bytes());
    }
    let parts = split(text, CHUNK);
    for (index, part) in parts.iter().enumerate() {
        let chunk =
            json!({ "chunk": { "id": id, "index": index, "count": parts.len(), "data": part } });
        write_frame(output, chunk.to_string().as_bytes())?;
    }
    Ok(())
}

/// `text` in pieces of at most `size` bytes, cut between characters.
fn split(text: &str, size: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut cut = rest.len().min(size);
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (part, after) = rest.split_at(cut);
        parts.push(part);
        rest = after;
    }
    parts
}

/// One line (without its newline), at most `max` bytes; `None` at the end.
pub(crate) fn read_line(input: &mut impl BufRead, max: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                line.extend_from_slice(&available[..end]);
                input.consume(end + 1);
                return Ok(Some(line));
            }
            None => {
                let taken = available.len();
                line.extend_from_slice(available);
                input.consume(taken);
            }
        }
        if line.len() > max {
            return Err(std::io::Error::other("message too large"));
        }
    }
}

/// Why agents cannot use Chrome now, for their tools' answers.
pub fn not_connected() -> anyhow::Error {
    anyhow!(
        "PilotHouse's Chrome extension is not connected, so you cannot use the user's Chrome. \
         Ask the user to set it up in PilotHouse (Settings → Chrome extension) and to keep Chrome \
         open, or use the built-in browser tools (browser_open, browser_snapshot…) for the \
         project's own pages."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_comes_from_the_manifest_key() {
        use base64::Engine as _;
        use sha2::Digest as _;
        let manifest: Value =
            serde_json::from_slice(include_bytes!("../extension/manifest.json")).unwrap();
        let key = base64::engine::general_purpose::STANDARD
            .decode(manifest["key"].as_str().unwrap())
            .unwrap();
        let digest = sha2::Sha256::digest(&key);
        let id: String = digest[..16]
            .iter()
            .flat_map(|byte| [byte >> 4, byte & 0xf])
            .map(|nibble| (b'a' + nibble) as char)
            .collect();
        assert_eq!(id, EXTENSION_ID);
        // The extension's version is the app's.
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        // Only what it needs. No site is reachable by default: pages are read
        // only when the user asks (activeTab), sites the user allows one by
        // one (optional), and nothing runs in pages by itself.
        assert_eq!(
            manifest["permissions"],
            json!([
                "debugger",
                "nativeMessaging",
                "tabs",
                "tabGroups",
                "activeTab",
                "scripting",
                "contextMenus",
                "notifications",
                "sidePanel",
                "storage",
                "offscreen",
                "tabCapture"
            ])
        );
        assert!(manifest.get("host_permissions").is_none());
        assert!(manifest.get("content_scripts").is_none());
        assert_eq!(manifest["optional_host_permissions"], json!(["<all_urls>"]));
    }

    #[test]
    fn every_file_of_the_extension_is_built_in() {
        fn walk(dir: &Path, base: &Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, found);
                } else if path.file_name().is_some_and(|name| name != ".DS_Store") {
                    let relative = path.strip_prefix(base).unwrap();
                    found.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("extension");
        let mut found = Vec::new();
        walk(&base, &base, &mut found);
        for file in found {
            assert!(
                FILES.iter().any(|(name, _)| *name == file),
                "extension/{file} is not in FILES, so it would not be shipped"
            );
        }
    }

    #[test]
    fn frames_round_trip_and_large_messages_go_in_chunks() {
        let mut out = Vec::new();
        write_message(&mut out, r#"{"id":1}"#, 1).unwrap();
        let mut input = out.as_slice();
        assert_eq!(
            read_frame(&mut input, 100).unwrap().unwrap(),
            br#"{"id":1}"#
        );
        assert!(read_frame(&mut input, 100).unwrap().is_none());

        // Over 1 MB, with quotes and non-ASCII that escaping grows.
        let big = json!({ "data": "\"é".repeat(400_000) }).to_string();
        let mut out = Vec::new();
        write_message(&mut out, &big, 7).unwrap();
        let mut input = out.as_slice();
        let mut joined = String::new();
        let mut count = 0;
        while let Some(frame) = read_frame(&mut input, MAX_TO_CHROME).unwrap() {
            assert!(frame.len() <= MAX_TO_CHROME);
            let chunk: Value = serde_json::from_slice(&frame).unwrap();
            assert_eq!(chunk["chunk"]["id"], 7);
            assert_eq!(chunk["chunk"]["index"], count);
            joined.push_str(chunk["chunk"]["data"].as_str().unwrap());
            count += 1;
        }
        assert!(count > 1);
        assert_eq!(joined, big);

        // A frame over the limit is refused.
        let mut out = Vec::new();
        write_frame(&mut out, &[b'x'; 10]).unwrap();
        assert!(read_frame(&mut out.as_slice(), 5).is_err());
    }

    #[test]
    fn lines_are_capped() {
        let mut input = BufReader::new(&b"{\"a\":1}\n{\"b\":2}"[..]);
        assert_eq!(read_line(&mut input, 100).unwrap().unwrap(), br#"{"a":1}"#);
        assert_eq!(read_line(&mut input, 100).unwrap().unwrap(), br#"{"b":2}"#);
        assert!(read_line(&mut input, 100).unwrap().is_none());
        let long = vec![b'x'; 10_000];
        assert!(read_line(&mut BufReader::with_capacity(16, long.as_slice()), 100).is_err());
    }

    #[test]
    fn only_the_extension_may_start_the_host() {
        assert_eq!(run_host(&["chrome-extension://someoneelse/".into()]), 1);
        assert_eq!(
            run_host(&[origin(), "--load-anything".into()]),
            1,
            "only Chrome's own --parent-window may follow"
        );
        assert_eq!(run_host(&[]), 2);
    }

    #[cfg(unix)]
    #[test]
    fn set_up_registers_with_browsers_found_and_remove_takes_it_back() {
        let root = std::env::temp_dir().join(format!("ph-chrome-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = Layout {
            home: root.join("home"),
            data: root.join("data o'clock"),
            program: PathBuf::from("/Applications/PilotHouse.app/Contents/MacOS/pilothouse"),
        };
        let browsers = layout.browsers();
        // No browser: nothing to register with.
        assert!(set_up(&layout).is_err());
        // Chrome was run once; Edge was not.
        std::fs::create_dir_all(&browsers[0].1).unwrap();
        assert_eq!(set_up(&layout).unwrap(), vec!["Google Chrome"]);
        assert_eq!(
            set_up(&layout).unwrap(),
            vec!["Google Chrome"],
            "again: the same"
        );
        let file = browsers[0]
            .1
            .join("NativeMessagingHosts")
            .join(format!("{HOST_NAME}.json"));
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(manifest["allowed_origins"], json!([origin()]));
        assert_eq!(manifest["path"], json!(layout.launcher()));
        assert!(!browsers[2].1.exists(), "Edge is left alone");
        let script = std::fs::read_to_string(layout.launcher()).unwrap();
        assert!(script.contains(r"PILOTHOUSE_DATA_DIR='"));
        assert!(script.contains(r"o'\''clock"), "quoted for sh: {script}");
        assert!(script.contains("chrome-host \"$@\""));
        assert_eq!(
            registration(&layout),
            Registration {
                browsers: vec!["Google Chrome"],
                elsewhere: false
            }
        );
        // Another data folder's registration is reported, and not removed.
        let other = Layout {
            data: root.join("other"),
            ..layout.clone()
        };
        assert_eq!(registration(&other).browsers, Vec::<&str>::new());
        set_up(&other).unwrap();
        assert!(registration(&layout).elsewhere);
        remove(&layout).unwrap();
        assert!(file.is_file(), "the other folder's manifest stays");
        remove(&other).unwrap();
        assert!(!file.exists());
        assert!(!other.launcher().exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_extension_is_written_once() {
        let dir = std::env::temp_dir().join(format!("ph-chrome-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_files(&dir).unwrap();
        let manifest = dir.join("manifest.json");
        let written = std::fs::metadata(&manifest).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_files(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&manifest).unwrap().modified().unwrap(),
            written
        );
        assert!(dir.join("icons/icon128.png").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
