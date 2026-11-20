//! Whether Gemini CLI trusts a folder, as it decides itself.
//!
//! Gemini reads a key saved in its own `.env` (`~/.gemini/.env`) only in a
//! folder it trusts. In any other folder it starts as if nobody had signed
//! in and asks for a login, which reads like a fault of the chat: the same
//! key works in a terminal in another folder. So when Gemini asks for a
//! login, this looks whether that is why, and the chat says so and how to
//! change it. Nothing is changed here: which folders Gemini trusts is the
//! user's to say, to Gemini.
//!
//! The rule is Gemini's (`packages/core/src/utils/trust.ts`, 0.66): its
//! environment switches first, then whether folder trust is on at all
//! (it is unless `security.folderTrust.enabled` is `false`), then the
//! longest rule of `trustedFolders.json` the folder is under. No rule is
//! not trusted. Only names are read from `.env`, never a value.

use std::path::{Path, PathBuf};

/// Gemini's own folder: `GEMINI_CLI_HOME`'s when that is set, else the
/// user's.
fn gemini_dir(home: &Path, var: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    var("GEMINI_CLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.to_path_buf())
        .join(".gemini")
}

/// A path as Gemini compares it: links resolved where it exists, and
/// without case where the file system has none.
fn compared(path: &Path) -> PathBuf {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        PathBuf::from(real.to_string_lossy().to_lowercase())
    } else {
        real
    }
}

