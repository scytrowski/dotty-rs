#!/usr/bin/env python3
"""Compare normalized Rust scanner output with the Scala 3.9.0 oracle."""

from __future__ import annotations

import pathlib
import sys


def main() -> int:
    ignore_layout = False
    arguments = sys.argv[1:]
    if arguments and arguments[0] == "--ignore-layout":
        ignore_layout = True
        arguments = arguments[1:]
    if len(arguments) != 3:
        print(
            "usage: compare.py [--ignore-layout] <source> <oracle-output> <rust-output>",
            file=sys.stderr,
        )
        return 2

    source_path = pathlib.Path(arguments[0])
    with source_path.open(encoding="utf-8", newline="") as source_file:
        source = source_file.read()
    oracle = normalize_oracle(
        pathlib.Path(arguments[1]).read_text(encoding="utf-8").split("\n"), source
    )
    rust = normalize_rust(pathlib.Path(arguments[2]).read_text(encoding="utf-8").split("\n"))

    if ignore_layout:
        oracle = [row for row in oracle if row[0] not in LAYOUT_KINDS]
        rust = [row for row in rust if row[0] not in LAYOUT_KINDS]

    oracle_kinds = [kind for kind, _ in oracle]
    rust_kinds = [kind for kind, _ in rust]
    if oracle_kinds != rust_kinds:
        report_mismatch("token kind", source, oracle, rust)
        return 1

    oracle_sources = [(kind, start) for kind, start in oracle if kind not in LAYOUT_KINDS]
    rust_sources = [(kind, start) for kind, start in rust if kind not in LAYOUT_KINDS]
    if oracle_sources != rust_sources:
        report_mismatch("source token position", source, oracle, rust)
        return 1

    print(f"ok: {source_path}")
    return 0


LAYOUT_KINDS = {"end of statement", "indent", "unindent"}


def normalize_oracle(lines: list[str], source: str) -> list[tuple[str, int]]:
    rows: list[tuple[str, int]] = []
    # Scala's offsets count UTF-16 code units. The intermediate surrogate
    # position rounds up to the next UTF-8 boundary as in the scanner dump.
    byte_offsets = [0]
    for character in source:
        end = byte_offsets[-1] + len(character.encode("utf-8"))
        byte_offsets.append(end)
        if ord(character) > 0xFFFF:
            byte_offsets.append(end)
    for line in lines:
        fields = line.split("\t")
        if len(fields) != 8 or fields[0] == "token":
            continue
        token, start, _, _, _, _, _, lexical_kind = fields
        offset = int(start)
        if not 0 <= offset < len(byte_offsets):
            raise ValueError(f"invalid oracle UTF-16 offset {offset}")
        rows.append((lexical_kind or oracle_kind(token), byte_offsets[offset]))
    if not rows:
        raise ValueError("oracle output did not contain token rows")
    return rows


def oracle_kind(token: str) -> str:
    if token == "erroneous token":
        return "error"
    if token == "_":
        return "identifier"
    if token == "$XMLSTART$<":
        return token
    if token and not token[0].isalnum() and not token.startswith("'"):
        return "operator"
    return token


def normalize_rust(lines: list[str]) -> list[tuple[str, int]]:
    rows: list[tuple[str, int]] = []
    for line in lines:
        fields = line.split("\t")
        if len(fields) != 3 or fields[0] == "kind":
            continue
        kind, start, _ = fields
        if kind == "':'":
            # The parser-facing scanner exposes colon protocol variants, while
            # the Scala oracle reports the underlying operator token.
            kind = "operator"
        rows.append((kind, int(start)))
    if not rows:
        raise ValueError("Rust output did not contain token rows")
    return rows


def report_mismatch(
    category: str,
    source: str,
    oracle: list[tuple[str, int]],
    rust: list[tuple[str, int]],
) -> None:
    limit = min(len(oracle), len(rust))
    if category == "token kind":
        index = next(
            (index for index in range(limit) if oracle[index][0] != rust[index][0]),
            limit,
        )
    else:
        index = next(
            (index for index in range(limit) if oracle[index] != rust[index]), limit
        )
    print(f"mismatch ({category}) at token index {index}", file=sys.stderr)
    print(f"  oracle: {oracle[index] if index < len(oracle) else '<missing>'}", file=sys.stderr)
    print(f"  rust:   {rust[index] if index < len(rust) else '<missing>'}", file=sys.stderr)
    print(f"  oracle length: {len(oracle)}", file=sys.stderr)
    print(f"  rust length:   {len(rust)}", file=sys.stderr)
    if index < len(oracle):
        print(
            f"  source near oracle token: {source_excerpt(source, oracle[index][1])!r}",
            file=sys.stderr,
        )
    elif index < len(rust):
        print(
            f"  source near Rust token: {source_excerpt(source, rust[index][1])!r}",
            file=sys.stderr,
        )


def source_excerpt(source: str, offset: int) -> str:
    source_bytes = source.encode("utf-8")
    return source_bytes[max(0, offset - 20) : offset + 40].decode(
        "utf-8", errors="replace"
    )


if __name__ == "__main__":
    raise SystemExit(main())
