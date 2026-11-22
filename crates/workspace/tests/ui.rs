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
        // The same two agents on every machine: with none found, a chat
        // greets a first run and has no message box to test.
        ph_agent::agents::assume_installed(&["claude", "codex"]);
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
/// that need no folder ("Chats", made for it: a workspace underneath, with
/// a place of its own in the column of icons); the Bridge lists it; a
/// folder added becomes a workspace with a badge in that column, and
/// Files as its first tool.
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
            assert_eq!(scratch.name, "Chats");
            assert!(scratch.folders().is_empty());
            assert_eq!(workspace.active_project_id(), Some(scratch.id));
            assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
            assert!(window.try_find("bridge").is_none(), "the chat is in front");
            assert!(
                window.try_find("view-chats").is_some(),
                "it has its place over the workspaces"
            );
            assert!(
                window.try_find("workspace-switcher").is_none(),
                "and is not one of the list in the title bar"
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
            window.try_find("workspace-switcher").is_some(),
            "its name in the title bar opens the list of workspaces"
        );
        assert!(
            window.try_find("view-workspaces").is_some(),
            "and the workspaces have their place in the column of icons"
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
    click(window, "view-workspaces", cx);
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
        assert!(window.try_find("context-tab-tests").is_none(), "under More");
        window.dispatch_action(Box::new(ph_workspace::ShowTests), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("context-tab-tests").is_some());
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Tests);
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
        assert_eq!(saved.context_tab.as_deref(), Some("tests"));
    });

    // (⌘J, the terminal, is left out: its first terminal starts a real
    // shell, whose reader thread makes the test scheduler fail.)

    // ⌥⌘B brings the pane back as it was: the tool that was in front.
    cx.update_window(window, |_, window, cx| window.press("secondary-alt-b", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let space = workspace.read(cx).active_space().unwrap().read(cx);
        assert_eq!(space.context_tab(), ContextTab::Tests);
        assert!(space.is_context_open());
    });
}

/// The Tests tab lists the project's own suites and its UI tests in one
/// list; a row shows what it is at its side, and the form for a new UI test
/// starts where the tests there are start. A project with neither says
/// what tests are and offers the two ways to have some.
#[gpui_kit::test]
fn the_tests_tab_lists_the_projects_tests_and_its_ui_tests(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let empty = temp_project("tests-none");
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&empty, window, cx)
        });
        window.dispatch_action(Box::new(ph_workspace::ShowTests), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("tests-empty-new").is_some());
        assert!(window.try_find("tests-empty-command").is_some());
    })
    .unwrap();

    // A crate with a UI test recorded for it.
    let folder = temp_project("tests-some");
    std::fs::write(folder.join("Cargo.toml"), "[package]\nname = \"some\"\n").unwrap();
    std::fs::create_dir_all(folder.join(".pilothouse/tests")).unwrap();
    std::fs::write(
        folder.join(".pilothouse/tests/sign-in.json"),
        r#"{ "name": "Sign in", "description": "Sign in as Ada",
             "target": { "kind": "web", "url": "http://localhost:3000" },
             "steps": [
               { "do": "click", "role": "button", "name": "Sign in" },
               { "do": "expect_text", "text": "Welcome" } ] }"#,
    )
    .unwrap();
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.add_project_path(&folder, window, cx)
        });
        window.dispatch_action(Box::new(ph_workspace::ShowTests), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("tests-empty-new").is_none());
        assert!(window.try_find("suite-cargo").is_some());
        assert!(window.try_find("ui-test-sign-in").is_some());
        assert!(window.try_find("tests-run-all").is_some());
    })
    .unwrap();
    // The suite is picked first: its command shows, with the way to run it.
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("suite-run").is_some());
    })
    .unwrap();
    // The UI test shows its steps, and can be run.
    click_id(window, gpui_kit::SharedString::from("ui-test-sign-in"), cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("test-run").is_some());
        assert!(window.try_find(("test-step", 0usize)).is_some());
        assert!(
            window.try_find(("test-step", 2usize)).is_some(),
            "its two steps, after the one that opens the page"
        );
        assert!(window.try_find("suite-run").is_none());
    })
    .unwrap();
    // A new UI test: the form, which is left again.
    click(window, "test-new", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("test-create").is_some());
        assert!(window.try_find("test-run").is_none());
    })
    .unwrap();
    click(window, "test-cancel", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("test-create").is_none());
        assert!(window.try_find("test-run").is_some());
    })
    .unwrap();
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
        window.dispatch_action(Box::new(ph_workspace::ShowTests), cx);
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

