"""Agent profiles: the model configurations M assigns to tasks.

A **launcher** says how to start one agent CLI: a command template and how the task's
prompt reaches it. A **profile** names a configuration of a launcher (the model, its
effort, environment, extra arguments, terminal, limits, context) under a codename, and
says what work it suits (``roles``, ``tags``). M asks for a profile by name or alias, or
for the best one for a role, and turns it and a prompt into an ``AgentSpec``.

``profiles.toml``, read globally and from a trusted project's ``.coagent`` directory::

    [launcher.cli]
    command = ["agent-cli", "--model", "{model}", "--effort", "{effort}"]
    prompt = "arg"                # "arg" appends it, "stdin" types it, "none" drops it

    [profile.007]
    aliases = ["JamesBond"]
    launcher = "cli"
    model = "<model-id>"
    effort = "high"
    roles = ["plan", "review/security"]
    priority = 10

    [profile.q-branch]
    extends = "007"
    effort = "medium"
    roles = ["implement"]
"""

from __future__ import annotations

import re
from importlib import metadata
from dataclasses import dataclass, field, fields, replace
from pathlib import Path
from typing import Any, Dict, Iterable, List, Mapping, Optional, Protocol, Sequence, Tuple

from .config import Scope, config_dir, read_toml
from .stations import Station, Stations, clearance_rank, local_config

__all__ = [
    "ADAPTER_GROUP",
    "PROFILES_FILE",
    "Adapter",
    "LaunchRequest",
    "load_adapter",
    "register_adapter",
    "Launcher",
    "Profile",
    "ProfileError",
    "ProfileSet",
    "load_profiles",
]

PROFILES_FILE = "profiles.toml"
_PROMPT_MODES = ("arg", "stdin", "none")
_PLACEHOLDER = re.compile(r"\{([A-Za-z_][A-Za-z0-9_]*)\}")


class ProfileError(Exception):
    """A profile or launcher is missing, malformed or cannot be resolved."""


