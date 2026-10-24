#!/usr/bin/env python3
"""Validate the bounded Scala 3.9 List fanout profile artifacts."""

from __future__ import annotations

import argparse
import csv
from collections import Counter
from pathlib import Path

EXPECTED = {
    "library/src/scala/collection/mutable/ListMap.scala#tree=146::scala.collection.mutable.ListMap.addOne",
    "library/src/scala/collection/mutable/ListBuffer.scala#tree=1088::scala.collection.mutable.ListBuffer.removeAfter",
    "library/src/scala/collection/concurrent/TrieMap.scala#tree=2721::scala.collection.concurrent.LNode.inserted",
    "library/src/scala/collection/concurrent/TrieMap.scala#tree=2780::scala.collection.concurrent.LNode.removed",
}
EXPECTED_METRICS = {
    "root": "scala/collection/immutable/List",
    "depth": "7",
    "distinct_classes": "63",
    "full_completions": "29",
    "shells_created": "30",
    "shells_reused": "154",
    "member_type_references": "300",
    "hierarchy_loads": "81",
    "completed_cache_hits": "53",
}


def validate_rows(roots: list[dict[str, str]], edges: list[dict[str, str]], classes: list[dict[str, str]]) -> None:
    expected_pairs = {(str(run), method) for run in range(1, 4) for method in EXPECTED}
    list_roots = [row for row in roots if row["root"] == EXPECTED_METRICS["root"]]
    actual_pairs = {(row["run"], row["method"]) for row in list_roots}
    if actual_pairs != expected_pairs or len(list_roots) != len(expected_pairs):
        raise ValueError("profile must contain each of the four exact List requests in all three runs")
    for row in list_roots:
        if any(row.get(key) != value for key, value in EXPECTED_METRICS.items()):
            raise ValueError(f"unexpected fanout metrics for {row['method']} in run {row['run']}")
        if "namespace=type::name=List::space=Prefix::List" not in row["request"]:
            raise ValueError(f"request is not the expected immutable.List type prefix: {row['method']}")

    edge_groups: dict[tuple[str, str], list[dict[str, str]]] = {}
    for row in edges:
        if row["root"] != EXPECTED_METRICS["root"]:
            continue
        edge_groups.setdefault((row["run"], row["method"]), []).append(row)
    if set(edge_groups) != expected_pairs:
        raise ValueError("dependency-edge profile is missing one or more exact requests")
    reference_edges = None
    for pair, rows in edge_groups.items():
        edge_set = {
            (row["reason"], row["owner"], row["dependency"], row["requests"])
            for row in rows
        }
        if reference_edges is None:
            reference_edges = edge_set
        elif edge_set != reference_edges:
            raise ValueError(f"dependency graph differs for {pair[1]} in run {pair[0]}")
        hierarchy = [row for row in rows if row["reason"] == "hierarchy"]
        member = [row for row in rows if row["reason"] == "member_type_identity"]
        if len(hierarchy) != 81 or sum(int(row["requests"]) for row in hierarchy) != 81:
            raise ValueError(f"unexpected inheritance graph for {pair[1]} in run {pair[0]}")
        if len(member) != 111 or sum(int(row["requests"]) for row in member) != 435:
            raise ValueError(f"unexpected member-identity graph for {pair[1]} in run {pair[0]}")
        if len(rows) != 192:
            raise ValueError(f"unexpected dependency reasons for {pair[1]} in run {pair[0]}")

    class_groups: dict[tuple[str, str], list[dict[str, str]]] = {}
    for row in classes:
        if row["root"] == EXPECTED_METRICS["root"]:
            class_groups.setdefault((row["run"], row["method"]), []).append(row)
    if set(class_groups) != expected_pairs:
        raise ValueError("class-identity profile is missing one or more exact requests")
    reference_classes = None
    for pair, rows in class_groups.items():
        class_set = {
            (row["class"], row["identity_shell_seen"], row["fully_completed"])
            for row in rows
        }
        if reference_classes is None:
            reference_classes = class_set
        elif class_set != reference_classes:
            raise ValueError(f"visited class identities differ for {pair[1]} in run {pair[0]}")
        if len(class_set) != 63:
            raise ValueError(f"expected 63 visited classes for {pair[1]} in run {pair[0]}")
        if sum(int(row["fully_completed"]) for row in rows) != 29:
            raise ValueError(f"expected 29 completed classes for {pair[1]} in run {pair[0]}")
        if sum(int(row["identity_shell_seen"]) for row in rows) != 33:
            raise ValueError(f"expected 33 identities observed as shells for {pair[1]} in run {pair[0]}")


def validate_profile(profile_dir: Path) -> None:
    def read(name: str) -> list[dict[str, str]]:
        with (profile_dir / name).open(encoding="utf-8", newline="") as stream:
            return list(csv.DictReader(stream, delimiter="\t"))

    roots = read("request-roots.tsv")
    edges = read("dependency-edges.tsv")
    classes = read("request-classes.tsv")
    validate_rows(roots, edges, classes)

    docs = Path(__file__).resolve().parents[2] / "docs"
    with (docs / "classloader-list-fanout-3.9.0-edges.tsv").open(encoding="utf-8", newline="") as stream:
        expected_edges = Counter(
            (row["source_method"], row["reason"], row["owner"], row["dependency"], row["requests"])
            for row in csv.DictReader(stream, delimiter="\t")
        )
    with (docs / "classloader-list-fanout-3.9.0-classes.tsv").open(encoding="utf-8", newline="") as stream:
        expected_classes = Counter(
            (row["source_method"], row["class"], row["identity_shell_seen"], row["fully_completed"])
            for row in csv.DictReader(stream, delimiter="\t")
        )
    for run in {row["run"] for row in roots if row["root"] == EXPECTED_METRICS["root"]}:
        run_edges = Counter(
            (row["method"], row["reason"], row["owner"], row["dependency"], row["requests"])
            for row in edges if row["run"] == run and row["root"] == EXPECTED_METRICS["root"]
        )
        run_classes = Counter(
            (row["method"], row["class"], row["identity_shell_seen"], row["fully_completed"])
            for row in classes if row["run"] == run and row["root"] == EXPECTED_METRICS["root"]
        )
        if run_edges != expected_edges:
            raise ValueError(f"dependency graph changed from the checked-in evidence in run {run}")
        if run_classes != expected_classes:
            raise ValueError(f"visited class identities changed from the checked-in evidence in run {run}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile_dir", type=Path)
    args = parser.parse_args()
    try:
        validate_profile(args.profile_dir)
    except (OSError, KeyError, ValueError) as error:
        parser.error(str(error))
    print("Validated four immutable.List requests across three runs: 29 completed, 63 visited, 81 hierarchy edges.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
