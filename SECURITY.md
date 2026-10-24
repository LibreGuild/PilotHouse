# Security policy

Please don't report security problems in public issues, discussions or pull requests.

Report them privately on GitHub: go to **Security → Report a vulnerability** in this repository and describe what an attacker can do and how to reproduce it. Only the maintainers can see the report. We acknowledge reports within 7 days, keep you updated, and credit you in the advisory unless you prefer not to be named.

## Supported versions

Security fixes go into the latest release.

## Scope

PilotHouse runs coding agents and opens repositories, web pages and devices on your machine, so we take particular interest in:

- ways for a repository, web page, pull request or agent output to run code or reach data without the user's approval;
- other local users or programs reaching PilotHouse's control socket or named pipe;
- secrets (tokens, keys) leaking into logs, chats, screenshots or files.

How PilotHouse guards these is described in [docs/security.md](docs/security.md).
