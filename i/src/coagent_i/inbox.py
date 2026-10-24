"""The inbox: a directory anything can drop a brief into, and M takes them from.

``new/`` holds briefs waiting, ``claimed/`` those an agent is on, ``done/`` and
``failed/`` the finished ones, each beside its ``.outcome.json`` and, when the mission
produced one, its ``.log`` transcript. Every move is a rename
within one directory tree, so two supervisors never claim the same brief.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, Iterator, List, Mapping, Optional, Tuple

from .brief import Brief, BriefError

__all__ = ["INBOX_ENV", "STATES", "Inbox", "default_inbox_dir"]

INBOX_ENV = "COAGENT_INBOX"
STATES = ("new", "claimed", "done", "failed")


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
        refused (``BriefError``) unless ``replace``, which only replaces one still new."""
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

    def claim(self, key: Optional[str] = None) -> Optional[Brief]:
        """Moves the best new brief (or the one keyed ``key``) to ``claimed/`` and answers
        it; ``None`` when there is none. Another claimer winning the race is skipped."""
        candidates = [b for b in self.list("new") if key is None or b.key == key]
        for brief in candidates:
            try:
                os.replace(
                    self.root / "new" / f"{brief.key}.json",
                    self._dir("claimed") / f"{brief.key}.json",
                )
            except FileNotFoundError:
                continue
            return brief
        return None

    def finish(
        self,
        brief: Brief,
        outcome: Mapping[str, Any],
        *,
        ok: bool,
        transcript: Optional[bytes] = None,
    ) -> Path:
        """Moves a claimed brief to ``done/`` or ``failed/`` with its outcome, and its
        transcript when given, beside it."""
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
        return target

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
