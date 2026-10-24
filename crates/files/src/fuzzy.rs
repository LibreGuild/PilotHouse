//! A small fuzzy matcher for file paths.
//!
//! Every query character must appear in order in the candidate. Matches score
//! higher when they are consecutive, start a path segment or word, or land in
//! the file name rather than a parent folder; shorter paths win ties. This is
//! the familiar ⌘P behaviour (`fpan` finds `files_panel.rs`).

/// A scored match: higher `score` is better; `positions` are byte offsets of
/// the matched characters, for highlighting.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch {
    pub score: i32,
    pub positions: Vec<usize>,
}

const SCORE_MATCH: i32 = 16;
const BONUS_CONSECUTIVE: i32 = 24;
const BONUS_BOUNDARY: i32 = 20;
const BONUS_FILE_NAME: i32 = 12;
const PENALTY_GAP: i32 = 1;

/// Scores `candidate` against `query`, ignoring ASCII case. `None` when some
/// query character is missing. An empty query matches everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<FuzzyMatch> {
    if query.is_empty() {
        return Some(FuzzyMatch {
            score: 0,
            positions: Vec::new(),
        });
    }
    let haystack: Vec<(usize, char)> = candidate.char_indices().collect();
    // Jumping ahead to word starts gives the nicest matches but can skip a
    // character a later query letter needed, so fall back to plain greedy.
    let slots =
        match_slots(query, &haystack, true).or_else(|| match_slots(query, &haystack, false))?;
    Some(score(candidate, &haystack, &slots))
}

/// Picks one haystack slot per query character, left to right.
fn match_slots(
    query: &str,
    haystack: &[(usize, char)],
    prefer_boundary: bool,
) -> Option<Vec<usize>> {
    let mut slots: Vec<usize> = Vec::with_capacity(query.len());
    let mut cursor = 0;
    for query_char in query.chars().map(|ch| ch.to_ascii_lowercase()) {
        let matches = |slot: &usize| haystack[*slot].1.to_ascii_lowercase() == query_char;
        let first = (cursor..haystack.len()).find(matches)?;
        let continues_run = slots.last().is_some_and(|last| last + 1 == first);
        let slot = if prefer_boundary && !continues_run {
            (first..haystack.len())
                .filter(matches)
                .find(|slot| is_boundary(haystack, *slot))
                .unwrap_or(first)
        } else {
            first
        };
        slots.push(slot);
        cursor = slot + 1;
    }
    Some(slots)
}

fn score(candidate: &str, haystack: &[(usize, char)], slots: &[usize]) -> FuzzyMatch {
    let file_name_start = candidate.rfind('/').map(|ix| ix + 1).unwrap_or(0);
    let mut score = 0;
    let mut previous: Option<usize> = None;
    for &slot in slots {
        score += SCORE_MATCH;
        match previous {
            Some(previous) if previous + 1 == slot => score += BONUS_CONSECUTIVE,
            Some(previous) => score -= PENALTY_GAP * (slot - previous) as i32,
            None => {}
        }
        if is_boundary(haystack, slot) {
            score += BONUS_BOUNDARY;
        }
        if haystack[slot].0 >= file_name_start {
            score += BONUS_FILE_NAME;
        }
        previous = Some(slot);
    }
    // Shorter candidates are more specific.
    score -= (candidate.len() / 8) as i32;
    FuzzyMatch {
        score,
        positions: slots.iter().map(|slot| haystack[*slot].0).collect(),
    }
}

fn is_boundary(haystack: &[(usize, char)], slot: usize) -> bool {
    if slot == 0 {
        return true;
    }
    let (_, previous) = haystack[slot - 1];
    let (_, current) = haystack[slot];
    matches!(previous, '/' | '_' | '-' | '.' | ' ')
        || (previous.is_lowercase() && current.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<_> = candidates
            .iter()
            .filter_map(|candidate| fuzzy_match(query, candidate).map(|m| (m.score, *candidate)))
            .collect();
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, candidate)| candidate).collect()
    }

    #[test]
    fn requires_every_character_in_order() {
        assert!(fuzzy_match("abc", "a_b_c.rs").is_some());
        assert!(fuzzy_match("acb", "abc").is_none());
        assert!(fuzzy_match("", "anything").is_some());
    }

    #[test]
    fn prefers_file_name_and_word_starts() {
        let ranked = rank(
            "fpan",
            &[
                "crates/files/src/fuzzy.rs",
                "crates/files/src/files_panel.rs",
                "docs/fpan.md",
            ],
        );
        assert_eq!(ranked[0], "docs/fpan.md");
        assert_eq!(ranked[1], "crates/files/src/files_panel.rs");
    }

    #[test]
    fn falls_back_when_jumping_to_a_boundary_would_fail() {
        // Preferring the `a` after `_` would leave no `b` to match.
        assert!(fuzzy_match("ab", "xab_a").is_some());
    }

    #[test]
    fn is_case_insensitive_and_reports_positions() {
        let found = fuzzy_match("MAIN", "src/main.rs").unwrap();
        assert_eq!(found.positions, vec![4, 5, 6, 7]);
    }
}
