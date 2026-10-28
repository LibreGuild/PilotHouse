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

/// Whether handing `path` to the system's default app only opens it (a
/// picture, a document), rather than running it: programs, scripts,
/// installers and launchers run when opened, so the app does not offer to
/// open those (an agent may have written them).
pub fn opens_safely(path: &std::path::Path) -> bool {
    const RUNNABLE: &[&str] = &[
        "action",
        "app",
        "appimage",
        "application",
        "applescript",
        "appref-ms",
        "appx",
        "bash",
        "bat",
        "chm",
        "cmd",
        "com",
        "command",
        "cpl",
        "csh",
        "deb",
        "desktop",
        "exe",
        "fileloc",
        "fish",
        "gadget",
        "hta",
        "inetloc",
        "inf",
        "ins",
        "jar",
        "jnlp",
        "js",
        "jse",
        "ksh",
        "lnk",
        "mobileconfig",
        "msc",
        "msi",
        "msix",
        "msp",
        "pif",
        "pkg",
        "pl",
        "ps1",
        "psm1",
        "py",
        "pyw",
        "rb",
        "reg",
        "rpm",
        "scpt",
        "scptd",
        "scr",
        "sct",
        "settingcontent-ms",
        "sh",
        "shb",
        "terminal",
        "tool",
        "url",
        "vb",
        "vbe",
        "vbs",
        "webloc",
        "workflow",
        "ws",
        "wsc",
        "wsf",
        "wsh",
        "xll",
        "zsh",
    ];
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    if extension.is_some_and(|extension| RUNNABLE.contains(&extension.as_str())) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        {
            return false;
        }
    }
    true
}

/// A `/`-separated path as the path part of a URL: the characters that
/// would end it or change its meaning (space, `#`, `?`, `%`) escaped.
pub fn url_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => encoded.push_str("%20"),
            '#' => encoded.push_str("%23"),
            '?' => encoded.push_str("%3F"),
            '%' => encoded.push_str("%25"),
            c => encoded.push(c),
        }
    }
    encoded
}

/// Whether a link from content the app shows (agent output, a document)
/// is a web or mail link, the only kinds opened with one click: a `file:`
/// or custom-scheme link could launch a local program.
pub fn is_web_link(url: &str) -> bool {
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("http" | "https" | "mailto"))
}

/// Whether `path` names a file on another computer (`\\server\share`,
/// `//server/share`, `\\?\UNC\…`): on Windows, merely reading one sends
/// the user's sign-in to that computer, so paths taken from a document
/// never point there.
pub fn is_network_path(path: &str) -> bool {
    let path = path.trim_start();
    path.starts_with("\\\\")
        || path.starts_with("//")
        || path.starts_with("\\/")
        || path.starts_with("/\\")
}

/// The menu item that shows a file in the system's file manager.
pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(windows) {
        "Show in Explorer"
    } else {
        "Open Containing Folder"
    }
}

/// A `file://` URL for an absolute path, valid on every platform
/// (`file:///C:/work/app.rs` on Windows).
pub fn file_url(path: &std::path::Path) -> String {
    let encoded = url_path(&path.to_string_lossy().replace('\\', "/"));
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
    fn runnable_files_do_not_open() {
        let dir = std::env::temp_dir().join(format!("ph-opens-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let picture = dir.join("a.png");
        std::fs::write(&picture, b"x").unwrap();
        assert!(opens_safely(&picture));
        assert!(!opens_safely(&dir.join("run.command")));
        assert!(!opens_safely(&dir.join("SETUP.EXE")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let tool = dir.join("tool");
            std::fs::write(&tool, b"x").unwrap();
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(!opens_safely(&tool));
        }
    }

    #[test]
    fn only_web_links_open_and_network_paths_are_known() {
        assert!(is_web_link("https://example.com/a"));
        assert!(is_web_link("MAILTO:a@example.com"));
        assert!(!is_web_link("file:///Applications/Calculator.app"));
        assert!(!is_web_link("ms-msdt:/id"));
        assert!(!is_web_link("notes.md"));
        assert!(is_network_path(r"\\server\share\a.png"));
        assert!(is_network_path("//server/share/a.png"));
        assert!(is_network_path(r"\\?\UNC\server\a.png"));
        assert!(!is_network_path("images/a.png"));
        assert!(!is_network_path("/Users/ada/a.png"));
    }

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
