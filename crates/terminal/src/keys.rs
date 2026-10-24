//! Translating GPUI keystrokes into the bytes a terminal program expects.

use gpui_kit::Keystroke;

/// Bytes for `keystroke`, or `None` when the terminal should not consume it
/// (⌘ shortcuts belong to the app: copy, paste, palette...).
///
/// `app_cursor` is the DECCKM mode: programs like vim ask for `ESC O A`
/// instead of `ESC [ A` for the arrow keys.
pub fn to_bytes(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = &keystroke.modifiers;
    if modifiers.platform {
        return None;
    }
    let arrow = |code: u8| -> Vec<u8> {
        if app_cursor {
            vec![0x1b, b'O', code]
        } else {
            vec![0x1b, b'[', code]
        }
    };
    let named: Option<Vec<u8>> = match keystroke.key.as_str() {
        "enter" => Some(b"\r".to_vec()),
        "backspace" if modifiers.alt => Some(vec![0x1b, 0x7f]),
        "backspace" => Some(vec![0x7f]),
        "tab" if modifiers.shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(b"\t".to_vec()),
        "escape" => Some(vec![0x1b]),
        "up" => Some(arrow(b'A')),
        "down" => Some(arrow(b'B')),
        "right" => Some(arrow(b'C')),
        "left" => Some(arrow(b'D')),
        "home" => Some(b"\x1b[H".to_vec()),
        "end" => Some(b"\x1b[F".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "insert" => Some(b"\x1b[2~".to_vec()),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    // Ctrl+letter → the control character (Ctrl+C is 0x03).
    if modifiers.control {
        let key = keystroke.key.as_bytes();
        if key.len() == 1 {
            let byte = key[0].to_ascii_lowercase();
            return match byte {
                b'a'..=b'z' => Some(vec![byte - b'a' + 1]),
                b'@' | b' ' => Some(vec![0]),
                b'[' => Some(vec![0x1b]),
                b'\\' => Some(vec![0x1c]),
                b']' => Some(vec![0x1d]),
                _ => None,
            };
        }
        return None;
    }

    let text = keystroke.key_char.as_deref().or_else(|| {
        // Plain printable keys without a composed character.
        (keystroke.key.chars().count() == 1).then_some(keystroke.key.as_str())
    })?;
    let mut bytes = Vec::new();
    if modifiers.alt {
        bytes.push(0x1b); // Meta sends ESC first.
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(key: &str) -> Option<Vec<u8>> {
        to_bytes(&Keystroke::parse(key).unwrap(), false)
    }

    #[test]
    fn control_and_named_keys() {
        assert_eq!(bytes("ctrl-c"), Some(vec![3]));
        assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
        assert_eq!(bytes("backspace"), Some(vec![0x7f]));
        assert_eq!(bytes("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            to_bytes(&Keystroke::parse("up").unwrap(), true),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn app_shortcuts_are_not_sent() {
        assert_eq!(bytes("cmd-k"), None);
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(bytes("alt-b"), Some(vec![0x1b, b'b']));
    }
}
