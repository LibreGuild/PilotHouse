//! Visual baselines: how each step's screen looked in a run the user
//! accepted, and how much a new run differs from it.
//!
//! Baselines are kept per machine in the data directory
//! (`test-baselines/<workspace>/<test>/<variant>/NNN.png`), not in the
//! project: fonts and rendering differ between machines, so another
//! person's baseline would differ everywhere. Each screen size or device has
//! its own.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use ph_media::Frame;
use serde::{Deserialize, Serialize};

/// Share of pixels that may change before a step "looks different" (a
/// blinking caret or a clock stays under it).
pub const TOLERANCE: f32 = 0.01;

/// How a step's screen compares with its baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visual {
    /// Share of pixels that changed, 0.0–1.0.
    pub changed: f32,
    /// The changes tinted red, relative to the report's folder; only when it
    /// looks different.
    #[serde(default)]
    pub diff: Option<String>,
}

impl Visual {
    pub fn differs(&self) -> bool {
        self.changed > TOLERANCE
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

pub fn has_baseline(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
}

/// Compares the step screenshot `name` in `report_dir` with the baseline
/// of the same name; writes `<name>-diff.png` when it looks different.
/// `None` without a baseline for it.
pub fn check(baseline: &Path, report_dir: &Path, name: &str) -> Option<Visual> {
    let before = Frame::decode(&std::fs::read(baseline.join(name)).ok()?).ok()?;
    let after = Frame::decode(&std::fs::read(report_dir.join(name)).ok()?).ok()?;
    let comparison = ph_media::compare(&before, &after);
    let mut visual = Visual {
        changed: comparison.changed,
        diff: None,
    };
    if visual.differs() {
        let diff = name.replace(".png", "-diff.png");
        if let Ok(png) = comparison.highlight.encode_png()
            && std::fs::write(report_dir.join(&diff), png).is_ok()
        {
            visual.diff = Some(diff);
        }
    }
    Some(visual)
}

/// Makes the step screenshots of the run in `report_dir` the new baseline
/// (`shots`: their file names). The old baseline is replaced.
pub fn accept(report_dir: &Path, shots: &[String], baseline: &Path) -> Result<()> {
    if baseline.exists() {
        std::fs::remove_dir_all(baseline)
            .with_context(|| format!("replacing {}", baseline.display()))?;
    }
    std::fs::create_dir_all(baseline)?;
    for shot in shots {
        std::fs::copy(report_dir.join(shot), baseline.join(shot))
            .with_context(|| format!("saving {shot}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(path: &Path, value: u8) {
        let frame = Frame::from_rgba(20, 20, vec![value; 20 * 20 * 4]);
        std::fs::write(path, frame.encode_png().unwrap()).unwrap();
    }

    #[test]
    fn compares_steps_with_the_accepted_run() {
        let root = std::env::temp_dir().join(format!("ph-visual-{}", std::process::id()));
        let (run, baseline) = (root.join("run"), root.join("baseline"));
        std::fs::create_dir_all(&run).unwrap();
        png(&run.join("000.png"), 10);
        png(&run.join("001.png"), 10);
        assert!(
            check(&baseline, &run, "000.png").is_none(),
            "no baseline yet"
        );

        accept(&run, &["000.png".into(), "001.png".into()], &baseline).unwrap();
        assert!(has_baseline(&baseline));
        assert!(!check(&baseline, &run, "000.png").unwrap().differs());

        png(&run.join("001.png"), 200);
        let changed = check(&baseline, &run, "001.png").unwrap();
        assert!(changed.differs() && changed.changed > 0.99);
        assert_eq!(changed.diff.as_deref(), Some("001-diff.png"));
        assert!(run.join("001-diff.png").is_file());
    }
}
