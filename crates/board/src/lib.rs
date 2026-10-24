//! The project board.
//!
//! A Kanban board per project with four columns (To do, In progress, Review,
//! Done). People edit it in [`BoardPanel`]; coding agents edit it through the
//! PilotHouse MCP server, whose tools call the `board.*` control methods
//! registered by [`register_control_methods`]. Either way the change goes
//! through the store and [`BoardHub`] tells every open board to refresh.
//!
//! A card can list UI tests it needs; it reaches Done only once their newest
//! runs passed ([`may_finish`]), whoever moves it.

mod board_panel;
mod card_editor;
mod control;
mod hub;
mod plan_dialog;
pub mod prompts;

pub use board_panel::{BoardPanel, BoardPanelEvent};
pub use control::register_control_methods;
pub use hub::BoardHub;

use ph_store::Store;
use ph_store::board::{CardId, CardStatus};

/// Whether card `id` may move to `status`: Done needs every test it lists
/// to have passed its newest run. The error says which do not.
pub fn may_finish(store: &Store, id: CardId, status: CardStatus) -> anyhow::Result<()> {
    if status != CardStatus::Done {
        return Ok(());
    }
    let Some(card) = store.card(id)? else {
        return Ok(());
    };
    let tests = store.card_tests(id)?;
    match ph_uitest::gate::blocking(card.project_id, &tests) {
        Some(why) => anyhow::bail!(why),
        None => Ok(()),
    }
}
