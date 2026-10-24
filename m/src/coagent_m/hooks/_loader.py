"""Finding hooks: global, then project, then agent; and running them in that order."""

from __future__ import annotations

import importlib
import logging
from importlib import metadata
from pathlib import Path
from typing import Any, Iterable, List, Optional, Sequence, Tuple

from ..config import TRUST_ENV, Scope, config_dir, read_toml
from ..stations import Station, Stations, local_config
from ._base import Hook, HookContext, HookError
from ._command import DEFAULT_TIMEOUT, CommandHook

ENTRY_POINT_GROUP = "coagent.hooks"
HOOKS_FILE = "hooks.toml"

_log = logging.getLogger(__name__)


class HookSet:
    """Hooks in the order they run. A hook that raises is logged and skipped, unless it is
    ``required`` and raised in ``prepare``, which raises ``HookError``."""

    def __init__(self, hooks: Iterable[Tuple[Scope, Hook]] = ()) -> None:
        self.hooks: List[Tuple[Scope, Hook]] = list(hooks)

    def __len__(self) -> int:
        return len(self.hooks)

    def prepare(self, ctx: HookContext) -> None:
        """Lets each hook change ``ctx`` in place, in order."""
        for scope, hook in self.hooks:
            prepare = getattr(hook, "prepare", None)
            if prepare is None:
                continue
            try:
                change = prepare(ctx)
                if change is not None:
                    change.apply(ctx)
            except Exception as exc:
                if getattr(hook, "required", False):
                    if isinstance(exc, HookError):
                        raise
                    raise HookError(f"{scope.value} hook {hook.name!r}: {exc}") from exc
                _log.warning("%s hook %r skipped: %s", scope.value, hook.name, exc)

    def on_event(self, event: Any) -> None:
        self._each("on_event", event)

    def finished(self, ctx: HookContext, outcome: Any) -> None:
        self._each("finished", ctx, outcome)

    def _each(self, method: str, *args: Any) -> None:
        for scope, hook in self.hooks:
            fn = getattr(hook, method, None)
            if fn is None:
                continue
            try:
                fn(*args)
            except Exception:
                _log.exception("%s hook %r failed in %s", scope.value, hook.name, method)


def load_file(path: Path, scope: Scope) -> List[Tuple[Scope, Hook]]:
    """The ``[[hook]]`` tables of one ``hooks.toml``. A table that cannot be built is
    logged and skipped; so is a missing file."""
    tables = (read_toml(path) or {}).get("hook", [])
    found: List[Tuple[Scope, Hook]] = []
    for i, table in enumerate(tables):
        try:
            found.append((scope, _build(table, path.parent, f"{path.name}#{i + 1}")))
        except (KeyError, TypeError, ValueError, ImportError, AttributeError) as exc:
            _log.warning("%s hook %d skipped: %s", path, i + 1, exc)
    return found


def entry_point_hooks() -> List[Tuple[Scope, Hook]]:
    """Hooks other packages declare in the ``coagent.hooks`` entry-point group, as global
    hooks. Each names a hook, or a class or callable answering one."""
    found: List[Tuple[Scope, Hook]] = []
    eps = metadata.entry_points()
    group = (
        eps.select(group=ENTRY_POINT_GROUP)
        if hasattr(eps, "select")
        else eps.get(ENTRY_POINT_GROUP, ())
    )
    for ep in group:
        try:
            found.append((Scope.GLOBAL, _instance(ep.load())))
        except Exception:  # a broken plugin must not take the supervisor down
            _log.exception("hook entry point %r failed to load", ep.name)
    return found


def load_hooks(
    cwd: Optional[str] = None,
    agent_hooks: Sequence[Hook] = (),
    *,
    trust_project: Optional[bool] = None,
    stations: Optional[Stations] = None,
    station: Optional[Station] = None,
) -> HookSet:
    """Global hooks (entry points, then ``<config_dir>/hooks.toml``), then those of the
    station holding ``cwd`` (``<station>/.coagent/hooks.toml``), then ``agent_hooks``.
    Outside every station a ``.coagent`` is read only with ``trust_project`` (or
    ``COAGENT_TRUST_PROJECT=1``)."""
    hooks = [*entry_point_hooks(), *load_file(config_dir() / HOOKS_FILE, Scope.GLOBAL)]
    local, station = local_config(cwd, stations, trust=trust_project, station=station)
    if local is not None:
        hooks += load_file(local / HOOKS_FILE, Scope.STATION)
    elif station is None:
        _log.debug("no station holds %s; its .coagent is not read (see %s)", cwd, TRUST_ENV)
    hooks += [(Scope.AGENT, h) for h in agent_hooks]
    return HookSet(hooks)


def _build(table: dict, base: Path, default_name: str) -> Hook:
    name = str(table.get("name", default_name))
    if "python" in table:
        hook = _instance(_import(str(table["python"])))
        if "name" in table:
            hook.name = name
        if "required" in table:
            hook.required = bool(table["required"])
        return hook
    command = table["command"]
    if isinstance(command, str):
        raise TypeError("command must be a list of arguments, not a string")
    command = [str(c) for c in command]
    if command and command[0].startswith(("./", "../")):
        command[0] = str((base / command[0]).resolve())
    return CommandHook(
        name=name,
        command=command,
        output=str(table.get("output", "json")),
        required=bool(table.get("required", False)),
        timeout=float(table.get("timeout", DEFAULT_TIMEOUT)),
        env={str(k): str(v) for k, v in table.get("env", {}).items()},
    )


def _import(spec: str) -> Any:
    module, _, attr = spec.partition(":")
    obj: Any = importlib.import_module(module)
    for part in filter(None, attr.split(".")):
        obj = getattr(obj, part)
    return obj


def _instance(obj: Any) -> Any:
    hook = obj() if isinstance(obj, type) else obj
    if not hasattr(hook, "name"):
        raise TypeError(f"{obj!r} is not a hook: it has no name")
    return hook
