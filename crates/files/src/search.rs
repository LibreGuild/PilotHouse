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

pub fn search_project(root: &Path, query: &str) -> anyhow::Result<SearchResults> {
    if query.trim().is_empty() {
        return Ok(SearchResults::default());
    }
    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(true)
        .case_smart(true)
        .build(query)?;
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
                if full.load(Ordering::Relaxed) {
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

        let lower = search_project(&root, "pilot").unwrap();
        assert_eq!(lower.matches.len(), 2);

        let upper = search_project(&root, "Pilot").unwrap();
        assert_eq!(upper.matches.len(), 1);
        assert_eq!(upper.matches[0].path, "src/a.rs");
        assert_eq!(upper.matches[0].line, 2);
    }
}
