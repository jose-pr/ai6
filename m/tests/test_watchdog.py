from __future__ import annotations

import os
import re
import sys
import threading
import time
from dataclasses import replace

import pytest

from coagent_m.watchdog import (
    ActivityMonitor,
    AgentSpec,
    CursorQueryResponder,
    KillReason,
    Limits,
    State,
    Watchdog,
)
from coagent_m.terminal import default_registry

FAST = Limits(stall_timeout=1.0, loop_timeout=1.0, loop_min_lines=5, blocked_after=0.3)
PROVIDERS = [i.name for i in default_registry().providers() if i.reason is None]


def mock(behavior: str, terminal: str) -> AgentSpec:
    argv = [sys.executable, "-m", "coagent_m.watchdog", "mock-agent", behavior]
    return AgentSpec(behavior, argv, terminal=terminal)


class TestActivityMonitor:
    def test_silence_kills_after_stall_timeout(self) -> None:
        m = ActivityMonitor(FAST, now=0.0)
        m.feed(b"working\n", now=0.5)
        assert m.verdict(1.4) is None
        assert m.verdict(1.5) is KillReason.SILENT

    def test_prompt_tail_is_an_input_hang(self) -> None:
        m = ActivityMonitor(FAST, now=0.0)
        m.feed(b"Continue? [y/N] ", now=0.0)
        assert not m.blocked(0.2)
        assert m.blocked(0.3)
        assert m.verdict(1.0) is KillReason.INPUT_HANG

    def test_repeated_lines_are_a_loop(self) -> None:
        m = ActivityMonitor(FAST, now=0.0)
        for i in range(20):
            m.feed(b"\x1b[31mretrying\x1b[0m\n", now=i * 0.1)
        assert m.verdict(1.9) is KillReason.LOOP

    def test_novel_lines_are_progress(self) -> None:
        m = ActivityMonitor(FAST, now=0.0)
        for i in range(20):
            m.feed(f"test {i} passed\n".encode(), now=i * 0.1)
        assert m.verdict(1.9) is None

    def test_normalize_digits_folds_counters(self) -> None:
        m = ActivityMonitor(replace(FAST, normalize_digits=True), now=0.0)
        for i in range(20):
            m.feed(f"retry {i}\n".encode(), now=i * 0.1)
        assert m.verdict(1.9) is KillReason.LOOP

    def test_operator_input_resets_both_clocks(self) -> None:
        m = ActivityMonitor(FAST, now=0.0)
        m.feed(b"Name: ", now=0.0)
        m.operator_input(now=0.9)
        assert m.verdict(1.5) is None

    def test_max_runtime(self) -> None:
        m = ActivityMonitor(Limits(max_runtime=2.0), now=0.0)
        assert m.verdict(2.0) is KillReason.MAX_RUNTIME


class TestCursorQueryResponder:
    def test_answers_and_removes_a_query(self) -> None:
        r = CursorQueryResponder()
        assert r.feed(b"a\x1b[6nb") == (b"ab", [b"\x1b[24;1R"])

    def test_a_split_query_is_held_until_it_completes(self) -> None:
        r = CursorQueryResponder()
        assert r.feed(b"a\x1b[") == (b"a", [])
        assert r.feed(b"6nb") == (b"b", [b"\x1b[24;1R"])

    def test_other_sequences_pass_through(self) -> None:
        r = CursorQueryResponder()
        assert r.feed(b"\x1b[6") == (b"", [])
        assert r.feed(b"m") == (b"\x1b[6m", [])

    def test_flush_releases_a_held_prefix(self) -> None:
        r = CursorQueryResponder()
        r.feed(b"x\x1b")
        assert r.flush() == b"\x1b"


@pytest.mark.parametrize("terminal", PROVIDERS)
class TestWatchdog:
    def test_healthy_agent_is_done(self, terminal: str) -> None:
        out = Watchdog(mock("work", terminal), FAST).run()
        assert (out.state, out.returncode, out.reason) == (State.DONE, 0, None)

    def test_looping_agent_is_killed(self, terminal: str) -> None:
        out = Watchdog(mock("loop", terminal), FAST).run()
        assert out.state is State.STALLED
        assert out.reason is KillReason.LOOP

    def test_hanging_agent_is_blocked_then_killed(self, terminal: str) -> None:
        events = []
        out = Watchdog(mock("hang", terminal), FAST, on_event=events.append).run()
        assert [e.state for e in events] == [State.WORKING, State.BLOCKED, State.STALLED]
        assert out.reason is KillReason.INPUT_HANG

    def test_operator_can_answer_a_prompt(self, terminal: str) -> None:
        dog = Watchdog(mock("hang", terminal), FAST)
        result = []
        thread = threading.Thread(target=lambda: result.append(dog.run()))
        thread.start()
        deadline = time.monotonic() + 5
        while dog.state is not State.BLOCKED and time.monotonic() < deadline:
            time.sleep(0.05)
        dog.send(b"y\n")
        thread.join(timeout=5)
        assert result[0].state is State.DONE

    def test_a_cursor_query_is_answered_and_hidden(self, terminal: str) -> None:
        output = []
        out = Watchdog(mock("cursor", terminal), FAST, on_output=output.append).run()
        text = b"".join(output)
        assert out.state is State.DONE
        assert b"cursor at 24;1" in text
        assert CursorQueryResponder.QUERY not in text

    @pytest.mark.skipif(
        sys.platform == "win32", reason="process-tree kill is a scaffold on Windows"
    )
    def test_descendants_die_with_the_agent(self, terminal: str) -> None:
        output = []
        out = Watchdog(mock("orphan", terminal), FAST, on_output=output.append).run()
        assert out.reason is KillReason.SILENT
        pid = int(re.search(rb"child (\d+)", b"".join(output)).group(1))
        assert _gone(pid)


def _gone(pid: int, within: float = 2.0) -> bool:
    deadline = time.monotonic() + within
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        if _zombie(pid):
            return True
        time.sleep(0.05)
    return False


def _zombie(pid: int) -> bool:
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().split(") ", 1)[1].startswith("Z")
    except (OSError, IndexError):
        return False
