# Run configurations and ports

Crate: `ph-runs`. The **Run** tab.

## Run configurations

- **Detected** from the project root, without running anything: `package.json` scripts (with npm, pnpm, yarn or bun, chosen by lockfile), cargo, go, Django `manage.py`, pytest, Gradle, Docker Compose, and `Makefile` and `justfile` targets. Script names come from the repository, so unusual ones are shell-quoted.
- **Your own**: **Add** a name and a command. These are stored per project in the database, not in the repository.
- **Run** opens a terminal tab named after the configuration and types the command, so the output stays and Ctrl-C works. Running it again stops the previous run first. Running a configuration turns the Terminal tab on if it was hidden.

## Android apps

When a folder is a Gradle project with an app module (`com.android.application`), the tab starts with an **Android app** card (`ph_devices::AndroidRun`), like Android Studio's run bar:

- **Module** (when there are several), **build variant** (product flavors per dimension times build types, read from `build.gradle(.kts)` without running Gradle; `debug` first) and **device** (the same menu as the Devices pane, with emulators to start and *Connect a device…*).
- **Run** builds `:<module>:assemble<Variant>` with the project's wrapper, installs the APK on that device, stops the app if it runs and starts it, then shows the Devices pane. A status line says what it is doing and for how long; **Build log** shows Gradle's output with errors in red, and opens by itself when a step fails. **Stop** ends the build, or the app.
- **Fast like Studio's builds**: Gradle is told the device's ABIs (`android.injected.build.abi`), so native code is built for that device only, and the Gradle daemon is reused between runs. Gradle runs on Android Studio's bundled JDK when one is installed (as Studio does), since a newer JDK on `PATH` can break the build. The APK and application id come from the plugin's own listing (`output-metadata.json`, following Studio's redirect file), so per-variant suffixes need no parsing.

## Ports

Listening TCP ports from `lsof`, refreshed every 3 s while the tab is visible. Ports opened by processes started in this project folder come first, marked **this project**; **Show more** lists the rest. Each row can **open in the browser pane**, copy its address, or stop the process (SIGTERM).

Agents get `ports_list`, to find the dev server before `browser_open`.

On Windows the ports come from `netstat`, without the working folder, so none is marked as this project's. In a workspace on a [network drive](network-drives.md) the ports are the server's, and opening one goes through an SSH tunnel.
