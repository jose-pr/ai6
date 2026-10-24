# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Monorepo layout: `m/` (supervisor), `moneypenny/` (web viewport), `q/` (armory).
- `coagent_m.watchdog`: runs a sub-agent on a pty in its own process group and sends
  the group SIGKILL after 300 s with no output (`input_hang` when it is quiet at a
  prompt, `silent` otherwise) or 300 s of output with no new line (`loop`). Reports
  `working`, `blocked`, `stalled`, `done` and `failed` transitions, and takes operator
  input through `Watchdog.send()`.
- `python -m coagent_m.watchdog run -- CMD` and `demo`.
- Licensed under the Mozilla Public License 2.0, with a contributor licence agreement.

[Unreleased]: https://github.com/jose-pr/coagent/commits/main
