# Pull requests and agent reviews

Crates: `ph-github` (API, accounts, secrets), `ph-pulls` (UI), `ph-review` (review template, parsing, re-review, posting). GitHub.com and GitHub Enterprise Server work the same way.

## Accounts (Settings → GitHub)

- One account per host: `github.com` or any Enterprise host (`github.example.net` → API `https://HOST/api/v3`; an API URL can be set for unusual setups).
- Paste a personal access token (scopes `repo`, `read:org`; **Create a token** opens the host's token page). PilotHouse checks it (`GET /user`) and shows who it belongs to.
- Tokens live in the system keychain (macOS Keychain, Windows Credential Manager); on Linux, and in debug builds, in a file only the user can read in the private data folder. Never in SQLite, logs or chats. Requests go only to hosts you added; a token is never sent to another host (checked per request, including pagination links).
- TLS uses the operating system (native-tls with the system's roots), so company CAs installed on the machine work.

## The GitHub view (⌘⇧G)

What you would see on GitHub's own pages, apart from any workspace (`crates/workspace/src/github_view/`). Picked in the title bar beside *Workspaces*.

- **Inbox** (sidebar): *Review requested*, *My pull requests* and *Assigned to me*: open pull requests across every repository each account sees, newest activity first (`ph_github::search`, GitHub's issue search). They are read when the view is first shown, again when it is shown after two minutes, and with the refresh button. The title bar's *GitHub* shows how many wait for your review.
- **Repositories** (sidebar): those your workspaces' folders come from (their remote, for a host with an account), and any you add with **+** (`owner/name` on github.com, else on your only account's host, or the repository's URL; GitHub confirms the token can see it). Added ones are kept in `github.repositories` and can be taken out of the list from their menu.
- **The middle** shows an inbox list, or a repository's open pull requests and one PR in full (the same list and PR view as a workspace's PRs tool, without *New* and *Check out*, which need a checkout of yours).
- **The chat** on the right is the open pull request's (it shows while one is open): ready to type into, with the templates as starters (*Review this PR*, *Security review*, *Explain this PR*, *Test plan*, and your own). The first message of a new chat clones the repository if needed and checks the PR out (the chat says so while it does), then sends. Its header lists that PR's earlier chats. A file a chat names opens in a window of its own (the view has no editor).
- **Chats here only read.** They work in PilotHouse's copy of the PR: reads, searches and read-only commands (also chained with `&&`, `;` or `|`, like `cd … && git log | head`) run without asking; edits, deletes and moves are refused without asking, and the agent is told the chat is read-only. Anything else asks. Antigravity can only run a command by going on with every permission for the rest of the turn, so that always asks you. Changes are made from a workspace.

### Templates (Settings → Templates)

What agents are asked to do with a pull request (`ph_review::templates`). Shipped: *Review this PR* and *Security review* (reviews with findings), *Explain this PR* and *Test plan* (answers). Each can be edited, put back as shipped, or taken out of the list and brought back; *New template* adds your own, making either a review or an answer.

- A **review** template's text is the reviewing guide: what to look for, what each severity means (critical: must not merge; major: fix before merging; minor: worth fixing; nit: polish), and that every finding needs **evidence** (the quoted lines, the input or call path) and a **fix**. PilotHouse adds the pull request, the repository's own rules from `.pilothouse/review.md` on the base branch (never the PR's head, which its author controls), the earlier findings to re-check, and the answer format the findings card reads, so a template changes how a review is done but never breaks the card.
- An **answer** template's text is a request (the shipped *Explain this PR* asks for a summary, how it works drawn as Mermaid diagrams, a file-by-file table, what to look at and how to try it). PilotHouse adds where the PR is checked out and how to see its diff.

### Where it works: the managed clones

```
<data dir>/github/<host>/<owner>/<name>/repo    a full clone (default branch)
<data dir>/github/<host>/<owner>/<name>/pr-12   pull request 12's head (a worktree of the clone)
```

`ph_pulls::clones`. The user's checkouts are never touched. The first review of a repository clones it; later ones fetch. Git reaches the host over HTTPS with the account's token, given to each call in the environment as a header for that host only (`GIT_CONFIG_*` setting `http.https://HOST/.extraHeader`): it is never on a command line, in a remote URL or in a config file. The clone is a full one, so a review's agent reads history without the network. Each repository has a space of its own (`projects.kind = 'github'`, named `owner/name`, its folder the clone) that holds its chats, so they have what workspace chats have (agents, the PilotHouse tools, usage); it is not a workspace and is not listed as one. Folders of closed pull requests are not cleaned up yet.

## The PRs tool (⌘8)

- The project's remote (`origin`, else `upstream`, read before `insteadOf` rewrites) gives host, owner and repo; the account for that host is used.
- Open PRs, newest activity first, with CI state (check runs and statuses), labels, draft and "Review requested". Filters: Open, Mine, To review.
- **New** opens a PR for the current branch (pushes it first if needed): title, base, description (**Write description**: an agent writes it from the branch's commits and diff), draft.
- **Review with agent** and **Open review chat** switch to the GitHub view, where reviews run.

## A pull request

- **Overview**: the description and conversation (Markdown), reviews with their state, then Comment / Request changes / Approve, and Merge (merge, squash or rebase; asks to confirm).
- **Findings**: what agent reviews found, kept across rounds (see [Findings](#findings)). The tab shows how many are open; opening it reads the author's ticks and replies.
- **Walkthrough**: the change explained (see [Walkthrough](#walkthrough)).
- **Files**: each file's diff with its line comment threads; Reply in a thread. Only the files in sight are drawn (patches are parsed once), and a diff over 400 lines is folded behind *Show diff*. **Ask about lines** under a file asks an agent about some of its lines (type `start-end: question`). When there is a walkthrough, its tour runs along the top: each stop shows its file.
- **Commits**, **Checks** (click to open a check's page).
- **Watch**: PilotHouse looks at the pull request every three minutes and tells you (a system notification) when the author pushes or answers a finding; answers are applied. Merged or closed pull requests stop being watched.
- **Address comments** (your own pull requests): a [task](#fix-in-a-task) on the branch whose agent works through the reviewers' comments not yet resolved, changes what is right and drafts a reply for each.
- **Check out** (in a workspace's PRs tool): fetches the PR head and switches to a local `pr/<number>` branch.

## The inbox, for review

The *Review requested*, *My pull requests* and *Assigned to me* lists show, for the first 15 pull requests of each: the size (small, medium, large, and lines changed), CI (green, failing, running), what the change touches that deserves care (auth, security, migrations, payments, CI, dependencies) and whether you reviewed it and the author pushed since. **Review first** (the default; **Newest first** sorts by date) puts first the reviews the author answered with commits, then pull requests whose CI is not failing, risky before plain, small before large (`ph_review::triage`).

## Review with agent

1. PilotHouse brings the repository's [managed clone](#where-it-works-the-managed-clones) up to date, fetches the PR head (`pull/<n>/head`) and checks it out in `pr-<n>` beside the clone. Your checkout is not touched. The reviewed commit is kept (`refs/pilothouse/reviewed/<n>/<sha>`) so the next round can see how lines moved, even after a force-push.
2. **A clean checkout.** What a pull request carries that would act on the agent instead of being read is taken out of the checkout before the agent starts: agent settings with hooks (`.claude/settings*.json`, `.claude/hooks`), MCP servers (`.mcp.json`, `.vscode/mcp.json`), other agents' folders (`.gemini`, `.codex`, `.cursor`, `.agents`, `.clinerules`) and the root's `.env` files. The chat's first message lists what was taken out. Findings are still checked against the commit itself.
3. **The repository's guidance**, from the base branch (never the head, which the author controls): its rules in `.pilothouse/review.md`, its notes for agents (`AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`) as context, and the rules PilotHouse learned for the repository (see *Remember why*). The first message says which were used.
4. A **review chat** opens beside the pull request and starts by itself with the default agent. It reads the change in the context of the whole codebase and ends with a fixed answer block (`ph_review::prompt`): per finding its place, title, details, the code it is about copied from the file (evidence), a fix (optionally replacement code), and two answers: **what goes wrong** (security, crash, wrong result, slow, maintainability, improvement) and **how often** (in normal use, in specific conditions, rarely). PilotHouse works the severity out from those two answers (`ph_review::severity`), so the same problem gets the same severity whichever agent finds it.
5. **Checked quotes.** Each finding's evidence is looked for in the file at the reviewed commit (whitespace and diff marks aside); found, the finding moves to the lines quoted; not found, it is marked *code not found* and never posted (`ph_review::grounding`).
6. When the turn ends the round is saved and a short card in the chat says what is new, fixed and open, with **Open findings** and **Check the fixes**. Later messages in that chat are an ordinary conversation.

### Rounds

A pull request's findings are one list carried from round to round (`ph_review::ledger`), each labelled `F1`, `F2`… for good:

- **Check the fixes** (after the author pushes): a round on the new head that checks each open finding (fixed or still there, and a "fixed" someone only claimed) and looks at what changed. Findings follow their lines through git's line map, not the agent's memory; one found in code that had not changed since the last round is marked *missed earlier*.
- **Second opinion**: a round by another installed agent that has not reviewed the pull request; a problem both find is marked *both agents*.
- **Double-check**: an agent tries to disprove each open finding; those that do not hold up are marked not a problem by the double-check.

### Findings

The Findings tab (Open, Done, All), each finding with its severity and why (*Crash or hang, in normal use*), its place (opens it in Files), its text, the last check's note, and who decided what (*Not a problem · by alice: validated upstream*). For each:

- **Fixed**, **Not a problem…**, **Won't fix…** (with why), **Reopen**.
- **Is the author right?** (after the author answered): an agent reads the code again and says whether the answer holds, with a drafted reply to **Post** or **Edit first**.
- **Fix in a task**, **Reply…** (on its thread on GitHub), **Open issue** (labelled `pilothouse`, with the place at the reviewed commit, the code, what is wrong and the fix; a security finding in a public repository first suggests a private security advisory instead).
- **Remember why…** (findings that were not a problem): a rule every later review of the repository is told, kept on this computer (`review.learned.<repository>`, listed and forgettable under the toolbar), or **Propose to the repository**: a pull request adding it to `.pilothouse/review.md`, for everyone's reviews.

And for all: **Post to GitHub** / **Update on GitHub**, **Summary only**, **Read replies**, **Fix N in a task**, **Copy as Markdown**, and **Stats** (for the repository: pull requests reviewed, findings and what came of them, rounds per pull request, findings missed by a round, the share of decided findings that were not a problem, and per agent how many it found and how many were not a problem).

### On GitHub

**Post to GitHub** (`ph_review::post`, `ph_review::github`):

- Each open, checked finding not yet on the code becomes a **line comment** (one GitHub review, neither approving nor requesting changes), on the part of its lines the diff shows. When the agent's replacement covers exactly those lines, it is a **one-click suggestion**. Should GitHub refuse one line, the rest are posted one by one.
- One **summary comment** for the whole pull request, rewritten in place on every post (only by its author, the reviewer): the verdict, open findings by severity, each with **tick boxes** (Fixed / Not a problem / Won't fix), what is done, and a hidden data block with the findings themselves (kept under GitHub's size limit by shortening). Anyone's PilotHouse that opens the pull request, the author's included, shows those findings in its Findings tab, and the author answers each there: their answer is posted as a reply.
- Threads of findings no longer open are **resolved**.
- **Reading back** (on opening the tab, **Read replies**, a watch): ticks in the summary comment, and replies on a finding's thread or naming it in the conversation (`F3: not a problem, …`) that start with *Fixed*, *Not a problem* (or *false positive*) or *Won't fix* set its status, with the author's name and reason. The next check of the fixes verifies a claimed fix.

### Fix in a task

**Fix in a task** (a finding, or all open ones) and **Address comments** make a [task](workspace.md) on the pull request's head: from a workspace's checkout of the repository when there is one, else the GitHub view's clone (branch `task/pr-<n>-fix`). Its chat starts with the findings (or comments) and says how to push to the pull request's branch. In a task on someone else's pull request, their agent settings are taken out (and hidden from git, so the removal is never committed).

## Walkthrough

**Walk me through it** (Walkthrough tab) asks an agent to explain the change, in three parts (`ph_review::walkthrough`):

- **The gist**: what it does, why, and where the why comes from (the description, the commits, or a guess, marked as such), and before and after, one line each.
- **At a glance**: the changed files grouped by purpose (core logic, API, UI, data, tests, config, docs…); a group shows only its files in the Files tab.
- **Read in this order**: three to six stops, each a few lines of a changed file with one sentence; each opens its file, and the tour runs along the top of the Files tab.
- When calls cross several files, a small **flow** of who calls what, drawn by PilotHouse from the agent's steps.

Stops in files the change does not touch are dropped. **Copy** gives it as Markdown (with the flow as a Mermaid diagram) for a description or a message.

## Review your changes before pushing

**Review** in the Git tab (over the changes) has an agent review what you have not pushed yet: commits ahead of the upstream and work not committed (or only the latter without an upstream), with the repository's rules and learned rules. Its findings come back as **notes on the changed lines**, and **Fix with agent** gives them to the chat. **Review again** carries the findings on, as rounds do for pull requests.

## Agents board

Review chats of the GitHub view show on the Agents board with the workspaces' chats (*GitHub · owner/name*), and count in its tab.

History lives in SQLite (`pr_reviews`: every round, its kind, agent, round number and summary comment; `pr_findings`: each finding of each round with its status, who set it and why, its comments and issue).
