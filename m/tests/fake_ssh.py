"""A stand-in ``ssh`` client: runs the remote command locally, in a new session the way
``sshd`` would, so the ssh provider's quoting, marker and remote kill are exercised
without a second machine."""

import os
import subprocess
import sys

args = sys.argv[1:]
line = " ".join(args[args.index("--") + 1 :])
if "-tt" not in args:
    sys.exit(subprocess.run(["sh", "-c", line]).returncode)
pid = os.fork()
if pid == 0:
    os.setsid()
    os.execvp("sh", ["sh", "-c", line])
_, status = os.waitpid(pid, 0)
sys.exit(
    os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") else status >> 8
)
