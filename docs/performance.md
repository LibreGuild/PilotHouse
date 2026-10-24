# Performance

Performance is a feature. These budgets are checked by `scripts/perf.sh`, which exits non-zero when one is missed.

| Metric | Budget | Measured (M4 Pro, macOS 26, release) |
|---|---|---|
| Binary size | < 50 MB | 20 MB |
| Start to interactive | < 500 ms | ~280 ms |
| Idle RAM, welcome screen | < 150 MB | 101 MB |
| Idle RAM, project + file tree + editor | < 150 MB | 124 MB |
| Idle CPU | < 1 % | 0.5 % |

RAM counts the PilotHouse process only. Agents, shells, Chrome and emulators are separate processes.

Live panes (informational, same machine):

| Situation | PilotHouse |
|---|---|
| Devices tab showing an emulator, screen still | ~115 MB RAM, 0.4 % CPU |
| Emulator screen moving (shown at up to 30 fps) | ~11 % of one core, mostly GPU texture upload and repaint |
| Browser started | no change in PilotHouse; headless Chrome adds its own processes (several hundred MB), only once the Browser tab or a browser tool is used |

```sh
cargo build --release -p pilothouse
scripts/perf.sh                      # binary, startup, idle RAM, idle CPU
```

The first launch of a freshly built binary on macOS is slower (about 1.5 s) because the OS scans new executables once. Run the script twice.

## Rules that keep the numbers

- **Lazy everything**: panels, watchers, shells and agent processes start on first use.
- **Coalesce bursts**: terminal output, logcat and file-system events are batched into one redraw or refresh.
- **Stream only what is seen**: browser and device frames flow only while their pane is visible in the shown project, at most one frame is in flight, and replaced GPU textures are freed right away.
- **One copy per frame**: frames are converted to BGRA once, on the decoding thread, inside the image GPUI draws; the UI thread only swaps a pointer. Live panes redraw at most ~30 times a second, and the newest frame always wins, so a busy UI never queues frames or shows a stale one.
- **Bounded memory**: recordings stream to disk; logs, logcat and requests are capped; idle hidden browsers stop after 10 minutes.
- **Ignore what no one shows**: the file watcher drops `.gitignore`d paths (`target/`, `node_modules/`…), and the Changes panel folds event bursts into one `git` run.
- **Off the UI thread**: file reads, directory listings, search, git, fuzzy matching and agent I/O.
- **Virtualize long lists**: the chat transcript (`MessageScroller`), the file tree and ⌘P results render only what is visible.
- **Cap pathological inputs**: files over 4 MB open as a notice, diffs are cut at 80–300 lines per block, search stops at 500 hits and the file index at 200k files.
