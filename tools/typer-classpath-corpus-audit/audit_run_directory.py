#!/usr/bin/env python3
"""Record and resolve the retained per-run audit directory."""

from __future__ import annotations

import argparse
from pathlib import Path
import sys


POINTER_NAME = "run-dir.txt"


def validate_run_dir(root: Path, run_dir: Path) -> Path:
    root = root.resolve()
    run_dir = run_dir.resolve()
    if run_dir.parent != root or not run_dir.name.startswith("run."):
        raise ValueError(f"audit run directory must be a direct run.* child of {root}")
    if not run_dir.is_dir():
        raise ValueError(f"audit run directory does not exist: {run_dir}")
    return run_dir


def record(root: Path, run_dir: Path) -> None:
    root.mkdir(parents=True, exist_ok=True)
    resolved = validate_run_dir(root, run_dir)
    (root / POINTER_NAME).write_text(f"{resolved}\n", encoding="utf-8")


def resolve(root: Path) -> Path:
    root = root.resolve()
    try:
        run_dir = Path((root / POINTER_NAME).read_text(encoding="utf-8").strip())
    except OSError as error:
        raise ValueError(f"cannot read retained audit run directory: {error}") from error
    return validate_run_dir(root, run_dir)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    record_parser = subparsers.add_parser("record")
    record_parser.add_argument("root", type=Path)
    record_parser.add_argument("run_dir", type=Path)
    resolve_parser = subparsers.add_parser("resolve")
    resolve_parser.add_argument("root", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "record":
            record(args.root, args.run_dir)
        else:
            print(resolve(args.root))
    except ValueError as error:
        print(f"audit run directory: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
