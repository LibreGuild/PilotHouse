# Git

Crates: `ph-git` (operations over the `git` CLI) and `ph-changes` (the UI). Everything runs off the UI thread; repository config cannot run programs (`core.fsmonitor` and external diff tools are turned off), and network commands never wait on a terminal prompt.

## Status bar

The branch item shows the branch (or detached commit), `↑ahead ↓behind`, a merge or rebase in progress, and the running git operation. Clicking it opens the **branch picker**:

- Fetch (all remotes, prune), Pull (fast-forward only), Push (sets the upstream on first push).
- Local and remote branches, newest first, with search. Click to switch; a remote branch gets a local tracking branch. Delete (merged only) on hover.
- Typing a new name offers **Create branch from here** (names are validated with `git check-ref-format`).

The shared `Repo` model (`repo.rs`) holds status and runs one operation at a time; results show as notifications. It refreshes when the project watcher sees `.git` HEAD, index, refs, `FETCH_HEAD` or `MERGE_HEAD` change.

## The Git tab

- **Changes**
  - **Commit box**: message, **Write message** (an agent writes it from the staged diff), **Amend** (prefills the last message), Commit. Staged files, each with Unstage.
  - **Conflicts**: during a merge, rebase or cherry-pick, conflicted files are listed with **Abort**. **Resolve** opens the resolver: each block side by side with **Ours / Theirs / Both / Ask agent**; **Save and mark resolved** writes the file and stages it. Unresolved blocks keep their markers.
  - **Review**: unstaged edits per hunk. **Keep** stages (`git apply --cached`); **Discard** reverts with an undo stack (20 deep). New files can be kept.
- **History**: commits with a branch graph (lanes, merges, branch and tag badges), all branches or HEAD only, 200 per page. Clicking a commit shows its message and diff against its first parent. **Write changelog**: an agent writes release notes from the commits since the last tag.
- **Stashes**: stash everything (new files included) with an optional message; list, show diff, Apply, Pop, Drop.

## Blame

In the editor toolbar, **Blame** shows each line with its author and age (a bar, stronger for newer lines), drawn lazily for big files. The editor footer shows the cursor line's author, date and commit summary once blame is loaded.

## Agent writing

`ph_agent::oneshot` runs the default agent's CLI in its non-interactive mode (Claude Code `-p`, `codex exec`, Gemini `-p`, `agy -p=`) with the prompt on stdin, a 3-minute limit, and the answer cleaned of fences and quotes. It writes commit messages, changelogs and conflict merges.

Not yet: discarding untracked files (it deletes data, so it waits for trash support).
