#!/usr/bin/env python3
"""Verify all high-fanout roots in a three-run focused classloader profile."""

from __future__ import annotations

import argparse
import csv
import re
from pathlib import Path

from fanout_policy import (
    CURRENT_SOURCE_SHA,
    SCALA_SOURCE_SHA,
    serialize_profile_record,
    verify_high_fanout_profiles,
)


def read_tsv(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as stream:
        return list(csv.DictReader(stream, delimiter="\t"))


def verify_run(profile_dir: Path, run: str):
    roots = read_tsv(profile_dir / "request-roots.tsv")
    edges = read_tsv(profile_dir / "dependency-edges.tsv")
    classes = read_tsv(profile_dir / "request-classes.tsv")
    high_roots = [row for row in roots if row["run"] == run and int(row["full_completions"]) > 16]
    lines = []
    for root in high_roots:
        method = root["method"]
        edge_rows = [
            row for row in edges
            if row["run"] == run and row["method"] == method and row["root"] == root["root"]
        ]
        class_rows = [
            row for row in classes
            if row["run"] == run and row["method"] == method and row["root"] == root["root"]
        ]
        lines.append(serialize_profile_record(root, edge_rows, class_rows))
    return verify_high_fanout_profiles(lines, len(high_roots))


def verify_profile_dir(profile_dir: Path):
    summary_path = profile_dir / "summary.md"
    summary = summary_path.read_text(encoding="utf-8")
    if not re.search(rf"dotty-rs commit: `{CURRENT_SOURCE_SHA}`(?:\s|$)", summary):
        raise ValueError(f"profile is not measured at pinned source {CURRENT_SOURCE_SHA}")
    if SCALA_SOURCE_SHA not in summary or "Scala artifacts: `3.9.0`" not in summary:
        raise ValueError("profile does not use the pinned Scala 3.9.0 source and artifacts")
    if "JDK: `21.0.8` at" not in summary:
        raise ValueError("profile does not use the pinned JDK 21.0.8")

    runs = [verify_run(profile_dir, str(run)) for run in range(1, 4)]
    if runs[0] != runs[1] or runs[1] != runs[2]:
        raise ValueError("verified high-fanout profiles differ across the three runs")
    return runs[0]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile_dir", type=Path)
    args = parser.parse_args()
    try:
        profiles = verify_profile_dir(args.profile_dir)
    except (OSError, KeyError, ValueError) as error:
        parser.error(str(error))
    print(
        f"Verified {len(profiles)} pinned high-fanout profiles; "
        f"raw maximum completions {max(p.completed for p in profiles)}."
    )
    for profile in profiles:
        print(
            f"  {profile.method}: raw={profile.completed}; "
            f"hierarchy={profile.proven_hierarchy}; unexpected={profile.unexpected}; "
            f"depth={profile.depth}; identities={profile.identities}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
