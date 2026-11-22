//! What an artifact's page stores: values by key, kept by the app in the
//! artifact's own folder (`data/`, one JSON file per key).
//!
//! A browser's own storage belongs to that browser: what a page keeps in
//! the built-in browser is not there in the user's Chrome, nor in another
//! workspace's pane. Kept here, it is the same wherever the page is opened,
//! goes with the folder, and an agent can be shown it.
//!
//! One file per key, so a change writes that key alone, and the folder
//! reads well by hand: `todos.json`, `settings.json`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// One value, and all of an artifact's values together.
pub const MAX_VALUE: usize = 5 * 1024 * 1024;
pub const MAX_TOTAL: u64 = 100 * 1024 * 1024;
pub const MAX_KEYS: usize = 10_000;
const MAX_KEY: usize = 200;
const EXTENSION: &str = ".json";

/// The file a key is kept in. Lowercase letters, digits, `-`, `_` and a dot
/// inside a word stand as they are; anything else is written as `%XX`,
/// capitals too, so `Todos` and `todos` stay two keys on a file system that
/// tells no cases apart.
pub fn file_name(key: &str) -> Result<String> {
    if key.is_empty() || key.len() > MAX_KEY {
        bail!("a key is 1 to {MAX_KEY} bytes long");
    }
    if key.chars().any(char::is_control) {
        bail!("a key cannot hold control characters");
    }
    let mut name = String::with_capacity(key.len() + EXTENSION.len());
    let mut after_plain = false;
    for byte in key.bytes() {
        let word = byte.is_ascii_lowercase() || byte.is_ascii_digit();
        // A dot only inside a word (`v1.2`): never first, where it would
        // hide the file, nor doubled.
        let plain = word || matches!(byte, b'-' | b'_') || (byte == b'.' && after_plain);
        if plain {
            name.push(byte as char);
        } else {
            name.push_str(&format!("%{byte:02X}"));
        }
        after_plain = word;
    }
    // Windows keeps a few names for devices, whatever follows the dot.
    let stem = name.split('.').next().unwrap_or_default();
    let device = matches!(stem, "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit());
    if device {
        name = format!("%{:02X}{}", name.as_bytes()[0], &name[1..]);
    }
    name.push_str(EXTENSION);
    Ok(name)
}

/// The key a file holds, if it is one of ours.
fn key_of(name: &str) -> Option<String> {
    let encoded = name.strip_suffix(EXTENSION)?;
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut rest = encoded.bytes();
    while let Some(byte) = rest.next() {
        if byte == b'%' {
            let hex = [rest.next()?, rest.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

/// Every key in `dir` with its value's size, by name.
pub fn keys(dir: &Path) -> Vec<(String, u64)> {
    let mut found: Vec<(String, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let key = key_of(&entry.file_name().to_string_lossy())?;
            let meta = entry.metadata().ok()?;
            meta.is_file().then_some((key, meta.len()))
        })
        .collect();
    found.sort();
    found
}

fn path(dir: &Path, key: &str) -> Result<PathBuf> {
    Ok(dir.join(file_name(key)?))
}

/// The value under `key`, as the JSON text it was stored as.
pub fn get(dir: &Path, key: &str) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path(dir, key)?) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).context("could not read the value"),
    }
}

/// Stores `json` (any JSON value) under `key`, within the limits.
pub fn set(dir: &Path, key: &str, json: &[u8]) -> Result<()> {
    let file = path(dir, key)?;
    if json.len() > MAX_VALUE {
        bail!(
            "a value can be {} MB at most; split it over several keys",
            MAX_VALUE / (1024 * 1024)
        );
    }
    serde_json::from_slice::<serde::de::IgnoredAny>(json).context("the value is not JSON")?;
    let kept = keys(dir);
    let replaced = kept
        .iter()
        .find(|(known, _)| known == key)
        .map(|(_, bytes)| *bytes);
    if replaced.is_none() && kept.len() >= MAX_KEYS {
        bail!("this artifact already stores {MAX_KEYS} keys, the most it can");
    }
    let total: u64 = kept.iter().map(|(_, bytes)| bytes).sum();
    if total - replaced.unwrap_or(0) + json.len() as u64 > MAX_TOTAL {
        bail!(
            "this artifact's storage is full ({} MB)",
            MAX_TOTAL / (1024 * 1024)
        );
    }
    std::fs::create_dir_all(dir).context("could not make the storage folder")?;
    crate::library::write_atomic(&file, json)
}

/// Forgets `key`; says whether there was one.
pub fn remove(dir: &Path, key: &str) -> Result<bool> {
    match std::fs::remove_file(path(dir, key)?) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).context("could not remove the value"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_file_names_and_back() {
        assert_eq!(file_name("todos").unwrap(), "todos.json");
        assert_eq!(file_name("budget-2026.v2").unwrap(), "budget-2026.v2.json");
        assert_eq!(file_name("Todos").unwrap(), "%54odos.json");
        assert_eq!(file_name("../x y").unwrap(), "%2E%2E%2Fx%20y.json");
        assert_eq!(file_name("con").unwrap(), "%63on.json");
        for key in [
            "todos",
            "Todos",
            "../x y",
            "scores/été",
            ".hidden",
            "a%41",
            "com1",
            "a..b",
        ] {
            assert_eq!(key_of(&file_name(key).unwrap()).as_deref(), Some(key));
        }
        assert!(file_name("").is_err());
        assert!(file_name("a\nb").is_err());
        assert!(file_name(&"k".repeat(201)).is_err());
        assert_eq!(key_of("artifact.txt"), None);
    }

    #[test]
    fn values_are_kept_listed_and_removed() {
        let dir = std::env::temp_dir().join(format!("ph-artifact-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(get(&dir, "todos").unwrap(), None);
        set(&dir, "todos", br#"[{"text":"milk"}]"#).unwrap();
        set(&dir, "Settings", b"{}").unwrap();
        assert_eq!(
            get(&dir, "todos").unwrap().as_deref(),
            Some(&br#"[{"text":"milk"}]"#[..])
        );
        assert_eq!(
            keys(&dir),
            [("Settings".to_string(), 2), ("todos".to_string(), 17)]
        );
        assert!(set(&dir, "bad", b"{not json").is_err());
        assert!(set(&dir, "big", &vec![b'1'; MAX_VALUE + 1]).is_err());
        assert!(remove(&dir, "todos").unwrap());
        assert!(!remove(&dir, "todos").unwrap());
        // Nothing of a key reaches outside the folder.
        set(&dir, "../../escape", b"1").unwrap();
        assert!(dir.join("%2E%2E%2F%2E%2E%2Fescape.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
