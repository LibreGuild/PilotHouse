# Contributing to PilotHouse

Thanks for helping! Bug reports, ideas, documentation and code are all welcome. Please follow the [Code of Conduct](https://github.com/LibreGuild/.github/blob/main/CODE_OF_CONDUCT.md) in every interaction.

## Ways to help

- **Report a bug:** [open an issue](https://github.com/LibreGuild/PilotHouse/issues/new) with the steps to reproduce it, what you expected, and your system.
- **Suggest a feature:** start a [discussion](https://github.com/LibreGuild/PilotHouse/discussions) so the idea can be shaped before anyone writes code.
- **Report a security problem:** never in a public issue. See [SECURITY.md](SECURITY.md).
- **Send a pull request:** fixes and small improvements can go straight to a PR. For anything big (a new panel, a change to stored data), open an issue first so we agree on the approach.

## Set up

You need Rust stable and `git`. On Linux, the system libraries in [docs/development.md](docs/development.md).

```sh
git clone https://github.com/LibreGuild/PilotHouse.git
cd PilotHouse
cargo dev        # the app with the UI automation socket the tests use
```

Use a throwaway profile, not your real data: `PILOTHOUSE_DATA_DIR=/tmp/ph-profile cargo dev`.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --features pilothouse/automation -- -D warnings
cargo test --workspace --features pilothouse/automation
```

CI runs the same on Linux, Windows and macOS, and a pull request merges only when it passes.

- Keep each crate to one feature; the shell composes features (see [docs/architecture.md](docs/architecture.md) and the conventions in [docs/development.md](docs/development.md)).
- Database migrations are append-only: add a new one, never edit one that shipped.
- Add or update the short page under `docs/` for what you changed.
- Only permissively licensed dependencies (Apache-2.0, MIT, BSD, ISC, Zlib).

By contributing, you agree that your contributions are licensed under the Apache License 2.0.
