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
    static DATA: std::sync::Once = std::sync::Once::new();
    DATA.call_once(|| {
        let dir = std::env::temp_dir().join(format!("ph-ui-data-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: set once, before any test reads it, and never changed.
        unsafe { std::env::set_var("PILOTHOUSE_DATA_DIR", &dir) };
    });
    cx.update(|cx| {
        gpui_kit::init(cx);
        ph_ui::init(cx);
        ph_theme::init(cx);
        AppState::install(Store::open_in_memory().unwrap(), cx);
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

/// A first start opens straight into a chat (the Chats space, no folder
/// needed); a folder added becomes a workspace in the sidebar, with Files
/// as its first tool.
#[gpui_kit::test]
fn a_first_start_opens_into_a_chat(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let workspace = workspace.read(cx);
        let chats = workspace.chats_space().expect("the Chats space").id;
        assert_eq!(workspace.active_project_id(), Some(chats));
        assert!(workspace.projects().is_empty(), "it is not a workspace");
        assert!(window.try_find("sidebar-chats").is_some());
        assert!(window.try_find("sidebar-workspaces").is_some());
        assert!(window.try_find("sidebar-open-folder").is_some());
        // Nothing to show of files there.
        assert!(window.try_find("context-tab-files").is_none());
    })
    .unwrap();

    let folder = temp_project("first");
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        window.render_frame(cx);
        assert!(window.try_find("sidebar-open-folder").is_none());
        // The tools open on Files by default, Git beside it.
        assert!(window.try_find("context-tab-files").is_some());
        assert!(window.try_find("context-tab-changes").is_some());
        assert!(window.try_find("files-add-folder").is_some());
    })
    .unwrap();
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.projects().len(), 1);
        let space = workspace.active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Files);
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

        let board = window.find_by_label("Board").bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, board, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, board, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Board);
        assert!(space.is_context_open());
    });

    // ⌘J hides the panel; the choice is remembered for the project.
    cx.update_window(window, |_, window, cx| window.press("secondary-j", cx))
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
        window.input("toggle sidebar", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(workspace.read(cx).is_sidebar_collapsed()));
}

/// A file opens in the editor, which shares the chat's tab group and comes
/// to the front (the Editor tab shows only while a file is open); the
/// sidebar lists the chats, and hiding the sidebar moves the chat list
/// into the chat's history menu.
#[gpui_kit::test]
fn files_open_in_front_of_the_chat(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("editor");
    // Plain text: no language server starts (its threads would make the
    // test scheduler non-deterministic).
    std::fs::write(folder.join("notes.txt"), "hello\n").unwrap();
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        window.render_frame(cx);
        assert!(window.try_find("sidebar-chats").is_some());
        assert!(
            window.try_find_by_label("Close file").is_none(),
            "no editor yet"
        );
        assert!(window.try_find("chat-history").is_none());
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
            "the editor is in front"
        );
        assert!(window.try_find("chat-history").is_none());
        window.press("secondary-b", cx);
        let space = workspace.read(cx).active_space().unwrap().clone();
        space.update(cx, |space, cx| space.show_chat(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("sidebar-chats").is_none(),
            "⌘B hid the sidebar"
        );
        assert!(
            window.try_find("chat-history").is_some(),
            "the chat offers its history instead"
        );
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

/// Every workspace is in the sidebar, newest first: a click opens one,
/// an unfolded one lists its chats, and a chat's row opens it at that chat
/// (whichever workspace is in front).
#[gpui_kit::test]
fn the_sidebar_opens_workspaces_and_their_chats(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, beta) = two_workspaces("sidebar", window, &workspace, cx);
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(beta));
        let order: Vec<_> = workspace.projects().iter().map(|p| p.id).collect();
        assert_eq!(order, vec![beta, alpha], "newest first, and it stays so");
    });

    // ⌃1 (Alt+1 off macOS) is the first row: beta.
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

    click_id(window, ("sidebar-workspace", alpha as u64), cx);
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(alpha));
        assert_eq!(
            workspace.projects()[1].id,
            alpha,
            "opening does not reorder"
        );
    });

    // A chat of beta's, listed while alpha is in front.
    let session = cx.update(|cx| {
        AppState::store(cx)
            .create_session(beta, "Plan the release", "claude")
            .unwrap()
            .id
    });
    // Beta's screen closes (hidden, idle): the sidebar reads its chats.
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    cx.update(|cx| assert!(!workspace.read(cx).has_space(beta)));
    click_id(
        window,
        gpui_kit::SharedString::from(format!("chat-{session}")),
        cx,
    );
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(beta));
        let space = workspace.active_space().unwrap().read(cx);
        assert_eq!(space.active_session(), Some(session));
    });
}

/// The GitHub view is one click away; without an account it asks for one,
/// and the workspace's sidebar gives way to its own.
#[gpui_kit::test]
fn the_github_view_asks_for_an_account(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    click(window, "view-github", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).view(), ph_workspace::View::GitHub);
        assert!(window.try_find("github-open-settings").is_some());
        assert!(window.try_find("github-sidebar").is_some());
        assert!(window.try_find("sidebar-chats").is_none());
    })
    .unwrap();
    press(window, "secondary-shift-e", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).view(), ph_workspace::View::Workspaces);
        assert!(window.try_find("sidebar-chats").is_some());
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
