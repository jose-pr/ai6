"""Hooks: what runs around an agent's session, declared globally, per project or per agent.

Before an agent starts, each hook's ``prepare`` may add environment, add context the
agent receives, or rewrite its command line; ``on_event`` sees every state transition
and ``finished`` the outcome. A hook is a Python object (an entry point in the
``coagent.hooks`` group, or ``python = "module:attr"`` in a ``hooks.toml``) or a program
in any language (``command = [...]`` in a ``hooks.toml``).
"""

from __future__ import annotations

from ..config import Scope
from ._base import Hook, HookBase, HookContext, HookError, Preparation
from ._command import CommandHook, parse_output
from ._loader import (
    ENTRY_POINT_GROUP,
    HOOKS_FILE,
    HookSet,
    entry_point_hooks,
    load_file,
    load_hooks,
)

__all__ = [
    "ENTRY_POINT_GROUP",
    "HOOKS_FILE",
    "CommandHook",
    "Hook",
    "HookBase",
    "HookContext",
    "HookError",
    "HookSet",
    "Preparation",
    "Scope",
    "entry_point_hooks",
    "load_file",
    "load_hooks",
    "parse_output",
]
