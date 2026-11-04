#!/usr/bin/env python3
"""Validate the frozen strict Scala/TASTy test inventory and shard proposal."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


class ManifestError(ValueError):
    """The target manifest is malformed or does not cover real test targets."""


FROZEN_WORKFLOW = ".github/workflows/jvm-tasty-factory-scala-consumer.yml"
FROZEN_BASELINE = "tools/ci/strict_tasty_target_baseline.json"


def _frozen_inventory(repository_root: Path, baseline_path: Path | None = None) -> list[str]:
    path = baseline_path or (repository_root / FROZEN_BASELINE)
    try:
        baseline = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ManifestError(f"cannot read frozen target baseline {path}: {error}") from error
    if not isinstance(baseline, dict) or not isinstance(baseline.get("targets"), list):
        raise ManifestError("frozen baseline targets must be an array")
    targets = baseline["targets"]
    if any(not isinstance(name, str) or re.fullmatch(r"[a-zA-Z0-9_]+", name) is None for name in targets):
        raise ManifestError("frozen baseline must contain one valid target name per entry")
    if _duplicates(targets):
        raise ManifestError("frozen baseline contains duplicate targets")
    return targets


def _workflow_inventory(workflow: str) -> list[str]:
    match = re.search(r"(?ms)^\s*strict_targets=\(\s*(.*?)^\s*\)", workflow)
    if match is None:
        raise ManifestError("source workflow does not contain a strict_targets array")
    names = [line.strip() for line in match.group(1).splitlines() if line.strip()]
    if any(re.fullmatch(r"[a-zA-Z0-9_]+", name) is None for name in names):
        raise ManifestError("strict_targets array must contain one target name per line")
    return names


def _duplicates(values: list[str]) -> list[str]:
    seen: set[str] = set()
    repeated: set[str] = set()
    for value in values:
        if value in seen:
            repeated.add(value)
        seen.add(value)
    return sorted(repeated)


def validate_manifest(
    manifest: dict[str, Any], repository_root: Path, baseline_path: Path | None = None
) -> None:
    if not isinstance(manifest, dict):
        raise ManifestError("manifest root must be an object")
    source = manifest.get("source")
    targets = manifest.get("targets")
    sharding = manifest.get("sharding")
    baseline = manifest.get("baseline")
    if not isinstance(sharding, dict) or not isinstance(baseline, dict):
        raise ManifestError("baseline and sharding must be objects")
    shards = sharding.get("shards")
    expected_count = baseline.get("inventory_count")
    shard_count = sharding.get("shard_count")
    if not isinstance(targets, list) or not isinstance(shards, list):
        raise ManifestError("targets and sharding.shards must be arrays")
    if not isinstance(expected_count, int) or expected_count <= 0:
        raise ManifestError("baseline.inventory_count must be a positive integer")
    if not isinstance(shard_count, int) or shard_count != len(shards):
        raise ManifestError("sharding.shard_count must match the shard list")
    if not isinstance(source, dict) or source.get("workflow") != FROZEN_WORKFLOW:
        raise ManifestError(f"source.workflow must remain {FROZEN_WORKFLOW!r}")

    frozen_names = _frozen_inventory(repository_root, baseline_path)
    workflow_path = repository_root / FROZEN_WORKFLOW
    try:
        workflow_text = workflow_path.read_text(encoding="utf-8")
    except OSError as error:
        raise ManifestError(f"cannot read frozen inventory workflow {source['workflow']!r}: {error}") from error
    if re.search(r"(?m)^\s*strict_targets\s*=", workflow_text):
        workflow_names = _workflow_inventory(workflow_text)
        if _duplicates(workflow_names):
            raise ManifestError("source workflow strict_targets array contains duplicate targets")
        if workflow_names != frozen_names:
            raise ManifestError("source workflow inventory differs from the independent frozen baseline")
    if len(frozen_names) != expected_count:
        raise ManifestError(f"frozen baseline has {len(frozen_names)} targets; manifest expects {expected_count}")

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
    if names != frozen_names:
        missing = sorted(set(frozen_names) - set(names))
        unexpected = sorted(set(names) - set(frozen_names))
        details = []
        if missing:
            details.append(f"omitted from manifest: {', '.join(missing)}")
        if unexpected:
            details.append(f"not in frozen workflow inventory: {', '.join(unexpected)}")
        if not details:
            details.append("target order differs from the frozen workflow inventory")
        raise ManifestError("manifest does not match frozen workflow inventory (" + "; ".join(details) + ")")

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


def shard_targets(manifest: dict[str, Any], shard_id: str) -> list[str]:
    shards = manifest["sharding"]["shards"]
    matches = [shard for shard in shards if shard["id"] == shard_id]
    if len(matches) != 1:
        raise ManifestError(f"unknown shard {shard_id!r}; expected one of: {', '.join(s['id'] for s in shards)}")
    return matches[0]["targets"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("tools/ci/strict_tasty_targets.json"))
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--shard", help="print the validated ordered target names assigned to this shard")
    args = parser.parse_args()
    try:
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
        validate_manifest(manifest, args.root)
        if args.shard:
            expected_ids = ["shard-1", "shard-2"]
            actual_ids = [shard["id"] for shard in manifest["sharding"]["shards"]]
            if actual_ids != expected_ids:
                raise ManifestError(f"workflow requires exactly the ordered shard ids {expected_ids}; found {actual_ids}")
        selected = shard_targets(manifest, args.shard) if args.shard else None
    except (OSError, json.JSONDecodeError, ManifestError) as error:
        parser.error(str(error))
    if selected is not None:
        print("\n".join(selected))
    else:
        print(f"validated {manifest['baseline']['inventory_count']} strict TASTy targets across {manifest['sharding']['shard_count']} proposed shards")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
