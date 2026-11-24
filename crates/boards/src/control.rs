//! `board.*` control-socket methods, used by the MCP bridge for agents (the
//! `board_` tools). An agent changes a board by editing its file; these
//! make a board for a workspace, say how a board is written, and what is
//! on one now.
//!
//! The board is the one whose chat the agent is in (`project_id`): a
//! board's own chat works in the board's folder, and that is how it is
//! found. In a workspace's chat it is a board made from that workspace
//! (`board.create`): the one named (`board`), else the one changed last.

use anyhow::{Context as _, Result, bail};
use gpui_kit::{App, AppContext as _, Task};
use ph_artifacts::{Artifact, Artifacts};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::board::{self, BOARD, KIND};
use crate::guide;
use crate::hub::Boards;

pub fn register_control_methods(cx: &mut App) {
    ControlServer::handle(
        "board.guide",
        |_, _| Ok(json!({ "text": guide::text() })),
        cx,
    );
    ControlServer::handle_async("board.look", look, cx);
    ControlServer::handle_async("board.create", create, cx);
    ControlServer::handle("board.list", list, cx);
}

/// Makes a board for the workspace whose chat asks: an artifact of its
/// own, belonging to that workspace, whose chats then work on it. Whoever
/// shows the app's windows is told (`made`), to show the board beside the
/// chat.
fn create(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let prepared = (|| {
        let project = params["project_id"]
            .as_i64()
            .context("missing `project_id`")?;
        let boards = Boards::global(cx).context("this build has no whiteboards")?;
        if let Some(own) = own_board(project, cx) {
            bail!(
                "this chat is the whiteboard “{}”'s own: draw on it (board_guide says how). Another board is made under {} → Artifacts → Whiteboard, or from a workspace's chat.",
                own.manifest.title,
                ph_brand::NAME
            );
        }
        let (workspace, folders) = ph_artifacts::workspace_of(project, cx)
            .context("this chat has no workspace to make a whiteboard for")?;
        let title: String = params["title"]
            .as_str()
            .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|title| !title.is_empty())
            .context("`title` is required: what the board is called (the user can rename it)")?
            .chars()
            .take(120)
            .collect();
        let from = ph_artifacts::library::Source {
            project,
            workspace,
            path: folders.into_iter().next().unwrap_or_default(),
        };
        Ok((boards, project, title, from))
    })();
    let (boards, project, title, from) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => return Task::ready(Err(err)),
    };
    let made = boards.update(cx, |boards, cx| boards.create_for(title, Some(from), cx));
    cx.spawn(async move |cx| {
        let board = made.await?;
        let others = cx.update(|cx| {
            boards.update(cx, |boards, cx| {
                boards.announce(board.id(), json!({ "made": { "project": project } }), cx)
            });
            Boards::of_workspace(project, cx).len().saturating_sub(1)
        });
        let mut text = format!(
            "Made the whiteboard {}. It is one file, {}, that lists what is on the board. You change that file without asking; every change shows to the user at once, beside this chat, and can be undone there. Read board_guide once before you write it, draw, then call board_look to check what you wrote reads as you meant. The user opens the board in its own window (Open in Whiteboard) to change it by hand.",
            listed(&board),
            board.site().join(BOARD).display(),
        );
        if others > 0 {
            text.push_str(&format!(
                " This workspace has {others} other whiteboard{}: board_look shows the one changed last unless `board` names one (board_list shows them).",
                if others == 1 { "" } else { "s" }
            ));
        }
        Ok(json!({ "text": text, "id": board.id() }))
    })
}

/// The boards of the workspace whose chat asks (in a board's own chat:
/// that board).
fn list(params: Value, cx: &mut App) -> Result<Value> {
    let project = params["project_id"]
        .as_i64()
        .context("missing `project_id`")?;
    let boards = match own_board(project, cx) {
        Some(own) => vec![own],
        None => Boards::of_workspace(project, cx),
    };
    if boards.is_empty() {
        return Ok(
            json!({ "text": "This workspace has no whiteboard yet. board_create makes one." }),
        );
    }
    let mut text = String::new();
    for (index, board) in boards.iter().enumerate() {
        text.push_str(&format!(
            "{}. {}, in {}{}\n",
            index + 1,
            listed(board),
            board.site().join(BOARD).display(),
            if index == 0 && boards.len() > 1 {
                " (changed last: board_look shows this one unless `board` names another)"
            } else {
                ""
            },
        ));
    }
    Ok(json!({ "text": text.trim_end() }))
}

