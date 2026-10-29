# Git

Crates: `ph-git` (operations over the `git` CLI) and `ph-changes` (the UI). Everything runs off the UI thread; repository config cannot run programs (`core.fsmonitor` and external diff tools are turned off), and network commands never wait on a terminal prompt.

## Status bar

The branch item shows the branch (or detached commit), `↑ahead ↓behind`, a merge or rebase in progress, and the running git operation. Clicking it opens the **branch picker**:

- Fetch (all remotes, prune), Pull (fast-forward only), Push (sets the upstream on first push).
- Local and remote branches on two tabs (**Local**, **Remote**, each with its count), newest first, with search. Click to switch; a remote branch gets a local tracking branch. Delete (merged only) on hover.
- Typing a new name offers **Create branch from here** (names are validated with `git check-ref-format`).

The shared `Repo` model (`repo.rs`) holds status and runs one operation at a time; results show as notifications. It refreshes when the project watcher sees `.git` HEAD, index, refs, `FETCH_HEAD` or `MERGE_HEAD` change.

## The Git tab

- **Changes**
  - **Commit box**: message, **Write message** (an agent writes it from the staged diff), **Amend** (prefills the last message), Commit. Staged files, each with Unstage.
  - **Conflicts**: during a merge, rebase or cherry-pick, conflicted files are listed with **Abort**. **Resolve** opens the resolver: each block side by side with **Ours / Theirs / Both / Ask agent**; **Save and mark resolved** writes the file and stages it. Unresolved blocks keep their markers.
  - **Review**: unstaged edits as one row per file: its name and folder, `+added −removed`, and what happened to it (**A** new, **M** modified, **D** deleted, **R** renamed). Clicking a row opens its diff in the editor (below); on hover, the diff button, Open, Discard and Keep. The chevron unfolds the file's hunks right there, to keep or discard one at a time. The list is drawn only where it shows (a row per file and per hunk), so many files with long diffs scroll smoothly. **Keep** stages (`git apply --cached`); **Discard** reverts with an undo stack (20 deep). New files can be kept.
  - **All changes** opens every changed file's diff as one, new files included (as added; binary files and ones over 512 KB are left out).
  - **Review notes**: hovering a line of a hunk offers **+**: a note on that line (of the file as it is now). Notes show under their lines; a bar counts them and **Send to chat** gives them all to the chat in front as one attachment, each with its file, line and code (`notes.rs`), asking the agent to address them.
  - **Review** (over the changes): an agent reviews what is not pushed yet; its findings come back as notes on their lines, and the bar's button becomes **Fix with agent**. See [pull requests](pulls.md#review-your-changes-before-pushing).
- **Branch**: what the branch checked out changed against the default branch (`origin/HEAD`, else `main` or `master`) since it left it, file by file (`ph_git::diff::branch_changes`). With the turn summary in chat (one turn) and Changes (uncommitted), it gives three scopes of review.
- **History**: commits with a branch graph (lanes, merges, branch and tag badges), all branches or HEAD only, 200 per page. Clicking a commit shows its message and diff against its first parent. **Write changelog**: an agent writes release notes from the commits since the last tag.
- **Stashes**: stash everything (new files included) with an optional message; list, show diff, Apply, Pop, Drop.

## Diffs in the editor

A file's changes, or all of them, open in the editor's **Changes** tab, first in its tab strip (`ph_files::diff_tab`, gpui-component's diff viewer). **Unified** shows one column; **Split** shows the file before and after side by side, long lines wrapping so both sides stay in view. Syntax colours, line numbers, files folding by their headers. The refresh button reads the changes again. The Files tree's **Compare with last commit** opens here too, against `HEAD`. One Changes tab is kept: opening other changes replaces what it shows.

## Blame

In the editor toolbar, **Blame** shows each line with its author and age (a bar, stronger for newer lines), drawn lazily for big files. The editor footer shows the cursor line's author, date and commit summary once blame is loaded.

## Agent writing

`ph_agent::oneshot` runs the default agent's CLI in its non-interactive mode (Claude Code `-p`, `codex exec`, Gemini `-p`, `agy -p=`) with the prompt on stdin, a 3-minute limit, and the answer cleaned of fences and quotes. It writes commit messages, changelogs and conflict merges.

Not yet: discarding untracked files (it deletes data, so it waits for trash support).
