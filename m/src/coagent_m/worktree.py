"""A git worktree per mission, so agents working in parallel never share a checkout.

A worktree is cut from the station's checkout onto a new branch and lives under M's state
directory, never inside the checkout itself. When the mission ends it is removed, with
its branch, if the agent left nothing behind; otherwise it is kept as the mission's work.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

__all__ = [
    "STATE_ENV",
    "Worktree",
    "WorktreeError",
    "branch_name",
    "is_git_checkout",
    "state_dir",
]

STATE_ENV = "COAGENT_STATE"
DEFAULT_BRANCH = "{id}-{slug}"
_SLUG = re.compile(r"[^a-z0-9]+")
# git check-ref-format: no "..", no leading "-" or ".", no control chars, ":", "?", "*", "[", "\".
_UNSAFE_REF = re.compile(r"[^A-Za-z0-9._/-]+")


class WorktreeError(Exception):
    """git refused to create or remove a worktree; the message carries git's stderr."""


def state_dir() -> Path:
    """``$COAGENT_STATE``, else ``$COAGENT_HOME/state``, else ``%LOCALAPPDATA%\\coagent``
    or ``$XDG_STATE_HOME/coagent`` (``~/.local/state/coagent``)."""
    if os.environ.get(STATE_ENV):
        return Path(os.environ[STATE_ENV])
    if os.environ.get("COAGENT_HOME"):
        return Path(os.environ["COAGENT_HOME"]) / "state"
    if sys.platform == "win32" and os.environ.get("LOCALAPPDATA"):
        return Path(os.environ["LOCALAPPDATA"]) / "coagent"
    base = os.environ.get("XDG_STATE_HOME") or os.path.join(
        os.path.expanduser("~"), ".local", "state"
    )
    return Path(base) / "coagent"


def branch_name(template: str, *, id: str, title: str, station: str = "") -> str:
    """``template`` with ``{id}``, ``{slug}`` (the title, lowercase, dashed, at most 40
    characters) and ``{station}`` filled, made safe as a git ref."""
    slug = _SLUG.sub("-", title.lower()).strip("-")[:40].rstrip("-") or "mission"
    name = template.format(id=id, slug=slug, station=station)
    name = _UNSAFE_REF.sub("-", name).replace("..", ".").strip("-./")
    return name or "mission"


def is_git_checkout(path: Path) -> bool:
    return _git(path, "rev-parse", "--is-inside-work-tree", check=False).strip() == "true"


@dataclass(frozen=True)
class Worktree:
    """One mission's checkout: ``path`` on ``branch``, cut from ``base`` (a commit)."""

    repo: Path
    path: Path
    branch: str
    base: str

    @classmethod
    def create(
        cls,
        repo: "str | Path",
        key: str,
        branch: str,
        *,
        root: Optional[Path] = None,
        base: str = "HEAD",
    ) -> "Worktree":
        """Adds a worktree for mission ``key`` at ``<root>/<key>`` on a new ``branch`` from
        ``base``. ``root`` defaults to ``state_dir()/worktrees``. Raises ``WorktreeError``
        when the branch or path already exists, or ``repo`` is not a git checkout."""
        repo = Path(repo).resolve()
        if not is_git_checkout(repo):
            raise WorktreeError(f"{repo} is not a git checkout")
        path = (root or state_dir() / "worktrees") / key
        if path.exists():
            raise WorktreeError(f"{path} already exists")
        commit = _git(repo, "rev-parse", "--verify", f"{base}^{{commit}}").strip()
        path.parent.mkdir(parents=True, exist_ok=True)
        _git(repo, "worktree", "add", "-b", branch, str(path), commit)
        return cls(repo, path.resolve(), branch, commit)

    def commits(self) -> List[str]:
        """Commits on the branch since ``base``, newest first, as ``<sha> <subject>``."""
        out = _git(self.path, "log", "--format=%h %s", f"{self.base}..HEAD")
        return [line for line in out.splitlines() if line]

    def dirty(self) -> bool:
        """Uncommitted changes, untracked files included (ignored files are not)."""
        return bool(_git(self.path, "status", "--porcelain").strip())

    def cleanup(self, *, keep: bool = False) -> bool:
        """Removes the worktree and its branch when the agent left nothing: no commits
        and no changes. Answers whether it is kept."""
        if keep or not self.path.exists() or self.commits() or self.dirty():
            return self.path.exists()
        _git(self.repo, "worktree", "remove", "--force", str(self.path))
        _git(self.repo, "branch", "-D", self.branch, check=False)
        return False

    def describe(self) -> dict:
        """What a report says about it."""
        kept = self.path.exists()
        return {
            "path": str(self.path),
            "branch": self.branch,
            "base": self.base,
            "kept": kept,
            "commits": self.commits() if kept else [],
        }


def _git(cwd: Path, *args: str, check: bool = True) -> str:
    try:
        done = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise WorktreeError(f"git {args[0]}: {exc}") from exc
    if check and done.returncode != 0:
        raise WorktreeError(f"git {' '.join(args)}: {done.stderr.strip()}")
    return done.stdout
