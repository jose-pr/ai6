"""The registry in a SQL database, for offices on different hosts: SQLite on one machine,
PostgreSQL (or any database SQLAlchemy supports) across many.

Same contract as ``coagent_i.registry.Registry``. Every step two offices could race on is one
conditional ``UPDATE`` (or ``DELETE``) whose row count says who won: claiming a brief,
renewing a lease, recovering it, finishing it, taking the leader lease. That holds on
SQLite and PostgreSQL alike, with no table locks. Needs the ``sql`` extra (SQLAlchemy);
PostgreSQL also needs a driver (the ``postgres`` extra installs psycopg).
"""

from __future__ import annotations

import json
import time
from typing import Any, Dict, List, Mapping, Optional, Tuple

from sqlalchemy import (
    Column,
    Float,
    Integer,
    LargeBinary,
    MetaData,
    String,
    Table,
    Text,
    and_,
    create_engine,
    delete,
    event,
    exists,
    insert,
    or_,
    select,
    update,
)
from sqlalchemy.exc import IntegrityError

from .brief import Brief, BriefError
from .registry import DEFAULT_LEASE, STATES, Lease, LeaseLost

__all__ = ["SqlRegistry"]

_meta = MetaData()
_briefs = Table(
    "coagent_briefs",
    _meta,
    Column("key", String(512), primary_key=True),
    Column("state", String(16), nullable=False, index=True),
    Column("priority", Integer, nullable=False, default=0),
    Column("created", String(64), nullable=False),
    Column("brief", Text, nullable=False),
    Column("attempts", Integer, nullable=False, default=0),
    Column("outcome", Text),
    Column("transcript", LargeBinary),
)
_leases = Table(
    "coagent_leases",
    _meta,
    Column("key", String(512), primary_key=True),
    Column("office", String(256), nullable=False),
    Column("expires", Float, nullable=False),
    Column("attempt", Integer, nullable=False),
    Column("meta", Text, nullable=False, default="{}"),
)
_offices = Table(
    "coagent_offices",
    _meta,
    Column("office", String(256), primary_key=True),
    Column("seen", Float, nullable=False),
    Column("record", Text, nullable=False),
)
_leader = Table(
    "coagent_leader",
    _meta,
    Column("id", Integer, primary_key=True),
    Column("office", String(256), nullable=False),
    Column("expires", Float, nullable=False),
)