@dataclass(frozen=True)
class Launcher:
    """How to start one agent CLI: a ``command`` template, or an ``adapter`` (a 00 Section
    package that knows the CLI) with its ``options``. ``{name}`` placeholders in
    ``command`` are filled from the profile: ``model``, ``effort``, ``profile``,
    ``prompt``, ``cwd`` and its ``vars``."""

    name: str
    command: Tuple[str, ...] = ()
    prompt: str = "arg"
    env: Mapping[str, str] = field(default_factory=dict)
    scope: Scope = Scope.GLOBAL
    adapter: Optional[str] = None
    options: Mapping[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class LaunchRequest:
    """What an adapter is asked to launch. ``cover`` is the station's
    ``coagent_m.cover.Cover``, when there is a station."""

    profile: "Profile"
    launcher: Launcher
    prompt: Optional[str]
    cwd: Optional[str]
    station: Optional[Station] = None
    cover: Optional[Any] = None

    @property
    def clearance(self) -> str:
        return self.profile.clearance or "field"

    @property
    def options(self) -> Dict[str, Any]:
        """The launcher's options, under the profile's ``vars``."""
        return {**self.launcher.options, **self.profile.vars}


class Adapter(Protocol):
    """Drives one agent CLI. ``launch`` answers the command line, the environment to add
    and any text to type once it starts; the optional ``debrief(transcript: bytes)``
    answers a mapping for the mission's report, or ``None``."""

    name: str

    def launch(self, request: LaunchRequest) -> Tuple[List[str], Dict[str, str], Optional[str]]: ...


ADAPTER_GROUP = "coagent.adapters"
_adapters: Dict[str, Any] = {}


def load_adapter(name: str) -> Any:
    """The adapter ``name`` from the ``coagent.adapters`` entry-point group: an object,
    or a class built with no arguments. Raises ``ProfileError`` when none is installed."""
    if name in _adapters:
        return _adapters[name]
    eps = metadata.entry_points()
    group = (
        eps.select(group=ADAPTER_GROUP) if hasattr(eps, "select") else eps.get(ADAPTER_GROUP, ())
    )
    for ep in group:
        if ep.name == name:
            obj = ep.load()
            _adapters[name] = obj() if isinstance(obj, type) else obj
            return _adapters[name]
    raise ProfileError(
        f"no adapter {name!r} is installed (entry-point group {ADAPTER_GROUP!r}; "
        "for the 00 Section's, pip install -e 00/)"
    )


def register_adapter(adapter: Any) -> None:
    """Makes ``adapter`` loadable by its ``name`` without an entry point (tests, embedding)."""
    _adapters[adapter.name] = adapter


@dataclass(frozen=True)
class Profile:
    """A model configuration under a codename. Unset fields inherit from ``extends``."""

    name: str
    aliases: Tuple[str, ...] = ()
    description: str = ""
    extends: Optional[str] = None
    launcher: Optional[str] = None
    model: Optional[str] = None
    effort: Optional[str] = None
    roles: Tuple[str, ...] = ()
    tags: Tuple[str, ...] = ()
    priority: int = 0
    env: Mapping[str, str] = field(default_factory=dict)
    args: Tuple[str, ...] = ()
    vars: Mapping[str, str] = field(default_factory=dict)
    terminal: Optional[str] = None
    limits: Mapping[str, Any] = field(default_factory=dict)
    context: str = ""
    # How many agents of this profile may run at once; None is no limit.
    max_concurrent: Optional[int] = None
    # One of stations.CLEARANCES; unset is "field". A station refuses a profile cleared
    # above it.
    clearance: Optional[str] = None
    scope: Scope = Scope.GLOBAL

    def suits(self, role: Optional[str] = None, tags: Iterable[str] = ()) -> bool:
        """Whether it lists ``role`` (or a role it is a sub-role of: ``review`` suits
        ``review/security``) and every one of ``tags``."""
        if role is not None and not any(
            role == r or role.startswith(r.rstrip("/") + "/") for r in self.roles
        ):
            return False
        return set(tags) <= set(self.tags)


class ProfileSet:
    """Launchers and profiles, by name. A later ``add`` of a name replaces the earlier one,
    which is how a project overrides a global profile."""

    def __init__(self) -> None:
        self.launchers: Dict[str, Launcher] = {}
        self._profiles: Dict[str, Profile] = {}

    def add_launcher(self, launcher: Launcher) -> None:
        self.launchers[launcher.name] = launcher

    def add(self, profile: Profile) -> None:
        self._profiles[profile.name] = profile

    def names(self) -> List[str]:
        return sorted(self._profiles)

    def get(self, name: str) -> Profile:
        """The profile called ``name``, or with it as an alias, with ``extends`` applied."""
        return self._resolve(self._find(name).name, ())

    def profiles(self) -> List[Profile]:
        """Every profile, resolved, best first: priority descending, then name."""
        resolved = [self.get(n) for n in self._profiles]
        return sorted(resolved, key=lambda p: (-p.priority, p.name))

    def choose(
        self,
        role: Optional[str] = None,
        tags: Iterable[str] = (),
        station: Optional[Station] = None,
    ) -> Profile:
        """The best profile that suits ``role`` and ``tags`` and, given a ``station``, may
        run there."""
        tags = tuple(tags)
        for profile in self.profiles():
            if profile.suits(role, tags) and (station is None or not station.why_not(profile)):
                return profile
        want = ", ".join(filter(None, [f"role {role!r}" if role else "", *tags])) or "anything"
        where = f" at {station.label}" if station is not None else ""
        raise ProfileError(f"no profile suits {want}{where}")

    def launcher_of(self, profile: Profile) -> Launcher:
        if not profile.launcher:
            raise ProfileError(f"profile {profile.name!r} names no launcher")
        launcher = self.launchers.get(profile.launcher)
        if launcher is None:
            raise ProfileError(
                f"profile {profile.name!r} names launcher {profile.launcher!r}, which is not defined"
            )
        return launcher

    def argv(
        self,
        profile: Profile,
        prompt: Optional[str] = None,
        cwd: Optional[str] = None,
        *,
        station: Optional[Station] = None,
        cover: Optional[Any] = None,
    ):
        """The command line, the environment additions and the text to type once it
        starts, for ``profile`` given ``prompt``. An adapter launcher asks its adapter;
        the profile's ``args`` and ``env`` are added either way."""
        launcher = self.launcher_of(profile)
        if launcher.adapter:
            adapter = load_adapter(launcher.adapter)
            request = LaunchRequest(profile, launcher, prompt, cwd, station, cover)
            argv, env, typed = adapter.launch(request)
            values = {**{k: str(v) for k, v in profile.vars.items()}, "profile": profile.name}
            values.update(model=profile.model or "", effort=profile.effort or "")
            values.update(prompt=prompt or "", cwd=cwd or "")
            argv = [*argv, *(_fill(part, values, profile.name) for part in profile.args)]
            return argv, {**launcher.env, **env, **profile.env}, typed
        values = {
            **{k: str(v) for k, v in profile.vars.items()},
            "profile": profile.name,
            "model": profile.model or "",
            "effort": profile.effort or "",
            "prompt": prompt or "",
            "cwd": cwd or "",
        }
        templated_prompt = any("{prompt}" in part for part in launcher.command)
        argv = [_fill(part, values, profile.name) for part in launcher.command]
        argv += [_fill(part, values, profile.name) for part in profile.args]
        typed: Optional[str] = None
        if prompt and not templated_prompt:
            if launcher.prompt == "arg":
                argv.append(prompt)
            elif launcher.prompt == "stdin":
                typed = prompt
        env = {**launcher.env, **profile.env}
        return argv, env, typed

    def _find(self, name: str) -> Profile:
        if name in self._profiles:
            return self._profiles[name]
        for profile in self._profiles.values():
            if name in profile.aliases:
                return profile
        known = ", ".join(self.names()) or "none"
        raise ProfileError(f"no profile or alias {name!r} (profiles: {known})")

    def _resolve(self, name: str, seen: Tuple[str, ...]) -> Profile:
        if name in seen:
            raise ProfileError(
                "profiles extend each other in a cycle: " + " -> ".join(seen + (name,))
            )
        profile = self._find(name)
        if not profile.extends:
            return profile
        return _merge(self._resolve(profile.extends, seen + (name,)), profile)


def load_profiles(
    cwd: Optional[str] = None,
    *,
    trust_project: Optional[bool] = None,
    stations: Optional[Stations] = None,
    station: Optional[Station] = None,
) -> ProfileSet:
    """The global ``profiles.toml``, then that of the station holding ``cwd``, whose
    profiles override global ones of the same name (``load_hooks`` explains
    ``trust_project``)."""
    found = ProfileSet()
    load_file(config_dir() / PROFILES_FILE, Scope.GLOBAL, found)
    local, _ = local_config(cwd, stations, trust=trust_project, station=station)
    if local is not None:
        load_file(local / PROFILES_FILE, Scope.STATION, found)
    return found


def load_file(path: Path, scope: Scope, into: ProfileSet) -> None:
    """Adds the launchers and profiles of one file. Raises ``ProfileError`` for a table
    that does not fit, naming it, so a typo is not silently a missing profile."""
    data = read_toml(path) or {}
    for name, table in data.get("launcher", {}).items():
        into.add_launcher(_launcher(name, table, scope, path))
    for name, table in data.get("profile", {}).items():
        into.add(_profile(name, table, scope, path))


def _launcher(name: str, table: Mapping[str, Any], scope: Scope, path: Path) -> Launcher:
    _known(table, {"command", "prompt", "env", "adapter", "options"}, f"{path}: launcher {name!r}")
    command = table.get("command", [])
    adapter = table.get("adapter")
    if not adapter and (not isinstance(command, list) or not command):
        raise ProfileError(f'{path}: launcher {name!r} needs command = [...] or adapter = ".."')
    prompt = str(table.get("prompt", "arg"))
    if prompt not in _PROMPT_MODES:
        raise ProfileError(f"{path}: launcher {name!r}: prompt must be one of {_PROMPT_MODES}")
    env = {str(k): str(v) for k, v in table.get("env", {}).items()}
    return Launcher(
        name,
        tuple(str(c) for c in command),
        prompt,
        env,
        scope,
        str(adapter) if adapter else None,
        dict(table.get("options", {})),
    )


_TUPLES = {"aliases", "roles", "tags", "args"}
_MAPS = {"env", "vars", "limits"}


def _profile(name: str, table: Mapping[str, Any], scope: Scope, path: Path) -> Profile:
    allowed = {f.name for f in fields(Profile)} - {"name", "scope"}
    _known(table, allowed, f"{path}: profile {name!r}")
    values: Dict[str, Any] = {"name": name, "scope": scope}
    if "clearance" in table:
        try:
            clearance_rank(str(table["clearance"]))
        except Exception as exc:
            raise ProfileError(f"{path}: profile {name!r}: {exc}") from None
    for key, value in table.items():
        if key in _TUPLES:
            values[key] = tuple(str(v) for v in value)
        elif key in _MAPS:
            values[key] = (
                dict(value) if key == "limits" else {str(k): str(v) for k, v in value.items()}
            )
        else:
            values[key] = value
    return Profile(**values)


def _known(table: Mapping[str, Any], allowed: set, where: str) -> None:
    unknown = sorted(set(table) - allowed)
    if unknown:
        raise ProfileError(f"{where}: unknown keys {unknown}")


def _merge(base: Profile, child: Profile) -> Profile:
    """``child`` over ``base``: a field the child sets wins, maps merge, aliases stay the
    child's own."""
    changes: Dict[str, Any] = {}
    for f in fields(Profile):
        if f.name in ("name", "aliases", "extends", "scope"):
            continue
        value = getattr(child, f.name)
        if f.name in _MAPS:
            changes[f.name] = {**getattr(base, f.name), **value}
        elif value != f.default and value != () and value != {}:
            changes[f.name] = value
    return replace(
        base, name=child.name, aliases=child.aliases, scope=child.scope, extends=None, **changes
    )


def _fill(part: str, values: Mapping[str, str], profile: str) -> str:
    def sub(match: "re.Match[str]") -> str:
        key = match.group(1)
        if key not in values:
            raise ProfileError(f"profile {profile!r}: no value for {{{key}}}")
        return values[key]

    return _PLACEHOLDER.sub(sub, part)
