# Explanations

Crates: `ph-explain` (the views), `ph-review` (`explanation`: the prompt, the shared file, the lookup), `ph-store` (`explanations`), `ph-agent` (`explanations`: what the chats were asked). Behind the `explanations` feature (`brand/brand.toml`).

Agents write the code and agents review it. What gets lost on the way is the developer's own picture of what happened. An explanation gives it back: an agent reads the change and the code around it, and tells you in plain words how this part worked before, what is different now, why it was done this way, what was checked, and what is worth a word with your team. It is asked for; nothing is written by itself.

## Explain, in the Git tab

**Explain** (over the changes, beside *Review*) has an agent explain the work that is not committed yet. It is written in a chat of its own that only reads (the chat beside the Git tab, where you can ask it more), and shows in place of the diff: over the pane, **Changes | Explanation** switches between the two ways to read the same work.

- **What it says** (`ph_review::explanation::prompt::SECTIONS`): *In short*; *How it worked before* (for someone who has never seen this code); *What changed*, in the order that is easiest to follow, each step with a code reference that opens the place in the editor; *Why this way*; *Checked and not checked*; *Worth a word with the team* (what reaches beyond the change or is hard to undo: a migration, an API, a new dependency, a number nobody asked for); *Look closely here*; *Words used here*. A section with nothing to say is left out.
- **How long**: as long as a two-minute read allows, by the size of the change. A small change (up to 60 changed lines) gets at most 150 words, an ordinary one 300, a large one 500, a very large one 650.
- **The "why" comes from your chats.** The code cannot say why something was done. So the prompt carries what the chats that made the change were asked, how the agent summed up its work, and what it ran (`ph_agent::explanations::asked_about`). A chat's request belongs to a change when its turn wrote or removed one of the change's lines, so work committed in another order than it was done is still matched. What a turn wrote is read off the diffs of its edit calls, and, for an agent that edits through shell commands (a heredoc, `sed`, a script) and leaves no diff in its chat, off the checkpoints taken before each request: the files are compared between one checkpoint and the next. The agent is told this is a record, not instructions, and that the code is right where the two differ. With no record (hand-written work, an agent in a terminal), it says where it is guessing.
- **It only reads.** The chat that writes an explanation creates and edits no file, and is told to run nothing but commands that read: no tests, no build. *Checked and not checked* says what the record shows was run, not what the explaining agent could run itself.
- **It says what it is doing.** Before an agent starts, the change and the chats that made it are read (seconds on a large change or a slow disk): the pane says so, then that the agent (named) is writing. A second press meanwhile starts nothing.
- **Pressed again**, *Explain* shows the explanation there is. An agent is run again only by **Again**, or when the work has moved on since.
- **When the work moves on**, the explanation says so (*The work moved on since (2 files): ask again for it*) and stays readable.
- **Several pieces of work at once**: one explanation, each piece under a heading of its own. Commit them one by one and each commit finds its part of the story (see below).
- **Copy** copies all of it as Markdown.

Explanations follow the changes, not the chat: one chat that builds three things is not three chapters to keep apart. What gets explained is what waits in Git.

## It stays with the commit

Once the work is committed, its explanation is found from the commit: in **History**, a commit's detail has the same **Changes | Explanation** switch. A commit nobody explained offers *Explain this commit* there (an agent explains it as it was made, "before" meaning the commit before it).

No commit id is kept for this. An explanation is filed under **the content it explains**: each file's path with the Git blob id of its content (`explanation_files`). A commit is known by the content it put in the repository (`ph_git::content::of_commit`). Where the two meet, the explanation is that commit's (`ph_review::explanation::find`). So nothing breaks when a commit is amended, rebased, cherry-picked or squashed, and nothing has to happen at the moment of committing, whoever commits (you, an agent, a terminal).

## Why Is This Here?

In the editor's right-click menu. For the line at the cursor it looks up, without running an agent (`ph_explain::look_up`):

