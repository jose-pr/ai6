"""Writes `coqui_acp_api_kinds.py` from the kinds' description, or with `--check` says whether it
is current (exit 1 if not). Needs Guatiao's Python binding on `PYTHONPATH`.

It writes what `python -m guatiao.kinds` writes, with the paths in the module's header relative to the
crate whatever the directory it runs from.
"""

from __future__ import annotations

import pathlib
import sys

from guatiao.kinds import generate

HERE = pathlib.Path(__file__).resolve().parent
DOCUMENT = HERE.parent / "include" / "coqui_acp_api.kinds.json"
MODULE = HERE / "coqui_acp_api_kinds.py"


def written() -> str:
    return generate(
        DOCUMENT.read_text(encoding="utf-8"),
        source="include/coqui_acp_api.kinds.json",
        command="python python/write_kinds.py",
    )


def main() -> int:
    text = written()
    if "--check" in sys.argv[1:]:
        if MODULE.read_bytes().decode("utf-8") != text:
            print(f"{MODULE.name} is not what the description writes: regenerate it", file=sys.stderr)
            return 1
        return 0
    MODULE.write_bytes(text.encode("utf-8"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
