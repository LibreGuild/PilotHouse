# UI tests

Crate: `ph-uitest`. The **Tests** tab (⌘9).

A UI test is a scenario in plain words plus the steps that carry it out, in the browser pane or on an Android device. It lives in the project as `.pilothouse/tests/<name>.json`, so it is reviewed and shared like code.

## Making one

**New test** asks for a name, the scenario ("Enter the name Ada and sign in; the page should greet Ada") and where it starts (a URL, or an Android package). **Create and record** opens a chat that asks the agent to do the scenario with PilotHouse's `browser_*` or `device_*` tools. While it works, PilotHouse records (`record.rs`):

- every action that succeeded (open, click, type, press, scroll; launch, tap, swipe, type, key) becomes a step, with the selector or coordinates the agent used; typing into the same field again replaces the earlier step;
- looking (snapshots, screenshots, logs) is not recorded;
- checks come from the `test_expect` tool (text shown, text not shown, URL contains), which checks at once so the agent knows it holds;
- only this workspace's agents count: another workspace's actions at the same time are left out;
- text typed into a password field is never saved: the step types `{{env:PILOTHOUSE_TEST_PASSWORD}}`, which a run fills from that variable in your shell's environment (any step's typed text can use `{{env:NAME}}` the same way). A run without the variable fails that step and says which one to set.

For that turn the agent may use PilotHouse's own app tools without asking (by the tool's name; a shell command still asks). Recording stops when the agent's turn ends (or with **Stop and save**).

## Running

**Run** replays the steps without AI (`run.rs`): each action is the same control call the agent made (tried once more after a short pause if it fails, for a page still loading), each check reads the screen and waits up to 8 seconds, and every step leaves a screenshot. The first failure stops the run; the rest are skipped. A web test starts with the browser's cache cleared (logins stay), so it sees what the app serves now, and runs at a fixed desktop size (1280×800), so its screens compare from run to run. An Android test runs on the device in view. Reports and screenshots go to `<data dir>/test-runs/<workspace>/<test>/<time>/`. Click a step to see its screenshot.

**Run on every size** runs a web test at desktop (1280×800), tablet (820×1180) and phone (390×844) sizes, the last two laid out as on mobile (`browser.emulate`); **Run on every device** runs an Android test on each connected device. Each run gets its own folder and a chip above the steps; pick one to see its steps.

## How it looks

Each step's screenshot is compared with the **baseline** for that size or device (`visual.rs`). **Save as baseline** (after a run whose steps worked) keeps its screens; later runs mark a step **looks different** when more than 1% of its pixels changed, and the run fails. Click the step to see the screen and the changes in red; **Accept new look** makes the new screens the baseline. Baselines live per machine in `<data dir>/test-baselines/<workspace>/<test>/<size or device>/`, not in the project, because fonts and rendering differ between machines.

## When it breaks

- **Repair with agent**: the steps before the broken one are kept; the agent continues the scenario from where it broke, adapting to how the app looks now, and what it does is offered as a **proposed fix** to keep or discard.
- **Make a card**: puts the failure (or the steps that look different) on the board, with the size or device, the error and the report's folder.
- **Edit steps** (in **⋯**, with **Record again**): opens the JSON file.

## Asking about results

`@test-run` in a chat attaches every test's newest result (per size or device), with the error of a failing step and its report folder, plus the screen of the first failing step.

## Tests and the board

A card linked to tests can only be Done once they pass (see [board.md](board.md)). Agents check their own work with two tools: `test_list` (the tests and how each last went) and `test_run` (runs one like **Run**, at the desktop size for web tests, and says which step failed or which screens look different).

Not yet: schedules, and running a matrix in parallel (variants run one after another).
