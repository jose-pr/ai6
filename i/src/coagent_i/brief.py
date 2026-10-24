"""Mission briefs: the one format every intake source produces and M consumes."""

from __future__ import annotations

import json
import re
import uuid
from dataclasses import asdict, dataclass, field, fields
from datetime import datetime, timezone
from typing import Any, Dict, Mapping, Optional, Tuple

__all__ = ["BRIEF_VERSION", "Brief", "BriefError"]

BRIEF_VERSION = 1
_SAFE = re.compile(r"[^A-Za-z0-9_.-]+")


class BriefError(Exception):
    """A brief is malformed."""


@dataclass(frozen=True)
class Brief:
    """A mission: what to do, for which station, by what kind of agent.

    ``id`` is unique within its ``source`` (a ticket key is a good one); ``title`` and
    ``body`` become the agent's prompt. ``station`` is an id, name or alias; ``profile``
    names an agent outright, else M picks the best one for ``role`` and ``tags``.
    ``data`` carries whatever else a source wants back when it hears the outcome.
    """

    title: str
    body: str = ""
    id: str = field(default_factory=lambda: uuid.uuid4().hex[:12])
    source: str = "manual"
    station: Optional[str] = None
    profile: Optional[str] = None
    role: Optional[str] = None
    tags: Tuple[str, ...] = ()
    priority: int = 0
    url: Optional[str] = None
    data: Mapping[str, Any] = field(default_factory=dict)
    created: str = field(
        default_factory=lambda: datetime.now(timezone.utc).isoformat(timespec="seconds")
    )
    version: int = BRIEF_VERSION

    @property
    def key(self) -> str:
        """``source/id``, filesystem-safe: unique across every source."""
        return f"{_SAFE.sub('_', self.source)}--{_SAFE.sub('_', self.id)}"

    @property
    def prompt(self) -> str:
        """What the agent is told: the title, then the body, then the link."""
        parts = [self.title.strip(), self.body.strip()]
        if self.url:
            parts.append(f"Source: {self.url}")
        return "\n\n".join(p for p in parts if p)

    def to_dict(self) -> Dict[str, Any]:
        out = asdict(self)
        out["tags"] = list(self.tags)
        out["data"] = dict(self.data)
        return out

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), indent=2, sort_keys=True)

    @classmethod
    def from_dict(cls, data: Mapping[str, Any]) -> "Brief":
        """Raises ``BriefError`` for a missing title, an unknown key or a newer version,
        so a producer's typo is an error rather than a silently ignored field."""
        if not isinstance(data, Mapping):
            raise BriefError("a brief is a JSON object")
        unknown = sorted(set(data) - _FIELDS)
        if unknown:
            raise BriefError(f"unknown brief keys {unknown}; put extras under 'data'")
        if not str(data.get("title") or "").strip():
            raise BriefError("a brief needs a title")
        version = int(data.get("version", BRIEF_VERSION))
        if version > BRIEF_VERSION:
            raise BriefError(f"brief version {version} is newer than this reader ({BRIEF_VERSION})")
        values = dict(data)
        values["tags"] = tuple(str(t) for t in values.get("tags", ()))
        for key in ("id", "source", "title", "body"):
            if key in values:
                values[key] = str(values[key])
        if "priority" in values:
            values["priority"] = int(values["priority"])
        return cls(**values)

    @classmethod
    def from_json(cls, text: str) -> "Brief":
        try:
            return cls.from_dict(json.loads(text))
        except ValueError as exc:
            raise BriefError(f"not JSON: {exc}") from exc


_FIELDS = {f.name for f in fields(Brief)}
