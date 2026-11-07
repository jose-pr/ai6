# AI6 — repository orientation

Contributor guidance for this checkout. Each Rust crate has an `AGENTS.md` API header
beside its README describing its public API.

## Layout

One Cargo workspace (`Cargo.toml`, one `Cargo.lock`).

| Path | What |
| --- | --- |
| `crates/ai6/` | The Station daemon, the field runner and AI6 contracts: `ai6::directorates::{m,registry,q,section_00,medical}` decide, `ai6::station` runs; `ai6` command (`ai6 serve`, `ai6 runner`) |
| `crates/universal-exports/` | Universal Exports: the authenticated public API gateway, Moneypenny's conversations and tools (`src/moneypenny/`), and the `universal-exports` command |
| `crates/process-tree/` | `ai6-process-tree`: a program's process tree, killed whole, shared by the pipe and pty terminals and the Codex sandbox probe |
| `crates/ai6-diptel/` | The DipTel wire contract (lifecycle and terminal frames), native and wasm |
| `crates/ai6ctl/` | The operator command: reads, and cancels, retries, resumes and interrupts Missions |
| `crates/providers/{agents,terminals,transports,theaters,safehouses,sources}/` | Guatiao providers: Claude, Codex, Antigravity CLI and `acp` (any Agent Client Protocol agent) handlers, the `pty` terminal, the `local`, `ssh`, `adb`, `container` and `qemu` transports, the `ephemeral-container` Theater provider, the `git` and `copy` safe houses, GitHub, GitLab and Jira Sources |
| `clients/outer-office/` | Outer Office (short name OO), the Flutter Linux, Windows and web operator app; Moneypenny is the AI assistant inside it |
| `examples/config/` | A runnable Operation and Agent roster |
| `docs/`, `mkdocs.yml` | Narrative guides; the Rust API reference is published under `/rust/` |
| `tools/public-api.ps1`, `tools/public-api/` | Checked public API snapshots of the library crates |
| `tools/sdk/` | `sdk.lock`, every SDK a build needs at exact versions; `bootstrap.sh` and `bootstrap.ps1` install them under one directory; `check_lock.py` holds the lock to the repository; `crates.py` keeps the `deps` branch's `crates/` equal to `Cargo.lock` and builds offline from it |

## Commands

From the repository root:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p ai6 --no-default-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
pwsh -File tools/public-api.ps1 -Mode check
cargo run -p ai6 --features cli --example settings_reference -- --check
```

The settings tables in `docs/` and Outer Office's copy of `[oo]` are generated from the settings'
structs; that command fails when one differs, and without `--check` it writes them.

`sh tools/sdk/bootstrap.sh` (`pwsh -File tools/sdk/bootstrap.ps1` on Windows) installs every SDK
at the versions `tools/sdk/sdk.lock` names; `tools/sdk/README.md` has the flags. The Flutter app
needs the Flutter `main` commit named there, because native desktop windowing is not in stable
Flutter. A change to that commit is one change: refresh the `deps` branch, then update the lock and
the defaults of the Dockerfiles in `containers/` on `deps`. `python3 tools/sdk/check_lock.py` fails
when they differ, when the lock disagrees with the workflows, `Cargo.toml` or `tool/cdn_files.json`,
or when the `deps` commit it names does not hold every crate `Cargo.lock` does. A change to
`Cargo.lock` needs `python3 tools/sdk/crates.py refresh DEPS_DIR`, with a worktree on `deps`.

The Registry test suite runs every case on the file Registry and SQLite. With
`AI6_TEST_POSTGRES_URL` set to a `postgres://…?sslmode=disable` server whose user may
create schemas, it also runs them on PostgreSQL (one schema per case, dropped afterwards),
as do the SQL roster and connection tests and `crates/ai6/tests/ha.rs` (two `ai6 serve`
Stations failing over); `AI6_TEST_POSTGRES_TLS_URL` names a TLS-only server, with
`sslrootcert=` pointing at its CA, for the TLS test. Unset, they pass without running and
`cargo test -- --nocapture` prints how many were skipped. CI's Linux legs start both
servers in Docker. Building the `sqlite` feature compiles SQLite, so it needs a C compiler.

