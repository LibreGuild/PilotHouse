//! Key bindings written with `secondary-` (⌘ on a Mac, Ctrl elsewhere).
//!
//! GPUI's `KeyBinding::new` takes only explicit modifiers; [`keys`] turns
//! `secondary` into this computer's key, once per binding.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// `binding` with `secondary` as ⌘ (`cmd`) on a Mac and `ctrl` elsewhere,
/// for `KeyBinding::new`. Each distinct binding is made once and kept.
pub fn keys(binding: &'static str) -> &'static str {
    if !binding.contains("secondary") {
        return binding;
    }
    static MADE: LazyLock<Mutex<HashMap<&'static str, &'static str>>> =
        LazyLock::new(Mutex::default);
    let mut made = MADE.lock().unwrap_or_else(|err| err.into_inner());
    made.entry(binding).or_insert_with(|| {
        let key = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        let resolved = binding
            .split(' ')
            .map(|stroke| {
                stroke
                    .split('-')
                    .map(|part| if part == "secondary" { key } else { part })
                    .collect::<Vec<_>>()
                    .join("-")
            })
            .collect::<Vec<_>>()
            .join(" ");
        Box::leak(resolved.into_boxed_str())
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn secondary_is_this_computers_key() {
        let key = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        assert_eq!(super::keys("secondary-shift-k"), format!("{key}-shift-k"));
        assert_eq!(
            super::keys("secondary-k secondary-s"),
            format!("{key}-k {key}-s")
        );
        assert_eq!(super::keys("alt-1"), "alt-1");
        // A "-" key itself stays.
        assert_eq!(super::keys("secondary--"), format!("{key}--"));
    }
}
