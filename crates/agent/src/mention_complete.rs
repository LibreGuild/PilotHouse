//! Completing mentions in the composer: `@` offers the live sources
//! (`@screenshot`, `@log`, `@test-run`), definitions (`@path:line`), pull
//! requests (`@pr:12`) and files; `#` offers board cards. What is inserted
//! is the plain mention text the composer already understands (see
//! [`crate::attachments::mentions`]).

use std::ops::Range;

/// Most suggestions shown.
pub const LIMIT: usize = 8;

/// The mention being typed at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typing {
    /// `@` or `#`.
    pub trigger: char,
    /// What follows the trigger, up to the cursor.
    pub query: String,
    /// The text to replace: the trigger through the cursor.
    pub range: Range<usize>,
}

/// The mention ending at byte `cursor`, if one is being typed: a trigger at
/// the start or after whitespace, then no whitespace up to the cursor.
pub fn typing_at(text: &str, cursor: usize) -> Option<Typing> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| !c.is_whitespace())
        .last()
        .map(|(ix, _)| ix)?;
    let word = &before[start..];
    let trigger = word.chars().next()?;
    if !matches!(trigger, '@' | '#') {
        return None;
    }
    let query = word[trigger.len_utf8()..].to_string();
    // `#` takes a card number or words of a title; never another trigger.
    if query.contains(['@', '#']) {
        return None;
    }
    Some(Typing {
        trigger,
        query,
        range: start..cursor,
    })
}

/// One suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    pub detail: String,
    /// The mention text put in place of the typed one (with a space after).
    pub insert: String,
}

/// Board cards as `(number, title)`.
pub type CardList<'a> = &'a [(i64, String)];

/// What else `@` can offer besides files.
#[derive(Default)]
pub struct Extra<'a> {
    pub symbols: &'a [ph_files::symbols::Symbol],
    /// Open pull requests: number and title.
    pub pulls: &'a [(u64, String)],
}

