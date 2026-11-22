//! Completing mentions in the composer: `@` offers the live sources
//! (`@screenshot`, `@log`, `@test-run`), definitions (`@path:line`), pull
//! requests (`@pr:12`), files and folders; `/` at the start
//! of a message offers the agent's commands (`/review`, `/compact`…). What is
//! inserted is the plain text the composer and the agent already understand
//! (see [`crate::attachments::mentions`]).

use std::ops::Range;

/// Most suggestions shown.
pub const LIMIT: usize = 8;
/// The most of an agent's commands offered for `/` (the menu shows
/// [`LIMIT`] at a time).
pub const COMMANDS: usize = 200;

/// The mention being typed at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typing {
    /// `@` or `/`.
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
    // A command only opens a message.
    if !(trigger == '@' || (trigger == '/' && before[..start].trim().is_empty())) {
        return None;
    }
    let query = word[trigger.len_utf8()..].to_string();
    // Never another trigger inside one.
    if query.contains('@') {
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

/// What else `@` can offer besides files.
#[derive(Default)]
pub struct Extra<'a> {
    pub symbols: &'a [ph_files::symbols::Symbol],
    /// Open pull requests: number and title.
    pub pulls: &'a [(u64, String)],
    /// The agent's commands: name (without `/`) and description.
    pub commands: &'a [(String, String)],
    /// `@tabs` can read the Chrome tabs the user shared.
    pub tabs: bool,
}

/// Whether what is typed after `@` asks for a pull request: `pr`, `pr:…`,
/// `pr#…` or `pr12`. Anything else that starts with those letters is a
/// file or a definition (`@preload`, `@project`).
pub fn asks_for_pulls(query: &str) -> bool {
    query.strip_prefix("pr").is_some_and(|rest| {
        !rest.contains('/')
            && (rest.starts_with([':', '#']) || rest.chars().all(|c| c.is_ascii_digit()))
    })
}

/// What to offer for `typing`, best first.
pub fn suggestions(typing: &Typing, files: &[impl AsRef<str>], extra: &Extra) -> Vec<Suggestion> {
    let query = typing.query.as_str();
    match typing.trigger {
        '/' => {
            let lower = query.to_lowercase();
            let mut matched: Vec<(i32, &(String, String))> = extra
                .commands
                .iter()
                .filter_map(|command| {
                    let name = command.0.to_lowercase();
                    if lower.is_empty() {
                        Some((0, command))
                    } else if name.starts_with(&lower) {
                        Some((1_000 - name.len() as i32, command))
                    } else {
                        ph_files::fuzzy::fuzzy_match(&lower, &name)
                            .map(|found| (found.score, command))
                    }
                })
                .collect();
            matched.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.0.cmp(&b.1.0)));
            // All of them: the menu shows a few at a time and moves with
            // the selection, so a command past the eighth is reached
            // without knowing its name.
            matched
                .into_iter()
                .take(COMMANDS)
                .map(|(_, (name, description))| Suggestion {
                    label: format!("/{name}"),
                    detail: description.clone(),
                    insert: format!("/{name} "),
                })
                .collect()
        }
        _ if asks_for_pulls(query) => {
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
                    "How the tests last went, with the failing screen",
                ),
                ("pr:", "A pull request of this workspace"),
                ("tabs", "The text of the Chrome tabs you shared with agents"),
            ]
            .into_iter()
            .filter(|(name, _)| *name != "tabs" || extra.tabs)
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
                // A folder (`src/app/`) shows as `app/` in `src`.
                let trimmed = path.trim_end_matches('/');
                let (parent, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
                let name = if path.ends_with('/') {
                    format!("{name}/")
                } else {
                    name.to_string()
                };
                Suggestion {
                    label: name,
                    detail: parent.to_string(),
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
        assert_eq!(typing_at("#1", 2), None, "a # starts nothing");
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
    fn folders_are_offered_with_a_slash() {
        let entries = ["src/", "src/app/", "src/app/mod.rs"];
        let typing = typing_at("@app", 4).unwrap();
        let found = suggestions(&typing, &entries, &Extra::default());
        let folder = found
            .iter()
            .find(|item| item.insert == "@src/app/ ")
            .expect("the folder");
        assert_eq!(
            (folder.label.as_str(), folder.detail.as_str()),
            ("app/", "src")
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
        let found = suggestions(&typing, &files, &Extra::default());
        assert_eq!(found[0].insert, "@screenshot ", "the live source first");
        assert!(found.iter().any(|s| s.insert == "@docs/screens.md "));

        let found = suggestions(&typing_at("@main", 5).unwrap(), &files, &Extra::default());
        assert_eq!(
            (found[0].label.as_str(), found[0].detail.as_str()),
            ("main.rs", "src")
        );
        assert!(suggestions(&typing_at("@", 1).unwrap(), &files, &Extra::default()).len() <= LIMIT);
    }

    #[test]
    fn commands_only_open_a_message() {
        let commands = [
            ("review".to_string(), "Review the changes".to_string()),
            ("compact".to_string(), "Summarize the chat".to_string()),
        ];
        let extra = Extra {
            tabs: false,
            commands: &commands,
            ..Extra::default()
        };
        let typing = typing_at("/rev", 4).unwrap();
        let found = suggestions(&typing, &[] as &[&str], &extra);
        assert_eq!(found[0].insert, "/review ");
        assert!(typing_at("fix a/b", 7).is_none());
        assert!(typing_at("see /rev", 8).is_none());
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
            tabs: false,
            symbols: std::slice::from_ref(&symbol),
            pulls: &pulls,
            ..Extra::default()
        };
        let found = suggestions(&typing_at("@parse", 6).unwrap(), &[] as &[&str], &extra);
        assert_eq!(found[0].insert, "@src/data.rs:42 ");
        let prs = suggestions(&typing_at("@pr:log", 7).unwrap(), &[] as &[&str], &extra);
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].insert, "@pr:12 ");
    }

    #[test]
    fn names_that_start_with_pr_are_files_not_pull_requests() {
        for query in ["pr", "pr:", "pr:12", "pr#7", "pr12", "pr:fix"] {
            assert!(asks_for_pulls(query), "{query}");
        }
        for query in ["p", "preload", "project", "pr1x", "pr/", "src/pr", "pr:a/b"] {
            assert!(!asks_for_pulls(query), "{query}");
        }
        let files = ["crates/agent/src/preload.rs", "docs/project.md"];
        let pulls = [(12, "Preload the models".to_string())];
        let extra = Extra {
            pulls: &pulls,
            ..Extra::default()
        };
        let found = suggestions(&typing_at("@prel", 5).unwrap(), &files, &extra);
        assert_eq!(found[0].insert, "@crates/agent/src/preload.rs ");
        // With no pull requests loaded either: the file, not a note that
        // they load.
        let found = suggestions(&typing_at("@proj", 5).unwrap(), &files, &Extra::default());
        assert_eq!(found[0].insert, "@docs/project.md ");
        let found = suggestions(&typing_at("@pr1", 4).unwrap(), &files, &extra);
        assert_eq!(found[0].insert, "@pr:12 ");
    }
}
