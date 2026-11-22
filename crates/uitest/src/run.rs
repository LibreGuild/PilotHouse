//! Replaying a test without AI: each step is the call the agent made, on the
//! element it made it on (found again by what it is and says, and waited
//! for), checks read the screen (waiting a little for it to settle), and
//! every step leaves a screenshot. The first failure stops the run.
//!
//! A run starts from the same place every time: what the test's site keeps
//! (sign-ins, storage) is erased first, an app is stopped (and its data
//! erased when the test asks), so a test that signs in does not find
//! itself signed in the second time.
//!
//! A run can be on one [`Variant`] (a screen size for web tests, a device
//! for Android ones); [`run_matrix`] runs several, one after another.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use gpui_kit::{App, AsyncApp, Task};
use ph_control::ControlServer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{self, Step, Target, TestCase};
use crate::secrets::Missing;
use crate::visual::{self, Visual};

/// How long a check waits for the screen to show what it expects.
const WAIT: Duration = Duration::from_secs(8);
/// A pause after each action on a phone, for the screen to react (the
/// browser's own calls wait for the page).
const SETTLE: Duration = Duration::from_millis(400);
/// The pause before an action that failed is tried once more.
const RETRY_AFTER: Duration = Duration::from_millis(800);
/// How long the screen is left to come to rest before its look is taken.
const LOOK_SETTLE: Duration = Duration::from_millis(600);
/// How long an address on this computer has to answer.
const ANSWER_WITHIN: Duration = Duration::from_millis(1500);
/// Runs kept per test (each a screenshot per step); older ones are deleted.
const KEEP_RUNS: usize = 20;

/// What would get a failed step past its failure, when that is not a change
/// to the test: the Tests tab offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Hint {
    /// Nothing answers at the page's address: the app is not running.
    StartApp { address: String },
    /// A saved password is not on this computer.
    Secret { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    pub step: Step,
    /// `passed`, `failed` or `skipped` (after a failure).
    pub outcome: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub hint: Option<Hint>,
    pub millis: u64,
    /// Relative to the report's folder.
    #[serde(default)]
    pub screenshot: Option<String>,
    /// A look check's screen against its accepted picture, when it has one.
    #[serde(default)]
    pub visual: Option<Visual>,
}

/// A step of a run that is going on, as it ends: for whoever shows it.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Which variant of a matrix run (0 for a single run).
    pub variant: usize,
    pub index: usize,
    pub result: StepResult,
    /// The report's folder (its screenshots are in it).
    pub dir: PathBuf,
}

/// Told each step of a run as it ends.
pub type OnProgress = Rc<dyn Fn(Progress, &mut App)>;

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

/// The Android device in view (the one a call that names none acts on),
/// for a run on it alone: its accepted looks are its own, not those of
/// whichever phone was in view before.
pub fn device_in_view(cx: &mut App) -> Task<Result<Vec<Variant>>> {
    let all = devices(cx);
    cx.spawn(async move |cx| {
        let front = call("device.front", json!({}), cx).await?;
        let serial = front["serial"].as_str().unwrap_or_default().to_string();
        Ok(all
            .await?
            .into_iter()
            .filter(|device| device.serial() == Some(serial.as_str()))
            .collect())
    })
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
    /// The size of its screenshots, in pixels: what shows them gives them
    /// the room their shape needs.
    #[serde(default)]
    pub picture: Option<(u32, u32)>,
    pub steps: Vec<StepResult>,
}

impl Report {
    /// Every step worked and none looks different from what was accepted.
    pub fn passed(&self) -> bool {
        self.steps_passed() && self.looks_different() == 0
    }

    /// Every step worked (however they look).
    pub fn steps_passed(&self) -> bool {
        self.steps.iter().all(|step| step.outcome == "passed")
    }

