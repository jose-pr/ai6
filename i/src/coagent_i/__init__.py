"""I Branch: intelligence and communications for C.O.A.G.E.N.T.

Mission briefs (``Brief``), the inbox anything can deliver them to (``Inbox``), and the
contract user-supplied intake sources fill (``sources``). Planned here as well: the wire
format M, Moneypenny and agents talk in, briefings, and debriefs.
"""

from __future__ import annotations

from .brief import BRIEF_VERSION, Brief, BriefError
from .inbox import DEFAULT_LEASE, INBOX_ENV, STATES, Inbox, Lease, LeaseLost, default_inbox_dir
from .sources import CommandSource, Source, SourceError, load_sources, poll, report


def open_inbox(location: "str | None" = None):
    """The inbox at ``location`` (default ``COAGENT_INBOX``): a SQLAlchemy URL
    (``sqlite:///…``, ``postgresql+psycopg://…``) opens ``SqlInbox``, anything else is a
    directory for ``Inbox``."""
    import os

    location = location or os.environ.get(INBOX_ENV) or None
    if location and "://" in str(location):
        from .sql import SqlInbox

        return SqlInbox(str(location))
    return Inbox(location)


__version__ = "0.1.0.dev0"

__all__ = [
    "BRIEF_VERSION",
    "DEFAULT_LEASE",
    "INBOX_ENV",
    "STATES",
    "Brief",
    "BriefError",
    "CommandSource",
    "Inbox",
    "Lease",
    "LeaseLost",
    "Source",
    "SourceError",
    "default_inbox_dir",
    "open_inbox",
    "load_sources",
    "poll",
    "report",
]
