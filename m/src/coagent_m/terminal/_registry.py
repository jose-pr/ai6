"""Which providers exist here, ranked; and which one to use."""

from __future__ import annotations

import logging
import os
from importlib import metadata
from typing import Dict, Iterable, List, Optional

from ._base import Provider, ProviderInfo, ProviderUnavailable

ENTRY_POINT_GROUP = "coagent.terminals"
PLUGIN_PATH_ENV = "COAGENT_TERMINAL_PLUGINS"

_log = logging.getLogger(__name__)


class Registry:
    """Providers by name, listed best first: priority descending, then name.

    A provider's own ``priority`` is its rank until ``set_priority`` says otherwise,
    so an operator can rank providers without touching them.
    """

    def __init__(self, providers: Iterable[Provider] = ()) -> None:
        self._providers: Dict[str, Provider] = {}
        self._priority: Dict[str, int] = {}
        for provider in providers:
            self.add(provider)

    def add(self, provider: Provider) -> None:
        """Raises ``ValueError`` for a name already present."""
        if provider.name in self._providers:
            raise ValueError(f"a terminal provider named {provider.name!r} is already registered")
        self._providers[provider.name] = provider

    def set_priority(self, name: str, priority: int) -> None:
        """Ranks ``name``, present yet or not."""
        self._priority[name] = priority

    def providers(self) -> List[ProviderInfo]:
        """Every provider, best first, each with why it cannot run (asked now)."""
        infos = [
            ProviderInfo(p, self._priority.get(p.name, p.priority), _reason(p))
            for p in self._providers.values()
        ]
        infos.sort(key=lambda i: (-i.priority, i.name))
        return infos

    def get(self, name: str) -> Provider:
        """The provider called ``name`` if it can run here, else ``ProviderUnavailable``."""
        provider = self._providers.get(name)
        if provider is None:
            known = ", ".join(sorted(self._providers)) or "none"
            raise ProviderUnavailable(f"no terminal provider {name!r} (known: {known})")
        reason = _reason(provider)
        if reason:
            raise ProviderUnavailable(f"terminal provider {name!r} cannot run here: {reason}")
        return provider

    def best(self) -> Provider:
        """The highest-ranked provider that can run here."""
        infos = self.providers()
        for info in infos:
            if info.reason is None:
                return info.provider
        why = "; ".join(f"{i.name}: {i.reason}" for i in infos) or "none is registered"
        raise ProviderUnavailable(f"no terminal provider can run here ({why})")


def builtin_providers() -> List[Provider]:
    from ._conpty import ConPtyProvider
    from ._pipes import PipesProvider
    from ._posix_pty import PosixPtyProvider

    return [PosixPtyProvider(), ConPtyProvider(), PipesProvider()]


def entry_point_providers() -> List[Provider]:
    """Providers other packages declare in the ``coagent.terminals`` entry-point group.

    Each entry point names a provider, or a callable answering one or a list of them.
    One that fails to load is logged and skipped.
    """
    found: List[Provider] = []
    for ep in _entry_points(ENTRY_POINT_GROUP):
        try:
            obj = ep.load()
            if callable(obj) and not isinstance(obj, Provider):
                obj = obj()
            found.extend(obj if isinstance(obj, (list, tuple)) else [obj])
        except Exception:  # a broken plugin must not take the supervisor down
            _log.exception("terminal provider entry point %r failed to load", ep.name)
    return found


def plugin_providers(dirs: Optional[Iterable[str]] = None) -> List[Provider]:
    """Providers served by guatiao libraries in ``dirs`` (default: the
    ``COAGENT_TERMINAL_PLUGINS`` path list). Empty when guatiao is not installed."""
    if dirs is None:
        dirs = [d for d in os.environ.get(PLUGIN_PATH_ENV, "").split(os.pathsep) if d]
    dirs = list(dirs)
    if not dirs:
        return []
    try:
        from ._guatiao import scan
    except ImportError:
        _log.warning(
            "%s is set but guatiao is not installed: pip install coagent-m[guatiao]",
            PLUGIN_PATH_ENV,
        )
        return []
    return scan(dirs)


def default_registry() -> Registry:
    """Built-ins, then entry points, then guatiao plugins. A later provider whose
    name is taken is logged and skipped."""
    registry = Registry()
    for provider in [*builtin_providers(), *entry_point_providers(), *plugin_providers()]:
        try:
            registry.add(provider)
        except ValueError as exc:
            _log.warning("%s", exc)
    return registry


def _reason(provider: Provider) -> Optional[str]:
    try:
        return provider.available()
    except Exception as exc:  # a provider that cannot answer cannot run
        return f"{type(exc).__name__}: {exc}"


def _entry_points(group: str) -> list:
    eps = metadata.entry_points()
    if hasattr(eps, "select"):
        return list(eps.select(group=group))
    return list(eps.get(group, ()))  # Python 3.9: a dict of groups
