//! How a screen looks: a test's look checks ([`Step::ExpectLook`]) compare
//! the screen with the picture accepted for that check.
//!
//! Only where a test asks: comparing every step's screen said "looks
//! different" for a clock and nothing for a misspelt heading (a share of
//! the whole screen was allowed to change, and words are a small share).
//! Here any change counts, down to a few pixels, and what may differ is
//! named: the areas a check ignores. *Ignore what changed* adds the areas
//! that did, so a clock is told once.
//!
//! Accepted pictures are kept per machine in the data directory
//! (`test-baselines/<workspace>/<test>/<variant>/<check id>.png`), not in
//! the project: fonts and rendering differ between machines, so another
//! person's would differ everywhere. Each screen size or device has its
//! own.
//!
//! [`Step::ExpectLook`]: crate::model::Step::ExpectLook

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use ph_media::Frame;
use serde::{Deserialize, Serialize};

/// Pixels that may differ before a screen "looks different": the specks a
/// renderer leaves at an edge, not a letter.
pub const TOLERANCE_PIXELS: u32 = 12;
/// How far apart two pixels' colours must be to differ (per channel, of
/// 255): below it is a shade no one sees.
const CHANNEL_TOLERANCE: u8 = 24;
/// The squares changes are gathered in to find the areas they make.
const CELL: u32 = 16;
/// Areas of change reported at most; more are joined into the last.
const MAX_AREAS: usize = 12;

/// How a look check's screen compares with its accepted picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visual {
    /// Share of pixels that changed, 0.0–1.0 (outside the ignored areas).
    pub changed: f32,
    /// How many pixels that is.
    #[serde(default)]
    pub pixels: u32,
    /// The changes tinted red, relative to the report's folder; only when it
    /// looks different.
    #[serde(default)]
    pub diff: Option<String>,
    /// The accepted picture it was compared with, copied into the report's
    /// folder; only when it looks different. (A copy: the accepted one is
    /// replaced when a new look is accepted, and a report says what it saw.)
    #[serde(default)]
    pub accepted: Option<String>,
    /// Where it changed: x, y, width, height in the picture's pixels.
    #[serde(default)]
    pub areas: Vec<[u32; 4]>,
    /// What to leave out from now on when the user says these changes do
    /// not matter: the areas above, each widened to the whole of what
    /// changed there (a clock, not the one digit that differed this time).
    #[serde(default)]
    pub ignorable: Vec<[u32; 4]>,
    /// The accepted picture has another size: nothing to compare.
    #[serde(default)]
    pub resized: bool,
}

impl Visual {
    pub fn differs(&self) -> bool {
        self.resized || self.pixels > TOLERANCE_PIXELS
    }
}

/// `<data dir>/test-baselines/<workspace>/<test>/<variant>`.
pub fn baseline_dir(project_id: i64, test_slug: &str, variant_slug: &str) -> PathBuf {
    ph_store::paths::data_dir()
        .join("test-baselines")
        .join(project_id.to_string())
        .join(test_slug)
        .join(variant_slug)
}

/// The accepted picture of look check `id`.
pub fn baseline(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.png"))
}

/// Compares the screen `shot` with the accepted picture `accepted`, leaving
/// out `ignore` (x, y, width, height); writes `diff` (beside the shot) when
/// it looks different. `None` without an accepted picture.
pub fn check(accepted: &Path, shot: &Path, ignore: &[[u32; 4]], diff: &Path) -> Option<Visual> {
    let before = Frame::decode(&std::fs::read(accepted).ok()?).ok()?;
    let after = Frame::decode(&std::fs::read(shot).ok()?).ok()?;
    let mut visual = compare(&before, &after, ignore);
    if !visual.differs() {
        return Some(visual);
    }
    let name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    if !visual.resized
        && let Ok(png) = highlight(&after, &before, ignore).encode_png()
        && std::fs::write(diff, png).is_ok()
    {
        visual.diff = name(diff);
    }
    let kept = diff.with_file_name(
        name(diff)
            .unwrap_or_default()
            .replace("-diff.png", "-accepted.png"),
    );
    if kept != diff && std::fs::copy(accepted, &kept).is_ok() {
        visual.accepted = name(&kept);
    }
    Some(visual)
}

fn differs_at(before: &[u8], after: &[u8]) -> bool {
    before
        .iter()
        .zip(after)
        .take(3)
        .any(|(a, b)| a.abs_diff(*b) > CHANNEL_TOLERANCE)
}

fn ignored(ignore: &[[u32; 4]], x: u32, y: u32) -> bool {
    ignore.iter().any(|[left, top, width, height]| {
        x >= *left && y >= *top && x < left + width && y < top + height
    })
}

