"""Missions: a brief, run at its station by the agent M assigns, in its own safe_house.

``MissionRunner`` runs one brief end to end; ``Scheduler`` keeps taking briefs from the
inbox while there is capacity, overall, per station and per profile. Both need the
``coagent-i`` package, which defines briefs and the inbox.
"""

from __future__ import annotations

import fnmatch
import logging
import os
import socket
import sys
import threading
import time
from dataclasses import dataclass, replace
from pathlib import Path, PurePosixPath
from typing import Any, Callable, Dict, Iterable, List, Mapping, Optional, Sequence, Tuple

import coagent_i

from .config import Scope
from .cover import Cover, CoverError, Covers, Identity, check_commits
from .hooks import HookBase, HookContext, HookSet, Preparation, load_hooks
from .theaters import TheaterError, Theaters
from .profiles import Profile, ProfileError, load_adapter, load_profiles
from .stations import Station, StationError, Stations
from .terminal import Registry, TerminalError
from .watchdog import AgentSpec, Event, Limits, Outcome, State, Watchdog
from .safehouse import (
    LOCAL,
    Git,
    SshGit,
    SafeHouse,
    SafeHouseError,
    branch_name,
    commit_log,
    head,
    is_git_checkout,
)

__all__ = ["ROLES", "Assignment", "MissionRunner", "Scheduler", "TRANSCRIPT_LIMIT"]

# What an office may do: be elected M, run missions.
ROLES = ("control", "execution")

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
    # The 00 Section adapter the profile's launcher names, if any.
    adapter: Optional[str] = None


