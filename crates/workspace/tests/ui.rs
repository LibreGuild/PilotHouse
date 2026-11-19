//! UI integration tests for the workspace shell.
//!
//! These render the real `Workspace` in a headless GPUI window, drive it with
//! clicks and key presses, and check what the user would see.

use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};
use ph_store::Store;
use ph_workspace::{AppState, ContextTab, Workspace};

/// Opens a workspace on a fresh in-memory database. The data folder (for
/// scratch folders, reports) is a temporary one, never the user's.
fn open_workspace(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    open_workspace_on(Store::open_in_memory().unwrap(), cx)
}

/// Opens a workspace window on `store`, as a start with what it holds.
fn open_workspace_on(
    store: Store,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    static DATA: std::sync::Once = std::sync::Once::new();
    DATA.call_once(|| {
        let dir = std::env::temp_dir().join(format!("ph-ui-data-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: set once, before any test reads it, and never changed.
        unsafe { std::env::set_var("PILOTHOUSE_DATA_DIR", &dir) };
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        ph_theme::init(cx);
        AppState::install(store, cx);
        ph_workspace::init(cx);
        // Real file watchers run on their own threads, which the test
        // scheduler cannot follow.
        ph_files::WatcherHub::turn_off(cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1280.), px(800.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap()
    })
}

fn temp_project(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ph-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    dir
}

/// A first start opens straight into a chat, in the workspace for chats
/// that need no folder ("Scratch", made for it: a workspace like any
/// other); the Bridge lists it; a folder added becomes a workspace with a
/// badge in the column of icons, and Files as its first tool.
#[gpui_kit::test]
fn a_first_start_opens_into_a_chat(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let scratch = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let workspace = workspace.read(cx);
            let [scratch] = workspace.projects() else {
                panic!("one workspace, made for the first chat");
            };
            assert_eq!(scratch.name, "Scratch");
            assert!(scratch.folders().is_empty());
            assert_eq!(workspace.active_project_id(), Some(scratch.id));
            assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
            assert!(window.try_find("bridge").is_none(), "the chat is in front");
            assert!(
                window
                    .try_find(gpui_kit::ElementId::from((
                        "rail-workspace",
                        scratch.id as u64
                    )))
                    .is_some(),
                "it has a badge, as every workspace"
            );
            scratch.id
        })
        .unwrap();

    // The Bridge, and back to the chat by the key for chats that need no
    // folder.
    click(window, "view-bridge", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).view(), ph_workspace::View::Bridge);
        assert!(window.try_find("bridge").is_some());
    })
    .unwrap();
    press(window, "secondary-shift-d", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
        assert_eq!(workspace.active_project_id(), Some(scratch));
        assert!(window.try_find("bridge").is_none());
    })
    .unwrap();

    let folder = temp_project("first");
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        window.render_frame(cx);
        // Newest first: the folder's workspace, before Scratch.
        let id = workspace.read(cx).projects()[0].id;
        assert_ne!(id, scratch);
        assert!(
            window
                .try_find(gpui_kit::ElementId::from(("rail-workspace", id as u64)))
                .is_some(),
            "its badge is in the column of icons"
        );
        // The result pane opens on Files (the tree and the editor), with
        // the review of the changes one name away.
        assert!(window.try_find("context-tab-files").is_some());
        assert!(window.try_find("context-tab-changes").is_some());
        assert!(window.try_find("files-add-folder").is_some());
    })
    .unwrap();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.projects().len(), 2);
        let space = workspace.active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Files);
    });
}

