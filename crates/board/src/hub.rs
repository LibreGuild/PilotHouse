use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global};
use ph_store::projects::ProjectId;

/// Broadcasts "this project's board changed" to every open board view, no
/// matter who changed it (the UI, an agent over MCP, another window).
pub struct BoardHub {
    events: Entity<BoardEvents>,
}

/// The entity board views subscribe to.
pub struct BoardEvents;

#[derive(Debug, Clone, Copy)]
pub struct BoardChanged {
    pub project_id: ProjectId,
}

impl EventEmitter<BoardChanged> for BoardEvents {}

impl Global for BoardHub {}

impl BoardHub {
    /// The shared event source, created on first use.
    pub fn events(cx: &mut App) -> Entity<BoardEvents> {
        if !cx.has_global::<Self>() {
            let events = cx.new(|_| BoardEvents);
            cx.set_global(Self { events });
        }
        cx.global::<Self>().events.clone()
    }

    pub fn notify(project_id: ProjectId, cx: &mut App) {
        let events = Self::events(cx);
        events.update(cx, |_, cx| cx.emit(BoardChanged { project_id }));
    }
}
