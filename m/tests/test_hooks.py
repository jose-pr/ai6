from __future__ import annotations

import json
import os
import sys
import textwrap
from pathlib import Path

import pytest

from coagent_m.hooks import (
    CommandHook,
    HookBase,
    HookError,
    Preparation,
    Scope,
    load_hooks,
    parse_output,
)
from coagent_m.watchdog import AgentSpec, Limits, State, Watchdog

FAST = Limits(stall_timeout=2.0, loop_timeout=2.0, blocked_after=0.5)
TERMINAL = "pipes"
SHOW = (
    "import os; print('X=' + os.environ.get('X', '-')); "
    "p = os.environ.get('COAGENT_CONTEXT_FILE'); print('CTX=' + (open(p).read().strip() if p else '-'))"
)


@pytest.fixture(autouse=True)
def isolated(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.delenv("COAGENT_TRUST_PROJECT", raising=False)
    return home


def run(spec: AgentSpec, hooks=None):
    output = []
    out = Watchdog(spec, FAST, on_output=output.append, hooks=hooks).run()
    return out, b"".join(output).decode()


def show(cwd=None, hooks=()) -> AgentSpec:
    return AgentSpec("show", [sys.executable, "-c", SHOW], cwd=cwd, terminal=TERMINAL, hooks=hooks)


def script(path: Path, body: str) -> list:
    with open(path, "w", newline="") as f:
        f.write(textwrap.dedent(body))
    return [sys.executable, str(path)]


class TestParseOutput:
    def test_env_lines(self) -> None:
        prep = parse_output("env", "export A=1\nB='two words'\nnot a line\nset C=3\n")
        assert prep.env == {"A": "1", "B": "two words", "C": "3"}

    def test_context(self) -> None:
        assert parse_output("context", "  hello \n").context == ["hello"]
        assert parse_output("context", "  ") is None

    def test_json(self) -> None:
        prep = parse_output("json", '{"env": {"A": "1", "B": null}, "context": "c", "argv": ["x"]}')
        assert prep == Preparation(env={"A": "1", "B": None}, context=["c"], argv=["x"])

    def test_bad_json_and_unknown_kind(self) -> None:
        with pytest.raises(HookError):
            parse_output("json", "{nope")
        with pytest.raises(HookError):
            parse_output("yaml", "")


class TestWatchdogHooks:
    def test_agent_hook_sets_env_and_context(self) -> None:
        class Inject(HookBase):
            name = "inject"

            def prepare(self, ctx):
                return Preparation(env={"X": "agent"}, context=["be brief"])

        out, text = run(show(hooks=[Inject()]))
        assert out.state is State.DONE
        assert "X=agent" in text and "CTX=be brief" in text

    def test_command_hook_receives_json_and_runs_in_cwd(self, tmp_path: Path) -> None:
        argv = script(
            tmp_path / "hook.py",
            """
            import json, os, sys
            ctx = json.load(sys.stdin)
            print(json.dumps({"env": {"X": ctx["agent"] + "@" + os.path.basename(os.getcwd())}}))
            """,
        )
        hook = CommandHook("cmd", argv)
        out, text = run(show(cwd=str(tmp_path), hooks=[hook]))
        assert f"X=show@{tmp_path.name}" in text

    def test_events_and_outcome_reach_hooks(self) -> None:
        seen = []

        class Watch(HookBase):
            name = "watch"

            def on_event(self, event):
                seen.append(event.state)

            def finished(self, ctx, outcome):
                seen.append(outcome.state)

        run(show(hooks=[Watch()]))
        assert seen == [State.WORKING, State.DONE, State.DONE]

    def test_a_failing_hook_is_skipped_unless_required(self, tmp_path: Path) -> None:
        fail = [sys.executable, "-c", "raise SystemExit(4)"]
        out, _ = run(show(hooks=[CommandHook("soft", fail)]))
        assert out.state is State.DONE
        with pytest.raises(HookError, match="exited 4"):
            run(show(hooks=[CommandHook("hard", fail, required=True)]))

    def test_argv_can_be_rewritten(self) -> None:
        class Wrap(HookBase):
            name = "wrap"

            def prepare(self, ctx):
                return Preparation(argv=[sys.executable, "-c", "print('wrapped')"])

        _, text = run(show(hooks=[Wrap()]))
        assert "wrapped" in text


class TestScopes:
    def _project(self, tmp_path: Path, value: str) -> Path:
        root = tmp_path / "proj"
        (root / ".coagent").mkdir(parents=True)
        (root / "sub").mkdir()
        hook = script(root / ".coagent" / "env.py", f"print('export X={value}')\n")
        (root / ".coagent" / "hooks.toml").write_text(
            f"[[hook]]\nname = 'proj'\noutput = 'env'\ncommand = {json.dumps(hook)}\n"
        )
        return root

    def test_global_hooks_toml(self, isolated: Path, tmp_path: Path) -> None:
        hook = script(tmp_path / "g.py", "print('global context')\n")
        (isolated / "hooks.toml").write_text(
            f"[[hook]]\nname = 'g'\noutput = 'context'\ncommand = {json.dumps(hook)}\n"
        )
        _, text = run(show(), hooks=load_hooks(None))
        assert "CTX=global context" in text

    def test_project_hooks_need_trust(self, tmp_path: Path, monkeypatch) -> None:
        root = self._project(tmp_path, "project")
        cwd = str(root / "sub")
        assert len(load_hooks(cwd)) == 0
        _, text = run(show(cwd=cwd), hooks=load_hooks(cwd))
        assert "X=-" in text
        monkeypatch.setenv("COAGENT_TRUST_PROJECT", "1")
        _, text = run(show(cwd=cwd), hooks=load_hooks(cwd))
        assert "X=project" in text

    def test_a_registered_station_is_read(self, isolated: Path, tmp_path: Path) -> None:
        root = self._project(tmp_path, "project")
        (isolated / "stations.toml").write_text(f"[station.P]\npath = {json.dumps(str(root))}\n")
        hooks = load_hooks(str(root / "sub"))
        assert [(s, h.name) for s, h in hooks.hooks] == [(Scope.STATION, "proj")]

    def test_narrower_scope_wins(self, tmp_path: Path) -> None:
        root = self._project(tmp_path, "project")

        class Agent(HookBase):
            name = "agent"

            def prepare(self, ctx):
                return Preparation(env={"X": "agent"})

        hooks = load_hooks(str(root), [Agent()], trust_project=True)
        assert [s for s, _ in hooks.hooks] == [Scope.STATION, Scope.AGENT]
        _, text = run(show(cwd=str(root)), hooks=hooks)
        assert "X=agent" in text

    def test_a_relative_command_resolves_beside_its_file(self, tmp_path: Path) -> None:
        root = tmp_path / "rel"
        (root / ".coagent").mkdir(parents=True)
        (root / ".coagent" / "hook.sh").write_text("#!/bin/sh\necho 'export X=relative'\n")
        os.chmod(root / ".coagent" / "hook.sh", 0o755)
        (root / ".coagent" / "hooks.toml").write_text(
            "[[hook]]\nname = 'rel'\noutput = 'env'\ncommand = ['./hook.sh']\n"
        )
        if sys.platform == "win32":
            pytest.skip("a shell script hook")
        _, text = run(show(cwd=str(root)), hooks=load_hooks(str(root), trust_project=True))
        assert "X=relative" in text