class MissionRunner:
    """Assigns and runs briefs.

    ``stations`` defaults to ``Stations.load()``; ``use_hooks`` loads headquarters and
    station hooks for each mission; ``safe_house_root`` overrides where safe houses go;
    ``keep_safe_houses`` keeps them even when the agent left nothing. ``on_event(key,
    event)`` and ``on_output(key, bytes)`` are called from the thread running the mission.
    """

    def __init__(
        self,
        inbox: Optional[coagent_i.Inbox] = None,
        *,
        stations: Optional[Stations] = None,
        theaters: Optional[Theaters] = None,
        covers: Optional[Covers] = None,
        sources: Optional[Sequence[Any]] = None,
        use_hooks: bool = True,
        registry: Optional[Registry] = None,
        safe_house_root: Optional[Path] = None,
        keep_safe_houses: bool = False,
        on_event: Optional[Callable[[str, Event], None]] = None,
        on_output: Optional[Callable[[str, bytes], None]] = None,
    ) -> None:
        self.inbox = inbox if inbox is not None else coagent_i.open_inbox()
        self.stations = stations if stations is not None else Stations.load()
        self.theaters = theaters if theaters is not None else Theaters.load()
        self.covers = covers if covers is not None else Covers.load()
        self.sources = sources
        self.use_hooks = use_hooks
        self.registry = registry
        self.safe_house_root = safe_house_root
        self.keep_safe_houses = keep_safe_houses
        self.on_event = on_event
        self.on_output = on_output
        self._dogs: Dict[str, Watchdog] = {}
        self._abandoned: set = set()
        self._dogs_lock = threading.Lock()

    def abandon(self, key: str) -> None:
        """Stops the mission keyed ``key`` without filing or reporting its outcome: the
        brief belongs to another office now."""
        with self._dogs_lock:
            self._abandoned.add(key)
            dog = self._dogs.get(key)
        if dog is not None:
            dog.kill()

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
        Raises ``StationError`` or ``ProfileError``; each override wins over the brief.
        """
        key = station or brief.station
        where = self.stations.get(key) if key else self.stations.for_path(os.getcwd())
        cwd = str(where.path) if where is not None and where.path else os.getcwd()
        profiles = load_profiles(cwd, stations=self.stations, station=where)
        name = profile or brief.profile
        chosen = (
            profiles.get(name)
            if name
            else profiles.choose(role or brief.role, [*brief.tags, *tags], where)
        )
        spec, limits = AgentSpec.from_profile(
            profiles, chosen, brief.prompt, name=brief.id, cwd=cwd, station=where
        )
        adapter = profiles.launcher_of(chosen).adapter
        return Assignment(brief, where, chosen, spec, limits, adapter)

    def run(
        self,
        brief: coagent_i.Brief,
        assignment: Optional[Assignment] = None,
        *,
        claimed: bool = False,
        node: Optional[str] = None,
        **assign: Any,
    ) -> Dict[str, Any]:
        """Runs ``brief`` and answers its outcome. With ``claimed``, files the outcome
        (and transcript) in the inbox, as ``node`` when the claim holds a lease; always
        reports it to the brief's source, unless the mission was abandoned or its lease
        passed to another office, which then owns the outcome."""
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
        try:
            assignment = assignment or self.assign(brief, **assign)
            outcome.update(
                profile=assignment.profile.name,
                station=assignment.station.id if assignment.station else None,
            )
            result, extras = self._run(assignment, transcript)
            outcome.update(
                agent=result.agent,
                state=result.state.value,
                returncode=result.returncode,
                reason=result.reason.value if result.reason else None,
                runtime=round(result.runtime, 3),
                **extras,
            )
            cover = extras.get("cover")
            station = assignment.station
            if cover and cover["blown"] and station is not None and station.on_blown == "fail":
                outcome.update(state="blown", reason="cover blown")
        except (
            ProfileError,
            StationError,
            TheaterError,
            CoverError,
            TerminalError,
            SafeHouseError,
        ) as exc:
            outcome["reason"] = str(exc)
            _log.warning("mission %s refused: %s", brief.key, exc)
        with self._dogs_lock:
            abandoned = brief.key in self._abandoned
            self._abandoned.discard(brief.key)
        if abandoned:
            outcome.update(state="abandoned", reason="handed to another office")
            return outcome
        if claimed:
            ok = outcome["state"] == State.DONE.value
            try:
                self.inbox.finish(brief, outcome, ok=ok, transcript=bytes(transcript), node=node)
            except coagent_i.LeaseLost:
                _log.warning("mission %s: its lease passed to another office", brief.key)
                outcome.update(state="abandoned", reason="lease lost")
                return outcome
        coagent_i.report(brief, outcome, self.sources)
        return outcome

    def _run(self, a: Assignment, transcript: bytearray) -> Tuple[Outcome, Dict[str, Any]]:
        spec, station, key = a.spec, a.station, a.brief.key
        git: Optional[Git] = LOCAL
        cover: Optional[Cover] = None
        safe_house: Optional[SafeHouse] = None
        if station is not None:
            cover = self.covers.get(station.cover)
            hook = _CoverHook(cover, station.identity, a.profile.name, a.profile.model or "")
            spec = replace(spec, hooks=(*spec.hooks, hook))
            if station.theater is not None:
                theater = self.theaters.get(station.theater)
                git = SshGit(theater) if theater.posix else None
        if (
            station is not None
            and station.safe_houses
            and station.path is not None
            and git is not None
            and is_git_checkout(station.path, git)
        ):
            root = self.safe_house_root
            remote = station.theater is not None
            if remote:
                # Without the theater's safe_houses a remote station's missions run in its checkout.
                safe_houses = self.theaters.get(station.theater).safe_houses  # type: ignore[arg-type]
                root = PurePosixPath(safe_houses) if safe_houses else None
            if not remote or root is not None:
                branch = branch_name(
                    station.branch, id=a.brief.id, title=a.brief.title, station=station.id
                )
                safe_house = SafeHouse.create(
                    station.path, key, branch, root=root, base=station.base, git=git
                )
                spec = replace(spec, cwd=str(safe_house.path))
        # Where the agent's commits land, to hold them to the cover afterwards.
        checked: Optional[Tuple[str, str]] = None
        if safe_house is not None:
            checked = (str(safe_house.path), safe_house.base)
        elif station is not None and station.path is not None and git is not None:
            before = head(station.path, git)
            checked = (str(station.path), before) if before else None
        # Configuration is the station's, wherever its safe_house is.
        hooks = (
            load_hooks(spec.cwd, spec.hooks, stations=self.stations, station=station)
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

        extras: Dict[str, Any] = {}
        try:
            dog = Watchdog(
                spec,
                a.limits,
                on_event=event,
                on_output=output,
                registry=self.registry,
                hooks=hooks,
            )
            with self._dogs_lock:
                self._dogs[key] = dog
                abandoned = key in self._abandoned
            if abandoned:
                raise TerminalError("abandoned before it started")
            try:
                result = dog.run()
            finally:
                with self._dogs_lock:
                    self._dogs.pop(key, None)
            if cover is not None and checked is not None and git is not None:
                commits = commit_log(git, *checked)
                broken = check_commits(
                    commits,
                    cover,
                    station.identity if station is not None else None,
                    model=a.profile.model or "",
                )
                extras["cover"] = {
                    "policy": cover.name,
                    "commits": len(commits),
                    "blown": bool(broken),
                    "violations": [v.as_dict() for v in broken],
                }
        finally:
            if safe_house is not None:
                safe_house.cleanup(keep=self.keep_safe_houses)
        if safe_house is not None:
            extras["safe_house"] = safe_house.describe()
        if a.adapter:
            debrief = getattr(load_adapter(a.adapter), "debrief", None)
            if debrief is not None:
                try:
                    found = debrief(bytes(transcript))
                except Exception:  # a debrief is a report, never a reason to fail
                    _log.exception("adapter %s could not debrief %s", a.adapter, key)
                    found = None
                if found:
                    extras["debrief"] = dict(found)
        return result, extras


class _CoverHook(HookBase):
    """The station's cover: its identity for git, and the agent told the policy."""

    def __init__(
        self, cover: Cover, identity: Optional[Identity], profile: str, model: str
    ) -> None:
        self.name = f"cover:{cover.name}"
        self.required = True
        self._cover = cover
        self._identity = identity
        self._profile = profile
        self._model = model

    def prepare(self, ctx: HookContext) -> Preparation:
        env: Dict[str, Optional[str]] = {"COAGENT_COVER": self._cover.name}
        if self._identity is not None:
            env.update(self._identity.git_env())
        briefing = self._cover.briefing(self._identity, self._profile, self._model)
        return Preparation(env=env, context=[briefing])


