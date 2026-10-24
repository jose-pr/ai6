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
- `coagent_m.terminal`: terminal providers behind one contract (`Provider`, `Session`,
  `Registry`), ranked by priority. Built in: `posix-pty`, `pipes` (any OS) and `conpty`
  (Windows; a scaffold that reports itself unavailable). Others load from the
  `coagent.terminals` entry-point group or, with the `guatiao` extra, from guatiao
  libraries on `COAGENT_TERMINAL_PLUGINS`.
- The watchdog runs on any provider (`AgentSpec.terminal`, `run --terminal`), answers
  cursor-position queries (`ESC[6n` → `ESC[24;1R`) and removes them from the output.
- Rust crates: `coagent-terminal-kind` (the `coagent_terminal` guatiao kinds),
  `coagent-terminal-pty` (a plugin on `portable-pty`) and `coagent-native` (guatiao's C
  ABI for the Python binding).
- `coagent_m.hooks`: global, project and agent hooks that add environment, add
  context (`COAGENT_CONTEXT_FILE`), rewrite the command line and receive events, as
  Python objects or programs (`hooks.toml`). A project's hooks load only when the
  project is trusted.
- `coagent_m.profiles`: launchers and agent profiles (`profiles.toml`) with aliases,
  `extends`, roles and tags; `AgentSpec.from_profile()`; `launch --profile` /
  `launch --role`, `profiles`, `hooks` and `providers` commands.
- Licensed under the Mozilla Public License 2.0, with a contributor licence agreement.

[Unreleased]: https://github.com/jose-pr/coagent/commits/main
