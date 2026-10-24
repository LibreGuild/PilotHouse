//! `device.*` control-socket methods, used by the MCP bridge for agents.
//!
//! Each method resolves the device (the `serial` given, else the one in the
//! pane, else the only one connected) and runs adb off the UI thread.

use anyhow::{Context as _, Result, anyhow};
use base64::Engine as _;
use gpui_kit::{App, AppContext as _, Task};
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
            let manager = DeviceManager::global(cx);
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
        },
        cx,
    );

    ControlServer::handle_async(
        "device.start_emulator",
        |params, cx| {
            let Some(avd) = params["avd"].as_str().map(str::to_string) else {
                return Task::ready(Err(anyhow!("`avd` is required")));
            };
            let manager = DeviceManager::global(cx);
            let start = manager.update(cx, |manager, cx| manager.start_emulator(&avd, cx));
            cx.background_spawn(async move { Ok(json!({ "serial": start.await? })) })
        },
        cx,
    );

    on_device("device.screenshot", cx, |adb, serial, _| {
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
        Ok(json!({
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
        Ok(json!({ "text": ui_tree::summary(&nodes) }))
    });

    on_device("device.tap", cx, |adb, serial, params| {
        adb.tap(&serial, int(&params, "x")?, int(&params, "y")?)?;
        Ok(Value::Null)
    });

    on_device("device.swipe", cx, |adb, serial, params| {
        let ms = params["duration_ms"].as_u64().unwrap_or(300) as u32;
        adb.swipe(
            &serial,
            (int(&params, "x1")?, int(&params, "y1")?),
            (int(&params, "x2")?, int(&params, "y2")?),
            ms,
        )?;
        Ok(Value::Null)
    });

    on_device("device.type", cx, |adb, serial, params| {
        adb.type_text(
            &serial,
            params["text"].as_str().context("`text` is required")?,
        )?;
        Ok(Value::Null)
    });

    on_device("device.key", cx, |adb, serial, params| {
        let key = params["key"].as_str().context("`key` is required")?;
        adb.key(
            &serial,
            keycode(key).with_context(|| format!("unknown key `{key}`"))?,
        )?;
        Ok(Value::Null)
    });

    on_device("device.logcat", cx, |adb, serial, params| {
        let lines = params["lines"].as_u64().unwrap_or(100) as usize;
        let filter = params["filter"].as_str();
        let errors_only = params["errors_only"].as_bool().unwrap_or(false);
        // Read more than asked when filtering, so matches are not cut short.
        let dump = adb.logcat_dump(
            &serial,
            if filter.is_some() || errors_only {
                5000
            } else {
                lines
            },
        )?;
        Ok(json!({ "text": logcat::filter_dump(&dump, filter, errors_only, lines) }))
    });

    on_device("device.install", cx, |adb, serial, params| {
        let apk = std::path::PathBuf::from(params["apk"].as_str().context("`apk` is required")?);
        if !crate::sdk::is_apk(&apk) {
            anyhow::bail!("{} is not an .apk file", apk.display());
        }
        adb.install(&serial, &apk)?;
        Ok(Value::Null)
    });

    on_device("device.launch", cx, |adb, serial, params| {
        adb.launch(
            &serial,
            params["package"]
                .as_str()
                .context("`package` is required")?,
        )?;
        Ok(Value::Null)
    });
}

/// Registers a method that runs `body` with adb and the resolved serial on
/// a background thread.
fn on_device(name: &'static str, cx: &mut App, body: fn(&Adb, String, Value) -> Result<Value>) {
    ControlServer::handle_async(
        name,
        move |params, cx| {
            let manager = DeviceManager::global(cx);
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
        },
        cx,
    );
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
