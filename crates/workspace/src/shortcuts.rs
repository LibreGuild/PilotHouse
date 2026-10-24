//! The keyboard shortcuts sheet (⌘/ or Ctrl+/). Each key is read from the
//! key bindings in force, so the sheet shows what the keyboard really does.

use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, kbd::Kbd, v_flex};
use gpui_kit::{
    Action, App, IntoElement, Keystroke, ParentElement as _, SharedString, Styled as _, Window,
    div, px,
};

use crate::actions::*;

/// Opens the sheet.
pub fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, window, cx| {
        dialog
            .title("Keyboard shortcuts")
            .w(px(640.))
            .child(render(window, cx))
    });
}

/// A row: what it does, and its keys.
enum Keys {
    /// The binding of this action in the workspace.
    Action(Box<dyn Action>),
    /// Keys that are not one action (typing in the chat, pickers).
    Fixed(&'static [&'static str]),
    /// Words, for a range of keys.
    Text(String),
}

fn groups() -> Vec<(&'static str, Vec<(&'static str, Keys)>)> {
    let action = |action: Box<dyn Action>| Keys::Action(action);
    let workspace_keys = if cfg!(target_os = "macos") {
        "⌃1 … ⌃9"
    } else {
        "Alt+1 … Alt+9"
    };
    vec![
        (
            "General",
            vec![
                ("Command palette", action(Box::new(ToggleCommandPalette))),
                ("Find a file", action(Box::new(ph_files::QuickOpen))),
                (
                    "Search the workspace",
                    action(Box::new(ph_files::FindInProject)),
                ),
                ("Search chats", action(Box::new(SearchChats))),
                ("New chat", action(Box::new(NewSession))),
                ("Open a folder as a workspace", action(Box::new(AddProject))),
                ("Settings", action(Box::new(OpenSettings))),
                ("Keyboard shortcuts", action(Box::new(ShowShortcuts))),
                ("Quit", action(Box::new(Quit))),
            ],
        ),
        (
            "Workspaces and layout",
            vec![
                ("Workspace 1 to 9", Keys::Text(workspace_keys.into())),
                ("Previous workspace", action(Box::new(PreviousWorkspace))),
                ("Show or hide the sidebar", action(Box::new(ToggleSidebar))),
                (
                    "Show or hide the tool panel",
                    action(Box::new(ToggleContextPanel)),
                ),
                ("Tool panel full size", action(Box::new(ToggleZoom))),
                (
                    "Screen capture to the chat",
                    action(Box::new(CaptureToChat)),
                ),
            ],
        ),
        (
            "Tools",
            vec![
                ("Files", action(Box::new(ShowFiles))),
                ("Git", action(Box::new(ShowChanges))),
                ("Board", action(Box::new(ShowBoard))),
                ("Terminal", action(Box::new(ShowTerminal))),
                ("Browser", action(Box::new(ShowBrowser))),
                ("Devices", action(Box::new(ShowDevices))),
                ("Runs", action(Box::new(ShowRuns))),
                ("Pull requests", action(Box::new(ShowPulls))),
                ("UI tests", action(Box::new(ShowTests))),
            ],
        ),
        (
            "Chat",
            vec![
                ("Send", Keys::Fixed(&["enter"])),
                ("New line", Keys::Fixed(&["shift-enter"])),
                (
                    "Mention a file, card, screenshot or log",
                    Keys::Text("@ or #".into()),
                ),
                ("Move in a menu or picker", Keys::Fixed(&["up", "down"])),
                ("Close a menu or picker", Keys::Fixed(&["escape"])),
                ("Next agent in the model picker", Keys::Fixed(&["tab"])),
            ],
        ),
    ]
}

fn render(window: &mut Window, cx: &mut App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let columns = groups().into_iter().map(|(title, rows)| {
        v_flex()
            .gap_1()
            .child(
                div()
                    .pb_1()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(title)),
            )
            .children(rows.into_iter().map(|(label, keys)| {
                let keys = match keys {
                    Keys::Action(action) => {
                        match Kbd::binding_for_action(
                            action.as_ref(),
                            Some(WORKSPACE_CONTEXT),
                            window,
                        ) {
                            Some(kbd) => kbd.into_any_element(),
                            None => div()
                                .text_xs()
                                .text_color(muted)
                                .child("—")
                                .into_any_element(),
                        }
                    }
                    Keys::Fixed(strokes) => h_flex()
                        .gap_1()
                        .children(
                            strokes
                                .iter()
                                .filter_map(|stroke| Keystroke::parse(stroke).ok())
                                .map(Kbd::new),
                        )
                        .into_any_element(),
                    Keys::Text(text) => div().text_xs().child(text).into_any_element(),
                };
                h_flex()
                    .justify_between()
                    .gap_3()
                    .text_sm()
                    .child(div().truncate().child(label))
                    .child(div().flex_none().child(keys))
            }))
    });
    // Two columns of groups.
    let mut columns: Vec<_> = columns.collect();
    let right = columns.split_off(2);
    h_flex()
        .items_start()
        .gap_6()
        .child(v_flex().flex_1().gap_4().children(columns))
        .child(v_flex().flex_1().gap_4().children(right))
}
