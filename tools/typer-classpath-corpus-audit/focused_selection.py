#!/usr/bin/env python3
"""Validate deterministic source-file selections for focused typer audits."""

from __future__ import annotations

import argparse
from pathlib import Path
import sys


class SelectionError(ValueError):
    pass


def normalize_manifest_path(value: str, *, line_number: int) -> str:
    path = Path(value)
    if not value or "\\" in value or path.is_absolute():
        raise SelectionError(f"line {line_number}: expected a normalized relative source path")
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise SelectionError(f"line {line_number}: path is not normalized: {value!r}")
    if not value.endswith(".scala"):
        raise SelectionError(f"line {line_number}: expected a .scala source path")
    return value


def load_manifest(manifest: Path, scala_root: Path) -> list[str]:
    """Return sorted unique Scala-root-relative source paths from the corpus."""
    resolved_root = scala_root.resolve(strict=True)
    corpus_roots = [
        (resolved_root / "library" / "src").resolve(strict=True),
        (resolved_root / "compiler" / "src").resolve(strict=True),
    ]
    selected: set[str] = set()
    try:
        lines = manifest.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise SelectionError(f"cannot read manifest {manifest}: {error}") from error
    for number, raw in enumerate(lines, 1):
        value = raw.strip()
        if not value or value.startswith("#"):
            continue
        relative = normalize_manifest_path(value, line_number=number)
        candidate = resolved_root / relative
        try:
            resolved = candidate.resolve(strict=True)
        except FileNotFoundError:
            raise SelectionError(f"line {number}: source file is missing or outside the corpus: {relative!r}")
        except OSError as error:
            raise SelectionError(f"line {number}: cannot resolve {relative!r}: {error}") from error
        if not any(resolved == root or root in resolved.parents for root in corpus_roots):
            raise SelectionError(f"line {number}: source path escapes the corpus: {relative!r}")
        if not resolved.is_file() or candidate.suffix != ".scala":
            raise SelectionError(f"line {number}: source file is missing or outside the corpus: {relative!r}")
        selected.add(relative)
    if not selected:
        raise SelectionError("manifest selected no source files")
    return sorted(selected)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--root", required=True, type=Path, help="pinned Scala repository root")
    args = parser.parse_args()
    try:
        for source in load_manifest(args.manifest, args.root):
            print(source)
    except SelectionError as error:
        print(f"focused selection: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
