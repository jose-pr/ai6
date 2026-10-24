"""Stations: the projects M runs missions for.

A station's ``id`` is its cover name, and it has exactly one: any short code (``G``,
``GT``, ``B2``; a letter is the convention, not a limit). ``name`` is the real project
name.

A station is a registered project: where its checkout lives, which repositories it spans,
which agents are posted there and the highest clearance a mission there may use. A
station's ``.coagent/`` (its hooks and profiles) is read because it is registered; the
``.coagent/`` of a directory no station contains is never read.

``stations.toml``, in the global config directory::

    [station.G]
    name = "guatiao"
    path = "~/devel/guatiao"
    repos = ["https://github.com/Boriken-Dev/guatiao"]
    roster = ["007", "q-branch"]    # absent: every profile
    clearance = "field"
    max_concurrent = 3
    branch = "{id}-{slug}"          # each mission's worktree branch
    intake = { jira = "GUA" }       # read by intake sources, not by M
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field, fields
from pathlib import Path, PurePosixPath
from typing import Any, Dict, Iterator, List, Mapping, Optional, Tuple

from .config import PROJECT_DIR, TRUST_ENV, config_dir, find_project, read_toml
from .cover import Identity

__all__ = [
    "CLEARANCES",
    "STATIONS_FILE",
    "Station",
    "StationError",
    "Stations",
    "clearance_rank",
    "local_config",
]

STATIONS_FILE = "stations.toml"
# Lowest first. "00" may act unattended and irreversibly; "desk" may not leave the desk.
CLEARANCES = ("desk", "field", "00")


class StationError(Exception):
    """A station is unknown, malformed, or refuses what was asked of it."""


def clearance_rank(clearance: str) -> int:
    """Its position in ``CLEARANCES``. Raises ``StationError`` for an unknown one."""
    try:
        return CLEARANCES.index(clearance)
    except ValueError:
        raise StationError(f"unknown clearance {clearance!r} (one of {CLEARANCES})") from None


@dataclass(frozen=True)
class Station:
    """One registered project."""

    id: str
    name: str = ""
    description: str = ""
    # The checkout; on `theater` when one is named, else on this machine.
    path: Optional[Path] = None
    theater: Optional[str] = None
    # A local directory with the station's hooks and profiles, for a station whose
    # checkout is on another theater; a local station's is <path>/.coagent.
    config: Optional[Path] = None
    repos: Tuple[str, ...] = ()
    # Profiles posted here; empty means every profile.
    roster: Tuple[str, ...] = ()
    clearance: str = "field"
    # How many agents may run here at once; None is no limit.
    max_concurrent: Optional[int] = None
    # Labels an office must carry to run this station's missions, as in a Kubernetes
    # nodeSelector; values may be globs ("lab-*"). Offices are labelled node, host, os.
    selector: Mapping[str, str] = field(default_factory=dict)
    # A git worktree per mission, cut from `base` onto a branch named by `branch`
    # ({id}, {slug}, {station}); off, missions run in the checkout itself.
    worktrees: bool = True
    branch: str = "{id}-{slug}"
    base: str = "HEAD"
    # Universal Exports: the cover policy (see coagent_m.cover), the identity commits
    # carry, and whether a commit that breaks cover fails the mission ("fail") or is
    # only reported ("flag").
    cover: str = "covert"
    identity: Optional[Identity] = None
    on_blown: str = "flag"
    # Free-form settings for intake sources; M does not read them.
    intake: Mapping[str, Any] = field(default_factory=dict)

    @property
    def label(self) -> str:
        return f"Station {self.id}" + (f" ({self.name})" if self.name else "")

    def contains(self, path: "str | Path") -> bool:
        """Whether ``path`` on this machine is inside the checkout; never, for a station
        on another theater."""
        if self.path is None or self.theater is not None:
            return False
        try:
            Path(path).resolve().relative_to(self.path)
        except ValueError:
            return False
        return True

    def why_not(self, profile: Any) -> Optional[str]:
        """Why ``profile`` may not run here, or ``None``: it must be on the roster (when
        there is one) and cleared no higher than the station."""
        names = {profile.name, *getattr(profile, "aliases", ())}
        if self.roster and not names & set(self.roster):
            return f"{profile.name} is not posted to {self.label}"
        cleared = getattr(profile, "clearance", None) or "field"
        if clearance_rank(cleared) > clearance_rank(self.clearance):
            return (
                f"{profile.name} holds clearance {cleared!r}, above {self.label}'s "
                f"{self.clearance!r}"
            )
        return None


class Stations:
    """The station registry."""

    def __init__(self, stations: "Iterator[Station] | List[Station]" = ()) -> None:
        self._by_id: Dict[str, Station] = {}
        for station in stations:
            self.add(station)

    def add(self, station: Station) -> None:
        """Raises ``StationError`` when its id or name is already taken."""
        for other in self._by_id.values():
            if _keys(station) & _keys(other):
                raise StationError(f"{station.label} clashes with {other.label}")
        self._by_id[station.id] = station

    def __iter__(self) -> Iterator[Station]:
        return iter(sorted(self._by_id.values(), key=lambda s: s.id))

    def __len__(self) -> int:
        return len(self._by_id)

    def get(self, key: str) -> Station:
        """By id, the cover name (``G``), or by project name (``guatiao``)."""
        if key in self._by_id:
            return self._by_id[key]
        for station in self._by_id.values():
            if key in _keys(station):
                return station
        known = ", ".join(s.id for s in self) or "none"
        raise StationError(f"no station {key!r} (stations: {known})")

    def for_path(self, path: "str | Path") -> Optional[Station]:
        """The station whose checkout holds ``path``; the deepest when they nest."""
        matches = [s for s in self._by_id.values() if s.contains(path)]
        return max(matches, key=lambda s: len(s.path.parts), default=None)  # type: ignore[union-attr]

    @classmethod
    def load(cls, path: Optional[Path] = None) -> "Stations":
        """``<config_dir>/stations.toml``. Raises ``StationError`` for a table that does
        not fit, naming the key, so a typo is not silently a missing setting."""
        path = path or config_dir() / STATIONS_FILE
        found = cls()
        for sid, table in ((read_toml(path) or {}).get("station", {})).items():
            found.add(_station(str(sid), table, path))
        return found


def local_config(
    cwd: Optional[str],
    stations: Optional[Stations] = None,
    *,
    trust: Optional[bool] = None,
    station: Optional[Station] = None,
) -> Tuple[Optional[Path], Optional[Station]]:
    """The ``.coagent`` directory to read for ``cwd`` (or for ``station``, when given),
    and the station it belongs to.

    A station's is its ``config`` directory, else ``<path>/.coagent`` for a local one; a
    station on another theater without ``config`` has none. Outside every station, the
    nearest ``.coagent`` only when ``trust`` is true or ``COAGENT_TRUST_PROJECT=1`` (for
    a checkout under development); otherwise none.
    """
    if station is not None:
        if station.config is not None:
            return station.config, station
        if station.theater is None and station.path is not None:
            return Path(station.path) / PROJECT_DIR, station
        return None, station
    stations = stations if stations is not None else Stations.load()
    here = cwd or os.getcwd()
    station = stations.for_path(here)
    if station is not None and station.path is not None:
        return station.config or station.path / PROJECT_DIR, station
    if trust is None:
        trust = os.environ.get(TRUST_ENV, "").lower() in ("1", "true", "yes")
    root = find_project(here) if trust else None
    return (root / PROJECT_DIR if root else None), None


_KEYS = {f.name for f in fields(Station)} - {"id"}


def _keys(station: Station) -> set:
    return {station.id, *([station.name] if station.name else [])}


def _station(sid: str, table: Mapping[str, Any], source: Path) -> Station:
    unknown = sorted(set(table) - _KEYS)
    if unknown:
        raise StationError(f"{source}: station {sid!r}: unknown keys {unknown}")
    values: Dict[str, Any] = dict(table)
    if "path" in values:
        raw = str(values["path"])
        # A remote checkout's path is the theater's; only a local one is resolved here.
        values["path"] = (
            PurePosixPath(raw) if values.get("theater") else Path(os.path.expanduser(raw)).resolve()
        )
    if "config" in values:
        values["config"] = Path(os.path.expanduser(str(values["config"]))).resolve()
    if "identity" in values:
        ident = values["identity"]
        if not isinstance(ident, Mapping) or not ident.get("name") or not ident.get("email"):
            raise StationError(f"{source}: station {sid!r}: identity needs name and email")
        values["identity"] = Identity(str(ident["name"]), str(ident["email"]))
    if values.get("on_blown", "flag") not in ("flag", "fail"):
        raise StationError(f"{source}: station {sid!r}: on_blown is flag or fail")
    if "selector" in values:
        values["selector"] = {str(k): str(v) for k, v in dict(values["selector"]).items()}
    for key in ("repos", "roster"):
        if key in values:
            values[key] = tuple(str(v) for v in values[key])
    station = Station(id=sid, **values)
    clearance_rank(station.clearance)
    return station
