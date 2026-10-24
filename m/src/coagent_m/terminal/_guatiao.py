"""Terminal providers served by guatiao libraries, in any language.

The kinds below mirror ``coagent-terminal-kind`` (Rust) slot for slot; their floor hashes
are the ``FLOOR_HASH`` that crate pins, so a library built against another contract is
refused when bound rather than called. The native ``guatiao`` library is found the way
the ``guatiao`` package finds it: ``GUATIAO_LIBRARY``, then the system's search path.
"""

from __future__ import annotations

import logging
import threading
from typing import Iterable, List, Optional, Protocol

from guatiao import GuatiaoError, Ref, Registry as _GuatiaoRegistry, Value
from guatiao.kinds import I64, KindProtocol, ValueAnswer, fallible, required

from ._base import Exit, TerminalError, TerminalSpec

KIND = "coagent_terminal"
READ_TIMED_OUT = -1
_READ_SIZE = 65536
# Above the built-in providers: a plugin someone installed is one they meant to use.
PLUGIN_PRIORITY = 30

_log = logging.getLogger(__name__)
# Libraries stay loaded while their sessions run, so one registry lives for the process.
_registry: Optional[_GuatiaoRegistry] = None
_registry_lock = threading.Lock()


class TerminalSessionKind(
    KindProtocol, Protocol, kind="coagent_terminal_session", floor_hash=3148381037, is_object=True
):
    @required
    def pid(self) -> I64: ...
    @required
    @fallible
    def read(self, dst: bytearray, timeout_ms: I64) -> I64: ...
    @required
    @fallible
    def write(self, data: bytes) -> None: ...
    @required
    @fallible
    def resize(self, rows: I64, cols: I64) -> None: ...
    @required
    @fallible
    def kill(self) -> None: ...
    @required
    @fallible
    def exit_status(self, timeout_ms: I64) -> ValueAnswer: ...


class TerminalKind(KindProtocol, Protocol, kind=KIND, floor_hash=3285297011):
    @required
    @fallible
    def spawn(self, spec: Ref) -> TerminalSessionKind: ...


class GuatiaoSession:
    """A ``coagent_terminal_session`` object, as a ``Session``."""

    def __init__(self, obj) -> None:
        self._obj = obj
        self._buf = bytearray(_READ_SIZE)
        self._read_lock = threading.Lock()
        self.pid = int(obj.pid())

    def read(self, timeout: float) -> Optional[bytes]:
        with self._read_lock:
            n = self._call("read", self._buf, int(timeout * 1000))
            if n == READ_TIMED_OUT:
                return None
            return bytes(self._buf[:n])

    def write(self, data: bytes) -> None:
        self._call("write", bytes(data))

    def resize(self, rows: int, cols: int) -> None:
        self._call("resize", rows, cols)

    def kill(self) -> None:
        self._call("kill")

    def poll(self) -> Optional[Exit]:
        return self._exit(0)

    def wait(self, timeout: Optional[float] = None) -> Optional[Exit]:
        if timeout is not None:
            return self._exit(int(timeout * 1000))
        while True:
            exit_ = self._exit(1000)
            if exit_ is not None:
                return exit_

    def close(self) -> None:
        obj, self._obj = self._obj, None
        if obj is not None:
            obj.close()  # the provider's destroy kills a program still running

    def _exit(self, timeout_ms: int) -> Optional[Exit]:
        answer = self._call("exit_status", timeout_ms)
        try:
            info = answer.to_python()
        finally:
            answer.close()
        if info.get("running"):
            return None
        return Exit(code=info.get("code"), signal=info.get("signal"))

    def _call(self, slot: str, *args):
        if self._obj is None:
            raise TerminalError("the session is closed")
        try:
            return getattr(self._obj, slot)(*args)
        except GuatiaoError as exc:
            raise TerminalError(f"{slot}: {exc}") from exc


class GuatiaoProvider:
    """One guatiao provider of ``coagent_terminal``, named by its registry key."""

    def __init__(self, registry: _GuatiaoRegistry, key: str, priority: int) -> None:
        self.name = key
        self.priority = priority
        self._registry = registry
        self._remote = registry.bind(key, TerminalKind)

    def available(self) -> Optional[str]:
        ok, reason = self._registry.provider_available(self.name)
        return None if ok else (reason or "the provider says it cannot run here")

    def spawn(self, spec: TerminalSpec) -> GuatiaoSession:
        data = {"argv": list(spec.argv), "rows": spec.rows, "cols": spec.cols}
        if spec.cwd is not None:
            data["cwd"] = spec.cwd
        if spec.env is not None:
            data["env"] = dict(spec.env)
        with Value(data) as value:
            try:
                obj = self._remote.spawn(value)
            except GuatiaoError as exc:
                raise TerminalError(f"{self.name}: {exc}") from exc
        return GuatiaoSession(obj)


def scan(dirs: Iterable[str], priority: int = PLUGIN_PRIORITY) -> List[GuatiaoProvider]:
    """Loads the libraries in ``dirs`` and answers every provider of the kind they serve.
    A provider whose table does not fit the kind is logged and skipped."""
    global _registry
    with _registry_lock:
        if _registry is None:
            from .. import __version__

            _registry = _GuatiaoRegistry("coagent-m", __version__)
        registry = _registry
        for d in dirs:
            registry.scan_dir(d, rules=f"kind={KIND}")
        found: List[GuatiaoProvider] = []
        for info in registry.providers(KIND):
            try:
                found.append(GuatiaoProvider(registry, info["key"], priority))
            except GuatiaoError as exc:
                _log.warning("terminal provider %r skipped: %s", info["key"], exc)
        return found
