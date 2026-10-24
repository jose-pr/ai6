"""I Branch: intelligence and communications for C.O.A.G.E.N.T.

Mission briefs (``Brief``), the registry anything can deliver them to (``Registry``), and the
contract user-supplied intake sources fill (``sources``). Planned here as well: the wire
format M, Moneypenny and agents talk in, briefings, and debriefs.
"""

from __future__ import annotations

from .brief import BRIEF_VERSION, Brief, BriefError
from .registry import (
    DEFAULT_LEASE,
    REGISTRY_ENV,
    STATES,
    Registry,
    Lease,
    LeaseLost,
    default_registry_dir,
)
from .sources import CommandSource, Source, SourceError, load_sources, poll, report


def open_registry(location: "str | None" = None):
    """The registry at ``location`` (default ``COAGENT_REGISTRY``): a SQLAlchemy URL
    (``sqlite:///…``, ``postgresql+psycopg://…``) opens ``SqlRegistry``, anything else is a
    directory for ``Registry``."""
    import os

    location = location or os.environ.get(REGISTRY_ENV) or None
    if location and "://" in str(location):
        from .sql import SqlRegistry

        return SqlRegistry(str(location))
    return Registry(location)


__version__ = "0.1.0.dev0"

__all__ = [
    "BRIEF_VERSION",
    "DEFAULT_LEASE",
    "REGISTRY_ENV",
    "STATES",
    "Brief",
    "BriefError",
    "CommandSource",
    "Registry",
    "Lease",
    "LeaseLost",
    "Source",
    "SourceError",
    "default_registry_dir",
    "open_registry",
    "load_sources",
    "poll",
    "report",
]
