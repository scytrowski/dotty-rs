#!/usr/bin/env python3
"""Turn pinned import-prefix observations into sorted classpath evidence."""

from __future__ import annotations

import argparse
import csv
import io
import json
import os
import re
import sys
import zipfile
from collections import defaultdict
from pathlib import Path


OBSERVATION_HEADER = (
    "source_file\tmethod\tblocker_origin\timport_tree\tline\tqualifier_source\t"
    "qualifier_segments\tcontext_owner\tcontext_package\tnamespace\tsource_lookup\t"
    "source_package_registry_lookup\tresolved_prefix\tfirst_failing_segment\t"
    "request_kind\tresolver_outcome\ttarget_key"
)


def extract_observations(report: str) -> list[dict[str, str]]:
    lines = report.splitlines()
    try:
        start = lines.index("import_prefix_observations:")
    except ValueError as error:
        raise ValueError("report has no import_prefix_observations section") from error
    if start + 2 >= len(lines) or not lines[start + 2].strip().startswith("source_file\t"):
        raise ValueError("import-prefix observation header is missing")
    rows = []
    for line in lines[start + 3 :]:
        if not line.startswith("  "):
            break
        columns = line.strip().split("\t")
        if len(columns) != 17:
            raise ValueError(f"expected 17 import-prefix columns, got {len(columns)}")
        rows.append(dict(zip(OBSERVATION_HEADER.split("\t"), columns, strict=True)))
    return rows


def logical_resources(archive: Path, release: int) -> tuple[set[str], dict[str, int], set[str]]:
    """Return effective .class/.tasty paths and MR versions for one JAR/JMOD."""
    with zipfile.ZipFile(archive) as zipped:
        manifest = next(
            (name for name in zipped.namelist() if name.lower() == "meta-inf/manifest.mf"),
            None,
        )
        multi_release = False
        if manifest:
            text = zipped.read(manifest).decode("utf-8", errors="replace")
            multi_release = bool(re.search(r"(?im)^Multi-Release:\s*true\s*$", text))
        selected: dict[str, tuple[int, str]] = {}
        directories: set[str] = set()
        is_jmod = archive.suffix.lower() == ".jmod"
        for raw in zipped.namelist():
            name = raw
            version = 0
            if is_jmod:
                if not name.startswith("classes/"):
                    continue
                name = name[len("classes/") :]
                if name.endswith("/"):
                    directories.add(name.rstrip("/"))
                    continue
            elif name.startswith("META-INF/versions/"):
                match = re.match(r"META-INF/versions/(\d+)/(.*)$", name)
                if not (multi_release and match):
                    continue
                version = int(match.group(1))
                if version > release:
                    continue
                name = match.group(2)
            elif name.startswith("META-INF/"):
                continue
            if name.endswith("/"):
                directories.add(name.rstrip("/"))
                continue
            if not name.endswith((".class", ".tasty")):
                continue
            old = selected.get(name)
            if old is None or version > old[0]:
                selected[name] = (version, raw)
        return set(selected), {path: item[0] for path, item in selected.items()}, directories


def package_resource_evidence(
    segments: list[str],
    roots: list[tuple[str, set[str], dict[str, int], set[str]]],
) -> dict[str, object]:
    package_path = "/".join(segments)
    direct: list[tuple[str, str, int]] = []
    descendants: list[tuple[str, str, int, int]] = []
    directory_evidence = []
    for label, resources, versions, directories in roots:
        prefix = package_path + "/" if package_path else ""
        for resource in resources:
            if not resource.startswith(prefix):
                continue
            suffix = resource[len(prefix) :]
            depth = suffix.count("/")
            version = versions.get(resource, 0)
            if depth == 0:
                direct.append((label, resource, version))
            else:
                descendants.append((label, resource, depth, version))
        if package_path in directories:
            directory_evidence.append(label)
    return {
        "direct": sorted(set(direct)),
        "descendants": sorted(set(descendants)),
        "directory_only": bool(directory_evidence) and not direct and not descendants,
    }


