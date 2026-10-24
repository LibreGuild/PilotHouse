# Pull requests and agent reviews

Crates: `ph-github` (API, accounts, secrets), `ph-pulls` (UI), `ph-review` (review template, parsing, re-review, posting). GitHub.com and GitHub Enterprise Server work the same way.

## Accounts (Settings → GitHub)

- One account per host: `github.com` or any Enterprise host (`github.example.net` → API `https://HOST/api/v3`; an API URL can be set for unusual setups).
- Paste a personal access token (scopes `repo`, `read:org`; **Create a token** opens the host's token page). PilotHouse checks it (`GET /user`) and shows who it belongs to.
- Tokens live in the system keychain (macOS Keychain, Windows Credential Manager); on Linux, and in debug builds, in a file only the user can read in the private data folder. Never in SQLite, logs or chats. Requests go only to hosts you added; a token is never sent to another host (checked per request, including pagination links).
- TLS uses the operating system (native-tls with the system's roots), so company CAs installed on the machine work.

## The PRs tab (⌘8)

- The project's remote (`origin`, else `upstream`, read before `insteadOf` rewrites) gives host, owner and repo; the account for that host is used.
- Open PRs, newest activity first, with CI state (check runs and statuses), labels, draft and "Review requested". Filters: Open, Mine, To review.
- **New** opens a PR for the current branch (pushes it first if needed): title, base, description (**Write description**: an agent writes it from the branch's commits and diff), draft.

## A pull request

- **Overview**: the description and conversation (Markdown), reviews with their state, then Comment / Request changes / Approve, and Merge (merge, squash or rebase; asks to confirm).
- **Files**: each file's diff with its line comment threads; Reply in a thread.
- **Commits**, **Checks** (click to open a check's page).
- **Check out**: fetches the PR head and switches to a local `pr/<number>` branch.
- The last agent review's state (open and fixed findings) with **Open review chat**.

## Review with agent

1. PilotHouse fetches the PR head (`pull/<n>/head`) and its base, and checks the head out in its own worktree: `<data dir>/worktrees/<project>-pr-<n>` (reused and moved to the new head on later runs). Your checkout is not touched.
2. A **review chat** opens there and starts by itself with the default agent. The prompt (`ph_review::prompt`) asks it to read the change in the context of the whole codebase, find real problems (bugs, security, data loss, races, error handling, performance, compatibility, tests), not edit anything, and end with a fixed template: a JSON block with a summary, a verdict and findings (`severity` critical/major/minor/nit, `path`, `line`, `title`, `details`, `suggestion`, a stable `key`).
3. In a review chat, reads, searches and a short list of read-only commands (`git diff/log/show/blame…`, `rg`, `grep`, `ls`, `cat`…, no shell operators) are approved automatically, so a review can run unattended. Anything else still asks.
4. When the turn ends, the findings become a **card** in the chat: grouped by severity, each with its file and line (opens in the editor), details and suggestion, and New / Still open / Fixed / Dismissed. Nothing goes to GitHub until you pick findings and click:
   - **Post as line comments**: one review on the reviewed commit, a line comment for each finding on a line the diff shows, the rest in the review's text.
   - **Post as one comment**: one conversation comment listing them.
   - **Dismiss** a finding (it will not come back as new).
5. **Review again** (after the author pushes): a new run on the latest head. The previous findings go into the prompt; the agent re-reports those still there with the same key and lists fixed ones. The card then shows what is still open (keeping its posted comment), what is new, and what is fixed. **Mark fixed on GitHub** replies "Fixed" on each fixed finding's line thread, or posts one comment for the rest.

History lives in SQLite (`pr_reviews`, `pr_findings`): every run, its findings, their status and the GitHub comment each was posted as.
