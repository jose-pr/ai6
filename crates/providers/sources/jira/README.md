# ai6-source-jira

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_source_jira/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `jira` Source provider for [AI6](https://github.com/jose-pr/ai6): **Jira issues that
carry a label become Missions, and the outcome comes back as a comment** and, if
configured, a workflow transition. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/sources/jira/AGENTS.md).

## Features

- Runs a JQL search per Operation: a project, labels and an optional extra condition,
  resuming from a cursor so only changed issues are fetched.
- Speaks Jira Cloud (REST v3, rich-text descriptions flattened to plain text) and Jira
  Server or Data Center (REST v2).
- Reads its credentials from environment variables the Operation names; they are never
  configured or logged.
- As a Tracker, reads, comments on (Markdown becomes rich text), labels, assigns,
  transitions, creates and links issues, and lists the replies that may answer a question.
- Builds as both a Rust library and the Guatiao provider library a Station loads.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-source-jira
```

## Quick start

In `operations.toml`:

```toml
[operation.G.sources.jira]
site = "https://example.atlassian.net"
project = "GUA"
email_env = "JIRA_EMAIL"
token_env = "JIRA_API_TOKEN"
status_transitions = { done = "In Review" }
```

Then start a Station that holds the Headquarters lease with the library on its plugin path:

```bash
JIRA_EMAIL=... JIRA_API_TOKEN=... cargo run -p ai6 --features cli -- serve --plugin-path target/debug
```

## Development

```bash
cargo test -p ai6-source-jira
cargo clippy -p ai6-source-jira --all-targets -- -D warnings
```

Tests run against a loopback HTTP server; they never call Jira.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
