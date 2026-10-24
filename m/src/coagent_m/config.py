"""Where coagent's configuration lives: one global directory, and a ``.coagent``
directory in any project, read only when the project is trusted."""

from __future__ import annotations

import enum
import logging
import os
import sys
from pathlib import Path
from typing import Any, Dict, Optional

if sys.version_info >= (3, 11):
    import tomllib
else:  # pragma: no cover - exercised on the 3.9/3.10 floor
    import tomli as tomllib

__all__ = [
    "CONFIG_FILE",
    "HOME_ENV",
    "PROJECT_DIR",
    "TRUST_ENV",
    "Scope",
    "config_dir",
    "find_project",
    "project_trusted",
    "read_toml",
]

HOME_ENV = "COAGENT_HOME"
TRUST_ENV = "COAGENT_TRUST_PROJECT"
PROJECT_DIR = ".coagent"
CONFIG_FILE = "config.toml"

_log = logging.getLogger(__name__)


class Scope(str, enum.Enum):
    """Where something was declared. Broader scopes are read first, so a narrower one can
    override them."""

    GLOBAL = "global"
    PROJECT = "project"
    AGENT = "agent"


def config_dir() -> Path:
    """``$COAGENT_HOME``, else ``%APPDATA%\\coagent`` on Windows, else
    ``$XDG_CONFIG_HOME/coagent`` (``~/.config/coagent``)."""
    home = os.environ.get(HOME_ENV)
    if home:
        return Path(home)
    if sys.platform == "win32" and os.environ.get("APPDATA"):
        return Path(os.environ["APPDATA"]) / "coagent"
    base = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
    return Path(base) / "coagent"


def find_project(cwd: Optional[str]) -> Optional[Path]:
    """The nearest directory at or above ``cwd`` holding a ``.coagent`` directory."""
    here = Path(cwd or os.getcwd()).resolve()
    for d in (here, *here.parents):
        if (d / PROJECT_DIR).is_dir():
            return d
    return None


def project_trusted(root: Path) -> bool:
    """Whether a project's ``.coagent`` files are read. They start programs inside the
    supervisor, which holds the credentials, so a project is trusted only when listed
    under ``[trust] projects`` in the global ``config.toml``, or when
    ``COAGENT_TRUST_PROJECT=1``."""
    if os.environ.get(TRUST_ENV, "").lower() in ("1", "true", "yes"):
        return True
    data = read_toml(config_dir() / CONFIG_FILE)
    trusted = (data or {}).get("trust", {}).get("projects", [])
    root = root.resolve()
    return any(Path(os.path.expanduser(str(p))).resolve() == root for p in trusted)


def read_toml(path: Path) -> Optional[Dict[str, Any]]:
    """The file's tables, or ``None`` when it is missing or unreadable (logged)."""
    if not path.is_file():
        return None
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        _log.warning("cannot read %s: %s", path, exc)
        return None
