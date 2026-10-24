//! Replaying a test without AI: each step is the same call the agent made,
//! checks read the screen (waiting a little for it to settle), and every
//! step leaves a screenshot, compared with the accepted baseline when there
//! is one ([`crate::visual`]). The first failure stops the run.
//!
//! A run can be on one [`Variant`] (a screen size for web tests, a device
//! for Android ones); [`run_matrix`] runs several, one after another.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use gpui_kit::{App, AsyncApp, Task};
use ph_control::ControlServer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{self, Step, Target, TestCase};
use crate::visual::{self, Visual};

/// How long a check waits for the screen to show what it expects.
const WAIT: Duration = Duration::from_secs(8);
/// A pause after each action, for the screen to react.
const SETTLE: Duration = Duration::from_millis(400);
/// The pause before an action that failed is tried once more.
const RETRY_AFTER: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    pub step: Step,
    /// `passed`, `failed` or `skipped` (after a failure).
    pub outcome: String,
    #[serde(default)]
    pub error: Option<String>,
    pub millis: u64,
    /// Relative to the report's folder.
    #[serde(default)]
    pub screenshot: Option<String>,
    /// The screenshot against the baseline, when there is one.
    #[serde(default)]
    pub visual: Option<Visual>,
}

/// Where a run happened: a screen size the browser shows the page at, or an
/// Android device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Variant {
    Screen {
        name: String,
        width: u32,
        height: u32,
        mobile: bool,
    },
    Device {
        serial: String,
        name: String,
    },
}

impl Variant {
    pub fn label(&self) -> String {
        match self {
            Variant::Screen {
                name,
                width,
                height,
                ..
            } => format!("{name} {width}×{height}"),
            Variant::Device { name, .. } => name.replace('_', " "),
        }
    }

    /// Its baseline folder's name.
    pub fn slug(variant: Option<&Variant>) -> String {
        match variant {
            Some(Variant::Screen { name, .. }) => model::slug(name),
            Some(Variant::Device { serial, .. }) => model::slug(serial),
            None => "default".into(),
        }
    }

    fn serial(&self) -> Option<&str> {
        match self {
            Variant::Device { serial, .. } => Some(serial),
            Variant::Screen { .. } => None,
        }
    }
}

/// The sizes a web test runs at in a matrix run: a laptop, a tablet and a
/// phone, the last two laid out as on mobile.
pub fn screens() -> Vec<Variant> {
    [
        ("Desktop", 1280, 800, false),
        ("Tablet", 820, 1180, true),
        ("Phone", 390, 844, true),
    ]
    .into_iter()
    .map(|(name, width, height, mobile)| Variant::Screen {
        name: name.into(),
        width,
        height,
        mobile,
    })
    .collect()
}

