//! Blame: each line of a file with who last changed it and when. Lines are
//! drawn lazily, so large files scroll smoothly.

use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
    uniform_list,
};
use ph_git::blame::BlameLine;

/// Lines of the file with their blame, shared with the lazy list.
pub struct Blamed {
    pub lines: Vec<SharedString>,
    pub blame: Vec<BlameLine>,
}

pub fn render(blamed: Rc<Blamed>, cx: &App) -> AnyElement {
    let count = blamed.lines.len();
    let newest = blamed.blame.iter().map(|line| line.time).max().unwrap_or(0);
    let oldest = blamed
        .blame
        .iter()
        .filter(|line| line.time > 0)
        .map(|line| line.time)
        .min()
        .unwrap_or(newest);
    let accent = cx.theme().info;
    uniform_list("blame-lines", count, move |range, _, cx| {
        let blamed = blamed.clone();
        range
            .map(|ix| {
                let blame = blamed.blame.get(ix);
                // Show who and when only where a new block of lines starts.
                let starts_block = ix == 0
                    || blame.map(|b| &b.commit) != blamed.blame.get(ix - 1).map(|b| &b.commit);
                let age = blame
                    .map(|line| age_tint(line.time, oldest, newest, accent))
                    .unwrap_or(accent.opacity(0.));
                div()
                    .flex()
                    .h(px(20.))
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(div().w(px(3.)).h_full().bg(age))
                    .child(
                        div()
                            .w(px(150.))
                            .flex_none()
                            .px_2()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .when(starts_block, |this| {
                                this.child(match blame {
                                    Some(line) if line.uncommitted() => {
                                        "Not committed yet".to_string()
                                    }
                                    Some(line) => format!(
                                        "{}, {}",
                                        short_author(&line.author),
                                        ph_ui::time::ago(line.time)
                                    ),
                                    None => String::new(),
                                })
                            }),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .pr_2()
                            .text_right()
                            .text_color(cx.theme().muted_foreground.opacity(0.7))
                            .child((ix + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(blamed.lines[ix].clone()),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>()
    })
    .size_full()
    .into_any_element()
}

/// Newer lines get a stronger accent bar.
fn age_tint(time: i64, oldest: i64, newest: i64, accent: Hsla) -> Hsla {
    if time <= 0 {
        return accent;
    }
    let span = (newest - oldest).max(1) as f32;
    let fresh = (time - oldest) as f32 / span;
    accent.opacity(0.15 + 0.75 * fresh)
}

fn short_author(author: &str) -> &str {
    author.split_whitespace().next().unwrap_or(author)
}

/// A date like `2026-10-24` from a Unix time (UTC; enough for blame).
pub fn date(time: i64) -> String {
    let days = time.div_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_700_000_000), "2023-11-14");
        assert_eq!(date(951_782_400), "2000-02-29");
    }
}
