//! The run configurations added for a workspace (beside those found in its
//! folder): by the user in the Run tab, or by an agent asked to in a chat
//! (`run_add`). Kept in settings (`runs.<workspace>`).
//!
//! An agent adds a configuration; it never starts one. The user does, with
//! the Run tab's button, where the command it runs is shown.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use gpui_kit::{App, AppContext as _, Entity, Global};
use ph_control::ControlServer;
use ph_store::Store;
use serde_json::{Value, json};

use crate::detect::{RunConfig, detect};

/// The longest name a configuration is given: it is a button's label.
const NAME_MAX: usize = 60;

fn key(project_id: i64) -> String {
    format!("runs.{project_id}")
}

/// The configurations added for workspace `project_id`.
pub fn added(store: &Store, project_id: i64) -> Vec<RunConfig> {
    store.setting(&key(project_id))
}

pub(crate) fn save(store: &Store, project_id: i64, configs: &[RunConfig]) -> Result<()> {
    store.set_setting(&key(project_id), &configs)
}

/// Counts the changes made to added configurations from outside a Run tab
/// (by an agent): a tab that shows them watches it, and reads them again
/// when it moves.
struct Changes(Entity<u64>);

impl Global for Changes {}

/// What a Run tab observes to learn that configurations were added or
/// removed elsewhere.
pub(crate) fn changes(cx: &mut App) -> Entity<u64> {
    if let Some(changes) = cx.try_global::<Changes>() {
        return changes.0.clone();
    }
    let changes = cx.new(|_| 0u64);
    cx.set_global(Changes(changes.clone()));
    changes
}

fn changed(cx: &mut App) {
    changes(cx).update(cx, |count, cx| {
        *count += 1;
        cx.notify();
    });
}

/// A configuration as an agent asks for it: a name, a command of one line,
/// and (optionally) the folder it runs in, which must be the project's or
/// one inside it.
fn asked(params: &Value, root: &Path) -> Result<RunConfig> {
    let text = |key: &str| params[key].as_str().unwrap_or_default().trim().to_string();
    let (name, command, folder) = (text("name"), text("command"), text("folder"));
    if name.is_empty() || command.is_empty() {
        bail!("give the configuration a `name` and a `command`");
    }
    if name.chars().count() > NAME_MAX || name.contains('\n') {
        bail!("the name is a button's label: one line of at most {NAME_MAX} characters");
    }
    if command.contains('\n') {
        bail!(
            "the command is one line (join steps with `&&`, or put them in a script of the project)"
        );
    }
    let cwd = if folder.is_empty() || folder == "." {
        None
    } else {
        let path = root.join(&folder);
        // As the disk has them: `..` and links must not lead outside.
        let (inside, whole) = (
            path.canonicalize()
                .with_context(|| format!("there is no folder `{folder}` in the project"))?,
            root.canonicalize()?,
        );
        if !inside.is_dir() || !inside.starts_with(&whole) {
            bail!("`{folder}` is not a folder of the project");
        }
        // Kept under the project's folder as the workspace names it (not
        // as the disk resolves it), so that it reads as a part of it.
        inside
            .strip_prefix(&whole)
            .ok()
            .filter(|under| !under.as_os_str().is_empty())
            .map(|under| root.join(under))
    };
    Ok(RunConfig {
        name,
        command,
        custom: true,
        cwd,
    })
}