/// A tool that is not one of the four names shows beside them once it is
/// in front; ⌥⌘B hides the result pane and brings it back as it was.
/// Someone who has workspaces meets the Bridge when the window opens, the
/// workspace shown last ready behind it. The keyboard is the window's from
/// the start (no click needed before a shortcut works), and opening on the
/// Bridge does not make the next start open on the workspace.
#[gpui_kit::test]
fn a_window_with_workspaces_opens_on_the_bridge(cx: &mut TestAppContext) {
    let store = Store::open_in_memory().unwrap();
    let folder = temp_project("start");
    let project = store.add_project(&folder).unwrap().id;
    // As a run before left it: that workspace was the one shown last.
    store
        .set_setting("workspace.last_project", &Some(project))
        .unwrap();
    let (window, workspace) = open_workspace_on(store, cx);
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Bridge);
        assert_eq!(workspace.active_project_id(), Some(project));
        assert!(window.try_find("bridge").is_some());
        let front: String = AppState::store(cx).setting("workspace.front_view");
        assert_ne!(
            front, "workspace",
            "the Bridge was in front, and is next time"
        );
    })
    .unwrap();
    press(
        window,
        if cfg!(target_os = "macos") {
            "ctrl-1"
        } else {
            "alt-1"
        },
        cx,
    );
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
        let front: String = AppState::store(cx).setting("workspace.front_view");
        assert_eq!(front, "workspace");
    });
}

#[gpui_kit::test]
fn switching_tabs_and_hiding_the_panel(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("tabs");
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        window.render_frame(cx);
        assert!(window.try_find("context-tab-board").is_none(), "under More");
        window.dispatch_action(Box::new(ph_workspace::ShowBoard), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("context-tab-board").is_some());
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Board);
        assert!(space.is_context_open());
    })
    .unwrap();

    // ⌥⌘B hides the result pane; the choice is remembered for the
    // workspace.
    cx.update_window(window, |_, window, cx| window.press("secondary-alt-b", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert!(!space.is_context_open());
        let project = space.project().id;
        let saved = AppState::store(cx).project_ui_state(project).unwrap();
        assert_eq!(saved.context_panel_open, Some(false));
        assert_eq!(saved.context_tab.as_deref(), Some("board"));
    });

    // (⌘J, the terminal, is left out: its first terminal starts a real
    // shell, whose reader thread makes the test scheduler fail.)

    // ⌥⌘B brings the pane back as it was: the board in front.
    cx.update_window(window, |_, window, cx| window.press("secondary-alt-b", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Board);
        assert!(space.is_context_open());
    });
}

/// ⌘K opens the palette; typing filters it and Enter runs the command.
/// (Regression test: opening it from the shortcut used to panic.)
#[gpui_kit::test]
fn command_palette_runs_a_command(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("palette");
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        window.render_frame(cx);
        window.press("secondary-k", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find_by_label("Type a command or search…")
                .is_some()
        );
        window.input("hide the chat", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert!(space.is_chat_folded(), "the result pane has the screen");
    });
}

/// A file opens in the editor, which is the result pane's Files beside the
/// chat; ⌘B folds the chat away so the editor has the screen, and anything
/// sent to the chat brings it back.
#[gpui_kit::test]
fn files_open_beside_the_chat(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("editor");
    // Plain text: no language server starts (its threads would make the
    // test scheduler non-deterministic).
    std::fs::write(folder.join("notes.txt"), "hello\n").unwrap();
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        // Another tool in front, so the file has to bring Files forward.
        window.dispatch_action(Box::new(ph_workspace::ShowBoard), cx);
        window.render_frame(cx);
        assert!(
            window.try_find_by_label("Close file").is_none(),
            "no editor yet"
        );
        assert!(
            window.try_find("chat-history").is_some(),
            "the chat lists its workspace's chats itself"
        );
        let space = workspace.read(cx).active_space().unwrap().clone();
        space.update(cx, |space, cx| {
            space.open_file("notes.txt", None, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find_by_label("Close file").is_some(),
            "the editor is in front of the result pane"
        );
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Files);
        assert!(window.try_find("unfold-chat").is_none());
        window.press("secondary-b", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("unfold-chat").is_some(),
            "⌘B folded the chat to a strip"
        );
        assert!(
            window.try_find("view-bridge").is_some(),
            "the column of icons stays"
        );
        let space = workspace.read(cx).active_space().unwrap().clone();
        space.update(cx, |space, cx| space.show_chat(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("unfold-chat").is_none(), "the chat is back");
    })
    .unwrap();
}