/// Counts what changed outside `ignore`, and where.
pub fn compare(before: &Frame, after: &Frame, ignore: &[[u32; 4]]) -> Visual {
    if before.width != after.width || before.height != after.height {
        return Visual {
            changed: 1.0,
            pixels: after.width * after.height,
            diff: None,
            accepted: None,
            areas: Vec::new(),
            ignorable: Vec::new(),
            resized: true,
        };
    }
    let (width, height) = (after.width, after.height);
    let (old, new) = (before.bgra(), after.bgra());
    let (columns, rows) = (width.div_ceil(CELL), height.div_ceil(CELL));
    let mut cells = vec![false; (columns * rows) as usize];
    let mut pixels = 0u32;
    for y in 0..height {
        for x in 0..width {
            let at = ((y * width + x) * 4) as usize;
            if differs_at(&old[at..at + 4], &new[at..at + 4]) && !ignored(ignore, x, y) {
                pixels += 1;
                cells[((y / CELL) * columns + x / CELL) as usize] = true;
            }
        }
    }
    Visual {
        changed: pixels as f32 / (width * height).max(1) as f32,
        pixels,
        diff: None,
        accepted: None,
        areas: areas(&mut cells, columns, rows, width, height),
        ignorable: Vec::new(),
        resized: false,
    }
}

/// The areas changed cells make: each group of cells that touch (also at a
/// corner), as the rectangle around it, a little wider than the change so
/// that a digit one pixel wider next time is still inside.
fn areas(cells: &mut [bool], columns: u32, rows: u32, width: u32, height: u32) -> Vec<[u32; 4]> {
    let mut found: Vec<[u32; 4]> = Vec::new();
    for start in 0..cells.len() {
        if !cells[start] {
            continue;
        }
        cells[start] = false;
        let (mut left, mut top) = (start as u32 % columns, start as u32 / columns);
        let (mut right, mut bottom) = (left, top);
        let mut todo = vec![(left, top)];
        while let Some((x, y)) = todo.pop() {
            (left, top) = (left.min(x), top.min(y));
            (right, bottom) = (right.max(x), bottom.max(y));
            for (nx, ny) in [
                (x.wrapping_sub(1), y.wrapping_sub(1)),
                (x, y.wrapping_sub(1)),
                (x + 1, y.wrapping_sub(1)),
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x.wrapping_sub(1), y + 1),
                (x, y + 1),
                (x + 1, y + 1),
            ] {
                if nx < columns
                    && ny < rows
                    && std::mem::take(&mut cells[(ny * columns + nx) as usize])
                {
                    todo.push((nx, ny));
                }
            }
        }
        let x = (left * CELL).saturating_sub(CELL / 2);
        let y = (top * CELL).saturating_sub(CELL / 2);
        let area = [
            x,
            y,
            ((right + 1) * CELL + CELL / 2).min(width) - x,
            ((bottom + 1) * CELL + CELL / 2).min(height) - y,
        ];
        if found.len() < MAX_AREAS {
            found.push(area);
        } else if let Some(last) = found.last_mut() {
            *last = union(*last, area);
        }
    }
    found
}

/// `area` with room around it, within a picture of `width` × `height`: a
/// change is wider next time (another digit of a clock, a longer word).
pub fn widened(area: [u32; 4], by: (u32, u32), (width, height): (u32, u32)) -> [u32; 4] {
    let (x, y) = (area[0].saturating_sub(by.0), area[1].saturating_sub(by.1));
    let right = (area[0] + area[2] + by.0).min(width);
    let bottom = (area[1] + area[3] + by.1).min(height);
    [x, y, right.saturating_sub(x), bottom.saturating_sub(y)]
}

/// The rectangle around both `a` and `b`.
pub fn union(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let (x, y) = (a[0].min(b[0]), a[1].min(b[1]));
    let right = (a[0] + a[2]).max(b[0] + b[2]);
    let bottom = (a[1] + a[3]).max(b[1] + b[3]);
    [x, y, right - x, bottom - y]
}

/// `after`, dimmed, with what changed since `before` in red (and what a
/// check ignores left as it is).
fn highlight(after: &Frame, before: &Frame, ignore: &[[u32; 4]]) -> Frame {
    let mut bgra = Vec::with_capacity(after.bgra().len());
    let (now, then) = (
        after.bgra().as_chunks::<4>().0,
        before.bgra().as_chunks::<4>().0,
    );
    for (at, (new, old)) in now.iter().zip(then).enumerate() {
        let (x, y) = (at as u32 % after.width, at as u32 / after.width);
        if differs_at(old, new) && !ignored(ignore, x, y) {
            bgra.extend_from_slice(&[50, 40, 235, 255]);
        } else {
            // Towards white, so the red stands out on any page.
            bgra.extend(new[..3].iter().map(|channel| 160 + channel / 3));
            bgra.push(255);
        }
    }
    Frame::from_bgra(after.width, after.height, bgra)
}

