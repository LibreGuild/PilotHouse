# Devices

Crate: `ph-devices`. Android emulators and phones, through the SDK you already have. PilotHouse finds it in `$ANDROID_HOME`, `$ANDROID_SDK_ROOT`, `~/Library/Android/sdk` or `~/Android/Sdk`, or on the login shell's PATH. It never installs SDK parts.

## Device manager

- **Connected devices** come from `adb track-devices`, which reports changes as they happen (no polling). Emulators are named by their AVD.
- **Emulators** (AVDs from `emulator -list-avds`) start headless (`-no-window -no-audio -no-boot-anim`) and show in the pane. Start them from the device menu, the empty state, or an agent (`device_start_emulator`). PilotHouse waits for `sys.boot_completed`. Stop one from the device menu. Emulators keep running after PilotHouse quits, as they do with Android Studio.
- **Phones** need USB debugging on. An unauthorized phone shows how to allow it.
- **Over the network**: *Connect a device…* in the device menu opens one page (`connect_dialog.rs`), the easiest way first. **Saved**: devices connected over the network before, each a click away again; they can be given names (devices of one model are told apart by them) and forgotten. **On this network**: phones with Wireless debugging on, which adb finds by itself (`adb mdns services`, asked every few seconds while the dialog is open). **On USB**: a phone on the cable is switched to Wi-Fi in one click (`adb tcpip 5555`, the phone's `wlan0` address, `adb connect`, as Android Studio's ADB Wi-Fi plugin does). **By address**: a device already listening (port 5555 unless one is given). **Pair with a code**: an Android 11+ phone, once, from its *Wireless debugging* screen (`adb pair`, then connected where adb finds it listening). The dialog stays open until a device is connected: a failure is said in it in words that say what to do (nothing answers, cannot be reached, not paired, wrong code), and what was typed stays.
- **Devices are remembered** (`manager.rs`, settings `devices.known` and `devices.selected`). A device that connects over the network at an address (`host:port`) is saved, however it was connected, also from a terminal; the same device at a new address keeps its entry and its name, since each is told by its own serial number (`ro.serialno`). Saved devices that are not connected are listed under *Connect again* in the device menu. The device chosen last is the one chosen again when it is there, also after a restart; nothing is connected by itself.
- **The device menu** names each connected device with its Android version and, over the network, its address, with an icon for how it is connected (cable, network, emulator): two phones of a model, or one phone on a cable and over Wi-Fi, are told apart. What is done to emulators once in a while (*New emulator…*, *Wipe data*, *Delete*) is under *Emulators*.

- **Make, wipe and delete emulators** from the device menu (*New emulator*, *Wipe data*, *Delete emulator*) or the empty state. *New emulator* asks for a name, one of the installed system images, and a phone or tablet screen, then starts it. PilotHouse writes the AVD's `<name>.ini` and `config.ini` itself (`avd.rs`), as `avdmanager` does, so it works without the SDK command-line tools, and Android Studio sees the same AVDs. *Wipe data* removes the user data, cache and snapshots (the configuration and SD card stay), like Android Studio's *Wipe Data*; *Delete* removes the AVD folder. Both ask first and are offered only for stopped emulators. AVDs live in `$ANDROID_AVD_HOME`, else `$ANDROID_USER_HOME/avd`, else `~/.android/avd`. With no system image installed, the dialog says how to get one in Android Studio's SDK Manager.

Not yet: iOS.

## Live screen

