"""Missions: a brief, run at its station by the agent M assigns, in its own worktree.

``MissionRunner`` runs one brief end to end; ``Scheduler`` keeps taking briefs from the
inbox while there is capacity, overall, per station and per profile. Both need the
``coagent-i`` package, which defines briefs and the inbox.
"""

from __future__ import annotations

import logging
import os
import threading
import time
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, List, Optional, Sequence

import coagent_i

from .config import Scope
from .hooks import HookSet, load_hooks
from .profiles import Profile, ProfileError, load_profiles
from .stations import Station, StationError, Stations
from .terminal import Registry, TerminalError
from .watchdog import AgentSpec, Event, Limits, Outcome, State, Watchdog
from .worktree import Worktree, WorktreeError, branch_name, is_git_checkout

__all__ = ["Assignment", "MissionRunner", "Scheduler", "TRANSCRIPT_LIMIT"]

# The tail of an agent's output kept as its transcript.
TRANSCRIPT_LIMIT = 4 * 1024 * 1024

_log = logging.getLogger(__name__)


@dataclass(frozen=True)
class Assignment:
    """Who runs a brief, where, and how."""

    brief: coagent_i.Brief
    station: Optional[Station]
    profile: Profile
    spec: AgentSpec
    limits: Limits


class MissionRunner:
    """Assigns and runs briefs.

    ``stations`` defaults to ``Stations.load()``; ``use_hooks`` loads headquarters and
    station hooks for each mission; ``worktree_root`` overrides where worktrees go;
    ``keep_worktrees`` keeps them even when the agent left nothing. ``on_event(key,
    event)`` and ``on_output(key, bytes)`` are called from the thread running the mission.
    """

    def __init__(
        self,
        inbox: Optional[coagent_i.Inbox] = None,
        *,
        stations: Optional[Stations] = None,
        sources: Optional[Sequence[Any]] = None,
        use_hooks: bool = True,
        registry: Optional[Registry] = None,
        worktree_root: Optional[Path] = None,
        keep_worktrees: bool = False,
        on_event: Optional[Callable[[str, Event], None]] = None,
        on_output: Optional[Callable[[str, bytes], None]] = None,
    ) -> None:
        self.inbox = inbox if inbox is not None else coagent_i.Inbox()
        self.stations = stations if stations is not None else Stations.load()
        self.sources = sources
        self.use_hooks = use_hooks
        self.registry = registry
        self.worktree_root = worktree_root
        self.keep_worktrees = keep_worktrees
        self.on_event = on_event
        self.on_output = on_output

    def assign(
        self,
        brief: coagent_i.Brief,
        *,
        station: Optional[str] = None,
        profile: Optional[str] = None,
        role: Optional[str] = None,
        tags: Iterable[str] = (),
    ) -> Assignment:
        """The station the brief names (else the one holding the current directory), the
        profile it names (else the best for its role and tags there), and the agent.
        Raises ``StationError`` or ``ProfileError``; each override wins over the brief."""
        key = station or brief.station
        where = self.stations.get(key) if key else self.stations.for_path(os.getcwd())
        cwd = str(where.path) if where is not None and where.path else os.getcwd()
        profiles = load_profiles(cwd, stations=self.stations)
        name = profile or brief.profile
        chosen = (
            profiles.get(name)
            if name
            else profiles.choose(role or brief.role, [*brief.tags, *tags], where)
        )
        spec, limits = AgentSpec.from_profile(
            profiles, chosen, brief.prompt, name=brief.id, cwd=cwd, station=where
        )
        return Assignment(brief, where, chosen, spec, limits)

    def run(
        self,
        brief: coagent_i.Brief,
        assignment: Optional[Assignment] = None,
        *,
        claimed: bool = False,
        **assign: Any,
    ) -> Dict[str, Any]:
        """Runs ``brief`` and answers its outcome. With ``claimed``, files the outcome
        (and transcript) in the inbox; always reports it to the brief's source."""
        outcome: Dict[str, Any] = {
            "agent": None,
            "profile": None,
            "station": brief.station,
            "state": "refused",
            "returncode": None,
            "reason": None,
            "runtime": 0.0,
        }
        transcript = bytearray()
        result: Optional[Outcome] = None
        try:
            assignment = assignment or self.assign(brief, **assign)
            outcome.update(
                profile=assignment.profile.name,
                station=assignment.station.id if assignment.station else None,
            )
            result, worktree = self._run(assignment, transcript)
            outcome.update(
                agent=result.agent,
                state=result.state.value,
                returncode=result.returncode,
                reason=result.reason.value if result.reason else None,
                runtime=round(result.runtime, 3),
            )
            if worktree is not None:
                outcome["worktree"] = worktree
        except (ProfileError, StationError, TerminalError, WorktreeError) as exc:
            outcome["reason"] = str(exc)
            _log.warning("mission %s refused: %s", brief.key, exc)
        if claimed:
            ok = result is not None and result.state is State.DONE
            self.inbox.finish(brief, outcome, ok=ok, transcript=bytes(transcript))
        coagent_i.report(brief, outcome, self.sources)
        return outcome

    def _run(self, a: Assignment, transcript: bytearray):
        spec, station, key = a.spec, a.station, a.brief.key
        worktree: Optional[Worktree] = None
        if (
            station is not None
            and station.worktrees
            and station.path is not None
            and is_git_checkout(station.path)
        ):
            branch = branch_name(
                station.branch, id=a.brief.id, title=a.brief.title, station=station.id
            )
            worktree = Worktree.create(
                station.path, key, branch, root=self.worktree_root, base=station.base
            )
            spec = replace(spec, cwd=str(worktree.path))
        # Configuration is the station's, wherever its worktree is.
        config_cwd = str(station.path) if station is not None and station.path else spec.cwd
        hooks = (
            load_hooks(config_cwd, spec.hooks, stations=self.stations)
            if self.use_hooks
            else HookSet((Scope.AGENT, h) for h in spec.hooks)
        )

        def output(data: bytes) -> None:
            transcript.extend(data)
            if len(transcript) > TRANSCRIPT_LIMIT:
                del transcript[: len(transcript) - TRANSCRIPT_LIMIT]
            if self.on_output is not None:
                self.on_output(key, data)

        def event(e: Event) -> None:
            if self.on_event is not None:
                self.on_event(key, e)

        try:
            result = Watchdog(
                spec,
                a.limits,
                on_event=event,
                on_output=output,
                registry=self.registry,
                hooks=hooks,
            ).run()
        finally:
            info = None
            if worktree is not None:
                worktree.cleanup(keep=self.keep_worktrees)
                info = worktree.describe()
        return result, info


