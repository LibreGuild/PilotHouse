# Development

## Requirements

- Rust (stable, edition 2024). Xcode command-line tools on macOS; full Xcode is **not** needed (GPUI compiles its shaders at runtime).
- `git` on the PATH (the Changes tab uses it).
- For agents: the CLIs you want to use (`claude`, `codex`, `gemini`, `agy`) and Node.js (`npx`) for the Claude Code and Codex adapters.

## Build and run

```sh
cargo dev                 # run with the dev automation socket (recommended)
cargo run -p pilothouse   # run without automation
cargo build --release -p pilothouse
```

`cargo dev` / `cargo dev-build` are aliases in `.cargo/config.toml`. They always use the same feature set, so builds reuse each other's artifacts.

Use a throwaway profile instead of your real data:

```sh
PILOTHOUSE_DATA_DIR=/tmp/ph-profile cargo dev
```

Logs go to stderr; `RUST_LOG=ph_agent=debug cargo dev` shows agent stderr too.

## Saving disk space

- One shared `target/` directory. Avoid git worktrees with their own `target/`.
- Dependencies build without debug info (`[profile.dev.package."*"] debug = false`); our crates keep line tables for backtraces.
- A full debug and release build together use about 8 GB. `cargo clean -p <crate>` rebuilds just one crate.

## Conventions

These follow the gpui-kit coding guide:

- **One crate per feature.** Put a feature's model, views and commands in its crate. The shell (`workspace`) composes features; features never depend on it.
- **Colors from the theme only**, read via `cx.theme()`. Raw colors belong in `theme/themes/pilothouse.json` (and the terminal's xterm palette).
- **Layout uses rem helpers** (`p_2()`, `gap_3()`). The few pane widths live in `theme/src/layout.rs`.
- **Stable element ids** come from domain ids (`("board-card", card.id)`), never list positions for reorderable rows.
- **No blocking work in `render`.** Disk, git and processes run on background tasks.
- **Every icon-only button gets an `accessibility_label`**. Screen readers and the automation harness both depend on it.
- Comments say why, not what. Every module starts with a `//!` summary.

Before sending a change:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --features pilothouse/automation
cargo test --workspace --features pilothouse/automation
```
