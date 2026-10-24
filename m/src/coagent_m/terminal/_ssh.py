"""A terminal on another theater: ``ssh -tt`` run on a local terminal provider.

The remote side gets a real pseudo-terminal from ``sshd`` (ConPTY on a Windows theater), so
the agent behaves as it would locally. On a POSIX theater the command first reports its
session id in an OSC escape sequence no terminal displays, then ``exec``s the agent, so
the agent leads that session and ``kill`` can end every process in it over a second
connection: dropping the connection alone only sends the session a hangup.
"""

from __future__ import annotations

import os
import re
import shlex
import subprocess
import threading
from typing import TYPE_CHECKING, Dict, List, Optional

from ._base import Exit, Provider, Session, TerminalError, TerminalSpec

if TYPE_CHECKING:
    from ..theaters import Theater

# OSC 777 is unassigned in xterm; terminals ignore an OSC they do not know.
_MARK = re.compile(rb"\x1b\]777;coagent-sid=(\d+)\x07")
_MARK_PREFIX = b"\x1b]777;coagent-sid="
_KILL_TIMEOUT = 15.0


def remote_command(spec: TerminalSpec, env: Dict[str, str], posix: bool) -> str:
    """The command line ``sshd`` hands the theater's shell."""
    if not posix:
        # Scaffold: cmd.exe quoting, no session id, so a kill ends only the connection.
        parts = [f'set "{k}={v}"' for k, v in env.items()]
        if spec.cwd:
            parts.append(f'cd /d "{spec.cwd}"')
        parts.append(subprocess.list2cmdline(list(spec.argv)))
        return " && ".join(parts)
    assigns = " ".join(f"{k}={shlex.quote(v)}" for k, v in env.items())
    cd = f"cd {shlex.quote(spec.cwd)} && " if spec.cwd else ""
    argv = " ".join(shlex.quote(a) for a in spec.argv)
    mark = r"printf '\033]777;coagent-sid=%s\007' $$"
    return f"{cd}{mark} && exec env {assigns} {argv}".replace("env  ", "env ")


def forwarded_env(spec: TerminalSpec) -> Dict[str, str]:
    """What M and the hooks added or changed: the supervisor's own environment stays
    on the supervisor's machine."""
    if spec.env is None:
        return {}
    base = os.environ
    env = {k: v for k, v in spec.env.items() if base.get(k) != v}
    env.setdefault("TERM", spec.env.get("TERM") or "xterm-256color")
    return env


class SshSession:
    """A local session running ``ssh``, plus what is known about the remote one."""

    def __init__(self, theater: "Theater", inner: Session) -> None:
        self._theater = theater
        self._inner = inner
        self._held = b""
        self._killed = False
        self.remote_sid: Optional[int] = None
        self.pid = inner.pid
        self._lock = threading.Lock()

    def read(self, timeout: float) -> Optional[bytes]:
        data = self._inner.read(timeout)
        if not data:
            if data == b"" and self._held:
                held, self._held = self._held, b""
                return held
            return data
        data = self._held + data
        self._held = b""
        if self.remote_sid is None:
            match = _MARK.search(data)
            if match:
                self.remote_sid = int(match.group(1))
                data = data[: match.start()] + data[match.end() :]
            else:
                data, self._held = _split_partial_mark(data)
        # Stripped to nothing is "nothing yet": b"" would say the output has ended.
        return data or None

    def write(self, data: bytes) -> None:
        self._inner.write(data)

    def resize(self, rows: int, cols: int) -> None:
        # The local pty's size reaches the remote one through ssh's window-change message.
        self._inner.resize(rows, cols)

    def kill(self) -> None:
        with self._lock:
            sid, already = self.remote_sid, self._killed
            self._killed = True
        if sid is not None and not already and self._theater.posix and self.poll() is None:
            remote_kill(self._theater, sid)
        self._inner.kill()

    def poll(self) -> Optional[Exit]:
        return self._inner.poll()

    def wait(self, timeout: Optional[float] = None) -> Optional[Exit]:
        return self._inner.wait(timeout)

    def close(self) -> None:
        if self.poll() is None:
            self.kill()
        self._inner.close()


def _split_partial_mark(data: bytes):
    """``data`` and a held tail that may be the start of the session-id mark."""
    start = data.rfind(b"\x1b")
    if start == -1:
        return data, b""
    tail = data[start:]
    if _MARK_PREFIX.startswith(tail) or (tail.startswith(_MARK_PREFIX) and b"\x07" not in tail):
        return data[:start], tail
    return data, b""


def remote_kill(theater: "Theater", sid: int) -> bool:
    """SIGKILLs every process in session ``sid`` on ``theater``; answers whether ssh ran.
    ``pkill -s`` where it exists, else the session leader's process group."""
    cmd = f"pkill -KILL -s {sid} 2>/dev/null || kill -KILL -- -{sid} 2>/dev/null; true"
    try:
        subprocess.run(
            [*theater.ssh_argv(tty=False), cmd],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=_KILL_TIMEOUT,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return True


class SshProvider:
    """``ssh:<theater>``: runs the agent on a registered theater. ``local`` is the provider the
    ``ssh`` client itself runs on (default: the best local one with a real terminal)."""

    priority = 10

    def __init__(self, theater: "Theater", local: Optional[Provider] = None) -> None:
        self.theater = theater
        self.name = theater.provider_name
        self._local = local

    def available(self) -> Optional[str]:
        try:
            self._inner_provider()
        except TerminalError as exc:
            return str(exc)
        return None

    def spawn(self, spec: TerminalSpec) -> SshSession:
        argv = [
            *self.theater.ssh_argv(tty=True),
            remote_command(spec, forwarded_env(spec), self.theater.posix),
        ]
        local = TerminalSpec(argv, None, None, spec.rows, spec.cols)
        return SshSession(self.theater, self._inner_provider().spawn(local))

    def _inner_provider(self) -> Provider:
        if self._local is not None:
            return self._local
        from ._registry import builtin_providers

        for provider in builtin_providers():
            if provider.name != "pipes" and provider.available() is None:
                return provider
        raise TerminalError("no local terminal to run ssh on")
