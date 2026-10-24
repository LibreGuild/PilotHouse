//! File paths as LSP `file://` URIs and back.

use std::path::{Path, PathBuf};

use lsp_types::Uri;

/// `/a b/c.rs` → `file:///a%20b/c.rs`; `C:\x\y.rs` → `file:///C:/x/y.rs`.
pub fn from_path(path: &Path) -> Option<Uri> {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut encoded = String::with_capacity(text.len() + 8);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    let prefix = if encoded.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    format!("{prefix}{encoded}").parse().ok()
}

/// The local path of a `file://` URI.
pub fn to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    let decoded = decode(rest);
    // `/C:/x` on Windows.
    let path = match decoded.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => decoded[1..].to_string(),
        _ => decoded,
    };
    Some(PathBuf::from(path))
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths() {
        let path = Path::new("/home/me/my project/src/main.rs");
        let uri = from_path(path).unwrap();
        assert_eq!(uri.as_str(), "file:///home/me/my%20project/src/main.rs");
        assert_eq!(to_path(&uri).unwrap(), path);
        let windows = from_path(Path::new(r"C:\Code\app\lib.rs")).unwrap();
        assert_eq!(windows.as_str(), "file:///C:/Code/app/lib.rs");
        assert_eq!(
            to_path(&windows).unwrap(),
            PathBuf::from("C:/Code/app/lib.rs")
        );
    }
}