    /// Look checks whose screen differs from the accepted picture.
    pub fn looks_different(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.visual.as_ref().is_some_and(Visual::differs))
            .count()
    }

    /// Look checks that ran and have no accepted picture yet.
    pub fn looks_new(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| {
                matches!(step.step, Step::ExpectLook { .. })
                    && step.outcome == "passed"
                    && step.visual.is_none()
            })
            .count()
    }

    /// The step that failed, with its index.
    pub fn failure(&self) -> Option<(usize, &StepResult)> {
        self.steps
            .iter()
            .enumerate()
            .find(|(_, step)| step.outcome == "failed")
    }

    /// How long its steps took.
    pub fn millis(&self) -> u64 {
        self.steps.iter().map(|step| step.millis).sum()
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

/// Words as a reader sees them: any run of spaces and line ends is one
/// space, and case does not matter. A page breaks a sentence over two
/// elements, or two lines, without it reading any differently.
fn plain(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether the screen (`text`, `url`) passes check `step`.
pub fn holds(step: &Step, text: &str, url: &str) -> bool {
    match step {
        Step::ExpectText { text: wanted } => plain(text).contains(&plain(wanted)),
        Step::ExpectNoText { text: unwanted } => !plain(text).contains(&plain(unwanted)),
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
        // All the page says (a long page too), or the words on the phone's
        // screen (not its elements' ids, which are not words anyone sees).
        let screen = if android {
            call("device.ui", json!({ "serial": serial }), cx).await
        } else {
            call(
                "browser.text",
                json!({ "project_id": project_id, "quiet": true }),
                cx,
            )
            .await
        };
        // A page on its way to another, or a screen that does not hold
        // still, has nothing to read yet: looked at again.
        let (text, url) = match &screen {
            Ok(screen) => (
                screen["texts"]
                    .as_str()
                    .or_else(|| screen["text"].as_str())
                    .unwrap_or_default(),
                screen["url"].as_str().unwrap_or_default(),
            ),
            Err(_) => ("", ""),
        };
        if screen.is_ok() && holds(step, text, url) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            if let Err(err) = screen {
                return Err(err.context("the screen could not be read"));
            }
            // What it shows instead, on one line: its start says enough,
            // and the step's screenshot shows the rest.
            let seen: String = text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(240)
                .collect();
            let waited = WAIT.as_secs();
            match step {
                Step::ExpectNoText { text } => bail!("“{text}” still shows after {waited} s."),
                Step::ExpectUrl { contains } => {
                    bail!("The address is {url}, without “{contains}”, after {waited} s.")
                }
                Step::ExpectText { text } => {
                    bail!("“{text}” did not show within {waited} s. The screen shows: {seen}")
                }
                _ => bail!("The check did not hold within {waited} s."),
            }
        }
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
    }
}

/// Stills the page for a picture of how it looks: nothing moves, and the
/// text cursor (which blinks) is not drawn. Undone by [`UNSTILL`].
const STILL: &str = r#"(() => { const style = document.createElement('style'); style.id = '__ph_still'; style.textContent = '*, *::before, *::after { animation: none !important; transition: none !important; caret-color: transparent !important; }'; document.documentElement.appendChild(style); return true; })()"#;
const UNSTILL: &str = r#"(() => { const style = document.getElementById('__ph_still'); if (style) style.remove(); return true; })()"#;

/// Saves the screen as `<dir>/<n>.png`; `None` if there is none to take.
async fn screenshot(
    project_id: i64,
    android: bool,
    serial: Option<&str>,
    dir: &Path,
    n: usize,
    cx: &mut AsyncApp,
) -> Option<String> {
    shot(project_id, android, serial, (dir, n), false, cx)
        .await
        .map(|(name, _)| name)
}

/// The width and height a PNG says it has, in its first bytes.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let number = |at: usize| Some(u32::from_be_bytes(png.get(at..at + 4)?.try_into().ok()?));
    (png.get(..8)? == b"\x89PNG\r\n\x1a\n" && png.get(12..16)? == b"IHDR")
        .then(|| Some((number(16)?, number(20)?)))
        .flatten()
        .filter(|(width, height)| *width > 0 && *height > 0)
}

