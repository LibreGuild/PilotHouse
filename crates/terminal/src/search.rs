//! Finding text in a terminal's history (⌘F, Ctrl+Shift+F off macOS): the
//! newest match first, Enter for older ones, Shift+Enter for newer. The
//! match is selected and scrolled into view.

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Direction, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{Match, RegexSearch};

/// `text` as a pattern that matches it literally (case-insensitive unless
/// it has a capital, as alacritty does).
pub(crate) fn literal(text: &str) -> String {
    let mut pattern = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern
}

/// A search of one terminal.
pub(crate) struct Search {
    regex: Option<RegexSearch>,
    /// The match shown.
    pub current: Option<Match>,
}

impl Search {
    pub fn new(query: &str) -> Self {
        let regex = (!query.is_empty())
            .then(|| RegexSearch::new(&literal(query)).ok())
            .flatten();
        Self {
            regex,
            current: None,
        }
    }

    /// Finds the next match: from the end the first time, then older
    /// (`Left`) or newer (`Right`) than the one shown. Shows it.
    pub fn step<T: alacritty_terminal::event::EventListener>(
        &mut self,
        term: &mut Term<T>,
        direction: Direction,
    ) {
        let Some(regex) = self.regex.as_mut() else {
            return;
        };
        let origin = match (&self.current, direction) {
            (None, _) => Point::new(term.bottommost_line(), term.last_column()),
            (Some(found), Direction::Left) => before(*found.start(), term),
            (Some(found), Direction::Right) => after(*found.end(), term),
        };
        let side = match direction {
            Direction::Left => Side::Right,
            Direction::Right => Side::Left,
        };
        let found = term.search_next(regex, origin, direction, side, None);
        if let Some(found) = &found {
            let mut selection = Selection::new(SelectionType::Simple, *found.start(), Side::Left);
            selection.update(*found.end(), Side::Right);
            term.selection = Some(selection);
            term.scroll_to_point(*found.start());
        }
        // Past the last one, the one shown stays.
        if found.is_some() || self.current.is_none() {
            self.current = found;
        }
    }

    /// Whether there is something to find and nothing matched.
    pub fn missing(&self) -> bool {
        self.regex.is_some() && self.current.is_none()
    }
}

fn before<T>(point: Point, term: &Term<T>) -> Point {
    if point.column.0 > 0 {
        Point::new(point.line, point.column - 1)
    } else if point.line > term.topmost_line() {
        Point::new(point.line - 1, term.last_column())
    } else {
        point
    }
}

fn after<T>(point: Point, term: &Term<T>) -> Point {
    if point.column < term.last_column() {
        Point::new(point.line, point.column + 1)
    } else if point.line < term.bottommost_line() {
        Point::new(point.line + 1, Column(0))
    } else {
        point
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_text_literally() {
        assert_eq!(literal("a.b(1)"), "a\\.b\\(1\\)");
        assert!(Search::new("").regex.is_none());
        assert!(Search::new("cargo test").regex.is_some());
    }
}
