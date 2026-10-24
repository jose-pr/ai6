"""Theaters of operations: the machines missions run on, reached over ssh.

Each registered theater is also a terminal provider,
``ssh:<id>``, and a station whose ``theater`` names it keeps its checkout there.

``theaters.toml``, in the global config directory::

    [theater.win1]
    ssh = "builder@win1.example.internal"   # anything `ssh` accepts as a destination
    os = "windows"                          # linux, macos, windows
    port = 22
    identity = "~/.ssh/coagent_win1"        # optional: a key file
    options = { ServerAliveInterval = "15" }
    safe_houses = "C:/coagent/worktrees"      # where mission worktrees go on the theater
    tags = ["gpu"]
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field, fields
from pathlib import Path
from typing import Any, Dict, Iterator, List, Mapping, Optional, Tuple

from .config import config_dir, read_toml

__all__ = ["THEATERS_FILE", "OSES", "Theater", "TheaterError", "Theaters"]

THEATERS_FILE = "theaters.toml"
OSES = ("linux", "macos", "windows")


class TheaterError(Exception):
    """A theater is unknown or its declaration does not fit."""


@dataclass(frozen=True)
class Theater:
    """One machine missions can run on."""

    id: str
    ssh: str
    os: str = "linux"
    description: str = ""
    port: Optional[int] = None
    identity: Optional[str] = None
    options: Mapping[str, str] = field(default_factory=dict)
    # Where mission worktrees go on the theater; required for a station there to use them.
    safe_houses: Optional[str] = None
    tags: Tuple[str, ...] = ()
    # The ssh client to run, as argv; tests and unusual setups replace it.
    client: Tuple[str, ...] = ("ssh",)

    @property
    def provider_name(self) -> str:
        return f"ssh:{self.id}"

    @property
    def posix(self) -> bool:
        return self.os != "windows"

    def ssh_argv(self, *, tty: bool) -> List[str]:
        """The client command up to, not including, the remote command."""
        argv = list(self.client)
        argv += ["-tt"] if tty else ["-T"]
        if self.port is not None:
            argv += ["-p", str(self.port)]
        if self.identity:
            argv += ["-i", os.path.expanduser(self.identity)]
        for key, value in self.options.items():
            argv += ["-o", f"{key}={value}"]
        argv += [self.ssh, "--"]
        return argv


class Theaters:
    """The theater registry."""

    def __init__(self, theaters: "Iterator[Theater] | List[Theater]" = ()) -> None:
        self._by_id: Dict[str, Theater] = {}
        for theater in theaters:
            self.add(theater)

    def add(self, theater: Theater) -> None:
        if theater.id in self._by_id:
            raise TheaterError(f"theater {theater.id!r} is registered twice")
        self._by_id[theater.id] = theater

    def __iter__(self) -> Iterator[Theater]:
        return iter(sorted(self._by_id.values(), key=lambda h: h.id))

    def __len__(self) -> int:
        return len(self._by_id)

    def get(self, key: str) -> Theater:
        try:
            return self._by_id[key]
        except KeyError:
            known = ", ".join(sorted(self._by_id)) or "none"
            raise TheaterError(f"no theater {key!r} (theaters: {known})") from None

    def providers(self) -> List[Any]:
        """One ``ssh:<id>`` terminal provider per theater."""
        from .terminal._ssh import SshProvider

        return [SshProvider(h) for h in self]

    @classmethod
    def load(cls, path: Optional[Path] = None) -> "Theaters":
        """``<config_dir>/theaters.toml``. Raises ``TheaterError`` for a table that does not fit."""
        path = path or config_dir() / THEATERS_FILE
        found = cls()
        for hid, table in ((read_toml(path) or {}).get("theater", {})).items():
            found.add(_theater(str(hid), table, path))
        return found


_KEYS = {f.name for f in fields(Theater)} - {"id"}


def _theater(hid: str, table: Mapping[str, Any], source: Path) -> Theater:
    unknown = sorted(set(table) - _KEYS)
    if unknown:
        raise TheaterError(f"{source}: theater {hid!r}: unknown keys {unknown}")
    if not table.get("ssh"):
        raise TheaterError(f'{source}: theater {hid!r} needs ssh = "<destination>"')
    values: Dict[str, Any] = dict(table)
    if values.get("os", "linux") not in OSES:
        raise TheaterError(f"{source}: theater {hid!r}: os must be one of {OSES}")
    for key in ("tags", "client"):
        if key in values:
            values[key] = tuple(str(v) for v in values[key])
    if "options" in values:
        values["options"] = {str(k): str(v) for k, v in values["options"].items()}
    if "port" in values:
        values["port"] = int(values["port"])
    return Theater(id=hid, **values)