/// The ready Android devices, for a matrix run.
pub fn devices(cx: &mut App) -> Task<Result<Vec<Variant>>> {
    cx.spawn(async move |cx| {
        let list = call("device.list", json!({}), cx).await?;
        Ok(list["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|device| device["state"] == "device")
            .filter_map(|device| {
                Some(Variant::Device {
                    serial: device["serial"].as_str()?.to_string(),
                    name: device["name"].as_str().unwrap_or("Device").to_string(),
                })
            })
            .collect())
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub test: String,
    pub started_at: i64,
    /// The Android device it ran on.
    #[serde(default)]
    pub device: Option<String>,
    /// The screen size or device of a matrix run.
    #[serde(default)]
    pub variant: Option<Variant>,
    pub steps: Vec<StepResult>,
}

impl Report {
    /// Every step worked and none looks different from the baseline.
    pub fn passed(&self) -> bool {
        self.steps.iter().all(|step| step.outcome == "passed") && self.looks_different() == 0
    }

    /// Every step worked (however they look).
    pub fn steps_passed(&self) -> bool {
        self.steps.iter().all(|step| step.outcome == "passed")
    }

    /// Steps whose screen differs from the baseline.
    pub fn looks_different(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.visual.as_ref().is_some_and(Visual::differs))
            .count()
    }

    /// Whether any step had a baseline to compare with.
    pub fn compared(&self) -> bool {
        self.steps.iter().any(|step| step.visual.is_some())
    }

    /// The step screenshots' file names.
    pub fn shots(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter_map(|step| step.screenshot.clone())
            .collect()
    }

    /// The step that failed, with its index.
    pub fn failure(&self) -> Option<(usize, &StepResult)> {
        self.steps
            .iter()
            .enumerate()
            .find(|(_, step)| step.outcome == "failed")
    }
}

/// The steps to replay: the test's own, after opening its target when the
/// recording did not start there.
pub fn plan(test: &TestCase) -> Vec<Step> {
    let mut steps = test.steps.clone();
    match &test.target {
        Target::Web { url } if !matches!(steps.first(), Some(Step::Open { .. })) => {
            steps.insert(0, Step::Open { url: url.clone() });
        }
        Target::Android { package } if !matches!(steps.first(), Some(Step::Launch { .. })) => {
            steps.insert(
                0,
                Step::Launch {
                    package: package.clone(),
                },
            );
        }
        _ => {}
    }
    steps
}

/// Whether the screen (`text`, `url`) passes check `step`.
pub fn holds(step: &Step, text: &str, url: &str) -> bool {
    let text = text.to_lowercase();
    match step {
        Step::ExpectText { text: wanted } => text.contains(&wanted.to_lowercase()),
        Step::ExpectNoText { text: unwanted } => !text.contains(&unwanted.to_lowercase()),
        Step::ExpectUrl { contains } => url.contains(contains.as_str()),
        _ => true,
    }
}

async fn call(method: &'static str, params: Value, cx: &mut AsyncApp) -> Result<Value> {
    cx.update(|cx| ControlServer::call(method, params, cx))
        .await
}

/// Waits until check `step` holds on the browser (or the device's screen).
pub fn check(
    step: Step,
    project_id: i64,
    android: bool,
    serial: Option<String>,
    cx: &mut App,
) -> Task<Result<()>> {
    cx.spawn(async move |cx| check_async(&step, project_id, android, serial.as_deref(), cx).await)
}

async fn check_async(
    step: &Step,
    project_id: i64,
    android: bool,
    serial: Option<&str>,
    cx: &mut AsyncApp,
) -> Result<()> {
    let deadline = Instant::now() + WAIT;
    loop {
        let screen = if android {
            call("device.ui", json!({ "serial": serial }), cx).await?
        } else {
            call("browser.snapshot", json!({ "project_id": project_id }), cx).await?
        };
        let text = screen["text"].as_str().unwrap_or_default();
        let url = screen["url"].as_str().unwrap_or_default();
        if holds(step, text, url) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let seen: String = text.chars().take(400).collect();
            bail!(
                "{} did not hold after {}s. The screen shows: {seen}",
                step.describe(),
                WAIT.as_secs()
            );
        }
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
    }
}

/// Saves the screen as `<dir>/<n>.png`; `None` if there is none to take.
async fn screenshot(
    project_id: i64,
    android: bool,
    serial: Option<&str>,
    dir: &Path,
    n: usize,
    cx: &mut AsyncApp,
) -> Option<String> {
    let shot = if android {
        call("device.screenshot", json!({ "serial": serial }), cx).await
    } else {
        call(
            "browser.screenshot",
            json!({ "project_id": project_id }),
            cx,
        )
        .await
    }
    .ok()?;
    let data = shot["images"][0]["data"].as_str()?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let name = format!("{n:03}.png");
    std::fs::write(dir.join(&name), png).ok()?;
    Some(name)
}

/// Runs `test` (on `variant`, else the device or pane in view) and writes
/// its report (and screenshots) into `dir`.
pub fn run(
    test: TestCase,
    project_id: i64,
    variant: Option<Variant>,
    dir: PathBuf,
    cx: &mut App,
) -> Task<Result<Report>> {
    cx.spawn(async move |cx| run_async(&test, project_id, variant, &dir, cx).await)
}

/// Runs `test` on each of `variants` in turn, each into its own folder in
/// `dir`. A variant that cannot run at all stops the matrix.
pub fn run_matrix(
    test: TestCase,
    project_id: i64,
    variants: Vec<Variant>,
    dir: PathBuf,
    cx: &mut App,
) -> Task<Result<Vec<Report>>> {
    cx.spawn(async move |cx| {
        let mut reports = Vec::new();
        for (n, variant) in variants.into_iter().enumerate() {
            let folder = dir.join(format!("{n:02}-{}", Variant::slug(Some(&variant))));
            reports.push(run_async(&test, project_id, Some(variant), &folder, cx).await?);
        }
        Ok(reports)
    })
}

async fn run_async(
    test: &TestCase,
    project_id: i64,
    variant: Option<Variant>,
    dir: &Path,
    cx: &mut AsyncApp,
) -> Result<Report> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let android = test.is_android();
    let serial = variant
        .as_ref()
        .and_then(Variant::serial)
        .map(str::to_string);
    let baseline = visual::baseline_dir(
        project_id,
        &model::slug(&test.name),
        &Variant::slug(variant.as_ref()),
    );
    if !android {
        // A cached page would hide what the app serves now.
        call(
            "browser.clear_cache",
            json!({ "project_id": project_id }),
            cx,
        )
        .await?;
    }
    if let Some(Variant::Screen {
        width,
        height,
        mobile,
        ..
    }) = &variant
    {
        call(
            "browser.emulate",
            json!({ "project_id": project_id, "width": width, "height": height, "mobile": mobile }),
            cx,
        )
        .await?;
    }
    let mut report = Report {
        test: test.name.clone(),
        started_at: now_millis(),
        device: serial.clone(),
        variant: variant.clone(),
        steps: Vec::new(),
    };
    let mut failed = false;
    for (n, step) in plan(test).into_iter().enumerate() {
        if failed {
            report.steps.push(StepResult {
                step,
                outcome: "skipped".into(),
                error: None,
                millis: 0,
                screenshot: None,
                visual: None,
            });
            continue;
        }
        let started = Instant::now();
        let result = match step
            .call(project_id, serial.as_deref())
            .map(|(method, params)| with_secrets(params).map(|params| (method, params)))
        {
            Some(Err(err)) => Err(err),
            Some(Ok((method, params))) => {
                // One retry: a page still loading (or a browser just
                // started) can turn away the first try.
                let mut done = call(method, params.clone(), cx).await.map(|_| ());
                if done.is_err() {
                    cx.background_executor().timer(RETRY_AFTER).await;
                    done = call(method, params, cx).await.map(|_| ());
                }
                cx.background_executor().timer(SETTLE).await;
                done
            }
            None => check_async(&step, project_id, android, serial.as_deref(), cx).await,
        };
        let millis = started.elapsed().as_millis() as u64;
        let shot = screenshot(project_id, android, serial.as_deref(), dir, n, cx).await;
        let visual = shot
            .as_deref()
            .and_then(|name| visual::check(&baseline, dir, name));
        failed = result.is_err();
        report.steps.push(StepResult {
            step,
            outcome: if failed { "failed" } else { "passed" }.into(),
            error: result.err().map(|err| format!("{err:#}")),
            millis,
            screenshot: shot,
            visual,
        });
    }
    if matches!(variant, Some(Variant::Screen { .. })) {
        // The page follows the pane again.
        let _ = call("browser.emulate", json!({ "project_id": project_id }), cx).await;
    }
    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(report)
}