| Source | Frames | Input |
|---|---|---|
| Emulator | gRPC `streamScreenshot` from the emulator's local controller, RGB at up to 640×1280: ~35–40 fps while the screen changes, nothing when it is still | gRPC `sendTouch` / `sendKey`, in real time (drags and scrolls feel native) |
| Phone (or an emulator without gRPC) | [scrcpy](https://github.com/Genymobile/scrcpy)'s server (`scrcpy.rs`), as scrcpy itself and Android Studio's mirroring work: the phone's hardware encoder compresses what changes into H.264 (up to 720×1280 at 8 Mbit/s over USB, half that over Wi-Fi, 60 frames a second at most), and the system decodes it (`h264/`: VideoToolbox on macOS, Media Foundation on Windows). A turned screen is a new video in the same stream | The same server, in real time: a drag is followed while the finger is down, a long press and a fling are what they are on the phone, the wheel scrolls. Text Android has keys for is typed; any other script goes through the phone's clipboard. ⌘V pastes this computer's clipboard on the phone, ⌘C and ⌘X bring the phone's selection back |

**scrcpy's server** is bundled unchanged (`crates/devices/assets/scrcpy-server-v5.0.1`, Apache-2.0, see `NOTICE`). It is sent to the phone through a pipe (`adb exec-in`, to `/data/local/tmp/pilothouse-scrcpy-server.jar`, a name of its own so a scrcpy of another version on the same phone is left alone), started with `app_process` (its file is removed again once it runs), and reached through one `adb forward`ed port: a video connection and a control connection. Its protocol is private to scrcpy and changes between versions, so the client and the server here are one version: updating the server means reading `scrcpy.rs` against scrcpy's `doc/develop.md` and `app/tests/test_control_msg_serialize.c` again (the tests in `scrcpy.rs` use that file's bytes). Nothing of the phone's settings is changed, and the server ends with the pane.

**When the server will not start** on a device, its screen is recorded with `adb exec-out screenrecord --output-format=h264` instead: the same video, restarted every three minutes (the tool's limit) and when the screen turns (seen in the video, where a turned screen sits between black bands; only then is the phone asked for its rotation), with gestures sent on release through one long-lived `adb shell` (`input tap`/`swipe`/`text`/`keyevent`). A device that streams no video at all is shown in snapshots (`screencap`, four a second at most), which leaves it usable. Where this computer has no H.264 decoder (Linux), `screenrecord` streams raw RGB frames (`--output-format=frames`): that is heavy for the phone, which reads every frame back from its GPU (measured on a Galaxy S20 FE: 97% of an app's frames late while it scrolled, against none with video). The toolbar shows a warning mark, with the reason, whenever the screen does not come as video.

**Mirroring leaves the phone as fast as it is.** Measured on a Galaxy S20 FE (Android 13) scrolling Settings, with `dumpsys gfxinfo`: 0.2% of frames late with scrcpy's server, the same as with nothing mirrored.

The emulator writes its gRPC port and token to a discovery file (`…/avd/running/pid_*.ini`); PilotHouse reads it, so nothing needs configuring. A killed emulator leaves its file behind, so the newest file for the port wins. Just after boot the endpoint turns callers away for a few seconds, so the screen tries again for up to 8 s before falling back to adb (an emulator with no endpoint falls back at once). The gRPC client is hand-written (`grpc.rs`, HTTP/2 cleartext with HPACK literals and protobuf for the few messages needed), so there is no tokio or protoc dependency. Input calls are sent one at a time in order: separate calls could otherwise be handled out of order, and a lift that overtakes a move leaves the finger down. Each received frame is handed over without copying and converted to the GPU's layout in one pass. A connection is closed as soon as its pane, screen or agent call is done, so no socket or thread is left behind.

Back, Home and Recents are in the toolbar, with the live tools (pick, mark, capture, record) beside them. Esc is Back, typing types, and ⌘V pastes.

## Tools in the pane

- **The toolbar in a narrow pane**: a long device name ends in an ellipsis (the whole name in a tooltip), then the live tools go into **⋯**; Back, Home, Recents, **⋯** and the button that stops mirroring always fit.
- **⋯** in the toolbar has what is done once in a while: **Install an APK…** (file picker → `adb install -r -t`), **Launch an app** (from `pm list packages -3`) and, for a phone, **Stream quality**.
- **Mirroring is asked for.** Choosing a device does not show its screen: a phone may be plugged in only for its log or its files. Without the screen the pane shows the device with **Mirror screen**, **Logcat** and **Files**; the toolbar's **Mirror** (an icon to stop, once it mirrors) switches it, and the choice is kept for that device (`devices.mirror`). An emulator mirrors unless told not to, since that is what it was started for. Install, Launch, Logcat, Files and Layout work either way, and so do the agents' device tools. A screen hidden by another tool keeps streaming for 20 seconds, so a look at a file or the terminal and back finds the picture there.
- **Drawer** (drag its top edge to resize): **Logcat** (the device's log under its screen, the same one as the Logcat tool; a button gives it the whole pane), **Layout** (the uiautomator hierarchy; click a node to outline it) and **Files** (browse, upload, save to Downloads).
- **The device in use** shows in the status bar once devices are used: its menu switches devices, connects one, starts an emulator, and opens Logcat or this pane.

## Logcat

A drawer under the phone's screen (Preview → Phone), mirrored or not, and a tool of its own (`logcat_panel.rs`) with the result pane to itself: the drawer's button gives it the pane, and so does the status bar's device menu. It is not in the result pane's ⋯ list of tools. As a tool, its header picks the device (the same one the app uses). It needs no screen.

- **It reads like text in an editor** (`logcat_view.rs`), after Android Studio's Logcat: the last 20,000 lines in columns (time, level, tag, message). Text is selected by dragging over it, by a double click (a word: a class name or an address is one) and a triple click (the line), and with Shift; ⌘C copies it, ⌘A selects everything shown, Esc lets go. Lines selected whole are copied as logcat printed them (with date, pid and tid), a part of a line as shown. A right click offers *Copy*, *Copy message*, *Send to chat*, *Only tag …*, *Hide tag …* and *Only* the line's app or process.
- **Quiet colours**: warnings are amber and errors red; everything else is plain text, the lowest level a little dimmer. Long messages wrap under the message column. With wrapping switched off in the toolbar (the choice is kept, `logcat.wrap`) each line is one row, as long as it is, and the list scrolls sideways.
- **A stack frame's file is a link** (`Main.kt:42`): a click opens it at that line in Files, found among the workspace's folders by the frame's package.
- **Filter** (`logcat.rs`): words (all must match the tag or message, and are marked), `-word`, `tag:Name`, `-tag:Name`, `package:com.app`, `package:mine` (the open project's apps), `pid:123`, `level:warn`, `is:crash` and `is:stacktrace`. The level and app pickers write their words into the box; the app picker is a list to search (*All apps*, *This project*, then the apps installed, found by a part of their name). In an Android project the box starts as `package:mine`, the project's application ids being read from its build files and builds (`android_project::application_ids`); an app run from the Run tab is added by its exact id.
- **Lines are told to belong to an app by its process**: the processes running when the log opens (`ps`), then every process ActivityManager starts or ends, and a crashing one named by its own report. So the lines of an app that crashed and started again are all its, also those from before the log opened.
- **It keeps running once it was opened** (or once an app was run from the Run tab), for as long as its device is in use: what the app wrote while another tool was in front is there on coming back. It follows new lines until one scrolls back or clicks into it.
- **A crash of the project's app is said** in a notice with the exception, which offers to send the stack to the chat, and on the Run tab's Android card.
- **Live tools**, in the toolbar: the element picker (the smallest UI node under the click, sent with its id, text and bounds), marks, capture, record. See [live-panes.md](live-panes.md).

The screen streams only while it is mirrored and the pane is visible (and 20 seconds after); logcat starts when it is first asked for.

## Agent tools

`device_list`, `device_start_emulator`, `device_screenshot` (scaled to 1280 px, with the factor to device pixels), `device_ui` (elements with bounds and centers), `device_tap`, `device_swipe`, `device_type`, `device_key`, `device_logcat` (with `package` for one app's lines, also of one that crashed, and `level`), `device_install`, `device_launch`. Each takes an optional `serial`. Without one, the device in the pane is used, or the only one connected.

## Code map

| File | Role |
|---|---|
| `sdk.rs`, `adb.rs` | Finding tools; every adb call (blocking, run off the UI thread) |
| `scrcpy.rs`, `assets/scrcpy-server-v…` | The client for scrcpy's server, and the server: a phone's video and input |
| `grpc.rs`, `emulator.rs` | Emulator controller: discovery, screen stream, touch and keys |
| `manager.rs` | Devices, AVDs, the selected device, the devices saved to connect again, starting and stopping emulators |
| `avd.rs`, `avd_dialog.rs` | System images; making, wiping and deleting AVDs; their dialogs |
| `screen.rs` | One device's live screen and input |
| `logcat.rs`, `logcat_view.rs`, `logcat_panel.rs`, `ui_tree.rs` | Live logcat, its query and view, the Logcat tool; uiautomator XML parsing and hit testing |
| `h264/` | Cutting the H.264 stream into pictures, and the system decoders |
| `connect_dialog.rs` | Connecting a device over the network: saved ones, those adb finds, by address, by pairing code, or switching a USB phone to Wi-Fi |
| `android_project.rs`, `android_run.rs` | App modules and build variants; the Run tab's Android card |
| `devices_panel.rs`, `control.rs` | The pane; `device.*` control methods |
