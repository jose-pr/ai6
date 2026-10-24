"""Hooks that are programs, in any language: the agent's details as JSON on stdin,
changes read back from stdout."""

from __future__ import annotations

import json
import os
import re
import shlex
import subprocess
from dataclasses import dataclass
from typing import Any, Dict, Optional, Sequence

from ._base import HookContext, HookError, Preparation

DEFAULT_TIMEOUT = 30.0

# `export K=V`, `K=V`, `set K=V`; the value may be quoted the way a POSIX shell quotes it.
_ENV_LINE = re.compile(r"^\s*(?:export\s+|set\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$")


@dataclass(frozen=True)
class CommandHook:
    """Runs ``command`` in the agent's working directory before it starts.

    ``output`` says what stdout is: ``env`` (``K=V`` lines, as ``env`` or ``export``
    prints them), ``context`` (text for the agent), ``json`` (a ``Preparation`` as an
    object: ``env``, ``context``, ``argv``) or ``none``.
    """

    name: str
    command: Sequence[str]
    output: str = "json"
    required: bool = False
    timeout: float = DEFAULT_TIMEOUT
    env: Optional[Dict[str, str]] = None

    def prepare(self, ctx: HookContext) -> Optional[Preparation]:
        env = dict(ctx.env)
        env.update(self.env or {})
        env.update(
            COAGENT_HOOK="prepare",
            COAGENT_AGENT=ctx.agent,
            COAGENT_RUN_DIR=ctx.run_dir,
            COAGENT_CWD=ctx.cwd or os.getcwd(),
        )
        if ctx.theater:
            env["COAGENT_THEATER"] = ctx.theater
        try:
            done = subprocess.run(
                list(self.command),
                input=json.dumps(ctx.as_json()).encode(),
                capture_output=True,
                # A remote agent's directory is not on this machine; the hook runs here.
                cwd=None if ctx.theater else ctx.cwd,
                env=env,
                timeout=self.timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise HookError(f"hook {self.name!r}: {exc}") from exc
        if done.returncode != 0:
            stderr = done.stderr.decode(errors="replace").strip()
            raise HookError(f"hook {self.name!r} exited {done.returncode}: {stderr[-500:]}")
        return parse_output(self.output, done.stdout.decode(errors="replace"), self.name)


def parse_output(kind: str, text: str, name: str = "hook") -> Optional[Preparation]:
    """Reads a command hook's stdout as ``kind``. Raises ``HookError`` for bad JSON or an
    unknown kind."""
    if kind == "none":
        return None
    if kind == "context":
        return Preparation(context=[text.strip()]) if text.strip() else None
    if kind == "env":
        return Preparation(env=_parse_env(text))
    if kind == "json":
        if not text.strip():
            return None
        try:
            data: Any = json.loads(text)
        except ValueError as exc:
            raise HookError(f"hook {name!r} printed invalid JSON: {exc}") from exc
        if not isinstance(data, dict):
            raise HookError(f"hook {name!r} must print a JSON object")
        context = data.get("context") or []
        return Preparation(
            env={
                str(k): (None if v is None else str(v)) for k, v in (data.get("env") or {}).items()
            },
            context=[context] if isinstance(context, str) else [str(c) for c in context],
            argv=[str(a) for a in data["argv"]] if data.get("argv") is not None else None,
        )
    raise HookError(f"hook {name!r}: unknown output kind {kind!r} (env, context, json, none)")


def _parse_env(text: str) -> Dict[str, Optional[str]]:
    env: Dict[str, Optional[str]] = {}
    for line in text.splitlines():
        match = _ENV_LINE.match(line)
        if not match:
            continue
        key, raw = match.groups()
        try:
            parts = shlex.split(raw)
        except ValueError:
            parts = [raw]
        env[key] = parts[0] if len(parts) == 1 else " ".join(parts)
    return env
