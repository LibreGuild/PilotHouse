# Git

Crates: `ph-git` (operations over the `git` CLI) and `ph-changes` (the UI). Everything runs off the UI thread; repository config cannot run programs (`core.fsmonitor` and external diff tools are turned off), and network commands never wait on a terminal prompt.

## Status bar

The branch item shows the branch (or detached commit), `↑ahead ↓behind`, a merge or rebase in progress, and the running git operation. Clicking it opens the **branch picker**:

- Fetch (all remotes, prune), Pull (fast-forward only), Push (sets the upstream on first push).
- Local and remote branches on two tabs (**Local**, **Remote**, each with its count), newest first, with search. Click to switch; a remote branch gets a local tracking branch. Delete (merged only) on hover.
- Typing a new name offers **Create branch from here** (names are validated with `git check-ref-format`).

The shared `Repo` model (`repo.rs`) holds status and runs one operation at a time; results show as notifications. It refreshes when the project watcher sees `.git` HEAD, index, refs, `FETCH_HEAD` or `MERGE_HEAD` change, or a file of the working tree, and once when the window comes back to the front. While files keep changing (an agent writing), reads follow each other with a pause of 0.3 s, never back to back. The Changes list (`changes_panel.rs`, three `git` runs, one a whole `git diff`) is read on every change only while the Git tool is on screen (`Attention::Shown`); with only its count showing on the icon bar it is read at most every 5 seconds (`Glance`), and not at all while its workspace is not in front (`Away`), then once when it is again. A read that finds the same changes draws nothing.

## The Git tab

Its four views are a row of tabs that fits the room (`ph_ui::PillTabs`): with space for every name they all show; in a narrow column the ones not in front fold to their icons, their names in tooltips. The list's header does the same with its named button (*Review*), and the commit row moves **Commit** to a line of its own. The tool is named **Git** over the result pane (it was *Changes* for a while, the same word as its first view).

- **Changes**
  - **Commit box**: message, **Write message** (an agent writes it from the staged diff), **Amend** (prefills the last message), Commit. Staged files, each with Unstage.
  - **Conflicts**: during a merge, rebase or cherry-pick, conflicted files are listed with **Abort**. **Resolve** opens the resolver: each block side by side with **Ours / Theirs / Both / Ask agent**; **Save and mark resolved** writes the file and stages it. Unresolved blocks keep their markers.
  - **The list**: unstaged edits as one row per file: a tick for *viewed*, its name and folder, `+added −removed`, and what happened to it (**A** new, **M** modified, **D** deleted, **R** renamed). Clicking a row shows its changes in the review beside the list (below); its right-click menu has Show Changes, Open File, Keep, Discard Changes and Add to .gitignore. New files are listed under *New files*; one picked there shows in the review with every line added. *View as Tree* groups the rows by folder.
  - **What stands behind the changes**: over the list, what the chat in front did to check its work, read off its tool calls (`ph_agent::evidence`): each test command's last result (`14 tests passed`, red when it failed; the command in the tooltip), the other commands it ran and how many failed, and the screenshots it took.
  - **Review notes**: a bar counts the notes written on lines of the review (below) and **Send to chat** gives them all to the chat in front as one attachment, each with its file, line and code (`notes.rs`), asking the agent to address them.
  - **Review** (over the changes): an agent reviews what is not pushed yet; its findings come back as notes on their lines, and the bar's button becomes **Fix with agent**. See [pull requests](pulls.md#review-your-changes-before-pushing).