/// Registers what agents call: `runs.configs` (those found and those
/// added), `runs.add` and `runs.remove`.
pub(crate) fn register(store: Store, folder_of: Rc<dyn Fn(i64) -> Option<PathBuf>>, cx: &mut App) {
    let project = |params: &Value| {
        params["project_id"]
            .as_i64()
            .context("missing `project_id`")
    };
    let root = {
        let folder_of = folder_of.clone();
        move |project_id: i64| {
            folder_of(project_id).with_context(|| format!("no workspace with id {project_id}"))
        }
    };

    let (list_store, list_root) = (store.clone(), root.clone());
    ControlServer::handle_async(
        "runs.configs",
        move |params, cx| {
            let found = (|| {
                let project_id = project(&params)?;
                Ok((list_root(project_id)?, added(&list_store, project_id)))
            })();
            let (root, added) = match found {
                Ok(found) => found,
                Err(err) => return gpui_kit::Task::ready(Err(err)),
            };
            // The project's files are read off the UI thread.
            cx.background_spawn(async move {
                let configs: Vec<Value> = detect(&root)
                    .into_iter()
                    .chain(added)
                    .map(|config| {
                        json!({
                            "name": config.name, "command": config.command,
                            "added": config.custom, "folder": config.cwd,
                        })
                    })
                    .collect();
                Ok(json!({ "configs": configs }))
            })
        },
        cx,
    );

    let (add_store, add_root) = (store.clone(), root.clone());
    ControlServer::handle(
        "runs.add",
        move |params, cx| {
            let project_id = project(&params)?;
            let config = asked(&params, &add_root(project_id)?)?;
            let mut configs = added(&add_store, project_id);
            // One of the same name is replaced: an agent asked to change a
            // configuration adds it again.
            let replaced = configs.iter().any(|known| known.name == config.name);
            configs.retain(|known| known.name != config.name);
            configs.push(config.clone());
            save(&add_store, project_id, &configs)?;
            changed(cx);
            Ok(json!({ "name": config.name, "command": config.command, "replaced": replaced }))
        },
        cx,
    );

    ControlServer::handle(
        "runs.remove",
        move |params, cx| {
            let project_id = project(&params)?;
            let name = params["name"].as_str().unwrap_or_default().trim();
            let mut configs = added(&store, project_id);
            let before = configs.len();
            configs.retain(|known| known.name != name);
            if configs.len() == before {
                bail!(
                    "No added configuration is named “{name}” (those found in the project are not removed here)."
                );
            }
            save(&store, project_id, &configs)?;
            changed(cx);
            Ok(json!({ "name": name }))
        },
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_asks_for_a_configuration_by_name_and_command() {
        let root = std::env::temp_dir().join(format!("ph-run-configs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("web/src")).unwrap();
        let ask = |params: Value| asked(&params, &root);

        let dev = ask(json!({ "name": " Dev server ", "command": "npm run dev -- --port 3000" }))
            .unwrap();
        assert_eq!(
            (
                dev.name.as_str(),
                dev.command.as_str(),
                dev.custom,
                &dev.cwd
            ),
            ("Dev server", "npm run dev -- --port 3000", true, &None)
        );
        // In a folder of the project, said from the project.
        let web = ask(json!({ "name": "Web", "command": "npm start", "folder": "web" })).unwrap();
        assert_eq!(web.cwd, Some(root.join("web")));
        // However it is said, it is the same folder of the project.
        let said = ask(json!({ "name": "Web", "command": "npm start", "folder": "./web/src/.." }));
        assert_eq!(said.unwrap().cwd, Some(root.join("web")));
        assert_eq!(
            ask(json!({ "name": "Here", "command": "make", "folder": "." }))
                .unwrap()
                .cwd,
            None
        );
        // Never outside it, and never a folder that is not there.
        assert!(ask(json!({ "name": "Out", "command": "ls", "folder": "../.." })).is_err());
        assert!(ask(json!({ "name": "Gone", "command": "ls", "folder": "nope" })).is_err());
        // A name and a command, each of one line.
        assert!(ask(json!({ "name": "", "command": "ls" })).is_err());
        assert!(ask(json!({ "name": "x", "command": "" })).is_err());
        assert!(ask(json!({ "name": "x", "command": "a\nb" })).is_err());
        assert!(ask(json!({ "name": "x".repeat(61), "command": "ls" })).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn added_configurations_are_kept_per_workspace() {
        let store = Store::open_in_memory().unwrap();
        assert!(added(&store, 3).is_empty());
        let config = RunConfig {
            name: "Dev".into(),
            command: "npm run dev".into(),
            custom: true,
            cwd: None,
        };
        save(&store, 3, std::slice::from_ref(&config)).unwrap();
        assert_eq!(added(&store, 3), [config]);
        assert!(added(&store, 4).is_empty());
    }
}
