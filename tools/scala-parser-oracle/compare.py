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
    with source_path.open(encoding="utf-8", newline="") as stream:
        offsets = utf8_offsets(stream.read())
    scala = convert_spans(scala, offsets)
    if scala == rust:
        return 0

    print(f"parser oracle mismatch: {source_path}", file=sys.stderr)
    print("scala:", json.dumps(scala, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    print("rust: ", json.dumps(rust, ensure_ascii=False, sort_keys=True), file=sys.stderr)
    return 1


def read_records(path: pathlib.Path) -> list[str]:
    # U+2028 and U+2029 can appear inside a JSON string or source path.
    with path.open(encoding="utf-8", newline="\n") as stream:
        return [line.removesuffix("\r\n").removesuffix("\n") for line in stream]


def compare_batch(manifest_path: pathlib.Path, scala_path: pathlib.Path, rust_path: pathlib.Path) -> int:
    entries = []
    for line in read_records(manifest_path):
        if line:
            mode, path = line.split("\t", 1)
            entries.append((mode, pathlib.Path(path)))

    scala_lines = read_records(scala_path)
    rust_lines = read_records(rust_path)
    if len(scala_lines) != len(entries) or len(rust_lines) != len(entries):
        print(
            "oracle batch output count mismatch: "
            f"manifest={len(entries)} scala={len(scala_lines)} rust={len(rust_lines)}",
            file=sys.stderr,
        )
        return 1

    for index, (mode, source_path) in enumerate(entries):
        if mode == "oracle-only":
            scala = json.loads(scala_lines[index])
            if isinstance(scala, dict) and scala.get("kind") == "OracleFailure":
                print(
                    f"Scala parser oracle failed on regression fixture {source_path}: "
                    f"{scala.get('error', 'unknown error')}",
                    file=sys.stderr,
                )
                return 1
            continue

        result = compare_one(
            source_path,
            json.loads(scala_lines[index]),
            json.loads(rust_lines[index]),
        )
        if result:
            return result

    print(f"ok ({len(entries)} fixtures)")
    return 0


def utf8_offsets(source: str) -> list[int]:
    offsets = [0]
    for character in source:
        end = offsets[-1] + len(character.encode("utf-8"))
        offsets.append(end)
        if ord(character) > 0xFFFF:
            offsets.append(end)
    return offsets


def convert_spans(value: object, offsets: list[int]) -> object:
    if isinstance(value, list):
        return [convert_spans(item, offsets) for item in value]
    if not isinstance(value, dict):
        return value

    converted = {key: convert_spans(item, offsets) for key, item in value.items()}
    span = value.get("span")
    if isinstance(span, dict) and {"start", "end"} <= span.keys():
        for field in ("start", "end"):
            position = span[field]
            if type(position) is not int or not 0 <= position < len(offsets):
                raise ValueError(f"invalid UTF-16 offset: {position!r}")
            converted["span"][field] = offsets[position]
    return converted


if __name__ == "__main__":
    raise SystemExit(main())
