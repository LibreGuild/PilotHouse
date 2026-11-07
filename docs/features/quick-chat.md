# Quick chat

Crate: `ph-workspace` (`quick_chat.rs`), behind the `quick_chat` feature (`brand/brand.toml`).

A small chat window over every other app, to ask an agent something without going to PilotHouse. Its chats are in the **Chats** space (no folder): they list under *Chats* in the sidebar and go on there like any other.

## Opening it

- **The shortcut**, from anywhere while PilotHouse runs: ⌥Space on macOS, Alt+Shift+Space on Windows and Linux until another is set. Pressed again while the window is in front, it hides it; while another window is in front, it brings it forward.
- **The command palette**: *Quick chat*.
- **`pilothouse quick-chat`** (also `app.quick_chat` on the control socket): the same, from a shell or a shortcut set in the system's own keyboard settings. With PilotHouse not running, it starts with the window open. On Wayland this is the only way: the desktop keeps keys for itself, so no app can take one.

**Settings → Chat → Quick chat** sets the shortcut: click the field and press the keys (a modifier and a letter, digit, Space or function key; F-keys alone are allowed). The shortcut held until then is let go meanwhile, so the same one can be pressed again. *Reset* goes back to the default, *Turn off* leaves none (the palette and the command still work). A shortcut another app or the system already holds is said there, as is Wayland's limit.

The shortcut is the system's (`global-hotkey`: Carbon's `RegisterEventHotKey` on macOS, which needs no permission; `RegisterHotKey` on Windows; an X11 key grab on Linux). It only works while PilotHouse runs; on Windows and Linux PilotHouse quits when its last window closes.

## The window

- **macOS**: a floating panel, on every Space and over full-screen apps, typed in without bringing PilotHouse's other windows forward (`WindowKind::PopUp`, a non-activating `NSPanel` that can take the keyboard).
- **Windows**: a small window kept above the others (`SetWindowPos(HWND_TOPMOST)`).
- **Linux**: a small window, raised and focused when it opens; whether it stays on top is the window manager's to say.

It can be moved by its title bar and resized (at least 320 × 240). It opens where it was last, while that is still on a screen, else centred on the main screen; and with the chat it showed last, while that chat is still there.

- **Esc** hides it (the chat goes on: a reply still comes, and the sidebar shows it).
- **⌘N / Ctrl+N** or the pen button: a new chat.
- The arrow button opens the chat in the main window, under *Chats*, and hides the quick window.

The chat is a full one: any agent and model, attachments, permissions, and the agent's work folder is the Chats space's own.
