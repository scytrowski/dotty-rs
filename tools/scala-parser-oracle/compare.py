#!/usr/bin/env python3
"""Compare the normalized Scala and Rust smoke-parser trees."""

from __future__ import annotations

import json
import pathlib
import sys


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: compare.py <scala-json> <rust-json>", file=sys.stderr)
        return 2

    scala = json.loads(pathlib.Path(sys.argv[1]).read_text())
    rust = json.loads(pathlib.Path(sys.argv[2]).read_text())
    if scala == rust:
        print("ok")
        return 0

    print("parser oracle mismatch", file=sys.stderr)
    print("scala:", json.dumps(scala, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    print("rust: ", json.dumps(rust, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
