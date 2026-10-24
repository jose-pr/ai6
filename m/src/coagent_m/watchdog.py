"""Supervisor watchdog: run one sub-agent on a terminal and kill it when it stalls or loops.

The terminal comes from a provider (``coagent_m.terminal``): a POSIX pty, a Windows
pseudo-console, plain pipes, or a plugin. Whichever it is, the provider kills the agent
together with every process it started. Three conditions end a run early:

* **input hang / silence** -- no output and no operator input for ``stall_timeout``;
* **loop** -- output keeps arriving but no line has been new for ``loop_timeout``;
* **max runtime** -- an optional hard cap on wall-clock time.
"""

from __future__ import annotations

import argparse
import enum
import hashlib
import os
import re
import shutil
import sys
import tempfile
import threading
import time
from collections import OrderedDict
from dataclasses import dataclass, replace
from typing import Callable, Dict, List, Optional, Sequence, Tuple

from .config import Scope
from .hooks import Hook, HookBase, HookContext, HookSet, Preparation, load_hooks
from .profiles import Profile, ProfileError, ProfileSet, load_profiles
from .stations import Station, StationError, Stations
from .terminal import (
    DEFAULT_COLS,
    DEFAULT_ROWS,
    Provider,
    Registry,
    Session,
    TerminalError,
    TerminalSpec,
    default_registry,
)

__all__ = [
    "AgentSpec",
    "ActivityMonitor",
    "CursorQueryResponder",
    "Event",
    "KillReason",
    "Limits",
    "Outcome",
    "State",
    "Watchdog",
]

DEFAULT_STALL_TIMEOUT = 300.0

# CSI and OSC escape sequences; colour and cursor movement must not make a repeated line look new.
_ANSI = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
_DIGITS = re.compile(rb"\d+")
# A trailing, newline-less fragment shaped like a question waiting for an answer.
_PROMPT = re.compile(rb"(?:[?:>$#\]\)]\s*$)|\[y/n\]|\(y/n\)|password", re.IGNORECASE)
_MAX_PARTIAL = 4096
# Output a session may still deliver after its program exits.
_DRAIN_READS = 20
_DRAIN_TIMEOUT = 0.05


class State(str, enum.Enum):
    """Lifecycle state of a supervised agent, as shown on the Moneypenny board."""

    PENDING = "pending"
    WORKING = "working"
    BLOCKED = "blocked"  # quiet at what looks like a prompt; still inside stall_timeout
    STALLED = "stalled"  # killed by the watchdog
    DONE = "done"
    FAILED = "failed"


class KillReason(str, enum.Enum):
    """Why the watchdog killed an agent."""

    INPUT_HANG = "input_hang"
    SILENT = "silent"
    LOOP = "loop"
    MAX_RUNTIME = "max_runtime"


@dataclass(frozen=True)
class Limits:
    """Watchdog thresholds. Every duration is in seconds."""

    stall_timeout: float = DEFAULT_STALL_TIMEOUT
    loop_timeout: float = DEFAULT_STALL_TIMEOUT
    # Repeated lines required since the last novel one, so a slow but quiet agent is not a loop.
    loop_min_lines: int = 50
    blocked_after: float = 10.0
    max_runtime: Optional[float] = None
    # Distinct lines remembered; a cycle longer than this reads as novel output.
    history: int = 4096
    # Treat "retry 1" and "retry 2" as the same line. Off by default: counters are also progress.
    normalize_digits: bool = False