/// A workspace without folders still has its chat and tools; Git says a
/// folder is needed and offers to add one.
#[gpui_kit::test]
fn a_workspace_without_folders_explains_git(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(ph_workspace::NewWorkspace), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let project = workspace
            .read(cx)
            .active_space()
            .unwrap()
            .read(cx)
            .project()
            .clone();
        assert!(project.empty && project.folders().is_empty());
        let scratch = ph_store::paths::canonical(&ph_store::paths::scratch_root()).unwrap();
        assert!(
            project.path.starts_with(scratch),
            "agents work in a scratch folder"
        );
    });
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(ph_workspace::ShowChanges), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("empty-add-folder").is_some());
    })
    .unwrap();
}

/// A second folder (another repository) shows at the top of Files beside
/// the first, and the Git tab can switch between them.
#[gpui_kit::test]
fn a_second_folder_joins_the_workspace(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (client, server) = (temp_project("client"), temp_project("server"));
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&client, window, cx);
            let id = workspace.active_project_id().unwrap();
            workspace.add_folder_path(id, &server, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        let names: Vec<String> = space
            .roots()
            .list()
            .iter()
            .map(|root| root.name.clone())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names[0].starts_with("ph-ui-client") && names[1].starts_with("ph-ui-server"));
    });
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let name = server.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            window.try_find_by_label(&name).is_some(),
            "the second folder is listed in Files"
        );
    })
    .unwrap();
}

/// A reminder that came due shows in the inbox, and Done clears it.
#[gpui_kit::test]
fn a_due_reminder_shows_in_the_inbox(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("inbox");
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx);
            let id = workspace.active_project_id();
            AppState::store(cx)
                .add_todo(id, "Call the team", Some(1))
                .unwrap();
            workspace.refresh_inbox(cx);
        });
        window.render_frame(cx);
        let bell = window.find("inbox").bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, bell, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, bell, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let done = window.find_by_label("Done").bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, done, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, done, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let store = AppState::store(cx);
        assert!(store.due_todos(i64::MAX).unwrap().is_empty(), "done");
    });
}

/// Adds two workspaces, `alpha` then `beta` (which ends up in front), in
/// folders named after `test` (tests run at the same time).
fn two_workspaces(
    test: &str,
    window: AnyWindowHandle,
    workspace: &Entity<Workspace>,
    cx: &mut TestAppContext,
) -> (ph_store::projects::ProjectId, ph_store::projects::ProjectId) {
    let alpha = temp_project(&format!("{test}-alpha"));
    let beta = temp_project(&format!("{test}-beta"));
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&alpha, window, cx);
            workspace.add_project_path(&beta, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        let id = |name: &str| {
            workspace
                .projects()
                .iter()
                .find(|project| project.name.contains(name))
                .unwrap()
                .id
        };
        (id("alpha"), id("beta"))
    })
}

fn click(window: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let at = window.find(id).bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, at, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click_id(window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    let id = id.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let at = window.find(id).bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, at, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

/// Every workspace has a badge in the column of icons, newest first: a
/// click opens one. The Bridge lists every workspace's tasks, and a card
/// opens its workspace at that chat (whichever was in front).
#[gpui_kit::test]
fn badges_open_workspaces_and_cards_open_their_chats(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, beta) = two_workspaces("sidebar", window, &workspace, cx);
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(beta));
        let order: Vec<_> = workspace.projects().iter().map(|p| p.id).collect();
        // (After them, the workspace the first start made for chats.)
        assert_eq!(order[..2], [beta, alpha], "newest first, and it stays so");
    });

    // ⌃1 (Alt+1 off macOS) is the first badge: beta.
    press(
        window,
        if cfg!(target_os = "macos") {
            "ctrl-1"
        } else {
            "alt-1"
        },
        cx,
    );
    cx.update(|cx| assert_eq!(workspace.read(cx).active_project_id(), Some(beta)));

    click_id(window, ("rail-workspace", alpha as u64), cx);
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(alpha));
        assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
        assert_eq!(
            workspace.projects()[1].id,
            alpha,
            "opening does not reorder"
        );
    });

    // A chat of beta's, made while alpha is in front.
    let session = cx.update(|cx| {
        AppState::store(cx)
            .create_session(beta, "Plan the release", "claude")
            .unwrap()
            .id
    });
    // Beta's screen closes (hidden, idle): the Bridge reads its chats from
    // the store.
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    cx.update(|cx| assert!(!workspace.read(cx).has_space(beta)));
    press(window, "secondary-shift-e", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).view(), ph_workspace::View::Bridge);
        assert!(
            window.try_find("bridge-column-review").is_some(),
            "a task nobody marked done waits for a look"
        );
    })
    .unwrap();
    click_id(
        window,
        gpui_kit::SharedString::from(format!("bridge-card-{session}")),
        cx,
    );
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
        assert_eq!(workspace.active_project_id(), Some(beta));
        let space = workspace.active_space().unwrap().read(cx);
        assert_eq!(space.active_session(), Some(session));
    });
}