class Scheduler:
    """Takes briefs from the inbox while there is capacity and runs each on a thread.

    Capacity is ``max_total`` missions at once, a station's ``max_concurrent`` and a
    profile's ``max_concurrent``. A brief over capacity stays new until a slot frees; one
    that cannot be assigned at all is claimed and filed as refused. ``poll_interval``
    polls the intake sources that often (``None``: never).
    """

    def __init__(
        self,
        runner: MissionRunner,
        *,
        max_total: int = 4,
        poll_interval: Optional[float] = None,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self.runner = runner
        self.max_total = max_total
        self.poll_interval = poll_interval
        self._clock = clock
        self._lock = threading.Lock()
        self._running: Dict[str, Assignment] = {}
        self._threads: List[threading.Thread] = []
        self._stop = threading.Event()
        self._last_poll: Optional[float] = None
        self.finished: List[Dict[str, Any]] = []

    @property
    def running(self) -> List[str]:
        with self._lock:
            return sorted(self._running)

    def poll(self) -> None:
        """Polls the intake sources now, logging any that failed."""
        sources = (
            self.runner.sources if self.runner.sources is not None else coagent_i.load_sources()
        )
        filed, errors = coagent_i.poll(sources, self.runner.inbox)
        for e in errors:
            _log.warning("intake source failed: %s", e)
        self._last_poll = self._clock()

    def step(self) -> int:
        """Starts every waiting brief that fits; answers how many started."""
        if self.poll_interval is not None and (
            self._last_poll is None or self._clock() - self._last_poll >= self.poll_interval
        ):
            self.poll()
        started = 0
        for brief in self.runner.inbox.list("new"):
            if self._stop.is_set() or len(self.running) >= self.max_total:
                break
            try:
                assignment = self.runner.assign(brief)
            except (ProfileError, StationError) as exc:
                if self.runner.inbox.claim(brief.key) is not None:
                    _log.warning("brief %s cannot be assigned: %s", brief.key, exc)
                    self.finished.append(self.runner.run(brief, claimed=True))
                continue
            if not self._fits(assignment):
                continue
            if self.runner.inbox.claim(brief.key) is None:
                continue  # another supervisor took it
            self._start(assignment)
            started += 1
        return started

    def run(self, *, once: bool = False, idle: float = 1.0) -> None:
        """Steps until ``stop()``; with ``once``, until nothing waits that could start and
        nothing runs."""
        while not self._stop.is_set():
            started = self.step()
            if once and not started and not self.running:
                break
            self._stop.wait(idle)
        self.wait()

    def stop(self) -> None:
        """Starts nothing more; running missions finish."""
        self._stop.set()

    def wait(self) -> None:
        for thread in list(self._threads):
            thread.join()

    def _fits(self, a: Assignment) -> bool:
        with self._lock:
            running = list(self._running.values())
        if len(running) >= self.max_total:
            return False
        station, profile = a.station, a.profile
        if station is not None and station.max_concurrent is not None:
            here = sum(1 for r in running if r.station is not None and r.station.id == station.id)
            if here >= station.max_concurrent:
                return False
        if profile.max_concurrent is not None:
            same = sum(1 for r in running if r.profile.name == profile.name)
            if same >= profile.max_concurrent:
                return False
        return True

    def _start(self, a: Assignment) -> None:
        key = a.brief.key
        with self._lock:
            self._running[key] = a

        def mission() -> None:
            try:
                self.finished.append(self.runner.run(a.brief, a, claimed=True))
            finally:
                with self._lock:
                    self._running.pop(key, None)

        thread = threading.Thread(target=mission, name=f"mission-{key}", daemon=True)
        self._threads.append(thread)
        thread.start()
