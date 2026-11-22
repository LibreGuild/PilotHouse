# Tests

Crate: `ph-uitest`. The **Tests** tab (⌘9).

One list for both kinds of test a project has: its **own tests** (`cargo test`, pytest, Vitest…), run with the project's command and read back one line per test, and **UI tests**, which check a flow in the browser pane or on an Android device. The header says how many passed, failed and never ran; **Run all** runs the project's suites, then the UI tests, one at a time; **Stop** ends what runs and forgets what waits.

## The tab

- **The list** (left): *Project tests* with each suite and, under it, its tests grouped as the runner groups them (a crate, a class, a file), failures first; then *UI tests*. A suite that just ran opens at what failed. *Find a test* narrows the list; **Failed** shows only what failed. A row's ▶ (under the pointer) runs it. The arrow keys move through the list.
- **What is picked** (right): a suite, one of its tests, or a UI test.
- **A narrow pane** shows the list alone, and a picked row takes its place (*‹ Tests* leads back). Rows are drawn only while in view (`uniform_list`), so a suite of thousands of tests stays quick.
- **⋯** has *Add a test command…* and *Look for tests again*.

A run stays in the tab: the browser pane is not brought in front (replay calls say `quiet`). **Watch** on a running UI test shows the browser or the phone.

## The project's own tests

Found without running anything (`suite.rs`): the project folder and the folders right under it (a `web` beside an `api`) are read for `Cargo.toml`, `package.json`, `pytest.ini` and friends. A workspace's parts are not listed twice (a root `cargo test --workspace` or `gradlew test` runs them).

| Found by | Runs | Results read from |
|---|---|---|
| `Cargo.toml` | `cargo test --workspace --no-fail-fast` | what it prints (`suite_read::libtest`) |
| `pytest.ini`, `conftest.py`, `tests/test_*.py`… | `python3 -m pytest` (`uv run`, `poetry run` or `.venv` when there) | a JUnit file (`--junitxml`) |
| `vitest` in `package.json` | `npx vitest run` (pnpm, yarn or bun by lockfile) | a JUnit file |
| `jest` | `npx jest` | Jest's JSON (`--json`) |
| `@playwright/test` | `npx playwright test` | a JUnit file |
| `build.gradle(.kts)` | `./gradlew test` (`testDebugUnitTest` for an Android app) | `build/test-results/<task>/*.xml` |
| `pom.xml` | `mvn test` | `target/surefire-reports/*.xml` |
| `go.mod` | `go test -json ./...` | its events |
| `npm test` of another runner, .NET, Flutter, Dart, Deno, Mix, RSpec | the command | its exit code only: one line, passed or failed, with its output |

- A suite's detail shows its command, how the last run went, and the end of its output (**Copy**). A test's detail shows why it failed and **where** (`src/lib.rs:42`, a click opens it): the place is taken from the failure's own text, past the runner's and the libraries' frames; a JVM trace's bare file name is looked for in the project.
- **Run failed** runs only what failed, **Run again** on a test only that one: by name where the runner takes names (cargo, pytest, Gradle, Maven, Go), by file where it takes files (Vitest, Jest, Playwright), which the button says. A partial run replaces only the tests it ran.
- **Fix with agent** starts a chat with the command, the failed tests, why and where (`panel/suites.rs`): it is asked to read before changing, and to fix the test only when the test is what is wrong.
- **Add a test command…** for what was not found: it runs in the project folder and passes when it exits without an error (`tests.commands.<workspace>` in settings).
- The command runs through `/bin/sh` (`cmd` on Windows) with the login shell's environment, in a process group of its own and with no signal blocked (a child keeps its starter's blocked signals, and the app's background threads block most), so **Stop** ends it and what it started: asked first, ended after three seconds (`suite_run.rs`).
- Results are kept in `<data dir>/test-runs/<workspace>/_suites/<suite>/result.json`.

Not read yet: .NET, Flutter and the others in the table's last row show as one line. Go's reader is written from the format's documentation and tested on samples; Go was not installed where it was written.

## UI tests

A UI test is what it should check, in plain words, plus the steps that do it. It lives in the project as `.pilothouse/tests/<name>.json`, so it is reviewed and shared like code.

### Where it runs

In the built-in browser, not the user's own Chrome and not through Playwright: it needs no setup, is the same size every time, and holds none of the user's sign-ins (a test clicking through a browser that is signed in to real accounts is neither safe nor repeatable). A team that runs tests elsewhere **exports** a test (below).

### Making one

**From a chat or a CLI session**: say what to check ("make a UI test: sign in as Ada, add a coffee, the cart should say Coffee") and the agent makes it. It calls `test_record` (what the test checks, and the page or app it starts at), does the flow with its browser or device tools, adds checks with `test_expect`, and ends with `test_save`, which says the steps; `test_run` then shows that it replays. The test is in the Tests tab from the first call, with its recording going. A recording an agent in a terminal forgot to end is saved when the next one starts, or with **Stop and save**.

**From the tab**: **+** asks *What should it check?* ("Sign in as Ada, add a coffee to the cart; the cart should say Coffee"). *Where it starts* is filled in: the page the browser pane shows (or where the tests there are start), or the app in front on the phone. The name comes from the first words unless one is typed. **Record with agent** opens a chat that asks the agent to do it with PilotHouse's `browser_*` or `device_*` tools. While it works, PilotHouse records (`record.rs`):