/// With the chat folded away its composer is not drawn; the keyboard must
/// not stay there, or no shortcut would reach the window (⌘B could not
/// bring the chat back). A chat asked for unfolds it.
#[gpui_kit::test]
fn keys_work_with_the_chat_folded(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("folded");
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        // A tool with no text to take the keyboard, and the chat typed in.
        window.dispatch_action(Box::new(ph_workspace::ShowBoard), cx);
        let space = workspace.read(cx).active_space().unwrap().clone();
        space.update(cx, |space, cx| space.focus_chat(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let folded = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            workspace
                .read(cx)
                .active_space()
                .unwrap()
                .read(cx)
                .is_chat_folded()
        })
    };
    press(window, "secondary-b", cx);
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    assert!(folded(cx));
    press(window, "secondary-b", cx);
    assert!(
        !folded(cx),
        "⌘B reached the window and brought the chat back"
    );

    // Folded again, then a new chat is asked for: it shows.
    press(window, "secondary-b", cx);
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    assert!(folded(cx));
    press(window, "secondary-n", cx);
    assert!(!folded(cx), "a new chat is not started out of sight");
}

/// Removing the workspace in front lands on the Bridge with another place
/// ready behind it, so a new chat and the other commands have somewhere to
/// go.
#[gpui_kit::test]
fn removing_the_workspace_in_front_leaves_one_behind(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, beta) = two_workspaces("removed", window, &workspace, cx);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_project(alpha, window, cx);
            workspace.open_project(beta, window, cx);
            workspace.remove_project(beta, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Bridge);
        assert_eq!(workspace.active_project_id(), Some(alpha));
    });
    press(window, "secondary-n", cx);
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
        assert_eq!(workspace.active_project_id(), Some(alpha));
    });
}

/// Someone who comes from a version without the Bridge finds their last
/// week of chats waiting for a look: the column's menu puts them away at
/// once.
#[gpui_kit::test]
fn what_waits_for_a_look_is_put_away_at_once(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("settle", window, &workspace, cx);
    let sessions = cx.update(|cx| {
        let store = AppState::store(cx);
        ["Sort the arrivals", "Name the berths"]
            .map(|title| store.create_session(alpha, title, "claude").unwrap().id)
    });
    // Alpha's screen closes (hidden, idle): the Bridge reads its chats from
    // the store.
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    press(window, "secondary-shift-e", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        for session in sessions {
            assert!(
                window
                    .try_find(gpui_kit::SharedString::from(format!(
                        "bridge-done-{session}"
                    )))
                    .is_some(),
                "each waits for a look, with Done on its card"
            );
        }
    })
    .unwrap();
    click(window, "bridge-review-menu", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let at = window.find_by_label("Mark all 2 as done").bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, at, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let done: Vec<i64> = AppState::store(cx).setting("workspace.sidebar_settled");
        for session in sessions {
            assert!(done.contains(&session), "kept as done");
            assert!(
                window
                    .try_find(gpui_kit::SharedString::from(format!(
                        "bridge-done-{session}"
                    )))
                    .is_none(),
                "and out of the column"
            );
        }
        assert!(window.try_find("bridge-review-menu").is_none());
    })
    .unwrap();
}

