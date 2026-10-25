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
- **Files**: each file's diff with its line comment threads; Reply in a thread. Only the files in sight are drawn (patches are parsed once), and a diff over 400 lines is folded behind *Show diff*, so a large pull request scrolls smoothly.
- **Commits**, **Checks** (click to open a check's page).
- **Check out** (in a workspace's PRs tool): fetches the PR head and switches to a local `pr/<number>` branch.
- The last agent review's state (open and fixed findings) with **Open review chat**.

## Review with agent

1. PilotHouse brings the repository's [managed clone](#where-it-works-the-managed-clones) up to date (every branch, so `origin/<base>` is current), fetches the PR head (`pull/<n>/head`) and checks it out in `pr-<n>` beside the clone (reused and moved to the new head on later runs). Your checkout is not touched. Reviews started from a workspace's PRs tool run here too. Reviews made before this lived in `<data dir>/worktrees/<project>-pr-<n>`, which is tidied as before.
2. A **review chat** opens in the GitHub view's chat for the repository and starts by itself with the default agent. The prompt (`ph_review::prompt`) asks it to read the change in the context of the whole codebase, find real problems (bugs, security, data loss, races, error handling, performance, compatibility, tests), not edit anything, and end with a fixed template: a JSON block with a summary, a verdict and findings (`severity` critical/major/minor/nit, `path`, `line`, `title`, `details`, `suggestion`, a stable `key`).
3. In a review chat, reads, searches and a short list of read-only commands (`git diff/log/show/blame…`, `rg`, `grep`, `ls`, `cat`…, no shell operators) are approved automatically, so a review can run unattended. Anything else still asks.
4. When the turn ends, the findings become a **card** in the chat: grouped by severity, each with its file and line, its **Issue**, **Evidence** and **Fix**, and New / Still open / Fixed / Dismissed. Posted comments carry the same three parts. Nothing goes to GitHub until you pick findings and click:
   - **Post as line comments**: one review on the reviewed commit, a line comment for each finding on a line the diff shows, the rest in the review's text.
   - **Post as one comment**: one conversation comment listing them.
   - **Dismiss** a finding (it will not come back as new).
5. **Review again** (after the author pushes): a new run on the latest head. The previous findings go into the prompt; the agent re-reports those still there with the same key and lists fixed ones. The card then shows what is still open (keeping its posted comment), what is new, and what is fixed. **Mark fixed on GitHub** replies "Fixed" on each fixed finding's line thread, or posts one comment for the rest.

History lives in SQLite (`pr_reviews`, `pr_findings`): every run, its findings, their status and the GitHub comment each was posted as.
