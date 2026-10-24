from __future__ import annotations

import sys
from typing import Optional

import pytest

from coagent_m.terminal import (
    ProviderUnavailable,
    Registry,
    TerminalError,
    TerminalSpec,
    default_registry,
)

PROVIDERS = [i.provider for i in default_registry().providers() if i.reason is None]


class Fake:
    def __init__(self, name: str, priority: int, reason: Optional[str] = None) -> None:
        self.name, self.priority, self.reason = name, priority, reason

    def available(self) -> Optional[str]:
        return self.reason

    def spawn(self, spec: TerminalSpec):
        raise TerminalError("fake")


class TestRegistry:
    def test_best_first_by_priority_then_name(self) -> None:
        reg = Registry([Fake("b", 1), Fake("a", 1), Fake("c", 5)])
        assert [i.name for i in reg.providers()] == ["c", "a", "b"]

    def test_set_priority_overrides_the_providers_own(self) -> None:
        reg = Registry([Fake("a", 1), Fake("b", 5)])
        reg.set_priority("a", 10)
        assert reg.best().name == "a"

    def test_best_skips_what_cannot_run(self) -> None:
        reg = Registry([Fake("top", 9, "no device"), Fake("low", 1)])
        assert reg.best().name == "low"

    def test_nothing_available_names_every_reason(self) -> None:
        reg = Registry([Fake("a", 1, "missing x")])
        with pytest.raises(ProviderUnavailable, match="a: missing x"):
            reg.best()

    def test_get_refuses_an_unavailable_provider(self) -> None:
        reg = Registry([Fake("a", 1, "missing x")])
        with pytest.raises(ProviderUnavailable, match="cannot run here: missing x"):
            reg.get("a")

    def test_duplicate_names_are_refused(self) -> None:
        with pytest.raises(ValueError):
            Registry([Fake("a", 1), Fake("a", 2)])

    def test_a_provider_that_raises_is_unavailable(self) -> None:
        class Broken(Fake):
            def available(self) -> Optional[str]:
                raise RuntimeError("boom")

        reg = Registry([Broken("x", 1)])
        assert reg.providers()[0].reason == "RuntimeError: boom"

    def test_builtins_are_registered(self) -> None:
        names = {i.name for i in default_registry().providers()}
        assert {"posix-pty", "conpty", "pipes"} <= names


@pytest.mark.parametrize("provider", PROVIDERS, ids=lambda p: p.name)
class TestSession:
    def test_output_then_exit_code(self, provider) -> None:
        session = provider.spawn(
            TerminalSpec([sys.executable, "-c", "print('hi'); raise SystemExit(3)"])
        )
        out = b""
        while True:
            data = session.read(2.0)
            if not data:
                break
            out += data
        exit_ = session.wait(5)
        session.close()
        assert b"hi" in out
        assert exit_ is not None and exit_.returncode == 3

    def test_kill_reports_a_signal_or_a_code(self, provider) -> None:
        session = provider.spawn(
            TerminalSpec([sys.executable, "-c", "import time; time.sleep(30)"])
        )
        assert session.poll() is None
        session.kill()
        exit_ = session.wait(5)
        session.kill()  # idempotent once reaped
        session.close()
        assert exit_ is not None and exit_.returncode != 0

    def test_read_times_out_with_none(self, provider) -> None:
        session = provider.spawn(
            TerminalSpec([sys.executable, "-c", "import time; time.sleep(30)"])
        )
        try:
            assert session.read(0.1) is None
        finally:
            session.close()

    def test_write_reaches_the_program(self, provider) -> None:
        code = "import sys; print('got', sys.stdin.readline().strip())"
        session = provider.spawn(TerminalSpec([sys.executable, "-c", code]))
        session.write(b"ping\n")
        out = b""
        while b"got ping" not in out:
            data = session.read(2.0)
            if not data:
                break
            out += data
        session.close()
        assert b"got ping" in out


def test_posix_pty_sets_term_and_size() -> None:
    reg = default_registry()
    if any(i.name == "posix-pty" and i.reason for i in reg.providers()):
        pytest.skip("no POSIX pty here")
    code = "import os, shutil; print(os.environ['TERM'], os.get_terminal_size(0))"
    session = reg.get("posix-pty").spawn(
        TerminalSpec([sys.executable, "-c", code], env={"PATH": ""}, rows=30, cols=100)
    )
    out = b""
    while True:
        data = session.read(2.0)
        if not data:
            break
        out += data
    session.close()
    assert b"xterm-256color" in out
    assert b"columns=100, lines=30" in out