/// The size of the screenshot `name` in `dir`. Reads its first bytes: call
/// it off the UI thread.
fn picture_size(dir: &Path, name: &str) -> Option<(u32, u32)> {
    use std::io::Read as _;
    let mut start = [0u8; 24];
    std::fs::File::open(dir.join(name))
        .ok()?
        .read_exact(&mut start)
        .ok()?;
    png_size(&start)
}

/// Saves the screen as `<dir>/<n>.png` and, with `status_bar`, says where a
/// phone's status bar is in it.
async fn shot(
    project_id: i64,
    android: bool,
    serial: Option<&str>,
    (dir, n): (&Path, usize),
    status_bar: bool,
    cx: &mut AsyncApp,
) -> Option<(String, Option<[u32; 4]>)> {
    let shot = if android {
        call(
            "device.screenshot",
            json!({ "serial": serial, "status_bar": status_bar }),
            cx,
        )
        .await
    } else {
        call(
            "browser.screenshot",
            json!({ "project_id": project_id, "quiet": true }),
            cx,
        )
        .await
    }
    .ok()?;
    let data = shot["images"][0]["data"].as_str()?.to_string();
    let bar: Option<[u32; 4]> = serde_json::from_value(shot["status_bar"].clone()).ok();
    let name = format!("{n:03}.png");
    let path = dir.join(&name);
    // A whole screen, decoded and written off the UI thread.
    cx.background_executor()
        .spawn(async move {
            let png = base64::engine::general_purpose::STANDARD
                .decode(data)
                .ok()?;
            std::fs::write(path, png).ok()
        })
        .await?;
    Some((name, bar))
}

/// Runs `test` (on `variant`, else the device or pane in view) and writes
/// its report (and screenshots) into `dir`.
pub fn run(
    test: TestCase,
    project_id: i64,
    variant: Option<Variant>,
    dir: PathBuf,
    progress: Option<OnProgress>,
    cx: &mut App,
) -> Task<Result<Report>> {
    cx.spawn(async move |cx| {
        let report = run_async(&test, project_id, variant, &dir, 0, progress, cx).await;
        prune(&dir, cx).await;
        report
    })
}

/// Runs `test` once, where the Tests tab's *Run* does: a page at the desktop
/// size (so its screens compare from run to run), a phone test on the
/// device in view. For an agent's `test_run` and a schedule.
pub fn run_once(
    test: TestCase,
    project_id: i64,
    dir: PathBuf,
    cx: &mut App,
) -> Task<Result<Report>> {
    if !test.is_android() {
        let variant = screens().into_iter().next();
        return run(test, project_id, variant, dir, None, cx);
    }
    let device = device_in_view(cx);
    cx.spawn(async move |cx| {
        let variant = device
            .await?
            .into_iter()
            .next()
            .context("No Android device is ready: connect a phone or start an emulator.")?;
        cx.update(|cx| run(test, project_id, Some(variant), dir, None, cx))
            .await
    })
}

/// Runs `test` on each of `variants` in turn, each into its own folder in
/// `dir`. A variant that cannot run at all stops the matrix.
pub fn run_matrix(
    test: TestCase,
    project_id: i64,
    variants: Vec<Variant>,
    dir: PathBuf,
    progress: Option<OnProgress>,
    cx: &mut App,
) -> Task<Result<Vec<Report>>> {
    cx.spawn(async move |cx| {
        let mut reports = Vec::new();
        for (n, variant) in variants.into_iter().enumerate() {
            let folder = dir.join(format!("{n:02}-{}", Variant::slug(Some(&variant))));
            let report = run_async(
                &test,
                project_id,
                Some(variant),
                &folder,
                n,
                progress.clone(),
                cx,
            )
            .await;
            match report {
                Ok(report) => reports.push(report),
                Err(err) => {
                    prune(&dir, cx).await;
                    return Err(err);
                }
            }
        }
        prune(&dir, cx).await;
        Ok(reports)
    })
}