/// Several chats of one workspace are a click apart: each one still open
/// has a tab in the chat's header, and one marked done leaves the row (it
/// is under *Done* on the Bridge).
#[gpui_kit::test]
fn open_chats_are_tabs_over_the_chat(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("tabs", window, &workspace, cx);
    let [first, second] = cx.update(|cx| {
        let store = AppState::store(cx);
        ["Sort the arrivals", "Name the berths"]
            .map(|title| store.create_session(alpha, title, "claude").unwrap().id)
    });
    // Alpha's screen is made anew, with the chats now stored.
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_chat(alpha, first, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let front = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let space = workspace.read(cx).active_space().unwrap().clone();
            space.read(cx).active_session()
        })
    };
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        for session in [first, second] {
            assert!(
                window.try_find(("chat-tab", session as u64)).is_some(),
                "each open chat has a tab"
            );
        }
    })
    .unwrap();
    assert_eq!(front(cx), Some(first));
    click_id(window, ("chat-tab", second as u64), cx);
    assert_eq!(front(cx), Some(second), "a tab shows its chat");

    // The first is marked done from its tab, which shows what to press
    // while the pointer is on it.
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        gpui_kit::test::TestWindowExt::hover(window, ("chat-tab", first as u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    click_id(window, ("chat-tab-done", first as u64), cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let done: Vec<i64> = AppState::store(cx).setting("workspace.sidebar_settled");
        assert_eq!(done, vec![first], "kept as done");
        assert!(
            window.try_find(("chat-tab", first as u64)).is_none(),
            "it left the row"
        );
        assert!(window.try_find(("chat-tab", second as u64)).is_some());
    })
    .unwrap();
    assert_eq!(front(cx), Some(second), "the chat in front stays");

    // On the Bridge it is under Done, its card without the button.
    press(window, "secondary-shift-e", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(gpui_kit::SharedString::from(format!("bridge-card-{first}")))
                .is_some()
        );
        assert!(
            window
                .try_find(gpui_kit::SharedString::from(format!("bridge-done-{first}")))
                .is_none()
        );
    })
    .unwrap();
}

/// The Git tool lists the changed files and shows their changes in one
/// diff beside (or under) the list: a row has a tick for "looked at" where
/// the arrow that unfolded the file in the list was.
#[gpui_kit::test]
fn changes_are_reviewed_beside_their_list(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("review");
    let git = |args: &[&str]| {
        let done = std::process::Command::new("git")
            .args(args)
            .current_dir(&folder)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(done.status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-q",
        "-m",
        "Start",
    ]);
    std::fs::write(
        folder.join("src/main.rs"),
        "fn main() {\n    println!(\"hi\");\n}\n",
    )
    .unwrap();
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        window.dispatch_action(Box::new(ph_workspace::ShowChanges), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("change-file-src/main.rs").is_some());
        assert!(
            window.try_find("fold-file-src/main.rs").is_none(),
            "no arrow unfolds the file in the list"
        );
    })
    .unwrap();
    // The diff is drawn once the pane's width is known and the patch read.
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("git-review").is_some(), "the changes show");
        assert!(window.try_find("review-keep-src/main.rs").is_some());
    })
    .unwrap();
    click(window, "viewed-src/main.rs", cx);
    cx.update_window(window, |_, window, cx| {
        let space = workspace.read(cx).active_space().unwrap().clone();
        let changes = space.update(cx, |space, cx| space.changes(window, cx));
        assert_eq!(
            changes.read(cx).viewed_count(),
            (1, 1),
            "the tick is counted"
        );
    })
    .unwrap();
}

