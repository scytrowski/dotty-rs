"""Validate and enumerate Scala sources from an immutable checkout."""

from __future__ import annotations

import pathlib
import subprocess

KYO_SCALA_2_12_PLUGIN_PREFIXES = (
    ("kyo-compat", "plugin"),
    ("kyo-doctest", "plugin"),
    ("kyo-ffi", "plugin"),
    ("kyo-test", "sbt"),
    ("kyo-test", "sbt-publish"),
)


def kyo_scala3_production_roots(
    repository: pathlib.Path, source_roots: list[pathlib.Path]
) -> list[pathlib.Path]:
    """Exclude known Scala 2.12 sbt plugins from Kyo's Scala 3 source roots."""
    repository = repository.resolve(strict=True)
    selected = []
    for root in source_roots:
        root = root.resolve(strict=True)
        try:
            relative = root.relative_to(repository)
        except ValueError as error:
            raise ValueError(f"source root is outside Kyo checkout: {root}") from error
        if any(
            relative.parts[: len(prefix)] == prefix
            for prefix in KYO_SCALA_2_12_PLUGIN_PREFIXES
        ):
            continue
        selected.append(root)
    return selected


def tracked_scala_sources(
    repository: pathlib.Path, source_roots: list[pathlib.Path]
) -> list[pathlib.Path]:
    """Return Scala files at HEAD, rejecting changed or extra corpus sources."""
    repository = repository.resolve()
    roots = [root.resolve() for root in source_roots]
    for root in roots:
        if not root.is_relative_to(repository) or not root.is_dir():
            raise ValueError(f"source root is missing or outside checkout: {root}")

    changed = subprocess.run(
        ["git", "-C", str(repository), "diff", "--quiet", "HEAD", "--"],
        check=False,
    )
    if changed.returncode != 0:
        raise ValueError(f"tracked files differ from pinned HEAD in {repository}")

    listing = subprocess.run(
        ["git", "-C", str(repository), "ls-tree", "-r", "-z", "--name-only", "HEAD"],
        check=True,
        stdout=subprocess.PIPE,
    ).stdout
    tracked = {
        (repository / pathlib.Path(path.decode("utf-8"))).resolve()
        for path in listing.split(b"\0")
        if path.endswith(b".scala")
    }
    actual: set[pathlib.Path] = set()
    for root in roots:
        for source in root.rglob("*.scala"):
            if source.is_symlink() or not source.is_file():
                continue
            resolved = source.resolve()
            if not resolved.is_relative_to(repository):
                raise ValueError(f"source escapes checkout root: {source}")
            actual.add(resolved)

    extras = sorted(actual - tracked)
    if extras:
        raise ValueError(f"untracked Scala source in corpus checkout: {extras[0]}")
    return sorted(
        (source for source in tracked if any(source.is_relative_to(root) for root in roots)),
        key=lambda source: source.as_posix(),
    )
