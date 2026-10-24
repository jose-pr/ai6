# `coagent_m` — public API header

Header-file-style reference for the `coagent_m` package: every public export with its
signature, arguments, contract and gotchas, so the package can be used without reading
its source. It ships inside the package and is self-contained. Development
documentation lives with the source at <https://github.com/jose-pr/coagent>.

Public modules: `coagent_m.watchdog`, `coagent_m.terminal`, `coagent_m.hooks`,
`coagent_m.profiles`, `coagent_m.stations`, `coagent_m.safehouse`, `coagent_m.missions`
(needs `coagent-i`), `coagent_m.config`. Names starting with `_` are private, and so
are the `_`-prefixed submodules of `terminal` and `hooks`.

Install with `pip install -e m/` from a checkout. Runtime dependencies: none, except
`tomli` on Python 3.9 and 3.10. Extras: `guatiao` (terminal plugins in any language,
pinned to a guatiao commit), `conpty` (`pywinpty`, Windows only). `launch --next` and
`--brief` need the sibling package `coagent-i`, imported only then. Python 3.9 or newer.

## Platforms

| | Linux, macOS, BSD | Windows |
| --- | --- | --- |
| `posix-pty` | yes | unavailable |
| `pipes` | yes, tree kill by process group | yes, but **kills the direct child only** |
| `conpty` | unavailable | **scaffold: always unavailable** |
| guatiao plugins | yes | loads; the `portable-pty` plugin kills the direct child only |

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

### `AgentSpec(name, argv, cwd=None, env=None, terminal=None, rows=24, cols=80, hooks=(), input=None, station=None)`

What to run. `env=None` inherits the supervisor's environment; a mapping replaces it.
`terminal` names a provider (default: the best available). `hooks` are agent-scope
hooks. `input` is typed into the terminal, plus a newline, right after the agent starts.
`station` is a station id; the agent gets it as `COAGENT_STATION`.

`AgentSpec.from_profile(profiles, profile, prompt=None, *, name=None, cwd=None, limits=Limits(), station=None) -> (AgentSpec, Limits)`:
the agent a profile (name, alias or resolved `Profile`) describes. The profile's env
and context are applied by an agent-scope hook (so they win over global and project
hooks), and the agent also gets `COAGENT_PROFILE`, `COAGENT_MODEL` and
`COAGENT_EFFORT`. The returned `Limits` are `limits` with the profile's `limits`
table applied. With a `Station`, `cwd` defaults to its path and a profile the station
refuses (`Station.why_not`) raises. Raises `ProfileError`.

### `class Watchdog(spec, limits=Limits(), on_event=None, on_output=None, clock=time.monotonic, registry=None, hooks=None)`

- `run() -> Outcome`, in this order:
  1. picks the provider (`spec.terminal`, or `registry.best()`; `registry=None` uses
     `terminal.default_registry()`);
  2. makes a private run directory (removed afterwards);
  3. runs the hooks' `prepare` (`hooks=None` runs only `spec.hooks`; it loads nothing
     from disk — pass `hooks.load_hooks(cwd, spec.hooks)` for global and project hooks);
  4. writes the collected context to `<run_dir>/context.md` and sets
     `COAGENT_CONTEXT_FILE`; `COAGENT_AGENT` and `COAGENT_RUN_DIR` are always set;
  5. spawns, then supervises until the agent exits or is killed;
  6. kills whatever the agent left running, reaps it, and calls the hooks' `finished`.

  Raises before any event: `ProviderUnavailable`, `HookError` (a required hook
  failed), `TerminalError` (the agent could not start).
- `send(data: bytes) -> None`: operator input; counts as activity for both the stall
  and loop rules. Thread-safe. `RuntimeError` when the agent is not running.
- `kill() -> None`: kills the agent and its descendants now. Thread-safe; no-op after.
- `provider() -> Provider`: what `run()` will use.
- `state: State`, updated on the `run()` thread.
- `on_event(Event)`, `on_output(bytes)` and every hook's `on_event` run on the `run()`
  thread. A slow callback delays the watchdog's own checks.

**Cursor-position queries are answered by the watchdog.** `ESC[6n` is removed from the
output (`on_output` never sees it) and answered with a fixed `ESC[24;1R`, for every
provider. Shells under a pty, and ConPTY at start-up, block until it is answered, so
without this an unattended agent looks hung. A viewer's emulator must not answer it.

