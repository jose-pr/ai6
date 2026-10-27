# ai6-safehouse-git

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_safehouse_git/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `git` safe-house provider for [AI6](https://github.com/jose-pr/ai6): **each Mission
works in its own Git worktree on its own branch**, cut from the Operation's checkout. An
untouched worktree and its branch are removed when the Mission ends; one that holds
commits or edits is kept and reported. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/safehouses/git/AGENTS.md).

## Features

- A worktree per Mission under AI6's state directory, never inside the checkout.
- Branch names from a template (`{id}`, `{slug}`, `{operation}`), made safe for Git refs.
- Commits since the base, with author, committer and message, for cover checks.
- Re-attaches to a kept worktree after the daemon restarts, and can work in place.
- Clones a repository (URL or local, created bare on request) on one branch and saves to
  that shared branch by fast-forward, replaying once and falling back to a branch of its
  own; Moneypenny's personal and shared repositories use it.

## Installation

Part of the AI6 workspace; built with it. It needs `git` on the `PATH` of the Station
that loads it, and nothing else does.

```bash
cargo build -p ai6 --features cli -p ai6-safehouse-git
```

## Quick start

```bash
cargo run -p ai6 --features cli -- serve --plugin-path target/debug
```

and in `operations.toml`:

```toml
[operation.G]
path = "~/work/project"
workspace = { provider = "git", base = "main", branch = "ai6/{id}-{slug}" }
```

## Development

```bash
cargo test -p ai6-safehouse-git
cargo clippy -p ai6-safehouse-git --all-targets -- -D warnings
```

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