@dataclass(frozen=True)
class AgentSpec:
    """What to run: ``argv`` in ``cwd`` (a safe house: a git worktree) with ``env`` (from ``q``), on the
    terminal provider named ``terminal`` (default: the best one available)."""

    name: str
    argv: Sequence[str]
    cwd: Optional[str] = None
    env: Optional[Dict[str, str]] = None
    terminal: Optional[str] = None
    rows: int = DEFAULT_ROWS
    cols: int = DEFAULT_COLS
    # Agent-scope hooks; they run after global and project ones.
    hooks: Sequence[Hook] = ()
    # Typed into the terminal once the agent starts, followed by a newline.
    input: Optional[str] = None
    # The id of the station the mission is for; the agent sees it as COAGENT_STATION.
    station: Optional[str] = None
    # The theater it runs on, when not this machine (COAGENT_THEATER); `cwd` is a path there.
    theater: Optional[str] = None

    @classmethod
    def from_profile(
        cls,
        profiles: ProfileSet,
        profile: "str | Profile",
        prompt: Optional[str] = None,
        *,
        name: Optional[str] = None,
        cwd: Optional[str] = None,
        limits: Limits = Limits(),
        station: Optional[Station] = None,
    ) -> Tuple["AgentSpec", Limits]:
        """The agent ``profile`` (a name, an alias or a resolved ``Profile``) describes, given
        ``prompt``, and ``limits`` with the profile's overrides. With a ``station``, ``cwd``
        defaults to its path and a profile it does not take is refused. Raises
        ``ProfileError``."""
        if isinstance(profile, str):
            profile = profiles.get(profile)
        if station is not None:
            why = station.why_not(profile)
            if why:
                raise ProfileError(why)
            if cwd is None and station.path is not None:
                cwd = str(station.path)
        cover = None
        if station is not None:
            from .cover import CoverError, Covers

            try:
                cover = Covers.load().get(station.cover)
            except CoverError as exc:
                raise ProfileError(str(exc)) from exc
        argv, env, typed = profiles.argv(profile, prompt, cwd, station=station, cover=cover)
        try:
            limits = replace(limits, **profile.limits)
        except TypeError as exc:
            raise ProfileError(f"profile {profile.name!r}: bad limits: {exc}") from exc
        spec = cls(
            name=name or profile.name,
            argv=argv,
            cwd=cwd,
            terminal=profile.terminal,
            hooks=(_ProfileHook(profile, env),),
            input=typed,
            station=station.id if station is not None else None,
            theater=station.theater if station is not None else None,
        )
        if spec.theater:
            # The checkout is on the theater, so the agent runs there whatever its terminal.
            spec = replace(spec, terminal=f"ssh:{spec.theater}")
        return spec, limits


@dataclass(frozen=True)
class Event:
    """A state transition, published to whoever renders the board."""

    agent: str
    state: State
    at: float  # time.time()
    detail: str = ""


@dataclass(frozen=True)
class Outcome:
    """How a run ended. ``returncode`` is negative for a signal, as in ``subprocess``."""

    agent: str
    state: State
    returncode: Optional[int]
    reason: Optional[KillReason]
    runtime: float


class CursorQueryResponder:
    """Answers the cursor-position query (``ESC[6n``) and removes it from the output.

    Shells under a pty, and ConPTY itself, send it and block until answered, so an agent
    with no emulator attached would look hung. The answer is always row 24, column 1: the
    query is a handshake, and the real size may not have reached the terminal yet.
    Removing it keeps an attached viewer's emulator from answering a second time.
    """

    QUERY = b"\x1b[6n"
    ANSWER = b"\x1b[24;1R"

    def __init__(self) -> None:
        self._held = b""

    def feed(self, data: bytes) -> Tuple[bytes, List[bytes]]:
        """The output without queries, and one answer per query. A query split across
        two reads is held back until it completes."""
        data = self._held + data
        self._held = b""
        count = data.count(self.QUERY)
        if count:
            data = data.replace(self.QUERY, b"")
        for keep in range(len(self.QUERY) - 1, 0, -1):
            if data.endswith(self.QUERY[:keep]):
                data, self._held = data[:-keep], data[-keep:]
                break
        return data, [self.ANSWER] * count

    def flush(self) -> bytes:
        """Whatever was held back, once no more output will arrive."""
        held, self._held = self._held, b""
        return held


