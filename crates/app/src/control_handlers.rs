//! App-level control socket methods.
//!
//! These expose workspace state and a few workspace commands to local tools.
//! They are always available (the MCP bridge needs them); the `ui.*`
//! automation methods are added only in `automation` builds.

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::App;
use ph_control::ControlServer;
use ph_store::paths;
use ph_workspace::{AppState, Workspace};
use serde_json::{Value, json};

pub fn register(cx: &mut App) {
    ControlServer::handle("app.ping", |_, _| Ok(json!("pong")), cx);
    ControlServer::handle("app.state", |_, cx| app_state(cx), cx);
    ControlServer::handle("project.resolve", resolve_project, cx);
    ControlServer::handle("app.open", open_paths, cx);
    ControlServer::handle("todo.list", list_todos, cx);
    ControlServer::handle("todo.add", add_todo, cx);
    ControlServer::handle("util.list", list_utilities, cx);
    ControlServer::handle("util.run", run_utility, cx);
    ph_board::register_control_methods(AppState::store(cx).clone(), cx);

    // Testing hook: add a project without the native folder picker.
    #[cfg(feature = "automation")]
    ControlServer::handle("app.add_project", add_project, cx);
    #[cfg(feature = "automation")]
    ControlServer::handle("app.add_folder", add_folder, cx);

    #[cfg(feature = "automation")]
    ph_control::automation::register(cx);
}

fn workspace(cx: &App) -> Result<gpui_kit::Entity<Workspace>> {
    ph_workspace::main_workspace(cx)
        .map(|(_, workspace)| workspace)
        .ok_or_else(|| anyhow!("the main window is not open"))
}

/// `{"paths": [...]}`: what a second `pilothouse <path>` launch forwards.
/// Folders open as projects, files in their project or in a window of their
/// own.
fn open_paths(params: Value, cx: &mut App) -> Result<Value> {
    let paths: Vec<std::path::PathBuf> = params
        .get("paths")
        .and_then(Value::as_array)
        .context("missing `paths`")?
        .iter()
        .filter_map(Value::as_str)
        .map(Into::into)
        .collect();
    // Another launch (with folders or without) while every window is closed:
    // on macOS the app keeps running, so the main window comes back.
    if ph_workspace::main_workspace(cx).is_none() {
        let (handle, workspace) = ph_workspace::open_window(None, cx)?;
        ph_workspace::set_main_window(handle, &workspace, cx);
    }
    ph_workspace::open_paths(&paths, cx)?;
    Ok(Value::Null)
}

/// Maps a folder (an agent's working directory) to the workspace holding it.
/// The deepest matching folder wins, so nested workspaces resolve correctly.
fn resolve_project(params: Value, cx: &mut App) -> Result<Value> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing `path`")?;
    let path = paths::canonical(std::path::Path::new(path))
        .unwrap_or_else(|_| std::path::PathBuf::from(path));
    // Any of a workspace's folders (or its scratch folder) counts; the
    // deepest match wins.
    let projects = AppState::store(cx).projects()?;
    let project = projects
        .iter()
        .filter_map(|project| {
            let depth = project
                .roots()
                .iter()
                .filter(|root| path.starts_with(root))
                .map(|root| root.as_os_str().len())
                .max()?;
            Some((depth, project))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, project)| project);
    Ok(match project {
        Some(project) => json!({ "id": project.id, "name": project.name, "path": project.path }),
        None => Value::Null,
    })
}

/// A summary of what the user is looking at.
fn app_state(cx: &mut App) -> Result<Value> {
    let workspace = workspace(cx)?;
    let workspace = workspace.read(cx);
    let space = workspace.active_space().map(|space| space.read(cx));
    Ok(json!({
        "projects": workspace.projects().iter().map(|project| json!({
            "id": project.id,
            "name": project.name,
            "path": project.path,
        })).collect::<Vec<_>>(),
        "active_project": workspace.active_project_id(),
        "chats_space": workspace.chats_space().map(|chats| chats.id),
        "view": match workspace.view() {
            ph_workspace::View::Workspaces => "workspaces",
            ph_workspace::View::GitHub => "github",
        },
        "sidebar_collapsed": workspace.is_sidebar_collapsed(),
        "context_panel_open": space.map(|space| space.is_context_open()),
        "context_tab": space.map(|space| space.context_tab().key()),
        "sessions": space.map(|space| space.sessions().len()),
        "active_session": space.and_then(|space| space.active_session()),
        "dark": gpui_kit::component::ActiveTheme::theme(cx).is_dark(),
    }))
}