def read_classpath_evidence(text: str) -> dict[tuple[str, str], dict[str, str]]:
    reader = csv.DictReader(io.StringIO(text), delimiter="\t")
    return {
        (row["target_key"], row["prefix"]): row
        for row in reader
    }


def failed_prefix(row: dict[str, str]) -> list[str]:
    resolved = row["resolved_prefix"]
    prefix = [] if resolved in ("", "-") else resolved.split(".")
    failed = row["first_failing_segment"]
    if failed not in ("", "-", "<unknown>"):
        prefix.extend(failed.split("."))
    return prefix


def source_package_candidates(row: dict[str, str]) -> set[str]:
    lookup = row["source_package_registry_lookup"]
    candidates = set()
    relative = lookup.partition("relative=")[2].split(";", 1)[0]
    if "@" in relative:
        candidates.add(relative.rsplit("@", 1)[1])
    enclosing = lookup.partition("enclosing=")[2].split(";root=", 1)[0]
    for candidate in enclosing.split(","):
        if "@" in candidate:
            candidates.add(candidate.rsplit("@", 1)[1])
    return candidates


def ordered_source_package_candidates(row: dict[str, str]) -> list[dict[str, str]]:
    """Decode source registry probes without losing lexical precedence."""
    lookup = row["source_package_registry_lookup"]
    candidates: list[dict[str, str]] = []

    def add(origin: str, probe: str) -> None:
        status, separator, path = probe.rpartition("@")
        if not separator or not path:
            return
        if "=" in status:
            status = status.rsplit("=", 1)[1]
        candidates.append(
            {
                "origin": origin,
                "path": path,
                "registry": status,
                "next_segment_lookup": status,
            }
        )

    relative = lookup.partition("relative=")[2].split(";", 1)[0]
    add("current", relative)
    enclosing = lookup.partition("enclosing=")[2].split(";root=", 1)[0]
    for probe in enclosing.split(","):
        add("enclosing", probe)
    root = lookup.partition(";root=")[2]
    root_registry = root.split("@", 1)[0] if "@" in root else root or "unknown"
    candidates.append(
        {
            "origin": "root",
            "path": "<root>",
            "registry": root_registry,
            "next_segment_lookup": root or "unknown",
        }
    )
    seen = set()
    ordered = []
    for candidate in candidates:
        key = candidate["path"]
        if key not in seen:
            seen.add(key)
            ordered.append(candidate)
    return ordered


