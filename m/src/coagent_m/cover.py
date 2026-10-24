"""Cover: how openly an agent's work may say an agent did it (Universal Exports).

A station has a cover identity (the author its commits carry) and a cover policy. The
policy says, field by field, what may appear in the work an agent leaves behind: who
the author is, which commit trailers, whether the model may be named, a footer. M tells
the agent its cover, sets git's identity for it, and checks every commit it made: a
commit that breaks cover is reported as blown.

Built in: ``covert`` (nothing says an agent did it), ``partial`` (a generic
``Assisted-by: AI`` trailer, never the model or the agent) and ``overt`` (full
disclosure). ``covers.toml`` in the global config directory defines more::

    [cover.disclosed]
    agent_author = false                 # the agent may not be the author or committer
    trailers = ["Assisted-by", "Reviewed-by"]
    suggest = "Assisted-by: AI ({profile})"
    model = false                        # the model's name may not appear
    footer = ""                          # text every commit message must end with
    forbid = ['claude\\.ai/code/session']   # never allowed anywhere in a message
"""

from __future__ import annotations

import re
from dataclasses import dataclass, fields
from pathlib import Path
from typing import Any, Dict, List, Mapping, Optional, Sequence, Tuple

from .config import config_dir, read_toml

__all__ = [
    "BUILTIN_COVERS",
    "COVERS_FILE",
    "Cover",
    "CoverError",
    "Covers",
    "Identity",
    "Violation",
    "check_commits",
]

COVERS_FILE = "covers.toml"

# Trailer keys that say who or what wrote the work. Any other trailer (Fixes, Refs,
# Signed-off-by) is about the change, not its author, and is never checked.
_ATTRIBUTION = re.compile(
    r"^(co-authored-by|generated-by|assisted-by|ai-assisted|written-by|[a-z0-9-]+-session)$",
    re.IGNORECASE,
)
_TRAILER = re.compile(r"^([A-Za-z][A-Za-z0-9-]*):\s*(.*)$")
# Phrases that announce generated work, checked under every cover except overt.
_ANNOUNCE = re.compile(r"generated (with|by) |🤖", re.IGNORECASE)


class CoverError(Exception):
    """A cover is unknown or its declaration does not fit."""


@dataclass(frozen=True)
class Identity:
    """The author an agent's commits carry."""

    name: str
    email: str

    def git_env(self) -> Dict[str, str]:
        return {
            "GIT_AUTHOR_NAME": self.name,
            "GIT_AUTHOR_EMAIL": self.email,
            "GIT_COMMITTER_NAME": self.name,
            "GIT_COMMITTER_EMAIL": self.email,
        }


@dataclass(frozen=True)
class Cover:
    """What an agent's work may say about who did it."""

    name: str
    description: str = ""
    # The agent may author or commit under its own name, not only the cover identity.
    agent_author: bool = False
    # Attribution trailers allowed; "*" allows any.
    trailers: Tuple[str, ...] = ()
    # A trailer the agent is asked to add ({profile} and {model} are filled in).
    suggest: str = ""
    # The model's name may appear in commit messages.
    model: bool = False
    # Text every commit message must end with; empty for none.
    footer: str = ""
    # Patterns never allowed anywhere in a commit message.
    forbid: Tuple[str, ...] = ()
    # Phrases such as "generated with" are refused unless the cover allows them.
    announce: bool = False

    def briefing(self, identity: Optional[Identity], profile: str, model: str) -> str:
        """What the agent is told about its cover."""
        lines = [f"Cover: {self.name}. {self.description}".rstrip()]
        if identity is not None:
            lines.append(f"Commit as {identity.name} <{identity.email}>; git is set up for it.")
        if not self.trailers:
            lines.append("Add no Co-Authored-By or other attribution trailer to commits.")
        elif "*" not in self.trailers:
            lines.append(f"The only attribution trailers allowed: {', '.join(self.trailers)}.")
        if self.suggest:
            lines.append(
                f"End each commit message with: {self._fill(self.suggest, profile, model)}"
            )
        if not self.model:
            lines.append("Do not name the model you run on in commits, comments or pull requests.")
        if not self.announce:
            lines.append('Do not write "generated with" or similar notices.')
        if self.footer:
            lines.append(f"Every commit message ends with: {self.footer}")
        return "\n".join(lines)

    @staticmethod
    def _fill(text: str, profile: str, model: str) -> str:
        return text.replace("{profile}", profile).replace("{model}", model)


