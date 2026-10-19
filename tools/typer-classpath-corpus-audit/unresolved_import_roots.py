#!/usr/bin/env python3
"""Reconcile the frozen #1180 import-prefix cohort with pinned archives.

This is a read-only evidence collector. Resource presence is deliberately
reported separately from Scala module identity and resolver completion.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import importlib.util
import io
import json
import os
import sys
import zipfile
from pathlib import Path


HERE = Path(__file__).resolve().parent
PROFILE = HERE / "import_prefix_profile.py"
spec = importlib.util.spec_from_file_location("import_prefix_profile", PROFILE)
assert spec and spec.loader
profile = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = profile
spec.loader.exec_module(profile)


def nat(data: bytes, offset: int) -> tuple[int, int]:
    value = 0
    while offset < len(data):
        byte = data[offset]
        offset += 1
        value = (value << 7) | (byte & 0x7F)
        if byte & 0x80:
            return value, offset
    raise ValueError("unterminated TASTy base-128 integer")


def tasty_header(data: bytes) -> str:
    if len(data) < 4 or data[:4] != bytes((0x5C, 0xA1, 0xAB, 0x1F)):
        return "invalid-magic"
    major, offset = nat(data, 4)
    minor, offset = nat(data, offset)
    experimental, offset = nat(data, offset)
    return f"{major}.{minor}.{experimental}"


def archive_resources(archives: list[Path]) -> tuple[list[dict[str, object]], dict[str, list[tuple[str, str, bytes]]]]:
    ordered = []
    index: dict[str, list[tuple[str, str, bytes]]] = {}
    for archive in archives:
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        label = f"{archive.name}@sha256:{digest}"
        with zipfile.ZipFile(archive) as jar:
            for name in jar.namelist():
                if name.endswith("/") or name.startswith("META-INF/versions/"):
                    continue
                if archive.suffix.lower() == ".jmod":
                    if not name.startswith("classes/"):
                        continue
                    logical_name = name[len("classes/") :]
                else:
                    logical_name = name
                if logical_name.endswith((".class", ".tasty")):
                    index.setdefault(logical_name, []).append((label, name, jar.read(name) if name.endswith(".tasty") else b""))
        ordered.append({"path": archive.as_posix(), "sha256": digest})
    return ordered, index


def candidate_paths(row: dict[str, str]) -> list[tuple[str, str]]:
    candidates = profile.ordered_source_package_candidates(row)
    return [(item["origin"], (row["target_key"].split("::<", 1)[0] if item["path"] == "<root>" else item["path"]).replace(".", "/")) for item in candidates]


def scan(archive_paths: list[Path], observations: list[dict[str, str]]) -> tuple[str, str, dict[str, int]]:
    archives, resources = archive_resources(archive_paths)
    rows_out = io.StringIO()
    writer = csv.writer(rows_out, delimiter="\t", lineterminator="\n")
    writer.writerow(("snapshot", "source_file", "method", "line", "blocker_origin", "import_tree", "context_owner", "context_package", "target_key", "first_failing_segment", "request_kind", "ordered_candidate_evidence", "classification"))
    totals: dict[str, int] = {}
    by_target: dict[str, dict[str, object]] = {}
    for row in observations:
        found = []
        for origin, stem in candidate_paths(row):
            exact = []
            for suffix in (".tasty", ".class", "$.class"):
                for artifact, path, data in resources.get(stem + suffix, []):
                    version = tasty_header(data) if suffix == ".tasty" else "-"
                    exact.append({"origin": origin, "path": path, "archive": artifact, "tasty_version": version})
            if exact:
                found.extend(exact)
        key = row["target_key"]
        classification = "no_candidate_resource_in_supplied_classpath" if not found else "candidate_resource_present_identity_requires_semantic_oracle"
        totals[classification] = totals.get(classification, 0) + 1
        target = by_target.setdefault(key, {"observations": 0, "classification": classification, "evidence": found})
        target["observations"] = int(target["observations"]) + 1
        if classification != target["classification"]:
            target["classification"] = "mixed_candidate_resource_presence"
        if found:
            target["evidence"] = found
        writer.writerow(("#1180-frozen-1063", row["source_file"], row["method"], row["line"], row["blocker_origin"], row["import_tree"], row["context_owner"], row["context_package"], key, row["first_failing_segment"], row["request_kind"], json.dumps(found, sort_keys=True, separators=(",", ":")) or "[]", classification))

    summary = io.StringIO()
    summary.write("# Unresolved import-root resource scan\n\n")
    summary.write("This report scans the frozen #1180 rows against the explicitly supplied, ordered JAR list. A `.class` or `.tasty` hit is resource evidence only; this scan does not prove a Scala `object`, `ModuleClass`, member completion, or selected import.\n\n")
    summary.write("## Pinned archives\n\n")
    summary.write("| order | archive | SHA-256 |\n|---:|---|---|\n")
    for order, item in enumerate(archives, 1):
        summary.write(f"| {order} | `{Path(str(item['path'])).name}` | `{item['sha256']}` |\n")
    summary.write("\n## Frozen cohort\n\n")
    summary.write(f"Observations: {len(observations)}; files: {len({row['source_file'] for row in observations})}; target keys: {len({row['target_key'] for row in observations})}; direct blocker origins: {len({row['blocker_origin'] for row in observations if row['blocker_origin'].startswith('direct:')})}; inherited blocker origins: {len({row['blocker_origin'] for row in observations if row['blocker_origin'].startswith('inherited:')})}.\n\n")
    summary.write("## Resource classifications\n\n")
    for name, count in sorted(totals.items()):
        summary.write(f"- `{name}`: {count} observations\n")
    summary.write("\n## Target matrix\n\n| target | observations | resource result | earliest evidence |\n|---|---:|---|---|\n")
    for key, item in sorted(by_target.items()):
        found = item["evidence"]
        first = "-" if not found else f"`{found[0]['path']}` from `{found[0]['archive'].split('@sha256:')[0]}` (TASTy {found[0]['tasty_version']})"
        summary.write(f"| `{key}` | {item['observations']} | {item['classification']} | {first} |\n")
    summary.write("\nThe row-level TSV preserves every frozen row. `ordered_candidate_evidence` records each direct class/TASTy resource hit in lexical-candidate order, including archive digest and parsed TASTy header. No historical source or selector outcome is rewritten. The report does not count Scala ModuleClass requests, completions, or selected members; those must come from a resolver-level audit, not inferred from archive names.\n")
    return rows_out.getvalue(), summary.getvalue(), totals


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", type=Path, help="#1180 markdown profile with embedded frozen observations")
    parser.add_argument("--classpath", required=True, help="ordered path-separated pinned JARs")
    parser.add_argument("--rows", required=True, type=Path)
    parser.add_argument("--summary", required=True, type=Path)
    args = parser.parse_args()
    observations = profile.extract_observations(args.profile.read_text(encoding="utf-8"))
    if len(observations) != 1063:
        raise SystemExit(f"refusing non-frozen cohort: expected 1063 rows, got {len(observations)}")
    archives = [Path(value) for value in args.classpath.split(os.pathsep)]
    rows, summary, totals = scan(archives, observations)
    args.rows.write_text(rows, encoding="utf-8")
    args.summary.write_text(summary, encoding="utf-8")
    print(f"observations={len(observations)} target_keys={len({row['target_key'] for row in observations})} classifications={totals}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