def render_lexical_candidate_evidence(
    observations: list[dict[str, str]],
    roots: list[tuple[str, set[str], dict[str, int], set[str]]],
    snapshot: str,
    families: tuple[str, ...] = ("core", "ast", "transform", "util", "io"),
    classpath_evidence: dict[tuple[str, str], dict[str, str]] | None = None,
    include_header: bool = True,
    allow_missing_families: bool = False,
) -> str:
    """Render stable representative rows without merging snapshot denominators."""
    output = io.StringIO()
    writer = csv.writer(output, delimiter="\t", lineterminator="\n")
    if include_header:
        writer.writerow(
            [
            "snapshot",
            "target_family",
            "source_file",
            "import_tree",
            "line",
            "context_owner",
            "source_context_owners_in_priority_order",
            "context_package",
            "requested_segment",
            "qualifier_source",
            "source_lookup",
            "candidates_priority_order",
            "collision_marker",
            "ambiguous_marker",
            "unsupported_marker",
            "resolver_outcome",
            ]
        )
    for family in families:
        rows = [
            row
            for row in observations
            if row["target_key"].split("::", 1)[0] == family
        ]
        rows.sort(key=lambda row: (row["source_file"], row["import_tree"], row["method"]))
        representatives = []
        seen_files = set()
        for row in rows:
            if row["source_file"] in seen_files:
                continue
            seen_files.add(row["source_file"])
            representatives.append(row)
            if len(representatives) == 2:
                break
        if not representatives:
            if allow_missing_families:
                continue
            raise ValueError(f"{snapshot} snapshot has no observations for {family}")
        for row in representatives:
            candidates = ordered_source_package_candidates(row)
            for candidate in candidates:
                package_path = candidate["path"]
                probe_path = (
                    row["first_failing_segment"]
                    if package_path == "<root>"
                    else package_path
                )
                path_segments = [] if not probe_path else probe_path.split(".")
                candidate["classpath_probe"] = probe_path or "<root>"
                evidence = package_resource_evidence(path_segments, roots)
                direct = evidence["direct"]
                descendants = evidence["descendants"]
                frozen = (classpath_evidence or {}).get((row["target_key"], probe_path))
                if frozen:
                    is_direct = frozen["contains_package_direct"] == "True"
                    is_descendant = frozen["descendant_only"] == "True"
                    candidate["classpath"] = (
                        "direct" if is_direct else "descendant" if is_descendant else "absent"
                    )
                    candidate["classpath_resources"] = sorted(
                        set(frozen["direct_origins"].split(";"))
                        | set(frozen["descendant_resources"].split(";"))
                    )
                    candidate["classpath_resources"] = [
                        item for item in candidate["classpath_resources"] if item != "-"
                    ][:5]
                else:
                    candidate["classpath"] = (
                        "direct"
                        if direct
                        else "descendant"
                        if descendants
                        else "absent"
                    )
                    candidate["classpath_resources"] = sorted(
                        {
                            f"{origin}:{resource}"
                            for origin, resource, _ in direct
                        }
                        | {
                            f"{origin}:{resource}"
                            for origin, resource, _, _ in descendants
                        }
                    )[:5]
                candidate["source_registry_known"] = "found:Package" in candidate["registry"]
            overlap = any(
                "found:Package" in candidate["next_segment_lookup"]
                and candidate["classpath"] != "absent"
                for candidate in candidates
            )
            lookup_text = row["source_lookup"] + " " + row["source_package_registry_lookup"]
            outcome = row["resolver_outcome"].lower()
            writer.writerow(
                [
                    snapshot,
                    family,
                    row["source_file"],
                    row["import_tree"],
                    row["line"],
                    row["context_owner"],
                    json.dumps(
                        [
                            row["context_owner"],
                            *[
                                probe.partition("=")[0]
                                for probe in row["source_package_registry_lookup"]
                                .partition("enclosing=")[2]
                                .split(";root=", 1)[0]
                                .split(",")
                                if probe and probe != "none"
                            ],
                        ],
                        separators=(",", ":"),
                    ),
                    row["context_package"],
                    row["first_failing_segment"],
                    row["qualifier_source"],
                    row["source_lookup"],
                    json.dumps(candidates, sort_keys=True, separators=(",", ":")),
                    "source_classpath_overlap" if overlap else "not_observed",
                    "ambiguous" if "ambiguous" in lookup_text.lower() else "not_observed",
                    "unsupported" if "unsupported" in outcome else "not_observed",
                    row["resolver_outcome"],
                ]
            )
    return output.getvalue()


def classify_target(
    rows: list[dict[str, str]],
    roots: list[tuple[str, set[str], dict[str, int], set[str]]],
) -> str:
    categories = set()
    for row in rows:
        lookup = row["source_lookup"]
        if "found:Package" in row["source_package_registry_lookup"]:
            categories.add("source_package_scope_resolver_registry_gap")
        if lookup.startswith("found:Package"):
            categories.add("source_package_scope")
        elif lookup.startswith("found:"):
            categories.add("qualifier_is_non_package_symbol")
        if row["qualifier_segments"].split(".", 1)[0] == "_root_":
            categories.add("root_qualified")
        if row["context_package"] not in ("-", "<root-or-unavailable>", "<unavailable>"):
            if not row["qualifier_segments"].startswith("_root_."):
                categories.add("source_relative_candidate")
        if lookup in ("unavailable", "<unavailable>") or "ambiguous" in lookup:
            categories.add("unknown_or_ambiguous_source_lookup")
    failed_evidence = []
    for row in rows:
        prefixes = {".".join(failed_prefix(row)), *source_package_candidates(row)}
        for prefix in prefixes:
            if prefix:
                failed_evidence.append(package_resource_evidence(prefix.split("."), roots))
    if any(item["direct"] for item in failed_evidence):
        categories.add("direct_class_or_tasty_evidence")
    if any(item["descendants"] and not item["direct"] for item in failed_evidence):
        categories.add("descendant_only_intermediate")
    if not any(item["direct"] or item["descendants"] for item in failed_evidence):
        categories.add("absent_from_configured_classpath")
    return ",".join(sorted(categories)) if categories else "unknown_or_ambiguous"


