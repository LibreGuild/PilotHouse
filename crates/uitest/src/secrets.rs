//! What a test types but does not keep in its file: a password.
//!
//! A step's typed text can be `{{secret:NAME}}`: the value is kept on this
//! computer in the operating system's credential store
//! ([`ph_store::secrets`]), per workspace. A password typed while a test is
//! recorded goes there by itself, so the test runs again with nothing to
//! set up; on another computer the first run says which one is missing and
//! the Tests tab asks for it.
//!
//! `{{env:NAME}}` is read from the shell's environment instead, for a
//! machine nobody sits at. Only variables meant for tests are typed (their
//! names have TEST or E2E in them): a test file from someone else's
//! repository must not type the user's tokens into a page it chose.
//!
//! Everything here reads or writes the credential store: call it off the UI
//! thread.

use anyhow::Result;

/// Why a step's text could not be filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// `{{secret:NAME}}` is not kept on this computer.
    Secret(String),
    /// `{{env:NAME}}` is not set.
    Env(String),
    /// `{{env:NAME}}` names a variable that is not meant for tests.
    NotForTests(String),
}

impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Missing::Secret(name) => write!(
                f,
                "This step types a saved password (“{name}”) that is not on this computer yet: enter it, then run again."
            ),
            Missing::Env(name) => write!(
                f,
                "Set {name} in your shell's environment: this step types it."
            ),
            Missing::NotForTests(name) => write!(
                f,
                "{name} is not typed: only variables meant for tests are (with TEST or E2E in the name, like E2E_PASSWORD)."
            ),
        }
    }
}

fn key(project_id: i64, name: &str) -> String {
    format!("tests:{project_id}:{name}")
}

/// The names kept for a workspace, so that they can all be forgotten.
const INDEX: &str = "names";

fn names(project_id: i64) -> Vec<String> {
    ph_store::secrets::get(&key(project_id, INDEX))
        .ok()
        .flatten()
        .map(|names| names.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The value kept as `name` in workspace `project_id`.
pub fn get(project_id: i64, name: &str) -> Option<String> {
    ph_store::secrets::get(&key(project_id, name))
        .ok()
        .flatten()
}

pub fn set(project_id: i64, name: &str, value: &str) -> Result<()> {
    ph_store::secrets::set(&key(project_id, name), value)?;
    let mut names = names(project_id);
    if !names.iter().any(|known| known == name) {
        names.push(name.to_string());
        ph_store::secrets::set(&key(project_id, INDEX), &names.join("\n"))?;
    }
    Ok(())
}

/// Keeps `value` (a password typed while recording) and gives the name it
/// is kept under: the one it already has, else a new one (`password`,
/// `password-2`…).
pub fn keep(project_id: i64, value: &str) -> Result<String> {
    for n in 1..=50 {
        let name = if n == 1 {
            "password".to_string()
        } else {
            format!("password-{n}")
        };
        match get(project_id, &name) {
            Some(kept) if kept == value => return Ok(name),
            Some(_) => continue,
            None => {
                set(project_id, &name, value)?;
                return Ok(name);
            }
        }
    }
    anyhow::bail!("too many passwords are kept for this workspace's tests")
}

/// Forgets everything kept for a removed workspace.
pub fn forget_workspace(project_id: i64) {
    for name in names(project_id) {
        let _ = ph_store::secrets::delete(&key(project_id, &name));
    }
    let _ = ph_store::secrets::delete(&key(project_id, INDEX));
}

/// `text` with each `{{secret:NAME}}` and `{{env:NAME}}` replaced by its
/// value.
pub fn fill(text: &str, project_id: i64) -> Result<String, Missing> {
    fill_with(
        text,
        |name| get(project_id, name),
        |name| ph_env::var(name).map(str::to_string),
    )
}

fn fill_with(
    text: &str,
    secret: impl Fn(&str) -> Option<String>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, Missing> {
    let mut filled = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start..].find("}}") else {
            break;
        };
        let inner = &rest[start + 2..start + end];
        let value = match inner.split_once(':') {
            Some(("secret", name)) => {
                secret(name).ok_or_else(|| Missing::Secret(name.to_string()))?
            }
            Some(("env", name)) => {
                let upper = name.to_ascii_uppercase();
                if !(upper.contains("TEST") || upper.contains("E2E")) {
                    return Err(Missing::NotForTests(name.to_string()));
                }
                // As the login shell has it, like every tool started here.
                env(name).ok_or_else(|| Missing::Env(name.to_string()))?
            }
            // Braces that are the text's own.
            _ => {
                filled.push_str(&rest[..start + 2]);
                rest = &rest[start + 2..];
                continue;
            }
        };
        filled.push_str(&rest[..start]);
        filled.push_str(&value);
        rest = &rest[start + end + 2..];
    }
    filled.push_str(rest);
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_is_filled_from_what_is_kept() {
        let secret = |name: &str| (name == "password").then(|| "hunter2".to_string());
        let env = |name: &str| (name == "E2E_USER").then(|| "ada".to_string());
        let fill = |text: &str| fill_with(text, secret, env);
        assert_eq!(fill("ada").as_deref(), Ok("ada"));
        assert_eq!(fill("{{secret:password}}").as_deref(), Ok("hunter2"));
        assert_eq!(
            fill("{{env:E2E_USER}}:{{secret:password}}!").as_deref(),
            Ok("ada:hunter2!")
        );
        assert_eq!(
            fill("{{secret:other}}"),
            Err(Missing::Secret("other".into()))
        );
        assert_eq!(
            fill("{{env:E2E_TOKEN}}"),
            Err(Missing::Env("E2E_TOKEN".into()))
        );
        // Other variables (a token, say) are never typed.
        assert_eq!(
            fill("at {{env:PATH}}!"),
            Err(Missing::NotForTests("PATH".into()))
        );
        // Braces of a template the page wants typed stay.
        assert_eq!(
            fill("{{ name }} and {{").as_deref(),
            Ok("{{ name }} and {{")
        );
    }
}