/// What to offer for `typing`, best first.
pub fn suggestions(
    typing: &Typing,
    files: &[impl AsRef<str>],
    cards: CardList,
    extra: &Extra,
) -> Vec<Suggestion> {
    let query = typing.query.as_str();
    match typing.trigger {
        '#' => {
            let lower = query.to_lowercase();
            let mut matched: Vec<(i32, &(i64, String))> = cards
                .iter()
                .filter_map(|card| {
                    let number = card.0.to_string();
                    if query.is_empty() {
                        Some((0, card))
                    } else if number.starts_with(query) {
                        Some((1_000 - number.len() as i32, card))
                    } else {
                        ph_files::fuzzy::fuzzy_match(&lower, &card.1.to_lowercase())
                            .map(|found| (found.score, card))
                    }
                })
                .collect();
            matched.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.0.cmp(&a.1.0)));
            matched
                .into_iter()
                .take(LIMIT)
                .map(|(_, (number, title))| Suggestion {
                    label: format!("#{number}"),
                    detail: title.clone(),
                    insert: format!("#{number} "),
                })
                .collect()
        }
        _ if query.starts_with("pr") && !query.contains('/') => {
            let wanted = query
                .trim_start_matches("pr")
                .trim_start_matches([':', '#'])
                .to_lowercase();
            if extra.pulls.is_empty() {
                return vec![Suggestion {
                    label: "@pr:".into(),
                    detail: "Pull requests load from GitHub…".into(),
                    insert: "@pr:".into(),
                }];
            }
            extra
                .pulls
                .iter()
                .filter(|(number, title)| {
                    wanted.is_empty()
                        || number.to_string().starts_with(&wanted)
                        || title.to_lowercase().contains(&wanted)
                })
                .take(LIMIT)
                .map(|(number, title)| Suggestion {
                    label: format!("@pr:{number}"),
                    detail: title.clone(),
                    insert: format!("@pr:{number} "),
                })
                .collect()
        }
        _ => {
            let mut out: Vec<Suggestion> = [
                (
                    "screenshot",
                    "The browser or device on screen, with its marks",
                ),
                ("log", "Recent console, network or logcat lines"),
                (
                    "test-run",
                    "The newest UI test results, with the failing screen",
                ),
                ("pr:", "A pull request of this workspace"),
            ]
            .into_iter()
            .filter(|(name, _)| name.starts_with(query))
            .map(|(name, detail)| Suggestion {
                label: format!("@{name}"),
                detail: detail.to_string(),
                insert: format!("@{name} "),
            })
            .collect();
            // Definitions whose name starts with what is typed come next.
            if query.len() >= 2 && !query.contains(['/', '.', ':']) {
                let lower = query.to_lowercase();
                let mut symbols: Vec<&ph_files::symbols::Symbol> = extra
                    .symbols
                    .iter()
                    .filter(|symbol| symbol.name.to_lowercase().starts_with(&lower))
                    .collect();
                symbols.sort_by_key(|symbol| (symbol.name.len(), symbol.path.len()));
                out.extend(symbols.into_iter().take(3).map(|symbol| Suggestion {
                    label: symbol.name.clone(),
                    detail: format!("{} · {}:{}", symbol.kind, symbol.path, symbol.line),
                    insert: format!("@{}:{} ", symbol.path, symbol.line),
                }));
            }
            let mut matched: Vec<(i32, &str)> = files
                .iter()
                .map(AsRef::as_ref)
                .filter_map(|path| {
                    if query.is_empty() {
                        Some((0, path))
                    } else {
                        ph_files::fuzzy::fuzzy_match(query, path).map(|found| (found.score, path))
                    }
                })
                .collect();
            // Better match first; then the shorter path.
            matched.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
            out.extend(matched.into_iter().take(LIMIT).map(|(_, path)| {
                let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
                Suggestion {
                    label: name.to_string(),
                    detail: folder.to_string(),
                    insert: format!("@{path} "),
                }
            }));
            out.truncate(LIMIT);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_mention_being_typed() {
        let typing = typing_at("look at @src/ma", 15).unwrap();
        assert_eq!(
            (typing.trigger, typing.query.as_str(), typing.range),
            ('@', "src/ma", 8..15)
        );
        assert_eq!(typing_at("#1", 2).unwrap().query, "1");
        assert_eq!(typing_at("@", 1).unwrap().query, "");
        assert_eq!(typing_at("mail me@home", 12), None, "not after a word");
        assert_eq!(typing_at("@src ", 5), None, "finished with a space");
        assert_eq!(typing_at("plain words", 5), None);
        assert_eq!(
            typing_at("héllo @x", 9).unwrap().range,
            7..9,
            "byte offsets"
        );
    }

    #[test]
    fn files_and_live_sources_for_at() {
        let files = [
            "src/main.rs",
            "src/app/mod.rs",
            "README.md",
            "docs/screens.md",
        ];
        let typing = typing_at("@scr", 4).unwrap();
        let found = suggestions(&typing, &files, &[], &Extra::default());
        assert_eq!(found[0].insert, "@screenshot ", "the live source first");
        assert!(found.iter().any(|s| s.insert == "@docs/screens.md "));

        let found = suggestions(
            &typing_at("@main", 5).unwrap(),
            &files,
            &[],
            &Extra::default(),
        );
        assert_eq!(
            (found[0].label.as_str(), found[0].detail.as_str()),
            ("main.rs", "src")
        );
        assert!(
            suggestions(&typing_at("@", 1).unwrap(), &files, &[], &Extra::default()).len() <= LIMIT
        );
    }

    #[test]
    fn cards_by_number_or_title_for_hash() {
        let cards = vec![
            (3, "Fix login retry".to_string()),
            (12, "Add dark mode".to_string()),
            (31, "Login page copy".to_string()),
        ];
        let by_number = suggestions(
            &typing_at("#3", 2).unwrap(),
            &[] as &[&str],
            &cards,
            &Extra::default(),
        );
        assert_eq!(by_number[0].insert, "#3 ", "an exact number first");
        assert_eq!(by_number[1].insert, "#31 ");
        let by_title = suggestions(
            &typing_at("#login", 6).unwrap(),
            &[] as &[&str],
            &cards,
            &Extra::default(),
        );
        let numbers: Vec<_> = by_title.iter().map(|s| s.label.as_str()).collect();
        assert!(numbers.contains(&"#3") && numbers.contains(&"#31") && !numbers.contains(&"#12"));
    }

    #[test]
    fn definitions_and_pull_requests_for_at() {
        let symbol = ph_files::symbols::Symbol {
            name: "parse_csv".into(),
            kind: "fn",
            path: std::sync::Arc::from("src/data.rs"),
            line: 42,
        };
        let pulls = [(12, "Fix login".to_string()), (7, "Docs".to_string())];
        let extra = Extra {
            symbols: std::slice::from_ref(&symbol),
            pulls: &pulls,
        };
        let found = suggestions(
            &typing_at("@parse", 6).unwrap(),
            &[] as &[&str],
            &[],
            &extra,
        );
        assert_eq!(found[0].insert, "@src/data.rs:42 ");
        let prs = suggestions(
            &typing_at("@pr:log", 7).unwrap(),
            &[] as &[&str],
            &[],
            &extra,
        );
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].insert, "@pr:12 ");
    }
}
