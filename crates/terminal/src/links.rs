//! Links in terminal output: a web address, or a file path with an optional
//! line (`src/main.rs:12:5`, as compilers and test runners print them).
//! ⌘-click (Ctrl-click elsewhere) opens one.

/// What a click on terminal text points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Url(String),
    /// A path as printed (relative to the shell's folder, or absolute), and
    /// a 1-based line.
    File {
        path: String,
        line: Option<u32>,
    },
}

/// Characters that end a link.
fn is_boundary(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | '(' | ')' | '<' | '>' | '[' | ']' | '{' | '}' | '|'
        )
}

/// The link at character `column` of `line`, if any.
pub fn link_at(line: &str, column: usize) -> Option<Link> {
    let chars: Vec<char> = line.chars().collect();
    if column >= chars.len() || is_boundary(chars[column]) {
        return None;
    }
    let start = chars[..column]
        .iter()
        .rposition(|c| is_boundary(*c))
        .map_or(0, |at| at + 1);
    let end = chars[column..]
        .iter()
        .position(|c| is_boundary(*c))
        .map_or(chars.len(), |at| column + at);
    let token: String = chars[start..end].iter().collect();
    let token = token.trim_end_matches(['.', ',', ';', '!', '?']);
    if token.starts_with("http://") || token.starts_with("https://") {
        return Some(Link::Url(token.trim_end_matches(':').to_string()));
    }
    // `path:line:column` or `path:line`; a Windows path keeps its drive
    // (`C:\\repo\\main.rs:12:5`).
    let drive = token.as_bytes().get(..3).is_some_and(|start| {
        start[0].is_ascii_alphabetic() && start[1] == b':' && matches!(start[2], b'\\' | b'/')
    });
    let (drive, rest) = token.split_at(if drive { 2 } else { 0 });
    let mut parts = rest.split(':');
    let path = format!("{drive}{}", parts.next()?);
    let line = parts.next().and_then(|line| line.parse::<u32>().ok());
    let looks_like_file = path.contains(['/', '\\'])
        || path.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (1..=6).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
    (looks_like_file && !path.is_empty()).then_some(Link::File { path, line })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_under_the_pointer() {
        let line = "  --> src/main.rs:12:5 (see https://example.com/a?b=1).";
        assert_eq!(
            link_at(line, 9),
            Some(Link::File {
                path: "src/main.rs".into(),
                line: Some(12)
            })
        );
        assert_eq!(
            link_at(line, 35),
            Some(Link::Url("https://example.com/a?b=1".into()))
        );
        assert_eq!(link_at(line, 1), None, "blank");
        assert_eq!(link_at("error: failed", 3), None, "a word is no link");
        assert_eq!(
            link_at("Local: http://localhost:5173/", 10),
            Some(Link::Url("http://localhost:5173/".into()))
        );
        assert_eq!(
            link_at("wrote notes.txt", 8),
            Some(Link::File {
                path: "notes.txt".into(),
                line: None
            })
        );
        assert_eq!(
            link_at(r"error at C:\repo\src\main.rs:12:5", 12),
            Some(Link::File {
                path: r"C:\repo\src\main.rs".into(),
                line: Some(12)
            })
        );
    }
}
