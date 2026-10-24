"""Intake sources: whatever turns a tracker, a webhook or a schedule into briefs.

coagent ships none; a user supplies them. A source is a Python object (an entry point in
the ``coagent.intake`` group) or a program in any language declared in ``sources.toml``.
Either can also be told how a brief ended, to comment on or move its ticket. A webhook
receiver needs no source at all: it writes briefs into the inbox
(``Inbox.submit``, or ``python -m coagent_i submit``).

``sources.toml``, in the global config directory::

    [[source]]
    name = "jira"
    command = ["jira-briefs", "--jql", "project = GUA AND labels = agent"]
    report = ["jira-report"]          # optional: the outcome as JSON on stdin
    station = "G"                     # for briefs that name none
    timeout = 60
    env = { JIRA_SITE = "example.atlassian.net" }
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
import sys
from dataclasses import dataclass, field, replace
from importlib import metadata
from pathlib import Path
from typing import Any, Iterable, List, Mapping, Optional, Protocol, Sequence, Tuple

from .brief import Brief, BriefError
from .inbox import Inbox

if sys.version_info >= (3, 11):
    import tomllib
else:  # pragma: no cover - exercised on the 3.9/3.10 floor
    import tomli as tomllib

__all__ = [
    "ENTRY_POINT_GROUP",
    "SOURCES_FILE",
    "CommandSource",
    "Source",
    "SourceError",
    "config_dir",
    "entry_point_sources",
    "load_sources",
    "poll",
    "report",
]

ENTRY_POINT_GROUP = "coagent.intake"
SOURCES_FILE = "sources.toml"
DEFAULT_TIMEOUT = 60.0

_log = logging.getLogger(__name__)


class SourceError(Exception):
    """A source failed, or its declaration does not fit."""


class Source(Protocol):
    """``name`` is unique; ``poll()`` answers new briefs. An optional
    ``report(brief, outcome)`` hears how one ended; ``outcome`` is a plain mapping
    (``state``, ``returncode``, ``reason``, ``runtime``, ``agent``, ``profile``,
    ``station``)."""

    name: str

    def poll(self) -> Iterable[Brief]: ...


@dataclass(frozen=True)
class CommandSource:
    """A program that prints briefs on stdout: a JSON list, an object with ``briefs``,
    or one JSON object per line. ``report`` is a program that reads
    ``{"brief": .., "outcome": ..}`` on stdin."""

    name: str
    command: Sequence[str]
    report_command: Optional[Sequence[str]] = None
    station: Optional[str] = None
    timeout: float = DEFAULT_TIMEOUT
    env: Mapping[str, str] = field(default_factory=dict)
    cwd: Optional[str] = None

    def poll(self) -> List[Brief]:
        stdout = self._run(self.command, None)
        return [self._own(b) for b in parse_briefs(stdout, self.name)]

    def report(self, brief: Brief, outcome: Mapping[str, Any]) -> None:
        if self.report_command:
            payload = json.dumps({"brief": brief.to_dict(), "outcome": dict(outcome)})
            self._run(self.report_command, payload)

    def _own(self, brief: Brief) -> Brief:
        return replace(brief, source=self.name, station=brief.station or self.station)

    def _run(self, command: Sequence[str], stdin: Optional[str]) -> str:
        env = {**os.environ, **self.env, "COAGENT_SOURCE": self.name}
        try:
            done = subprocess.run(
                list(command),
                input=(stdin or "").encode(),
                capture_output=True,
                cwd=self.cwd,
                env=env,
                timeout=self.timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise SourceError(f"source {self.name!r}: {exc}") from exc
        if done.returncode != 0:
            err = done.stderr.decode(errors="replace").strip()[-500:]
            raise SourceError(f"source {self.name!r} exited {done.returncode}: {err}")
        return done.stdout.decode(errors="replace")


def parse_briefs(text: str, name: str = "source") -> List[Brief]:
    """Briefs from a source's stdout. Raises ``SourceError`` naming the source."""
    text = text.strip()
    if not text:
        return []
    try:
        data: Any = json.loads(text)
        items = data.get("briefs", []) if isinstance(data, dict) else data
    except ValueError:
        try:
            items = [json.loads(line) for line in text.splitlines() if line.strip()]
        except ValueError as exc:
            raise SourceError(
                f"source {name!r} printed neither JSON nor JSON lines: {exc}"
            ) from exc
    if not isinstance(items, list):
        raise SourceError(f"source {name!r} must print a list of briefs")
    try:
        return [Brief.from_dict(item) for item in items]
    except BriefError as exc:
        raise SourceError(f"source {name!r}: {exc}") from exc