class ActivityMonitor:
    """Decides stall, loop and blocked from output timing alone; holds no process or fd.

    Every method takes ``now`` from a monotonic clock, so it can be driven by a fake one.
    """

    def __init__(self, limits: Limits, now: float) -> None:
        self.limits = limits
        self.started = now
        self.last_activity = now
        self.last_novel = now
        self.lines_since_novel = 0
        self._seen: "OrderedDict[bytes, None]" = OrderedDict()
        self._partial = b""

    def feed(self, data: bytes, now: float) -> None:
        """Account for output; ``\\r`` ends a line so a spinner redrawing in place repeats."""
        self.last_activity = now
        lines = re.split(rb"[\r\n]", self._partial + data)
        self._partial = lines.pop()
        for line in lines:
            self._line(line, now)
        if len(self._partial) > _MAX_PARTIAL:
            self._line(self._partial, now)
            self._partial = b""

    def operator_input(self, now: float) -> None:
        """A human typed into the session: it is being driven, so neither clock may fire."""
        self.last_activity = now
        self.last_novel = now
        self.lines_since_novel = 0

    def awaiting_input(self) -> bool:
        """Whether the unterminated tail of the output looks like a prompt."""
        tail = _ANSI.sub(b"", self._partial[-256:])
        return bool(tail.strip()) and _PROMPT.search(tail) is not None

    def blocked(self, now: float) -> bool:
        return self.awaiting_input() and now - self.last_activity >= self.limits.blocked_after

    def verdict(self, now: float) -> Optional[KillReason]:
        """The reason to kill the agent now, or ``None`` to let it run."""
        limits = self.limits
        if limits.max_runtime is not None and now - self.started >= limits.max_runtime:
            return KillReason.MAX_RUNTIME
        if now - self.last_activity >= limits.stall_timeout:
            return KillReason.INPUT_HANG if self.awaiting_input() else KillReason.SILENT
        if (
            now - self.last_novel >= limits.loop_timeout
            and self.lines_since_novel >= limits.loop_min_lines
        ):
            return KillReason.LOOP
        return None

    def _line(self, raw: bytes, now: float) -> None:
        key = _ANSI.sub(b"", raw).strip()
        if self.limits.normalize_digits:
            key = _DIGITS.sub(b"#", key)
        if not key:
            return
        digest = hashlib.blake2b(key, digest_size=8).digest()
        if digest in self._seen:
            self._seen.move_to_end(digest)
            self.lines_since_novel += 1
            return
        self._seen[digest] = None
        if len(self._seen) > self.limits.history:
            self._seen.popitem(last=False)
        self.last_novel = now
        self.lines_since_novel = 0