/// Makes `shot` the accepted picture of look check `id` in `dir`.
pub fn accept(shot: &Path, dir: &Path, id: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::copy(shot, baseline(dir, id)).with_context(|| format!("saving the look of {id}"))?;
    Ok(())
}

/// Deletes the pictures in `dir` that none of `ids` (the test's look
/// checks) names: those of checks that were removed, and the one per step
/// kept before looks were checks.
pub fn prune(dir: &Path, ids: &[&str]) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let kept = path
            .file_stem()
            .is_some_and(|stem| ids.iter().any(|id| stem == *id));
        if !kept && path.extension().is_some_and(|ext| ext == "png") {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey screen with a block of another shade at each of `blocks`.
    fn screen(blocks: &[(u32, u32, u32, u32)]) -> Frame {
        let (width, height) = (200u32, 120u32);
        let mut rgba = vec![200u8; (width * height * 4) as usize];
        for (left, top, w, h) in blocks {
            for y in *top..top + h {
                for x in *left..left + w {
                    let at = ((y * width + x) * 4) as usize;
                    rgba[at..at + 3].copy_from_slice(&[20, 20, 20]);
                }
            }
        }
        Frame::from_rgba(width, height, rgba)
    }

    #[test]
    fn a_small_change_counts_and_says_where() {
        let before = screen(&[]);
        // A word's worth of pixels: 0.4% of the screen.
        let after = screen(&[(40, 30, 12, 8)]);
        let visual = compare(&before, &after, &[]);
        assert_eq!(visual.pixels, 96);
        assert!(visual.changed < 0.01 && visual.differs());
        assert_eq!(visual.areas.len(), 1);
        let [x, y, w, h] = visual.areas[0];
        assert!(
            x <= 40 && y <= 30 && x + w >= 52 && y + h >= 38,
            "{:?}",
            visual.areas
        );
        // The same screen, and a few specks, do not.
        assert!(!compare(&before, &before, &[]).differs());
        assert!(!compare(&before, &screen(&[(5, 5, 3, 3)]), &[]).differs());
    }

    #[test]
    fn what_a_check_ignores_may_change() {
        let before = screen(&[]);
        let after = screen(&[(150, 10, 30, 10), (20, 80, 10, 10)]);
        let all = compare(&before, &after, &[]);
        assert_eq!(all.areas.len(), 2, "two changes apart are two areas");
        // Ignoring the areas a run reported leaves nothing.
        assert!(!compare(&before, &after, &all.areas).differs());
        // Ignoring one leaves the other.
        let left = compare(&before, &after, &[[140, 0, 60, 30]]);
        assert!(left.differs());
        assert_eq!(left.pixels, 100);
        // An area is given room, within the picture.
        assert_eq!(
            widened([150, 10, 30, 10], (40, 8), (200, 120)),
            [110, 2, 90, 26]
        );
        assert_eq!(widened([0, 0, 10, 10], (40, 8), (200, 120)), [0, 0, 50, 18]);
        assert_eq!(union([10, 10, 10, 10], [30, 5, 10, 10]), [10, 5, 30, 15]);
    }

    #[test]
    fn a_screen_of_another_size_looks_different() {
        let wide = Frame::from_rgba(10, 4, vec![0; 160]);
        let tall = Frame::from_rgba(4, 10, vec![0; 160]);
        let visual = compare(&wide, &tall, &[]);
        assert!(visual.resized && visual.differs());
    }

    #[test]
    fn checks_compare_with_the_accepted_picture() {
        let root = std::env::temp_dir().join(format!("ph-visual-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (run, accepted) = (root.join("run"), root.join("accepted"));
        std::fs::create_dir_all(&run).unwrap();
        let shot = run.join("003.png");
        std::fs::write(&shot, screen(&[]).encode_png().unwrap()).unwrap();
        let diff = run.join("003-diff.png");
        assert!(
            check(&baseline(&accepted, "look-1"), &shot, &[], &diff).is_none(),
            "nothing accepted yet"
        );

        accept(&shot, &accepted, "look-1").unwrap();
        // One per step, from before looks were checks, and a removed check's.
        std::fs::write(accepted.join("000.png"), b"old").unwrap();
        prune(&accepted, &["look-1"]);
        assert!(!accepted.join("000.png").exists());
        let same = check(&baseline(&accepted, "look-1"), &shot, &[], &diff).unwrap();
        assert!(!same.differs() && same.diff.is_none());

        std::fs::write(&shot, screen(&[(40, 30, 12, 8)]).encode_png().unwrap()).unwrap();
        let changed = check(&baseline(&accepted, "look-1"), &shot, &[], &diff).unwrap();
        assert!(changed.differs());
        assert_eq!(changed.diff.as_deref(), Some("003-diff.png"));
        assert!(diff.is_file());
        assert_eq!(changed.accepted.as_deref(), Some("003-accepted.png"));
        assert!(run.join("003-accepted.png").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