BUILTIN_COVERS: Dict[str, Cover] = {
    "covert": Cover(
        "covert",
        "Nothing in the work says an agent did it.",
        forbid=(r"claude\.ai/code/session",),
    ),
    "partial": Cover(
        "partial",
        "The work may say AI helped, but never which model or agent.",
        trailers=("Assisted-by",),
        suggest="Assisted-by: AI",
        forbid=(r"claude\.ai/code/session",),
    ),
    "overt": Cover(
        "overt",
        "Full disclosure: the agent and its model may be named.",
        agent_author=True,
        trailers=("*",),
        model=True,
        announce=True,
    ),
}


@dataclass(frozen=True)
class Violation:
    """One way one commit broke cover."""

    commit: str
    rule: str
    detail: str

    def as_dict(self) -> Dict[str, str]:
        return {"commit": self.commit, "rule": self.rule, "detail": self.detail}


def check_commits(
    commits: Sequence[Mapping[str, str]],
    cover: Cover,
    identity: Optional[Identity] = None,
    *,
    model: str = "",
) -> List[Violation]:
    """Every way ``commits`` (``sha``, ``author``, ``committer``, ``message``; author and
    committer as ``Name <email>``) break ``cover``."""
    found: List[Violation] = []
    forbid = [re.compile(p, re.IGNORECASE) for p in cover.forbid]
    for c in commits:
        sha, message = c["sha"][:12], c.get("message", "")
        if identity is not None and not cover.agent_author:
            expected = f"{identity.name} <{identity.email}>"
            for role in ("author", "committer"):
                if c.get(role) and c[role] != expected:
                    found.append(Violation(sha, role, f"{c[role]}, not {expected}"))
        for key, value in _trailers(message):
            if _ATTRIBUTION.match(key) and "*" not in cover.trailers:
                if key.lower() not in {t.lower() for t in cover.trailers}:
                    found.append(Violation(sha, "trailer", f"{key}: {value}"))
        if model and not cover.model and model.lower() in message.lower():
            found.append(Violation(sha, "model", f"names the model {model}"))
        if not cover.announce and _ANNOUNCE.search(message):
            found.append(Violation(sha, "announce", _ANNOUNCE.search(message).group(0).strip()))
        for pattern in forbid:
            match = pattern.search(message)
            if match:
                found.append(Violation(sha, "forbid", match.group(0)))
        if cover.footer and not message.rstrip().endswith(cover.footer.rstrip()):
            found.append(Violation(sha, "footer", f"does not end with {cover.footer!r}"))
    return found


def _trailers(message: str) -> List[Tuple[str, str]]:
    """The ``Key: value`` lines of a message's last paragraph."""
    paragraphs = [p for p in message.strip().split("\n\n") if p.strip()]
    if len(paragraphs) < 2:
        return []
    out = []
    for line in paragraphs[-1].splitlines():
        match = _TRAILER.match(line.strip())
        if match:
            out.append((match.group(1), match.group(2)))
    return out


class Covers:
    """The built-in covers plus those in ``covers.toml``, which may replace them."""

    def __init__(self, covers: Optional[Mapping[str, Cover]] = None) -> None:
        self._by_name: Dict[str, Cover] = dict(BUILTIN_COVERS)
        self._by_name.update(covers or {})

    def get(self, name: str) -> Cover:
        try:
            return self._by_name[name]
        except KeyError:
            known = ", ".join(sorted(self._by_name))
            raise CoverError(f"no cover {name!r} (covers: {known})") from None

    def names(self) -> List[str]:
        return sorted(self._by_name)

    @classmethod
    def load(cls, path: Optional[Path] = None) -> "Covers":
        path = path or config_dir() / COVERS_FILE
        found: Dict[str, Cover] = {}
        for name, table in ((read_toml(path) or {}).get("cover", {})).items():
            found[str(name)] = _cover(str(name), table, path)
        return cls(found)


_KEYS = {f.name for f in fields(Cover)} - {"name"}


def _cover(name: str, table: Mapping[str, Any], source: Path) -> Cover:
    unknown = sorted(set(table) - _KEYS)
    if unknown:
        raise CoverError(f"{source}: cover {name!r}: unknown keys {unknown}")
    values: Dict[str, Any] = dict(table)
    for key in ("trailers", "forbid"):
        if key in values:
            values[key] = tuple(str(v) for v in values[key])
    for pattern in values.get("forbid", ()):
        try:
            re.compile(pattern)
        except re.error as exc:
            raise CoverError(f"{source}: cover {name!r}: bad pattern {pattern!r}: {exc}") from None
    return Cover(name=name, **values)
