# coagent — repository orientation

Contributor notes for a checkout. The shipped API header for a package is the
`AGENTS.md` inside it (`m/src/coagent_m/AGENTS.md`); this file is about the
repository.

## Layout

| Path | What |
| --- | --- |
| `m/` | Python project `coagent-m` (package `coagent_m`, `src/` layout): the supervisor. Only `watchdog.py` exists |
| `m/tests/` | pytest suite; the integration tests run the watchdog's own mock agents with sub-second limits |
| `moneypenny/` | design for a Cargo workspace: gateway (axum) and a wasm client sharing one protocol crate |
| `q/` | design for the Python package `coagent_q`: secrets broker, sandbox profiles, MCP loadouts |

## Rules

- The three components share no code. They meet at two formats: M's socket frames
  (guatiao values; schema in `moneypenny-proto`, read from Python through guatiao's
  bindings) and Q's loadout TOML.
- Nothing in M's watchdog may assume a backend: it sees bytes and timestamps only.
- POSIX only (`pty`, process groups, SIGKILL).
- Python 3.9 floor; `from __future__ import annotations` in every module; black,
  line length 100.
- MPL-2.0 (`LICENSE`), with a CLA for outside contributions (`CLA.md`,
  `.github/workflows/cla.yaml`). `m/LICENSE` is a copy of the root one, because a
  Python package can only ship a licence file from inside its own project root.

## Commands

```bash
cd m
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
.pyvenv/local/bin/pytest -q
.pyvenv/local/bin/black --check src tests
```
