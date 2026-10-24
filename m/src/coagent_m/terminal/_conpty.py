"""Windows pseudo-console (ConPTY), through the ``pywinpty`` extra.

Scaffold: ``available()`` refuses, so a registry never picks it. To finish it:

* spawn with ``winpty.PtyProcess.spawn(argv, cwd=..., env=..., dimensions=(rows, cols))``;
  its ``read`` blocks and answers ``str``, so pump it on a thread into a queue (as
  ``_pipes.PipeSession`` does) and encode to UTF-8;
* watch the child's exit separately from reads: the pseudo-console stays readable after
  the child exits, so EOF alone never ends a session;
* adopt the child into a ``_wintree.ProcessTree`` so ``kill`` ends its descendants;
* ConPTY sends a cursor-position query (``ESC[6n``) at start-up and waits for the
  answer; the watchdog answers it for every provider, so nothing is needed here.
"""

from __future__ import annotations

import sys
from typing import Optional

from ._base import Session, TerminalError, TerminalSpec


class ConPtyProvider:
    """``conpty``: Windows 10 1809+ pseudo-console. Not implemented yet."""

    name = "conpty"
    priority = 20

    def available(self) -> Optional[str]:
        if sys.platform != "win32":
            return "ConPTY exists only on Windows"
        try:
            import winpty  # noqa: F401
        except ImportError:
            return "needs the pywinpty package: pip install coagent-m[conpty]"
        return "the conpty provider is not implemented yet"

    def spawn(self, spec: TerminalSpec) -> Session:
        raise TerminalError(self.available() or "the conpty provider is not implemented yet")