class Watchdog:
    """Runs one agent to completion or death.

    ``run()`` blocks; ``send()`` and ``kill()`` are safe to call from other threads, which
    is how an operator attached through Moneypenny types into the session.
    ``on_output`` receives the terminal's bytes (cursor queries removed); ``on_event``
    receives state transitions. Both are called on the ``run()`` thread and must not block.
    """

    def __init__(
        self,
        spec: AgentSpec,
        limits: Limits = Limits(),
        on_event: Optional[Callable[[Event], None]] = None,
        on_output: Optional[Callable[[bytes], None]] = None,
        clock: Callable[[], float] = time.monotonic,
        registry: Optional[Registry] = None,
        hooks: Optional[HookSet] = None,
    ) -> None:
        self.spec = spec
        self.limits = limits
        self.state = State.PENDING
        self._on_event = on_event
        self._on_output = on_output
        self._clock = clock
        self._registry = registry
        self._hooks = hooks if hooks is not None else HookSet((Scope.AGENT, h) for h in spec.hooks)
        self._lock = threading.Lock()
        self._monitor: Optional[ActivityMonitor] = None
        self._session: Optional[Session] = None

    def send(self, data: bytes) -> None:
        """Write operator input to the agent's terminal. Raises ``RuntimeError`` if not running."""
        with self._lock:
            if self._session is None or self._monitor is None:
                raise RuntimeError(f"agent {self.spec.name!r} is not running")
            self._session.write(data)
            self._monitor.operator_input(self._clock())

    def kill(self) -> None:
        """Kill the agent and everything it started. A no-op once it has exited."""
        session = self._session
        if session is not None:
            session.kill()

    def provider(self) -> Provider:
        """The terminal provider ``run()`` will use. Raises ``ProviderUnavailable``."""
        registry = self._registry if self._registry is not None else default_registry()
        name = self.spec.terminal
        return registry.get(name) if name else registry.best()

    def run(self) -> Outcome:
        """Start the agent and supervise it until it exits or is killed.

        Hooks prepare it first; their context reaches the agent as the file named by
        ``COAGENT_CONTEXT_FILE``. Raises ``ProviderUnavailable`` when no terminal can run
        here, ``HookError`` when a required hook fails, and ``TerminalError`` when the
        agent cannot start; none of them emits an event.
        """
        provider = self.provider()
        run_dir = tempfile.mkdtemp(prefix=f"coagent-{_safe(self.spec.name)}-")
        try:
            ctx = self._prepare(run_dir)
            return self._run(provider, ctx)
        finally:
            shutil.rmtree(run_dir, ignore_errors=True)

    def _prepare(self, run_dir: str) -> HookContext:
        env = dict(os.environ if self.spec.env is None else self.spec.env)
        env["COAGENT_AGENT"] = self.spec.name
        env["COAGENT_RUN_DIR"] = run_dir
        if self.spec.station:
            env["COAGENT_STATION"] = self.spec.station
        if self.spec.theater:
            env["COAGENT_THEATER"] = self.spec.theater
        ctx = HookContext(
            self.spec.name,
            list(self.spec.argv),
            self.spec.cwd,
            env,
            run_dir,
            theater=self.spec.theater,
        )
        self._hooks.prepare(ctx)
        text = "\n\n".join(ctx.context)
        path: Optional[str] = None
        if ctx.context:
            path = os.path.join(run_dir, "context.md")
            with open(path, "w", encoding="utf-8", newline="") as f:
                f.write(text + "\n")
            ctx.env["COAGENT_CONTEXT_FILE"] = path
        ctx.argv = fill_context(ctx.argv, text if ctx.context else None, path)
        return ctx

    def _run(self, provider: Provider, ctx: HookContext) -> Outcome:
        start = self._clock()
        session = provider.spawn(
            TerminalSpec(ctx.argv, ctx.cwd, ctx.env, self.spec.rows, self.spec.cols)
        )
        with self._lock:
            self._session = session
            self._monitor = ActivityMonitor(self.limits, start)
        self._transition(State.WORKING, f"pid {session.pid} on {provider.name}")
        if self.spec.input is not None:
            try:
                session.write(self.spec.input.encode() + b"\n")
            except TerminalError:
                pass  # it exited already; the outcome says so

        reason = None
        try:
            reason = self._supervise(session)
        finally:
            # Also ends whatever a finished agent left running.
            session.kill()
            exit_ = session.wait()
            with self._lock:
                self._session = None
            session.close()
        returncode = exit_.returncode if exit_ is not None else None

        if reason is not None:
            final = State.STALLED
        else:
            final = State.DONE if returncode == 0 else State.FAILED
        self._transition(final, reason.value if reason else f"exit {returncode}")
        outcome = Outcome(self.spec.name, final, returncode, reason, self._clock() - start)
        self._hooks.finished(ctx, outcome)
        return outcome

    def _supervise(self, session: Session) -> Optional[KillReason]:
        assert self._monitor is not None
        limits = self.limits
        tick = max(0.05, min(1.0, limits.blocked_after / 2, limits.stall_timeout / 4))
        responder = CursorQueryResponder()
        eof = False
        while True:
            if eof:
                # No more output can come; the clock keeps running until the exit.
                if session.wait(tick) is not None:
                    return None
                data: Optional[bytes] = None
            else:
                data = session.read(tick)
            now = self._clock()
            if data:
                self._output(session, responder, data, now)
            elif data == b"":
                eof = True
                self._output(session, responder, responder.flush(), now)
            elif session.poll() is not None:
                self._drain(session, responder)
                return None
            with self._lock:
                reason = self._monitor.verdict(now)
                blocked = self._monitor.blocked(now)
            if reason is not None:
                session.kill()
                return reason
            if blocked and self.state is State.WORKING:
                self._transition(State.BLOCKED, "waiting at a prompt")

    def _output(
        self, session: Session, responder: CursorQueryResponder, data: bytes, now: float
    ) -> None:
        clean, answers = responder.feed(data) if data else (b"", [])
        for answer in answers:
            try:
                session.write(answer)
            except TerminalError:
                pass  # its input is gone; the exit follows
        if not clean:
            return
        assert self._monitor is not None
        with self._lock:
            self._monitor.feed(clean, now)
        if self._on_output is not None:
            self._on_output(clean)
        if self.state is State.BLOCKED:
            self._transition(State.WORKING, "output resumed")

    def _drain(self, session: Session, responder: CursorQueryResponder) -> None:
        """Output written just before the exit; bounded, because a pseudo-console can
        stay readable after its program has gone."""
        for _ in range(_DRAIN_READS):
            data = session.read(_DRAIN_TIMEOUT)
            if not data:
                break
            self._output(session, responder, data, self._clock())
        self._output(session, responder, responder.flush(), self._clock())

    def _transition(self, state: State, detail: str) -> None:
        self.state = state
        event = Event(self.spec.name, state, time.time(), detail)
        if self._on_event is not None:
            self._on_event(event)
        self._hooks.on_event(event)