/// Deletes the older runs of the test whose run folder is `run`, off the
/// UI thread. Only folders made by [`report_dir`].
async fn prune(run: &Path, cx: &mut AsyncApp) {
    let runs = ph_store::paths::data_dir().join("test-runs");
    let Some(test) = run.parent().filter(|test| test.starts_with(&runs)) else {
        return;
    };
    let test = test.to_path_buf();
    cx.background_executor()
        .spawn(async move { prune_runs(&test, KEEP_RUNS) })
        .await;
}

/// Deletes all but the newest `keep` run folders in `test` (their names are
/// times, so they sort by age).
fn prune_runs(test: &Path, keep: usize) {
    let mut runs: Vec<PathBuf> = std::fs::read_dir(test)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    runs.sort();
    let old = runs.len().saturating_sub(keep);
    // One that cannot go now is tried again after the next run.
    for run in &runs[..old] {
        let _ = std::fs::remove_dir_all(run);
    }
}

/// Deletes the runs, accepted looks and saved passwords of a removed
/// workspace. Blocks: call it off the UI thread.
pub fn forget_workspace(project_id: i64) {
    let data = ph_store::paths::data_dir();
    for folder in ["test-runs", "test-baselines"] {
        let _ = std::fs::remove_dir_all(data.join(folder).join(project_id.to_string()));
    }
    crate::secrets::forget_workspace(project_id);
}

/// `localhost:3000` of `http://localhost:3000/login`, when the address is
/// on this computer (the app being built); `None` for the web.
fn local_address(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    // `[::1]:3000`, `localhost:3000`, `127.0.0.1`.
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next()?,
        None => host.split(':').next()?,
    };
    let local = name == "localhost"
        || name.ends_with(".localhost")
        || name == "::1"
        || name == "0.0.0.0"
        || name.starts_with("127.");
    if !local {
        return None;
    }
    let has_port = host.rsplit(':').next().is_some_and(|port| {
        !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) && host.contains(':')
    }) && !host.ends_with(']');
    Some(if has_port {
        host.to_string()
    } else {
        format!("{host}:{port}")
    })
}

/// Whether anything listens at `address` (`host:port`). Blocks for up to
/// [`ANSWER_WITHIN`] per address the name stands for.
fn answers(address: &str) -> bool {
    use std::net::ToSocketAddrs as _;
    // `0.0.0.0` is every address of this computer: reached as itself.
    let address = address.replacen("0.0.0.0", "127.0.0.1", 1);
    address.to_socket_addrs().is_ok_and(|mut addresses| {
        addresses
            .any(|address| std::net::TcpStream::connect_timeout(&address, ANSWER_WITHIN).is_ok())
    })
}

/// How a step ended, before it is written down.
struct Ended {
    error: Option<String>,
    hint: Option<Hint>,
    visual: Option<Visual>,
    /// The screenshot, when the step took its own (a look check).
    shot: Option<String>,
}

impl Ended {
    fn ok() -> Self {
        Self {
            error: None,
            hint: None,
            visual: None,
            shot: None,
        }
    }

    fn failed(error: impl std::fmt::Display) -> Self {
        Self {
            error: Some(sentence(&error.to_string())),
            ..Self::ok()
        }
    }
}

/// An error as a sentence: said by a call in the middle of one
/// (`no button “Go” shows`), shown alone under a step.
fn sentence(error: &str) -> String {
    let mut letters = error.chars();
    let Some(first) = letters.next() else {
        return String::new();
    };
    let mut sentence: String = first.to_uppercase().chain(letters).collect();
    if !sentence.ends_with(['.', '!', '?', ':']) && !sentence.contains('\n') {
        sentence.push('.');
    }
    sentence
}

