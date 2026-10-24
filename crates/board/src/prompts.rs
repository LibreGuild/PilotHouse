//! Prompts PilotHouse sends to an agent on the user's behalf.
//!
//! Kept in one place so their wording can be reviewed and tuned together.

/// Asks the agent to break `goal` into board cards (it gets the tools from
/// the PilotHouse MCP server).
pub fn plan(goal: &str) -> String {
    format!(
        "Plan this work as cards on the PilotHouse board, using the `board_add_cards` tool.\n\
         Make each card a small, independently verifiable step, in the order it should be done. \
         Give every card a clear title, a short description and acceptance criteria (one per line). \
         Do not start implementing yet.\n\n\
         Goal:\n{goal}"
    )
}

/// Asks the agent to work through the To do column one card at a time.
pub fn work_through_board() -> String {
    "Work through the PilotHouse board one card at a time.\n\
     1. Call `board_next_card` to claim the next card (it moves to In progress).\n\
     2. Implement it and check its acceptance criteria.\n\
     3. If the card lists UI tests, run each with `test_run` and fix what fails.\n\
     4. Record what you did with `board_add_note`, then move it to `review` with `board_update_card`.\n\
     5. Repeat until `board_next_card` says the board is empty, then summarise what changed."
        .to_string()
}
