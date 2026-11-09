# Customizing PilotHouse for your organisation

PilotHouse is Apache-2.0: you may take this repository, change it, build it and give it to your own people. This page is for the people (and the coding agents) who do that. It covers what you can change without touching PilotHouse's code, how to add features of your own, and how to keep merging new PilotHouse releases into your copy.

Everything an organisation usually changes lives in three places:

| Where | What | Who edits it |
|---|---|---|
| `brand/` | The name, ids, icons, features on or off, and where updates come from | you |
| `crates/custom/` | Your own views, commands and startup code, and the crates that hold your code | you |
| `CUSTOM.md` (you create it) | Your rules: what your build changed and what to keep when merging | you |

PilotHouse ships `brand/` with its own values, `crates/custom/` with empty lists, and no `CUSTOM.md`. It changes these only to add something new (a new key, a new kind of addition), with defaults, and says so in [CHANGELOG.md](../CHANGELOG.md). So when you merge a new release, your changes there stay as they are.

## 1. Name and icons

Edit `brand/brand.toml`:

```toml
[app]
name = "Acme Code Desk"        # what people see: menus, title bar, About, installers
vendor = "Acme"                # the installers' publisher
id = "com.acme.codedesk"       # the macOS bundle id
data_folder = "AcmeCodeDesk"   # settings, chats, logs: ~/Library/Application Support/AcmeCodeDesk
keychain = "AcmeCodeDesk"      # where GitHub tokens are kept in the system keychain

[links]
repository = "https://git.acme.example/tools/code-desk"
releases = "https://git.acme.example/tools/code-desk/releases/latest"
```

The app reads this file when it is built (`crates/brand`), and so do the packaging scripts (`scripts/package/brand.sh`), so it is the one place to change. A mistake (an unknown key, a name with a `/`) stops the build with a message saying what is wrong.

- **A new `data_folder` is a separate install.** Nothing is copied from the old folder. That is usually what you want: your build and PilotHouse can be installed side by side without touching each other's data. If your people already use PilotHouse and should keep their chats, keep `data_folder = "PilotHouse"` (and `keychain`).
- **About** says "Acme Code Desk 0.6.2 · based on PilotHouse · Apache-2.0".
- The installers and their file names follow the name: `Acme Code Desk.app`, `AcmeCodeDesk-macOS-universal.dmg`, `AcmeCodeDesk-Windows-Setup-x64.exe`, `AcmeCodeDesk-Linux-x86_64.AppImage` (the name without spaces).

**Icons** are in `brand/icons/`. Make them all from one square picture (a PNG, 1024 px or more):

```sh
pip install pillow
python3 scripts/package/icon.py my-icon.png                       # on a Mac, to make app.icns too
python3 scripts/package/icon.py my-icon.png --title-bar mark.png  # a separate mark for the title bar
```

That writes `app-1024.png` (Linux), `app-256.png` (in the app), `title-bar.png` (the title bar, drawn at 20 px), `app.ico` (Windows) and `app.icns` (macOS). `app.svg` and `app-tile.svg` are PilotHouse's drawings; the app does not use them.

**Names that stay `pilothouse`.** These are read by other programs, written into your people's repositories or stored in their data, so they keep their names in every build:

- the `pilothouse` program and its subcommands (`ctl`, `mcp`, `agy-acp`, `agent-hook`, `chrome-host`)
- the MCP server agents see (`pilothouse`, tools named `mcp__pilothouse__…`)
- `pilothouse.toml` and `.pilothouse/` in repositories
- the `refs/pilothouse/*` Git refs and checkpoint commits
- the `<!-- pilothouse-… -->` markers in pull request comments
- `PILOTHOUSE_*` environment variables and `TERM_PROGRAM=PilotHouse` in terminals
- the slide deck data inside `.pptx` files
- the Chrome extension, which is named PilotHouse. To give it your name, change `crates/browser/extension/manifest.json` (and its `key`, which sets its id), `EXTENSION_ID`, `HOST_NAME` and `GROUP_TITLE` in `crates/browser/src/extension.rs`, and `HOST` and `GROUP_TITLE` in `crates/browser/extension/background.js`. Or switch the extension off (below).

## 2. Features on or off

Add the features your people should not have to `[features]` in `brand/brand.toml`:

```toml
[features]
browser = false
devices = false
logcat = false
github = false
```

A feature that is off is not in the app at all. It is not in the tools beside the chat, the menus, the command palette, Settings or the keyboard shortcuts sheet. Its shortcuts do nothing, and agents do not get its MCP tools. Missing keys mean on, so a feature added in a new PilotHouse release arrives switched on in your build until you list it here.