#[allow(clippy::too_many_arguments)]
async fn run_async(
    test: &TestCase,
    project_id: i64,
    variant: Option<Variant>,
    dir: &Path,
    variant_index: usize,
    progress: Option<OnProgress>,
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
    let steps = plan(test);
    // The app is not running: said once, before anything is tried.
    let unreachable = match steps.first() {
        Some(Step::Open { url }) => match local_address(url) {
            Some(address) => {
                let asked = address.clone();
                let up = cx
                    .background_executor()
                    .spawn(async move { answers(&asked) })
                    .await;
                (!up).then_some(address)
            }
            None => None,
        },
        _ => None,
    };
    if !android && unreachable.is_none() {
        if test.starts_clean() {
            // Every site the test opens starts with nothing kept.
            let urls: Vec<&String> = steps
                .iter()
                .filter_map(|step| match step {
                    Step::Open { url } => Some(url),
                    _ => None,
                })
                .collect();
            call(
                "browser.clear_sites",
                json!({ "project_id": project_id, "urls": urls, "quiet": true }),
                cx,
            )
            .await?;
        } else {
            // A cached page would hide what the app serves now.
            call(
                "browser.clear_cache",
                json!({ "project_id": project_id, "quiet": true }),
                cx,
            )
            .await?;
        }
    }
    if let Some(Variant::Screen {
        width,
        height,
        mobile,
        ..
    }) = &variant
        && unreachable.is_none()
    {
        call(
            "browser.emulate",
            json!({ "project_id": project_id, "width": width, "height": height, "mobile": mobile, "quiet": true }),
            cx,
        )
        .await?;
    }
    let mut report = Report {
        test: test.name.clone(),
        started_at: now_millis(),
        device: serial.clone(),
        variant: variant.clone(),
        picture: None,
        steps: Vec::new(),
    };
    let mut failed = false;
    let mut launched = false;
    for (n, step) in steps.into_iter().enumerate() {
        let started = Instant::now();
        let ended = if failed {
            None
        } else if let Some(address) = &unreachable {
            Some(Ended {
                error: Some(format!(
                    "Nothing answers at {address}: the app is not running. Start it, then run again."
                )),
                hint: Some(Hint::StartApp {
                    address: address.clone(),
                }),
                ..Ended::ok()
            })
        } else {
            Some(match &step {
                Step::ExpectLook { id, ignore } => {
                    look(
                        project_id,
                        android,
                        serial.as_deref(),
                        (dir, n),
                        &visual::baseline(&baseline, id),
                        ignore,
                        cx,
                    )
                    .await
                }
                _ => match step.call(project_id, serial.as_deref()) {
                    Some((method, mut params)) => {
                        // The app starts at its first screen; with nothing
                        // kept when the test asks.
                        if method == "device.launch" && !std::mem::replace(&mut launched, true) {
                            params["fresh"] = json!(true);
                            params["clear"] = json!(test.starts_clean());
                        }
                        act(method, params, project_id, android, cx).await
                    }
                    None => {
                        match check_async(&step, project_id, android, serial.as_deref(), cx).await {
                            Ok(()) => Ended::ok(),
                            Err(err) => Ended::failed(format!("{err:#}")),
                        }
                    }
                },
            })
        };
        let Some(ended) = ended else {
            report.steps.push(StepResult {
                step,
                outcome: "skipped".into(),
                error: None,
                hint: None,
                millis: 0,
                screenshot: None,
                visual: None,
            });
            continue;
        };
        let millis = started.elapsed().as_millis() as u64;
        let shot = match ended.shot {
            Some(shot) => Some(shot),
            None if unreachable.is_some() => None,
            None => screenshot(project_id, android, serial.as_deref(), dir, n, cx).await,
        };
        if report.picture.is_none()
            && let Some(name) = shot.clone()
        {
            let dir = dir.to_path_buf();
            report.picture = cx
                .background_executor()
                .spawn(async move { picture_size(&dir, &name) })
                .await;
        }
        failed = ended.error.is_some();
        let result = StepResult {
            step,
            outcome: if failed { "failed" } else { "passed" }.into(),
            error: ended.error,
            hint: ended.hint,
            millis,
            screenshot: shot,
            visual: ended.visual,
        };
        if let Some(progress) = &progress {
            let progress = progress.clone();
            let told = Progress {
                variant: variant_index,
                index: n,
                result: result.clone(),
                dir: dir.to_path_buf(),
            };
            cx.update(|cx| progress(told, cx));
        }
        report.steps.push(result);
    }
    if matches!(variant, Some(Variant::Screen { .. })) && unreachable.is_none() {
        // The page follows the pane again.
        let _ = call(
            "browser.emulate",
            json!({ "project_id": project_id, "quiet": true }),
            cx,
        )
        .await;
    }
    let json = serde_json::to_string_pretty(&report)?;
    let path = dir.join("report.json");
    cx.background_executor()
        .spawn(async move { std::fs::write(path, json) })
        .await?;
    Ok(report)
}