### `CursorQueryResponder()`

`feed(data) -> (clean_bytes, [answers])`; a query split across reads is held back until
it completes. `flush() -> bytes` releases a held prefix. `QUERY`, `ANSWER` constants.

### `Outcome(agent, state, returncode, reason, runtime)`

`state` is `DONE` (exit 0), `FAILED` (non-zero exit) or `STALLED` (killed; `reason`
says why). `returncode` follows `subprocess`: a signal is negative. `runtime` in seconds.

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
`feed(data, now)`, `operator_input(now)`, `verdict(now) -> KillReason | None` (checked
in order: max runtime, stall, loop), `blocked(now) -> bool`, `awaiting_input() -> bool`.
`\r` and `\n` both end a line; ANSI CSI and OSC sequences are stripped before comparing.

### CLI: `python -m coagent_m.watchdog <command>`

| Command | Does |
| --- | --- |
| `run [--stall-timeout S] [--loop-timeout S] [--max-runtime S] [--terminal NAME] [--no-hooks] -- CMD...` | supervises `CMD` in the current directory with global and project hooks |
| `launch (--profile NAME \| --role ROLE [--tag T]...) [--station S] [--no-hooks] [PROMPT]` | starts the agent a profile describes, at the station holding `.` or `--station` |
| `launch (--next [--inbox DIR] \| --brief FILE) [--station S] [--profile\|--role ..] [--no-hooks]` | runs a brief: its station, its profile or the best one for its role and tags there, its prompt; files the outcome (`--next`) and reports it to the brief's source |
| `stations` | registered stations |
| `serve [--max N] [--poll S] [--once] [--idle S] [--inbox DIR\|URL] [--keep-safe-houses] [--no-hooks] [--office NAME] [--role control\|execution]... [--label K=V]... [--lease S] [--max-attempts N]` | runs as an office: M when it holds the leader lease, a worker otherwise; exits 1 if any mission did not finish `done` |
| `providers` | terminal providers, best first, with why each cannot run |
| `hooks` | the hooks a run in this directory would use |
| `profiles` | agent profiles, best first |
| `demo [--timeout S] [--terminal NAME]` | a healthy, a looping and a hanging mock agent |

`run` and `launch` exit with the agent's code, or `128 + signal` when it was killed;
`launch` exits 2 for a profile or station error; `--next` exits 0 on an empty inbox. A
brief no profile suits is filed as failed with state `refused` and still reported.

## `coagent_m.terminal`

### `TerminalSpec(argv, cwd=None, env=None, rows=24, cols=80)`

`environment(term=None) -> dict`: the environment to start with; `term` fills `TERM`
only when unset (the pty providers pass `xterm-256color`).

### `Session` (protocol)

`pid: int`; `read(timeout) -> bytes | b"" | None` (data; `b""` once no more output can
ever arrive; `None` when nothing came within `timeout`); `write(bytes)`;
`resize(rows, cols)`; `kill()` (the program and every process it started; idempotent;
no-op once reaped); `poll() -> Exit | None`; `wait(timeout=None) -> Exit | None`;
`close()` (kills first if still running). `write`, `resize` and `kill` are safe from
another thread while `read` blocks. Output can still arrive after the program exits;
an exit is never inferred from EOF alone.

### `Provider` (protocol)

`name: str` (unique in a registry), `priority: int`, `available() -> str | None` (why
it cannot run here), `spawn(TerminalSpec) -> Session` (raises `TerminalError`).

### `Exit(code=None, signal=None)`

`returncode` property: `code`, or `-signal`.

### `class Registry(providers=())`

`add(provider)` (`ValueError` on a duplicate name), `set_priority(name, priority)`
(overrides the provider's own), `providers() -> [ProviderInfo]` best first (priority
descending, then name; each with `reason` asked now), `get(name)` and `best()` (both
raise `ProviderUnavailable`, naming every reason).

### Discovery

- `builtin_providers()`: `posix-pty` (priority 20), `conpty` (20), `pipes` (0).
- `entry_point_providers()`: the `coagent.terminals` entry-point group; each names a
  provider, or a callable answering one or a list. A broken one is logged and skipped.