The `tests/live.rs` of `crates/providers/sources/{github,gitlab,jira}` run a provider against the real
service. They are ignored by default and need `AI6_TEST_LIVE_SOURCES` and the settings each
file names; asked for without them, they fail. Their fixtures are recorded and scrubbed:
`tests/fixtures_scrubbed.rs` refuses a token, an address or an account ID in one.

## Rules

- Work and infrastructure are separate: Operation → Mission → Session is the work;
  Station → Theater is where it runs. Operations are global; no Station owns one.
- The Station (`ai6`) owns Missions, Agent Sessions and policy. Universal Exports owns the
  authenticated public API; Outer Office and `ai6ctl` are its clients and never reach a
  Station directly.
- An Agent (codename) owns its handler, model and effort, or several routes to its model
  (`[[agent.<id>.route]]`), each a handler, a model name and an account source; a role is purpose
  and policy only. The Station picks an available terminal provider that meets the handler's stdio
  requirement (a JSON handler gets one without a terminal, `pipe`; otherwise `pty` is
  preferred) unless the `station` record pins `terminal`, and refuses a pin that does not meet it; the
  handler declares the requirement, and neither the Agent nor the handler names a provider.
- Agent identity, role, Operation policy, Mission ID and provider-issued Session ID are
  distinct contracts.
- Directorates are modules of the `ai6` crate. Providers live under `crates/providers/` and
  carry their own Guatiao entry point; do not add forwarding-only crates.
- Access kinds, the role authorizer, the access providers and Station mutual TLS are the shared
  `coqui-access`, `coqui-roles`, `coqui-peer` and `coqui-peer-tls` crates: `coqui-peer` and
  `coqui-peer-tls` at an exact crates.io version (`coqui-peer` patched to the git revision coqui-agentic-peer uses, so one copy builds), `coqui-access` and `coqui-roles` (unpublished)
  pinned by git revision beside Guatiao. AI6 keeps only its scope kinds, actions and where it
  stores things.
  The host's tests build the access plugins as dev-dependencies and load them from beside the
  test binary: clean `target/` after a plugin is renamed, since the host loads every library.
- Clearance is how far an Agent may act in its own work; a Compartment is what one may know of, and
  say to, other Operations' work. A compartment carries no policy authority and its action list is
  closed. Ask `compartments::current` for every decision: nothing that crosses an Operation answers
  from memory, and each crossing is audited first.
- A project's `.ai6/` configuration is read only for a registered Operation.
- Names come from the Bond / SIS lore in `docs/organization.md`; use a field name only
  where the code has the thing.
- Rust 1.89 is the minimum; stable and 1.89 are checked in CI.
- MPL-2.0. Outside contributors sign `CLA.md`.
- A harness on a remote Theater runs under the field runner (`ai6 runner`) or the
  Station's detached wrapper over the transport; remote safe houses, cross-Station adoption
  of a running Session and federated Registry placement are not implemented, so do not
  describe them as available.

## CI and release

- `.github/workflows/test.yml` runs the workspace matrix, public API snapshots and the
  Flutter app, on demand and on `ci-*` tags, with one leg on floating stable Rust so a new compiler
  is still noticed. The app's jobs and, on `ci-*` tags, a `bootstrap` job on all six platforms
  install their SDKs with `tools/sdk/bootstrap`, through `.github/actions/sdk`.
- `.github/workflows/docs.yml` strictly builds the narrative site and Rust API reference,
  then deploys Pages.
- `.github/workflows/release.yml` gates a source release on tests and a strict docs build.
  Every `v*` tag needs the maintainer's explicit go-ahead for that release.