/// Makes the run in `dir` (its `report`) the baseline for its variant.
pub fn accept_look(project_id: i64, dir: &Path, report: &Report) -> Result<()> {
    let baseline = visual::baseline_dir(
        project_id,
        &model::slug(&report.test),
        &Variant::slug(report.variant.as_ref()),
    );
    visual::accept(dir, &report.shots(), &baseline)
}

/// Whether the run's variant has a baseline yet.
pub fn has_baseline(project_id: i64, report: &Report) -> bool {
    visual::has_baseline(&visual::baseline_dir(
        project_id,
        &model::slug(&report.test),
        &Variant::slug(report.variant.as_ref()),
    ))
}

/// `<data dir>/test-runs/<project>/<test>/<time>`.
pub fn report_dir(project_id: i64, test_slug: &str) -> PathBuf {
    ph_store::paths::data_dir()
        .join("test-runs")
        .join(project_id.to_string())
        .join(test_slug)
        .join(now_millis().to_string())
}

/// The newest run of a test: one report, or one per variant of a matrix
/// run (in run order). Empty if it never ran.
pub fn last_reports(project_id: i64, test_slug: &str) -> Vec<(PathBuf, Report)> {
    let folder = ph_store::paths::data_dir()
        .join("test-runs")
        .join(project_id.to_string())
        .join(test_slug);
    let load = |dir: &Path| -> Option<Report> {
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).ok()?).ok()
    };
    let subfolders = |dir: &Path| -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        dirs
    };
    // The newest run folder that holds a report, directly or per variant.
    for run in subfolders(&folder).into_iter().rev() {
        if let Some(report) = load(&run) {
            return vec![(run, report)];
        }
        let reports: Vec<(PathBuf, Report)> = subfolders(&run)
            .into_iter()
            .filter_map(|dir| Some((dir.clone(), load(&dir)?)))
            .collect();
        if !reports.is_empty() {
            return reports;
        }
    }
    Vec::new()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// `params` with each `{{env:NAME}}` in its typed text replaced by that
