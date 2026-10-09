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
ZIO_NON_SCALA3_PROJECTS = ("zio-docs",)
FS2_NON_LIBRARY_PROJECTS = ("benchmark",)
IRON_NON_LIBRARY_PROJECTS = ("docs", "examples", "sandbox")


def magnolia_scala3_production_roots(repository: pathlib.Path) -> list[pathlib.Path]:
    """Return Magnolia's published Scala 3 core sources, excluding examples."""
    repository = repository.resolve(strict=True)
    core_root = repository / "core" / "src" / "main" / "scala"
    if not core_root.is_dir():
        raise ValueError(f"Magnolia core source root is missing: {core_root}")
    core_root = core_root.resolve(strict=True)
    try:
        core_root.relative_to(repository)
    except ValueError as error:
        raise ValueError(f"Magnolia source root is outside checkout: {core_root}") from error
    if not any(core_root.rglob("*.scala")):
        return []
    return [core_root]


def iron_library_production_roots(repository: pathlib.Path) -> list[pathlib.Path]:
    """Return Iron library module source roots, excluding examples and docs."""
    repository = repository.resolve(strict=True)
    selected = []
    for module in sorted(repository.iterdir()):
        if not module.is_dir() or module.name in IRON_NON_LIBRARY_PROJECTS:
            continue
        module = module.resolve(strict=True)
        try:
            module.relative_to(repository)
        except ValueError as error:
            raise ValueError(f"Iron source module is outside Iron checkout: {module}") from error
        source_root = module / "src"
        if source_root.is_dir() and any(source_root.rglob("*.scala")):
            selected.append(source_root.resolve(strict=True))
    return selected


def fs2_library_production_roots(
    repository: pathlib.Path, source_roots: list[pathlib.Path]
) -> list[pathlib.Path]:
    """Exclude FS2's benchmark project from the library source corpus."""
    repository = repository.resolve(strict=True)
    selected = []
    for root in source_roots:
        root = root.resolve(strict=True)
        try:
            relative = root.relative_to(repository)
        except ValueError as error:
            raise ValueError(f"source root is outside FS2 checkout: {root}") from error
        if relative.parts and relative.parts[0] in FS2_NON_LIBRARY_PROJECTS:
            continue
        selected.append(root)
    return selected


def shapeless3_compile_roots(
    repository: pathlib.Path, source_roots: list[pathlib.Path]
) -> list[pathlib.Path]:
    """Include Shapeless's Compile sources, including its test-support module."""
    repository = repository.resolve(strict=True)
    selected = []
    for root in source_roots:
        root = root.resolve(strict=True)
        try:
            root.relative_to(repository)
        except ValueError as error:
            raise ValueError(f"source root is outside Shapeless 3 checkout: {root}") from error
        selected.append(root)

    # This is the Compile source root of the `shapeless3-test` sbt project,
    # not a test fixture tree. Generic discovery excludes paths named `test`.
    test_support_root = repository / "modules/test/src/main/scala"
    if test_support_root.is_dir():
        test_support_root = test_support_root.resolve(strict=True)
        try:
            test_support_root.relative_to(repository)
        except ValueError as error:
            raise ValueError(
                f"Shapeless 3 test-support source root is outside checkout: {test_support_root}"
            ) from error
        selected.append(test_support_root)

    return sorted(set(selected))


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


def zio_scala3_production_roots(
    repository: pathlib.Path, source_roots: list[pathlib.Path]
) -> list[pathlib.Path]:
    """Exclude ZIO's mdoc project, which does not build with Scala 3."""
    repository = repository.resolve(strict=True)
    selected = []
    for root in source_roots:
        root = root.resolve(strict=True)
        try:
            relative = root.relative_to(repository)
        except ValueError as error:
            raise ValueError(f"source root is outside ZIO checkout: {root}") from error
        if relative.parts and relative.parts[0] in ZIO_NON_SCALA3_PROJECTS:
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
