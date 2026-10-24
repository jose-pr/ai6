"""A pseudo-terminal from the standard library: Linux, macOS, the BSDs."""

from __future__ import annotations

import os
import select
import signal
import subprocess
import sys
import threading
from typing import Optional

from ._base import Exit, TerminalError, TerminalSpec

_READ_SIZE = 65536


class PosixPtySession:
    """A program in its own session and process group on a fresh pty."""

    def __init__(self, spec: TerminalSpec) -> None:
        import pty

        master, slave = pty.openpty()
        _set_size(slave, spec.rows, spec.cols)
        try:
            self._proc = subprocess.Popen(
                list(spec.argv),
                stdin=slave,
                stdout=slave,
                stderr=slave,
                cwd=spec.cwd,
                env=spec.environment(term="xterm-256color"),
                start_new_session=True,  # pgid == pid, so killpg reaches every descendant
                close_fds=True,
            )
        except OSError as exc:
            os.close(master)
            raise TerminalError(f"cannot start {spec.argv[0]!r}: {exc}") from exc
        finally:
            os.close(slave)
        self._master: Optional[int] = master
        self._lock = threading.Lock()
        self._reaped = False
        self.pid = self._proc.pid

    def read(self, timeout: float) -> Optional[bytes]:
        master = self._master
        if master is None:
            return b""
        readable, _, _ = select.select([master], [], [], timeout)
        if not readable:
            return None
        try:
            return os.read(master, _READ_SIZE)
        except OSError:  # EIO on Linux once every slave fd is closed
            return b""

    def write(self, data: bytes) -> None:
        master = self._master
        if master is None:
            raise TerminalError("the terminal is closed")
        view = memoryview(data)
        while view:
            view = view[os.write(master, view) :]

    def resize(self, rows: int, cols: int) -> None:
        if self._master is not None:
            _set_size(self._master, rows, cols)

    def kill(self) -> None:
        with self._lock:
            # Once reaped, the pid may belong to an unrelated process.
            if self._reaped:
                return
            try:
                os.killpg(self.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass

    def poll(self) -> Optional[Exit]:
        return self._exit(self._proc.poll())

    def wait(self, timeout: Optional[float] = None) -> Optional[Exit]:
        try:
            return self._exit(self._proc.wait(timeout))
        except subprocess.TimeoutExpired:
            return None

    def close(self) -> None:
        if self.poll() is None:
            self.kill()
            self._proc.wait()
        with self._lock:
            master, self._master = self._master, None
        if master is not None:
            os.close(master)

    def _exit(self, returncode: Optional[int]) -> Optional[Exit]:
        if returncode is None:
            return None
        with self._lock:
            self._reaped = True
        return Exit(signal=-returncode) if returncode < 0 else Exit(code=returncode)


class PosixPtyProvider:
    """``posix-pty``: the stdlib ``pty`` module; no dependencies."""

    name = "posix-pty"
    priority = 20

    def available(self) -> Optional[str]:
        if sys.platform == "win32":
            return "Windows has no POSIX pty; use conpty"
        return None

    def spawn(self, spec: TerminalSpec) -> PosixPtySession:
        reason = self.available()
        if reason:
            raise TerminalError(reason)
        return PosixPtySession(spec)


def _set_size(fd: int, rows: int, cols: int) -> None:
    import fcntl
    import struct
    import termios

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
