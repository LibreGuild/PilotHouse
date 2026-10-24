# Run configurations and ports

Crate: `ph-runs`. The **Run** tab.

## Run configurations

- **Detected** from the project root, without running anything: `package.json` scripts (with npm, pnpm, yarn or bun, chosen by lockfile), cargo, go, Django `manage.py`, pytest, Gradle (including `installDebug` for Android apps), Docker Compose, and `Makefile` and `justfile` targets. Script names come from the repository, so unusual ones are shell-quoted.
- **Your own**: **Add** a name and a command. These are stored per project in the database, not in the repository.
- **Run** opens a terminal tab named after the configuration and types the command, so the output stays and Ctrl-C works. Running it again stops the previous run first. Running a configuration turns the Terminal tab on if it was hidden.

## Ports

Listening TCP ports from `lsof`, refreshed every 3 s while the tab is visible. Ports opened by processes started in this project folder come first, marked **this project**; **Show more** lists the rest. Each row can **open in the browser pane**, copy its address, or stop the process (SIGTERM).

Agents get `ports_list`, to find the dev server before `browser_open`.

Not yet: Windows (no `lsof`).
