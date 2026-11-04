#!/usr/bin/env python3
"""Validate independently declared strict workflow targets and shard proposal."""

from __future__ import annotations

import argparse
import json
import re
import shlex
from pathlib import Path
from textwrap import dedent
from typing import Any


class ManifestError(ValueError):
    """The target manifest is malformed or does not cover real test targets."""


FROZEN_WORKFLOW = ".github/workflows/jvm-tasty-factory-scala-consumer.yml"
TARGET_DECLARATION = "STRICT_REQUIRED_TARGETS"
SUPPORT_DECLARATION = "STRICT_SUPPORT_TARGETS"
ENV_DECLARATION = "STRICT_REQUIRED_ENVS"


def _workflow_values(workflow: str, key: str) -> list[str]:
    pattern = rf"(?ms)^\s*{re.escape(key)}:\s*(?:>-\s*\n(.*?)(?=^\s*[A-Z][A-Z0-9_]*:|^\s*STRICT_TASTY_SHARD_1_JOB:)|([^\n]+))"
    match = re.search(pattern, workflow)
    if match is None:
        raise ManifestError(f"workflow is missing independent {key} declaration")
    values = (match.group(1) if match.group(1) is not None else match.group(2)).split()
    if not values or any(re.fullmatch(r"[A-Za-z0-9_]+", name) is None for name in values):
        raise ManifestError(f"workflow {key} declaration must contain valid names")
    if _duplicates(values):
        raise ManifestError(f"workflow {key} declaration contains duplicates")
    return values


def _validate_invocations(workflow: str, strict: list[str], support: list[str]) -> None:
    scripts = _workflow_run_scripts(workflow)
    allowed = set(strict) | set(support)
    literal: list[str] = []
    shard_array_invocations = 0
    shard_array_builders = 0

    for script in scripts:
        source = re.sub(r"\\\r?\n[ \t]*", " ", script)
        for line in source.splitlines():
            try:
                words = shlex.split(line, comments=True, posix=True)
            except ValueError as error:
                raise ManifestError(f"cannot parse workflow shell arguments: {error}") from error
            if not words:
                continue

            if any(word.startswith("test_args") for word in words):
                if words == ["test_args=()"]:
                    continue
                if words == ['test_args+=(--test', '$target)']:
                    shard_array_builders += 1
                else:
                    raise ManifestError("unsupported or dynamic manifest-driven Cargo test argument construction")

            cargo_positions = [index for index, word in enumerate(words) if word == "cargo"]
            if not cargo_positions:
                continue
            if cargo_positions != [0]:
                if any(word.startswith("$") or "$(" in word or "`" in word for word in words[cargo_positions[0] + 1:]):
                    raise ManifestError("unsupported dynamic or wrapped Cargo command construction")
                raise ManifestError("unsupported wrapped Cargo invocation in workflow shell")

            command = words[1:]
            if command and command[0].startswith("+"):
                command = command[1:]
            if command and (command[0].startswith("$") or "$(" in command[0] or "`" in command[0]):
                raise ManifestError(f"unsupported dynamic Cargo subcommand: {command[0]}")
            if not command or command[0] != "test":
                continue

            arguments = command[1:]
            index = 0
            while index < len(arguments):
                argument = arguments[index]
                if argument == "--":
                    break
                if argument == "${test_args[@]}":
                    shard_array_invocations += 1
                    index += 1
                    continue
                if argument.startswith("${") or argument.startswith("$") or "$(" in argument or "`" in argument:
                    raise ManifestError(f"unsupported dynamic Cargo test arguments: {argument}")
                if argument == "--test":
                    if index + 1 >= len(arguments):
                        raise ManifestError("Cargo --test option is missing its target")
                    target = arguments[index + 1]
                    if target.startswith("$") or "$(" in target or "`" in target:
                        raise ManifestError(f"unsupported dynamic Cargo --test target: {target}")
                    if re.fullmatch(r"[A-Za-z0-9_]+", target) is None:
                        raise ManifestError(f"unsupported Cargo --test target syntax: {target!r}")
                    literal.append(target)
                    index += 2
                    continue
                if argument.startswith("--test="):
                    target = argument[len("--test="):]
                    if target.startswith("$") or "$(" in target or "`" in target:
                        raise ManifestError(f"unsupported dynamic Cargo --test target: {target}")
                    if re.fullmatch(r"[A-Za-z0-9_]+", target) is None:
                        raise ManifestError(f"unsupported Cargo --test target syntax: {target!r}")
                    literal.append(target)
                index += 1

            if any(argument.startswith("${") and argument != "${test_args[@]}" for argument in arguments):
                raise ManifestError("unsupported dynamic Cargo test argument array")

    if shard_array_builders != 2 or shard_array_invocations != 2:
        raise ManifestError(
            "expected exactly two manifest-driven --test loops and Cargo invocations; "
            f"found {shard_array_builders} loop builders and {shard_array_invocations} invocations"
        )
    unknown = sorted(set(literal) - allowed)
    if unknown:
        raise ManifestError("required Cargo test invocation absent from workflow inventory/support declarations: " + ", ".join(unknown))
    duplicates = sorted(set(literal) & set(strict))
    if duplicates:
        raise ManifestError("strict targets invoked outside manifest-driven shards: " + ", ".join(duplicates))
    undeclared = sorted(set(literal) - set(strict) - set(support))
    if undeclared:
        raise ManifestError("undeclared Cargo test invocations: " + ", ".join(undeclared))