- every action that succeeded becomes a step. **A step names its element as a person would**, not by where it stands: a page's by what it is and says (`"role": "button", "name": "Sign in"`), with the words of the row or card around it when several say the same (`"within": "Coffee"`), which the browser works out when the click lands (`scripts.rs`: `describe`); a phone's by its words or id, which the device reads off the screen before the tap. The selector or the point the agent used is kept beside that and only used when there is no name: a hook made for tests (`data-testid`), an element's own id;
- looking (snapshots, screenshots, logs) is not recorded, nor are the calls of a test the agent runs meanwhile;
- checks come from the `test_expect` tool (text shown, text not shown, URL contains, and `look`: how the screen looks here), which checks at once so the agent knows it holds;
- only this workspace's agents count;
- **a password typed is never saved in the file**: it goes to this computer's credential store (the Keychain, Credential Manager; `ph_store::secrets`) and the step types `{{secret:password}}`. A password the user wrote into what the test should check is taken out of that text too. On another computer the first run stops at that step and **Enter it** asks for it. `{{env:NAME}}` is still read from the shell's environment, for variables meant for tests (TEST or E2E in the name).

For that turn the agent may use PilotHouse's own app tools without asking, `test_list` and `test_run` among them. Recording stops when the agent's turn ends (or with **Stop and save**). It starts where every run starts (next).

### Running

**Run** replays the steps without AI (`run.rs`):

- **From a clean start.** What the test's sites keep (cookies and sign-ins, storage) is erased first (`browser.clear_sites`), so a test that signs in does not find itself signed in the second time; other sites keep theirs. An app is stopped and started from its first screen with its task cleared (`Adb::launch_fresh`); its data is only erased when the test asks (*Erase the app's data before each run*), since it may be the one build on the user's phone. Both are a switch in the test's **⋯** (`"clean"` in the file).
- **Elements are waited for**, up to 10 seconds: until one of that name shows, is switched on, has come to rest and nothing covers it (`scripts.rs`: `resolve`). Several of the same name with nothing to tell them apart is a failure that says so, not a click on the first. On a phone a tap waits for its element (never another row that shares its id), and typing waits for a field to have the focus. A point recorded on one screen is moved to another's size.
- **Checks** read all the page says (or the words on the phone's screen), as a reader does (line breaks and case do not matter), and wait up to 8 seconds.
- Every step leaves a screenshot. The first failure stops the run; the rest are skipped.
- **The app is not running**: a page on this computer that nothing answers at fails before anything is tried, and says so, with **Open Run**.
- A web test runs at a fixed desktop size (1280×800); an Android test on the device in view. Reports and screenshots go to `<data dir>/test-runs/<workspace>/<test>/<time>/`; the newest 20 runs of a test are kept.

**Run on every size** (in **⋯**) runs a web test at desktop, tablet (820×1180) and phone (390×844) sizes; **Run on every device** an Android test on each connected device. Each starts clean, gets its own folder and a chip above the steps.

The detail shows the steps in plain words ("Click the “Add” button by “Coffee”") with how each went, and the screen after the step that is picked: beside the steps in a wide pane, under the step in a narrow one, at the size its shape needs. A step's **⋯** removes it or adds a look check after it.

### How it looks

A **look check** (`visual.rs`) compares the screen with the picture accepted for that check. Only where the test asks: comparing every step said "different" for a clock and nothing for a misspelt heading.

- Any change counts, down to a dozen pixels; the page is stilled first (no animation, no text cursor) and a phone's status bar is left out.
- A run without an accepted picture passes and offers **Accept this look**. One that differs shows *Now*, *Accepted* and *Changes* (the changes in red), and offers **Accept new look** or **Ignore what changed**: the latter leaves out, from now on, the whole element that changed (a clock, not the one digit that differed this time), kept in the step as `ignore`.
- Accepted pictures are kept per machine and per size or device in `<data dir>/test-baselines/<workspace>/<test>/<size or device>/<check>.png`, not in the project: fonts and rendering differ between machines.

### When it breaks

- **Repair with agent** (for a failure of the test itself, not a missing password or an app that is not running): the steps before the broken one are kept; the agent goes on from where it broke, and what it does is offered as a **proposed fix** to keep or discard.
- **Edit steps (JSON)** and **Record again** are in **⋯**, with **Delete this test**. A file edited outside the app is read again before the app changes it.

### For a build server

**Export as a Playwright test** writes `e2e/<name>.spec.ts`; **Export as a Maestro flow** writes `.maestro/<name>.yaml` (`export.rs`). The file says the same steps in that runner's words (`getByRole`, `tapOn`), with the same screen size, look checks as the runner's own screenshot checks with the left-out areas covered, and secrets as environment variables. It is written again at each export and says so in its first line; a file that does not start with that line is left alone. An exported Playwright spec then shows under *Project tests* like any other.

## For agents

`test_record` and `test_save` (make a UI test, above), `test_expect` (a check, while recording), `test_list` (the UI tests and how each last went, and the commands that run the project's own tests) and `test_run` (runs a UI test like **Run** and says which step failed or which looks differ). Tests of code are the project's own: an agent writes them in the project's framework, and the tab finds them. `@test-run` in a chat attaches how every test last went: the suites with their failures, the UI tests with the failing step's screen.

A schedule can run the UI tests ([schedules.md](schedules.md)).

## Limits

- UI tests cover pages and Android apps. A screen whose elements say nothing of themselves (a game, a canvas, a Flutter view without semantics) is tapped by its place, scaled to the screen.
- Variants of a matrix run one after another.
- The Maestro export is checked against Maestro's documented format, not by running Maestro.
