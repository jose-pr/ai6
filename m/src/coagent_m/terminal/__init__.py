"""Terminal providers: where an agent's terminal comes from.

M asks a ``Registry`` for a ``Provider``, and the provider starts a ``Session``. Built in:
``posix-pty`` (Linux, macOS, BSD), ``conpty`` (Windows; scaffold) and ``pipes`` (any OS,
no terminal); each theater in ``theaters.toml`` adds ``ssh:<id>``. More arrive through the ``coagent.terminals`` entry-point group, or as
guatiao libraries serving the ``coagent_terminal`` kind in any language.
"""

from __future__ import annotations

from ._base import (
    DEFAULT_COLS,
    DEFAULT_ROWS,
    Exit,
    Provider,
    ProviderInfo,
    ProviderUnavailable,
    Session,
    TerminalError,
    TerminalSpec,
)
from ._registry import (
    ENTRY_POINT_GROUP,
    PLUGIN_PATH_ENV,
    Registry,
    builtin_providers,
    default_registry,
    entry_point_providers,
    theater_providers,
    plugin_providers,
)

__all__ = [
    "DEFAULT_COLS",
    "DEFAULT_ROWS",
    "ENTRY_POINT_GROUP",
    "PLUGIN_PATH_ENV",
    "Exit",
    "Provider",
    "ProviderInfo",
    "ProviderUnavailable",
    "Registry",
    "Session",
    "TerminalError",
    "TerminalSpec",
    "builtin_providers",
    "default_registry",
    "entry_point_providers",
    "theater_providers",
    "plugin_providers",
]
