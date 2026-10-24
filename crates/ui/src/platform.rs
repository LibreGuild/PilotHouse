//! Platform conventions for text shown to people.

/// A shortcut written as a GPUI binding (`secondary-shift-s`) in the
/// platform's own style: `⌘⇧S` on macOS, `Ctrl+Shift+S` elsewhere.
pub fn shortcut(binding: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let mut parts = Vec::new();
    let keys: Vec<&str> = binding.split('-').collect();
    let (modifiers, key) = keys.split_at(keys.len().saturating_sub(1));
    for modifier in modifiers {
        parts.push(match (*modifier, mac) {
            ("secondary" | "cmd" | "platform", true) => "⌘",
            ("secondary" | "ctrl" | "control", false) => "Ctrl",
            ("ctrl" | "control", true) => "⌃",
            ("cmd" | "platform", false) => "Win",
            ("alt", true) => "⌥",
            ("alt", false) => "Alt",
            ("shift", true) => "⇧",
            ("shift", false) => "Shift",
            (other, _) => other,
        });
    }
    let key = key
        .first()
        .map(|key| key.to_uppercase())
        .unwrap_or_default();
    if mac {
        format!("{}{key}", parts.concat())
    } else {
        parts.push(&key);
        parts.join("+")
    }
}

/// A `file://` URL for an absolute path, valid on every platform
/// (`file:///C:/work/app.rs` on Windows).
pub fn file_url(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let encoded: String = text
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3F".to_string(),
            '%' => "%25".to_string(),
            c => c.to_string(),
        })
        .collect();
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// A folder for people to read: the home folder shown as `~`.
pub fn display_path(path: &std::path::Path) -> String {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from);
    shorten_home(path, home.as_deref())
}

fn shorten_home(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_shown_as_a_tilde() {
        let home = std::path::Path::new("/Users/ada");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            shorten_home(std::path::Path::new("/Users/ada/notes"), Some(home)),
            format!("~{sep}notes")
        );
        assert_eq!(shorten_home(home, Some(home)), "~");
        assert_eq!(
            shorten_home(std::path::Path::new("/tmp/x"), Some(home)),
            "/tmp/x"
        );
        assert_eq!(shorten_home(std::path::Path::new("/tmp/x"), None), "/tmp/x");
    }

    #[test]
    fn writes_shortcuts_and_urls() {
        if cfg!(target_os = "macos") {
            assert_eq!(shortcut("secondary-shift-s"), "⌘⇧S");
        } else {
            assert_eq!(shortcut("secondary-shift-s"), "Ctrl+Shift+S");
        }
        assert_eq!(
            file_url(std::path::Path::new("/a b/c#d.rs")),
            "file:///a%20b/c%23d.rs"
        );
        assert_eq!(
            file_url(std::path::Path::new(r"C:\work\app.rs")),
            "file:///C:/work/app.rs"
        );
    }
}
