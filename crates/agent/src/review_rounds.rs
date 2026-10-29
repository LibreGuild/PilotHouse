//! Word that a PR review's round was recorded, for every view showing that
//! pull request: the review's chat may run in another window, another
//! workspace or the background, so its own panel cannot be the one to say.

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global};

pub struct ReviewRounds;

/// A round of review `review_id` was saved: its findings and walkthrough
/// are in the store.
pub struct RoundRecorded {
    pub review_id: i64,
}

impl EventEmitter<RoundRecorded> for ReviewRounds {}

struct GlobalReviewRounds(Entity<ReviewRounds>);

impl Global for GlobalReviewRounds {}

impl ReviewRounds {
    /// The app's one emitter, to subscribe to.
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(rounds) = cx.try_global::<GlobalReviewRounds>() {
            return rounds.0.clone();
        }
        let rounds = cx.new(|_| Self);
        cx.set_global(GlobalReviewRounds(rounds.clone()));
        rounds
    }

    pub(crate) fn recorded(review_id: i64, cx: &mut App) {
        Self::global(cx).update(cx, |_, cx| cx.emit(RoundRecorded { review_id }));
    }
}
