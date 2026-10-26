#!/usr/bin/env python3
"""Validate exact ordinary-method path#tree selections for the typer audit."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path
import re
import sys


class SelectionError(ValueError):
    pass


def normalize_source_path(value: str, line_number: int) -> str:
    path = Path(value)
    if (
        not value
        or "\\" in value
        or path.is_absolute()
        or re.match(r"^[A-Za-z]:", value) is not None
    ):
        raise SelectionError(f"line {line_number}: expected a normalized relative Scala source path")
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts) or not value.endswith(".scala"):
        raise SelectionError(f"line {line_number}: invalid Scala source path {value!r}")
    return value


def load_selection(manifest: Path, scala_root: Path) -> list[str]:
    root = scala_root.resolve(strict=True)
    corpus_roots = [
        (root / "library" / "src").resolve(strict=True),
        (root / "compiler" / "src").resolve(strict=True),
    ]
    try:
        content = manifest.read_text(encoding="utf-8")
    except OSError as error:
        raise SelectionError(f"cannot read selection {manifest}: {error}") from error

    if content.startswith("identity\t"):
        try:
            rows = csv.DictReader(content.splitlines(), delimiter="\t")
            lines = [row["identity"] for row in rows]
        except (csv.Error, KeyError, TypeError) as error:
            raise SelectionError(f"cannot read identity column from {manifest}: {error}") from error
        if any(not identity for identity in lines):
            raise SelectionError(f"selection {manifest} contains an empty identity")
    else:
        lines = content.splitlines()

    selected: set[str] = set()
    for number, raw in enumerate(lines, 1):
        value = raw.strip()
        if not value or value.startswith("#"):
            continue
        source_path, marker, tree_id = value.rpartition("#tree=")
        if not marker or re.fullmatch(r"(?:0|[1-9][0-9]*)", tree_id) is None:
            raise SelectionError(f"line {number}: expected source/path.scala#tree=N")
        source_path = normalize_source_path(source_path, number)
        candidate = root / source_path
        try:
            resolved = candidate.resolve(strict=True)
        except OSError as error:
            raise SelectionError(f"line {number}: source file is missing: {source_path!r}") from error
        if not any(resolved == corpus or corpus in resolved.parents for corpus in corpus_roots):
            raise SelectionError(f"line {number}: source path escapes the Scala source roots")
        if not resolved.is_file():
            raise SelectionError(f"line {number}: source path is not a file: {source_path!r}")
        if value in selected:
            raise SelectionError(f"line {number}: duplicate method identity {value!r}")
        selected.add(value)
    if not selected:
        raise SelectionError("selection contains no method identities")
    return sorted(selected)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--root", required=True, type=Path, help="pinned Scala repository root")
    args = parser.parse_args()
    try:
        print("\n".join(load_selection(args.manifest, args.root)))
    except (SelectionError, OSError) as error:
        print(f"ordinary method selection: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
