#!/usr/bin/env python3
"""Compare normalized Scala and Rust parser trees."""

from __future__ import annotations

import json
import pathlib
import sys


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: compare.py <source> <scala-json> <rust-json>", file=sys.stderr)
        return 2

    source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
    scala = convert_spans(
        json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")), source
    )
    rust = json.loads(pathlib.Path(sys.argv[3]).read_text(encoding="utf-8"))
    if scala == rust:
        print("ok")
        return 0

    print("parser oracle mismatch", file=sys.stderr)
    print("scala:", json.dumps(scala, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    print("rust: ", json.dumps(rust, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    return 1


def convert_spans(value: object, source: str) -> object:
    if isinstance(value, list):
        return [convert_spans(item, source) for item in value]
    if not isinstance(value, dict):
        return value

    converted = {key: convert_spans(item, source) for key, item in value.items()}
    span = value.get("span")
    if isinstance(span, dict) and {"start", "end"} <= span.keys():
        converted["span"] = {
            "start": utf16_to_utf8_offset(source, span["start"]),
            "end": utf16_to_utf8_offset(source, span["end"]),
        }
    return converted


def utf16_to_utf8_offset(source: str, offset: object) -> int:
    if not isinstance(offset, int) or offset < 0:
        raise ValueError(f"invalid UTF-16 offset: {offset!r}")
    units = 0
    for index, character in enumerate(source):
        if units == offset:
            return len(source[:index].encode("utf-8"))
        units += len(character.encode("utf-16-le")) // 2
    if units == offset:
        return len(source.encode("utf-8"))
    raise ValueError(f"UTF-16 offset {offset} is outside the source")


if __name__ == "__main__":
    raise SystemExit(main())