/// Goes to workspace `id` as a person does: the name of the one in front
/// in the title bar, then its row in the list that opens.
fn switch_workspace(window: AnyWindowHandle, id: i64, cx: &mut TestAppContext) {
    click(window, "workspace-switcher", cx);
    click_id(window, ("switch-workspace", id as u64), cx);
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
fn the_list_opens_workspaces_and_cards_open_their_chats(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, beta) = two_workspaces("sidebar", window, &workspace, cx);
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_project_id(), Some(beta));
        let order: Vec<_> = workspace.projects().iter().map(|p| p.id).collect();
        // (After them, the workspace the first start made for chats.)
        assert_eq!(order[..2], [beta, alpha], "newest first, and it stays so");
    });

    switch_workspace(window, alpha, cx);
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

    // The list by its key: it opens on the workspace before, so Enter
    // goes back to it. Ctrl+Tab does the same without the list.
    let list = if cfg!(target_os = "macos") {
        "cmd-e"
    } else {
        "ctrl-e"
    };
    press(window, list, cx);
    press(window, "enter", cx);
    cx.update(|cx| assert_eq!(workspace.read(cx).active_project_id(), Some(beta)));
    press(window, "ctrl-tab", cx);
    cx.update(|cx| assert_eq!(workspace.read(cx).active_project_id(), Some(alpha)));

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
        window.dispatch_action(Box::new(ph_workspace::ShowTests), cx);
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

    // A tab's cross (shown while the pointer is on the tab) closes the
    // tab and nothing else: the chat is not marked done, and is still there.
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("chat-tab-done", first as u64)).is_none(),
            "a tab has no tick"
        );
        gpui_kit::test::TestWindowExt::hover(window, ("chat-tab", first as u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    click_id(window, ("chat-tab-close", first as u64), cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let done: Vec<i64> = AppState::store(cx).setting("workspace.sidebar_settled");
        assert!(done.is_empty(), "closing a tab marks nothing done");
        assert!(
            AppState::store(cx).session(first).unwrap().is_some(),
            "the chat is kept"
        );
        assert!(
            window.try_find(("chat-tab", first as u64)).is_none(),
            "its tab left the row"
        );
        assert!(window.try_find(("chat-tab", second as u64)).is_some());
    })
    .unwrap();
    assert_eq!(front(cx), Some(second), "the chat in front stays");

    // On the Bridge it is where it was: not under Done.
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
                .is_some(),
            "still to be marked done"
        );
    })
    .unwrap();
}

/// With no workspace of the user's, *Workspaces* says what one is and
/// offers the two ways to make one; the first one made takes its place.
#[gpui_kit::test]
fn workspaces_without_any_say_what_they_are(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    click(window, "view-workspaces", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(workspace.read(cx).view(), ph_workspace::View::NoWorkspaces);
        assert!(window.try_find("no-workspaces-open").is_some());
        assert!(window.try_find("no-workspaces-new").is_some());
        assert!(
            window.try_find("workspace-switcher").is_none(),
            "there is none to switch between"
        );
    })
    .unwrap();

    let (_alpha, beta) = two_workspaces("first-ones", window, &workspace, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let workspace = workspace.read(cx);
        assert_eq!(workspace.view(), ph_workspace::View::Workspaces);
        assert_eq!(workspace.active_project_id(), Some(beta));
        assert!(window.try_find("no-workspaces-open").is_none());
        assert!(window.try_find("workspace-switcher").is_some());
    })
    .unwrap();
}

