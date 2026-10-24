//! UI automation over the control socket (development builds only).
//!
//! Wraps gpui-kit's UI test helpers so a script or coding agent can drive the
//! *running* app: list elements, click by id or accessible label, type, press
//! keys and capture the window as a PNG. Input is injected straight into
//! GPUI's event dispatch, so the real mouse and keyboard focus are untouched
//! and the window can stay in the background.
//!
//! | Method          | Params                                   |
//! | --------------- | ---------------------------------------- |
//! | `ui.elements`   | `{}`                                     |
//! | `ui.click`      | `{"id": "save"}` or `{"label": "Save"}`  |
//! | `ui.right_click`| same as `ui.click`                       |
//! | `ui.input`      | `{"text": "hello"}`                      |
//! | `ui.press`      | `{"key": "secondary-k"}`                 |
//! | `ui.screenshot` | `{"path": "/tmp/shot.png"}`              |
//! | `ui.dispatch`   | `{"action": "workspace::NewSession"}`    |
//!
//! Targets can also be given as `{"path": "suffix"}`, matching the end of an
//! element's identity path as listed by `ui.elements` (for rows that have no
//! stable id or observable label).

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::test::{ElementSnapshot, TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{App, ElementId, MouseButton, SharedString, Window};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ControlServer;

/// Which window `ui.*` calls act on: an index into the app's windows, set
/// with `ui.window`. Unset means the first (the main window).
#[derive(Default)]
struct TargetWindow(Option<usize>);

impl gpui_kit::Global for TargetWindow {}

/// Registers every `ui.*` method.
pub fn register(cx: &mut App) {
    ControlServer::handle(
        "ui.window",
        |params, cx| {
            let count = cx.windows().len();
            if let Some(index) = params.get("index").and_then(Value::as_u64) {
                let index = index as usize;
                if index >= count {
                    bail!("there are {count} windows");
                }
                cx.default_global::<TargetWindow>().0 = Some(index);
            }
            Ok(
                json!({ "windows": count, "target": cx.default_global::<TargetWindow>().0.unwrap_or(0) }),
            )
        },
        cx,
    );
    ControlServer::handle("ui.elements", |_, cx| with_window(cx, elements), cx);
    ControlServer::handle(
        "ui.click",
        |params, cx| {
            with_window(cx, |window, cx| {
                click(params, MouseButton::Left, window, cx)
            })
        },
        cx,
    );
    ControlServer::handle(
        "ui.right_click",
        |params, cx| {
            with_window(cx, |window, cx| {
                click(params, MouseButton::Right, window, cx)
            })
        },
        cx,
    );
    ControlServer::handle(
        "ui.hover",
        |params, cx| with_window(cx, |window, cx| hover(params, window, cx)),
        cx,
    );
    ControlServer::handle(
        "ui.drag",
        |params, cx| with_window(cx, |window, cx| drag(params, window, cx)),
        cx,
    );
    ControlServer::handle(
        "ui.scroll",
        |params, cx| with_window(cx, |window, cx| scroll(params, window, cx)),
        cx,
    );
    ControlServer::handle(
        "ui.input",
        |params, cx| {
            let text = string_param(&params, "text")?;
            with_window(cx, |window, cx| {
                window.input(&text, cx);
                Ok(Value::Null)
            })
        },
        cx,
    );
    ControlServer::handle(
        "ui.press",
        |params, cx| {
            let key = string_param(&params, "key")?;
            with_window(cx, |window, cx| {
                window.press(&key, cx);
                Ok(Value::Null)
            })
        },
        cx,
    );
    ControlServer::handle(
        "ui.dispatch",
        |params, cx| {
            let name = string_param(&params, "action")?;
            let action = cx
                .build_action(&name, params.get("data").cloned())
                .map_err(|err| anyhow!("unknown action `{name}`: {err}"))?;
            with_window(cx, |window, cx| {
                window.dispatch_action(action, cx);
                Ok(Value::Null)
            })
        },
        cx,
    );
    ControlServer::handle(
        "ui.screenshot",
        |params, cx| {
            let path = string_param(&params, "path")?;
            with_window(cx, |window, cx| screenshot(&path, window, cx))
        },
        cx,
    );
}

/// Runs `f` against the target window, after rendering a fresh frame so
/// queries and captures reflect the current state even when the window is
/// hidden behind others.
fn with_window(
    cx: &mut App,
    f: impl FnOnce(&mut Window, &mut App) -> Result<Value>,
) -> Result<Value> {
    let windows = cx.windows();
    let target = cx.try_global::<TargetWindow>().and_then(|target| target.0);
    let handle = target
        .and_then(|index| windows.get(index))
        .or_else(|| windows.first())
        .copied()
        .ok_or_else(|| anyhow!("no window is open"))?;
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            f(window, cx)
        })
        .map_err(|err| anyhow!("window is gone: {err}"))?
}

#[derive(Deserialize)]
struct Target {
    id: Option<String>,
    label: Option<String>,
    path: Option<String>,
}

