#!/usr/bin/env python3
"""Reject JVM implementation dependencies in production frontend closures."""

from __future__ import annotations

import json
import subprocess
import sys
from collections import deque
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
FRONTEND_CRATES = (
    "dotty-core",
    "dotty-lexer",
    "dotty-parser",
    "dotty-namer",
    "dotty-typer",
)
FORBIDDEN_CRATES = frozenset(
    {"dotty-jvm", "dotty-classfile", "dotty-classloader"}
)


def _production_edge(dependency: dict[str, Any]) -> bool:
    """Keep normal and build edges; ignore dev-only edges."""
    kinds = dependency.get("dep_kinds", [])
    return not kinds or any(kind.get("kind") != "dev" for kind in kinds)


def find_violations(metadata: dict[str, Any]) -> list[str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    package_ids_by_name = {
        package["name"]: package["id"]
        for package in metadata["packages"]
        if package.get("source") is None
    }
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    violations: list[str] = []

    for root_name in FRONTEND_CRATES:
        root_id = package_ids_by_name.get(root_name)
        if root_id is None:
            violations.append(f"missing workspace package {root_name}")
            continue

        pending = deque([(root_id, [root_name])])
        visited = {root_id}
        while pending:
            package_id, path = pending.popleft()
            node = nodes.get(package_id)
            if node is None:
                violations.append(f"missing resolved node for {' -> '.join(path)}")
                continue

            for dependency in node.get("deps", []):
                if not _production_edge(dependency):
                    continue
                dependency_id = dependency["pkg"]
                package = packages.get(dependency_id)
                if package is None:
                    violations.append(
                        f"missing package metadata for {' -> '.join(path)}"
                    )
                    continue
                dependency_name = package["name"]
                dependency_path = [*path, dependency_name]
                if dependency_name in FORBIDDEN_CRATES:
                    violations.append(" -> ".join(dependency_path))
                if dependency_id not in visited:
                    visited.add(dependency_id)
                    pending.append((dependency_id, dependency_path))

    return sorted(set(violations))


def _fixture_metadata(*, inject_forbidden: bool) -> dict[str, Any]:
    names = (*FRONTEND_CRATES, "dotty-classloader", "dotty-classfile", "dotty-jvm")
    package_ids = {name: f"path+file:///fixture/{name}#0.1.0" for name in names}
    packages = [
        {"id": package_id, "name": name, "source": None}
        for name, package_id in package_ids.items()
    ]
    nodes_by_name: dict[str, dict[str, Any]] = {
        name: {"id": package_ids[name], "deps": []} for name in names
    }

    def edge(source: str, target: str, kind: str | None) -> None:
        nodes_by_name[source]["deps"].append(
            {"pkg": package_ids[target], "dep_kinds": [{"kind": kind}]}
        )

    # A real current-workspace pattern: test-only adapter dependencies are fine.
    edge("dotty-typer", "dotty-core", None)
    edge("dotty-typer", "dotty-classloader", "dev")
    if inject_forbidden:
        # The classfile edge is transitive and build-only, so it must be rejected.
        edge("dotty-core", "dotty-classfile", "build")

    return {
        "packages": packages,
        "resolve": {"nodes": list(nodes_by_name.values())},
    }


def self_test() -> None:
    allowed = find_violations(_fixture_metadata(inject_forbidden=False))
    if allowed:
        raise AssertionError(f"dev-only fixture dependency was rejected: {allowed}")

    rejected = find_violations(_fixture_metadata(inject_forbidden=True))
    expected = "dotty-typer -> dotty-core -> dotty-classfile"
    if expected not in rejected:
        raise AssertionError(f"injected build dependency was not rejected: {rejected}")


def main() -> int:
    try:
        self_test()
        result = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        violations = find_violations(json.loads(result.stdout))
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError, AssertionError) as error:
        print(f"frontend boundary check failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr, file=sys.stderr, end="")
        return 1

    if violations:
        print("Forbidden production dependency paths:", file=sys.stderr)
        for violation in violations:
            print(f"  {violation}", file=sys.stderr)
        return 1

    print("Frontend production dependency boundaries passed (self-test included).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