/// Chats that need no folder (the quick chat's) always have their place,
/// the Scratch workspace, and are found there: it is not removed, takes no
/// folder, and a chat made away from the window (as the quick chat makes
/// one) opens as itself, not as a new, empty chat.
#[gpui_kit::test]
fn standalone_chats_keep_their_place(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let scratch = cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.projects().len(), 1);
        workspace.projects()[0].id
    });
    // Asked to remove it: it stays, with a word why.
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.remove_project(scratch, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.projects().len(), 1, "Scratch is still there");
        assert!(AppState::store(cx).project(scratch).unwrap().is_some());
    });
    // A folder added while it is in front opens as a workspace of its own.
    let folder = temp_project("standalone");
    cx.update(|cx| {
        assert!(
            AppState::store(cx)
                .add_workspace_folder(scratch, &folder)
                .is_err(),
            "it takes no folder"
        );
    });

    // The quick chat makes a chat there while the window has Scratch's
    // screen open, and asks the window to show it.
    let session = cx.update(|cx| {
        AppState::store(cx)
            .create_session(scratch, "Asked from the quick window", "claude")
            .unwrap()
            .id
    });
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_chat(scratch, session, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(scratch));
        let space = workspace.active_space().unwrap().read(cx);
        assert_eq!(
            space.active_session(),
            Some(session),
            "that chat is in front"
        );
        assert!(
            space.sessions().iter().any(|known| known.id == session),
            "and among the workspace's chats"
        );
    });

    // A workspace made away from the window (another window, an earlier
    // quick chat) is read when a chat of it is asked for.
    let (other, chat) = cx.update(|cx| {
        let store = AppState::store(cx);
        let other = store
            .create_empty_workspace("Notes", &temp_project("standalone-notes"))
            .unwrap()
            .id;
        let chat = store.create_session(other, "A note", "claude").unwrap().id;
        (other, chat)
    });
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_chat(other, chat, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(other));
        let space = workspace.active_space().unwrap().read(cx);
        assert_eq!(space.active_session(), Some(chat));
    });
}

/// Removing a workspace removes its chats: with any, it says so and asks
/// first. (It went at once before, chats and all.)
#[gpui_kit::test]
fn removing_a_workspace_with_chats_asks_first(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("remove-asks", window, &workspace, cx);
    cx.update(|cx| {
        AppState::store(cx)
            .create_session(alpha, "Sort the arrivals", "claude")
            .unwrap();
    });
    // Alpha in front, its screen made anew with the chat now stored.
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    click_id(window, ("rail-workspace", alpha as u64), cx);
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(ph_workspace::RemoveWorkspace), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("dialog-ok").is_some(), "it asks");
        assert!(
            workspace
                .read(cx)
                .projects()
                .iter()
                .any(|project| project.id == alpha),
            "and nothing went yet"
        );
    })
    .unwrap();
    click(window, "dialog-ok", cx);
    cx.update(|cx| {
        assert!(
            !workspace
                .read(cx)
                .projects()
                .iter()
                .any(|project| project.id == alpha)
        );
    });
}

/// Settings opens at the page it is asked for (the pages are found by
/// their place in the list, which changes when one is moved), and the
/// Chrome extension's page is its guide: the steps with what each needs,
/// drawn without a hitch.
#[gpui_kit::test]
fn settings_open_at_the_page_asked_for(cx: &mut TestAppContext) {
    let (window, _workspace) = open_workspace(cx);
    let shows = |id: &'static str, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    };
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(ph_browser::OpenChromeExtensionSettings), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(shows("chrome-extension-set-up", cx), "the first step");
    assert!(shows("chrome-extension-copy-address", cx), "the second");
    assert!(shows("chrome-extension-copy", cx), "the third");
    assert!(
        !shows("chrome-extension-steps", cx),
        "no Chrome, so no fold"
    );

    cx.update_window(window, |_, window, cx| {
        use gpui_kit::component::WindowExt as _;
        window.close_dialog(cx);
        window.dispatch_action(Box::new(ph_workspace::OpenGitHubSettings), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!shows("chrome-extension-set-up", cx));
    assert!(shows("add-account", cx), "GitHub's page");
}

/// Reviews is one click away; without an account it asks for one. The
/// Bridge and the chats that need no folder each have their key.
#[gpui_kit::test]
fn reviews_ask_for_an_account(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    click(window, "view-github", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).view(), ph_workspace::View::GitHub);
        assert!(window.try_find("github-open-settings").is_some());
        assert!(window.try_find("github-sidebar").is_some());
    })
    .unwrap();
    press(window, "secondary-shift-e", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).view(), ph_workspace::View::Bridge);
        assert!(window.try_find("bridge").is_some());
        assert!(window.try_find("github-sidebar").is_none());
    })
    .unwrap();
    press(window, "secondary-shift-d", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
        let scratch = workspace
            .projects()
            .iter()
            .find(|project| project.name == "Scratch")
            .map(|project| project.id);
        assert!(scratch.is_some());
        assert_eq!(workspace.active_project_id(), scratch);
    })
    .unwrap();
}

