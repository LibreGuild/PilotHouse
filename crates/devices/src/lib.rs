//! Android devices and emulators.
//!
//! - [`sdk`]: find the user's SDK tools (never installed by PilotHouse).
//! - [`avd`]: make, wipe and delete emulators by writing their AVD files.
//! - [`adb`]: blocking adb calls: devices, screencap, input, UI dump, logcat, files.
//! - [`emulator`] + `grpc`: the emulator's gRPC controller, for a live screen
//!   stream and real-time touch, like Android Studio's embedded emulator.
//! - [`DeviceManager`]: connected devices (followed with `adb track-devices`),
//!   AVDs, and starting emulators headless.
//! - [`DeviceScreen`]: one device's live screen and input while it is shown.
//! - [`DevicesPanel`]: the pane.
//! - `control`: `device.*` control methods behind the MCP tools.

pub mod adb;
pub mod avd;
mod avd_dialog;
mod control;
mod devices_panel;
pub mod emulator;
mod grpc;
pub mod logcat;
mod manager;
mod screen;
pub mod sdk;
pub mod ui_tree;

pub use devices_panel::DevicesPanel;
pub use manager::{DeviceManager, DevicesChanged};
pub use screen::{DeviceScreen, ScreenEvent, SystemKey};

/// Registers the control methods. Call once at startup.
pub fn init(cx: &mut gpui_kit::App) {
    control::register(cx);
}
