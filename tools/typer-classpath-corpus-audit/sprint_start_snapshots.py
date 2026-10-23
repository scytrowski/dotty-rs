#!/usr/bin/env python3
"""Regenerate the compact sprint-start gate and target movement sidecars."""

from __future__ import annotations

import argparse
import collections
import gzip
import hashlib
from pathlib import Path
import re
import sys


TARGET_PATTERN = re.compile(r"^      (?:target=)?(.*?)::outcome=(.*?)=(\d+)$")


def lines(path: Path):
    opener = gzip.open if path.suffix == ".gz" else open
    with opener(path, "rt", encoding="utf-8") as stream:
        yield from stream


def extract(path: Path) -> tuple[int, dict[str, collections.Counter[str]]]:
    section = ""
    class_successes = None
    targets: dict[str, collections.Counter[str]] = collections.defaultdict(collections.Counter)
    for line in lines(path):
        line = line.rstrip("\n")
        if re.fullmatch(r"[a-zA-Z_0-9]+:", line):
            section = line[:-1]
            continue
        if section == "resolver_metrics":
            match = re.match(r"^  external_class_symbol_successes=(\d+)(?:\s|$)", line)
            if match:
                if class_successes is not None:
                    raise ValueError(f"duplicate class-success metric in {path}")
                class_successes = int(match[1])
        if section != "resolver_observations_by_enclosing_method":
            continue
        match = TARGET_PATTERN.match(line)
        if match:
            target, outcome, count = match.groups()
            targets[target][outcome] += int(count)
    if class_successes is None or not targets:
        raise ValueError(f"report lacks resolver metrics or request outcomes: {path}")
    return class_successes, targets


def write_gate(path: Path, sha: str, class_successes: int, targets) -> None:
    successful = sorted(
        target for target, outcomes in targets.items()
        if outcomes["external class success"] > 0
    )
    with path.open("w", encoding="utf-8", newline="") as stream:
        stream.write("kind\tkey\tvalue\n")
        stream.write(f"metric\tsprint_start_commit\t{sha}\n")
        stream.write(f"metric\texternal_class_symbol_successes\t{class_successes}\n")
        key_sha = hashlib.sha256(("\n".join(successful) + "\n").encode()).hexdigest()
        stream.write(f"metric\tsuccessful_target_keys_sha256\t{key_sha}\n")
        for target in successful:
            stream.write(f"successful_target_key\t{target}\t1\n")


def write_movement(path: Path, before, after) -> None:
    with path.open("w", encoding="utf-8", newline="") as stream:
        stream.write("target_key\tsprint_start_outcomes\tcurrent_outcomes\tsprint_start_success_count\tcurrent_success_count\n")
        for target in sorted(before.keys() | after.keys()):
            old = before.get(target, collections.Counter())
            new = after.get(target, collections.Counter())
            old_text = " | ".join(f"{key}={old[key]}" for key in sorted(old))
            new_text = " | ".join(f"{key}={new[key]}" for key in sorted(new))
            stream.write(
                f"{target}\t{old_text}\t{new_text}\t"
                f"{old['external class success']}\t{new['external class success']}\n"
            )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline_report", type=Path)
    parser.add_argument("current_report", type=Path)
    parser.add_argument("--baseline-sha", required=True)
    parser.add_argument("--gate-output", required=True, type=Path)
    parser.add_argument("--movement-output", required=True, type=Path)
    args = parser.parse_args()
    try:
        base_count, base_targets = extract(args.baseline_report)
        current_count, current_targets = extract(args.current_report)
        write_gate(args.gate_output, args.baseline_sha, base_count, base_targets)
        write_movement(args.movement_output, base_targets, current_targets)
    except (OSError, ValueError) as error:
        print(f"sprint-start snapshot: {error}", file=sys.stderr)
        return 1
    base_keys = {key for key, counts in base_targets.items() if counts["external class success"]}
    current_keys = {key for key, counts in current_targets.items() if counts["external class success"]}
    print(
        f"baseline: {len(base_keys)} successful keys, {base_count} class successes; "
        f"current: {len(current_keys)} successful keys, {current_count} class successes; "
        f"retained={len(base_keys & current_keys)}, missing={len(base_keys - current_keys)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
