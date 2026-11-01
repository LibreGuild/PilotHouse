//! Undo and redo for an edit: each change keeps the edit before it.
//! Changes of the same thing in quick succession (a slider dragged, a mark
//! moved) undo as one step.

use std::time::{Duration, Instant};

/// Changes of the same thing this close together are one step.
const COALESCE: Duration = Duration::from_millis(1200);
const MAX_STEPS: usize = 200;

pub(crate) struct History<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    /// The last change (what, when).
    last: Option<(&'static str, Instant)>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
            last: None,
        }
    }
}

impl<T> History<T> {
    /// `before` is the edit a change named `what` replaced.
    pub fn record(&mut self, what: &'static str, before: T) {
        let now = Instant::now();
        let folds = self
            .last
            .is_some_and(|(last, at)| last == what && now - at < COALESCE);
        if !folds {
            self.undo.push(before);
            if self.undo.len() > MAX_STEPS {
                self.undo.remove(0);
            }
        }
        self.last = Some((what, now));
        self.redo.clear();
    }

    /// The edit before `current`, which redo brings back.
    pub fn undo(&mut self, current: T) -> Result<T, T> {
        self.last = None;
        match self.undo.pop() {
            Some(previous) => {
                self.redo.push(current);
                Ok(previous)
            }
            None => Err(current),
        }
    }

    /// The edit undone last, `current` going back to undo.
    pub fn redo(&mut self, current: T) -> Result<T, T> {
        self.last = None;
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(current);
                Ok(next)
            }
            None => Err(current),
        }
    }

    /// The next change is its own step.
    pub fn settle(&mut self) {
        self.last = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        *self = History::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_fold_and_come_back() {
        let mut history = History::default();
        history.record("brightness", 0);
        history.record("brightness", 1);
        history.record("contrast", 2);
        assert_eq!(history.undo(3), Ok(2));
        assert_eq!(history.undo(2), Ok(0), "the drag was one step");
        assert_eq!(history.undo(0), Err(0));
        assert_eq!(history.redo(0), Ok(2));
        history.record("crop", 2);
        assert!(!history.can_redo(), "a new change ends redo");
    }
}