- `plugin_providers(dirs=None)`: guatiao libraries serving the `coagent_terminal` kind,
  in `dirs` or in `COAGENT_TERMINAL_PLUGINS` (an `os.pathsep` list); priority 30, keyed
  by the provider's guatiao key. Empty without the `guatiao` extra. The native guatiao
  library comes from `GUATIAO_LIBRARY` (a `coagent-native` build).
- `default_registry()`: all three, in that order; a later duplicate name is skipped.

`TerminalError`, `ProviderUnavailable(TerminalError)`.

## `coagent_m.hooks`

A hook is any object with `name` and any of:
`prepare(ctx: HookContext) -> Preparation | None`, `on_event(event)`,
`finished(ctx, outcome)`; `required = True` makes a failing `prepare` raise `HookError`
instead of being logged and skipped. `HookBase` is an optional no-op base.

- `HookContext(agent, argv, cwd, env, run_dir, context=[])`: mutable; each hook sees
  what earlier ones prepared. `as_json()` is what a command hook reads on stdin.
- `Preparation(env={}, context=(), argv=None)`: `env` merges (`None` removes a key),
  `context` appends, `argv` replaces.
- `CommandHook(name, command, output="json", required=False, timeout=30.0, env=None)`:
  runs `command` in the agent's directory with `COAGENT_HOOK`, `COAGENT_AGENT`,
  `COAGENT_RUN_DIR`, `COAGENT_CWD` set and the context as JSON on stdin. `output` is how
  stdout is read: `env` (`K=V` / `export K=V` lines), `context` (text), `json`
  (`{"env": .., "context": .., "argv": ..}`) or `none`. Non-zero exit or timeout →
  `HookError`.
- `HookSet(hooks)`: `(Scope, hook)` pairs in run order; `prepare(ctx)`,
  `on_event(event)`, `finished(ctx, outcome)`.
- `load_hooks(cwd=None, agent_hooks=(), *, trust_project=None) -> HookSet`: global
  hooks (the `coagent.hooks` entry-point group, then `<config_dir>/hooks.toml`), then the
  `.coagent/hooks.toml` of the station holding `cwd`, then `agent_hooks`. A `.coagent`
  no station holds is read only with `trust_project=True` or `COAGENT_TRUST_PROJECT=1`.
  `stations=` passes a registry instead of loading `stations.toml`.
- `load_file(path, scope)`, `entry_point_hooks()`, `parse_output(kind, text)`.

`hooks.toml`:

```toml
[[hook]]
name = "env-for-this-dir"
command = ["some-tool", "env"]   # a list; "./x" is relative to this file
output = "env"                    # env | context | json | none
required = false
timeout = 30
env = { KEY = "value" }

[[hook]]
python = "package.module:attr"    # an object, or a class built with no arguments
```

## `coagent_m.profiles`

- `Launcher(name, command=(), prompt="arg", env={}, adapter=None, options={})`: how to
  start one agent CLI — a `command` template, or an `adapter` (found by
  `load_adapter(name)` in the `coagent.adapters` entry-point group, or
  `register_adapter(obj)`) given a `LaunchRequest(profile, launcher, prompt, cwd,
  station, cover)` (`clearance`, `options` = launcher options under profile `vars`).
  An argv element `{context}` / `{context_file}` is filled after hooks run
  (`watchdog.fill_context`) and dropped with its flag when there is no context.
  `command` placeholders: `{model}`, `{effort}`, `{profile}`, `{prompt}`, `{cwd}` and
  the profile's `vars`. `prompt`: `arg` appends the prompt, `stdin` types it after
  start, `none` drops it; a `{prompt}` placeholder overrides all three.
- `Profile(name, aliases=(), description="", extends=None, launcher=None, model=None, effort=None, roles=(), tags=(), priority=0, env={}, args=(), vars={}, terminal=None, limits={}, context="", max_concurrent=None, clearance=None)`:
  a model configuration under a codename. `extends` inherits every unset field; `env`,
  `vars` and `limits` merge; aliases are never inherited. `suits(role=None, tags=())`:
  lists the role or a parent of it (`review` suits `review/security`) and every tag.
  `max_concurrent` is recorded for a scheduler; nothing enforces it yet. `clearance` is
  one of `stations.CLEARANCES`; unset means `field`.