class _ProfileHook(HookBase):
    """A profile's environment and context, applied after global and project hooks."""

    def __init__(self, profile: Profile, env: Dict[str, str]) -> None:
        self.name = f"profile:{profile.name}"
        self.profile = profile.name
        self.required = True
        self._profile = profile
        self._env = env

    def prepare(self, ctx: HookContext) -> Preparation:
        p = self._profile
        env: Dict[str, Optional[str]] = {**self._env, "COAGENT_PROFILE": p.name}
        if p.model:
            env["COAGENT_MODEL"] = p.model
        if p.effort:
            env["COAGENT_EFFORT"] = p.effort
        return Preparation(env=env, context=[p.context] if p.context else [])


CONTEXT_TOKENS = ("{context}", "{context_file}")


def fill_context(argv: Sequence[str], text: Optional[str], path: Optional[str]) -> List[str]:
    """``argv`` with an element ``{context}`` replaced by the context and
    ``{context_file}`` by its path. Without context, the token is dropped together with
    the flag before it, so ``["--system", "{context}"]`` disappears entirely."""
    out: List[str] = []
    for part in argv:
        if part not in CONTEXT_TOKENS:
            out.append(part)
        elif text is None:
            if out and out[-1].startswith("-"):
                out.pop()
        else:
            out.append(text if part == "{context}" else str(path))
    return out


def _safe(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "_", name)[:40]


def _mock_agent(behavior: str) -> int:
    """Stand-in sub-agents for the demo and tests."""
    if behavior == "work":
        for step in range(5):
            print(f"[agent] step {step}: edited src/module_{step}.py", flush=True)
            time.sleep(0.1)
        print("[agent] all tests pass", flush=True)
        return 0
    if behavior == "loop":
        while True:
            print("[agent] Retrying tool call: permission denied", flush=True)
            time.sleep(0.02)
    if behavior == "hang":
        sys.stdout.write("Overwrite existing worktree? [y/N] ")
        sys.stdout.flush()
        answer = sys.stdin.readline()
        print("[agent] ok" if answer.strip().lower() == "y" else "[agent] aborted", flush=True)
        return 0 if answer.strip().lower() == "y" else 1
    if behavior == "cursor":
        # What a shell does on start-up: ask where the cursor is, and wait for the answer.
        if sys.stdin.isatty():
            import tty

            tty.setraw(sys.stdin.fileno())
        os.write(sys.stdout.fileno(), CursorQueryResponder.QUERY)
        reply = b""
        while not reply.endswith(b"R"):
            chunk = os.read(sys.stdin.fileno(), 1)
            if not chunk:
                return 1
            reply += chunk
        os.write(sys.stdout.fileno(), b"[agent] cursor at " + reply[2:-1] + b"\r\n")
        return 0
    if behavior == "orphan":
        # Starts a descendant that outlives it unless the whole tree is killed, then goes quiet.
        import subprocess

        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        print(f"[agent] child {child.pid}", flush=True)
        time.sleep(60)
        return 0
    raise SystemExit(f"unknown mock behavior: {behavior}")