/// The board whose own chat `project` is (a board's chat works in the
/// board's folder), if it is one.
fn own_board(project: i64, cx: &App) -> Option<Artifact> {
    let same = |a: &std::path::Path, b: &std::path::Path| {
        a == b
            || std::fs::canonicalize(a)
                .ok()
                .zip(std::fs::canonicalize(b).ok())
                .is_some_and(|(a, b)| a == b)
    };
    let folders = ph_artifacts::folders_of(project, cx);
    Artifacts::global(cx)?
        .read(cx)
        .all()
        .iter()
        .find(|artifact| {
            artifact.manifest.kind == KIND
                && folders.iter().any(|folder| same(folder, &artifact.folder))
        })
        .cloned()
}

/// One line a board of a workspace is listed by.
fn listed(board: &Artifact) -> String {
    format!("“{}” (board `{}`)", board.manifest.title, board.id())
}

/// The board of the chat that asks: the board the chat is beside, or, in a
/// workspace's chat, a board made from that workspace (the one `board`
/// names by its id or its title; without one, the one changed last).
fn find(params: &Value, cx: &App) -> Result<Artifact> {
    let project = params["project_id"]
        .as_i64()
        .context("missing `project_id`")?;
    if let Some(own) = own_board(project, cx) {
        return Ok(own);
    }
    let made = Boards::of_workspace(project, cx);
    let named = params["board"]
        .as_str()
        .map(str::trim)
        .filter(|named| !named.is_empty());
    let Some(named) = named else {
        return made.first().cloned().with_context(|| {
            format!(
                "this workspace has no whiteboard yet. Make one with board_create (a title is enough); board_look then shows it. (A board made under {} → Artifacts → Whiteboard has a chat of its own.)",
                ph_brand::NAME
            )
        });
    };
    let lower = named.to_lowercase();
    made.iter()
        .find(|board| board.id() == named)
        .or_else(|| {
            made.iter()
                .find(|board| board.manifest.title.to_lowercase() == lower)
        })
        .or_else(|| {
            made.iter()
                .find(|board| board.manifest.title.to_lowercase().contains(&lower))
        })
        .cloned()
        .with_context(|| {
            if made.is_empty() {
                "this workspace has no whiteboard yet: make one with board_create".to_string()
            } else {
                format!(
                    "this workspace has no whiteboard “{named}”. Its boards: {}.",
                    made.iter().map(listed).collect::<Vec<_>>().join(", ")
                )
            }
        })
}

