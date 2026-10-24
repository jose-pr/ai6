"""Hosts: the machines missions run on, reached over ssh.

A host is a theatre of operations. Each registered host is also a terminal provider,
``ssh:<id>``, and a station whose ``host`` names it keeps its checkout there.

``hosts.toml``, in the global config directory::

    [host.win1]
    ssh = "builder@win1.example.internal"   # anything `ssh` accepts as a destination
    os = "windows"                          # linux, macos, windows
    port = 22
    identity = "~/.ssh/coagent_win1"        # optional: a key file
    options = { ServerAliveInterval = "15" }
    work_root = "C:/coagent/worktrees"      # where mission worktrees go on the host
    tags = ["gpu"]
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field, fields
from pathlib import Path
from typing import Any, Dict, Iterator, List, Mapping, Optional, Tuple

from .config import config_dir, read_toml

__all__ = ["HOSTS_FILE", "OSES", "Host", "HostError", "Hosts"]

HOSTS_FILE = "hosts.toml"
OSES = ("linux", "macos", "windows")


class HostError(Exception):
    """A host is unknown or its declaration does not fit."""


@dataclass(frozen=True)
class Host:
    """One machine missions can run on."""

    id: str
    ssh: str
    os: str = "linux"
    description: str = ""
    port: Optional[int] = None
    identity: Optional[str] = None
    options: Mapping[str, str] = field(default_factory=dict)
    # Where mission worktrees go on the host; required for a station there to use them.
    work_root: Optional[str] = None
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


class Hosts:
    """The host registry."""

    def __init__(self, hosts: "Iterator[Host] | List[Host]" = ()) -> None:
        self._by_id: Dict[str, Host] = {}
        for host in hosts:
            self.add(host)

    def add(self, host: Host) -> None:
        if host.id in self._by_id:
            raise HostError(f"host {host.id!r} is registered twice")
        self._by_id[host.id] = host

    def __iter__(self) -> Iterator[Host]:
        return iter(sorted(self._by_id.values(), key=lambda h: h.id))

    def __len__(self) -> int:
        return len(self._by_id)

    def get(self, key: str) -> Host:
        try:
            return self._by_id[key]
        except KeyError:
            known = ", ".join(sorted(self._by_id)) or "none"
            raise HostError(f"no host {key!r} (hosts: {known})") from None

    def providers(self) -> List[Any]:
        """One ``ssh:<id>`` terminal provider per host."""
        from .terminal._ssh import SshProvider

        return [SshProvider(h) for h in self]

    @classmethod
    def load(cls, path: Optional[Path] = None) -> "Hosts":
        """``<config_dir>/hosts.toml``. Raises ``HostError`` for a table that does not fit."""
        path = path or config_dir() / HOSTS_FILE
        found = cls()
        for hid, table in ((read_toml(path) or {}).get("host", {})).items():
            found.add(_host(str(hid), table, path))
        return found


_KEYS = {f.name for f in fields(Host)} - {"id"}


def _host(hid: str, table: Mapping[str, Any], source: Path) -> Host:
    unknown = sorted(set(table) - _KEYS)
    if unknown:
        raise HostError(f"{source}: host {hid!r}: unknown keys {unknown}")
    if not table.get("ssh"):
        raise HostError(f'{source}: host {hid!r} needs ssh = "<destination>"')
    values: Dict[str, Any] = dict(table)
    if values.get("os", "linux") not in OSES:
        raise HostError(f"{source}: host {hid!r}: os must be one of {OSES}")
    for key in ("tags", "client"):
        if key in values:
            values[key] = tuple(str(v) for v in values[key])
    if "options" in values:
        values["options"] = {str(k): str(v) for k, v in values["options"].items()}
    if "port" in values:
        values["port"] = int(values["port"])
    return Host(id=hid, **values)
