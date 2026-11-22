//! Tests: the project's own, and UI tests of the app being built, in the
//! browser pane or on an Android device. One tab shows both.
//!
//! The project's own tests:
//! - [`suite`]: the suites found in the project (`cargo test`, pytest,
//!   Vitest, Jest, Playwright, Gradle, Maven, `go test`…) and what a run of
//!   one found.
//! - [`suite_run`]: running a suite's command; [`suite_read`]: its results
//!   as one line per test.
//!
//! UI tests:
//! - [`model`]: a test is a scenario in plain words plus recorded steps, in
//!   the project's `.pilothouse/tests/`. A step names its element by what it
//!   is and says.
//! - [`record`]: an agent works through the scenario with the browser or
//!   device tools; each successful action becomes a step, and `test_expect`
//!   adds checks.
//! - [`run`]: replays the steps without AI, from a clean start, with a
//!   screenshot per step and a report, on one screen size or device or on
//!   several (a matrix). A failure can go to an agent to repair.
//! - [`visual`]: a look check's screen against its accepted picture.
//! - [`secrets`]: the passwords a test types, kept out of its file.
//! - [`export`]: a test as a Playwright spec or a Maestro flow.
//!
//! Both:
//! - [`runner`]: each workspace's runs from the Tests tab, kept outside it.
//! - [`control`]: what agents call to list and run tests.

pub mod control;
pub mod export;
pub mod model;
mod panel;
pub mod record;
pub mod run;
pub mod runner;
pub mod secrets;
pub mod suite;
pub mod suite_read;
pub mod suite_run;
pub mod visual;

pub use panel::{TestsPanel, TestsPanelEvent};

/// Registers the `test.*` control methods (and so the agents' tools).
pub fn init(store: ph_store::Store, cx: &mut gpui_kit::App) {
    runner::TestRunHub::set_store(store.clone(), cx);
    record::register(cx);
    control::register(store, cx);
}
