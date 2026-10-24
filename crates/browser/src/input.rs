//! Turning GPUI input into DevTools `Input.*` events.

use gpui_kit::Keystroke;
use serde_json::{Value, json};

/// DevTools modifier bits: Alt=1, Ctrl=2, Meta=4, Shift=8.
fn modifiers(keystroke: &Keystroke) -> u32 {
    let m = &keystroke.modifiers;
    (m.alt as u32)
        | ((m.control as u32) << 1)
        | ((m.platform as u32) << 2)
        | ((m.shift as u32) << 3)
}

/// A named key: (DOM key, DOM code, Windows virtual key code, text it types).
fn named_key(key: &str) -> Option<(&'static str, &'static str, u32, Option<&'static str>)> {
    Some(match key {
        "enter" => ("Enter", "Enter", 13, Some("\r")),
        "backspace" => ("Backspace", "Backspace", 8, None),
        "tab" => ("Tab", "Tab", 9, None),
        "escape" => ("Escape", "Escape", 27, None),
        "delete" => ("Delete", "Delete", 46, None),
        "space" => (" ", "Space", 32, Some(" ")),
        "left" => ("ArrowLeft", "ArrowLeft", 37, None),
        "up" => ("ArrowUp", "ArrowUp", 38, None),
        "right" => ("ArrowRight", "ArrowRight", 39, None),
        "down" => ("ArrowDown", "ArrowDown", 40, None),
        "home" => ("Home", "Home", 36, None),
        "end" => ("End", "End", 35, None),
        "pageup" => ("PageUp", "PageUp", 33, None),
        "pagedown" => ("PageDown", "PageDown", 34, None),
        _ => return None,
    })
}

/// The key events (down, up) for a keystroke, or `None` for keys a page
/// cannot use (lone modifiers, function keys).
pub fn key_events(keystroke: &Keystroke) -> Option<Vec<Value>> {
    let modifiers = modifiers(keystroke);
    if let Some((key, code, vk, text)) = named_key(&keystroke.key) {
        let mut down = json!({
            "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers,
        });
        if let Some(text) = text {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        let up = json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers });
        return Some(vec![down, up]);
    }
    let text = keystroke.key_char.clone().filter(|text| !text.is_empty())?;
    let vk = text
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase() as u32)
        .unwrap_or(0);
    Some(vec![
        json!({ "type": "keyDown", "key": text, "text": text, "unmodifiedText": text, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
        json!({ "type": "keyUp", "key": text, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
    ])
}

/// Key events for a key named by an agent ("Enter", "Tab", "ArrowDown", "a").
pub fn named_key_events(name: &str) -> Vec<Value> {
    let lower = name.to_ascii_lowercase();
    let key = match lower.as_str() {
        "return" => "enter",
        "esc" => "escape",
        "arrowleft" => "left",
        "arrowright" => "right",
        "arrowup" => "up",
        "arrowdown" => "down",
        " " => "space",
        other => other,
    };
    let keystroke = Keystroke {
        key: key.to_string(),
        key_char: (name.chars().count() == 1).then(|| name.to_string()),
        ..Default::default()
    };
    key_events(&keystroke).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_and_printable_keys() {
        let enter = named_key_events("Enter");
        assert_eq!(enter[0]["key"], "Enter");
        assert_eq!(enter[0]["text"], "\r");
        let a = named_key_events("a");
        assert_eq!(a[0]["text"], "a");
        assert!(named_key_events("F13").is_empty());
    }
}
