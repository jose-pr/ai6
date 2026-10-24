"""Every registry backend the suite runs against: files, SQLite, and PostgreSQL when
``COAGENT_TEST_POSTGRES`` names a database (its tables are emptied first)."""

from __future__ import annotations

import os
from pathlib import Path

from coagent_i import Registry

BACKENDS = ["file", "sqlite"] + (["postgres"] if os.environ.get("COAGENT_TEST_POSTGRES") else [])


def make_registry(kind: str, tmp_path: Path):
    if kind == "file":
        return Registry(tmp_path / "registry")
    from coagent_i.sql import SqlRegistry

    if kind == "sqlite":
        return SqlRegistry(f"sqlite:///{tmp_path / 'registry.db'}")
    box = SqlRegistry(os.environ["COAGENT_TEST_POSTGRES"])
    from coagent_i import sql

    with box._engine.begin() as db:
        for table in reversed(sql._meta.sorted_tables):
            db.execute(table.delete())
    return box
