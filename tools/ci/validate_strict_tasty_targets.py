#!/usr/bin/env python3
"""Validate the frozen strict Scala/TASTy test inventory and shard proposal."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


class ManifestError(ValueError):
    """The target manifest is malformed or does not cover real test targets."""


def _duplicates(values: list[str]) -> list[str]:
    seen: set[str] = set()
    repeated: set[str] = set()
    for value in values:
        if value in seen:
            repeated.add(value)
        seen.add(value)
    return sorted(repeated)


def validate_manifest(manifest: dict[str, Any], repository_root: Path) -> None:
    targets = manifest.get("targets")
    shards = manifest.get("sharding", {}).get("shards")
    expected_count = manifest.get("baseline", {}).get("inventory_count")
    shard_count = manifest.get("sharding", {}).get("shard_count")
    if not isinstance(targets, list) or not isinstance(shards, list):
        raise ManifestError("targets and sharding.shards must be arrays")
    if not isinstance(expected_count, int) or expected_count <= 0:
        raise ManifestError("baseline.inventory_count must be a positive integer")
    if not isinstance(shard_count, int) or shard_count != len(shards):
        raise ManifestError("sharding.shard_count must match the shard list")

    names: list[str] = []
    durations: dict[str, float] = {}
    for target in targets:
        if not isinstance(target, dict) or not isinstance(target.get("name"), str):
            raise ManifestError("each target must have a string name")
        name = target["name"]
        if not name or Path(name).name != name:
            raise ManifestError(f"invalid Cargo test target name: {name!r}")
        duration = target.get("measured_seconds")
        if not isinstance(duration, (int, float)) or duration < 0:
            raise ManifestError(f"target {name!r} needs a non-negative measured_seconds value")
        names.append(name)
        durations[name] = float(duration)

    duplicates = _duplicates(names)
    if duplicates:
        raise ManifestError(f"duplicate inventory targets: {', '.join(duplicates)}")
    if len(names) != expected_count:
        raise ManifestError(f"inventory has {len(names)} targets; expected {expected_count}")

    assignments: list[str] = []
    shard_ids: list[str] = []
    for shard in shards:
        if not isinstance(shard, dict) or not isinstance(shard.get("id"), str):
            raise ManifestError("each shard must have a string id")
        shard_ids.append(shard["id"])
        shard_targets = shard.get("targets")
        if not isinstance(shard_targets, list) or any(not isinstance(name, str) for name in shard_targets):
            raise ManifestError(f"shard {shard['id']!r} targets must be an array of names")
        assignments.extend(shard_targets)
    repeated_assignments = _duplicates(assignments)
    if repeated_assignments:
        raise ManifestError(f"targets assigned more than once: {', '.join(repeated_assignments)}")
    if len(set(shard_ids)) != len(shard_ids):
        raise ManifestError("shard ids must be unique")

    missing = sorted(set(names) - set(assignments))
    unknown = sorted(set(assignments) - set(names))
    if missing or unknown:
        details = []
        if missing:
            details.append(f"unassigned targets: {', '.join(missing)}")
        if unknown:
            details.append(f"unknown assigned targets: {', '.join(unknown)}")
        raise ManifestError("; ".join(details))

    for shard in shards:
        measured_total = round(sum(durations[name] for name in shard["targets"]), 2)
        declared_total = shard.get("estimated_test_seconds")
        if not isinstance(declared_total, (int, float)) or round(float(declared_total), 2) != measured_total:
            raise ManifestError(f"shard {shard['id']!r} estimated_test_seconds does not match its target measurements")

    missing_files = [name for name in names if not (repository_root / "tests" / f"{name}.rs").is_file()]
    if missing_files:
        raise ManifestError(f"missing tests/<target>.rs files: {', '.join(missing_files)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("tools/ci/strict_tasty_targets.json"))
    parser.add_argument("--root", type=Path, default=Path.cwd())
    args = parser.parse_args()
    try:
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
        validate_manifest(manifest, args.root)
    except (OSError, json.JSONDecodeError, ManifestError) as error:
        parser.error(str(error))
    print(f"validated {manifest['baseline']['inventory_count']} strict TASTy targets across {manifest['sharding']['shard_count']} proposed shards")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