/// A workspace moved to a window of its own leaves this one, which shows
/// the other workspace.
#[gpui_kit::test]
fn a_workspace_moves_to_a_window_of_its_own(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, beta) = two_workspaces("new-window", window, &workspace, cx);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_in_new_window(beta, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(cx.windows().len(), 2);
        let workspace = workspace.read(cx);
        assert!(!workspace.has_space(beta));
        assert_eq!(workspace.active_project_id(), Some(alpha));
    });
}

/// Hidden, idle workspaces close their screens; one with a message being
/// written keeps it, and a closed one comes back when opened.
#[gpui_kit::test]
fn idle_hidden_workspaces_close_their_screens(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, beta) = two_workspaces("unload", window, &workspace, cx);
    let unload = |cx: &mut TestAppContext| {
        workspace.update(cx, |workspace, cx| {
            workspace.unload_idle(std::time::Duration::ZERO, cx)
        });
    };

    // Beta has a draft, then alpha comes to the front.
    cx.update_window(window, |_, window, cx| {
        let space = workspace.read(cx).active_space().unwrap().clone();
        space.update(cx, |space, cx| space.focus_chat(window, cx));
        window.input("half a thought", cx);
        workspace.update(cx, |workspace, cx| {
            workspace.open_project(alpha, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    unload(cx);
    cx.update(|cx| assert!(workspace.read(cx).has_space(beta), "the draft keeps it"));

    // Alpha, idle and hidden, closes; opening it again rebuilds it.
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.open_project(beta, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    unload(cx);
    cx.update(|cx| assert!(!workspace.read(cx).has_space(alpha)));
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_project(alpha, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert!(workspace.has_space(alpha));
        assert_eq!(workspace.active_project_id(), Some(alpha));
    });
}

/// ⌘V with an image on the clipboard (a screenshot) attaches it to the
/// next message instead of pasting nothing.
#[gpui_kit::test]
fn a_pasted_image_becomes_an_attachment(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let png = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, vec![0x89, b'P', b'N', b'G']);
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(&png));
    cx.update_window(window, |_, window, cx| {
        let space = workspace.read(cx).active_space().unwrap().clone();
        space.update(cx, |space, cx| space.focus_chat(window, cx));
        window.press("secondary-v", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("Remove attachment").is_some());
    })
    .unwrap();
}

/// Rendered Markdown, as the preview and the chat show it.
struct Document;

impl gpui_kit::Render for Document {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::base::text::{SelectionFormat, TextView};
        use gpui_kit::{InteractiveElement as _, ParentElement as _, Styled as _, div};
        let text = "# Notes\n\nSome **bold** text.\n\n| Name | Count |\n|---|---|\n| a | 1 |\n";
        ph_ui::rich_copy::on(
            div().id("document").size_full().child(
                TextView::markdown("document-text", text)
                    .selectable(true)
                    .selection_format(SelectionFormat::Source),
            ),
        )
    }
}

/// Copying from rendered Markdown puts the text as it reads on the
/// clipboard, a table's cells apart by tabs (the HTML form goes to the
/// system's clipboard, which a test leaves alone).
#[gpui_kit::test]
fn rendered_markdown_copies_as_the_text_read(cx: &mut TestAppContext) {
    let (window, _) = cx
        .update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Document))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        // The document fills the window: its heading is at the top left.
        let at = gpui_kit::point(px(12.), px(12.));
        gpui_kit::test::TestEventExt::pointer_down(window, at, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    press(window, "secondary-a", cx);
    press(window, "secondary-c", cx);
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert_eq!(copied, "Notes\n\nSome bold text.\n\nName\tCount\na\t1");
}
