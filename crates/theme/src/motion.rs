//! Motion spec: the only durations PilotHouse animates with.
//!
//! Motion explains a change (a panel opening, a card moving); it is never
//! ambient decoration. Keep to these three steps so the app moves consistently.

use std::time::Duration;

/// Hover and press feedback, small fades.
pub const FAST: Duration = Duration::from_millis(120);
/// Panels, popovers and list items appearing.
pub const STANDARD: Duration = Duration::from_millis(180);
/// Larger spatial changes such as switching the context panel.
pub const SLOW: Duration = Duration::from_millis(260);
