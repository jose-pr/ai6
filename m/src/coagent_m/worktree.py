"""A git worktree per mission, so agents working in parallel never share a checkout.

A worktree is cut from the station's checkout onto a new branch and lives under M's state
directory (or a host's ``work_root``), never inside the checkout itself. Git runs here
(``LocalGit``) or on the station's host over ssh (``SshGit``). When the mission ends it is removed, with
its branch, if the agent left nothing behind; otherwise it is kept as the mission's work.
"""

from __future__ import annotations

import os
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path, PurePath, PurePosixPath
from typing import TYPE_CHECKING, List, Optional, Protocol, Union

if TYPE_CHECKING:
    from .hosts import Host

__all__ = [
    "LOCAL",
    "STATE_ENV",
    "Git",
    "LocalGit",
    "SshGit",
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


class Git(Protocol):
    """Where git runs: ``run`` answers stdout and raises ``WorktreeError`` on failure
    (unless ``check`` is false); paths are on that machine."""

    def run(self, cwd: str, *args: str, check: bool = True) -> str: ...

    def exists(self, path: str) -> bool: ...

    def mkdirs(self, path: str) -> None: ...


class LocalGit:
    """git on this machine."""

    def run(self, cwd: str, *args: str, check: bool = True) -> str:
        try:
            done = subprocess.run(
                ["git", *args], cwd=cwd, capture_output=True, text=True, timeout=120
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise WorktreeError(f"git {args[0]}: {exc}") from exc
        if check and done.returncode != 0:
            raise WorktreeError(f"git {' '.join(args)}: {done.stderr.strip()}")
        return done.stdout

    def exists(self, path: str) -> bool:
        return os.path.exists(path)

    def mkdirs(self, path: str) -> None:
        os.makedirs(path, exist_ok=True)


LOCAL = LocalGit()


@dataclass(frozen=True)
class SshGit:
    """git on a POSIX host, over ssh."""

    host: "Host"
    timeout: float = 120.0

    def run(self, cwd: str, *args: str, check: bool = True) -> str:
        done = self._sh(" ".join(shlex.quote(a) for a in ["git", "-C", cwd, *args]))
        if check and done.returncode != 0:
            raise WorktreeError(f"{self.host.id}: git {' '.join(args)}: {done.stderr.strip()}")
        return done.stdout

    def exists(self, path: str) -> bool:
        return self._sh(f"test -e {shlex.quote(path)}").returncode == 0

    def mkdirs(self, path: str) -> None:
        done = self._sh(f"mkdir -p {shlex.quote(path)}")
        if done.returncode != 0:
            raise WorktreeError(f"{self.host.id}: mkdir {path}: {done.stderr.strip()}")

    def _sh(self, command: str) -> "subprocess.CompletedProcess[str]":
        if not self.host.posix:
            raise WorktreeError(
                f"{self.host.id}: worktrees on a Windows host are not supported yet"
            )
        try:
            return subprocess.run(
                [*self.host.ssh_argv(tty=False), command],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                timeout=self.timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise WorktreeError(f"{self.host.id}: ssh: {exc}") from exc


def is_git_checkout(path: "str | Path", git: Git = LOCAL) -> bool:
    out = git.run(str(path), "rev-parse", "--is-inside-work-tree", check=False)
    return out.strip() == "true"


@dataclass(frozen=True)
class Worktree:
    """One mission's checkout: ``path`` on ``branch``, cut from ``base`` (a commit), on
    the machine ``git`` runs on."""

    repo: PurePath
    path: PurePath
    branch: str
    base: str
    git: Git = field(default=LOCAL, compare=False, repr=False)

    @classmethod
    def create(
        cls,
        repo: "str | Path",
        key: str,
        branch: str,
        *,
        root: "Optional[Union[Path, PurePosixPath]]" = None,
        base: str = "HEAD",
        git: Git = LOCAL,
    ) -> "Worktree":
        """Adds a worktree for mission ``key`` at ``<root>/<key>`` on a new ``branch`` from
        ``base``. ``root`` defaults to ``state_dir()/worktrees`` for local git and is
        required for any other. Raises ``WorktreeError`` when the branch or path already
        exists, or ``repo`` is not a git checkout."""
        local = isinstance(git, LocalGit)
        if root is None:
            if not local:
                raise WorktreeError("a remote worktree needs a root (the host's work_root)")
            root = state_dir() / "worktrees"
        repo = Path(repo).resolve() if local else PurePosixPath(str(repo))
        if not is_git_checkout(repo, git):
            raise WorktreeError(f"{repo} is not a git checkout")
        path = root / key
        if git.exists(str(path)):
            raise WorktreeError(f"{path} already exists")
        commit = git.run(str(repo), "rev-parse", "--verify", f"{base}^{{commit}}").strip()
        git.mkdirs(str(path.parent))
        git.run(str(repo), "worktree", "add", "-b", branch, str(path), commit)
        return cls(
            repo, Path(path).resolve() if local else PurePosixPath(str(path)), branch, commit, git
        )

    def commits(self) -> List[str]:
        """Commits on the branch since ``base``, newest first, as ``<sha> <subject>``."""
        out = self.git.run(str(self.path), "log", "--format=%h %s", f"{self.base}..HEAD")
        return [line for line in out.splitlines() if line]

    def dirty(self) -> bool:
        """Uncommitted changes, untracked files included (ignored files are not)."""
        return bool(self.git.run(str(self.path), "status", "--porcelain").strip())

    def exists(self) -> bool:
        return self.git.exists(str(self.path))

    def cleanup(self, *, keep: bool = False) -> bool:
        """Removes the worktree and its branch when the agent left nothing: no commits
        and no changes. Answers whether it is kept."""
        if keep or not self.exists() or self.commits() or self.dirty():
            return self.exists()
        self.git.run(str(self.repo), "worktree", "remove", "--force", str(self.path))
        self.git.run(str(self.repo), "branch", "-D", self.branch, check=False)
        return False

    def describe(self) -> dict:
        """What a report says about it."""
        kept = self.exists()
        return {
            "path": str(self.path),
            "branch": self.branch,
            "base": self.base,
            "kept": kept,
            "commits": self.commits() if kept else [],
        }
