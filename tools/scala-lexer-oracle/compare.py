#!/usr/bin/env python3
"""Compare normalized Rust scanner output with the Scala 3.9.0 oracle."""

from __future__ import annotations

import pathlib
import re
import sys


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: compare.py <source> <oracle-output> <rust-output>", file=sys.stderr)
        return 2

    source_path = pathlib.Path(sys.argv[1])
    with source_path.open(encoding="utf-8", newline="") as source_file:
        source = source_file.read()
    oracle = normalize_oracle(
        pathlib.Path(sys.argv[2]).read_text().splitlines(), source
    )
    rust = normalize_rust(pathlib.Path(sys.argv[3]).read_text().splitlines())

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
    interpolation_ranges = find_interpolation_ranges(source)
    for line in lines:
        fields = line.split("\t")
        if len(fields) < 6 or fields[0] == "token":
            continue
        try:
            start = utf16_to_utf8_offset(source, int(fields[1]))
            end = utf16_to_utf8_offset(source, int(fields[2]))
        except ValueError:
            continue
        token, _, _, _, name, *_ = fields
        spelling = source.encode("utf-8")[start:end].decode("utf-8", errors="replace")
        in_interpolation = any(
            range_start <= start < range_end
            for range_start, range_end in interpolation_ranges
        )
        rows.append((oracle_kind(token, name, spelling, in_interpolation), start))
    if not rows:
        raise ValueError("oracle output did not contain token rows")
    return rows


def oracle_kind(token: str, name: str, spelling: str, in_interpolation: bool) -> str:
    if token == "$XMLSTART$<":
        return token
    if token == "string literal" and in_interpolation:
        return "string part"
    if token == "number literal with exponent":
        return "exponent literal"
    if token == "number literal":
        if "." in spelling:
            return "decimal literal"
        if "e" in spelling.lower():
            return "exponent literal"
        return "integer literal"
    if token == "identifier" and spelling.startswith("`"):
        return "backquoted identifier"
    if token == "identifier" and name and not is_identifier_name(name):
        return "operator"
    if token and not token[0].isalnum() and not token.startswith("'"):
        return "operator"
    return token


def find_interpolation_ranges(source: str) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    pattern = re.compile(r'(?<![\w$])(?:[A-Za-z_][A-Za-z0-9_]*)("""|")')
    for match in pattern.finditer(source):
        quote = match.group(1)
        content_start = match.end()
        closing = source.find(quote, content_start)
        if closing < 0:
            continue
        start = len(source[:content_start - len(quote)].encode("utf-8"))
        end = len(source[: closing + len(quote)].encode("utf-8"))
        ranges.append((start, end))
    return ranges


def is_identifier_name(name: str) -> bool:
    first = name[0]
    return first == "_" or first.isalpha() or first.isdigit() or first in "$"


def utf16_to_utf8_offset(source: str, offset: int) -> int:
    """Convert a Scala UTF-16 source offset to a Rust UTF-8 byte offset."""
    utf16_units = 0
    utf8_bytes = 0
    for character in source:
        if utf16_units >= offset:
            break
        utf16_units += len(character.encode("utf-16-le")) // 2
        utf8_bytes += len(character.encode("utf-8"))
    return utf8_bytes


def normalize_rust(lines: list[str]) -> list[tuple[str, int]]:
    rows: list[tuple[str, int]] = []
    for line in lines:
        fields = line.split("\t")
        if len(fields) != 3 or fields[0] == "kind":
            continue
        kind, start, _ = fields
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