/// A tab's menu closes the others, or all of them: the chats stay, only
/// their tabs go.
#[gpui_kit::test]
fn a_tabs_menu_closes_the_others_or_all(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("tab-menu", window, &workspace, cx);
    let [first, second, third] = cx.update(|cx| {
        let store = AppState::store(cx);
        ["Sort the arrivals", "Name the berths", "Paint the buoys"]
            .map(|title| store.create_session(alpha, title, "claude").unwrap().id)
    });
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
    // What the tab's menu does (its items have no ids to click by).
    let close = |keep: Option<i64>, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            let space = workspace.read(cx).active_space().unwrap().clone();
            space.update(cx, |space, cx| space.close_tabs(keep, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
    };
    let tabs = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            [first, second, third]
                .map(|session| window.try_find(("chat-tab", session as u64)).is_some())
        })
        .unwrap()
    };
    assert_eq!(tabs(cx), [true, true, true]);
    assert_eq!(front(cx), Some(first));

    // The others close, and the tab asked from is in front.
    close(Some(third), cx);
    assert_eq!(tabs(cx), [false, false, true]);
    assert_eq!(front(cx), Some(third));
    cx.update(|cx| {
        let store = AppState::store(cx);
        for session in [first, second] {
            assert!(
                store.session(session).unwrap().is_some(),
                "the chats are kept"
            );
        }
        let mut closed = store.project_ui_state(alpha).unwrap().closed_tabs;
        closed.sort();
        assert_eq!(closed, vec![first, second], "kept for the next start");
    });

    // All close: a new chat is in front.
    close(None, cx);
    assert_eq!(tabs(cx), [false, false, false]);
    assert_eq!(front(cx), None);
}

/// Closing the tab in front shows the one beside it; a closed tab is back
/// when its chat is opened again, and stays closed when the app starts
/// again.
#[gpui_kit::test]
fn a_closed_tab_gives_way_and_comes_back(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("close-tabs", window, &workspace, cx);
    let [first, second, third] = cx.update(|cx| {
        let store = AppState::store(cx);
        ["Sort the arrivals", "Name the berths", "Paint the buoys"]
            .map(|title| store.create_session(alpha, title, "claude").unwrap().id)
    });
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_chat(alpha, second, window, cx)
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
    let hover_and_close = |session: i64, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            gpui_kit::test::TestWindowExt::hover(window, ("chat-tab", session as u64), cx);
        })
        .unwrap();
        cx.run_until_parked();
        click_id(window, ("chat-tab-close", session as u64), cx);
    };
    assert_eq!(front(cx), Some(second));
    hover_and_close(second, cx);
    let beside = front(cx);
    assert!(
        beside == Some(first) || beside == Some(third),
        "the tab beside it is in front: {beside:?}"
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("chat-tab", second as u64)).is_none());
        let saved = AppState::store(cx).project_ui_state(alpha).unwrap();
        assert_eq!(saved.closed_tabs, vec![second], "kept for the next start");
    })
    .unwrap();

    // Past chats still lists it, and opening it there gives it its tab.
    click(window, "chat-history", cx);
    click_id(window, ("history-row", second as u64), cx);
    assert_eq!(front(cx), Some(second));
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("chat-tab", second as u64)).is_some());
        let saved = AppState::store(cx).project_ui_state(alpha).unwrap();
        assert!(saved.closed_tabs.is_empty());
    })
    .unwrap();

    // With every tab closed, a new chat is in front.
    for session in [first, second, third] {
        hover_and_close(session, cx);
    }
    assert_eq!(front(cx), None, "nothing is left to show but a new chat");
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        for session in [first, second, third] {
            assert!(window.try_find(("chat-tab", session as u64)).is_none());
        }
        assert!(
            window.try_find("send-message").is_some(),
            "with its message box"
        );
    })
    .unwrap();
}