/// environment variable (a password kept out of the test file).
fn with_secrets(mut params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let Some(text) = params["text"].as_str() else {
        return Ok(params);
    };
    let mut filled = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{env:") {
        let Some(end) = rest[start..].find("}}") else {
            break;
        };
        let name = &rest[start + 6..start + end];
        // As the login shell has it, like every tool PilotHouse starts.
        let value = ph_env::var(name).ok_or_else(|| {
            anyhow::anyhow!("set {name} in your shell's environment: this step types it")
        })?;
        filled.push_str(&rest[..start]);
        filled.push_str(value);
        rest = &rest[start + end + 2..];
    }
    filled.push_str(rest);
    params["text"] = serde_json::Value::String(filled);
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_secrets_come_from_the_environment() {
        let plain = serde_json::json!({ "text": "ada" });
        assert_eq!(with_secrets(plain.clone()).unwrap(), plain);
        let secret = serde_json::json!({ "text": "{{env:PH_SURELY_UNSET_VARIABLE}}" });
        let err = with_secrets(secret).unwrap_err().to_string();
        assert!(err.contains("PH_SURELY_UNSET_VARIABLE"), "{err}");
        let path = serde_json::json!({ "text": "at {{env:PATH}}!" });
        let filled = with_secrets(path).unwrap();
        assert_eq!(
            filled["text"],
            format!("at {}!", ph_env::var("PATH").unwrap())
        );
    }

    #[test]
    fn checks_read_the_screen_without_case() {
        assert!(holds(
            &Step::ExpectText {
                text: "Welcome".into()
            },
            "welcome back, ada",
            ""
        ));
        assert!(!holds(
            &Step::ExpectText {
                text: "Error".into()
            },
            "all good",
            ""
        ));
        assert!(holds(
            &Step::ExpectNoText {
                text: "Error".into()
            },
            "all good",
            ""
        ));
        assert!(holds(
            &Step::ExpectUrl {
                contains: "/home".into()
            },
            "",
            "http://x/home?a=1"
        ));
    }

    #[test]
    fn replay_starts_at_the_target() {
        let mut test = TestCase {
            name: "t".into(),
            description: String::new(),
            target: Target::Web {
                url: "http://app".into(),
            },
            steps: vec![Step::Click {
                selector: "#go".into(),
            }],
        };
        assert_eq!(
            plan(&test)[0],
            Step::Open {
                url: "http://app".into()
            }
        );
        test.steps.insert(
            0,
            Step::Open {
                url: "http://app/login".into(),
            },
        );
        assert_eq!(
            plan(&test).len(),
            2,
            "a recording that opened a page keeps it"
        );
        test.target = Target::Android {
            package: "com.acme".into(),
        };
        assert_eq!(
            plan(&test)[0],
            Step::Launch {
                package: "com.acme".into()
            }
        );
    }

    #[test]
    fn reports_say_what_failed() {
        let result = |outcome: &str| StepResult {
            step: Step::Press {
                key: "Enter".into(),
            },
            outcome: outcome.into(),
            error: None,
            millis: 1,
            screenshot: None,
            visual: None,
        };
        let report = Report {
            test: "t".into(),
            started_at: 0,
            device: None,
            variant: None,
            steps: vec![result("passed"), result("failed"), result("skipped")],
        };
        assert!(!report.passed());
        assert_eq!(report.failure().map(|(ix, _)| ix), Some(1));
    }
}
