//! `device.*` control-socket methods, used by the MCP bridge for agents.
//!
//! Each method resolves the device (the `serial` given, else the one in the
//! pane, else the only one connected) and runs adb off the UI thread.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use base64::Engine as _;
use gpui_kit::{App, AppContext as _, Entity, Task};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::adb::Adb;
use crate::emulator::EmulatorControl;
use crate::logcat;
use crate::manager::DeviceManager;
use crate::ui_tree;

/// Screenshots for agents are scaled to at most this many pixels per side.
const AGENT_IMAGE_MAX: u32 = 1280;

pub fn register(cx: &mut App) {
    ControlServer::handle_async(
        "device.list",
        |_, cx| {
            with_manager(cx, |manager, cx| {
                manager.update(cx, |manager, cx| manager.ensure_tracking(cx));
                let Some(adb) = manager.read(cx).adb().cloned() else {
                    return Task::ready(Err(anyhow!(crate::sdk::Sdk::missing_message())));
                };
                let avds = manager.read(cx).avds().to_vec();
                let emulator = manager.read(cx).emulator_tool();
                cx.background_spawn(async move {
                    let devices = adb.devices()?;
                    // The list may still be loading on the first call.
                    let avds = if avds.is_empty() {
                        crate::manager::list_avds(emulator.as_deref())
                    } else {
                        avds
                    };
                    Ok(json!({
                        "devices": devices.iter().map(|device| json!({
                            "serial": device.serial, "name": device.name,
                            "state": device.state, "emulator": device.emulator,
                        })).collect::<Vec<_>>(),
                        "avds": avds,
                    }))
                })
            })
        },
        cx,
    );

    ControlServer::handle_async(
        "device.start_emulator",
        |params, cx| {
            let Some(avd) = params["avd"].as_str().map(str::to_string) else {
                return Task::ready(Err(anyhow!("`avd` is required")));
            };
            with_manager(cx, move |manager, cx| {
                let start = manager.update(cx, |manager, cx| manager.start_emulator(&avd, cx));
                cx.background_spawn(async move { Ok(json!({ "serial": start.await? })) })
            })
        },
        cx,
    );

    // With `status_bar`, where the status bar is in the picture: a UI
    // test's look check leaves it out (its clock is never the same).
    on_device("device.screenshot", cx, |adb, serial, params| {
        let frame = if serial.starts_with("emulator-")
            && let Ok(control) = EmulatorControl::connect(&serial)
        {
            control.screenshot(10_000, 10_000)?
        } else {
            adb.screencap(&serial, true)?
        };
        let (device_width, device_height) = (frame.width, frame.height);
        let image = frame.downscaled(AGENT_IMAGE_MAX);
        let png = base64::engine::general_purpose::STANDARD.encode(image.encode_png()?);
        let scale = device_width as f32 / image.width as f32;
        let status_bar = (params["status_bar"] == true)
            .then(|| adb.status_bar(&serial))
            .flatten()
            .map(|(left, top, right, bottom)| {
                let on_picture = |value: i32| (value as f32 / scale).round().max(0.) as u32;
                // A little more than it says: a picture made smaller blurs
                // its lower edge into the next rows.
                [
                    on_picture(left),
                    on_picture(top),
                    on_picture(right - left),
                    on_picture(bottom - top) + 2,
                ]
            });
        Ok(json!({
            "status_bar": status_bar,
            "width": device_width,
            "height": device_height,
            "note": if scale > 1.01 {
                format!("The image is {}×{}; multiply its coordinates by {scale:.3} for device_tap.", image.width, image.height)
            } else {
                String::new()
            },
            "images": [{ "data": png, "mimeType": "image/png" }],
        }))
    });

    on_device("device.ui", cx, |adb, serial, _| {
        let nodes = ui_tree::parse(&adb.ui_dump(&serial)?);
        let result = json!({ "text": ui_tree::summary(&nodes), "texts": ui_tree::texts(&nodes) });
        remember(&serial, nodes);
        Ok(result)
    });

    // A tap at a point, or (for a UI test's replay) on the element named by
    // `id`, `text` or `desc`, waited for up to `wait_ms`. A point recorded
    // on another screen says which (`screen`) and is moved to this one's.
    on_device("device.tap", cx, |adb, serial, params| {
        let mark = ui_tree::Mark {
            id: text(&params, "id"),
            text: text(&params, "text"),
            description: text(&params, "desc"),
        };
        let recorded = match (params["x"].as_f64(), params["y"].as_f64()) {
            (Some(x), Some(y)) => Some((x.round() as i32, y.round() as i32)),
            _ => None,
        };
        let from = size(&params["screen"]);
        if !mark.is_empty() {
            let deadline = Instant::now() + wait(&params);
            loop {
                // A screen that does not hold still gives no hierarchy: not
                // there yet, like an element that has not come.
                let found = adb
                    .ui_dump(&serial)
                    .map_err(|err| format!("{err:#}"))
                    .and_then(|xml| {
                        let nodes = ui_tree::parse(&xml);
                        let near = recorded.map(|point| match (from, ui_tree::screen(&nodes)) {
                            (Some(from), Some(to)) => ui_tree::scale(point, from, to),
                            _ => point,
                        });
                        ui_tree::find(&nodes, &mark, near).map(|node| node.center())
                    });
                match found {
                    Ok((x, y)) => {
                        adb.tap(&serial, x, y)?;
                        forget(&serial);
                        return Ok(json!({ "x": x, "y": y }));
                    }
                    Err(err) if Instant::now() >= deadline => anyhow::bail!("{err}"),
                    Err(_) => std::thread::sleep(Duration::from_millis(300)),
                }
            }
        }
        let (x, y) =
            recorded.context("give `x` and `y`, or the element's `text`, `id` or `desc`")?;
        let (x, y) = match from.zip(from.and_then(|_| adb.screen_size(&serial))) {
            Some((from, to)) => ui_tree::scale((x, y), from, upright(from, to)),
            None => (x, y),
        };
        // While a UI test is recorded, the tap is kept as the element it
        // lands on: what is on the screen is read before it changes.
        let mut result = json!({});
        if RECORDING.load(Ordering::Relaxed) {
            let nodes = match recall(&serial) {
                Some(nodes) => nodes,
                None => adb
                    .ui_dump(&serial)
                    .map(|xml| ui_tree::parse(&xml))
                    .unwrap_or_default(),
            };
            if let Some(node) = ui_tree::node_at(&nodes, x, y) {
                let mark = ui_tree::Mark::of(node);
                if !mark.is_empty() {
                    result["element"] =
                        json!({ "id": mark.id, "text": mark.text, "desc": mark.description });
                }
            }
            if let Some((width, height)) = ui_tree::screen(&nodes) {
                result["screen"] = json!([width, height]);
            }
        }
        adb.tap(&serial, x, y)?;
        forget(&serial);
        Ok(result)
    });

    on_device("device.swipe", cx, |adb, serial, params| {
        let ms = params["duration_ms"].as_u64().unwrap_or(300) as u32;
        let (mut start, mut end) = (
            (int(&params, "x1")?, int(&params, "y1")?),
            (int(&params, "x2")?, int(&params, "y2")?),
        );
        if let Some(from) = size(&params["screen"])
            && let Some(to) = adb.screen_size(&serial)
        {
            let to = upright(from, to);
            (start, end) = (
                ui_tree::scale(start, from, to),
                ui_tree::scale(end, from, to),
            );
        }
        let mut result = json!({});
        if RECORDING.load(Ordering::Relaxed)
            && let Some((width, height)) = adb.screen_size(&serial)
        {
            result["screen"] = json!([width, height]);
        }
        adb.swipe(&serial, start, end, ms)?;
        forget(&serial);
        Ok(result)
    });

    on_device("device.type", cx, |adb, serial, params| {
        // A replay waits (up to `wait_ms`) until something has the focus:
        // text typed while a field is still opening is lost, all but its
        // end. Typed anyway when nothing says it has (a view of the app's
        // own making may never say).
        let deadline = Instant::now() + wait(&params);
        while Instant::now() < deadline {
            let focused = adb
                .ui_dump(&serial)
                .is_ok_and(|xml| ui_tree::parse(&xml).iter().any(|node| node.focused));
            if focused {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        // Said in the result while a UI test is recorded, so that it never
        // keeps a password: whether the field typed into hides its text.
        let mut result = json!({});
        if RECORDING.load(Ordering::Relaxed) {
            let nodes = match recall(&serial) {
                Some(nodes) => nodes,
                None => adb
                    .ui_dump(&serial)
                    .map(|xml| ui_tree::parse(&xml))
                    .unwrap_or_default(),
            };
            if nodes.iter().any(|node| node.focused && node.password) {
                result["secret"] = json!(true);
            }
        }
        adb.type_text(
            &serial,
            params["text"].as_str().context("`text` is required")?,
        )?;
        forget(&serial);
        Ok(result)
    });

    on_device("device.key", cx, |adb, serial, params| {
        let key = params["key"].as_str().context("`key` is required")?;
        adb.key(
            &serial,
            keycode(key).with_context(|| format!("unknown key `{key}`"))?,
        )?;
        forget(&serial);
        Ok(Value::Null)
    });

    on_device("device.logcat", cx, |adb, serial, params| {
        let lines = params["lines"].as_u64().unwrap_or(100) as usize;
        let level = match params["level"].as_str() {
            Some(word) => Some(logcat::Priority::from_word(word).with_context(|| {
                format!("unknown level `{word}`: verbose, debug, info, warn, error or fatal")
            })?),
            None => params["errors_only"]
                .as_bool()
                .unwrap_or(false)
                .then_some(logcat::Priority::Error),
        };
        let package = match params["package"].as_str() {
            Some(package) if !crate::adb::is_package_name(package) => {
                anyhow::bail!("`{package}` is not a package name")
            }
            Some(package) => Some((package, adb.pids_of(&serial, package))),
            None => None,
        };
        let filter = logcat::DumpFilter {
            text: params["filter"].as_str(),
            level,
            package,
        };
        // Read more than asked when filtering, so matches are not cut short.
        let filtering = filter.text.is_some() || filter.level.is_some() || filter.package.is_some();
        let dump = adb.logcat_dump(&serial, if filtering { 5000 } else { lines })?;
        Ok(json!({ "text": logcat::filter_dump(&dump, &filter, lines) }))
    });

    on_device("device.install", cx, |adb, serial, params| {
        let apk = std::path::PathBuf::from(params["apk"].as_str().context("`apk` is required")?);
        if !crate::sdk::is_apk(&apk) {
            anyhow::bail!("{} is not an .apk file", apk.display());
        }
        adb.install(&serial, &apk)?;
        Ok(Value::Null)
    });

    // `fresh` stops the app first, so that it starts at its first screen;
    // `clear` erases its data too (a UI test that asks for a clean start).
    on_device("device.launch", cx, |adb, serial, params| {
        let package = params["package"]
            .as_str()
            .context("`package` is required")?;
        if params["clear"] == true {
            adb.clear_data(&serial, package)?;
        }
        if params["fresh"] == true || params["clear"] == true {
            adb.launch_fresh(&serial, package)?;
        } else {
            adb.launch(&serial, package)?;
        }
        forget(&serial);
        Ok(Value::Null)
    });

    // The app in front: where a new UI test of the phone starts. And the
    // device asked (the one in view, when none is named), which a test's
    // run keeps its accepted looks by.
    on_device("device.front", cx, |adb, serial, _| {
        Ok(json!({ "package": adb.front_app(&serial), "serial": serial }))
    });

    // While a UI test is recorded (`on`), taps say which element they
    // landed on.
    ControlServer::handle(
        "device.record",
        |params, _| {
            RECORDING.store(params["on"] == true, Ordering::Relaxed);
            Ok(json!({}))
        },
        cx,
    );
}

/// A UI test is being recorded: taps look up the element under them.
static RECORDING: AtomicBool = AtomicBool::new(false);

/// How long a screen read for an agent (`device.ui`) stands for the screen:
/// the agent taps what it just read.
const UI_FRESH: Duration = Duration::from_secs(20);

/// A screen as it was read, and when.
type ReadScreen = (Instant, Vec<ui_tree::UiNode>);

/// The screen as last read, by device, until something is done on it.
static LAST_UI: Mutex<Option<HashMap<String, ReadScreen>>> = Mutex::new(None);

fn remember(serial: &str, nodes: Vec<ui_tree::UiNode>) {
    if let Ok(mut last) = LAST_UI.lock() {
        last.get_or_insert_with(HashMap::new)
            .insert(serial.to_string(), (Instant::now(), nodes));
    }
}

/// The screen as an agent last read it, if nothing was done on it since.
fn recall(serial: &str) -> Option<Vec<ui_tree::UiNode>> {
    let last = LAST_UI.lock().ok()?;
    let (read, nodes) = last.as_ref()?.get(serial)?;
    (read.elapsed() < UI_FRESH).then(|| nodes.clone())
}

fn forget(serial: &str) {
    if let Ok(mut last) = LAST_UI.lock()
        && let Some(last) = last.as_mut()
    {
        last.remove(serial);
    }
}

/// `[1080, 2400]` → the screen a point was recorded on.
fn size(value: &Value) -> Option<(i32, i32)> {
    let width = value.get(0)?.as_i64()?;
    let height = value.get(1)?.as_i64()?;
    (width > 0 && height > 0).then_some((width as i32, height as i32))
}

/// `to` (which `wm size` gives upright) turned the way `from` was held.
fn upright(from: (i32, i32), to: (i32, i32)) -> (i32, i32) {
    if (from.0 > from.1) != (to.0 > to.1) {
        (to.1, to.0)
    } else {
        to
    }
}

fn text(params: &Value, key: &str) -> String {
    params[key].as_str().unwrap_or_default().to_string()
}

/// How long a tap waits for its element (`wait_ms`).
fn wait(params: &Value) -> Duration {
    Duration::from_millis(params["wait_ms"].as_u64().unwrap_or(0).min(60_000))
}

/// Registers a method that runs `body` with adb and the resolved serial on
/// a background thread.
fn on_device(name: &'static str, cx: &mut App, body: fn(&Adb, String, Value) -> Result<Value>) {
    ControlServer::handle_async(
        name,
        move |params, cx| {
            with_manager(cx, move |manager, cx| {
                manager.update(cx, |manager, cx| manager.ensure_tracking(cx));
                let manager = manager.read(cx);
                let Some(adb) = manager.adb().cloned() else {
                    return Task::ready(Err(anyhow!(crate::sdk::Sdk::missing_message())));
                };
                // The list may still be loading; then ask adb directly.
                let known = manager.resolve(params["serial"].as_str()).ok();
                cx.background_spawn(async move {
                    let serial = match known {
                        Some(serial) => serial,
                        None => DeviceManager::resolve_from(&adb.devices()?)?,
                    };
                    body(&adb, serial, params)
                })
            })
        },
        cx,
    );
}

/// Runs `then` with the device manager once it has looked for the SDK (it
/// does that on a background thread when it is first used): an agent's
/// first call is not told there is no SDK while it is still being found.
fn with_manager(
    cx: &mut App,
    then: impl FnOnce(Entity<DeviceManager>, &mut App) -> Task<Result<Value>> + 'static,
) -> Task<Result<Value>> {
    let manager = DeviceManager::global(cx);
    let located = manager.read(cx).located();
    cx.spawn(async move |cx| {
        located.await;
        cx.update(|cx| then(manager, cx)).await
    })
}

fn int(params: &Value, key: &str) -> Result<i32> {
    params[key]
        .as_f64()
        .map(|value| value.round() as i32)
        .with_context(|| format!("`{key}` is required"))
}

/// Android key codes for the keys agents can press.
fn keycode(key: &str) -> Option<u32> {
    Some(match key.to_ascii_lowercase().as_str() {
        "back" => 4,
        "home" => 3,
        "enter" => 66,
        "delete" | "backspace" => 67,
        "tab" => 61,
        "menu" => 82,
        "recents" | "app_switch" => 187,
        "up" => 19,
        "down" => 20,
        "left" => 21,
        "right" => 22,
        "volume_up" => 24,
        "volume_down" => 25,
        "power" => 26,
        "escape" => 111,
        _ => return None,
    })
}