| The line | What you get |
|---|---|
| was written by a commit that has an explanation | its title and *In short*, who wrote it and when, the request that asked for it (where the chat is on this computer, with *Open the chat*), and **Read all of it** |
| is not committed yet | the request in a chat that wrote it, and the explanation of the work in progress if there is one, else *Explain these changes* |
| was written by a commit nobody explained | what Git knows (who, when, the commit's first line), **Explain this commit**, and the newest explanation of the file, marked as being about another change |

Which explanation is "the newest" for a line is never decided here: Git knows which commit last wrote the line (`git blame -w -M -C`, so a commit that only reformats or moves code does not take the line), and that commit's content picks the explanation. A line a teammate rewrote yesterday shows theirs; the lines around it that they left alone still show the older one. An older explanation is never shown as the answer for a newer change. One whose files have changed since says *Parts of this changed since*, and names the newer explanation when there is one.

*Explain* in the same menu is something else: an agent reads the code and says what it does. *Why Is This Here?* says who asked for it and why, from what was written down then.

## Sharing one with the team

An explanation stays on this computer (the store) until you press **Add to commit** on it. That writes it as one Markdown file under `.pilothouse/explanations/` (`2026-11-20-forgot-password-emails.md`), to commit with your changes: it shows in the list as a new file like any other (kept with them, when you have kept files for the commit), and **Take out** removes it again. Nothing is ever written there without that click, and there is no setting that makes it automatic: it is written from your chat, so you read what you share.

- **The file is the explanation as it reads**, so it is as readable on GitHub as in the app. What it explains stands in a comment at its end (`<!-- pilothouse-explanation-of`: when, who, the agent, each file's blob id and path), which Markdown viewers do not show (`ph_review::explanation::file`).
- **A teammate who pulls** finds it the same way a local one is found, by content: History and *Why Is This Here?* read the repository's folder of explanations as well as the store. The chat does not travel, only the explanation.
- **One file per explanation**, each under a name of its own: two people's never conflict.
- **A squash that carries several** shows, for a line, the one that explains that file.
- **An old commit** can be explained later and shared the same way: its file names the content, not the commit.

## Trying the prompt

The wording is the feature: an explanation nobody reads is worth nothing. `prompt.rs` says who reads it, gives the length, and spends most of its words on how to write (everyday words, what the product does rather than what the diff does, no praise, the words that give machine writing away, a sample of each voice). To try a change to it on real agents and real changes without the app, see [testing](../testing.md#the-explanation-prompt).

## Code map

| File | What |
|---|---|
| `crates/review/src/explanation/prompt.rs` | The prompt: sections, length by size, how to write, the chats' record |
| `crates/review/src/explanation/mod.rs` | Reading the agent's reply (between two marker lines), title and *In short* |
| `crates/review/src/explanation/file.rs` | The shared file: its text, its name, reading the folder |
| `crates/review/src/explanation/find.rs` | Lookup by content: of a commit, of files, what changed since, the newer one |
| `crates/git/src/content.rs` | Blob ids of the work not committed and of a commit, a line's origin, a change's lines, what changed between two checkpoints |
| `crates/store/src/explanations.rs` | The explanations kept on this computer (migration 18) |
| `crates/agent/src/explanations.rs` | What the chats were asked, by the lines a change touches (edit calls, else checkpoints); word that one was written |
| `crates/agent/src/thread.rs` | `ReviewTurn::Explanation`: the turn's reply is saved, or given up when it has none |
| `crates/explain/src/view.rs` | The explanation beside the changes and on a commit |
| `crates/explain/src/why.rs` | *Why Is This Here?* |
| `crates/workspace/src/workspace_screen/explain.rs` | Where they meet the chat, the Git panel and the editor |

## Not yet

- A line an agent wrote in a terminal has no request to show while it is not committed: its session is the agent's own file, with no checkpoints of ours.
- Explanations are read from the working tree's folder, so one that was deleted from the repository later is no longer found.
- A commit that reformats very much of a file can still take lines from the commit that wrote them, where Git's own tracking of moved and reformatted lines gives up; those lines then say nobody explained them.
