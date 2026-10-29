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
//! - [`android_project`] + [`AndroidRun`]: build, install and start an app
//!   module's variant on the chosen device (the Run tab's Android card).
//! - `control`: `device.*` control methods behind the MCP tools.

pub mod adb;
pub mod android_project;
mod android_run;
pub mod avd;
mod avd_dialog;
mod connect_dialog;
mod control;
mod devices_panel;
pub mod emulator;
mod grpc;
pub mod h264;
mod input_shell;
pub mod logcat;
mod logcat_view;
mod manager;
mod screen;
pub mod sdk;
pub mod ui_tree;

pub use android_run::{AndroidRun, AndroidRunEvent};
pub use devices_panel::{DevicesPanel, StreamQualityChanged};
pub use manager::{DeviceManager, DevicesChanged};
pub use screen::{DeviceScreen, ScreenEvent, StreamQuality, SystemKey};

/// Registers the control methods. Call once at startup.
pub fn init(cx: &mut gpui_kit::App) {
    control::register(cx);
}