/// Past chats is one list, the last used first, whatever a chat's state:
/// the pop-up has the last ten and the way to all of them, which show
/// where the chat was (no message box) until one is picked. A row marks
/// its chat done, renames it and deletes it.
#[gpui_kit::test]
fn past_chats_are_one_list_with_what_can_be_done(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("history", window, &workspace, cx);
    let sessions: Vec<i64> = cx.update(|cx| {
        let store = AppState::store(cx);
        (1..=12)
            .map(|n| {
                let agent = if n % 2 == 0 { "codex" } else { "claude" };
                store
                    .create_session(alpha, &format!("Chat number {n}"), agent)
                    .unwrap()
                    .id
            })
            .collect()
    });
    // The store lists them the last used first: that order is the list's.
    let listed: Vec<i64> = cx.update(|cx| {
        AppState::store(cx)
            .sessions(alpha)
            .unwrap()
            .iter()
            .map(|session| session.id)
            .collect()
    });
    assert_eq!(listed.len(), 12);
    let (newest, oldest) = (listed[0], listed[11]);
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_chat(alpha, newest, window, cx)
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
    let hover_row = |session: i64, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            gpui_kit::test::TestWindowExt::hover(window, ("history-row", session as u64), cx);
        })
        .unwrap();
        cx.run_until_parked();
    };

    // The pop-up: the ten last used, in that order, and no more.
    click(window, "chat-history", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let mut last = f32::MIN;
        for session in &listed[..10] {
            let row = window.find(("history-row", *session as u64));
            let top: f32 = row.bounds().origin.y.into();
            assert!(top > last, "the last used first, one under the other");
            last = top;
        }
        for session in &listed[10..] {
            assert!(
                window.try_find(("history-row", *session as u64)).is_none(),
                "the pop-up stops at ten"
            );
        }
        assert!(window.try_find("history-show-all").is_some());
    })
    .unwrap();

    // Marked done from its row: kept, and the row stays in the list.
    let marked = listed[2];
    hover_row(marked, cx);
    click_id(
        window,
        gpui_kit::SharedString::from(format!("history-done-{marked}")),
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let done: Vec<i64> = AppState::store(cx).setting("workspace.sidebar_settled");
        assert_eq!(done, vec![marked]);
        assert!(
            window.try_find(("history-row", marked as u64)).is_some(),
            "a chat marked done is not moved out of the list"
        );
    })
    .unwrap();

    // All of them, where the chat was.
    click(window, "history-show-all", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("all-chats").is_some());
        assert!(
            window.try_find("send-message").is_none(),
            "the list takes the chat's place, message box and all"
        );
        assert!(
            window.try_find(("history-row", oldest as u64)).is_some(),
            "every chat is listed"
        );
    })
    .unwrap();

    // Renamed from its row.
    let renamed = listed[5];
    hover_row(renamed, cx);
    click_id(
        window,
        gpui_kit::SharedString::from(format!("history-rename-{renamed}")),
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("dialog-ok").is_some(),
            "it asks for the name"
        );
        window.press("secondary-a", cx);
        window.input("Moor the tender", cx);
    })
    .unwrap();
    cx.run_until_parked();
    click(window, "dialog-ok", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            AppState::store(cx).session(renamed).unwrap().unwrap().title,
            "Moor the tender"
        );
        assert!(
            window.try_find("all-chats").is_some(),
            "the list is still up"
        );
        assert!(window.try_find_by_label("Moor the tender").is_some());
    })
    .unwrap();

    // Deleted from its row, after it asked.
    let deleted = listed[7];
    hover_row(deleted, cx);
    click_id(
        window,
        gpui_kit::SharedString::from(format!("history-delete-{deleted}")),
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("dialog-ok").is_some(), "it asks first");
        assert!(AppState::store(cx).session(deleted).unwrap().is_some());
    })
    .unwrap();
    click(window, "dialog-ok", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(AppState::store(cx).session(deleted).unwrap().is_none());
        assert!(window.try_find(("history-row", deleted as u64)).is_none());
    })
    .unwrap();

    // A chat picked from the list shows as a past chat does.
    click_id(window, ("history-row", oldest as u64), cx);
    assert_eq!(front(cx), Some(oldest));
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("all-chats").is_none());
        assert!(window.try_find("send-message").is_some());
    })
    .unwrap();
    let _ = sessions;
}

