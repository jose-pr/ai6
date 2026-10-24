# `coagent_i` — public API header

Mission briefs, the inbox, and the contract user-supplied intake sources fill.
Self-contained: it ships inside the package. Source: <https://github.com/jose-pr/coagent>.

Import from `coagent_i` (or its modules `brief`, `inbox`, `sources`). No runtime
dependencies except `tomli` on Python 3.9 and 3.10. Python 3.9 or newer, any OS.

## `Brief(title, body="", id=<random>, source="manual", station=None, profile=None, role=None, tags=(), priority=0, url=None, data={}, created=<now, UTC ISO>, version=1)`

Frozen. `key` is `<source>--<id>`, filesystem-safe, unique across sources. `prompt` is
the title, the body and `Source: <url>`, blank-line separated. `station` is a station id,
name or alias; `profile` names an agent outright, else M chooses by `role` and `tags`.
`data` is free-form and travels back in reports.

`to_dict()`, `to_json()`, `from_dict(mapping)`, `from_json(text)`. `from_dict` raises
`BriefError` for a missing title, any unknown key (extras go under `data`) or a
`version` newer than `BRIEF_VERSION`.

## `Inbox(root=None)`

`root` defaults to `default_inbox_dir()`: `COAGENT_INBOX`, else `$COAGENT_HOME/inbox`,
else `%LOCALAPPDATA%\coagent\inbox` or `$XDG_STATE_HOME/coagent/inbox`
(`~/.local/state/coagent/inbox`). States are directories: `new`, `claimed`, `done`,
`failed`; a brief is `<state>/<key>.json`, its outcome `<key>.outcome.json`.

- `submit(brief, *, replace=False) -> Path`: into `new/`, written atomically. A key
  already present in any state raises `BriefError`; `replace` replaces only a new one.
- `list(state="new") -> [Brief]`: priority descending, then oldest; unparsable files
  are skipped.
- `claim(key=None) -> Brief | None`: moves the best new brief (or `key`) to `claimed/`
  by rename, so concurrent claimers never get the same one.
- `finish(brief, outcome, *, ok, transcript=None) -> Path`: to `done/` or `failed/`,
  with `<key>.outcome.json` and, given a transcript, `<key>.log` beside it.
- `find(key) -> (state, path) | None`, `outcome(key) -> dict | None`,
  `transcript(key) -> bytes | None`.

Anything may write a brief straight into `new/`: write a temporary file whose name does
not end in `.json` in that directory, then rename it.

## Sources

A source has `name` and `poll() -> Iterable[Brief]`; optionally
`report(brief, outcome: Mapping)`. The outcome M sends has `state`, `returncode`,
`reason`, `runtime`, `agent`, `profile`, `station`.

- `CommandSource(name, command, report_command=None, station=None, timeout=60.0, env={}, cwd=None)`:
  `command` prints a JSON list of briefs, an object with `briefs`, or JSON lines;
  `report_command` reads `{"brief": .., "outcome": ..}` on stdin. Both run with
  `COAGENT_SOURCE` set; non-zero exit or timeout raises `SourceError`. Polled briefs get
  `source = name` and, if they name none, `station`.
- `load_sources(path=None, *, entry_points=True)`: the `coagent.intake` entry points
  (each a source, or a class or callable answering one), then `[[source]]` tables of
  `<config_dir>/sources.toml` (keys: `name`, `command`, `report`, `station`, `timeout`,
  `env`, `cwd`; `./x` is relative to the file). Unknown keys and duplicate names raise
  `SourceError`.
- `poll(sources, inbox) -> (filed, errors)`: each source once; a key already in the
  inbox is not filed again; a failing source is one line in `errors`, not an exception.
- `report(brief, outcome, sources=None) -> bool`: to the brief's source; `False` if it
  is unknown or takes no reports. A failing report is logged, never raised.
- `config_dir()`: the same rule as `coagent_m.config.config_dir`.

## CLI: `python -m coagent_i [--inbox DIR] <command>`

`submit [FILE|-] [--replace]`, `list [--state S] [--json]`, `sources`, `poll` (exits 1
when a source failed). Errors exit 2.
