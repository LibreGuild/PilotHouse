# Run configurations and ports

Crate: `ph-runs`. The **Run** tab.

## Run configurations

- **Detected** from the project root, without running anything: `package.json` scripts (with npm, pnpm, yarn or bun, chosen by lockfile), cargo, go, Django `manage.py`, `main.py` and `app.py`, pytest, Maven (`mvnw` when there is one; `spring-boot:run` for Spring Boot), .NET (`dotnet run`, `build`, `test` for a project or solution), CMake (configure and build), Flutter and Dart, Deno tasks, Gradle (Android: install and *Build debug APK*; a JVM application: `gradle run`), Docker Compose, and `Makefile` and `justfile` targets. Script names come from the repository, so unusual ones are shell-quoted.
- **Your own**: **Add** a name and a command. These are stored per project in the database, not in the repository.
- **Added by an agent**: ask in a chat or a CLI session ("add a run configuration that starts the API with the test database") and the agent adds it with `run_add` (`configs.rs`): a name, a command of one line and, when it is not the project folder, a folder of the project, which the row says (*in web*). It shows in the Run tab at once, also while the tab is open. The agent only adds it: **you** start it with its button, where its command is in view, and adding one asks for your permission like any change of your setup. Adding one of the same name changes it; `run_remove` takes an added one away; `run_list` says those found and those added.
- **Run** opens a terminal tab named after the configuration and types the command once the new shell has shown its prompt (a shell still loading its profile, PowerShell on a slow computer, would drop what is typed before), so the output stays and Ctrl-C works. Running it again stops the previous run first. Running a configuration turns the Terminal tab on if it was hidden.

## Android apps

When a folder is a Gradle project with an app module (`com.android.application`), the tab starts with an **Android app** card (`ph_devices::AndroidRun`), like Android Studio's run bar:

- **Module** (when there are several: the app modules among the projects `settings.gradle(.kts)` includes, so a folder left over from before is not offered), **build variant** (product flavors per dimension times build types, read from `build.gradle(.kts)` without running Gradle; `debug` first) and **device** (the same menu as the Devices pane, with emulators to start and *Connect a device*).
- **Build APK** builds the picked module and variant for every ABI with no device needed, then shows the APK's name with buttons to show it in its folder or copy its path, as Android Studio's *Build APK(s)*.
- **Run** builds `:<module>:assemble<Variant>` with the project's wrapper, installs the APK on that device, stops the app if it runs and starts it, then shows the Devices pane. A status line says what it is doing and for how long (all of a long message on hover); **Build log** shows Gradle's output with errors in red, and opens by itself when the build fails. **Stop** ends the build, or the app.
- **What was chosen is kept** per project (module and variant, setting `android.run:<folder>`). The variants are read from the build files, not by running Gradle, so flavors that a plugin of the project's own sets up are not seen: *Other…* in the variant menu takes a variant by its name.
- **The run hands the app to Logcat**: the device's log starts with the run, the app counts as the project's (`package:mine`), and a crash is said on the card and in a notice that offers the stack to the chat ([devices.md](devices.md#logcat)).
- **When something fails**: a failed build has **Send to chat** (the end of its log). A device that refuses the APK because another build of the app is on it (another signature, a newer version) is said so, with **Uninstall and run again**. A device older than the app's `minSdk` is said before anything is built.
- **The app's menu** (⋯) does on the chosen device what is otherwise typed after `adb shell`: *Force stop*, *Clear data*, *Grant its permissions*, *Open a link…* (a deep link into the app), *App info on the device* and *Uninstall*. It is the app run last, else the one the chosen variant built.
- The plain Gradle tasks for the same (`installDebug`, `assembleDebug`) are not listed among the run configurations while the card is there.
- **Fast like Studio's builds**: Gradle is told the device's ABIs (`android.injected.build.abi`), so native code is built for that device only, and the Gradle daemon is reused between runs. Gradle runs on Android Studio's bundled JDK when one is installed (as Studio does), since a newer JDK on `PATH` can break the build. The APK and application id come from the plugin's own listing (`output-metadata.json`, following Studio's redirect file), so per-variant suffixes need no parsing.

## Ports

Listening TCP ports from `lsof`, refreshed every 3 s while the tab is visible. Ports opened by processes started in this project folder come first, marked **this project**; **Show more** lists the rest. Each row can **open in the browser pane**, copy its address, or stop the process (SIGTERM).

Agents get `ports_list`, to find the dev server before `browser_open`, and `run_list`, `run_add` and `run_remove` for the configurations (above).

On Windows the ports come from `netstat`, without the working folder, so none is marked as this project's. In a workspace on a [network drive](network-drives.md) the ports are the server's, and opening one goes through an SSH tunnel.