#[cfg(feature = "automation")]
fn add_project(params: Value, cx: &mut App) -> Result<Value> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing `path`")?
        .to_string();
    let workspace = workspace(cx)?;
    let window = *cx.windows().first().context("no window")?;
    window
        .update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.add_project_path(std::path::Path::new(&path), window, cx)
            })
        })
        .map_err(|err| anyhow!("{err}"))?;
    app_state(cx)
}

/// Testing hook: add a folder to the workspace in front without the picker.
#[cfg(feature = "automation")]
fn add_folder(params: Value, cx: &mut App) -> Result<Value> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .context("missing `path`")?
        .to_string();
    let workspace = workspace(cx)?;
    let id = workspace
        .read(cx)
        .active_project_id()
        .context("no workspace is open")?;
    let window = *cx.windows().first().context("no window")?;
    window
        .update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.add_folder_path(id, std::path::Path::new(&path), window, cx)
            })
        })
        .map_err(|err| anyhow!("{err}"))?;
    app_state(cx)
}

/// The user's to-dos for a workspace (and the shared ones).
fn list_todos(params: Value, cx: &mut App) -> Result<Value> {
    let project_id = params.get("project_id").and_then(Value::as_i64);
    let todos = AppState::store(cx).todos(project_id)?;
    Ok(json!({
        "todos": todos.iter().map(|todo| json!({
            "id": todo.id, "text": todo.text, "done": todo.done,
            "remind_at": todo.remind_at, "all_workspaces": todo.project_id.is_none(),
        })).collect::<Vec<_>>()
    }))
}

/// Adds a to-do for the user; `remind_in_minutes` sets a reminder.
fn add_todo(params: Value, cx: &mut App) -> Result<Value> {
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .context("missing `text`")?;
    let project_id = params
        .get("project_id")
        .and_then(Value::as_i64)
        .filter(|_| params.get("all_workspaces").and_then(Value::as_bool) != Some(true));
    let remind_at = params
        .get("remind_in_minutes")
        .and_then(Value::as_i64)
        .filter(|minutes| *minutes > 0)
        .map(|minutes| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|now| now.as_millis() as i64)
                .unwrap_or_default()
                + minutes * 60_000
        });
    let todo = AppState::store(cx).add_todo(project_id, text, remind_at)?;
    if let Ok(workspace) = workspace(cx) {
        workspace.update(cx, |workspace, cx| workspace.refresh_inbox(cx));
    }
    Ok(json!({ "id": todo.id, "text": todo.text, "remind_at": todo.remind_at }))
}

/// The offline utilities, with their modes.
fn list_utilities(_: Value, _: &mut App) -> Result<Value> {
    Ok(json!({
        "utilities": ph_utils::UTILITIES.iter().map(|utility| json!({
            "id": utility.id, "name": utility.name, "input": utility.input,
            "second": utility.second, "modes": utility.modes,
        })).collect::<Vec<_>>()
    }))
}

/// Runs a utility on `text` (and `second`), in mode `mode` (an index or a
/// mode's name).
fn run_utility(params: Value, cx: &mut App) -> Result<Value> {
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .context("missing `id`")?;
    let utility = ph_utils::find(id).with_context(|| format!("no utility `{id}`"))?;
    let mode = match params.get("mode") {
        Some(Value::Number(number)) => number.as_u64().unwrap_or(0) as usize,
        Some(Value::String(name)) => utility
            .modes
            .iter()
            .position(|mode| mode.eq_ignore_ascii_case(name))
            .with_context(|| {
                format!(
                    "{} has the modes: {}",
                    utility.name,
                    utility.modes.join(", ")
                )
            })?,
        _ => 0,
    };
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    // An agent (its call names its workspace) may only open SQLite files
    // inside that workspace's folders; the Utilities dialog is the user's.
    if id == "sqlite"
        && let Some(project_id) = params.get("project_id").and_then(Value::as_i64)
    {
        let project = AppState::store(cx)
            .project(project_id)?
            .context("no such workspace")?;
        let file = ph_store::paths::canonical(std::path::Path::new(text("text").trim()))
            .context("no such file")?;
        let inside = project
            .roots()
            .iter()
            .any(|root| ph_store::paths::canonical(root).is_ok_and(|root| file.starts_with(root)));
        if !inside {
            bail!("Only SQLite files inside this workspace's folders can be opened.");
        }
    }
    let output = ph_utils::run(
        id,
        &ph_utils::Input {
            text: text("text"),
            second: text("second"),
            mode,
        },
    )?;
    Ok(json!({ "output": output }))
}
