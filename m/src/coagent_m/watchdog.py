"""Supervisor watchdog: run one sub-agent under a pty and SIGKILL it when it stalls or loops.

The agent runs in its own session and process group on a pseudo-terminal, so it behaves
as it would in an interactive shell and every child it spawns dies with it. Three
conditions end a run early:

* **input hang / silence** -- no output and no operator input for ``stall_timeout``;
* **loop** -- output keeps arriving but no line has been new for ``loop_timeout``;
* **max runtime** -- an optional hard cap on wall-clock time.

POSIX only (``pty``, process groups, ``SIGKILL``).
"""

from __future__ import annotations

import argparse
import enum
import hashlib
import os
import pty
import re
import select
import signal
import subprocess
import sys
import threading
import time
from collections import OrderedDict
from dataclasses import dataclass
from typing import Callable, Dict, Optional, Sequence

__all__ = [
    "AgentSpec",
    "ActivityMonitor",
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
_READ_SIZE = 65536


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
    """What to run: ``argv`` in ``cwd`` (a git worktree) with ``env`` (from ``q``)."""

    name: str
    argv: Sequence[str]
    cwd: Optional[str] = None
    env: Optional[Dict[str, str]] = None


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
    ``on_output`` receives raw pty bytes for the terminal stream; ``on_event`` receives
    state transitions. Both are called on the ``run()`` thread and must not block.
    """

    def __init__(
        self,
        spec: AgentSpec,
        limits: Limits = Limits(),
        on_event: Optional[Callable[[Event], None]] = None,
        on_output: Optional[Callable[[bytes], None]] = None,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self.spec = spec
        self.limits = limits
        self.state = State.PENDING
        self._on_event = on_event
        self._on_output = on_output
        self._clock = clock
        self._lock = threading.Lock()
        self._monitor: Optional[ActivityMonitor] = None
        self._master: Optional[int] = None
        self._proc: Optional[subprocess.Popen] = None
        self._reaped = False

    def send(self, data: bytes) -> None:
        """Write operator input to the agent's terminal. Raises ``RuntimeError`` if not running."""
        with self._lock:
            if self._master is None or self._monitor is None:
                raise RuntimeError(f"agent {self.spec.name!r} is not running")
            os.write(self._master, data)
            self._monitor.operator_input(self._clock())

    def kill(self) -> None:
        """SIGKILL the agent's whole process group. A no-op once it has exited."""
        proc = self._proc
        # Once reaped, the pid may belong to an unrelated process.
        if proc is None or self._reaped:
            return
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def run(self) -> Outcome:
        """Start the agent and supervise it until it exits or is killed."""
        master, slave = pty.openpty()
        start = self._clock()
        try:
            self._proc = subprocess.Popen(
                list(self.spec.argv),
                stdin=slave,
                stdout=slave,
                stderr=slave,
                cwd=self.spec.cwd,
                env=self.spec.env,
                start_new_session=True,  # pgid == pid, so killpg reaches every descendant
                close_fds=True,
            )
        except BaseException:
            os.close(master)
            raise
        finally:
            os.close(slave)
        with self._lock:
            self._master = master
            self._monitor = ActivityMonitor(self.limits, start)
        self._transition(State.WORKING, f"pid {self._proc.pid}")

        reason = None
        try:
            reason = self._supervise(master)
        finally:
            with self._lock:
                self._master = None
            os.close(master)
            # Also reaps children a finished agent left running in its group.
            self.kill()
            returncode = self._proc.wait()
            self._reaped = True

        if reason is not None:
            final = State.STALLED
        else:
            final = State.DONE if returncode == 0 else State.FAILED
        self._transition(final, reason.value if reason else f"exit {returncode}")
        return Outcome(self.spec.name, final, returncode, reason, self._clock() - start)

    def _supervise(self, master: int) -> Optional[KillReason]:
        assert self._proc is not None and self._monitor is not None
        limits = self.limits
        tick = max(0.05, min(1.0, limits.blocked_after / 2, limits.stall_timeout / 4))
        while True:
            readable, _, _ = select.select([master], [], [], tick)
            now = self._clock()
            if readable:
                try:
                    data = os.read(master, _READ_SIZE)
                except OSError:  # EIO on Linux once every slave fd is closed
                    data = b""
                if not data:
                    return None
                with self._lock:
                    self._monitor.feed(data, now)
                if self._on_output is not None:
                    self._on_output(data)
                if self.state is State.BLOCKED:
                    self._transition(State.WORKING, "output resumed")
            elif self._proc.poll() is not None:
                # Exited, but a detached descendant may still hold the pty open.
                return None
            with self._lock:
                reason = self._monitor.verdict(now)
                blocked = self._monitor.blocked(now)
            if reason is not None:
                self.kill()
                return reason
            if blocked and self.state is State.WORKING:
                self._transition(State.BLOCKED, "waiting at a prompt")

    def _transition(self, state: State, detail: str) -> None:
        self.state = state
        if self._on_event is not None:
            self._on_event(Event(self.spec.name, state, time.time(), detail))


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
    raise SystemExit(f"unknown mock behavior: {behavior}")


def _print_event(event: Event) -> None:
    stamp = time.strftime("%H:%M:%S", time.localtime(event.at))
    print(f"{stamp} {event.agent:<8} {event.state.value:<8} {event.detail}", flush=True)


def _demo(timeout: float) -> int:
    limits = Limits(
        stall_timeout=timeout,
        loop_timeout=timeout,
        loop_min_lines=10,
        blocked_after=timeout / 4,
    )
    me = [sys.executable, "-m", "coagent_m.watchdog", "mock-agent"]
    dogs = [
        Watchdog(AgentSpec(b, [*me, b]), limits, on_event=_print_event)
        for b in ("work", "loop", "hang")
    ]
    threads = [threading.Thread(target=d.run, name=d.spec.name) for d in dogs]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    return 0


def _run(argv: Sequence[str], limits: Limits) -> int:
    def echo(data: bytes) -> None:
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()

    def report(event: Event) -> None:
        print(f"\n[watchdog] {event.state.value}: {event.detail}", file=sys.stderr, flush=True)

    spec = AgentSpec(os.path.basename(argv[0]), argv)
    outcome = Watchdog(spec, limits, on_event=report, on_output=echo).run()
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
    run.add_argument("cmd", nargs=argparse.REMAINDER, help="-- command [args...]")

    demo = sub.add_parser("demo", help="run a healthy, a looping and a hanging mock agent")
    demo.add_argument("--timeout", type=float, default=3.0, help="stall and loop window")

    mock = sub.add_parser("mock-agent", help=argparse.SUPPRESS)
    mock.add_argument("behavior", choices=("work", "loop", "hang"))

    args = parser.parse_args(argv)
    if args.command == "mock-agent":
        return _mock_agent(args.behavior)
    if args.command == "demo":
        return _demo(args.timeout)
    cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
    if not cmd:
        parser.error("run needs a command after --")
    limits = Limits(
        stall_timeout=args.stall_timeout,
        loop_timeout=args.loop_timeout,
        max_runtime=args.max_runtime,
    )
    return _run(cmd, limits)


if __name__ == "__main__":
    sys.exit(main())