def _workflow_run_scripts(workflow: str) -> list[str]:
    """Extract inline and block `run` values without scanning YAML metadata/comments."""
    lines = workflow.splitlines()
    scripts: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        match = re.match(r"^(\s*)(?:-\s+)?run:\s*(.*)$", line)
        if match is None:
            if not line.lstrip().startswith("#") and re.search(r"\brun\s*:", line):
                raise ManifestError(f"unsupported workflow run key syntax: {line.strip()!r}")
            index += 1
            continue
        indent = len(match.group(1))
        value = match.group(2).strip()
        block_indicator = re.fullmatch(r"([|>])(?:[1-9])?(?:[+-])?|([|>])(?:[+-])?(?:[1-9])?", value)
        if block_indicator:
            index += 1
            block: list[str] = []
            while index < len(lines):
                candidate = lines[index]
                if candidate.strip() and len(candidate) - len(candidate.lstrip()) <= indent:
                    break
                block.append(candidate)
                index += 1
            scripts.append(dedent("\n".join(block)))
            continue
        if not value or value.startswith(("'", '"')) or value.startswith(("|", ">")):
            raise ManifestError(f"unsupported workflow run scalar syntax: {value!r}")
        scripts.append(value)
        index += 1
    if not scripts:
        raise ManifestError("workflow does not contain any shell run steps")
    return scripts

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

    workflow_path = repository_root / FROZEN_WORKFLOW
    try:
        workflow_text = workflow_path.read_text(encoding="utf-8")
    except OSError as error:
        raise ManifestError(f"cannot read frozen inventory workflow {source['workflow']!r}: {error}") from error
    workflow_names = _workflow_values(workflow_text, TARGET_DECLARATION)
    support_names = _workflow_values(workflow_text, SUPPORT_DECLARATION)
    required_envs = _workflow_values(workflow_text, ENV_DECLARATION)
    _validate_invocations(workflow_text, workflow_names, support_names)
    if len(workflow_names) != expected_count:
        raise ManifestError(f"workflow declares {len(workflow_names)} strict targets; manifest expects {expected_count}")

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
    if names != workflow_names:
        missing = sorted(set(workflow_names) - set(names))
        unexpected = sorted(set(names) - set(workflow_names))
        details = []
        if missing:
            details.append(f"omitted from manifest: {', '.join(missing)}")
        if unexpected:
            details.append(f"not in workflow inventory: {', '.join(unexpected)}")
        if not details:
            details.append("target order differs from workflow inventory")
        raise ManifestError("manifest does not match independently declared workflow inventory (" + "; ".join(details) + ")")

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

    env_blocks = re.findall(
        r"(?ms)^\s*-\s+name: Run manifest-driven strict Scala and TASTy shard\n\s+id: strict_tests\n\s+env:\n(.*?)(?=^\s+shell:|^\s+run:)",
        workflow_text,
    )
    if len(env_blocks) != 2:
        raise ManifestError(f"expected two strict shard env blocks; found {len(env_blocks)}")
    for index, env_block in enumerate(env_blocks, 1):
        actual_envs = re.findall(r"^\s*(DOTTY_REQUIRE_[A-Z0-9_]+):\s*'1'\s*$", env_block, re.MULTILINE)
        if sorted(actual_envs) != sorted(required_envs) or len(actual_envs) != len(set(actual_envs)):
            raise ManifestError(f"shard {index} strict env differs from independent requirements")

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