- **Branch**: what the branch checked out changed against the default branch (`origin/HEAD`, else `main` or `master`) since it left it, file by file (`ph_git::diff::branch_changes`). With the turn summary in chat (one turn) and Changes (uncommitted), it gives three scopes of review.
- **History**: commits with a branch graph (lanes, merges, branch and tag badges), all branches or HEAD only, 200 per page. Clicking a commit shows its message and diff against its first parent. **Write changelog**: an agent writes release notes from the commits since the last tag.
- **The branch picker** (the branch in the status bar) always offers a new branch under its search: with no name yet it says to type one; a name that is not a branch offers *Create branch “name” from current* (Enter does it too; spaces become dashes); a name Git would refuse says why (`new_branch_name`). Enter on a name that finds one branch switches to it.
- **Stashes**: stash everything (new files included) with an optional message; list, show diff, Apply, Pop, Drop.

## Diffs in lists

The diffs under a file in the Git tab, in commit details, stashes, a pull request's files, `.diff` files and a chat's summary of an edit are drawn by one piece of code (`ph_preview::diff`): the line numbers of the old and the new file, `+` or `−` in a column of its own, lines tinted by kind, and where a line replaces another the words that changed marked more strongly (a renamed variable shows as that word, not as two lines to compare by eye; `similar`'s word diff, skipped for lines that share too little). A long line neither wraps nor is cut: a file's diff scrolls sideways as one, with a scrollbar (a swipe, Shift with the wheel, or the bar; the wheel alone stays the list's, so a list of diffs scrolls on over a long line). Comparing words costs time, so it is done for the lines that are drawn (a hunk's first 300), not for lines over 400 bytes (a minified file), and each comparison is remembered while the app runs. In the Git tab, a line's review note button shows over its line number.

## The review beside the list

`changes_panel/review.rs`. Beside the list of changed files (under it when the pane is narrower than 720 px; the line between them drags) stands **one diff of every changed file** (gpui-component's diff viewer: *Unified* or *Split*, syntax colours, unchanged lines folded, **↑ ↓** for the previous and next change). The list picks, the diff shows: a row unfolded to its hunks in the list before, and a click on it opened the same changes once more in the editor, with keep, discard and notes in only one of the two.

- **Viewed.** A file's tick (on its row, and before its path in the diff) says its changes were looked at: the file folds away in the diff, as on a pull request's page, and *N of M viewed* counts them. A change made to the file after takes the tick off (the tick is kept with a fingerprint of the file's changes). Ticks are kept while the workspace's screen is open.
- **On a file**, after its path: **Undo** (discards its changes; *Undo discard* over the list brings them back) and **Keep** (stages it: it leaves the changes and is ready to commit).
- **On a hunk**, under each one of a file with several: **Keep these lines** and **Discard these lines**. The diff comes back to where the hunk was.
- **A note on a line**: the **+** beside a line (or at the end of a selection of lines) asks for a note; it shows under its line and counts in the list's bar. A line the changes removed takes one too (the agent is told it is a removed line).
- **While an agent writes**, the list follows it; the diff does not, or the lines being read would jump away. It says *The files changed since this was drawn* and **Show them** draws it anew, as does picking a file. What you do yourself (keep, discard, refresh) shows at once. Nothing is offered on the hunks of a diff that is behind.
- The patch drawn is the one `git diff` gave for the list (`WorkingChanges::patch`): Git is not asked a second time. A new file's lines are read when it is picked.

## Diffs in the editor

A file's changes since the last commit (*Compare with last commit*, from the file's menu) and two files compared open in the editor's **Changes** tab, first in its tab strip (`ph_files::diff_tab`, the same viewer), read when opened and again with its refresh button.

## Blame

In the editor toolbar, **Blame** shows each line with its author and age (a bar, stronger for newer lines), drawn lazily for big files. The editor footer shows the cursor line's author, date and commit summary once blame is loaded.

## Agent writing

`ph_agent::oneshot` runs the default agent's CLI in its non-interactive mode (Claude Code `-p`, `codex exec`, Gemini `-p`, `agy -p=`) with the prompt on stdin, a 3-minute limit, and the answer cleaned of fences and quotes. It writes commit messages, changelogs and conflict merges.

Not yet: discarding untracked files (it deletes data, so it waits for trash support).
