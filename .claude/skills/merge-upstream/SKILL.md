---
name: merge-upstream
description: Merge a new PilotHouse release into an organisation's customised copy of PilotHouse, keeping the organisation's brand, features, additions and the changes CUSTOM.md lists. Use when asked to "merge upstream", "update to PilotHouse vX.Y.Z", "take the new PilotHouse release" or similar in a repository that is a copy of PilotHouse.
---

# Merging a PilotHouse release into an organisation's build

The organisation's changes live in `brand/`, `crates/custom/`, its own crates (`crates/<org>-…`), and the PilotHouse files that `CUSTOM.md` lists. PilotHouse never edits `brand/` values or `crates/custom/`'s lists, so most merges only conflict where `CUSTOM.md` says the organisation changed PilotHouse's own files. Read [docs/customizing.md](../../../docs/customizing.md) section 7 if anything here is unclear.

## 1. Before merging

1. Read `CUSTOM.md` at the repository root. If there is none, stop and ask the user what this build changed: you need it to merge safely. Offer to write one from `git log` and `git diff <last-merged-tag>..HEAD --stat`.
2. Make sure the working tree is clean (`git status`). Do not merge over uncommitted work.
3. Find the release to merge. Ask the user if they did not say. Fetch it:
   ```sh
   git remote get-url upstream || git remote add upstream https://github.com/LibreGuild/PilotHouse.git
   git fetch upstream --tags
   ```
4. Find the last release already merged: `git describe --tags --abbrev=0 --match 'v*'` (or ask).
5. Read the *Merge notes* of every release between the two in the new `CHANGELOG.md` (`git show vX.Y.Z:CHANGELOG.md`). Note:
   - new `[features]` keys (new features arrive switched on);
   - new or renamed `brand.toml` keys;
   - changes to `ph_custom` or `Store::apply_track`;
   - moved files that `CUSTOM.md` mentions.

## 2. Merge

```sh
git switch -c merge-vX.Y.Z
git merge vX.Y.Z
```

Resolve each conflict:

| Path | Keep |
|---|---|
| `brand/brand.toml` | The organisation's values. Add any new keys from the release, with the release's default unless `CUSTOM.md` says otherwise. |
| `brand/icons/*`, `brand/update_key.pub` | The organisation's files. |
| `crates/custom/**` | The organisation's lists and `init`, adapted to any API change the merge notes describe. |
| `crates/<org>-*` | The organisation's crates; update them for API changes in PilotHouse crates they use. |
| `CUSTOM.md` | The organisation's. |
| A PilotHouse file that `CUSTOM.md` lists | The release's version, with the organisation's change re-applied on top, as `CUSTOM.md` describes it. |
| Any other PilotHouse file | The release's version. |
| `Cargo.lock` | Either side; `cargo build` fixes it. |
| `Cargo.toml` `version` | The release's (the organisation keeps PilotHouse's version). |

Never renumber or edit `crates/store/src/migrations.rs`: take the release's version as it is. The organisation's tables are on its own track.

## 3. After merging

1. If the release added features the organisation does not want (`CUSTOM.md`, or ask), switch them off in `[features]` of `brand/brand.toml`.
2. Check it builds and passes:
   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets --features pilothouse/automation -- -D warnings
   cargo test --workspace --features pilothouse/automation
   ```
   Fix what the merge broke. Prefer changing the organisation's code over PilotHouse's.
3. Run the app on a throwaway profile and look at what the organisation changed. Check four things: the name in the title bar and About; the switched-off features gone; the organisation's views and commands working; the new release's features. Close it when done.
   ```sh
   PILOTHOUSE_DATA_DIR=/tmp/ph-profile cargo dev
   ```
4. Update `CUSTOM.md` if anything it lists moved or changed, and add the merged release to it ("Merged up to PilotHouse vX.Y.Z").
5. Commit the merge with a message naming the release and summarising any conflicts and how they were resolved. Do not push or open a pull request unless the user asks.

## Report

Tell the user:

- which release was merged;
- which files conflicted and how each was resolved;
- which new features arrived and whether they are on;
- the check results;
- anything you could not verify.
