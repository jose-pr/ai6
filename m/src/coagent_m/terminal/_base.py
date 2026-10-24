"""The terminal contract every provider fills: a spec in, a session out."""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Dict, Optional, Protocol, Sequence, runtime_checkable

DEFAULT_ROWS = 24
DEFAULT_COLS = 80


class TerminalError(Exception):
    """A provider could not do what was asked."""


class ProviderUnavailable(TerminalError):
    """No provider can run here, or the one named cannot."""


@dataclass(frozen=True)
class TerminalSpec:
    """A program to start on a terminal.

    ``env=None`` inherits the supervisor's environment; a mapping replaces it whole.
    """

    argv: Sequence[str]
    cwd: Optional[str] = None
    env: Optional[Dict[str, str]] = None
    rows: int = DEFAULT_ROWS
    cols: int = DEFAULT_COLS

    def environment(self, term: Optional[str] = None) -> Dict[str, str]:
        """The environment to start with; ``term`` fills ``TERM`` only when it is unset."""
        env = dict(os.environ if self.env is None else self.env)
        if term is not None and not env.get("TERM"):
            env["TERM"] = term
        return env


@dataclass(frozen=True)
class Exit:
    """How a program ended: exactly one of ``code`` or ``signal`` is set."""

    code: Optional[int] = None
    signal: Optional[int] = None

    @property
    def returncode(self) -> int:
        """The ``subprocess`` convention: the exit code, or minus the signal."""
        return -self.signal if self.signal is not None else (self.code or 0)


@runtime_checkable
class Session(Protocol):
    """One running program on one terminal. ``write``, ``resize`` and ``kill`` are
    safe to call from another thread while ``read`` blocks."""

    pid: int

    def read(self, timeout: float) -> Optional[bytes]:
        """Output, ``b""`` once no more can ever arrive, or ``None`` if nothing came
        within ``timeout`` seconds. Output may still arrive after the program exits."""
        ...

    def write(self, data: bytes) -> None:
        """Input, as typed at the terminal."""
        ...

    def resize(self, rows: int, cols: int) -> None: ...

    def kill(self) -> None:
        """Ends the program and every process it started. Idempotent; a no-op once
        the program has been reaped."""
        ...

    def poll(self) -> Optional[Exit]:
        """The exit, or ``None`` while it runs. Never blocks."""
        ...

    def wait(self, timeout: Optional[float] = None) -> Optional[Exit]:
        """Blocks for the exit; ``None`` if ``timeout`` passed first."""
        ...

    def close(self) -> None:
        """Releases the terminal. Kills the program first if it still runs."""
        ...


@runtime_checkable
class Provider(Protocol):
    """Starts sessions. ``name`` is unique in a registry; ``priority`` is the
    provider's own rank, which a registry may override."""

    name: str
    priority: int

    def available(self) -> Optional[str]:
        """``None`` when it can run here, else why not, in words for a person."""
        ...

    def spawn(self, spec: TerminalSpec) -> Session:
        """Starts ``spec``. Raises ``TerminalError`` when it cannot."""
        ...


@dataclass
class ProviderInfo:
    """A provider as a registry lists it."""

    provider: Provider
    priority: int
    reason: Optional[str] = field(default=None)

    @property
    def name(self) -> str:
        return self.provider.name
