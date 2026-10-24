"""The inbox: a directory anything can drop a brief into, and M takes them from.

``new/`` holds briefs waiting, ``claimed/`` those an agent is on, ``done/`` and
``failed/`` the finished ones, each beside its ``.outcome.json`` and, when the mission
produced one, its ``.log`` transcript.

Several supervisors (offices) may share one inbox, on one machine or a shared directory.
An office claims with a **lease** it keeps renewing; when an office is lost its leases expire,
and any other office ``recover``s those briefs: back to ``new/`` for another attempt, or to
``failed/`` once ``max_attempts`` are spent. Lease expiry compares wall clocks, so the
offices' clocks must agree to within a small part of the lease. Every move is a rename
within one directory tree, so two supervisors never claim the same brief.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Dict, Iterator, List, Mapping, Optional, Tuple

from .brief import Brief, BriefError

__all__ = [
    "DEFAULT_LEASE",
    "INBOX_ENV",
    "STATES",
    "Inbox",
    "Lease",
    "LeaseLost",
    "default_inbox_dir",
]

INBOX_ENV = "COAGENT_INBOX"
STATES = ("new", "claimed", "done", "failed")
DEFAULT_LEASE = 60.0


class LeaseLost(Exception):
    """The brief's lease belongs to another office now (this one was presumed lost)."""


@dataclass(frozen=True)
class Lease:
    """An office's hold on a claimed brief: until ``expires`` (seconds since the epoch)."""

    key: str
    office: str
    expires: float
    attempt: int
    # What the claimer recorded about the assignment (station, profile), for others.
    meta: Mapping[str, Any] = field(default_factory=dict, compare=False)

    def to_json(self) -> str:
        data = {"office": self.office, "expires": self.expires, "attempt": self.attempt}
        return json.dumps({**data, "meta": dict(self.meta)}, indent=2)


def default_inbox_dir() -> Path:
    """``$COAGENT_INBOX``, else ``$COAGENT_HOME/inbox``, else the platform's state
    directory: ``%LOCALAPPDATA%\\coagent\\inbox`` or ``$XDG_STATE_HOME/coagent/inbox``
    (``~/.local/state/coagent/inbox``)."""
    if os.environ.get(INBOX_ENV):
        return Path(os.environ[INBOX_ENV])
    if os.environ.get("COAGENT_HOME"):
        return Path(os.environ["COAGENT_HOME"]) / "inbox"
    if sys.platform == "win32" and os.environ.get("LOCALAPPDATA"):
        return Path(os.environ["LOCALAPPDATA"]) / "coagent" / "inbox"
    base = os.environ.get("XDG_STATE_HOME") or os.path.join(
        os.path.expanduser("~"), ".local", "state"
    )
    return Path(base) / "coagent" / "inbox"