/// Where new sessions open as a CLI, *New chat* asks which agent's first:
/// nothing starts until one is picked, and the header's switch shows the
/// chat instead.
#[gpui_kit::test]
fn a_new_cli_session_asks_which_agent(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("cli-pick", window, &workspace, cx);
    cx.update(|cx| {
        let store = AppState::store(cx);
        let mut profiles = ph_agent::profiles::Profiles::load(store);
        profiles.open_as = ph_agent::profiles::OpenAs::Cli;
        profiles.save(store).unwrap();
    });
    switch_workspace(window, alpha, cx);
    click(window, "chat-new", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("cli-pick-claude").is_some()
                && window.try_find("cli-pick-codex").is_some(),
            "the agents that have a CLI are offered"
        );
        assert!(
            window.try_find("send-message").is_none(),
            "in place of the chat"
        );
        assert!(
            ph_terminal::TerminalHub::existing_agent_terminals(alpha, cx).is_none(),
            "nothing starts until one is picked"
        );
        // The chat instead: the new chat it is, and still nothing started.
        let chat = window.find_by_label("Chat").bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, chat, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, chat, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("cli-pick-claude").is_none());
        assert!(window.try_find("send-message").is_some());
        assert!(ph_terminal::TerminalHub::existing_agent_terminals(alpha, cx).is_none());
        assert!(
            AppState::store(cx).sessions(alpha).unwrap().is_empty(),
            "no session was made for asking"
        );
    })
    .unwrap();
    // Back to how sessions open by default, for the tests after this one
    // (the profiles as last read are kept for the whole run).
    cx.update(|cx| {
        let store = AppState::store(cx);
        let mut profiles = ph_agent::profiles::Profiles::load(store);
        profiles.open_as = ph_agent::profiles::OpenAs::Chat;
        profiles.save(store).unwrap();
    });
}