def _print_event(event: Event) -> None:
    stamp = time.strftime("%H:%M:%S", time.localtime(event.at))
    print(f"{stamp} {event.agent:<8} {event.state.value:<8} {event.detail}", flush=True)


def _demo(timeout: float, terminal: Optional[str]) -> int:
    limits = Limits(
        stall_timeout=timeout,
        loop_timeout=timeout,
        loop_min_lines=10,
        blocked_after=timeout / 4,
    )
    me = [sys.executable, "-m", "coagent_m.watchdog", "mock-agent"]
    dogs = [
        Watchdog(AgentSpec(b, [*me, b], terminal=terminal), limits, on_event=_print_event)
        for b in ("work", "loop", "hang")
    ]
    threads = [threading.Thread(target=d.run, name=d.spec.name) for d in dogs]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    return 0


def _providers() -> int:
    for info in default_registry().providers():
        state = "ok" if info.reason is None else f"unavailable: {info.reason}"
        print(f"{info.priority:>4}  {info.name:<20} {state}")
    return 0


def _list_profiles() -> int:
    for p in load_profiles(os.getcwd()).profiles():
        alias = f" ({', '.join(p.aliases)})" if p.aliases else ""
        what = " ".join(filter(None, [p.model, p.effort])) or "-"
        roles = ", ".join(p.roles) or "-"
        print(f"{p.priority:>4}  {p.name}{alias}  {what}  [{p.scope.value}]  roles: {roles}")
    return 0


def _list_stations() -> int:
    for s in Stations.load():
        roster = ", ".join(s.roster) or "all"
        print(f"{s.id:<4} {s.name or '-':<20} {s.clearance:<6} {s.path or '-'}  roster: {roster}")
    return 0


def _launch(args: argparse.Namespace) -> int:
    if args.next or args.brief:
        return _mission(args)
    if not (args.profile or args.role):
        print("coagent: launch needs --profile, --role, --next or --brief", file=sys.stderr)
        return 2
    try:
        spec, limits = _plan(args.station, args.profile, args.role, args.tag, args.prompt)
    except (ProfileError, StationError) as exc:
        print(f"coagent: {exc}", file=sys.stderr)
        return 2
    return _exit_code(_supervise_cli(spec, limits, not args.no_hooks))


def _plan(
    station_key: Optional[str],
    profile_name: Optional[str],
    role: Optional[str],
    tags: Sequence[str],
    prompt: Optional[str],
    name: Optional[str] = None,
) -> Tuple[AgentSpec, Limits]:
    stations = Stations.load()
    station = stations.get(station_key) if station_key else stations.for_path(os.getcwd())
    cwd = str(station.path) if station is not None and station.path else os.getcwd()
    profiles = load_profiles(cwd, stations=stations)
    profile = profiles.get(profile_name) if profile_name else profiles.choose(role, tags, station)
    return AgentSpec.from_profile(
        profiles, profile, prompt, cwd=cwd, station=station, name=name or profile.name
    )


