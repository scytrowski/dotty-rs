#!/usr/bin/env python3
"""Validate deterministic source-file selections for focused typer audits."""

from __future__ import annotations

import argparse
import csv
import hashlib
from pathlib import Path
import re
import sys


class SelectionError(ValueError):
    pass


COHORTS = {
    "package-term": (
        "docs/typer-classpath-corpus-audit-3.9.0-package-term-first-blockers.tsv",
        "e7fd7b295b6b502d61a2b0d61b9f15fda01e7d28dfc8bb228d9a7def55276e32",
        "package-term",
    ),
    "object-module": (
        "docs/typer-classpath-corpus-audit-1051-object-module-blockers.tsv",
        "4c7d7e4bf87ef1e213b1d26b785cbc4998ba4d0eff1a699c26ccda2d469efaa4",
        "object-module",
    ),
}


def normalize_manifest_path(value: str, *, line_number: int) -> str:
    path = Path(value)
    if (
        not value
        or "\\" in value
        or path.is_absolute()
        or re.match(r"^[A-Za-z]:", value) is not None
    ):
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
    resolved_paths: dict[Path, str] = {}
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
        previous = resolved_paths.setdefault(resolved, relative)
        if previous != relative:
            raise SelectionError(
                f"line {number}: two manifest paths resolve to the same source: {previous!r}, {relative!r}"
            )
        selected.add(relative)
    if not selected:
        raise SelectionError("manifest selected no source files")
    return sorted(selected)


def load_cohort(name: str, repo_root: Path, scala_root: Path) -> tuple[list[str], list[str]]:
    try:
        relative_path, expected_digest, kind = COHORTS[name]
    except KeyError as error:
        raise SelectionError(f"unknown focused cohort: {name!r}") from error
    path = repo_root / relative_path
    try:
        payload = path.read_bytes()
    except OSError as error:
        raise SelectionError(f"cannot read frozen cohort {path}: {error}") from error
    if hashlib.sha256(payload).hexdigest() != expected_digest:
        raise SelectionError(f"frozen cohort identity changed: {relative_path}")
    try:
        rows = list(csv.DictReader(payload.decode("utf-8").splitlines(), delimiter="\t"))
    except (UnicodeDecodeError, csv.Error) as error:
        raise SelectionError(f"cannot parse frozen cohort {relative_path}: {error}") from error
    if not rows:
        raise SelectionError(f"frozen cohort is empty: {relative_path}")
    methods: set[str] = set()
    for number, row in enumerate(rows, 2):
        if kind == "package-term":
            source = row.get("file", "")
            method = row.get("method", "")
            method_path, separator, method_tree = method.rpartition("#tree=")
            if not separator or method_path != source or not method_tree.isdecimal():
                raise SelectionError(f"{relative_path}:{number}: method identity disagrees with file")
            identity = method
        else:
            source = row.get("source_file", "")
            tree = row.get("method_tree", "")
            if not tree.isdecimal():
                raise SelectionError(f"{relative_path}:{number}: invalid method tree identity")
            identity = f"{source}#tree={tree}"
        if not source:
            raise SelectionError(f"{relative_path}:{number}: missing stable source/tree identity")
        normalize_manifest_path(source, line_number=number)
        methods.add(identity)
    files = sorted({method.rsplit("#tree=", 1)[0] for method in methods})
    validated = load_manifest_from_paths(files, scala_root)
    return validated, sorted(methods)


def load_manifest_from_paths(paths: list[str], scala_root: Path) -> list[str]:
    with _TemporaryManifest(paths) as manifest:
        return load_manifest(manifest, scala_root)


class _TemporaryManifest:
    """Small context manager that avoids leaving generated files in the repo."""

    def __init__(self, paths: list[str]):
        import tempfile

        self._temporary = tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", delete=False)
        self._temporary.write("\n".join(paths) + "\n")
        self._temporary.close()
        self.path = Path(self._temporary.name)

    def __enter__(self) -> Path:
        return self.path

    def __exit__(self, *_exc) -> None:
        self.path.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", nargs="?", type=Path)
    parser.add_argument("--root", required=True, type=Path, help="pinned Scala repository root")
    parser.add_argument("--repo-root", type=Path, help="dotty-rs repository root, required for --cohort")
    parser.add_argument("--cohort", choices=sorted(COHORTS))
    parser.add_argument("--methods-output", type=Path)
    args = parser.parse_args()
    try:
        if bool(args.manifest) == bool(args.cohort):
            raise SelectionError("provide exactly one manifest or --cohort")
        if args.cohort:
            if args.repo_root is None or args.methods_output is None:
                raise SelectionError("--cohort requires --repo-root and --methods-output")
            files, methods = load_cohort(args.cohort, args.repo_root, args.root)
            args.methods_output.write_text("\n".join(methods) + "\n", encoding="utf-8")
        else:
            files = load_manifest(args.manifest, args.root)
            methods = []
            if args.methods_output is not None:
                args.methods_output.write_text("", encoding="utf-8")
        for source in files:
            print(source)
    except SelectionError as error:
        print(f"focused selection: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