- `ProfileSet()`: `add_launcher`, `add` (a later name replaces an earlier one),
  `names()`, `get(name_or_alias)` (resolved), `profiles()` (resolved, priority
  descending, then name), `choose(role=None, tags=(), station=None)` (with a station,
  only profiles it accepts),
  `argv(profile, prompt=None, cwd=None) -> (argv, env, typed_prompt)`.
- `load_profiles(cwd=None, *, trust_project=None, stations=None)`:
  `<config_dir>/profiles.toml`, then the station's `.coagent/profiles.toml`, which
  overrides a global profile of the same name.
- `load_file(path, scope, into)`: raises `ProfileError` for an unknown key, so a typo
  is an error, not a missing setting.

## `coagent_m.stations`

- `Station(id, name="", description="", path=None, theater=None, config=None, repos=(), roster=(), clearance="field", max_concurrent=None, selector={}, safe_houses=True, branch="{id}-{slug}", base="HEAD", cover="covert", identity=None, on_blown="flag", intake={})`:
  `id` is the station's one cover name (any short code); `name` the real project name;
  the station answers to both. `theater` names the theater its checkout (`path`, a path
  there) is on; `config` is a local directory with its hooks and profiles (needed for a
  remote station to have any). `roster`
  empty means every profile. `intake` is for intake sources; M ignores it.
  `contains(path)`, `label`, `why_not(profile) -> str | None` (not on the roster, or
  cleared above the station).
- `Stations(stations=())`: `add` (`StationError` when an id or name is taken),
  iteration by id, `get(id_or_name)`, `for_path(path)` (the deepest station
  holding it), `Stations.load(path=None)` from `<config_dir>/stations.toml`; unknown keys
  and unknown clearances raise `StationError`.
- `CLEARANCES = ("desk", "field", "00")`, lowest first; `clearance_rank(c)`.
- `local_config(cwd, stations=None, *, trust=None) -> (dir | None, Station | None)`: the
  `.coagent` to read for `cwd`: the station's, else the nearest one only when trusted.

`stations.toml`:

```toml
[station.G]
name = "guatiao"
path = "~/devel/guatiao"
repos = ["https://github.com/Boriken-Dev/guatiao"]
roster = ["007", "006"]
clearance = "field"
max_concurrent = 3
intake = { jira = "GUA" }
```

## `coagent_m.safehouse`

