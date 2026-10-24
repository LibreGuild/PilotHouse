# Devices

Crate: `ph-devices`. Android emulators and phones, through the SDK you already have. PilotHouse finds it in `$ANDROID_HOME`, `$ANDROID_SDK_ROOT`, `~/Library/Android/sdk` or `~/Android/Sdk`, or on the login shell's PATH. It never installs SDK parts.

## Device manager

- **Connected devices** come from `adb track-devices`, which reports changes as they happen (no polling). Emulators are named by their AVD.
- **Emulators** (AVDs from `emulator -list-avds`) start headless (`-no-window -no-audio -no-boot-anim`) and show in the pane. Start them from the device menu, the empty state, or an agent (`device_start_emulator`). PilotHouse waits for `sys.boot_completed`. Stop one from the device menu. Emulators keep running after PilotHouse quits, as they do with Android Studio.
- **Phones** need USB debugging on. An unauthorized phone shows how to allow it.

- **Make, wipe and delete emulators** from the device menu (*New emulator…*, *Wipe data*, *Delete emulator*) or the empty state. *New emulator* asks for a name, one of the installed system images, and a phone or tablet screen, then starts it. PilotHouse writes the AVD's `<name>.ini` and `config.ini` itself (`avd.rs`), as `avdmanager` does, so it works without the SDK command-line tools, and Android Studio sees the same AVDs. *Wipe data* removes the user data, cache and snapshots (the configuration and SD card stay), like Android Studio's *Wipe Data*; *Delete* removes the AVD folder. Both ask first and are offered only for stopped emulators. AVDs live in `$ANDROID_AVD_HOME`, else `$ANDROID_USER_HOME/avd`, else `~/.android/avd`. With no system image installed, the dialog says how to get one in Android Studio's SDK Manager.

Not yet: iOS.

## Live screen

| Source | Frames | Input |
|---|---|---|
| Emulator | gRPC `streamScreenshot` from the emulator's local controller, RGB at up to 640×1280: ~35–40 fps while the screen changes, nothing when it is still | gRPC `sendTouch` / `sendKey`, in real time (drags and scrolls feel native) |
| Phone (or an emulator without gRPC) | `adb exec-out screencap`, polled as fast as the device answers (raw pixels, or PNG if raw is slow) | Gestures sent on release through one long-lived `adb shell` (`input tap`/`swipe`/`text`/`keyevent`) |

The emulator writes its gRPC port and token to a discovery file (`…/avd/running/pid_*.ini`); PilotHouse reads it, so nothing needs configuring. A killed emulator leaves its file behind, so the newest file for the port wins. Just after boot the endpoint turns callers away for a few seconds, so the screen tries again for up to 8 s before falling back to adb (an emulator with no endpoint falls back at once). The gRPC client is hand-written (`grpc.rs`, HTTP/2 cleartext with HPACK literals and protobuf for the few messages needed), so there is no tokio or protoc dependency. Input calls are sent one at a time in order: separate calls could otherwise be handled out of order, and a lift that overtakes a move leaves the finger down. Each received frame is handed over without copying and converted to the GPU's layout in one pass. A connection is closed as soon as its pane, screen or agent call is done, so no socket or thread is left behind.

Back, Home and Recents are in the toolbar. Esc is Back, typing types, and ⌘V pastes.

## Tools in the pane

- **Install an APK** (file picker → `adb install -r`) and **Launch** an installed app (from `pm list packages -3`).
- **Drawer**: **Logcat** (live, filter, errors only, send to chat), **UI** (the uiautomator hierarchy; click a node to outline it), and **Files** (browse, upload, save to Downloads).
- **Live tools**: annotate, the element picker (the smallest UI node under the click, sent with its id, text and bounds), capture, record, before/after. See [live-panes.md](live-panes.md).

The screen and logcat run only while the pane is visible.

## Agent tools

`device_list`, `device_start_emulator`, `device_screenshot` (scaled to 1280 px, with the factor to device pixels), `device_ui` (elements with bounds and centers), `device_tap`, `device_swipe`, `device_type`, `device_key`, `device_logcat`, `device_install`, `device_launch`. Each takes an optional `serial`. Without one, the device in the pane is used, or the only one connected.

## Code map

| File | Role |
|---|---|
| `sdk.rs`, `adb.rs` | Finding tools; every adb call (blocking, run off the UI thread) |
| `grpc.rs`, `emulator.rs` | Emulator controller: discovery, screen stream, touch and keys |
| `manager.rs` | Devices, AVDs, the selected device, starting and stopping emulators |
| `avd.rs`, `avd_dialog.rs` | System images; making, wiping and deleting AVDs; their dialogs |
| `screen.rs` | One device's live screen and input |
| `logcat.rs`, `ui_tree.rs` | Live logcat; uiautomator XML parsing and hit testing |
| `devices_panel.rs`, `control.rs` | The pane; `device.*` control methods |
