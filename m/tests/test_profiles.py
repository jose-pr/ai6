from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

from coagent_m.config import Scope
from coagent_m.profiles import (
    Launcher,
    Profile,
    ProfileError,
    ProfileSet,
    load_file,
    load_profiles,
)
from coagent_m.watchdog import AgentSpec, Limits, State, Watchdog

PY = json.dumps(sys.executable)
# A stand-in agent CLI: prints its arguments, the profile's env and context, and a typed line.
FAKE_CLI = (
    "import os, sys; print('ARGS', sys.argv[1:]); print('MODEL', os.environ.get('COAGENT_MODEL'));"
    " print('TOKEN', os.environ.get('TOKEN'));"
    " p = os.environ.get('COAGENT_CONTEXT_FILE'); print('CTX', open(p).read().strip() if p else '-');"
    " print('TYPED', sys.stdin.readline().strip() if '--typed' in sys.argv else '-')"
)

GLOBAL = f"""
[launcher.cli]
command = [{PY}, "-c", {json.dumps(FAKE_CLI)}, "--model", "{{model}}", "--effort", "{{effort}}"]

[launcher.typed]
command = [{PY}, "-c", {json.dumps(FAKE_CLI)}, "--typed"]
prompt = "stdin"

[profile.007]
aliases = ["JamesBond"]
description = "planning and security review"
launcher = "cli"
model = "model-a"
effort = "high"
roles = ["plan", "review"]
priority = 10
env = {{ TOKEN = "t-007" }}
context = "Licensed to commit."
limits = {{ stall_timeout = 2.0 }}

[profile.q-branch]
extends = "007"
effort = "medium"
roles = ["implement"]
priority = 5

[profile.moneypenny]
launcher = "typed"
model = "model-b"
roles = ["implement", "docs"]
tags = ["cheap"]
"""


@pytest.fixture
def home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    (home / "profiles.toml").write_text(GLOBAL)
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.delenv("COAGENT_TRUST_PROJECT", raising=False)
    return home


@pytest.fixture
def profiles(home: Path) -> ProfileSet:
    return load_profiles(str(home))


class TestResolution:
    def test_alias(self, profiles: ProfileSet) -> None:
        assert profiles.get("JamesBond").name == "007"

    def test_extends_inherits_and_overrides(self, profiles: ProfileSet) -> None:
        q = profiles.get("q-branch")
        assert (q.model, q.effort, q.launcher) == ("model-a", "medium", "cli")
        assert q.roles == ("implement",)
        assert q.env == {"TOKEN": "t-007"}
        assert q.aliases == ()

    def test_a_cycle_is_refused(self) -> None:
        s = ProfileSet()
        s.add(Profile("a", extends="b"))
        s.add(Profile("b", extends="a"))
        with pytest.raises(ProfileError, match="cycle"):
            s.get("a")

    def test_unknown_name(self, profiles: ProfileSet) -> None:
        with pytest.raises(ProfileError, match="no profile or alias"):
            profiles.get("006")


class TestChoose:
    def test_best_by_priority(self, profiles: ProfileSet) -> None:
        assert profiles.choose("implement").name == "q-branch"

    def test_a_sub_role_matches_its_parent(self, profiles: ProfileSet) -> None:
        assert profiles.choose("review/security").name == "007"

    def test_tags_narrow_it(self, profiles: ProfileSet) -> None:
        assert profiles.choose("implement", ["cheap"]).name == "moneypenny"

    def test_nothing_suits(self, profiles: ProfileSet) -> None:
        with pytest.raises(ProfileError, match="no profile suits role 'deploy'"):
            profiles.choose("deploy")


class TestArgv:
    def test_placeholders_and_prompt_as_argument(self, profiles: ProfileSet) -> None:
        argv, env, typed = profiles.argv(profiles.get("007"), "fix it")
        assert argv[-5:] == ["--model", "model-a", "--effort", "high", "fix it"]
        assert env == {"TOKEN": "t-007"} and typed is None

    def test_prompt_on_stdin(self, profiles: ProfileSet) -> None:
        argv, _, typed = profiles.argv(profiles.get("moneypenny"), "write docs")
        assert "write docs" not in argv and typed == "write docs"

    def test_missing_placeholder(self) -> None:
        s = ProfileSet()
        s.add_launcher(Launcher("l", ("x", "{nope}")))
        s.add(Profile("p", launcher="l"))
        with pytest.raises(ProfileError, match=r"no value for \{nope\}"):
            s.argv(s.get("p"))


class TestFiles:
    def test_unknown_key_is_an_error(self, tmp_path: Path) -> None:
        path = tmp_path / "profiles.toml"
        path.write_text("[profile.x]\nmodle = 'typo'\n")
        with pytest.raises(ProfileError, match="unknown keys"):
            load_file(path, Scope.GLOBAL, ProfileSet())

    def test_a_trusted_project_overrides_a_global_profile(self, home: Path, tmp_path: Path) -> None:
        root = tmp_path / "proj"
        (root / ".coagent").mkdir(parents=True)
        (root / ".coagent" / "profiles.toml").write_text(
            "[profile.007]\nlauncher = 'cli'\nmodel = 'model-p'\nroles = ['plan']\n"
        )
        assert load_profiles(str(root)).get("007").model == "model-a"
        overridden = load_profiles(str(root), trust_project=True).get("007")
        assert (overridden.model, overridden.scope) == ("model-p", Scope.PROJECT)


class TestLaunch:
    def run(self, profiles: ProfileSet, name: str, prompt: str):
        spec, limits = AgentSpec.from_profile(profiles, name, prompt)
        spec = AgentSpec(**{**spec.__dict__, "terminal": "pipes"})
        output = []
        out = Watchdog(spec, limits, on_output=output.append).run()
        return out, limits, b"".join(output).decode()

    def test_a_profile_launches_with_its_env_context_and_limits(self, profiles) -> None:
        out, limits, text = self.run(profiles, "JamesBond", "fix it")
        assert out.state is State.DONE and out.agent == "007"
        assert "'--model', 'model-a'" in text and "'fix it'" in text
        assert "MODEL model-a" in text and "TOKEN t-007" in text
        assert "CTX Licensed to commit." in text
        assert limits.stall_timeout == 2.0 and limits.loop_timeout == Limits().loop_timeout

    def test_a_stdin_prompt_is_typed_after_start(self, profiles) -> None:
        out, _, text = self.run(profiles, "moneypenny", "write docs")
        assert out.state is State.DONE
        assert "TYPED write docs" in text

    def test_bad_limits(self) -> None:
        s = ProfileSet()
        s.add_launcher(Launcher("l", ("x",)))
        s.add(Profile("p", launcher="l", limits={"stall": 1}))
        with pytest.raises(ProfileError, match="bad limits"):
            AgentSpec.from_profile(s, "p")
