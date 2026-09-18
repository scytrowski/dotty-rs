#!/usr/bin/env python3
"""Compare normalized Rust scanner output with the Scala 3.9.0 oracle."""

from __future__ import annotations

import pathlib
import re
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
        pathlib.Path(arguments[1]).read_text().splitlines(), source
    )
    rust = normalize_rust(pathlib.Path(arguments[2]).read_text().splitlines())

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
            and not is_in_interpolation_expression(source, range_start, start)
            for range_start, range_end in interpolation_ranges
        )
        rows.append((oracle_kind(token, name, spelling, in_interpolation), start))
    if not rows:
        raise ValueError("oracle output did not contain token rows")
    return rows


def oracle_kind(token: str, name: str, spelling: str, in_interpolation: bool) -> str:
    if token == "erroneous token":
        return "error"
    if token == "_":
        # Scala exposes wildcard underscore as a dedicated raw token; the
        # current shared token surface represents it as an identifier.
        return "identifier"
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
        if is_inside_string_literal(source, match.start()):
            continue
        quote = match.group(1)
        content_start = match.end()
        closing = find_interpolation_end(source, content_start, quote)
        if closing is None:
            continue
        content = source[content_start:closing]
        if "\n" in content or "\r" in content:
            continue
        if has_invalid_simple_splice(content):
            continue
        start = len(source[:content_start - len(quote)].encode("utf-8"))
        end = len(source[: closing + len(quote)].encode("utf-8"))
        ranges.append((start, end))
    return ranges


def find_interpolation_end(source: str, content_start: int, quote: str) -> int | None:
    index = content_start
    brace_depth = 0
    while index < len(source):
        if brace_depth:
            if source.startswith('"""', index):
                end = source.find('"""', index + 3)
                if end < 0:
                    return None
                index = end + 3
                continue
            if source[index] == '"':
                end = index + 1
                while end < len(source):
                    if source[end] == "\\":
                        end += 2
                    elif source[end] == '"':
                        break
                    else:
                        end += 1
                index = end + 1
                continue
            if source[index] == "{":
                brace_depth += 1
            elif source[index] == "}":
                brace_depth -= 1
            index += 1
            continue

        if source.startswith(quote, index):
            return index
        if source.startswith("${", index):
            brace_depth = 1
            index += 2
        elif source[index] == "\\":
            index += 2
        else:
            index += 1
    return None


def is_inside_string_literal(source: str, offset: int) -> bool:
    """Reject interpolator-looking text that is itself inside a string."""
    index = 0
    in_string: str | None = None
    while index < offset:
        if in_string is None:
            if source.startswith('"""', index):
                in_string = '"""'
                index += 3
                continue
            if source[index] == '"':
                in_string = '"'
            index += 1
            continue

        if source.startswith(in_string, index):
            index += len(in_string)
            in_string = None
        elif source[index] == "\\" and in_string == '"':
            index += 2
        else:
            index += 1
    return in_string is not None


def is_in_interpolation_expression(source: str, range_start: int, offset: int) -> bool:
    quote = source.find('"', range_start, offset)
    if quote < 0:
        return False
    content_start = quote + (3 if source.startswith('"""', quote) else 1)
    depth = 0
    index = content_start
    while index < offset:
        if source[index] == "\\":
            index += 2
            continue
        if source[index] == "{" and index > 0 and source[index - 1] == "$":
            depth += 1
        elif source[index] == "}" and depth:
            depth -= 1
        index += 1
    return depth > 0


def has_invalid_simple_splice(content: str) -> bool:
    index = 0
    while index < len(content):
        character = content[index]
        if character != "$":
            index += 1
            continue
        next_character = content[index + 1] if index + 1 < len(content) else None
        if next_character in {"$", '"', "{"}:
            index += 2
            continue
        if next_character is not None and (
            next_character == "_"
            or next_character == "$"
            or next_character.isalpha()
        ):
            index += 2
            continue
        return True
    return False


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