| Key | What |
|---|---|
| `files`, `changes`, `pulls`, `board`, `browser`, `devices`, `runs`, `tests`, `terminal`, `logcat` | The tools beside the chat (`changes` is Git; `pulls` is pull requests; `devices` is Android; `runs` is run configurations and ports; `tests` is UI tests) |
| `github` | The GitHub view beside Workspaces (agent reviews of pull requests run there too) |
| `schedules` | Schedules: agent prompts and test runs at set times or on events |
| `utilities` | The offline utilities (formats, hashes, times…) and their agent tools |
| `usage` | The usage dashboard |
| `network_drives` | Network drives: Git, shells and runs on a server over SSH |
| `parallel_runs` | One prompt to several agents, each in its own copy |
| `presentations` | Slides in Studio, and their agent tools |
| `chrome_extension` | Agents in the user's own Chrome, through PilotHouse's extension |

Your people can still switch tools off for themselves in *Settings → Features*; that list only shows the tools your build has.

## 3. Updates

PilotHouse looks for new versions of itself on GitHub and its mirrors. A build of your own should either not look, or look at your own releases.

**No updates** (you hand out new versions yourselves):

```toml
[updates]
enabled = false
```

Settings → About then has no Updates section (it still links `[links] repository`), the app never asks a server, and the release workflow publishes no `latest.json`.

**Your own releases:** list where they are, and use your own signing key, since updates are only used when `latest.json` is signed with the key the app carries.

```toml
[updates]
enabled = true
key = "update_key.pub"          # in brand/: the public half of YOUR key
[[updates.sources]]
name = "Acme releases"
manifest = "https://git.acme.example/tools/code-desk/releases/latest/download/latest.json"
files = "https://git.acme.example/tools/code-desk/releases/download/"
```

Make a key, put its public half in `brand/update_key.pub`, and keep the private half as the `UPDATE_SIGNING_KEY` secret of your release workflow:

```sh
openssl genpkey -algorithm ed25519 -out key.pem
openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64 > brand/update_key.pub
```

`scripts/package/latest-json.py` names the files by your first source, and `scripts/package/sign-manifest.sh` refuses a key that is not the one in `brand/`. See [packaging.md](packaging.md).

## 4. Your own features

### A view of your own beside Workspaces and GitHub

Put your code in a crate of its own, say `crates/acme-reports` (it is part of the workspace by being under `crates/`). Add it to `crates/custom/Cargo.toml`, and list your view in `crates/custom/src/lib.rs`:

```rust
pub fn views() -> Vec<CustomView> {
    vec![CustomView {
        id: "reports",
        label: "Reports",
        icon: || ph_ui::custom_icon("reports"),
        build: |context, window, cx| acme_reports::view(context.store.clone(), window, cx),
    }]
}
```

`build` returns any GPUI view (`cx.new(|cx| ReportsView::new(...)).into()`). The view then shows:

- in the switch at the top left (Workspaces | GitHub | Reports), filling the window below the title bar;
- in the command palette as *Show the Reports view*.

From your own code, `window.dispatch_action(Box::new(ph_custom::ShowView { id: "reports".into() }), cx)` opens it.

### Commands in the command palette

```rust
pub fn commands() -> Vec<CustomCommand> {
    vec![CustomCommand {
        label: "Open the team wiki",
        icon: || ph_ui::Icons::BookOpen.into(),
        keywords: &["wiki", "docs"],
        run: |_, _, cx| cx.open_url("https://wiki.acme.example"),
    }]
}
```

They are listed under *Custom*.

### Icons of your own

