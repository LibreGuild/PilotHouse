//! Project-wide text search, using ripgrep's own crates.
//!
//! Plain-text, smart-case matching (case-insensitive unless the query has an
//! upper-case letter), skipping ignored and binary files like `rg` does, on
//! every core.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};
use ignore::{WalkBuilder, WalkState};

/// Stop after this many matching lines; the UI says when results were cut.
pub const MAX_RESULTS: usize = 500;
/// Long minified lines are clipped so one hit cannot dominate the list.
const MAX_LINE_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchMatch {
    /// Project-relative path with `/` separators.
    pub path: String,
    /// 1-based line number.
    pub line: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchResults {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

/// Searches the files under `root` for `query`. Stops early, with what it
/// found so far, once `cancel` is set (a newer search replaced it).
pub fn search_project(
    root: &Path,
    query: &str,
    cancel: &AtomicBool,
) -> anyhow::Result<SearchResults> {
    if query.trim().is_empty() {
        return Ok(SearchResults::default());
    }
    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(true)
        .case_smart(true)
        .build(query)?;
    search_with(root, matcher, cancel)
}

/// Where the identifier `word` appears as a whole word (its usages, when no
/// language server says better), case-sensitive.
pub fn search_word(root: &Path, word: &str, cancel: &AtomicBool) -> anyhow::Result<SearchResults> {
    if word.is_empty() {
        return Ok(SearchResults::default());
    }
    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(true)
        .word(true)
        .build(word)?;
    search_with(root, matcher, cancel)
}

fn search_with(
    root: &Path,
    matcher: grep_regex::RegexMatcher,
    cancel: &AtomicBool,
) -> anyhow::Result<SearchResults> {
    // Every core searches its own files, like `rg`; the hits are sorted at
    // the end so the list does not jump between runs.
    let found = Mutex::new(Vec::<SearchMatch>::new());
    let full = AtomicBool::new(false);
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build_parallel()
        .run(|| {
            let (matcher, found, full) = (matcher.clone(), &found, &full);
            let mut searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .line_number(true)
                .build();
            Box::new(move |entry| {
                if full.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return WalkState::Continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                let mut hits = Vec::new();
                let sink = UTF8(|line, text| {
                    hits.push(SearchMatch {
                        path: relative.clone(),
                        line,
                        text: text.trim_end().chars().take(MAX_LINE_CHARS).collect(),
                    });
                    Ok(hits.len() < MAX_RESULTS)
                });
                // Unreadable files are skipped, like ripgrep does.
                let _ = searcher.search_path(&matcher, entry.path(), sink);
                if hits.is_empty() {
                    return WalkState::Continue;
                }
                let mut found = found.lock().unwrap_or_else(|err| err.into_inner());
                found.extend(hits);
                if found.len() >= MAX_RESULTS {
                    full.store(true, Ordering::Relaxed);
                    return WalkState::Quit;
                }
                WalkState::Continue
            })
        });
    let mut matches = found.into_inner().unwrap_or_else(|err| err.into_inner());
    matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let truncated = matches.len() >= MAX_RESULTS;
    matches.truncate(MAX_RESULTS);
    Ok(SearchResults { matches, truncated })
}

/// Most file-name matches shown above the text matches.
pub const MAX_FILE_MATCHES: usize = 20;

/// Paths in `files` whose name, or else path, contains `query` (ignoring
/// case), best first: names that start with it, then names that contain
/// it, then paths; shorter paths first within each. `keep` limits them to
/// a folder.
pub fn match_file_names(
    files: &[std::sync::Arc<str>],
    query: &str,
    keep: impl Fn(&str) -> bool,
) -> Vec<std::sync::Arc<str>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<(u8, usize, &std::sync::Arc<str>)> = files
        .iter()
        .filter_map(|path| {
            let lower = path.to_lowercase();
            let name = lower.rsplit('/').next().unwrap_or(&lower);
            let rank = if name.starts_with(&query) {
                0
            } else if name.contains(&query) {
                1
            } else if lower.contains(&query) {
                2
            } else {
                return None;
            };
            keep(path).then_some((rank, path.len(), path))
        })
        .collect();
    found.sort_unstable_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    found
        .into_iter()
        .take(MAX_FILE_MATCHES)
        .map(|(_, _, path)| path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_lines_with_smart_case() {
        let root = std::env::temp_dir().join(format!("ph-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn main() {}\nlet Pilot = 1;\n").unwrap();
        std::fs::write(root.join("b.txt"), "pilothouse\n").unwrap();

        let go = AtomicBool::new(false);
        let lower = search_project(&root, "pilot", &go).unwrap();
        assert_eq!(lower.matches.len(), 2);

        let upper = search_project(&root, "Pilot", &go).unwrap();
        assert_eq!(upper.matches.len(), 1);
        assert_eq!(upper.matches[0].path, "src/a.rs");
        assert_eq!(upper.matches[0].line, 2);
        // A replaced search stops.
        let stopped = search_project(&root, "pilot", &AtomicBool::new(true)).unwrap();
        assert!(stopped.matches.is_empty());
    }

    #[test]
    fn whole_words_only() {
        let root = std::env::temp_dir().join(format!("ph-words-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.rs"),
            "load();\nreload();\nlet Load = load_all;\nx.load\n",
        )
        .unwrap();
        let found = search_word(&root, "load", &AtomicBool::new(false)).unwrap();
        let lines: Vec<u64> = found.matches.iter().map(|hit| hit.line).collect();
        assert_eq!(lines, [1, 4]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_names_rank_before_paths() {
        let files: Vec<std::sync::Arc<str>> = [
            "docs/menu-notes.md",
            "src/menu/mod.rs",
            "src/tree_menu.rs",
            "src/menu.rs",
            "README.md",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        let found = match_file_names(&files, "Menu", |_| true);
        let found: Vec<&str> = found.iter().map(|path| path.as_ref()).collect();
        assert_eq!(
            found,
            [
                "src/menu.rs",
                "docs/menu-notes.md",
                "src/tree_menu.rs",
                "src/menu/mod.rs"
            ]
        );
        let inside = match_file_names(&files, "menu", |path| path.starts_with("docs/"));
        assert_eq!(inside.len(), 1);
        assert!(match_file_names(&files, "  ", |_| true).is_empty());
    }
}