/// Whether Gemini trusts `folder`. `None` when that cannot be told (its
/// list is there and cannot be read): nothing is claimed then.
pub fn trusted(folder: &Path, home: &Path, var: &dyn Fn(&str) -> Option<String>) -> Option<bool> {
    let switch = var("GEMINI_CLI_TRUST_WORKSPACE");
    if var("GEMINI_RESTRICTED_MODE").as_deref() == Some("true")
        || switch.as_deref() == Some("false")
    {
        return Some(false);
    }
    if switch.as_deref() == Some("true") {
        return Some(true);
    }
    let dir = gemini_dir(home, var);
    let json = |path: PathBuf| -> Option<Option<serde_json::Value>> {
        match std::fs::read_to_string(path) {
            Ok(text) => Some(serde_json::from_str(&text).ok()),
            Err(_) => None,
        }
    };
    // Folder trust switched off: every folder is trusted.
    if let Some(Some(settings)) = json(dir.join("settings.json"))
        && settings["security"]["folderTrust"]["enabled"] == false
    {
        return Some(true);
    }
    let list = var("GEMINI_CLI_TRUSTED_FOLDERS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("trustedFolders.json"));
    let rules = match json(list) {
        // No list: no folder was ever trusted.
        None => return Some(false),
        Some(None) => return None,
        Some(Some(rules)) => rules,
    };
    let folder = compared(folder);
    let level = rules
        .as_object()?
        .iter()
        .filter_map(|(rule, level)| {
            let level = level.as_str()?;
            let covers = match level {
                "TRUST_PARENT" => Path::new(rule).parent()?.to_path_buf(),
                _ => PathBuf::from(rule),
            };
            folder
                .starts_with(compared(&covers))
                .then_some((rule.len(), level))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, level)| level);
    Some(matches!(level, Some("TRUST_FOLDER" | "TRUST_PARENT")))
}

/// Whether Gemini's own `.env` names a key (the name, not the value).
fn saved_key(home: &Path, var: &dyn Fn(&str) -> Option<String>) -> bool {
    std::fs::read_to_string(gemini_dir(home, var).join(".env")).is_ok_and(|text| {
        text.lines().any(|line| {
            let line = line.trim_start();
            let name = line.strip_prefix("export ").unwrap_or(line).trim_start();
            ["GEMINI_API_KEY", "GOOGLE_API_KEY"].iter().any(|key| {
                name.strip_prefix(key)
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            })
        })
    })
}

/// Whether a key is saved for Gemini that it does not read in `folder`,
/// because it does not trust the folder: why it asks for a login there
/// and not elsewhere.
pub fn keeps_saved_key_from(
    folder: &Path,
    home: &Path,
    var: &dyn Fn(&str) -> Option<String>,
) -> bool {
    trusted(folder, home, var) == Some(false) && saved_key(home, var)
}

/// What the chat says then, in place of "sign in".
pub fn explanation(folder: &Path) -> String {
    format!(
        "Gemini reads the key saved in ~/.gemini/.env only in folders it trusts, and this one is not: {}\n\n\
         To use that key here:\n\
         • Switch to CLI above and answer Gemini's question about the folder, then switch back to Chat.\n\
         • Or set GEMINI_CLI_TRUST_WORKSPACE=true for Gemini CLI in Settings → Agents (it then trusts every folder opened here).\n\n\
         Or sign in another way:",
        folder.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn home(name: &str) -> PathBuf {
        let home =
            std::env::temp_dir().join(format!("ph-gemini-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        home
    }

    #[test]
    fn a_folder_is_trusted_as_gemini_says() {
        let home = home("rules");
        let work = home.join("work");
        for folder in [
            "app",
            "app/src",
            "other",
            "kept/out",
            "parent/child",
            "parent/sibling",
        ] {
            std::fs::create_dir_all(work.join(folder)).unwrap();
        }
        // No list: nothing was ever trusted.
        assert_eq!(trusted(&work.join("app"), &home, &none), Some(false));

        let rule = |folder: &str| work.join(folder).to_string_lossy().to_string();
        std::fs::write(
            home.join(".gemini/trustedFolders.json"),
            serde_json::json!({
                rule("app"): "TRUST_FOLDER",
                rule("kept"): "TRUST_FOLDER",
                rule("kept/out"): "DO_NOT_TRUST",
                rule("parent/child"): "TRUST_PARENT",
            })
            .to_string(),
        )
        .unwrap();
        // The folder and what is under it.
        assert_eq!(trusted(&work.join("app"), &home, &none), Some(true));
        assert_eq!(trusted(&work.join("app/src"), &home, &none), Some(true));
        // No rule covers it.
        assert_eq!(trusted(&work.join("other"), &home, &none), Some(false));
        // The longest rule decides.
        assert_eq!(trusted(&work.join("kept/out"), &home, &none), Some(false));
        // "Trust the parent" covers the folder beside the one it was said in.
        assert_eq!(
            trusted(&work.join("parent/sibling"), &home, &none),
            Some(true)
        );

        // Gemini's switches come first.
        let on = |name: &str| (name == "GEMINI_CLI_TRUST_WORKSPACE").then(|| "true".to_string());
        assert_eq!(trusted(&work.join("other"), &home, &on), Some(true));
        let off = |name: &str| (name == "GEMINI_CLI_TRUST_WORKSPACE").then(|| "false".to_string());
        assert_eq!(trusted(&work.join("app"), &home, &off), Some(false));

        // Folder trust switched off: every folder is trusted.
        std::fs::write(
            home.join(".gemini/settings.json"),
            r#"{ "security": { "folderTrust": { "enabled": false } } }"#,
        )
        .unwrap();
        assert_eq!(trusted(&work.join("other"), &home, &none), Some(true));
        std::fs::remove_file(home.join(".gemini/settings.json")).unwrap();

        // A list that cannot be read: nothing is claimed.
        std::fs::write(home.join(".gemini/trustedFolders.json"), "not json").unwrap();
        assert_eq!(trusted(&work.join("app"), &home, &none), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_saved_key_gemini_does_not_read_here_is_said() {
        let home = home("key");
        let work = home.join("work");
        std::fs::create_dir_all(&work).unwrap();
        // Not trusted, but no key saved: asking for a login is right.
        assert!(!keeps_saved_key_from(&work, &home, &none));
        std::fs::write(home.join(".gemini/.env"), "# mine\nOTHER=1\n").unwrap();
        assert!(!keeps_saved_key_from(&work, &home, &none));
        // A key saved, in a folder Gemini does not trust.
        std::fs::write(
            home.join(".gemini/.env"),
            "OTHER=1\nexport GEMINI_API_KEY = \"x\"\n",
        )
        .unwrap();
        assert!(keeps_saved_key_from(&work, &home, &none));
        // Trusted: Gemini reads it; whatever it asks for is something else.
        std::fs::write(
            home.join(".gemini/trustedFolders.json"),
            serde_json::json!({ work.to_string_lossy().to_string(): "TRUST_FOLDER" }).to_string(),
        )
        .unwrap();
        assert!(!keeps_saved_key_from(&work, &home, &none));
        // A name that only starts like the key's is not the key.
        std::fs::remove_file(home.join(".gemini/trustedFolders.json")).unwrap();
        std::fs::write(home.join(".gemini/.env"), "GEMINI_API_KEY_OLD=x\n").unwrap();
        assert!(!keeps_saved_key_from(&work, &home, &none));
        let _ = std::fs::remove_dir_all(&home);
    }
}