class SqlRegistry:
    """An registry at a SQLAlchemy URL (``sqlite:///path/registry.db``,
    ``postgresql+psycopg://user@host/db``). Tables are created on first use."""

    def __init__(self, url: str, **engine_options: Any) -> None:
        self.url = url
        if url.startswith("sqlite"):
            # Offices in threads share connections; waiting beats failing on a busy file.
            engine_options.setdefault("connect_args", {"check_same_thread": False, "timeout": 30})
        self._engine = create_engine(url, **engine_options)
        if url.startswith("sqlite"):

            @event.listens_for(self._engine, "connect")
            def _wal(dbapi_connection: Any, _: Any) -> None:
                cursor = dbapi_connection.cursor()
                cursor.execute("PRAGMA journal_mode=WAL")
                cursor.close()

        _meta.create_all(self._engine)

    @property
    def root(self) -> str:
        return self.url

    def submit(self, brief: Brief, *, replace: bool = False) -> str:
        row = dict(
            key=brief.key,
            state="new",
            priority=brief.priority,
            created=brief.created,
            brief=brief.to_json(),
            attempts=0,
        )
        with self._engine.begin() as db:
            current = db.execute(select(_briefs.c.state).where(_briefs.c.key == brief.key)).first()
            if current is not None:
                if not (replace and current.state == "new"):
                    raise BriefError(f"brief {brief.key} is already {current.state}")
                db.execute(
                    update(_briefs)
                    .where(_briefs.c.key == brief.key, _briefs.c.state == "new")
                    .values(priority=brief.priority, created=brief.created, brief=brief.to_json())
                )
                return brief.key
            try:
                db.execute(insert(_briefs).values(**row))
            except IntegrityError as exc:
                raise BriefError(f"brief {brief.key} was filed meanwhile") from exc
        return brief.key

    def find(self, key: str) -> Optional[Tuple[str, str]]:
        with self._engine.connect() as db:
            row = db.execute(select(_briefs.c.state).where(_briefs.c.key == key)).first()
        return (row.state, key) if row is not None else None

    def list(self, state: str = "new") -> List[Brief]:
        if state not in STATES:
            raise ValueError(f"state is one of {STATES}")
        query = (
            select(_briefs.c.brief)
            .where(_briefs.c.state == state)
            .order_by(_briefs.c.priority.desc(), _briefs.c.created, _briefs.c.key)
        )
        with self._engine.connect() as db:
            rows = db.execute(query).all()
        out = []
        for row in rows:
            try:
                out.append(Brief.from_json(row.brief))
            except BriefError:
                continue
        return out

    def claim(
        self,
        key: Optional[str] = None,
        *,
        office: Optional[str] = None,
        lease: float = DEFAULT_LEASE,
        meta: Optional[Mapping[str, Any]] = None,
    ) -> Optional[Brief]:
        for brief in self.list("new"):
            if key is not None and brief.key != key:
                continue
            with self._engine.begin() as db:
                won = db.execute(
                    update(_briefs)
                    .where(_briefs.c.key == brief.key, _briefs.c.state == "new")
                    .values(
                        state="claimed",
                        attempts=_briefs.c.attempts + (1 if office is not None else 0),
                    )
                ).rowcount
                if won != 1:
                    continue
                if office is not None:
                    attempt = db.execute(
                        select(_briefs.c.attempts).where(_briefs.c.key == brief.key)
                    ).scalar_one()
                    db.execute(delete(_leases).where(_leases.c.key == brief.key))
                    db.execute(
                        insert(_leases).values(
                            key=brief.key,
                            office=office,
                            expires=time.time() + lease,
                            attempt=attempt,
                            meta=json.dumps(dict(meta or {})),
                        )
                    )
            return brief
        return None

    def read(self, key: str) -> Optional[Brief]:
        with self._engine.connect() as db:
            row = db.execute(select(_briefs.c.brief).where(_briefs.c.key == key)).first()
        if row is None:
            return None
        try:
            return Brief.from_json(row.brief)
        except BriefError:
            return None

    def lease(self, key: str) -> Optional[Lease]:
        with self._engine.connect() as db:
            row = db.execute(select(_leases).where(_leases.c.key == key)).first()
        return _lease(row) if row is not None else None

    def attempts(self, key: str) -> int:
        with self._engine.connect() as db:
            n = db.execute(select(_briefs.c.attempts).where(_briefs.c.key == key)).scalar()
        return int(n or 0)

    def renew(self, key: str, office: str, lease: float = DEFAULT_LEASE) -> bool:
        claimed = exists().where(_briefs.c.key == key, _briefs.c.state == "claimed")
        with self._engine.begin() as db:
            won = db.execute(
                update(_leases)
                .where(_leases.c.key == key, _leases.c.office == office, claimed)
                .values(expires=time.time() + lease)
            ).rowcount
        return won == 1

    def leases(self) -> List[Lease]:
        query = (
            select(_leases)
            .join(_briefs, _briefs.c.key == _leases.c.key)
            .where(_briefs.c.state == "claimed")
            .order_by(_leases.c.key)
        )
        with self._engine.connect() as db:
            return [_lease(row) for row in db.execute(query).all()]

    def expired(self, now: Optional[float] = None) -> List[Lease]:
        now = time.time() if now is None else now
        return [lease for lease in self.leases() if lease.expires < now]

    def recover(self, lease: Lease, *, max_attempts: int = 3, by: str = "") -> Optional[str]:
        with self._engine.begin() as db:
            # Deleting exactly the lease that was seen, and only while lapsed, is the claim.
            won = db.execute(
                delete(_leases).where(
                    _leases.c.key == lease.key,
                    _leases.c.office == lease.office,
                    _leases.c.expires == lease.expires,
                    _leases.c.expires < time.time(),
                )
            ).rowcount
            if won != 1:
                return None
            if lease.attempt < max_attempts:
                db.execute(
                    update(_briefs)
                    .where(_briefs.c.key == lease.key, _briefs.c.state == "claimed")
                    .values(state="new")
                )
                return "new"
            outcome = {
                "state": "lost",
                "reason": f"office {lease.office} was lost; {lease.attempt} attempts spent",
                "recovered_by": by,
            }
            db.execute(
                update(_briefs)
                .where(_briefs.c.key == lease.key, _briefs.c.state == "claimed")
                .values(state="failed", outcome=json.dumps(outcome))
            )
            return "failed"

    def finish(
        self,
        brief: Brief,
        outcome: Mapping[str, Any],
        *,
        ok: bool,
        transcript: Optional[bytes] = None,
        office: Optional[str] = None,
    ) -> str:
        state = "done" if ok else "failed"
        values: Dict[str, Any] = {"state": state, "outcome": json.dumps(dict(outcome))}
        if transcript is not None:
            values["transcript"] = transcript
        where = [_briefs.c.key == brief.key]
        if office is not None:
            held = exists().where(_leases.c.key == brief.key, _leases.c.office == office)
            where += [_briefs.c.state == "claimed", held]
        with self._engine.begin() as db:
            done = db.execute(update(_briefs).where(and_(*where)).values(**values)).rowcount
            if done != 1:
                if office is not None:
                    raise LeaseLost(f"{brief.key} is no longer held by {office}")
                row = dict(
                    key=brief.key,
                    priority=brief.priority,
                    created=brief.created,
                    brief=brief.to_json(),
                    attempts=0,
                    **values,
                )
                db.execute(insert(_briefs).values(**row))
            db.execute(delete(_leases).where(_leases.c.key == brief.key))
        return brief.key

    def outcome(self, key: str) -> Optional[Dict[str, Any]]:
        with self._engine.connect() as db:
            row = db.execute(select(_briefs.c.outcome).where(_briefs.c.key == key)).first()
        return json.loads(row.outcome) if row is not None and row.outcome else None

    def transcript(self, key: str) -> Optional[bytes]:
        with self._engine.connect() as db:
            row = db.execute(select(_briefs.c.transcript).where(_briefs.c.key == key)).first()
        return bytes(row.transcript) if row is not None and row.transcript is not None else None

    def heartbeat(self, office: str, info: Mapping[str, Any]) -> None:
        now = time.time()
        record = json.dumps({**dict(info), "office": office, "seen": now})
        with self._engine.begin() as db:
            n = db.execute(
                update(_offices).where(_offices.c.office == office).values(seen=now, record=record)
            ).rowcount
            if n == 0:
                try:
                    with db.begin_nested():
                        db.execute(insert(_offices).values(office=office, seen=now, record=record))
                except IntegrityError:
                    db.execute(
                        update(_offices)
                        .where(_offices.c.office == office)
                        .values(seen=now, record=record)
                    )

    def offices(self, within: float = DEFAULT_LEASE) -> List[Dict[str, Any]]:
        since = time.time() - within
        with self._engine.connect() as db:
            rows = db.execute(
                select(_offices.c.record)
                .where(_offices.c.seen >= since)
                .order_by(_offices.c.office)
            ).all()
        return [json.loads(row.record) for row in rows]

    def elect(self, office: str, ttl: float = DEFAULT_LEASE) -> bool:
        now = time.time()
        with self._engine.begin() as db:
            won = db.execute(
                update(_leader)
                .where(_leader.c.id == 1, or_(_leader.c.office == office, _leader.c.expires < now))
                .values(office=office, expires=now + ttl)
            ).rowcount
            if won == 1:
                return True
            if db.execute(select(_leader.c.id).where(_leader.c.id == 1)).first() is not None:
                return False
            try:
                with db.begin_nested():
                    db.execute(insert(_leader).values(id=1, office=office, expires=now + ttl))
            except IntegrityError:
                return False
        return True

    def leader(self) -> Optional[Dict[str, Any]]:
        with self._engine.connect() as db:
            row = db.execute(select(_leader).where(_leader.c.id == 1)).first()
        return {"office": row.office, "expires": row.expires} if row is not None else None

    def dispose(self) -> None:
        """Closes the connection pool."""
        self._engine.dispose()


def _lease(row: Any) -> Lease:
    return Lease(row.key, row.office, float(row.expires), int(row.attempt), json.loads(row.meta))