Put SVG files in `brand/icons/custom/` (`reports.svg`; names in lowercase letters, digits, `-` and `_`) and draw one with `ph_ui::custom_icon("reports")`. Icons drawn like Lucide's (24 × 24, `stroke="currentColor"`) take the theme's colours. Any of [Lucide's icons](https://lucide.dev/icons) is also available: add its name to the `ExtraIcons` list in `crates/ui/src/assets.rs` and use `ph_ui::Icons::Name`.

### Your settings and data

- **Settings:** `context.store.setting::<T>(key)` and `set_setting(key, &value)` store any JSON value. Start your keys with `x.<your name>.` (`x.acme.reports.range`), so they never meet one of PilotHouse's.
- **Tables:** keep them on a migration track of your own, never in PilotHouse's list (`crates/store/src/migrations.rs`, whose numbering your build must not change):

  ```rust
  const ACME_MIGRATIONS: &[&str] = &[
      "CREATE TABLE x_acme_reports (id INTEGER PRIMARY KEY, title TEXT NOT NULL)",
      // Later releases of your build append here; never edit one that shipped.
  ];
  store.apply_track("acme", ACME_MIGRATIONS)?;
  ```

  Call it from `init` in `crates/custom/src/lib.rs`, which the app runs once at startup (also the place for globals and key bindings of your own). Only the migrations a database does not have yet run, each in a transaction, after a backup of the database. Name your tables `x_<track>_…`.

### Deeper changes

Some changes need PilotHouse's own code, such as a new tool beside the chat (like Board or Terminal) or a new Settings page. Make them, and list each file you changed in `CUSTOM.md`, so whoever merges the next release knows to keep them. A new tool beside the chat touches:

- `crates/workspace/src/context_panel.rs` (the `ContextTab` list)
- `actions.rs` (a `Show…` action and its key)
- `workspace.rs` (its handler)
- `workspace_screen/panels.rs` and `render.rs` (making and drawing the panel)
- `workspace_screen/dock.rs` (its saved name)
- `shortcuts.rs`

Copy what `Board` does in each.

## 5. Your rules: `CUSTOM.md`

Create `CUSTOM.md` at the repository root. Coding agents read it first: [AGENTS.md](../AGENTS.md) tells them to, and to keep what it lists when they merge. A starting point:

```markdown
# Acme Code Desk: what this build changes

This is Acme's build of PilotHouse (upstream: https://github.com/LibreGuild/PilotHouse).
When merging a new PilotHouse release, keep everything listed here.

## Brand
- brand/brand.toml: name Acme Code Desk, data folder AcmeCodeDesk, updates from git.acme.example.
- brand/icons/: our icon (made with scripts/package/icon.py from design/icon.png).
- brand/update_key.pub: our release key.

## Features switched off
browser, devices, logcat, github: not used at Acme.

## Our additions
- crates/acme-reports: the Reports view (listed in crates/custom/src/lib.rs).
- Settings under x.acme.*; tables on the `acme` migration track.

## Changes to PilotHouse's own files
- .github/workflows/release.yml: release notes text, runs on our runners.
- (every other upstream file we changed, and why)

## Rules for agents working here
- Never send code or data outside git.acme.example.
- Run the checks in AGENTS.md before every commit.
```

Keep it current: ask your agent to update it whenever it customises something.

## 6. Building and shipping

- **Checks:** `cargo fmt --all`, `cargo clippy --workspace --all-targets --features pilothouse/automation -- -D warnings`, `cargo test --workspace --features pilothouse/automation`. The tests work with your brand.
- **Installers:** `scripts/package/macos.sh`, `windows.ps1` and `linux.sh` ([packaging.md](packaging.md)). They read the name, vendor, id and icons from `brand/`.
- **CI:** `.github/workflows/ci.yml` runs as it is. In `release.yml`, the mirror job and in `pages.yml` the site run only in PilotHouse's own repository, so they skip in yours. Edit the release notes text in `release.yml` (it describes PilotHouse's downloads) and set the `UPDATE_SIGNING_KEY` secret if your build updates itself.
- **Signing:** PilotHouse's builds are not code-signed. If you have an Apple Developer ID or a Windows code-signing certificate, sign in your release workflow.
- **Keep `version` in `Cargo.toml` as PilotHouse's** (it is the version you merged), so merges do not conflict there and the Chrome extension's version check holds. Tell your releases apart by their tag or release notes.

## 7. Merging a new PilotHouse release

PilotHouse publishes a tag per release (`v0.7.0`), one commit each. To take one in:

```sh
git remote add upstream https://github.com/LibreGuild/PilotHouse.git   # once
git fetch upstream --tags
git switch -c merge-v0.7.0
git merge v0.7.0
```

Then, or ask your coding agent to (the `merge-upstream` skill in `.claude/skills/` walks it through):

1. Read the release's **Merge notes** in [CHANGELOG.md](../CHANGELOG.md): new `[features]` keys, new brand keys, renamed files, changes to `ph-custom`.
2. Resolve conflicts:
   - Keep your values in `brand/` and your lists in `crates/custom/`.
   - Take PilotHouse's version of everything else, then re-apply the changes `CUSTOM.md` lists.
   - In `Cargo.lock`, take either side and let `cargo` fix it.
3. Switch off any new feature you do not want (`[features]`).
4. Run the checks (section 6), then run the app on a throwaway profile (`PILOTHOUSE_DATA_DIR=/tmp/ph-profile cargo dev`) and look at what you changed.
5. Merge the branch.

The fewer of PilotHouse's own files you change, the smaller every merge is. Prefer `brand/`, `crates/custom/` and crates of your own.

## 8. The licence

Keep `LICENSE` and `NOTICE` in what you ship (the macOS app, the Windows installer and zip, the `.deb` and the tarball include them), and say in changed files that they were changed (Apache-2.0, section 4). PilotHouse's name and icon are its own; a build with its own name and icon makes clear whose build it is.
