"""Safe houses: a git worktree per mission, so agents working in parallel never share a
checkout.

A safe house is cut from the station's checkout onto a new branch and lives under M's state
directory (or a theater's ``safe_houses``), never inside the checkout itself. Git runs here
(``LocalGit``) or on the station's theater over ssh (``SshGit``). When the mission ends it is removed, with
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
from typing import TYPE_CHECKING, Dict, List, Optional, Protocol, Union

if TYPE_CHECKING:
    from .theaters import Theater

__all__ = [
    "LOCAL",
    "STATE_ENV",
    "Git",
    "LocalGit",
    "SshGit",
    "SafeHouse",
    "SafeHouseError",
    "branch_name",
    "commit_log",
    "head",
    "is_git_checkout",
    "state_dir",
]

STATE_ENV = "COAGENT_STATE"
DEFAULT_BRANCH = "{id}-{slug}"
_SLUG = re.compile(r"[^a-z0-9]+")
# git check-ref-format: no "..", no leading "-" or ".", no control chars, ":", "?", "*", "[", "\".
_UNSAFE_REF = re.compile(r"[^A-Za-z0-9._/-]+")


class SafeHouseError(Exception):
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
    """Where git runs: ``run`` answers stdout and raises ``SafeHouseError`` on failure
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
            raise SafeHouseError(f"git {args[0]}: {exc}") from exc
        if check and done.returncode != 0:
            raise SafeHouseError(f"git {' '.join(args)}: {done.stderr.strip()}")
        return done.stdout

    def exists(self, path: str) -> bool:
        return os.path.exists(path)

    def mkdirs(self, path: str) -> None:
        os.makedirs(path, exist_ok=True)


LOCAL = LocalGit()


@dataclass(frozen=True)
class SshGit:
    """git on a POSIX theater, over ssh."""

    theater: "Theater"
    timeout: float = 120.0

    def run(self, cwd: str, *args: str, check: bool = True) -> str:
        done = self._sh(" ".join(shlex.quote(a) for a in ["git", "-C", cwd, *args]))
        if check and done.returncode != 0:
            raise SafeHouseError(f"{self.theater.id}: git {' '.join(args)}: {done.stderr.strip()}")
        return done.stdout

    def exists(self, path: str) -> bool:
        return self._sh(f"test -e {shlex.quote(path)}").returncode == 0

    def mkdirs(self, path: str) -> None:
        done = self._sh(f"mkdir -p {shlex.quote(path)}")
        if done.returncode != 0:
            raise SafeHouseError(f"{self.theater.id}: mkdir {path}: {done.stderr.strip()}")

    def _sh(self, command: str) -> "subprocess.CompletedProcess[str]":
        if not self.theater.posix:
            raise SafeHouseError(
                f"{self.theater.id}: worktrees on a Windows theater are not supported yet"
            )
        try:
            return subprocess.run(
                [*self.theater.ssh_argv(tty=False), command],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                timeout=self.timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise SafeHouseError(f"{self.theater.id}: ssh: {exc}") from exc


def head(path: "str | PurePath", git: Git = LOCAL) -> Optional[str]:
    """The commit checked out at ``path``, or ``None`` when it is not a git checkout."""
    out = git.run(str(path), "rev-parse", "--verify", "HEAD^{commit}", check=False).strip()
    return out or None


def commit_log(git: Git, path: str, base: str) -> List[Dict[str, str]]:
    """Commits in ``base..HEAD`` at ``path``, as ``commits`` dicts for ``cover``."""
    fmt = "%H%x1f%an <%ae>%x1f%cn <%ce>%x1f%B%x1e"
    out = git.run(path, "log", f"--format={fmt}", f"{base}..HEAD")
    commits = []
    for record in out.split("\x1e"):
        parts = record.strip("\n").split("\x1f")
        if len(parts) == 4:
            sha, author, committer, message = parts
            commits.append(
                {"sha": sha, "author": author, "committer": committer, "message": message}
            )
    return commits


def is_git_checkout(path: "str | Path", git: Git = LOCAL) -> bool:
    out = git.run(str(path), "rev-parse", "--is-inside-work-tree", check=False)
    return out.strip() == "true"


@dataclass(frozen=True)
class SafeHouse:
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
    ) -> "SafeHouse":
        """Adds a worktree for mission ``key`` at ``<root>/<key>`` on a new ``branch`` from
        ``base``. ``root`` defaults to ``state_dir()/safe-houses`` for local git and is
        required for any other. Raises ``SafeHouseError`` when the branch or path already
        exists, or ``repo`` is not a git checkout."""
        local = isinstance(git, LocalGit)
        if root is None:
            if not local:
                raise SafeHouseError("a remote safe house needs a root (the theater's safe_houses)")
            root = state_dir() / "safe-houses"
        repo = Path(repo).resolve() if local else PurePosixPath(str(repo))
        if not is_git_checkout(repo, git):
            raise SafeHouseError(f"{repo} is not a git checkout")
        path = root / key
        if git.exists(str(path)):
            raise SafeHouseError(f"{path} already exists")
        commit = git.run(str(repo), "rev-parse", "--verify", f"{base}^{{commit}}").strip()
        git.mkdirs(str(path.parent))
        git.run(str(repo), "worktree", "add", "-b", branch, str(path), commit)
        return cls(
            repo, Path(path).resolve() if local else PurePosixPath(str(path)), branch, commit, git
        )

    def log(self) -> List[Dict[str, str]]:
        """Each commit since ``base``, newest first: ``sha``, ``author``, ``committer``
        (as ``Name <email>``) and the whole ``message``."""
        return commit_log(self.git, str(self.path), self.base)

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
