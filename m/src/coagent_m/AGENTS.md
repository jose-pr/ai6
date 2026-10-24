# `coagent_m` — public API header

Header-file-style reference for the `coagent_m` package: every public export with its
signature, arguments, contract and gotchas, so the package can be used without reading
its source. It ships inside the package and is self-contained. Development
documentation lives with the source at <https://github.com/jose-pr/coagent>.

Everything public is in `coagent_m.watchdog`; names starting with `_` are private.
Install with `pip install -e m/` from a checkout. There are no extras and no runtime
dependencies. POSIX only: importing the module needs `pty`.

## `coagent_m.watchdog`

### `Limits(stall_timeout=300.0, loop_timeout=300.0, loop_min_lines=50, blocked_after=10.0, max_runtime=None, history=4096, normalize_digits=False)`

Frozen dataclass; every duration is in seconds.

- `stall_timeout`: no output **and** no operator input for this long → kill.
- `loop_timeout`, `loop_min_lines`: output continues, but no novel line for
  `loop_timeout` seconds and at least `loop_min_lines` repeated lines since the last
  novel one → kill.
- `blocked_after`: a prompt-shaped tail that has been quiet this long reports `BLOCKED`.
- `max_runtime`: hard wall-clock cap; `None` means no cap.
- `history`: distinct lines remembered. A cycle longer than this reads as new output.
- `normalize_digits`: compare lines with digit runs folded, so `retry 1` and `retry 2`
  count as the same line. Off by default, because counters are also how progress looks.

### `AgentSpec(name, argv, cwd=None, env=None)`

What to run. `env=None` inherits the supervisor's environment; pass a full mapping to
replace it.

### `class Watchdog(spec, limits=Limits(), on_event=None, on_output=None, clock=time.monotonic)`

- `run() -> Outcome`: starts `argv` on a new pty in a new session (its pgid is its pid)
  and blocks until it exits or is killed. On return, the whole process group has been
  sent SIGKILL and the agent has been reaped, so descendants of a finished agent do not
  survive it.
- `send(data: bytes) -> None`: writes operator input to the terminal, and counts as
  activity for both the stall and loop rules. Thread-safe. Raises `RuntimeError` when
  the agent is not running.
- `kill() -> None`: SIGKILL the process group now. Thread-safe; no-op after exit.
- `state: State`: the current state, updated on the `run()` thread.
- `on_event(Event)` and `on_output(bytes)` run on the `run()` thread. A slow callback
  delays the watchdog's own checks.

### `Outcome(agent, state, returncode, reason, runtime)`

`state` is `DONE` (exit 0), `FAILED` (non-zero exit) or `STALLED` (killed; `reason`
says why). `returncode` follows `subprocess`, so a SIGKILL is `-9`; `runtime` is in
seconds.

### `Event(agent, state, at, detail="")`

A transition, with `at` taken from `time.time()`. One run emits, in order: `WORKING`,
then `BLOCKED` and `WORKING` alternating any number of times, then exactly one of
`DONE`/`FAILED`/`STALLED`.

### `State`, `KillReason`

`str` enums. `State`: `pending`, `working`, `blocked`, `stalled`, `done`, `failed`.
`KillReason`: `input_hang` (quiet at a prompt), `silent` (quiet, no prompt), `loop`,
`max_runtime`.

### `class ActivityMonitor(limits, now)`

The stall and loop rules with no process attached; drive it with any clock.

- `feed(data, now)`, `operator_input(now)`.
- `verdict(now) -> KillReason | None`, checked in this order: max runtime, stall, loop.
- `blocked(now) -> bool`, `awaiting_input() -> bool`.

`\r` and `\n` both end a line. ANSI CSI and OSC sequences are stripped before lines are
compared.

### CLI

`python -m coagent_m.watchdog run [--stall-timeout S] [--loop-timeout S] [--max-runtime S] -- CMD...`
exits with the agent's code, or `128 + signal` when the agent was killed.
`python -m coagent_m.watchdog demo [--timeout S]` runs the three built-in mock agents.