def render_profile(
    observations: list[dict[str, str]],
    roots: list[tuple[str, set[str], dict[str, int], set[str]]],
) -> tuple[str, str, dict[str, int]]:
    grouped: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in observations:
        grouped[row["target_key"]].append(row)
    summary = io.StringIO()
    summary_writer = csv.writer(summary, delimiter="\t", lineterminator="\n")
    summary_writer.writerow([
        "target_key", "weighted_observations", "files", "direct_blockers",
        "inherited_blockers", "unknown_origin_blockers", "unique_blocker_origins",
        "source_lookups", "source_package_registry_lookups", "classification",
    ])
    detail = io.StringIO()
    detail_writer = csv.writer(detail, delimiter="\t", lineterminator="\n")
    detail_writer.writerow(
        ["target_key", "prefix", "contains_package_direct", "descendant_only", "direct_origins", "descendant_resources", "effective_mr_versions", "directory_only"]
    )
    categories: dict[str, int] = defaultdict(int)
    for key in sorted(grouped):
        rows = sorted(grouped[key], key=lambda row: tuple(row.values()))
        prefixes = set()
        for row in rows:
            segments = [
                segment for segment in row["qualifier_segments"].split(".")
                if segment and segment != "_root_"
            ]
            prefixes.update(".".join(segments[:length]) for length in range(1, len(segments) + 1))
            prefixes.update(source_package_candidates(row))
        for prefix in sorted(prefixes):
            package_segments = prefix.split(".")
            evidence = package_resource_evidence(package_segments, roots)
            direct = evidence["direct"]
            descendants = evidence["descendants"]
            origins = sorted({origin for origin, _, _ in direct})
            descendant_resources = sorted(
                {f"{origin}:{resource}" for origin, resource, _, _ in descendants}
            )
            versions = sorted(
                {f"{origin}:{version}" for origin, _, version in direct}
                | {f"{origin}:{version}" for origin, _, _, version in descendants}
            )
            detail_writer.writerow(
                [
                    key,
                    prefix,
                    bool(direct),
                    bool(descendants) and not bool(direct),
                    ";".join(origins) or "-",
                    ";".join(descendant_resources[:5]) or "-",
                    ";".join(versions) or "-",
                    evidence["directory_only"],
                ]
            )
        classification = classify_target(rows, roots)
        categories[classification] += 1
        blocker_origins = {row["blocker_origin"] for row in rows}
        direct_count = sum(row["blocker_origin"].startswith("direct:") for row in rows)
        inherited_count = sum(row["blocker_origin"].startswith("inherited:") for row in rows)
        unknown_count = len(rows) - direct_count - inherited_count
        summary_writer.writerow(
            [
                key,
                len(rows),
                len({row["source_file"] for row in rows}),
                direct_count,
                inherited_count,
                unknown_count,
                ";".join(sorted(blocker_origins)) or "-",
                ";".join(sorted({row["source_lookup"] for row in rows})) or "-",
                ";".join(
                    sorted({row["source_package_registry_lookup"] for row in rows})
                ) or "-",
                classification,
            ]
        )
    return summary.getvalue(), detail.getvalue(), dict(sorted(categories.items()))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=Path)
    parser.add_argument("--rows", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--classpath", required=True, help="path-separated pinned Scala JAR list")
    parser.add_argument("--jdk-jmods", required=True, type=Path)
    parser.add_argument("--jdk-release", required=True, type=int)
    parser.add_argument("--candidate-baseline", type=Path)
    parser.add_argument("--candidate-baseline-evidence", type=Path)
    parser.add_argument("--candidates", type=Path)
    args = parser.parse_args()
    observations = extract_observations(args.report.read_text(encoding="utf-8"))
    roots = []
    jmods = sorted(args.jdk_jmods.glob("*.jmod"))
    for jmod in jmods:
        resources, versions, directories = logical_resources(jmod, args.jdk_release)
        roots.append((f"JMOD:{jmod.name}", resources, versions, directories))
    for item in sorted(args.classpath.split(os.pathsep)):
        archive = Path(item)
        resources, versions, directories = logical_resources(archive, args.jdk_release)
        roots.append((f"JAR:{archive.name}", resources, versions, directories))
    summary, evidence, categories = render_profile(observations, roots)
    args.rows.write_text(summary, encoding="utf-8")
    args.evidence.write_text(evidence, encoding="utf-8")
    if args.candidates:
        if not args.candidate_baseline:
            parser.error("--candidates requires --candidate-baseline")
        baseline = extract_observations(
            args.candidate_baseline.read_text(encoding="utf-8")
        )
        baseline_evidence = (
            read_classpath_evidence(
                args.candidate_baseline_evidence.read_text(encoding="utf-8")
            )
            if args.candidate_baseline_evidence
            else None
        )
        args.candidates.write_text(
            render_lexical_candidate_evidence(
                baseline,
                roots,
                "frozen-1128",
                classpath_evidence=baseline_evidence,
            )
            + render_lexical_candidate_evidence(
                observations,
                roots,
                "current-1180",
                classpath_evidence=read_classpath_evidence(evidence),
                include_header=False,
                allow_missing_families=True,
            ),
            encoding="utf-8",
        )
    targets = len({row["target_key"] for row in observations})
    files = len({row["source_file"] for row in observations})
    direct = sum(row["blocker_origin"].startswith("direct:") for row in observations)
    inherited = sum(row["blocker_origin"].startswith("inherited:") for row in observations)
    unknown_origins = len(observations) - direct - inherited
    blocker_origins = {row["blocker_origin"] for row in observations}
    root_qualified = sum(
        row["qualifier_segments"].split(".", 1)[0] == "_root_" for row in observations
    )
    source_relative = sum(
        row["context_package"] not in ("-", "<root-or-unavailable>", "<unavailable>")
        and not row["qualifier_segments"].startswith("_root_.")
        for row in observations
    )
    outcomes = sorted({row["resolver_outcome"] for row in observations})
    source_package_hits = sum(
        "found:Package" in row["source_package_registry_lookup"]
        for row in observations
    )
    relative_package_hits = sum(
        "relative=found:Package" in row["source_package_registry_lookup"]
        for row in observations
    )
    enclosing_package_hits = sum(
        "found:Package"
        in row["source_package_registry_lookup"].split("enclosing=", 1)[-1].split(";root=", 1)[0]
        for row in observations
    )
    ranked_targets = defaultdict(list)
    for row in observations:
        ranked_targets[row["target_key"]].append(row)
    print(
        f"observations={len(observations)} files={files} target_keys={targets} "
        f"unique_blocker_origins={len(blocker_origins)} direct={direct} inherited={inherited} "
        f"unknown_origins={unknown_origins} root_qualified={root_qualified} "
        f"source_relative_candidates={source_relative} source_package_registry_hits={source_package_hits} "
        f"relative_package_registry_hits={relative_package_hits} "
        f"enclosing_package_registry_hits={enclosing_package_hits} roots={len(roots)} "
        f"classifications={categories} outcomes={outcomes}"
    )
    print("ranked_targets_by_weight:")
    for key, rows in sorted(
        ranked_targets.items(), key=lambda item: (-len(item[1]), item[0])
    ):
        source_package_hits = sum(
            "found:Package" in row["source_package_registry_lookup"]
            for row in rows
        )
        unique_origins = len({row["blocker_origin"] for row in rows})
        print(
            f"  {key}: observations={len(rows)} "
            f"files={len({row['source_file'] for row in rows})} "
            f"unique_blocker_origins={unique_origins} "
            f"source_package_registry_hits={source_package_hits}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