def _mission(args: argparse.Namespace) -> int:
    """A brief from the inbox (``--next``) or a file (``--brief``): run it in its own
    safe house, file the outcome in the inbox, and report it to the brief's source."""
    try:
        import coagent_i

        from .missions import MissionRunner
    except ImportError:
        print(
            "coagent: --next and --brief need coagent-i installed (pip install -e i/)",
            file=sys.stderr,
        )
        return 2
    try:
        runner = MissionRunner(
            coagent_i.open_inbox(args.inbox),
            use_hooks=not args.no_hooks,
            keep_safe_houses=args.keep_safe_houses,
            on_event=lambda key, e: _report_event(e),
            on_output=lambda key, data: _echo(data),
        )
        if args.brief:
            with open(args.brief, encoding="utf-8") as f:
                brief = coagent_i.Brief.from_json(f.read())
        else:
            brief = runner.inbox.claim()
            if brief is None:
                print("coagent: the inbox is empty", file=sys.stderr)
                return 0
    except (OSError, coagent_i.BriefError, StationError) as exc:
        print(f"coagent: {exc}", file=sys.stderr)
        return 2
    outcome = runner.run(
        brief,
        claimed=args.next,
        station=args.station,
        profile=args.profile,
        role=args.role,
        tags=args.tag,
    )
    if outcome["state"] == "refused":
        print(f"coagent: {outcome['reason']}", file=sys.stderr)
        return 2
    code = outcome["returncode"]
    return 1 if code is None else (code if code >= 0 else 128 - code)


def _serve(args: argparse.Namespace) -> int:
    """Takes briefs from the inbox until interrupted (or, with ``--once``, drained)."""
    try:
        import coagent_i

        from .missions import MissionRunner, Scheduler
    except ImportError:
        print("coagent: serve needs coagent-i installed (pip install -e i/)", file=sys.stderr)
        return 2
    lock = threading.Lock()

    def event(key: str, e: Event) -> None:
        with lock:
            stamp = time.strftime("%H:%M:%S", time.localtime(e.at))
            print(f"{stamp} {key:<32} {e.state.value:<8} {e.detail}", flush=True)

    try:
        runner = MissionRunner(
            coagent_i.open_inbox(args.inbox),
            use_hooks=not args.no_hooks,
            keep_safe_houses=args.keep_safe_houses,
            on_event=event,
        )
    except StationError as exc:
        print(f"coagent: {exc}", file=sys.stderr)
        return 2
    labels = dict(item.split("=", 1) for item in args.label if "=" in item)
    scheduler = Scheduler(
        runner,
        node=args.node,
        max_total=args.max,
        poll_interval=args.poll,
        lease=args.lease,
        max_attempts=args.max_attempts,
        roles=args.role or ("control", "execution"),
        labels=labels,
    )
    try:
        scheduler.run(once=args.once, idle=args.idle)
    except KeyboardInterrupt:
        print("coagent: stopping; waiting for running missions", file=sys.stderr)
        scheduler.stop()
        scheduler.wait()
    failed = [o for o in scheduler.finished if o["state"] != State.DONE.value]
    return 1 if failed else 0


def _echo(data: bytes) -> None:
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()


def _report_event(event: Event) -> None:
    print(f"\n[watchdog] {event.state.value}: {event.detail}", file=sys.stderr, flush=True)


def _list_hooks() -> int:
    for scope, hook in load_hooks(os.getcwd()).hooks:
        print(f"{scope.value:<8} {hook.name}")
    return 0


def _run(argv: Sequence[str], limits: Limits, terminal: Optional[str], use_hooks: bool) -> int:
    spec = AgentSpec(os.path.basename(argv[0]), argv, cwd=os.getcwd(), terminal=terminal)
    return _exit_code(_supervise_cli(spec, limits, use_hooks))


def _supervise_cli(spec: AgentSpec, limits: Limits, use_hooks: bool) -> Outcome:
    def echo(data: bytes) -> None:
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()

    def report(event: Event) -> None:
        print(f"\n[watchdog] {event.state.value}: {event.detail}", file=sys.stderr, flush=True)

    hooks = (
        load_hooks(spec.cwd, spec.hooks)
        if use_hooks
        else HookSet((Scope.AGENT, h) for h in spec.hooks)
    )
    return Watchdog(spec, limits, on_event=report, on_output=echo, hooks=hooks).run()


