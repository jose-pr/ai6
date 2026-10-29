# AI6

AI6 is a self-hosted control plane for coding agents. You register **Operations**
(projects and their policy), hand it **Missions** (briefs), and its **Stations** (nodes
running the `ai6` daemon) post an Agent to each Mission and run it as a provider
**Session** of Claude Code or Codex, in an isolated Git worktree, under a watchdog.
Operators follow and steer the work through **Universal Exports**, the authenticated
public API, from the **Outer Office** desktop and web app or the `ai6ctl` command.

## Build and run

From the repository root:

```console
cargo build --workspace --all-features
cargo run -p ai6 --features cli -- serve --config ./examples/config/ai6.cfg
cargo run -p universal-exports --features cli -- --help
cargo run -p ai6ctl -- --help
```

Outer Office, the operator app, needs the Flutter `main` SDK; see
[its README](https://github.com/jose-pr/ai6/blob/main/clients/outer-office/README.md).

## Learn more

- [Architecture](architecture.md): work versus infrastructure, the runtime, and what is
  not implemented yet.
- [Organization](organization.md): the directorates, the names and what each one owns.
- Guides: [Operations](operations.md), [Stations](stations.md),
  [Theaters](theaters.md), [Missions and intake](missions.md), [Agents](agents.md),
  [Terminals](terminals.md), [the Watchdog](watchdog.md), [Hooks](hooks.md) and
  [Examples](examples.md).
- Rust API: [ai6](https://jose-pr.github.io/ai6/rust/ai6/),
  [universal-exports](https://jose-pr.github.io/ai6/rust/universal_exports/),
  [ai6-diptel](https://jose-pr.github.io/ai6/rust/ai6_diptel/).
- [Changelog](changelog.md).
