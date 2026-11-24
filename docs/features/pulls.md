# Pull requests, issues and agent reviews

Crates: `ph-github` (API, accounts, pull requests, issues, search), `ph-pulls` (UI), `ph-review` (review template, how findings are written, parsing, re-review, posting). GitHub.com and GitHub Enterprise Server work the same way.

## Accounts (Settings → GitHub)

- One account per host: `github.com` or any Enterprise host (`github.example.net` → API `https://HOST/api/v3`; an API URL can be set for unusual setups).
- Paste a personal access token (scopes `repo`, `read:org`; **Create a token** opens the host's token page). PilotHouse checks it (`GET /user`) and shows who it belongs to.
- Tokens live in the system keychain (macOS Keychain, Windows Credential Manager); on Linux, and in debug builds, in a file only the user can read in the private data folder. Never in SQLite, logs or chats. Requests go only to hosts you added; a token is never sent to another host (checked per request, including pagination links).
- TLS uses the operating system (native-tls with the system's roots), so company CAs installed on the machine work.

## Reviews

*Reviews* in the column of icons (⌘⇧G): pull requests across every repository, by whose turn it is, and what agents make of them, apart from any workspace (`crates/workspace/src/github_view/`; the code calls it the GitHub view). It is for whoever reviews for a team as much as for an author: a workspace's own pull requests are in its *Pull requests* tool.

- **Inbox** (the list), three lists read with one search each (`ph_github::search`, GitHub's GraphQL search, which brings what the rows show: size, checks, reviews, who was asked; a server that does not answer it is asked the plain search, and the rows show less):
  - **Needs my review**: open pull requests that ask for your review, or a team of yours.
  - **Waiting for author**: those you reviewed, and that do not ask for you again. Once you review a pull request GitHub no longer lists it as asking for you, so without this list it was gone from sight until you remembered it.
  - **My pull requests**: those you opened.
- The number beside each list is of what **needs you now**: every one to review; those whose author pushed since your review; your own with changes requested, checks failing or a conflict.
- **Read while the app runs.** With an account, the lists are read when the app starts and every five minutes after (three searches per account), so the number on the *Reviews* icon is right before the view is opened. A new request for review is announced with a system notification (Settings → GitHub → *Tell me*, on unless turned off).
- **An agent can review each new request by itself** (Settings → GitHub → *Have an agent review it*, off unless turned on): the default agent and review template, on Reviews' own copy, reading only. Nothing is sent to GitHub until you finish the review.
- **Repositories** (the list): those your workspaces' folders come from (their remote, for a host with an account), and any you add with **+** (`owner/name` on github.com, else on your only account's host, or the repository's URL; GitHub confirms the token can see it). Added ones are kept in `github.repositories` and can be taken out of the list from their menu.
- **The middle** shows an inbox list as a queue (below), or a repository's pull requests and issues and one of them in full (the same lists and pages as a workspace's PRs tool, without *New* and *Check out*, which need a checkout of yours).
- **The chat** on the right is the open pull request's (it shows while one is open, and folds away with the second switch in the title bar, ⌥⌘B, to give the page the room; the first, ⌘B, folds the list): ready to type into, with *Security review*, *Test plan* and your own templates as starters. (The page's own *Review with agent* button and *Walkthrough* tab are the other two shipped templates, so they are not offered twice.) The first message of a new chat clones the repository if needed and checks the PR out (the chat says so while it does), then sends. Its header lists that PR's earlier chats. A file a chat names opens in a window of its own (the view has no editor).
- **Chats here only read.** They work in PilotHouse's copy of the PR: reads, searches and read-only commands (also chained with `&&`, `;` or `|`, like `cd … && git log | head`) run without asking; edits, deletes and moves are refused without asking, and the agent is told the chat is read-only. Anything else asks. Antigravity can only run a command by going on with every permission for the rest of the turn, so that always asks you. Changes are made from a workspace.

### Templates (Settings → Review templates)

What agents are asked to do with a pull request (`ph_review::templates`). Shipped: *Review this PR* and *Security review* (reviews with findings), *Explain this PR* and *Test plan* (answers). Each can be edited, put back as shipped, or taken out of the list and brought back; *New template* adds your own, making either a review or an answer.

The page shows the list or one template, never both (`templates_view.rs`): a click on a row opens it with the page to itself, *← Templates*, *Cancel* and *Save* over its fields, so they are in reach wherever the text is scrolled to. The instructions box gets the rows the page has room for (`editor_rows`, from the window's height), and scrolls its own text beyond that.

- A **review** template's text is the reviewing guide: what to look for, what each severity means (critical: must not merge; major: fix before merging; minor: worth fixing; nit: polish), and that every finding needs **evidence** (the quoted lines, the input or call path) and a **fix**. PilotHouse adds the pull request, the repository's own rules from `.pilothouse/review.md` on the base branch (never the PR's head, which its author controls), the earlier findings to re-check, and the answer format the findings card reads, so a template changes how a review is done but never breaks the card.
- An **answer** template's text is a request (the shipped *Explain this PR* asks for a summary, how it works drawn as Mermaid diagrams, a file-by-file table, what to look at and how to try it). PilotHouse adds where the PR is checked out and how to see its diff.

### Where it works: the managed clones

```
<data dir>/github/<host>/<owner>/<name>/repo    a full clone (default branch)
<data dir>/github/<host>/<owner>/<name>/pr-12   pull request 12's head (a worktree of the clone)
```

`ph_pulls::clones`. The user's checkouts are never touched. The first review of a repository clones it; later ones fetch. Git reaches the host over HTTPS with the account's token, given to each call in the environment as a header for that host only (`GIT_CONFIG_*` setting `http.https://HOST/.extraHeader`): it is never on a command line, in a remote URL or in a config file. The clone is a full one, so a review's agent reads history without the network. Each repository has a space of its own (`projects.kind = 'github'`, named `owner/name`, its folder the clone) that holds its chats, so they have what workspace chats have (agents, the PilotHouse tools, usage); it is not a workspace and is not listed as one. The folder of a pull request that was merged or closed is removed the next time the repository's list is read (`clones::prune_closed`; the commits it was reviewed at are kept, for a reopened one).

## The PRs tool (⌘8)

- The project's remote (`origin`, else `upstream`, read before `insteadOf` rewrites) gives host, owner and repo; the account for that host is used.
- **Pull requests** and **Issues**, as two lists of the repository (below for issues).
- Open PRs, newest activity first, with CI state (check runs and statuses), labels, draft and "Review requested". Filters: Open, Mine, To review.
- **New** opens a PR for the current branch (pushes it first if needed): title, base (the repository's default branch, filled in a moment after the form opens: Git is asked off the UI thread), description (**Write description**: an agent writes it from the branch's commits and diff), draft.
- **Review with agent** switches to Reviews, where reviews run.

## Issues

`ph_pulls::issues_view`, `ph_github::issues`. A repository's issues beside its pull requests: **Open**, **Assigned to me** and **Closed** (the hundred most recently updated; GitHub lists pull requests as issues too, and they are left out), with a box to find one once the list is long.

An issue is work waiting to be done, so its page offers doing it first:

- **Fix in a task** starts a [task](workspace.md) (a branch of its own, from a workspace's checkout of the repository when there is one, else from Reviews' copy, made if need be) whose chat is given the issue and its comments, and told that a pull request saying `Fixes #n` closes it.
- **Copy for an agent** puts the issue and its comments on the clipboard as Markdown, for a chat that is already open.
- Its text and comments (Markdown), a box to comment, and under **⋯**: *Assign it to me* (or take it off), *Refresh*.
- **New** writes an issue in a dialog: title, description, and **who it is assigned to** (the people GitHub says can be assigned in that repository, you first; a box to find one when there are many). An assignee GitHub refuses is an error, since who it goes to was chosen.

The issue's text is its reporter's: the agent is told it is a report to act on, never instructions beyond what it asks to be fixed.

## A pull request

Four tabs, in this order: *Conversation*, *Findings*, *Walkthrough*, *Files*. The page opens on the conversation, as the host's own does; a row of the queue that has findings to read opens it on *Findings*. Four names fit beside a chat (the commits and the checks, which were two more tabs, are in the conversation's).

- **Conversation**: the description, the commits (folded to one line), the conversation (Markdown) and reviews with their state, a box to comment (⌘↵ sends), and what stands before the merge (see [Before the merge](#before-the-merge)).
- **Findings**: what agent reviews found, kept across rounds (see [Findings](#findings)). The tab shows how many are open; opening it reads the author's ticks and replies.
- **Walkthrough**: a briefing on the change for whoever reviews it (see [Walkthrough](#walkthrough)).
- **Files**: each file's diff with what is said about its lines where they are (see [Files](#files)).
- **The header** has one filled button, what comes next: **Review with** the agent chats start with (named on the button) while nobody reviewed it, **Finish review** once there is something to send (it counts the findings and comments that wait) or an agent's review here has nothing on GitHub yet. *Finish review* is on every open pull request, your own included. On your own pull request with reviewers' comments, **Address comments** is beside it. A word says when checks fail or still run; a click goes to them. The rest is under **⋯** (*Review again with an agent*, *Watch*, *Check out this branch*, *Refresh*); **Watching** shows beside it while it is on.
- **Watch**: PilotHouse looks at the pull request every three minutes and tells you (a system notification) when the author pushes or answers a finding; answers are applied. A pull request is watched from the moment you finish its review, since its author's answer is what you wait for. Merged or closed pull requests stop being watched.
- **Address comments** (your own pull requests): a [task](#fix-in-a-task) on the branch whose agent works through the reviewers' comments not yet resolved, changes what is right and drafts a reply for each.
- **Check out** (in a workspace's PRs tool): fetches the PR head and switches to a local `pr/<number>` branch.
- **Keys**: a click anywhere on the page gives it the keyboard. **j** and **k** go to the next finding (or file) and the one before, **v** ticks the file as read; ⌘↵ in a box does what its button does.

### Finish review

`ph_pulls::finish_view`, `ph_review::github::finish`. Nothing of a review leaves the computer until this: findings can be reworded or set aside, and comments written on the files wait (kept in the settings, so a restart does not lose them). The sheet decides, in one place, what goes to GitHub:

- **How it ends**: *Comment*, *Approve* or *Request changes* (on your own pull request, GitHub takes only a comment, and the sheet says so). Approving while the agent's critical or major findings are open is said, not stopped.
- **A note** to the author (optional).
- **Your comments** on lines, each with a cross to delete it.
- **The agent's findings**, each with a tick. Those at least *minor* are ticked to begin with; nits, the noise authors complain of, are left for you to add (*Tick:* all, minor, major or critical and above, kept for next time). **Ticked findings are sent; the others stay on this computer**, open, marked *not sent*.
- **A summary comment** the author can tick findings off in (on unless unticked, kept for next time).

- **A review that found nothing** is still worth a word to the author. When nothing the agent found is open any more (it found nothing, or all is dealt with), *Post the result as a comment* (ticked unless unticked, kept for next time) posts the summary comment alone: what was reviewed, *Looks good*, nothing open. No GitHub review is made of it, so it works on your own pull request too. It is not offered while a finding is open that only cannot be sent (its code was not found). A summary written by a finished review lists what was sent, and stays so when it is brought up to date (`review.finished` in the settings names the pull requests; a summary of an older version, of every finding, goes on listing them all).

It is sent as **one GitHub review**: the ticked findings and your comments on their lines, the note, and how it ends, so the author is told once (the findings on the code, a separate summary and an approval were three notices before). A request for changes without a note says how many comments there are, since GitHub takes one only with words. Should GitHub refuse one line, the comments are posted one by one and the review's end after them.

### Files

`ph_pulls::pull_view::files`. Only the files in sight are drawn (patches are parsed once), and a diff over 400 lines is folded behind *Show diff*.

- **One column or side by side** (the button over the files, kept): side by side, the old file is on the left and the new on the right, a removed line beside the line that replaces it, long lines wrapped in their half.
- **Lines are picked by pressing on one and dragging over more** (or Shift-click). The box under them keeps a comment for the review (**Add comment**, ⌘↵), asks an agent about those lines (**Ask an agent**: what was typed goes to the chat as a question about them), or puts the lines in the box as a **suggestion** to edit, which the author takes with one click.
- **A right click on a line**, or on the lines picked: **Explain** (the chat's agent says what they do, why they are written so, and what in them deserves a second look), *Ask or comment…*, *Copy*. A file's **⋯** has *Explain what changes in this file*. The chat comes back for the answer if it was folded away. (Lines are picked whole, not parts of one.)
- **What is said about a line sits under it**: threads from GitHub (with *Reply*), the agent's open findings (a click reads one in full), and your comments not sent yet (*Edit*, *Delete*). A thread whose line the diff no longer shows is under the file, marked as about an earlier version.
- **Read** (the tick on a file's name, or **v**): the file folds, and the strip over the files counts them (*3 of 12 read*; its menu goes to a file). A tick is kept by the file's content, so a file that changes is to be read again.
- **Since your review**: only what the author changed after the commit your last review read (an agent's round here, else your own review on GitHub), from GitHub's comparison of the two commits. It shows once a review read an older commit.

Not here: syntax colours (the diff is drawn as the Git tab's is), and comments on removed lines.

### Before the merge

`ph_pulls::pull_view::status`, under the conversation:

- **Reviews**: each reviewer's newest word (approved, asked for changes), and who is still asked.
- **Checks**, failing ones first. **A failed check opens to why**: what it said of itself, the places it points at (`file:line` and the message), and for a GitHub Actions job the part of its log around the first error (read from GitHub's storage without the token, which goes only to the account's host). From there: **Fix in a task** (a task on the pull request's branch, its agent given what the check said and told not to weaken the check), **Run again**, and its page.
- **The merge** (merge, squash or rebase, from a menu; asks to confirm; it merges the commit the page shows, and says so when something was pushed since). It says what stands in the way instead of offering a button GitHub will refuse: a draft, a conflict, a missing required review or check (no button), a branch behind its base (**Update branch** merges the base into it), checks failing or running (the confirmation then says *Merge with failing checks*).

## The queue

`github_view/queue.rs`. An inbox list groups its pull requests by what needs doing, what waits for you first.

**Needs my review**, by what an agent has made of each:

| Group | What is in it |
|---|---|
| **Findings to read** | the newest finished round left findings open: how many, and the worst (*3 findings, 1 major*). **Read** opens the pull request on its Findings tab, as a click on the row does. |
| **Agents at work** | a review is being readied (*Cloning owner/repo 12s*), runs (what the agent is doing), or *waits its turn*. **Watch** opens its chat; **Answer** when it asks something; **Cancel** takes one out of the line. |
| **Not started** | nobody reviewed it. A tick box, and **Review** for one. A review started in this run that ended on an error says so (*The review stopped*, **See why** opens its chat). |
| **Reviewed, nothing found** | the newest round has no open finding. |

**Waiting for author**: *New commits since your review* (**Check the fixes** when an agent reviewed it, else **Review**), *Waiting for the author*, *Approved by you*.

**My pull requests**: *Changes requested*, *Checks failing*, *Conflicts with the base*, *Ready to merge* (approved, nothing failing), *Waiting for review*, *Drafts* (`ph_review::triage`).

- **Drafts and pull requests from bots** (Dependabot, Renovate…) are left out of *Needs my review* and of its count; the list says how many under it, with **Show**, and the button beside the sort turns each on.
- **A box to find** one by title, repository, author or number shows once a list is longer than a glance takes in.
- **Reviews of several at once.** Tick pull requests under *Not started* (the bar's tick takes them all) and press **Review N**. The bar names the agent and the review template and lets you choose them. Each gets a full review in the view's own copy of the repository, reading only; nothing is sent until you finish the review on the pull request's page. The repository is cloned once and each pull request checked out in turn (one Git operation at a time). **Three run at a time**; the rest wait their turn, and can be taken out of the line.
- **A review from the queue runs behind the chat in view** (`ChatPanel::start_review_behind`): a chat of its own in the repository's space, which does not take the panel beside whatever pull request is open. Its row follows it, and so does its card on the Bridge.
- **One round at a time on a pull request.** Its rounds share one checkout: while a review of it is readied or runs, *Review with agent* on its page says so instead of starting another, and its tick is dropped.
- What agents found is read from the store when the lists are read and whenever a round ends (`GithubView::read_reviews`), off the UI thread.

The rows also carry the triage, for every pull request listed: that it is a draft or a bot's, the team that was asked when it was not you by name, the size (small, medium, large, and lines changed), the checks, what the change touches that deserves care (auth, security, migrations, payments, CI, dependencies) and whether you reviewed it and the author pushed since. **Review first** (the default; **Newest first** sorts by date) puts first the reviews the author answered with commits, then pull requests whose CI is not failing, risky before plain, small before large.

## Review with agent

The chat beside the pull request shows the review as begun the moment it is asked for: the request as its first message and, under the agent's name, *Cloning owner/repo…* (the first time) or *Checking out #n…* with the time it has taken ([a chat being readied](chat.md#a-chat-being-readied)). The Bridge lists it under *Working*. A clone that fails says why over the composer.

1. PilotHouse brings the repository's [managed clone](#where-it-works-the-managed-clones) up to date, fetches the PR head (`pull/<n>/head`) and checks it out in `pr-<n>` beside the clone. Your checkout is not touched. The reviewed commit is kept (`refs/pilothouse/reviewed/<n>/<sha>`) so the next round can see how lines moved, even after a force-push.
2. **A clean checkout.** What a pull request carries that would act on the agent instead of being read is taken out of the checkout before the agent starts: agent settings with hooks (`.claude/settings*.json`, `.claude/hooks`), MCP servers (`.mcp.json`, `.vscode/mcp.json`), other agents' folders (`.gemini`, `.codex`, `.cursor`, `.agents`, `.clinerules`) and the root's `.env` files. The chat's first message lists what was taken out. Findings are still checked against the commit itself.
3. **The repository's guidance**, from the base branch (never the head, which the author controls): its rules in `.pilothouse/review.md` and in `REVIEW.md`; GitHub's path instructions (`.github/instructions/*.instructions.md`) whose `applyTo` globs match a file the pull request changes (`ph_review::rules`; one marked `excludeAgent: "code-review"` is left out); its notes for agents (`AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`) as context; and the rules PilotHouse learned for the repository (see *Remember why*). The first message says which were used.
4. **What the change is for.** The issues the description says it closes (`Fixes #12`, the first two) are read from GitHub and given to the agent, which checks that the change does what the issue asks and says what it leaves out. An issue's text is its reporter's: something to check against, never instructions.
5. A **review chat** opens beside the pull request and starts by itself with the default agent. It reads the change in the context of the whole codebase and ends with a fixed answer block (`ph_review::prompt`): per finding its place, and the text in the parts below; the code it is about copied from the file (evidence), a fix (optionally replacement code), and two answers: **what goes wrong** (security, crash, wrong result, slow, maintainability, improvement) and **how often** (in normal use, in specific conditions, rarely). PilotHouse works the severity out from those two answers (`ph_review::severity`), so the same problem gets the same severity whichever agent finds it.
6. **Written to be read.** A finding is read by someone who has not read the code and decides in a minute whether it matters, so the agent is told how to write it, whatever the template (`ph_review::writing`): a **title** that is one plain sentence saying what goes wrong and for whom (the effect, not the code: *A shopper whose cart was saved before the last update cannot check out*, not *Unhandled error in restore()*); **what happens** (what the code does now, and what it should do); **why it matters** (who notices, and what they see or lose); an **example** (one concrete case, with real values, and what happens then); short sentences, terms explained, no *could potentially*. The three parts are kept in the finding's text under their labels, so the same text reads well on GitHub.
7. **Checked before it is shown.** The agent is told to go through its findings once more and try to prove each wrong (callers, configuration, tests), and to drop what does not hold: a wrong finding costs the reader more than a small one left out. Then each finding's evidence is looked for in the file at the reviewed commit (whitespace and diff marks aside); found, the finding moves to the lines quoted; not found, it is marked *code not found* and never sent (`ph_review::grounding`).
8. When the turn ends the round is saved and a short card in the chat says what is new, fixed and open, with **Open findings** and **Check the fixes**. Later messages in that chat are an ordinary conversation.

### Rounds

A pull request's findings are one list carried from round to round (`ph_review::ledger`), each labelled `F1`, `F2`… for good:

- **Check the fixes** (after the author pushes): a round on the new head that checks each open finding (fixed or still there, and a "fixed" someone only claimed) and looks at what changed. Findings follow their lines through git's line map, not the agent's memory; one found in code that had not changed since the last round is marked *missed earlier*.
- **Second opinion**: a round by another installed agent that has not reviewed the pull request; a problem both find is marked *both agents*.
- **Double-check**: an agent tries to disprove each open finding; those that do not hold up are marked not a problem by the double-check.

### Findings

The Findings tab: the verdict, how much is open and the review in two or three sentences, then each finding as a card that reads top to bottom: its severity and label, **its title in full**, its place (opens it in Files) and why it has that severity (*Crash or hang, in specific conditions*), then *What happens*, *Why it matters*, *Example*, **the change that fixes it as a diff** (the quoted code alone when no replacement was given) and the fix in words. Open findings are unfolded; done ones fold to their title and who decided what (*Not a problem by alice: validated upstream*). *Open*, *Done* and *All* show once something is done.

What a reviewer does most is in sight, the rest behind one menu:

- On a finding: **Not a problem…** (with why) and **Fix in a task**; under **⋯**: *Mark as fixed*, *Won't fix…*, *Reply on GitHub…*, *Edit the wording…* (before it is sent: the title and the text, as it will be posted under your name) and *Open an issue…*.
- **Is the author right?** (after the author answered): an agent reads the code again and says whether the answer holds, with a drafted reply to **Post** or **Edit first**.
- **Open an issue…** writes it in a dialog: title and text (drafted from the finding: its place at the reviewed commit, the code, what is wrong and the fix), and **who it is assigned to**. It is labelled `pilothouse`. For a security finding in a public repository the dialog warns that an issue would tell everyone, and offers a private security advisory first.
- **Remember why** (findings that were not a problem): a rule every later review of the repository is told, kept on this computer (`review.learned.<repository>`), or **Propose to the repository**: a pull request adding it to `.pilothouse/review.md`, for everyone's reviews.
- Over the list: **Check the fixes** (when the author pushed since the round) and **Fix N in a task**; under **⋯**: *Second opinion*, *Double-check*, *Read the author's replies*, *Update on GitHub*, *Copy as Markdown*, *Open the review's chat*, how reviews of the repository went (pull requests reviewed, findings and what came of them, rounds per pull request, findings missed by a round, the share of decided findings that were not a problem, and per agent how many it found and how many were not a problem), and the rules remembered for it (each can be forgotten).

Sending them is [Finish review](#finish-review), on the page's header.

### On GitHub

[Finish review](#finish-review) sends (`ph_review::post`, `ph_review::github`):

- Each ticked finding as a **line comment** on the part of its lines the diff shows, in the one review that also carries your own comments, your note and how the review ends. When the agent's replacement covers exactly those lines, it is a **one-click suggestion**.
- One **summary comment** for the whole pull request (unless unticked), rewritten in place on every post (only by its author, the reviewer): the verdict, the findings sent by severity, each with **tick boxes** (Fixed / Not a problem / Won't fix), what is done, and a hidden data block with those findings (kept under GitHub's size limit by shortening). A finding you held back is not in it. Anyone's PilotHouse that opens the pull request, the author's included, shows those findings in its Findings tab, and the author answers each there: their answer is posted as a reply. Without a summary comment, a ticked finding whose lines the diff does not show is said in the review's own text, so nothing ticked is dropped.
- **Update on GitHub** (the Findings menu) brings GitHub up to date with what became of the findings there: the summary rewritten, threads of findings no longer open **resolved**. It puts nothing new on the code.
- **Reading back** (on opening the tab, *Read the author's replies*, a watch): ticks in the summary comment, and replies on a finding's thread or naming it in the conversation (`F3: not a problem, …`) that start with *Fixed*, *Not a problem* (or *false positive*) or *Won't fix* set its status, with the author's name and reason. The next check of the fixes verifies a claimed fix.

### Fix in a task

**Fix in a task** (a finding, all open ones, or a failing check) and **Address comments** make a [task](workspace.md) on the pull request's head: from a workspace's checkout of the repository when there is one, else Reviews' clone (branch `task/pr-<n>-fix`). Its chat starts with the findings (or the comments, or what the check said) and says how to push to the pull request's branch. In a task on someone else's pull request, their agent settings are taken out (and hidden from git, so the removal is never committed).

## Walkthrough

For whoever reviews the pull request (a tech lead, a manager) and has to understand it first, often without knowing that part of the code. **Explain this pull request** (Walkthrough tab) has an agent read the change and the code around it (what calls it and what it calls, the modules it lives in, the repository's documentation), then write a briefing that goes from plain words to detail (`ph_review::walkthrough`):

- **In short**: what changes and why it matters, in two or three sentences.
- **Why**: the problem or goal, and where that comes from (the description, the commits, an issue, or the agent's reading of the code, marked as a guess).
- **How it works today**: the part of the system the change touches, as it is before it, for someone new to the code, with a diagram.
- **What changes**: step by step, with a diagram of the new flow when it differs.
- **How it is done**: the main choices, by area.
- **What it affects**: users, APIs, data and migrations, configuration, security, performance, compatibility, rollout, whichever apply.
- **Where to look closely**: the places that most deserve a reviewer's attention, each with what to check, and what is safe to skim.
- **How it is tested**, **Questions for the author** and **Terms** (the domain words used).

It is shown as the chat shows a reply: Markdown with its Mermaid diagrams drawn (hover one to zoom it or open it large), and text you can select and copy (⌘C or Ctrl+C), as on the Overview. A reference to a changed file (`src/store.rs:40-58`) opens it in the Files tab; one to any other file opens it on the web at the commit the briefing read. **Copy** copies all of it as Markdown, for a description or a message; **Again** writes it again (the header says when new commits came since). The agent works in the chat beside, where you can ask it more.

## Review your changes before pushing

**Review** in the Git tab (over the changes) has an agent review what you have not pushed yet: commits ahead of the upstream and work not committed (or only the latter without an upstream), with the repository's rules and learned rules. Its findings come back as **notes on the changed lines**, and **Fix with agent** gives them to the chat. **Review again** carries the findings on, as rounds do for pull requests.

## On the Bridge

Reviews an agent works on, or waits for you in, show on the [Bridge](workspace.md#the-bridge) with the workspaces' tasks (*Reviews · owner/name*), also while their repository is still being cloned. One finished in this run waits under *Ready for review* (a click opens it in Reviews) until **Done** puts it away; after that, and after a restart, finished ones are read here, in the queue.

History lives in SQLite (`pr_reviews`: every round, its kind, agent, round number and summary comment; `pr_findings`: each finding of each round with its status, who set it and why, its comments and issue).
