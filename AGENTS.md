# AI6 — repository orientation

Contributor guidance for this checkout. Each Rust crate has an `AGENTS.md` API header
beside its README describing its public API.

## Layout

One Cargo workspace (`Cargo.toml`, one `Cargo.lock`).

| Path | What |
| --- | --- |
| `crates/ai6/` | The Station daemon and AI6 contracts: `ai6::directorates::{m,registry,q,section_00,medical}` decide, `ai6::station` runs; `ai6` command |
| `crates/universal-exports/` | Universal Exports: the authenticated public API gateway and `universal-exports` command |
| `crates/ai6-diptel/` | The DipTel wire contract (lifecycle and terminal frames), native and wasm |
| `crates/ai6ctl/` | The read-only operator command |
| `crates/providers/{agents,terminals,transports,safehouses,sources}/` | Guatiao providers: Claude/Codex handlers, the `pty` terminal, the `local` and `ssh` transports, the `git` and `copy` safe houses, GitHub and Jira Sources |
| `clients/moneypenny/` | Moneypenny, the Flutter desktop/web operator app |
| `examples/config/` | A runnable Operation and Agent roster |
| `docs/`, `mkdocs.yml` | Narrative guides; the Rust API reference is published under `/rust/` |
| `tools/public-api.ps1`, `tools/public-api/` | Checked public API snapshots of the library crates |

## Commands

From the repository root:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p ai6 --no-default-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
pwsh -File tools/public-api.ps1 -Mode check
```

The Flutter app needs the isolated Flutter `main` SDK described in
`clients/moneypenny/README.md`, because native desktop windowing is not in stable Flutter.

## Rules

- Work and infrastructure are separate: Operation → Mission → Session is the work;
  Station → Theater is where it runs. Operations are global; no Station owns one.
- The Station (`ai6`) owns Missions, Agent Sessions and policy. Universal Exports owns the
  authenticated public API; Moneypenny and `ai6ctl` are its clients and never reach a
  Station directly.
- An Agent (codename) owns its handler, model and effort; a role is purpose and policy
  only. Terminal providers are interchangeable: the Station picks an available one
  (preferring `pty`) unless pinned with `--terminal`; neither the Agent nor the handler
  chooses.
- Agent identity, role, Operation policy, Mission ID and provider-issued Session ID are
  distinct contracts.
- Directorates are modules of the `ai6` crate. Providers live under `crates/providers/` and
  carry their own Guatiao entry point; do not add forwarding-only crates.
- Access kinds, the role authorizer, the access providers and Station mutual TLS are the shared
  `coqui-access`, `coqui-roles`, `coqui-peer` and `coqui-peer-tls` crates, pinned by git
  revision beside Guatiao; AI6 keeps only its scope kinds, actions and where it stores things.
  The host's tests build the access plugins as dev-dependencies and load them from beside the
  test binary: clean `target/` after a plugin is renamed, since the host loads every library.
- A project's `.ai6/` configuration is read only for a registered Operation.
- Names come from the Bond / SIS lore in `docs/organization.md`; use a field name only
  where the code has the thing.
- Rust 1.89 is the minimum; stable and 1.89 are checked in CI.
- MPL-2.0. Outside contributors sign `CLA.md`.
- Remote Theaters and SQL/federated Registry placement are not implemented; do not
  describe them as available.

## CI and release

- `.github/workflows/test.yml` runs the workspace matrix, public API snapshots and the
  Flutter app, on demand and on `ci-*` tags.
- `.github/workflows/docs.yml` strictly builds the narrative site and Rust API reference,
  then deploys Pages.
- `.github/workflows/release.yml` gates a source release on tests and a strict docs build.
  Every `v*` tag needs the maintainer's explicit go-ahead for that release.
