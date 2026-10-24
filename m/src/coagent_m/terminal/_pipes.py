"""Plain pipes: no terminal at all, so it runs anywhere ``subprocess`` does.

A program that checks ``isatty()`` behaves non-interactively here, which suits an agent
run headless (``claude -p``) and nothing that draws a full-screen UI. ``resize`` does
nothing.
"""

from __future__ import annotations

import os
import queue
import signal
import subprocess
import sys
import threading
from typing import Optional

from ._base import Exit, TerminalError, TerminalSpec
from ._wintree import ProcessTree

_READ_SIZE = 65536


class PipeSession:
    """stdout and stderr merged into one stream, read on a thread."""

    def __init__(self, spec: TerminalSpec) -> None:
        kwargs = {}
        if sys.platform == "win32":
            kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
        else:
            kwargs["start_new_session"] = True
        try:
            self._proc = subprocess.Popen(
                list(spec.argv),
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                cwd=spec.cwd,
                env=spec.environment(),
                bufsize=0,
                **kwargs,
            )
        except OSError as exc:
            raise TerminalError(f"cannot start {spec.argv[0]!r}: {exc}") from exc
        self.pid = self._proc.pid
        self._tree = ProcessTree.adopt(self._proc) if sys.platform == "win32" else None
        self._lock = threading.Lock()
        self._reaped = False
        self._chunks: "queue.Queue[bytes]" = queue.Queue()
        self._eof = False
        self._reader = threading.Thread(target=self._pump, name=f"pipe-{self.pid}", daemon=True)
        self._reader.start()

    def _pump(self) -> None:
        stdout = self._proc.stdout
        assert stdout is not None
        while True:
            try:
                # Unbuffered (bufsize=0): answers what is available, up to the size.
                data = stdout.read(_READ_SIZE)
            except (OSError, ValueError):
                data = b""
            self._chunks.put(data)
            if not data:
                return

    def read(self, timeout: float) -> Optional[bytes]:
        if self._eof:
            return b""
        try:
            data = self._chunks.get(timeout=timeout)
        except queue.Empty:
            return None
        if not data:
            self._eof = True
        return data

    def write(self, data: bytes) -> None:
        stdin = self._proc.stdin
        if stdin is None or stdin.closed:
            raise TerminalError("the program's input is closed")
        try:
            stdin.write(data)
            stdin.flush()
        except (BrokenPipeError, ValueError) as exc:
            raise TerminalError("the program's input is closed") from exc

    def resize(self, rows: int, cols: int) -> None:
        pass

    def kill(self) -> None:
        with self._lock:
            if self._reaped:
                return
            if self._tree is not None:
                self._tree.kill()
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
        for stream in (self._proc.stdin, self._proc.stdout):
            if stream is not None:
                try:
                    stream.close()
                except OSError:
                    pass
        if self._tree is not None:
            self._tree.close()

    def _exit(self, returncode: Optional[int]) -> Optional[Exit]:
        if returncode is None:
            return None
        with self._lock:
            self._reaped = True
        return Exit(signal=-returncode) if returncode < 0 else Exit(code=returncode)


class PipesProvider:
    """``pipes``: works on every OS; the fallback when no terminal provider can run."""

    name = "pipes"
    priority = 0

    def available(self) -> Optional[str]:
        return None

    def spawn(self, spec: TerminalSpec) -> PipeSession:
        return PipeSession(spec)