fn resolve(params: Value, window: &mut Window) -> Result<ElementSnapshot> {
    let target: Target =
        serde_json::from_value(params).context("expected {\"id\"}, {\"label\"} or {\"path\"}")?;
    let found = if let Some(id) = target.id {
        // `ui.elements` prints `("name", 7)` ids as `name-7`, so accept both forms.
        let named_integer = id.rsplit_once('-').and_then(|(name, number)| {
            Some(ElementId::NamedInteger(
                name.to_string().into(),
                number.parse().ok()?,
            ))
        });
        window
            .try_find(ElementId::Name(SharedString::from(id.clone())))
            .or_else(|| named_integer.and_then(|id| window.try_find(id)))
            .ok_or_else(|| anyhow!("no element with id `{id}`"))?
    } else if let Some(label) = target.label {
        // Never the panicking lookup: an ambiguous label is an error here,
        // not a crash of the app being driven.
        let mut visible: Vec<_> = window
            .find_all_by_label(&label)
            .into_iter()
            .filter(|element| element.visible())
            .collect();
        match visible.len() {
            0 => bail!("no element labelled `{label}`"),
            1 => visible.remove(0),
            n => bail!("{n} elements are labelled `{label}`; use an `id` or `path`"),
        }
    } else if let Some(suffix) = target.path {
        let mut matches = window.elements().into_iter().filter(|element| {
            let path: Vec<String> = element.path().iter().map(ToString::to_string).collect();
            path.join("/").ends_with(&suffix)
        });
        let found = matches
            .next()
            .ok_or_else(|| anyhow!("no element path ends with `{suffix}`"))?;
        if matches.next().is_some() {
            bail!("more than one element path ends with `{suffix}`");
        }
        found
    } else {
        bail!("give an `id`, a `label` or a `path`");
    };
    if !found.visible() {
        bail!("element is not visible: {found:?}");
    }
    Ok(found)
}

fn click(params: Value, button: MouseButton, window: &mut Window, cx: &mut App) -> Result<Value> {
    // `at: [x, y]` clicks a window point, e.g. inside a live browser pane.
    if let Some(at) = point_param(&params, "at") {
        window.pointer_move(at, None, cx);
        window.pointer_down(at, button, cx);
        window.pointer_up(at, button, cx);
        return Ok(json!({ "clicked": [f32::from(at.x), f32::from(at.y)] }));
    }
    let target = resolve(params, window)?;
    let center = target.bounds().center();
    window.pointer_move(center, None, cx);
    window.pointer_down(center, button, cx);
    window.pointer_up(center, button, cx);
    Ok(json!({ "clicked": describe(&target) }))
}

/// Moves the pointer over a point or element (hover cards, tooltips).
fn hover(params: Value, window: &mut Window, cx: &mut App) -> Result<Value> {
    let at = match point_param(&params, "at") {
        Some(at) => at,
        None => resolve(params, window)?.bounds().center(),
    };
    window.pointer_move(at, None, cx);
    Ok(json!({ "hovered": [f32::from(at.x), f32::from(at.y)] }))
}

/// Presses at `from`, moves to `to` and releases (drawing on a live pane).
fn drag(params: Value, window: &mut Window, cx: &mut App) -> Result<Value> {
    let from = point_param(&params, "from").context("missing `from: [x, y]`")?;
    let to = point_param(&params, "to").context("missing `to: [x, y]`")?;
    window.pointer_move(from, None, cx);
    window.pointer_down(from, MouseButton::Left, cx);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let at = gpui_kit::point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
        window.pointer_move(at, Some(MouseButton::Left), cx);
    }
    window.pointer_up(to, MouseButton::Left, cx);
    Ok(Value::Null)
}

/// Scrolls the wheel by `dy` pixels (positive scrolls down) at `at: [x, y]`.
fn scroll(params: Value, window: &mut Window, cx: &mut App) -> Result<Value> {
    use gpui_kit::InputEvent as _;
    let at = point_param(&params, "at").context("missing `at: [x, y]`")?;
    let dy = params.get("dy").and_then(Value::as_f64).unwrap_or(300.0) as f32;
    window.pointer_move(at, None, cx);
    window.dispatch_event(
        gpui_kit::ScrollWheelEvent {
            position: at,
            // A positive wheel delta scrolls up, as on a trackpad.
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-dy),
            )),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    );
    Ok(json!({ "scrolled": dy }))
}

fn point_param(params: &Value, key: &str) -> Option<gpui_kit::Point<gpui_kit::Pixels>> {
    let pair = params.get(key)?.as_array()?;
    Some(gpui_kit::point(
        gpui_kit::px(pair.first()?.as_f64()? as f32),
        gpui_kit::px(pair.get(1)?.as_f64()? as f32),
    ))
}

fn elements(window: &mut Window, _: &mut App) -> Result<Value> {
    let list: Vec<Value> = window
        .elements()
        .iter()
        .filter(|element| element.visible())
        .map(describe)
        .collect();
    Ok(Value::Array(list))
}

fn describe(element: &ElementSnapshot) -> Value {
    let bounds = element.bounds();
    let path: Vec<String> = element.path().iter().map(ToString::to_string).collect();
    json!({
        "id": path.last(),
        "path": path.join("/"),
        "role": element.role().map(|role| format!("{role:?}")),
        "label": element.label(),
        "value": element.value(),
        "focused": element.focused(),
        "selected": element.selected(),
        "disabled": element.disabled(),
        "bounds": [
            f32::from(bounds.origin.x), f32::from(bounds.origin.y),
            f32::from(bounds.size.width), f32::from(bounds.size.height)
        ],
    })
}

fn screenshot(path: &str, window: &mut Window, _: &mut App) -> Result<Value> {
    let image = window
        .render_to_image()
        .context("rendering the window to an image")?;
    let (width, height) = image.dimensions();
    image
        .save_with_format(path, image::ImageFormat::Png)
        .with_context(|| format!("writing {path}"))?;
    Ok(json!({ "path": path, "width": width, "height": height }))
}

fn string_param(params: &Value, key: &str) -> Result<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing string parameter `{key}`"))
}