/// Performs an action step: `method` with `params`, what it types filled in
/// from what is kept out of the test file.
async fn act(
    method: &'static str,
    mut params: Value,
    project_id: i64,
    android: bool,
    cx: &mut AsyncApp,
) -> Ended {
    if let Some(text) = params["text"].as_str().map(str::to_string)
        && text.contains("{{")
    {
        // The credential store is read off the UI thread.
        let filled = cx
            .background_executor()
            .spawn(async move { crate::secrets::fill(&text, project_id) })
            .await;
        match filled {
            Ok(filled) => params["text"] = Value::String(filled),
            Err(missing) => {
                return Ended {
                    error: Some(missing.to_string()),
                    hint: match missing {
                        Missing::Secret(name) => Some(Hint::Secret { name }),
                        _ => None,
                    },
                    ..Ended::ok()
                };
            }
        }
    }
    // A step that names its element waits for it itself; the others get
    // one more try, for a page still loading or a browser just started.
    let waits = params.get("wait_ms").is_some();
    let mut done = call(method, params.clone(), cx).await.map(drop);
    if done.is_err() && !waits {
        cx.background_executor().timer(RETRY_AFTER).await;
        done = call(method, params, cx).await.map(drop);
    }
    if android {
        cx.background_executor().timer(SETTLE).await;
    }
    match done {
        Ok(()) => Ended::ok(),
        Err(err) => Ended::failed(format!("{err:#}")),
    }
}

/// A look check: the screen, at rest, against its accepted picture
/// (`accepted`). Without one yet it passes, and the run says so.
async fn look(
    project_id: i64,
    android: bool,
    serial: Option<&str>,
    (dir, n): (&Path, usize),
    accepted: &Path,
    ignore: &[[u32; 4]],
    cx: &mut AsyncApp,
) -> Ended {
    cx.background_executor().timer(LOOK_SETTLE).await;
    let still = |expression: &'static str| json!({ "project_id": project_id, "expression": expression, "quiet": true });
    if !android {
        let _ = call("browser.evaluate", still(STILL), cx).await;
    }
    let taken = shot(project_id, android, serial, (dir, n), android, cx).await;
    if !android {
        let _ = call("browser.evaluate", still(UNSTILL), cx).await;
    }
    let Some((name, status_bar)) = taken else {
        return Ended::failed("The screen could not be captured.");
    };
    // A phone's status bar is never the same twice (its clock, its
    // battery): it is left out of every look, beside what the check says.
    let mut ignore = ignore.to_vec();
    ignore.extend(status_bar);
    // Decoding and comparing whole screens is too slow for the UI thread.
    let (accepted, shot) = (accepted.to_path_buf(), dir.join(&name));
    let diff = dir.join(name.replace(".png", "-diff.png"));
    let mut visual = cx
        .background_executor()
        .spawn(async move { visual::check(&accepted, &shot, &ignore, &diff) })
        .await;
    if let Some(visual) = visual.as_mut().filter(|visual| !visual.areas.is_empty()) {
        visual.ignorable = ignorable(&visual.areas, project_id, android, cx).await;
    }
    Ended {
        visual,
        shot: Some(name),
        ..Ended::ok()
    }
}

/// The room given around a change that is to be left out, where nothing
/// says what it is a part of: sideways (a line of text grows that way) and
/// up and down.
const ROOM: (u32, u32) = (48, 8);
/// The share of the page an element may take and still be "what changed":
/// more is the page itself, and leaving it out would leave nothing checked.
const ELEMENT_MAX: f64 = 0.2;