/// A session has two forms, the chat and its agent's CLI. One stored in its
/// CLI form whose program is not running (the app was started again) shows
/// what was said read only, with a way to go on there, and the header's
/// switch takes it back to the chat. Nothing starts by itself.
#[gpui_kit::test]
fn a_session_in_its_cli_form_goes_on_in_the_chat(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let (alpha, _beta) = two_workspaces("forms", window, &workspace, cx);
    let session = cx.update(|cx| {
        let store = AppState::store(cx);
        let session = store
            .create_session(alpha, "Sort the arrivals", "claude")
            .unwrap();
        // As a chat that went on in its agent's CLI is kept.
        store
            .set_session_place(session.id, "terminal", None)
            .unwrap();
        session.id
    });
    workspace.update(cx, |workspace, cx| {
        workspace.unload_idle(std::time::Duration::ZERO, cx)
    });
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_chat(alpha, session, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("cli-continue").is_some(),
            "its CLI is not running, and can be started"
        );
        assert!(
            window.try_find("send-message").is_none(),
            "there is no message box in its CLI form"
        );
        // It keeps its tab among the workspace's chats, and has the switch.
        assert!(window.try_find(("chat-tab", session as u64)).is_some());
        assert!(window.try_find_by_label("Chat").is_some());
        assert!(window.try_find_by_label("CLI").is_some());
        // No agent program was started for showing it.
        assert!(
            ph_terminal::TerminalHub::existing_agent_terminals(alpha, cx).is_none(),
            "nothing starts by itself"
        );
        // Back to the chat.
        let chat = window.find_by_label("Chat").bounds().center();
        gpui_kit::test::TestEventExt::pointer_down(window, chat, gpui_kit::MouseButton::Left, cx);
        gpui_kit::test::TestEventExt::pointer_up(window, chat, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    // What its CLI part held is read off the UI thread first.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let kind = cx.update(|cx| AppState::store(cx).session(session).unwrap().unwrap().kind);
        if kind == "chat" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the session did not go back to the chat"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("send-message").is_some(),
            "the chat has its message box"
        );
        assert!(window.try_find("cli-continue").is_none());
        assert!(window.try_find(("chat-tab", session as u64)).is_some());
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

/// The changes are read in two ways in one pane: as they are, or told in
/// plain words. *Explain* shows the explanation there is of the work as it
/// stands, and runs no agent for it.
#[gpui_kit::test]
fn changes_are_explained_in_place_of_their_diff(cx: &mut TestAppContext) {
    let (window, workspace) = open_workspace(cx);
    let folder = temp_project("explain");
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
    // An agent explained this work earlier.
    let top = ph_git::content::top(&folder).unwrap();
    let content = ph_git::content::pending(&top).unwrap();
    let project = cx
        .update_window(window, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.add_project_path(&folder, window, cx)
            });
            window.dispatch_action(Box::new(ph_workspace::ShowChanges), cx);
            let store = AppState::store(cx);
            let id = store
                .create_explanation(
                    &top.to_string_lossy(),
                    "changes",
                    "Claude Code",
                    &content,
                    "[]",
                )
                .unwrap();
            store
                .finish_explanation(
                    id,
                    "It says hi",
                    "# It says hi\n\n## In short\n\nThe program prints a greeting now.",
                    None,
                )
                .unwrap();
            workspace
                .read(cx)
                .active_space()
                .unwrap()
                .read(cx)
                .project()
                .id
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("explain-changes").is_some(),
            "Explain stands beside Review"
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("git-review").is_some(), "the changes show");
        assert!(window.try_find("git-explanation").is_none());
    })
    .unwrap();
    click(window, "explain-changes", cx);
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("git-explanation").is_some(),
            "the explanation shows in place of the diff"
        );
        assert!(window.try_find("explain-again").is_some());
        // The one there is was shown: no chat was started to write it.
        assert!(AppState::store(cx).sessions(project).unwrap().is_empty());
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
    switch_workspace(window, alpha, cx);
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
            .find(|project| project.name == "Chats")
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

#[gpui_kit::test]
fn utilities_open_from_the_rail_and_every_one_draws(cx: &mut TestAppContext) {
    use gpui_kit::component::WindowExt as _;
    let (window, _) = open_workspace(cx);
    click(window, "open-utilities", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("utilities").is_some(),
            "the rail opens the dialog"
        );
    })
    .unwrap();
    // Enter in the search takes the first found, and leaves the dialog open
    // (it used to close it).
    click(window, "utilities-search", cx);
    cx.update_window(window, |_, window, cx| {
        window.input("cron", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("utilities").is_some(),
            "Enter does not close it"
        );
        assert!(
            window.try_find("cron-preset-Hourly").is_some(),
            "Cron is in front"
        );
    })
    .unwrap();
    // Every utility's own view is made and drawn.
    for utility in ph_utils::UTILITIES {
        let store = cx.update(|cx| AppState::store(cx).clone());
        cx.update_window(window, |_, window, cx| {
            window.close_dialog(cx);
            ph_utils::dialog::open(Some(utility.id), store, window, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find(gpui_kit::SharedString::from(format!(
                        "utility-{}",
                        utility.id
                    )))
                    .is_some(),
                "{} is listed",
                utility.id
            );
        })
        .unwrap();
    }
}
