"""Every inbox backend the suite runs against: files, SQLite, and PostgreSQL when
``COAGENT_TEST_POSTGRES`` names a database (its tables are emptied first)."""

from __future__ import annotations

import os
from pathlib import Path

from coagent_i import Inbox

BACKENDS = ["file", "sqlite"] + (["postgres"] if os.environ.get("COAGENT_TEST_POSTGRES") else [])


def make_inbox(kind: str, tmp_path: Path):
    if kind == "file":
        return Inbox(tmp_path / "inbox")
    from coagent_i.sql import SqlInbox

    if kind == "sqlite":
        return SqlInbox(f"sqlite:///{tmp_path / 'inbox.db'}")
    box = SqlInbox(os.environ["COAGENT_TEST_POSTGRES"])
    from coagent_i import sql

    with box._engine.begin() as db:
        for table in reversed(sql._meta.sorted_tables):
            db.execute(table.delete())
    return box
