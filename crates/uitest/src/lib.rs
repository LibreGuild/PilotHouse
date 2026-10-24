//! UI tests for the app being built, in the browser pane or on an Android
//! device.
//!
//! - [`model`]: a test is a plain-English scenario plus recorded steps, in
//!   the project's `.pilothouse/tests/`.
//! - [`record`]: an agent works through the scenario with the browser or
//!   device tools; each successful action becomes a step, and `test_expect`
//!   adds checks.
//! - [`run`]: replays the steps without AI, with a screenshot per step and a
//!   report, on one screen size or device or on several (a matrix). A
//!   failure can go to an agent to repair, or to the board.
//! - [`runner`]: each workspace's runs from the Tests tab, kept outside it.
//! - [`visual`]: each step's screen against the accepted baseline.
//! - [`gate`]: tests a board card must pass to be Done; agents run tests.

pub mod gate;
pub mod model;
mod panel;
pub mod record;
pub mod run;
pub mod runner;
pub mod visual;

pub use panel::{TestsPanel, TestsPanelEvent};

/// Registers the `test.*` control methods (and so the agents' tools).
pub fn init(store: ph_store::Store, cx: &mut gpui_kit::App) {
    record::register(cx);
    gate::register(store, cx);
}
