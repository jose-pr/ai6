"""What a hook sees and what it may change."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Protocol, Sequence, runtime_checkable


class HookError(Exception):
    """A required hook failed, so the agent is not started."""


@dataclass
class HookContext:
    """An agent about to start, as each hook in turn receives it.

    ``env`` is the whole environment it will get; ``context`` the text blocks it will be
    given at start, in order. ``run_dir`` is a private directory that lives as long as
    the run.
    """

    agent: str
    argv: List[str]
    cwd: Optional[str]
    env: Dict[str, str]
    run_dir: str
    context: List[str] = field(default_factory=list)
    # The theater the agent runs on, when not this machine; `cwd` is then a path there.
    theater: Optional[str] = None

    def as_json(self) -> Dict[str, Any]:
        return {
            "agent": self.agent,
            "argv": list(self.argv),
            "cwd": self.cwd,
            "run_dir": self.run_dir,
            "context": list(self.context),
            "theater": self.theater,
        }


@dataclass(frozen=True)
class Preparation:
    """A hook's changes: ``env`` merged over the context's (``None`` removes a key),
    ``context`` appended, ``argv`` replacing it when set."""

    env: Dict[str, Optional[str]] = field(default_factory=dict)
    context: Sequence[str] = ()
    argv: Optional[Sequence[str]] = None

    def apply(self, ctx: HookContext) -> None:
        for key, value in self.env.items():
            if value is None:
                ctx.env.pop(key, None)
            else:
                ctx.env[key] = value
        ctx.context.extend(c for c in self.context if c)
        if self.argv is not None:
            ctx.argv = list(self.argv)


@runtime_checkable
class Hook(Protocol):
    """Any object with ``name`` and any of ``prepare``, ``on_event`` and ``finished``.

    ``required``: a failing ``prepare`` stops the run instead of being logged and skipped.
    """

    name: str


class HookBase:
    """Optional base: every callback is a no-op."""

    name = "hook"
    required = False

    def prepare(self, ctx: HookContext) -> Optional[Preparation]:
        return None

    def on_event(self, event: Any) -> None:
        pass

    def finished(self, ctx: HookContext, outcome: Any) -> None:
        pass
