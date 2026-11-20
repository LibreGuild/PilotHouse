//! Translating GPUI keystrokes into the bytes a terminal program expects.

use gpui_kit::Keystroke;

/// Bytes for `keystroke`, or `None` when the terminal should not consume it
/// (⌘ shortcuts belong to the app: copy, paste, palette...).
///
/// `app_cursor` is the DECCKM mode: programs like vim ask for `ESC O A`
/// instead of `ESC [ A` for the arrow keys.
pub fn to_bytes(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    bytes_for(keystroke, app_cursor, cfg!(target_os = "macos"))
}

/// [`to_bytes`], with `macos` saying whether Option+Left/Right move by word
/// as Terminal.app has them (`ESC b`, `ESC f`): the shells there bind those,
/// not xterm's `ESC [1;3D`.
fn bytes_for(keystroke: &Keystroke, app_cursor: bool, macos: bool) -> Option<Vec<u8>> {
    let modifiers = &keystroke.modifiers;
    if modifiers.platform {
        return None;
    }
    // xterm's modifier number: Shift 1, Alt 2, Ctrl 4, plus one. Not `fn`,
    // which macOS reports with every arrow key.
    let modifier = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    // A cursor key (`ESC [ A`): `ESC O A` in application cursor mode, and
    // `ESC [1;5A` with modifiers held, as xterm sends them.
    let cursor = |code: u8| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{}", code as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', code]
        } else {
            vec![0x1b, b'[', code]
        }
    };
    // F1–F4 are `ESC O P`…, `ESC [1;5P` with modifiers.
    let function = |code: u8| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{}", code as char).into_bytes()
        } else {
            vec![0x1b, b'O', code]
        }
    };
    // `ESC [5~`, `ESC [5;5~` with modifiers.
    let tilde = |number: u8| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[{number};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{number}~").into_bytes()
        }
    };
    let alt_only = modifiers.alt && !modifiers.control && !modifiers.shift;
    let named: Option<Vec<u8>> = match keystroke.key.as_str() {
        // A new line without sending, in a program that takes several
        // lines (agent CLIs do): a line feed, which is what Ctrl+J types.
        // Claude Code, Codex and Gemini CLI read it as "new line"; to a
        // shell it is Enter, as before.
        "enter" if modifiers.shift => Some(b"\n".to_vec()),
        // Meta+Enter, their other way to say it.
        "enter" if modifiers.alt => Some(b"\x1b\r".to_vec()),
        "enter" => Some(b"\r".to_vec()),
        "backspace" if modifiers.alt => Some(vec![0x1b, 0x7f]),
        "backspace" => Some(vec![0x7f]),
        "tab" if modifiers.shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(b"\t".to_vec()),
        "escape" => Some(vec![0x1b]),
        "left" if macos && alt_only => Some(b"\x1bb".to_vec()),
        "right" if macos && alt_only => Some(b"\x1bf".to_vec()),
        "up" => Some(cursor(b'A')),
        "down" => Some(cursor(b'B')),
        "right" => Some(cursor(b'C')),
        "left" => Some(cursor(b'D')),
        "home" => Some(cursor(b'H')),
        "end" => Some(cursor(b'F')),
        "pageup" => Some(tilde(5)),
        "pagedown" => Some(tilde(6)),
        "delete" => Some(tilde(3)),
        "insert" => Some(tilde(2)),
        "f1" => Some(function(b'P')),
        "f2" => Some(function(b'Q')),
        "f3" => Some(function(b'R')),
        "f4" => Some(function(b'S')),
        "f5" => Some(tilde(15)),
        "f6" => Some(tilde(17)),
        "f7" => Some(tilde(18)),
        "f8" => Some(tilde(19)),
        "f9" => Some(tilde(20)),
        "f10" => Some(tilde(21)),
        "f11" => Some(tilde(23)),
        "f12" => Some(tilde(24)),
        // GPUI names the key `space`, so the Ctrl+character rule below
        // never sees it: NUL, Emacs' set-mark.
        "space" if modifiers.control => Some(vec![0]),
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
        to_bytes(
            &Keystroke::parse_with_os(key, gpui_kit::OperatingSystem::Mac).unwrap(),
            false,
        )
    }

    #[test]
    fn control_and_named_keys() {
        assert_eq!(bytes("ctrl-c"), Some(vec![3]));
        assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
        // A new line in an agent's prompt, not "send".
        assert_eq!(bytes("shift-enter"), Some(b"\n".to_vec()));
        assert_eq!(bytes("alt-enter"), Some(b"\x1b\r".to_vec()));
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

    fn on(key: &str, app_cursor: bool, macos: bool) -> Option<Vec<u8>> {
        bytes_for(
            &Keystroke::parse_with_os(key, gpui_kit::OperatingSystem::Linux).unwrap(),
            app_cursor,
            macos,
        )
    }

    #[test]
    fn function_keys_and_ctrl_space() {
        assert_eq!(on("f1", false, false), Some(b"\x1bOP".to_vec()));
        assert_eq!(on("f5", false, false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(on("f10", false, false), Some(b"\x1b[21~".to_vec()));
        assert_eq!(on("f12", false, false), Some(b"\x1b[24~".to_vec()));
        assert_eq!(on("shift-f1", false, false), Some(b"\x1b[1;2P".to_vec()));
        assert_eq!(on("ctrl-space", false, false), Some(vec![0]));
    }

    #[test]
    fn named_keys_carry_their_modifiers() {
        assert_eq!(on("ctrl-left", false, false), Some(b"\x1b[1;5D".to_vec()));
        // With modifiers, the form is the same in application cursor mode.
        assert_eq!(on("ctrl-right", true, false), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(on("shift-up", false, false), Some(b"\x1b[1;2A".to_vec()));
        assert_eq!(on("alt-left", false, false), Some(b"\x1b[1;3D".to_vec()));
        assert_eq!(on("ctrl-pageup", false, false), Some(b"\x1b[5;5~".to_vec()));
        assert_eq!(on("home", true, false), Some(b"\x1bOH".to_vec()));
        assert_eq!(on("end", false, false), Some(b"\x1b[F".to_vec()));
        // Option+Left/Right on macOS move by word as Terminal.app has them.
        assert_eq!(on("alt-left", false, true), Some(b"\x1bb".to_vec()));
        assert_eq!(on("alt-right", false, true), Some(b"\x1bf".to_vec()));
        assert_eq!(on("ctrl-left", false, true), Some(b"\x1b[1;5D".to_vec()));
    }
}
