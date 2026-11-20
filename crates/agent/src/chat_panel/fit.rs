//! Which of the message box's buttons say their choice in words, for the
//! room the pane has.
//!
//! The row under the message holds the agent and model, the account, the
//! effort, the mode, what the chat has used, its menu and Send. In a narrow
//! pane that is more than fits, and a row that is cut off hides whole
//! buttons ("Auto rev…", then nothing). So as the pane narrows the buttons
//! give up their words one at a time and stay as icons, each still opening
//! its menu and saying what it is when pointed at; with the room back, the
//! words are back.
//!
//! Widths are worked out, not measured: a button's width is its icon, its
//! padding and its words at the size they are drawn. That is close enough
//! to decide who keeps their words, and needs no second pass to lay out.

/// What the row has to show.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Row {
    /// The model's name (the agent's when it names none).
    pub model: String,
    pub effort: Option<String>,
    pub mode: Option<String>,
    /// The account's name, for an agent with more than its own.
    pub account: Option<String>,
    /// The chat has used something to show.
    pub usage: bool,
    /// Stop and Send both show (the agent works and something is typed).
    pub two_actions: bool,
}

/// Which buttons keep their words. One that does not is its icon alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fit {
    pub model: bool,
    pub mode: bool,
    pub account: bool,
    pub effort: bool,
    /// The bar beside the share of the context used.
    pub usage_bar: bool,
}

impl Fit {
    /// Everything in words: a pane not measured yet, or one with the room.
    pub const ALL: Self = Self {
        model: true,
        mode: true,
        account: true,
        effort: true,
        usage_bar: true,
    };
}

/// A character of the row's small text, on average.
const CHAR: f32 = 6.4;
/// An icon-only button.
const ICON_BUTTON: f32 = 26.;
/// Between two buttons.
const GAP: f32 = 4.;
/// A model's name is cut at this, however long it is.
const MODEL_NAME: f32 = 150.;
/// Around the row: the pane's own margins and the box's padding, both
/// sides, and the space between the buttons and Send.
const AROUND: f32 = 64.;

fn words(text: &str) -> f32 {
    text.chars().count() as f32 * CHAR
}

/// The row's width with the buttons `fit` leaves in words.
fn needed(row: &Row, fit: Fit) -> f32 {
    // Attach.
    let mut width = ICON_BUTTON + GAP;
    // The agent's mark, the model's name, the caret.
    width += if fit.model {
        54. + (row.model.chars().count() as f32 * 6.8).min(MODEL_NAME)
    } else {
        46.
    } + GAP;
    let pill = |label: &Option<String>, in_words: bool| match label {
        // An icon, the words, the caret.
        Some(label) if in_words => 50. + words(label) + GAP,
        Some(_) => ICON_BUTTON + GAP,
        None => 0.,
    };
    width += match &row.account {
        Some(name) if fit.account => 16. + words(name) + GAP,
        Some(_) => ICON_BUTTON + GAP,
        None => 0.,
    };
    width += pill(&row.effort, fit.effort) + pill(&row.mode, fit.mode);
    // What the chat used, its menu, Send (and Stop).
    if row.usage {
        width += if fit.usage_bar { 72. } else { 34. } + GAP;
    }
    width += ICON_BUTTON + GAP + 28.;
    if row.two_actions {
        width += 28. + GAP;
    }
    width + AROUND
}

/// The most the row can say in a pane `width` wide (pixels at the default
/// interface size). Words go in this order, the least missed first: the
/// usage bar (its number stays), the effort, the account, the mode, and
/// last the model's name.
pub(super) fn fit(width: f32, row: &Row) -> Fit {
    let mut fit = Fit::ALL;
    let steps: [fn(&mut Fit); 5] = [
        |fit| fit.usage_bar = false,
        |fit| fit.effort = false,
        |fit| fit.account = false,
        |fit| fit.mode = false,
        |fit| fit.model = false,
    ];
    for give_up in steps {
        if needed(row, fit) <= width {
            break;
        }
        give_up(&mut fit);
    }
    fit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Row {
        Row {
            model: "Opus 5.5".into(),
            effort: Some("Medium".into()),
            mode: Some("Auto review".into()),
            account: None,
            usage: true,
            two_actions: false,
        }
    }

    #[test]
    fn a_wide_pane_says_everything() {
        assert_eq!(fit(900., &row()), Fit::ALL);
        // A row with little to say fits a narrower pane whole.
        let plain = Row {
            model: "Luna".into(),
            ..Row::default()
        };
        assert_eq!(fit(260., &plain), Fit::ALL);
    }

    #[test]
    fn a_narrowing_pane_gives_up_words_one_at_a_time() {
        let row = row();
        let said = |fit: Fit| {
            [fit.model, fit.mode, fit.account, fit.effort, fit.usage_bar]
                .iter()
                .filter(|said| **said)
                .count()
        };
        let mut seen = vec![Fit::ALL];
        for width in (200..=900).rev() {
            let now = fit(width as f32, &row);
            let last = *seen.last().unwrap();
            // Never back to more words in a narrower pane.
            assert!(said(now) <= said(last), "{width}: {now:?} after {last:?}");
            if now != last {
                seen.push(now);
            }
        }
        let off = Fit {
            model: false,
            mode: false,
            account: false,
            effort: false,
            usage_bar: false,
        };
        assert_eq!(
            seen,
            vec![
                Fit::ALL,
                Fit {
                    usage_bar: false,
                    ..Fit::ALL
                },
                Fit {
                    usage_bar: false,
                    effort: false,
                    ..Fit::ALL
                },
                // No account to give up: its step changes nothing seen.
                Fit { model: true, ..off },
                off,
            ],
            "the bar first, then the effort, the mode, and the model's name last"
        );
    }

    #[test]
    fn what_fits_is_what_was_worked_out() {
        let row = row();
        for width in [320., 400., 480., 560., 640.] {
            let fit = fit(width, &row);
            let all_icons = !(fit.model || fit.mode || fit.effort || fit.usage_bar);
            // What is chosen fits, unless nothing more can be given up.
            assert!(
                needed(&row, fit) <= width || all_icons,
                "{width}: {fit:?} needs {}",
                needed(&row, fit)
            );
        }
        // A long mode name costs its room; a second button beside Send too.
        let long = Row {
            mode: Some("Accept edits and run commands".into()),
            ..row.clone()
        };
        assert!(needed(&long, Fit::ALL) > needed(&row, Fit::ALL));
        let busy = Row {
            two_actions: true,
            ..row.clone()
        };
        assert!(needed(&busy, Fit::ALL) > needed(&row, Fit::ALL));
    }
}