/// The elements at the middle of each of `areas` of the page, as rectangles
/// (`null` where there is none, or it is most of the page).
fn elements_at(areas: &[[u32; 4]]) -> String {
    let points: Vec<[u32; 2]> = areas
        .iter()
        .map(|[x, y, width, height]| [x + width / 2, y + height / 2])
        .collect();
    format!(
        r#"(() => {{ const most = innerWidth * innerHeight * {ELEMENT_MAX}; return {}.map(([x, y]) => {{ const el = document.elementFromPoint(x, y); if (!el) return null; const r = el.getBoundingClientRect(); return r.width * r.height > most ? null : [Math.max(0, Math.floor(r.x)), Math.max(0, Math.floor(r.y)), Math.ceil(r.width), Math.ceil(r.height)]; }}); }})()"#,
        json!(points)
    )
}

/// What to leave out for each of `areas` (where a look differed) if the
/// user says the change does not matter: on a page, the whole element that
/// changed there (the clock, not the digit that differed this time); with
/// some room around it everywhere.
async fn ignorable(
    areas: &[[u32; 4]],
    project_id: i64,
    android: bool,
    cx: &mut AsyncApp,
) -> Vec<[u32; 4]> {
    let elements = if android {
        Value::Null
    } else {
        call(
            "browser.evaluate",
            json!({ "project_id": project_id, "expression": elements_at(areas), "quiet": true }),
            cx,
        )
        .await
        .map(|answer| answer["value"].clone())
        .unwrap_or_default()
    };
    areas
        .iter()
        .enumerate()
        .map(|(at, area)| {
            let element: Option<[u32; 4]> = serde_json::from_value(elements[at].clone()).ok();
            let around = visual::widened(*area, ROOM, (u32::MAX, u32::MAX));
            match element {
                Some(element) => visual::union(around, element),
                None => around,
            }
        })
        .collect()
}

/// The folder of the accepted looks of the run that left `report`.
fn baseline_of(project_id: i64, report: &Report) -> PathBuf {
    visual::baseline_dir(
        project_id,
        &model::slug(&report.test),
        &Variant::slug(report.variant.as_ref()),
    )
}

