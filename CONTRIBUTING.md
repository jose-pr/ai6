# Contributing to coagent

## Licence

This project is distributed under the **Mozilla Public License 2.0**
(`LICENSE`). Its copyleft is per file, which means:

- **Running coagent imposes nothing on your code.** The agents it supervises,
  your loadouts, your tracker integrations and anything else you write beside
  it are yours, under whatever licence you choose.
- **Changing coagent's own files does carry an obligation.** A modified file
  of this project must be published under the MPL 2.0 to the people you ship
  it to.
- A combined work may be distributed under terms of your choosing (MPL 2.0
  section 3.3), and MPL 2.0 remains compatible with the GPL.

## Contributor licence agreement

A first pull request needs a one-time signature. The bot will comment with
the exact sentence to reply with; read [`CLA.md`](CLA.md) first.

It exists so the project can be relicensed later without tracking down every
past contributor. You keep the copyright in everything you write.

## Before you open a pull request

```bash
cd m
python3 -m venv .pyvenv/local && .pyvenv/local/bin/pip install -e ".[dev]"
.pyvenv/local/bin/pytest -q
.pyvenv/local/bin/black --check src tests
```

Both must be clean. See [`AGENTS.md`](AGENTS.md) for the repository's layout
and the rules every change keeps:

- the three components share no code, only the socket frames and the loadout
  format;
- the watchdog sees bytes and timestamps, never a session backend;
- a secret value never reaches a worktree, a log or a terminal stream;
- files are LF, on every platform.
