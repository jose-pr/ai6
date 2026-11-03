# ai6-source-gitlab

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_source_gitlab/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `gitlab` Source provider for [AI6](https://github.com/jose-pr/ai6): **open GitLab
issues that carry a label become Missions, and the outcome comes back as a comment.** It
talks to gitlab.com or a self-managed instance. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/sources/gitlab/AGENTS.md).

## Features

- Polls one project's open issues by label and assignee, resuming from a cursor so only
  changed issues are fetched.
- Reads its token from the environment variable the Operation names; the token is never
  configured or logged.
- Reports a rate limit as an error and tries again on the next poll; pagination never
  leaves the configured API origin.
- As a Tracker, reads, comments on, labels, assigns, closes and creates issues, attaches a
  child as a task under its parent, links related issues, and lists the replies that may
  answer a question, from the issue's author or a project member.
- Builds as both a Rust library and the Guatiao provider library a Station loads.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-source-gitlab
```

## Quick start

In `operations.toml`:

```toml
[operation.G.sources.gitlab]
project = "group/project"
labels = ["ai6:ready"]
token_env = "GITLAB_TOKEN"
```

A self-managed instance adds `api_url = "https://gitlab.example.com/api/v4"`. Then start a
Station that holds the Headquarters lease with the library on its plugin path:

```bash
GITLAB_TOKEN=... cargo run -p ai6 --features cli -- serve --plugin-path target/debug
```

## Development

```bash
cargo test -p ai6-source-gitlab
cargo clippy -p ai6-source-gitlab --all-targets -- -D warnings
```

Tests run against a loopback HTTP server that answers with responses recorded from gitlab.com
and scrubbed; they never call GitLab. `tests/live.rs` is the exception: it runs the provider
against a real project, is ignored by default, and says at its top what it needs.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