/// Makes the screens of the look checks of the run in `dir` (its `report`)
/// their accepted pictures, for its variant: all of them, or only the one
/// at step `only`. Says how many. Blocks: call it off the UI thread.
pub fn accept_looks(
    project_id: i64,
    dir: &Path,
    report: &Report,
    only: Option<usize>,
) -> Result<usize> {
    let baseline = baseline_of(project_id, report);
    let mut accepted = 0;
    let mut ids = Vec::new();
    for (index, result) in report.steps.iter().enumerate() {
        let Step::ExpectLook { id, .. } = &result.step else {
            continue;
        };
        ids.push(id.as_str());
        if only.is_some_and(|only| only != index) {
            continue;
        }
        if let Some(shot) = &result.screenshot {
            visual::accept(&dir.join(shot), &baseline, id)?;
            accepted += 1;
        }
    }
    visual::prune(&baseline, &ids);
    // The run now looks as accepted: its report says so, and the test
    // passes without being run again.
    let mut report = report.clone();
    for (index, result) in report.steps.iter_mut().enumerate() {
        if matches!(result.step, Step::ExpectLook { .. })
            && only.is_none_or(|only| only == index)
            && result.screenshot.is_some()
        {
            result.visual = Some(Visual {
                changed: 0.0,
                pixels: 0,
                diff: None,
                accepted: None,
                areas: Vec::new(),
                ignorable: Vec::new(),
                resized: false,
            });
        }
    }
    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(accepted)
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

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_runs_are_kept() {
        let test = std::env::temp_dir().join(format!("ph-uitest-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&test);
        for millis in [1_700_000_000_003_i64, 1_700_000_000_001, 1_700_000_000_002] {
            std::fs::create_dir_all(test.join(millis.to_string())).unwrap();
        }
        prune_runs(&test, 2);
        let mut left: Vec<String> = std::fs::read_dir(&test)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["1700000000002", "1700000000003"]);
        let _ = std::fs::remove_dir_all(&test);
    }

    #[test]
    fn checks_read_the_screen_as_a_reader_does() {
        let shows = |text: &str| Step::ExpectText { text: text.into() };
        assert!(holds(&shows("Welcome"), "welcome back, ada", ""));
        assert!(!holds(&shows("Error"), "all good", ""));
        // Words a page breaks over two lines or elements read the same.
        assert!(holds(&shows("Cart: Coffee"), "Cart:\n\n  coffee\n", ""));
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
    fn an_address_on_this_computer_is_told_from_the_web() {
        assert_eq!(
            local_address("http://localhost:3000/login").as_deref(),
            Some("localhost:3000")
        );
        assert_eq!(
            local_address("http://127.0.0.1/").as_deref(),
            Some("127.0.0.1:80")
        );
        assert_eq!(
            local_address("https://app.localhost").as_deref(),
            Some("app.localhost:443")
        );
        assert_eq!(
            local_address("http://[::1]:8080/x").as_deref(),
            Some("[::1]:8080")
        );
        assert_eq!(local_address("https://example.com/"), None);
        assert_eq!(local_address("file:///x.html"), None);
        // A port nothing listens on, and one something does.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(answers(&format!("127.0.0.1:{port}")));
        drop(listener);
        assert!(!answers(&format!("127.0.0.1:{port}")));
    }

    #[test]
    fn the_elements_under_changes_are_asked_for_by_their_middles() {
        let script = elements_at(&[[100, 40, 20, 10], [0, 0, 16, 16]]);
        assert!(script.contains("[[110,45],[8,8]].map("), "{script}");
        assert!(script.contains("innerWidth * innerHeight * 0.2"));
    }

    #[test]
    fn a_pictures_size_is_read_from_its_first_bytes() {
        let png = ph_media::Frame::from_rgba(7, 3, vec![0; 7 * 3 * 4])
            .encode_png()
            .unwrap();
        assert_eq!(png_size(&png), Some((7, 3)));
        assert_eq!(png_size(&png[..20]), None);
        assert_eq!(png_size(b"not a picture, though long enough"), None);
    }

    #[test]
    fn an_error_reads_as_a_sentence() {
        assert_eq!(
            sentence("no button “Go” shows, after waiting 10 s"),
            "No button “Go” shows, after waiting 10 s."
        );
        assert_eq!(sentence("Already one."), "Already one.");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn replay_starts_at_the_target() {
        let mut test = TestCase {
            name: "t".into(),
            description: String::new(),
            target: Target::Web {
                url: "http://app".into(),
            },
            clean: None,
            steps: vec![Step::Press {
                key: "Enter".into(),
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
    fn reports_say_what_failed_and_what_looks_new() {
        let result = |step: Step, outcome: &str, visual: Option<Visual>| StepResult {
            step,
            outcome: outcome.into(),
            error: None,
            hint: None,
            millis: 1,
            screenshot: None,
            visual,
        };
        let press = || Step::Press {
            key: "Enter".into(),
        };
        let look = || Step::ExpectLook {
            id: "look-1".into(),
            ignore: Vec::new(),
        };
        let mut report = Report {
            test: "t".into(),
            started_at: 0,
            device: None,
            variant: None,
            picture: None,
            steps: vec![
                result(press(), "passed", None),
                result(press(), "failed", None),
                result(press(), "skipped", None),
            ],
        };
        assert!(!report.passed());
        assert_eq!(report.failure().map(|(ix, _)| ix), Some(1));
        // A look with nothing accepted yet passes, and is counted as new;
        // one that differs does not pass.
        report.steps = vec![
            result(press(), "passed", None),
            result(look(), "passed", None),
        ];
        assert!(report.passed());
        assert_eq!(report.looks_new(), 1);
        let differs = Visual {
            changed: 0.001,
            pixels: 90,
            diff: None,
            accepted: None,
            areas: Vec::new(),
            ignorable: Vec::new(),
            resized: false,
        };
        report.steps[1].visual = Some(differs);
        assert!(report.steps_passed() && !report.passed());
        assert_eq!((report.looks_different(), report.looks_new()), (1, 0));
    }
}
