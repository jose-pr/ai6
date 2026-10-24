from __future__ import annotations

import sys
import threading
import time
from dataclasses import replace

import pytest

if sys.platform == "win32":
    pytest.skip("the watchdog needs a POSIX pty", allow_module_level=True)

from coagent_m.watchdog import (
    ActivityMonitor,
    AgentSpec,
    KillReason,
    Limits,
    State,
    Watchdog,
)

FAST = Limits(stall_timeout=1.0, loop_timeout=1.0, loop_min_lines=5, blocked_after=0.3)


def mock(behavior: str) -> AgentSpec:
    return AgentSpec(behavior, [sys.executable, "-m", "coagent_m.watchdog", "mock-agent", behavior])


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


class TestWatchdog:
    def test_healthy_agent_is_done(self) -> None:
        out = Watchdog(mock("work"), FAST).run()
        assert (out.state, out.returncode, out.reason) == (State.DONE, 0, None)

    def test_looping_agent_is_killed(self) -> None:
        out = Watchdog(mock("loop"), FAST).run()
        assert out.state is State.STALLED
        assert out.reason is KillReason.LOOP
        assert out.returncode == -9

    def test_hanging_agent_is_blocked_then_killed(self) -> None:
        events = []
        out = Watchdog(mock("hang"), FAST, on_event=events.append).run()
        states = [e.state for e in events]
        assert states == [State.WORKING, State.BLOCKED, State.STALLED]
        assert out.reason is KillReason.INPUT_HANG

    def test_operator_can_answer_a_prompt(self) -> None:
        dog = Watchdog(mock("hang"), FAST)
        result = []
        thread = threading.Thread(target=lambda: result.append(dog.run()))
        thread.start()
        deadline = time.monotonic() + 5
        while dog.state is not State.BLOCKED and time.monotonic() < deadline:
            time.sleep(0.05)
        dog.send(b"y\n")
        thread.join(timeout=5)
        assert result[0].state is State.DONE
