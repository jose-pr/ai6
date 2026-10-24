# coagent — repository orientation

Contributor notes for a checkout. The shipped API header for a package is the
`AGENTS.md` inside it (`m/src/coagent_m/AGENTS.md`); this file is about the
repository.

## Layout

| Path | What |
| --- | --- |
| `m/` | Python project `coagent-m` (package `coagent_m`, `src/` layout): the supervisor — `watchdog`, `terminal/` (providers), `hooks/`, `profiles`, `config` |
| `m/tests/` | pytest suite; the watchdog cases run its own mock agents (`mock-agent work\|loop\|hang\|cursor\|orphan`) with sub-second limits, once per terminal provider that can run here |
| `m/crates/coagent-terminal-kind/` | the terminal contract as guatiao kinds; its test pins both floor hashes |
| `m/crates/coagent-terminal-pty/` | a terminal plugin (`cdylib`) on `portable-pty` |
| `i/` | Python project `coagent-i` (package `coagent_i`): briefs, the inbox, the intake-source contract; `i/tests/` |
| `00/` | Python project `coagent-00` (package `coagent_00`): agent-CLI adapters, `claude` first, registered in the `coagent.adapters` entry-point group; `00/tests/` (one checks every emitted flag against the installed `claude --help`); `roster.example.toml` |
| `docs/`, `mkdocs.yml` | the documentation site: hand-written pages, the Python reference (mkdocstrings over `m/src` and `i/src`), the changelog by snippet; rustdoc is copied under `/rust/` by `docs.yml`. Build: `pip install -e "m[docs]" -e i && mkdocs build --strict` |
| `m/crates/coagent-native/` | a `cdylib` carrying guatiao's C ABI with `load`, for the Python binding |
| `Cargo.toml` | the workspace; guatiao is a git dependency pinned to a commit, the same commit `m/pyproject.toml`'s `guatiao` extra names |
| `moneypenny/` | design for a Cargo workspace: gateway (axum) and a wasm client sharing one protocol crate |
| `q/` | design for the Python package `coagent_q`: secrets broker, sandbox profiles, MCP loadouts |

## Rules

- No division (`m/`, `00/`, `q/`, `i/`, `moneypenny/`) imports another, except that any
  may use I Branch's formats: briefs (`coagent_i`), and the wire frames (a crate in `i/`,
  planned). M imports `coagent_i` only inside `launch --next/--brief`.
- Intake integrations (Jira, webhooks, ...) are never built in: they are user-supplied
  sources, entry points or programs writing into the inbox. Keep that contract
  (`i/src/coagent_i/AGENTS.md`) the only thing coagent owns there.
- Nothing in M's watchdog may assume a terminal provider: it sees bytes and timestamps
  only. Platform code lives in a provider.
- The two floor hashes in `coagent-terminal-kind`'s test and in
  `m/src/coagent_m/terminal/_guatiao.py` are one fact in two places: change both, and
  bump guatiao in `Cargo.toml` and `m/pyproject.toml` together.
- A project's `.coagent/` is read only when it is a registered station (or explicitly
  trusted); never add a code path that reads it otherwise.
- Windows: `conpty` and process-tree kill (Job Objects) are scaffolds, marked as such
  in their modules and in `m/src/coagent_m/AGENTS.md`.
- Python 3.9 floor; `from __future__ import annotations` in every module; black,
  line length 100.
- MPL-2.0 (`LICENSE`), with a CLA for outside contributions (`CLA.md`,
  `.github/workflows/cla.yaml`). `m/LICENSE` is a copy of the root one, because a
  Python package can only ship a licence file from inside its own project root.

## Languages

The control plane (M's decisions, the plugin surfaces users extend, I's formats) is
Python. The execution plane moves to Rust piece by piece behind the existing seams: the
terminal plugin is Rust today; an execution office as one static binary, and
Moneypenny's gateway, are next. Do not port working Python for its own sake; the tests
are the specification a Rust replacement must pass.

## Commands

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p coagent-native -p coagent-terminal-pty

cd m
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev,guatiao]"
export GUATIAO_LIBRARY=$PWD/../target/debug/libcoagent_native.so COAGENT_TERMINAL_PLUGINS=$PWD/../target/debug
.pyvenv/local/bin/pytest -q
.pyvenv/local/bin/black --check src tests
```

The test suite needs the package installed (`pip install -e`), not only on
`sys.path`: the watchdog cases start `python -m coagent_m.watchdog mock-agent ...` in a
child process.

`i/` has its own venv and suite (`cd i && pip install -e ".[dev]" && pytest -q`); M's
mission tests skip unless `coagent-i` is installed into M's venv too (`pip install -e
../i`).

CI: `.github/workflows/test.yml`, on `workflow_dispatch` or a pushed `ci-*` tag; Linux
and macOS, Python 3.9 and 3.14, Rust fmt, clippy and tests.
