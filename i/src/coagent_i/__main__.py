"""``python -m coagent_i``: deliver briefs, see the registry, run the intake sources."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Optional, Sequence

from .brief import Brief, BriefError
from . import open_registry
from .registry import STATES
from .sources import SourceError, load_sources, poll


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m coagent_i", description=__doc__)
    parser.add_argument("--registry", help="registry directory (default: COAGENT_REGISTRY, ...)")
    sub = parser.add_subparsers(dest="command", required=True)

    submit = sub.add_parser("submit", help="file a brief: a JSON file, or - for stdin")
    submit.add_argument("file", nargs="?", default="-")
    submit.add_argument("--replace", action="store_true", help="replace one still new")

    ls = sub.add_parser("list", help="briefs in a state, best first")
    ls.add_argument("--state", choices=STATES, default="new")
    ls.add_argument("--json", action="store_true")

    sub.add_parser("sources", help="the intake sources configured")
    sub.add_parser("poll", help="poll every source once and file what is new")

    args = parser.parse_args(argv)
    registry = open_registry(args.registry)
    try:
        if args.command == "submit":
            text = (
                sys.stdin.read() if args.file == "-" else open(args.file, encoding="utf-8").read()
            )
            path = registry.submit(Brief.from_json(text), replace=args.replace)
            print(path)
        elif args.command == "list":
            briefs = registry.list(args.state)
            if args.json:
                print(json.dumps([b.to_dict() for b in briefs], indent=2))
            else:
                for b in briefs:
                    where = f" @{b.station}" if b.station else ""
                    print(f"{b.priority:>4}  {b.key}{where}  {b.title}")
        elif args.command == "sources":
            for source in load_sources():
                print(f"{source.name}  {'reports' if hasattr(source, 'report') else ''}")
        elif args.command == "poll":
            filed, errors = poll(load_sources(), registry)
            for b in filed:
                print(f"filed {b.key}")
            for e in errors:
                print(f"failed {e}", file=sys.stderr)
            return 1 if errors else 0
    except (BriefError, SourceError, OSError) as exc:
        print(f"coagent_i: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