def _exit_code(outcome: Outcome) -> int:
    if outcome.returncode is None:
        return 1
    return outcome.returncode if outcome.returncode >= 0 else 128 - outcome.returncode


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m coagent_m.watchdog", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    run = sub.add_parser("run", help="supervise a command; output passes through")
    run.add_argument("--stall-timeout", type=float, default=DEFAULT_STALL_TIMEOUT)
    run.add_argument("--loop-timeout", type=float, default=DEFAULT_STALL_TIMEOUT)
    run.add_argument("--max-runtime", type=float, default=None)
    run.add_argument("--terminal", help="terminal provider name (see `providers`)")
    run.add_argument("--no-hooks", action="store_true", help="run no global or project hooks")
    run.add_argument("cmd", nargs=argparse.REMAINDER, help="-- command [args...]")

    demo = sub.add_parser("demo", help="run a healthy, a looping and a hanging mock agent")
    demo.add_argument("--timeout", type=float, default=3.0, help="stall and loop window")
    demo.add_argument("--terminal", help="terminal provider name (see `providers`)")

    sub.add_parser("providers", help="list terminal providers, best first")
    sub.add_parser("hooks", help="list the hooks a run in this directory would use")
    sub.add_parser("profiles", help="list agent profiles, best first")
    sub.add_parser("stations", help="list registered stations")

    launch = sub.add_parser("launch", help="start an agent from a profile, or for a brief")
    pick = launch.add_mutually_exclusive_group()
    pick.add_argument("--profile", help="profile name or alias")
    pick.add_argument("--role", help="the best profile for this role")
    mission = launch.add_mutually_exclusive_group()
    mission.add_argument("--next", action="store_true", help="claim the best brief in the inbox")
    mission.add_argument("--brief", help="run the brief in this JSON file")
    launch.add_argument("--inbox", help="inbox directory (with --next)")
    launch.add_argument(
        "--keep-safe-houses", action="store_true", help="keep a mission's safe house even if unused"
    )
    launch.add_argument("--tag", action="append", default=[], help="a tag the profile must have")
    launch.add_argument("--station", help="station id or name (default: the one holding .)")
    launch.add_argument("--no-hooks", action="store_true", help="run no global or project hooks")
    launch.add_argument("prompt", nargs="?", help="the task")

    serve = sub.add_parser("serve", help="take briefs from the inbox and run them")
    serve.add_argument("--max", type=int, default=4, help="missions at once (default 4)")
    serve.add_argument("--poll", type=float, default=None, help="poll intake sources every S")
    serve.add_argument("--once", action="store_true", help="exit once nothing is left to start")
    serve.add_argument("--idle", type=float, default=1.0, help="seconds between steps")
    serve.add_argument("--inbox", help="inbox directory")
    serve.add_argument("--keep-safe-houses", action="store_true")
    serve.add_argument("--node", help="this office's name (default: host-pid)")
    serve.add_argument(
        "--role",
        action="append",
        choices=("control", "execution"),
        help="what this office does: control (may be elected M), execution (runs "
        "missions); repeat for both, the default",
    )
    serve.add_argument(
        "--label", action="append", default=[], help="KEY=VALUE for station selectors"
    )
    serve.add_argument("--lease", type=float, default=60.0, help="lease and heartbeat window, S")
    serve.add_argument("--max-attempts", type=int, default=3, help="before a lost brief fails")
    serve.add_argument("--no-hooks", action="store_true", help="run no global or station hooks")

    mock = sub.add_parser("mock-agent", help=argparse.SUPPRESS)
    mock.add_argument("behavior", choices=("work", "loop", "hang", "cursor", "orphan"))

    args = parser.parse_args(argv)
    if args.command == "mock-agent":
        return _mock_agent(args.behavior)
    if args.command == "providers":
        return _providers()
    if args.command == "hooks":
        return _list_hooks()
    if args.command == "profiles":
        return _list_profiles()
    if args.command == "stations":
        return _list_stations()
    if args.command == "launch":
        return _launch(args)
    if args.command == "serve":
        return _serve(args)
    if args.command == "demo":
        return _demo(args.timeout, args.terminal)
    cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
    if not cmd:
        parser.error("run needs a command after --")
    limits = Limits(
        stall_timeout=args.stall_timeout,
        loop_timeout=args.loop_timeout,
        max_runtime=args.max_runtime,
    )
    return _run(cmd, limits, args.terminal, not args.no_hooks)


if __name__ == "__main__":
    sys.exit(main())
