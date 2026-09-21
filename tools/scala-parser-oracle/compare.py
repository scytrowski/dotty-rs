#!/usr/bin/env python3
"""Compare normalized Scala and Rust parser trees."""

from __future__ import annotations

import json
import pathlib
import sys


def main() -> int:
    if len(sys.argv) == 5 and sys.argv[1] == "--batch":
        return compare_batch(
            pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]), pathlib.Path(sys.argv[4])
        )
    if len(sys.argv) != 4:
        print(
            "usage: compare.py <source> <scala-json> <rust-json> | "
            "--batch <manifest> <scala-jsonl> <rust-jsonl>",
            file=sys.stderr,
        )
        return 2

    return compare_one(
        pathlib.Path(sys.argv[1]),
        json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")),
        json.loads(pathlib.Path(sys.argv[3]).read_text(encoding="utf-8")),
    )


def compare_one(source_path: pathlib.Path, scala: object, rust: object) -> int:
    source = source_path.read_text(encoding="utf-8")
    scala = convert_spans(scala, source)
    if scala == rust:
        return 0

    print(f"parser oracle mismatch: {source_path}", file=sys.stderr)
    print("scala:", json.dumps(scala, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    print("rust: ", json.dumps(rust, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    return 1


def compare_batch(manifest_path: pathlib.Path, scala_path: pathlib.Path, rust_path: pathlib.Path) -> int:
    entries = []
    for line in manifest_path.read_text(encoding="utf-8").splitlines():
        if line:
            mode, path = line.split("\t", 1)
            entries.append((mode, pathlib.Path(path)))

    scala_lines = scala_path.read_text(encoding="utf-8").splitlines()
    rust_lines = rust_path.read_text(encoding="utf-8").splitlines()
    if len(scala_lines) != len(entries) or len(rust_lines) != len(entries):
        print(
            "oracle batch output count mismatch: "
            f"manifest={len(entries)} scala={len(scala_lines)} rust={len(rust_lines)}",
            file=sys.stderr,
        )
        return 1

    for index, (_, source_path) in enumerate(entries):
        result = compare_one(
            source_path,
            json.loads(scala_lines[index]),
            json.loads(rust_lines[index]),
        )
        if result:
            return result

    print(f"ok ({len(entries)} fixtures)")
    return 0


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