- `SafeHouse.create(repo, key, branch, *, root=None, base="HEAD") -> SafeHouse`: a new
  worktree at `<root>/<key>` (default root `state_dir()/safe-houses`) on a new `branch`
  from `base`. `SafeHouseError` (with git's words) when the path or branch exists or
  `repo` is not a git checkout.
- `SafeHouse`: `repo`, `path`, `branch`, `base` (a commit); `commits()` since `base`,
  `dirty()`, `cleanup(*, keep=False) -> kept` (removes the worktree and its branch only
  when there are no commits and no changes), `describe() -> dict`.
- `branch_name(template, *, id, title, station="")`: `{id}`, `{slug}`, `{station}` filled
  and made a valid ref.
- `state_dir()`: `COAGENT_STATE`, else `$COAGENT_HOME/state`, else `%LOCALAPPDATA%\coagent`
  or `$XDG_STATE_HOME/coagent`. `is_git_checkout(path)`.

## `coagent_m.missions` (needs `coagent-i`)

- `MissionRunner(inbox=None, *, stations=None, sources=None, use_hooks=True, registry=None, safe_house_root=None, keep_safe_houses=False, on_event=None, on_output=None)`:
  `on_event(key, Event)` and `on_output(key, bytes)` run on the mission's thread.
  - `assign(brief, *, station=None, profile=None, role=None, tags=()) -> Assignment`:
    the brief's station (else the one holding the current directory), its profile
    (else the best for its role and tags there), the `AgentSpec` and `Limits`. Raises
    `StationError` / `ProfileError`.
  - `run(brief, assignment=None, *, claimed=False, **assign) -> dict`: cuts a safe house
    when the station is a git checkout with `safe_houses` on (station hooks and profiles
    are still read from the station's own path), runs the agent, cleans the safe house
    up, files the outcome and the transcript (the last `TRANSCRIPT_LIMIT` bytes) when
    `claimed`, and always reports to the brief's source. The outcome has `agent`,
    `profile`, `station`, `state` (`done`, `failed`, `stalled`, or `refused` when it
    could not be assigned or started), `returncode`, `reason`, `runtime` and, with a
    safe house, `safe_house` (`path`, `branch`, `base`, `kept`, `commits`).
- `MissionRunner.abandon(key)`: stops a mission without filing or reporting it (its
  brief belongs to another office); `run(..., office=None)` files as `office` and answers
  state `abandoned` when the lease passed elsewhere.
- `Scheduler(runner, *, office=None, max_total=4, poll_interval=None, lease=60.0, max_attempts=3, roles=("control", "execution"), labels=None)`:
  an office. `step()` renews its leases (abandoning any it lost), heartbeats, takes or
  renews the leader lease if it has `control`, and as leader polls intake, recovers
  lapsed leases and places waiting briefs; with `execution` it then starts what is
  leased to it. Placement: live execution offices with room, labels matching the
  station's `selector` (`fnmatch` globs; automatic labels `office`, `host`, `os`), able to
  reach a local checkout, least loaded first. Budgets (`max_concurrent` of stations and
  profiles) count leases across offices. `run(*, once, idle)`, `stop()`, `wait()`,
  `leading`, `running`, `finished`. `ROLES = ("control", "execution")`; an unknown role
  is `ValueError`.

## `coagent_m.cover`

- `Cover(name, description="", agent_author=False, trailers=(), suggest="", model=False, footer="", forbid=(), announce=False)`;
  `briefing(identity, profile, model) -> str` (what the agent is told).
- `BUILTIN_COVERS`: `covert`, `partial` (`Assisted-by` only), `overt`.
- `Covers(covers=None)`: built-ins plus custom; `get(name)`, `names()`,
  `Covers.load(path=None)` from `<config_dir>/covers.toml` (`CoverError` for unknown keys
  or a bad pattern).
- `Identity(name, email)`, `git_env()` (`GIT_AUTHOR_*`, `GIT_COMMITTER_*`).
- `check_commits(commits, cover, identity=None, *, model="") -> [Violation]`: each commit
  a dict of `sha`, `author`, `committer` (`Name <email>`), `message`. Rules: `author`,
  `committer`, `trailer` (attribution trailers in the last paragraph: Co-Authored-By,
  Generated-by, Assisted-by, Written-by, `*-Session`), `model`, `announce`, `forbid`,
  `footer`. `Violation(commit, rule, detail)`, `as_dict()`.

Stations carry `cover` (default `covert`), `identity` and `on_blown` (`flag` or
`fail`). `MissionRunner` applies them; the outcome gains `cover: {policy, commits, blown,
violations}`, and state `blown` when the station fails on it.

## `coagent_m.theaters`

- `Theater(id, ssh, os="linux", description="", port=None, identity=None, options={}, safe_houses=None, tags=(), client=("ssh",))`:
  `provider_name` (`ssh:<id>`), `posix`, `ssh_argv(*, tty)`.
- `Theaters`: `add`, `get`, iteration, `providers()`, `Theaters.load(path=None)` from
  `<config_dir>/theaters.toml` (`TheaterError` for unknown keys or an unknown `os`).
- The `ssh:<id>` provider runs the agent through `ssh -tt` on the best local terminal,
  forwards only the environment M and its hooks added, and on a POSIX theater kills the
  remote session (`pkill -s`) over a second connection. Windows theaters: commands run,
  but a kill only drops the connection (scaffold).

## `coagent_m.config`

- `config_dir()`: `COAGENT_HOME`, else `%APPDATA%\coagent` on Windows, else
  `$XDG_CONFIG_HOME/coagent` (`~/.config/coagent`).
- `find_project(cwd)`: the nearest directory at or above `cwd` with a `.coagent` dir.
- `Scope`: `global`, `station`, `agent` — read in that order, narrower wins.
- `read_toml(path)`: the tables, or `None` when missing or unreadable (logged).
- `TRUST_ENV` (`COAGENT_TRUST_PROJECT`): read an unregistered checkout's `.coagent`.
  **A `.coagent` holds hooks and launchers that run inside the supervisor, so only a
  registered station's, or one explicitly trusted, is ever read.**