class Scheduler:
    """An office of M: runs the missions assigned to it and, while it holds the leader
    lease, is M — the one office that decides. ``roles`` is ``control`` (may be elected
    M), ``execution`` (runs missions) or both; ``labels`` describe the office to station
    selectors, beside the automatic ``node``, ``host`` and ``os``.

    Every office heartbeats into the shared inbox, renews the leases of what it runs
    (stopping any mission whose lease another office took) and runs what is leased to
    it. The leader also polls intake (``poll_interval``), recovers briefs whose office
    was lost (back to new until ``max_attempts``), and places each waiting brief on the
    least-loaded live office that can reach its station's checkout, within the station's
    and the profile's ``max_concurrent`` counted across all offices. One office alone is
    simply M with no standby. ``max_total`` is this office's own capacity.
    """

    def __init__(
        self,
        runner: MissionRunner,
        *,
        node: Optional[str] = None,
        max_total: int = 4,
        poll_interval: Optional[float] = None,
        lease: float = coagent_i.DEFAULT_LEASE,
        max_attempts: int = 3,
        roles: Sequence[str] = ("control", "execution"),
        labels: Optional[Mapping[str, str]] = None,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        unknown = sorted(set(roles) - set(ROLES))
        if unknown or not roles:
            raise ValueError(f"roles are {ROLES}, not {list(roles)}")
        self.runner = runner
        self.node = node or f"{socket.gethostname()}-{os.getpid()}"
        self.roles = tuple(r for r in ROLES if r in roles)
        self.labels = {
            "node": self.node,
            "host": socket.gethostname(),
            "os": _os_label(),
            **dict(labels or {}),
        }
        self.max_total = max_total
        self.poll_interval = poll_interval
        self.lease = lease
        self.max_attempts = max_attempts
        self.leading = False
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
        _, errors = coagent_i.poll(sources, self.runner.inbox)
        for e in errors:
            _log.warning("intake source failed: %s", e)
        self._last_poll = self._clock()

    def step(self) -> int:
        """One round: renew, heartbeat, lead if this office holds the lease, then start
        what is assigned here. Answers how many briefs it placed or started."""
        inbox = self.runner.inbox
        self._renew()
        inbox.heartbeat(self.node, self._info())
        was_leading = self.leading
        self.leading = "control" in self.roles and inbox.elect(self.node, ttl=self.lease)
        if self.leading and not was_leading:
            _log.info("office %s is M now", self.node)
        placed = self._lead() if self.leading else 0
        return placed + (self._execute() if "execution" in self.roles else 0)

    def run(self, *, once: bool = False, idle: float = 1.0) -> None:
        """Steps until ``stop()``; with ``once``, until nothing runs anywhere and nothing
        waiting could be placed."""
        while not self._stop.is_set():
            moved = self.step()
            if once and not moved and not self.running and not self.runner.inbox.leases():
                break
            self._stop.wait(idle)
        self.wait()

    def stop(self) -> None:
        """Starts nothing more; running missions finish."""
        self._stop.set()

    def wait(self) -> None:
        for thread in list(self._threads):
            thread.join()

    def _info(self) -> Dict[str, Any]:
        local = [
            s.id
            for s in self.runner.stations
            if s.theater is None and s.path is not None and Path(s.path).is_dir()
        ]
        return {
            "roles": list(self.roles),
            "labels": self.labels,
            "capacity": self.max_total if "execution" in self.roles else 0,
            "running": len(self.running),
            "stations": local,
        }

    def _renew(self) -> None:
        for key in self.running:
            if not self.runner.inbox.renew(key, self.node, lease=self.lease):
                _log.warning("office %s lost the lease on %s; stopping it", self.node, key)
                self.runner.abandon(key)

    def _lead(self) -> int:
        inbox = self.runner.inbox
        if self.poll_interval is not None and (
            self._last_poll is None or self._clock() - self._last_poll >= self.poll_interval
        ):
            self.poll()
        for lease in inbox.expired():
            where = inbox.recover(lease, max_attempts=self.max_attempts, by=self.node)
            if where:
                _log.warning("brief %s: office %s was lost; %s", lease.key, lease.node, where)
        offices = {o["node"]: o for o in inbox.offices(within=self.lease)}
        leases = inbox.leases()
        placed = 0
        for brief in inbox.list("new"):
            if self._stop.is_set():
                break
            try:
                assignment = self.runner.assign(brief)
            except (ProfileError, StationError) as exc:
                if inbox.claim(brief.key, node=self.node, lease=self.lease) is not None:
                    _log.warning("brief %s cannot be assigned: %s", brief.key, exc)
                    self.finished.append(self.runner.run(brief, claimed=True, node=self.node))
                continue
            if not self._fits(assignment, leases):
                continue
            office = self._place(assignment, offices, leases)
            if office is None:
                continue
            meta = {"station": assignment.station.id if assignment.station else None}
            meta["profile"] = assignment.profile.name
            if inbox.claim(brief.key, node=office, lease=self.lease, meta=meta) is None:
                continue
            leases.append(coagent_i.Lease(brief.key, office, 0.0, 0, meta))
            placed += 1
        return placed

    def _fits(self, a: Assignment, leases: Sequence[Any]) -> bool:
        station, profile = a.station, a.profile
        if station is not None and station.max_concurrent is not None:
            here = sum(1 for l in leases if l.meta.get("station") == station.id)
            if here >= station.max_concurrent:
                return False
        if profile.max_concurrent is not None:
            same = sum(1 for l in leases if l.meta.get("profile") == profile.name)
            if same >= profile.max_concurrent:
                return False
        return True

    def _place(
        self, a: Assignment, offices: Dict[str, Any], leases: Sequence[Any]
    ) -> Optional[str]:
        """The live execution office with room whose labels match the station's selector
        and that can reach its checkout, least loaded first, this office winning a tie.
        A station on a remote theater is reachable from any office."""
        load: Dict[str, int] = {}
        for lease in leases:
            load[lease.node] = load.get(lease.node, 0) + 1
        station = a.station
        needs_local = station is not None and station.theater is None and station.path is not None
        selector = station.selector if station is not None else {}
        candidates = []
        for name, office in offices.items():
            if "execution" not in office.get("roles", ROLES):
                continue
            if load.get(name, 0) >= int(office.get("capacity", 1)):
                continue
            labels = office.get("labels", {})
            if any(not fnmatch.fnmatchcase(str(labels.get(k, "")), v) for k, v in selector.items()):
                continue
            if needs_local and station.id not in office.get("stations", ()):
                continue
            candidates.append((load.get(name, 0), name != self.node, name))
        return min(candidates)[2] if candidates else None

    def _execute(self) -> int:
        inbox, started, now = self.runner.inbox, 0, time.time()
        for lease in inbox.leases():
            if lease.node != self.node or lease.expires < now or lease.key in self.running:
                continue
            if len(self.running) >= self.max_total:
                break
            brief = inbox.read(lease.key)
            if brief is None:
                continue
            try:
                assignment = self.runner.assign(brief)
            except (ProfileError, StationError):
                self.finished.append(self.runner.run(brief, claimed=True, node=self.node))
                continue
            self._start(assignment)
            started += 1
        return started

    def _start(self, a: Assignment) -> None:
        key = a.brief.key
        with self._lock:
            self._running[key] = a

        def mission() -> None:
            try:
                self.finished.append(self.runner.run(a.brief, a, claimed=True, node=self.node))
            finally:
                with self._lock:
                    self._running.pop(key, None)

        thread = threading.Thread(target=mission, name=f"mission-{key}", daemon=True)
        self._threads.append(thread)
        thread.start()


def _os_label() -> str:
    if sys.platform.startswith("linux"):
        return "linux"
    if sys.platform == "darwin":
        return "macos"
    if sys.platform == "win32":
        return "windows"
    return sys.platform
