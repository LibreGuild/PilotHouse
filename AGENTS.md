# Notes for coding agents

PilotHouse is one native Rust desktop app (GPUI through `gpui-kit`) for working with coding agents. Everything runs on the user's machine: no backend, no account, no telemetry. This file is what an agent needs before changing the code; the details are in `docs/`.

## If this is an organisation's build

Organisations take this repository, change it for their own people and keep merging new PilotHouse releases into it. **If `CUSTOM.md` exists at the repository root, read it first**: it lists what this build changed and why, and its rules win over anything here. Keep it up to date when you customise something, and keep everything it lists when you merge a new PilotHouse release. [docs/customizing.md](docs/customizing.md) is the guide; in short:

- **Name, ids, icons, features on or off, updates:** `brand/brand.toml` and `brand/icons/` only. The app reads them through `crates/brand` (`ph_brand::NAME`, `ph_brand::enabled(Feature::…)`); the packaging scripts through `scripts/package/brand.sh`. Never write the product's name into code: use `ph_brand::NAME`, or `ph_brand::name!()` inside `concat!` where a `&'static str` is needed.
- **The organisation's own features:** crates of their own (`crates/acme-…`), wired in only through `crates/custom` (`views`, `commands`, `init`). Their settings keys start with `x.<name>.`; their tables are on a migration track of their own (`Store::apply_track`), never in `crates/store/src/migrations.rs`.
- **Merging a PilotHouse release:** the `merge-upstream` skill (`.claude/skills/merge-upstream/SKILL.md`) and the *Merge notes* in `CHANGELOG.md`.

## Where things are

- [docs/architecture.md](docs/architecture.md): the crates, how they depend on each other, the main data flows. One crate per feature under `crates/`; `crates/workspace` is the window shell that composes them; `crates/app` is the binary.
- [docs/development.md](docs/development.md): building, conventions. [docs/styleguide.md](docs/styleguide.md): UI rules.
- [docs/testing.md](docs/testing.md): unit tests, UI tests in a headless window, and driving the running app.
- `docs/features/*.md`: one short page per feature. Update the page for what you change.

## Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --features pilothouse/automation -- -D warnings
cargo test --workspace --features pilothouse/automation
PILOTHOUSE_DATA_DIR=/tmp/ph-profile cargo dev   # the app, with the automation socket
```

Always run the app on a throwaway data folder (`PILOTHOUSE_DATA_DIR`), never the user's own. With the app running, `pilothouse ctl` drives it (see docs/testing.md).

## Rules

- **One crate per feature.** A feature's model, views and commands live in its crate; the shell composes features, and features never depend on the shell.
- **A feature the brand can switch off** is listed in `ph_brand::Feature` (and `FEATURES` in `crates/brand/build.rs`), and every way into it checks it: its tool or view, its palette and menu entries (`crates/workspace/src/features.rs` maps actions to features), its dialogs, its MCP tools (`crates/mcp/src/tools/mod.rs`). A new PilotHouse feature that can be left out gets a key there, on by default.
- **Colours come from the theme** (`cx.theme()`); raw colours belong in `crates/theme/themes/pilothouse.json`. Layout uses rem helpers; pane widths live in `crates/theme/src/layout.rs`.
- **Stable element ids** from domain ids, never list positions. **Every icon-only button gets an `accessibility_label`**: screen readers and the automation harness use it.
- **No blocking work in `render`**: disk, git and processes run on background tasks.
- **Database migrations are append-only** (`crates/store/src/migrations.rs`): add one at the end, never edit or reorder one that shipped.
- **Settings** are JSON values by key (`Store::setting`, `Store::set_setting`); fields a build does not know are kept (`Preferences::other`).
- **Changes an organisation's build depends on** (`brand.toml` keys, `ph_brand`, `ph_custom`, `Store::apply_track`) keep working for one release when they change, and get a line in the *Merge notes* of `CHANGELOG.md`.
- Only permissively licensed dependencies (Apache-2.0, MIT, BSD, ISC, Zlib).
- Comments say why, not what. Every module starts with a `//!` summary.

## Names that stay as they are

These are read by other programs, written into users' repositories or stored in their data, so renaming them breaks something. Keep them, also in a rebranded build:

- the `pilothouse` binary and its subcommands (`ctl`, `mcp`, `agy-acp`, `agent-hook`, `chrome-host`, `quick-chat`)
- the MCP server name `pilothouse` (and the `mcp__pilothouse__` tool prefix agents see)
- `pilothouse.toml` and `.pilothouse/` in repositories, and the `pilothouse.desktop` entry
- the `refs/pilothouse/*` Git refs and checkpoint commits (author `PilotHouse`, `PilotHouse-Untracked:` trailer)
- the `<!-- pilothouse-…` markers in pull request comments and the `(Workspace note from PilotHouse` prefix in transcripts
- `PILOTHOUSE_*` environment variables and `TERM_PROGRAM=PilotHouse`
- the slide deck schema and parts inside `.pptx` files
- the Chrome extension's name, id, native host and `PilotHouse.*` messages
