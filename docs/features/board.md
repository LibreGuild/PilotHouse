# Board

Crate: `ph-board` (UI and control methods). Data: `cards`, `card_events` and `card_tests` tables (`ph-store`).

There are four columns: **To do → In progress → Review → Done**. Each card has a title, a description, acceptance criteria (one per line), an assignee and an activity log.

- **Add card**, click a card to edit or delete it, and drag cards between and within columns. Drop a card on another card to insert it above.
- **Work through board** sends the agent a prompt (`prompts.rs`) to take To do cards one by one.
- Every change, from the UI or an agent, goes through the store, and `BoardHub` refreshes every open board.

## Done needs tests

A card can list **UI tests to pass before Done** (in its editor: *Link a test*; a card made from a failing test links that test). Its tile shows how many. Moving it to Done, by dragging, in the editor, or by an agent, is refused until each test's newest run passed (steps worked and screens match the baseline); the refusal names the tests and how they last went (`may_finish` in `lib.rs`, `ph_uitest::gate`). Cards without tests move freely.

## Agent tools

Every agent session gets the PilotHouse MCP server (`pilothouse mcp`, crate `ph-mcp`). It offers these tools:

| Tool | Does |
|---|---|
| `board_list` | List all cards |
| `board_add_cards` | Append cards to To do (planning) |
| `board_next_card` | Claim the top To do card: it moves to In progress, assigned to the agent |
| `board_update_card` | Edit fields or move to `todo`, `in_progress`, `review` or `done` |
| `board_add_note` | Add to the card's activity log |
| `test_list`, `test_run` | List the workspace's UI tests, or run one and get what failed (see [tests.md](tests.md)) |

Cards from `board_list` and `board_next_card` include the tests they need, and *Work through board* asks the agent to run them before moving a card on.

To use the board from an agent started outside PilotHouse (PilotHouse must be running):

```sh
claude mcp add pilothouse -- /path/to/pilothouse mcp --agent claude
```

The project is the agent's working directory, or `--project PATH`.

Card positions are floats with gaps, so moving a card never renumbers a column (`store/src/board.rs`).