/// What is on the board, as its file reads: each thing with its id, place
/// and words; what the user has selected; whether they changed it by hand.
fn look(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let artifact = match find(&params, cx) {
        Ok(artifact) => artifact,
        Err(err) => return Task::ready(Err(err)),
    };
    let mut after = String::new();
    if let Some(boards) = Boards::global(cx) {
        let boards = boards.read(cx);
        match boards.selection(artifact.id()) {
            Some(selection) => {
                let ids: Vec<&str> = selection["ids"]
                    .as_array()
                    .map(|ids| ids.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                if !ids.is_empty() {
                    after.push_str(&format!(
                        "\nSelected by the user: {}. \"This\" in their message means that.",
                        ids.join(", ")
                    ));
                }
            }
            None => after.push_str("\nNothing is selected."),
        }
        if boards.take_by_hand(artifact.id()) > 0 {
            after.push_str(
                "\nThe user changed the board by hand since you last looked: read board.json again before you change it.",
            );
        }
    }
    // The file is read and gone through off the app's own thread.
    cx.background_spawn(async move {
        let site = artifact.site();
        let said = format!(
            "Whiteboard “{}” ({}).\n{}{after}",
            artifact.manifest.title,
            site.join(BOARD).display(),
            board::outline(&board::read(&site))
        );
        Ok(json!({ "text": said }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace's agent makes a board; board_look then finds it from
    /// that workspace's chat (the one changed last, or the one named), and
    /// from no other. A board's own chat has its board and makes no other.
    #[gpui_kit::test]
    fn a_workspace_makes_boards_and_its_chats_find_them(cx: &mut gpui_kit::TestAppContext) {
        use std::rc::Rc;

        let base = dunce::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("ph-board-workspace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (work, kept) = (base.join("work"), base.join("kept"));
        std::fs::create_dir_all(&work).unwrap();
        let folders = vec![work.clone()];
        // Space 40 is a board's own chat: it works in that board's folder,
        // once there is such a board.
        let own: Rc<std::cell::RefCell<Option<std::path::PathBuf>>> = Rc::default();
        let own_folder = own.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
            ph_artifacts::start(kept.clone(), cx);
            ph_artifacts::set_host(
                ph_artifacts::Host {
                    workspace: Rc::new(move |project, _| match project {
                        3 => Some(("shop".to_string(), folders.clone())),
                        40 => own_folder
                            .borrow()
                            .clone()
                            .map(|folder| ("board".to_string(), vec![folder])),
                        _ => None,
                    }),
                    show: Rc::new(|_, _, _| Task::ready(Ok(Vec::new()))),
                    open_in_workspace: Rc::new(|_, _, _, _| {}),
                    trash: |_| Ok(()),
                    renamed: |_, _, _| {},
                    kinds: Vec::new(),
                },
                cx,
            );
            crate::init(cx);
        });
        cx.run_until_parked();
        let run = |cx: &mut gpui_kit::TestAppContext, made: Task<Result<Value>>| {
            cx.run_until_parked();
            cx.foreground_executor().block_test(made)
        };
        let found = |cx: &mut gpui_kit::TestAppContext, params: Value| {
            cx.update(|cx| find(&params, cx).map(|board| board.manifest.title))
        };

        // Nothing yet, and nothing for a chat that is no workspace's.
        let none = found(cx, json!({ "project_id": 3 }))
            .unwrap_err()
            .to_string();
        assert!(none.contains("board_create"), "{none}");
        let nowhere = cx.update(|cx| create(json!({ "project_id": 9, "title": "x" }), cx));
        assert!(run(cx, nowhere).is_err());
        let untitled = cx.update(|cx| create(json!({ "project_id": 3, "title": "  " }), cx));
        assert!(run(cx, untitled).is_err());

        let first =
            cx.update(|cx| create(json!({ "project_id": 3, "title": "Checkout flow" }), cx));
        let first = run(cx, first).unwrap();
        let first_id = first["id"].as_str().unwrap().to_string();
        assert!(
            first["text"].as_str().unwrap().contains("Checkout flow"),
            "{first}"
        );
        let made = cx.update(|cx| Boards::of_workspace(3, cx));
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].manifest.kind, KIND);
        let source = made[0].manifest.source.clone().unwrap();
        assert_eq!((source.project, source.workspace.as_str()), (3, "shop"));
        assert_eq!(source.path, work);
        assert!(made[0].site().join(BOARD).is_file());
        assert_eq!(
            found(cx, json!({ "project_id": 3 })).unwrap(),
            "Checkout flow"
        );
        assert!(
            found(cx, json!({ "project_id": 9 })).is_err(),
            "another chat has none"
        );
        // What is on it, as the agent reads it.
        let looked = cx.update(|cx| look(json!({ "project_id": 3 }), cx));
        let looked = run(cx, looked).unwrap();
        let looked = looked["text"].as_str().unwrap();
        assert!(
            looked.contains("Checkout flow") && looked.contains("empty"),
            "{looked}"
        );

        // A second one: it is the one changed last; either is named.
        let second = cx.update(|cx| create(json!({ "project_id": 3, "title": "Parts" }), cx));
        let second = run(cx, second).unwrap();
        assert!(
            second["text"]
                .as_str()
                .unwrap()
                .contains("1 other whiteboard"),
            "{second}"
        );
        let by = |cx: &mut gpui_kit::TestAppContext, board: &str| {
            found(cx, json!({ "project_id": 3, "board": board }))
        };
        assert_eq!(by(cx, &first_id).unwrap(), "Checkout flow");
        assert_eq!(by(cx, "parts").unwrap(), "Parts");
        assert_eq!(by(cx, "check").unwrap(), "Checkout flow");
        let unknown = by(cx, "budget").unwrap_err().to_string();
        assert!(
            unknown.contains("Checkout flow") && unknown.contains("Parts"),
            "{unknown}"
        );
        let all = cx
            .update(|cx| list(json!({ "project_id": 3 }), cx))
            .unwrap();
        let all = all["text"].as_str().unwrap();
        assert!(
            all.contains("Checkout flow") && all.contains("Parts") && all.contains(&first_id),
            "{all}"
        );

        // A board's own chat: its board, whatever is named, and no new one.
        *own.borrow_mut() = Some(made[0].folder.clone());
        assert_eq!(
            found(cx, json!({ "project_id": 40 })).unwrap(),
            "Checkout flow"
        );
        assert_eq!(
            found(cx, json!({ "project_id": 40, "board": "parts" })).unwrap(),
            "Checkout flow"
        );
        let again = cx.update(|cx| create(json!({ "project_id": 40, "title": "More" }), cx));
        assert!(run(cx, again).unwrap_err().to_string().contains("own"));

        let _ = std::fs::remove_dir_all(&base);
    }
}
