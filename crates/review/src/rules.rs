//! Review rules a repository keeps for some of its files: GitHub's path
//! instructions (`.github/instructions/NAME.instructions.md`, with an
//! `applyTo` list of globs in their front matter). A review is given those
//! that apply to the files a pull request changes.

/// Where path instructions live.
pub const FOLDER: &str = ".github/instructions";
/// What their names end with.
pub const SUFFIX: &str = ".instructions.md";

/// One instructions file: the globs it applies to (none: every file) and
/// its text without the front matter. `None` when it says it is not for
/// reviews (`excludeAgent: "code-review"`).
pub fn read(text: &str) -> Option<(Vec<String>, String)> {
    let Some(rest) = text.trim_start().strip_prefix("---") else {
        return Some((Vec::new(), text.trim().to_string()));
    };
    let Some((front, body)) = rest.split_once("\n---") else {
        return Some((Vec::new(), text.trim().to_string()));
    };
    let mut globs = Vec::new();
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        match key.trim() {
            "applyTo" => globs.extend(
                value
                    .split(',')
                    .map(|glob| glob.trim().to_string())
                    .filter(|glob| !glob.is_empty()),
            ),
            "excludeAgent" if value == "code-review" => return None,
            _ => {}
        }
    }
    Some((globs, body.trim().to_string()))
}

/// Whether any of `globs` matches any of `paths` (no globs: yes).
pub fn applies(globs: &[String], paths: &[String]) -> bool {
    globs.is_empty()
        || paths
            .iter()
            .any(|path| globs.iter().any(|glob| matches(glob, path)))
}

/// `path` against `glob`: `*` within a folder, `**` across folders, `?` one
/// character. A glob without a `/` is tried on the file's name too.
pub fn matches(glob: &str, path: &str) -> bool {
    let glob = glob.trim_start_matches("./");
    if glob_match(glob.as_bytes(), path.as_bytes()) {
        return true;
    }
    !glob.contains('/')
        && path
            .rsplit('/')
            .next()
            .is_some_and(|name| glob_match(glob.as_bytes(), name.as_bytes()))
}

fn glob_match(glob: &[u8], path: &[u8]) -> bool {
    match glob {
        [] => path.is_empty(),
        [b'*', b'*', rest @ ..] => {
            // `**/` also matches no folder at all.
            let rest = rest.strip_prefix(b"/").unwrap_or(rest);
            (0..=path.len()).any(|at| glob_match(rest, &path[at..]))
        }
        [b'*', rest @ ..] => (0..=path.len())
            .take_while(|at| *at == 0 || path[at - 1] != b'/')
            .any(|at| glob_match(rest, &path[at..])),
        [b'?', rest @ ..] => path
            .split_first()
            .is_some_and(|(first, path)| *first != b'/' && glob_match(rest, path)),
        [first, rest @ ..] => path
            .split_first()
            .is_some_and(|(head, path)| head == first && glob_match(rest, path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_as_github_reads_them() {
        assert!(matches("**/*.ts", "src/app/main.ts"));
        assert!(matches("**/*.ts", "main.ts"));
        assert!(matches("src/**", "src/a/b.rs"));
        assert!(matches("*.py", "tools/gen.py"));
        assert!(matches("docs/*.md", "docs/a.md"));
        assert!(!matches("docs/*.md", "docs/deep/a.md"));
        assert!(!matches("**/*.ts", "src/main.tsx"));
        assert!(matches("src/**/test_?.rs", "src/x/test_a.rs"));
    }

    #[test]
    fn reads_front_matter() {
        let (globs, body) =
            read("---\napplyTo: \"**/*.ts, **/*.tsx\"\n---\nUse strict mode.\n").unwrap();
        assert_eq!(globs, ["**/*.ts", "**/*.tsx"]);
        assert_eq!(body, "Use strict mode.");
        assert!(applies(&globs, &["web/a.tsx".into()]));
        assert!(!applies(&globs, &["src/a.rs".into()]));
        assert_eq!(read("---\nexcludeAgent: \"code-review\"\n---\nx"), None);
        assert_eq!(read("No front matter.").unwrap().0, Vec::<String>::new());
    }
}
