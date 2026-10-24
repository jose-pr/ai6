"""Killing a whole process tree on Windows, through a Job Object.

Scaffold: not implemented yet. ``ProcessTree.kill`` ends the direct child only, so
grandchildren of a killed agent survive on Windows until this is filled in. The
intended shape: ``CreateJobObjectW`` with ``JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE``,
``AssignProcessToJobObject`` on the child right after it starts, and
``TerminateJobObject`` to kill. See ``m/src/coagent_m/AGENTS.md``.
"""

from __future__ import annotations

import subprocess


class ProcessTree:
    """A started process and, once implemented, everything it starts."""

    def __init__(self, proc: subprocess.Popen) -> None:
        self._proc = proc

    @classmethod
    def adopt(cls, proc: subprocess.Popen) -> "ProcessTree":
        return cls(proc)

    def kill(self) -> None:
        try:
            self._proc.kill()
        except OSError:
            pass

    def close(self) -> None:
        pass