def config_dir() -> Path:
    """The global config directory, by the rule ``coagent_m.config.config_dir`` uses:
    ``$COAGENT_HOME``, else ``%APPDATA%\\coagent``, else ``$XDG_CONFIG_HOME/coagent``."""
    if os.environ.get("COAGENT_HOME"):
        return Path(os.environ["COAGENT_HOME"])
    if sys.platform == "win32" and os.environ.get("APPDATA"):
        return Path(os.environ["APPDATA"]) / "coagent"
    base = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
    return Path(base) / "coagent"


def entry_point_sources() -> List[Any]:
    """Sources other packages declare in the ``coagent.intake`` group: each names a
    source, or a class or callable answering one. A broken one is logged and skipped."""
    found: List[Any] = []
    eps = metadata.entry_points()
    group = (
        eps.select(group=ENTRY_POINT_GROUP)
        if hasattr(eps, "select")
        else eps.get(ENTRY_POINT_GROUP, ())
    )
    for ep in group:
        try:
            obj = ep.load()
            found.append(obj() if callable(obj) and not hasattr(obj, "poll") else obj)
        except Exception:  # a broken plugin must not take intake down
            _log.exception("intake entry point %r failed to load", ep.name)
    return found


def load_sources(path: Optional[Path] = None, *, entry_points: bool = True) -> List[Any]:
    """Entry-point sources, then those in ``sources.toml``. Raises ``SourceError`` for a
    table that does not fit or a name used twice."""
    sources: List[Any] = entry_point_sources() if entry_points else []
    path = path or config_dir() / SOURCES_FILE
    if path.is_file():
        try:
            with open(path, "rb") as f:
                tables = tomllib.load(f).get("source", [])
        except (OSError, tomllib.TOMLDecodeError) as exc:
            raise SourceError(f"cannot read {path}: {exc}") from exc
        sources += [_command_source(t, path) for t in tables]
    names = [s.name for s in sources]
    dupes = sorted({n for n in names if names.count(n) > 1})
    if dupes:
        raise SourceError(f"intake sources named twice: {dupes}")
    return sources


def poll(sources: Iterable[Any], inbox: Inbox) -> Tuple[List[Brief], List[str]]:
    """Polls every source once and files what is new. Answers the briefs filed and one
    line per source that failed; a brief already in the inbox is not filed again."""
    filed: List[Brief] = []
    errors: List[str] = []
    for source in sources:
        try:
            briefs = list(source.poll())
        except Exception as exc:
            errors.append(f"{source.name}: {exc}")
            continue
        for brief in briefs:
            if brief.source != source.name:
                brief = replace(brief, source=source.name)
            if inbox.find(brief.key) is None:
                inbox.submit(brief)
                filed.append(brief)
    return filed, errors


def report(
    brief: Brief, outcome: Mapping[str, Any], sources: Optional[Iterable[Any]] = None
) -> bool:
    """Tells the brief's source how it ended. ``False`` when the source is unknown or
    takes no reports; a failing report is logged, never raised: the work is done."""
    for source in sources if sources is not None else load_sources():
        if source.name != brief.source:
            continue
        fn = getattr(source, "report", None)
        if fn is None:
            return False
        try:
            fn(brief, outcome)
            return True
        except Exception:
            _log.exception("source %r could not take the report for %s", source.name, brief.key)
            return False
    return False


_KEYS = {"name", "command", "report", "station", "timeout", "env", "cwd"}


def _command_source(table: Mapping[str, Any], path: Path) -> CommandSource:
    unknown = sorted(set(table) - _KEYS)
    if unknown:
        raise SourceError(f"{path}: source {table.get('name', '?')!r}: unknown keys {unknown}")
    if "name" not in table or not isinstance(table.get("command"), list):
        raise SourceError(f"{path}: a source needs name and command = [...]")
    base = path.parent

    def resolve(cmd: Sequence[Any]) -> Tuple[str, ...]:
        cmd = [str(c) for c in cmd]
        if cmd and cmd[0].startswith(("./", "../")):
            cmd[0] = str((base / cmd[0]).resolve())
        return tuple(cmd)

    return CommandSource(
        name=str(table["name"]),
        command=resolve(table["command"]),
        report_command=resolve(table["report"]) if table.get("report") else None,
        station=str(table["station"]) if table.get("station") else None,
        timeout=float(table.get("timeout", DEFAULT_TIMEOUT)),
        env={str(k): str(v) for k, v in table.get("env", {}).items()},
        cwd=str(table["cwd"]) if table.get("cwd") else None,
    )