class Inbox:
    """Briefs on disk, by state."""

    def __init__(self, root: Optional["str | Path"] = None) -> None:
        self.root = Path(root) if root is not None else default_inbox_dir()

    def submit(self, brief: Brief, *, replace: bool = False) -> Path:
        """Files ``brief`` under ``new/``. A brief whose key is already in any state is
        refused (``BriefError``) unless ``replace``, which only replaces one still new.
        """
        existing = self.find(brief.key)
        if existing is not None:
            state, path = existing
            if not (replace and state == "new"):
                raise BriefError(f"brief {brief.key} is already {state}")
        target = self._dir("new") / f"{brief.key}.json"
        _write_atomic(target, brief.to_json())
        return target

    def find(self, key: str) -> Optional[Tuple[str, Path]]:
        for state in STATES:
            path = self.root / state / f"{key}.json"
            if path.is_file():
                return state, path
        return None

    def list(self, state: str = "new") -> List[Brief]:
        """Briefs in ``state``, highest priority first, then oldest. A file that does not
        parse is skipped."""
        briefs = list(self._read(state))
        briefs.sort(key=lambda b: (-b.priority, b.created, b.key))
        return briefs

    def claim(
        self,
        key: Optional[str] = None,
        *,
        office: Optional[str] = None,
        lease: float = DEFAULT_LEASE,
        meta: Optional[Mapping[str, Any]] = None,
    ) -> Optional[Brief]:
        """Moves the best new brief (or the one keyed ``key``) to ``claimed/`` and answers
        it; ``None`` when there is none. Another claimer winning the race is skipped.
        With ``office``, the claim holds a lease for ``lease`` seconds (see ``renew``)."""
        candidates = [b for b in self.list("new") if key is None or b.key == key]
        for brief in candidates:
            try:
                os.replace(
                    self.root / "new" / f"{brief.key}.json",
                    self._dir("claimed") / f"{brief.key}.json",
                )
            except FileNotFoundError:
                continue
            if office is not None:
                attempt = self.attempts(brief.key) + 1
                _write_atomic(self._dir("attempts") / f"{brief.key}.json", str(attempt))
                self._write_lease(
                    Lease(brief.key, office, time.time() + lease, attempt, dict(meta or {}))
                )
            return brief
        return None

    def read(self, key: str) -> Optional[Brief]:
        """The brief keyed ``key``, in whatever state it is, or ``None``."""
        found = self.find(key)
        if found is None:
            return None
        try:
            return Brief.from_json(found[1].read_text(encoding="utf-8"))
        except (OSError, BriefError):
            return None

    def lease(self, key: str) -> Optional[Lease]:
        """The lease on a claimed brief, or ``None`` (not claimed, or claimed without one)."""
        path = self.root / "leases" / f"{key}.json"
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None
        return Lease(
            key,
            str(data["office"]),
            float(data["expires"]),
            int(data["attempt"]),
            dict(data.get("meta") or {}),
        )

    def attempts(self, key: str) -> int:
        """How many times the brief has been claimed with a lease."""
        try:
            return int((self.root / "attempts" / f"{key}.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return 0

    def renew(self, key: str, office: str, lease: float = DEFAULT_LEASE) -> bool:
        """Extends ``office``'s lease; ``False`` when the lease is no longer its own, which
        means another office recovered the brief and this one must stop working on it."""
        current = self.lease(key)
        if current is None or current.office != office:
            return False
        if not (self.root / "claimed" / f"{key}.json").is_file():
            return False
        self._write_lease(replace(current, expires=time.time() + lease))
        return True

    def leases(self) -> List[Lease]:
        """Every lease on a claimed brief, live or expired."""
        directory = self.root / "claimed"
        found = []
        for path in sorted(directory.glob("*.json")) if directory.is_dir() else ():
            lease = self.lease(path.stem)
            if lease is not None:
                found.append(lease)
        return found

    def heartbeat(self, office: str, info: Mapping[str, Any]) -> None:
        """Announces ``office`` as alive now, with whatever it says about itself."""
        record = {**dict(info), "office": office, "seen": time.time()}
        _write_atomic(self._dir("offices") / f"{_safe(office)}.json", json.dumps(record, indent=2))

    def offices(self, within: float = DEFAULT_LEASE) -> List[Dict[str, Any]]:
        """Nodes whose last heartbeat is no older than ``within`` seconds."""
        directory = self.root / "offices"
        now, found = time.time(), []
        for path in sorted(directory.glob("*.json")) if directory.is_dir() else ():
            try:
                record = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                continue
            if now - float(record.get("seen", 0)) <= within:
                found.append(record)
        return found

    def elect(self, office: str, ttl: float = DEFAULT_LEASE) -> bool:
        """Holds, renews or takes the leader lease for ``office``; answers whether ``office``
        is the leader now. Taking an expired lease is a rename only one office can win,
        then an exclusive create."""
        path = self._dir("office") / "leader.json"
        current = self.leader()
        now = time.time()
        if current is not None and current["office"] == office:
            _write_atomic(path, json.dumps({"office": office, "expires": now + ttl}))
            return True
        if current is not None and float(current["expires"]) >= now:
            return False
        if current is not None:
            try:
                os.replace(path, path.with_name(f"leader.lapsed.{_safe(office)}"))
            except FileNotFoundError:
                return False
        fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".", suffix=".tmp")
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(json.dumps({"office": office, "expires": now + ttl}))
        try:
            os.link(tmp, path)  # fails if another office created it first
        except FileExistsError:
            return False
        finally:
            os.unlink(tmp)
        return True

    def leader(self) -> Optional[Dict[str, Any]]:
        """The leader lease (``office``, ``expires``), live or lapsed, or ``None``."""
        try:
            return json.loads((self.root / "office" / "leader.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None

    def expired(self, now: Optional[float] = None) -> List[Lease]:
        """Leases on claimed briefs that ran out: their office stopped renewing them."""
        now = time.time() if now is None else now
        found = []
        directory = self.root / "claimed"
        for path in sorted(directory.glob("*.json")) if directory.is_dir() else ():
            lease = self.lease(path.stem)
            if lease is not None and lease.expires < now:
                found.append(lease)
        return found

    def recover(self, lease: Lease, *, max_attempts: int = 3, by: str = "") -> Optional[str]:
        """Takes back a brief whose office was lost: to ``new/`` (answers ``"new"``) while it
        has attempts left, else to ``failed/`` with a ``lost`` outcome (``"failed"``).
        ``None`` when another office got there first, or the lease was renewed meanwhile.
        """
        current = self.lease(lease.key)
        if current is None or current != lease or current.expires >= time.time():
            return None
        source = self.root / "claimed" / f"{lease.key}.json"
        if lease.attempt < max_attempts:
            try:
                os.replace(source, self._dir("new") / f"{lease.key}.json")
            except FileNotFoundError:
                return None
            self._drop_lease(lease.key)
            return "new"
        try:
            brief = Brief.from_json(source.read_text(encoding="utf-8"))
        except (OSError, BriefError):
            return None
        outcome = {
            "state": "lost",
            "reason": f"office {lease.office} was lost; {lease.attempt} attempts spent",
            "recovered_by": by,
        }
        self.finish(brief, outcome, ok=False)
        return "failed"

    def finish(
        self,
        brief: Brief,
        outcome: Mapping[str, Any],
        *,
        ok: bool,
        transcript: Optional[bytes] = None,
        office: Optional[str] = None,
    ) -> Path:
        """Moves a claimed brief to ``done/`` or ``failed/`` with its outcome, and its
        transcript when given, beside it. With ``office``, raises ``LeaseLost`` (filing
        nothing) when the lease is no longer that office's."""
        if office is not None:
            current = self.lease(brief.key)
            claimed = (self.root / "claimed" / f"{brief.key}.json").is_file()
            if current is None or current.office != office or not claimed:
                raise LeaseLost(f"{brief.key} is no longer held by {office}")
        state = "done" if ok else "failed"
        target = self._dir(state) / f"{brief.key}.json"
        _write_atomic(target.with_suffix(".outcome.json"), json.dumps(dict(outcome), indent=2))
        if transcript is not None:
            target.with_suffix(".log").write_bytes(transcript)
        source = self.root / "claimed" / f"{brief.key}.json"
        if source.is_file():
            os.replace(source, target)
        else:
            _write_atomic(target, brief.to_json())
        self._drop_lease(brief.key)
        return target

    def _write_lease(self, lease: Lease) -> None:
        _write_atomic(self._dir("leases") / f"{lease.key}.json", lease.to_json())

    def _drop_lease(self, key: str) -> None:
        try:
            (self.root / "leases" / f"{key}.json").unlink()
        except FileNotFoundError:
            pass

    def transcript(self, key: str) -> Optional[bytes]:
        for state in ("done", "failed"):
            path = self.root / state / f"{key}.log"
            if path.is_file():
                return path.read_bytes()
        return None

    def outcome(self, key: str) -> Optional[Dict[str, Any]]:
        for state in ("done", "failed"):
            path = self.root / state / f"{key}.outcome.json"
            if path.is_file():
                return json.loads(path.read_text(encoding="utf-8"))
        return None

    def _read(self, state: str) -> Iterator[Brief]:
        directory = self.root / state
        if not directory.is_dir():
            return
        for path in sorted(directory.glob("*.json")):
            if path.name.endswith(".outcome.json"):
                continue
            try:
                yield Brief.from_json(path.read_text(encoding="utf-8"))
            except (OSError, BriefError):
                continue

    def _dir(self, state: str) -> Path:
        path = self.root / state
        path.mkdir(parents=True, exist_ok=True)
        return path


def _safe(name: str) -> str:
    return "".join(c if c.isalnum() or c in "._-" else "_" for c in name)


def _write_atomic(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as f:
            f.write(text + "\n")
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
