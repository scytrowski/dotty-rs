#!/usr/bin/env python3
"""Freeze package-term observations and exact Scala/JDK binary evidence."""

import argparse
import collections
import csv
import io
import os
import zipfile
from pathlib import Path


OBSERVATION_HEADER = (
    "request_key",
    "prefix_kind",
    "package",
    "name",
    "member_space",
    "selector",
    "source_file",
    "enclosing_attempt",
    "resolver_outcome",
    "type_resource",
    "companion_resource",
)
EVIDENCE_HEADER = (
    "request_key",
    "selected_resource",
    "classification",
    "object_identity",
    "module_class_identity",
    "class_identity",
    "visibility",
    "detail",
)
BLOCKER_HEADER = (
    "blocker",
    "file",
    "method",
    "attribution",
    "request_attempts",
    "failed_qualifier",
    "request_targets",
)


def section(text, name):
    lines = text.splitlines()
    marker = f"{name}:"
    try:
        start = lines.index(marker) + 1
    except ValueError as error:
        raise ValueError(f"missing report section: {name}") from error
    result = []
    for line in lines[start:]:
        if line and not line.startswith(" ") and line.endswith(":"):
            break
        result.append(line)
    return result


def parse_table(lines, header):
    expected = "  " + "\t".join(header)
    try:
        start = lines.index(expected) + 1
    except ValueError as error:
        raise ValueError(f"missing TSV header: {expected}") from error
    result = []
    for line in lines[start:]:
        if not line.startswith("  ") or "\t" not in line:
            if result:
                break
            continue
        row = next(csv.DictReader(["\t".join(header), line[2:]], delimiter="\t"))
        result.append(row)
    return result


def request_rows(text):
    rows = parse_table(section(text, "package_term_request_rows"), OBSERVATION_HEADER)
    if not rows:
        raise ValueError("package-term request table is empty")
    for row in rows:
        if row["resolver_outcome"] != "package term member unsupported":
            raise ValueError(f"unexpected package-term outcome: {row}")
        if row["prefix_kind"] != "Package":
            raise ValueError(f"package-term request has a non-package prefix: {row}")
        if "namespace=term" not in row["request_key"] or "::space=" not in row["request_key"]:
            raise ValueError(f"invalid package-term key: {row['request_key']}")
    return sorted(rows, key=lambda row: tuple(row[field] for field in OBSERVATION_HEADER))


def evidence_rows(text):
    rows = parse_table(section(text, "package_term_module_evidence"), EVIDENCE_HEADER)
    for row in rows:
        for field in EVIDENCE_HEADER[1:]:
            if row[field] == "":
                row[field] = "-"
    by_key = {row["request_key"]: row for row in rows}
    if len(by_key) != len(rows):
        raise ValueError("duplicate module-evidence target")
    return by_key


def blocker_rows(text):
    lines = text.splitlines()
    marker = "package_term_first_blocker_rows:"
    try:
        start = lines.index(marker) + 1
    except ValueError as error:
        raise ValueError("missing package-term blocker table") from error
    result = parse_table(lines[start:], BLOCKER_HEADER)
    counts = collections.Counter(row["blocker"] for row in result)
    if counts != {"ImportQualifierNotFound": 361, "TermNameNotFound": 8}:
        raise ValueError(f"unexpected module first-blocker cohort: {dict(counts)}")
    return result


def archive_label(path, maven_root):
    path = Path(path)
    if maven_root:
        try:
            return path.relative_to(maven_root).as_posix()
        except ValueError:
            pass
    if path.suffix == ".jmod":
        return f"jmods/{path.name}"
    return path.name


def exact_resource_index(keys, classpath, jmods, maven_root=None):
    candidates = set()
    for package, name in keys.values():
        internal = f"{package.replace('.', '/')}/{name}" if package else name
        for binary in (internal, f"{internal}$"):
            candidates.add(f"{binary}.tasty")
            candidates.add(f"{binary}.class")
    found = collections.defaultdict(list)

    def scan(path, jmod=False):
        label = archive_label(path, maven_root)
        try:
            with zipfile.ZipFile(path) as archive:
                names = set(archive.namelist())
        except (OSError, zipfile.BadZipFile) as error:
            raise ValueError(f"cannot inspect classpath archive {path}: {error}") from error
        for candidate in candidates:
            entry = f"classes/{candidate}" if jmod else candidate
            if entry in names:
                found[candidate].append(f"{label}!{entry}")

    if jmods:
        for path in sorted(Path(jmods).glob("*.jmod")):
            scan(path, jmod=True)
    for path in classpath:
        candidate = Path(path)
        if candidate.is_dir():
            for resource in candidates:
                resource_path = candidate / resource
                if resource_path.is_file():
                    found[resource].append(f"{candidate.name}!{resource}")
        else:
            scan(candidate)
    return found


def freeze(report, observations_path, blockers_path, classpath, jmods, expected=(2749, 80)):
    text = Path(report).read_text()
    observations = request_rows(text)
    targets = {}
    for row in observations:
        identity = row["request_key"]
        value = (row["package"], row["name"])
        if identity in targets and targets[identity] != value:
            raise ValueError(f"request key changed identity: {identity}")
        targets[identity] = value
    actual = (len(observations), len(targets))
    if expected is not None and actual != expected:
        raise ValueError(f"baseline package-term volume changed: {actual}, expected {expected}")

    evidence = evidence_rows(text)
    if set(evidence) != set(targets):
        missing = sorted(set(targets) - set(evidence))
        extra = sorted(set(evidence) - set(targets))
        raise ValueError(f"module evidence does not cover request keys: missing={missing}, extra={extra}")
    resources = exact_resource_index(
        targets,
        classpath,
        jmods,
        Path(os.environ["COURSIER_MAVEN_ROOT"]) if os.environ.get("COURSIER_MAVEN_ROOT") else None,
    )

    observation_fields = list(OBSERVATION_HEADER) + [
        "type_tasty_resources",
        "type_class_resources",
        "companion_tasty_resources",
        "companion_class_resources",
        "module_classification",
        "tasty_object_identity",
        "tasty_module_class_identity",
        "tasty_class_identity",
        "tasty_visibility",
        "tasty_detail",
    ]
    observations_path = Path(observations_path)
    observations_path.parent.mkdir(parents=True, exist_ok=True)
    with observations_path.open("w", newline="") as output:
        writer = csv.DictWriter(output, observation_fields, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        for row in observations:
            identity = row["request_key"]
            package, name = targets[identity]
            internal = f"{package.replace('.', '/')}/{name}" if package else name
            module = evidence[identity]
            item = dict(row)
            for label, suffix in (
                ("type_tasty_resources", f"{internal}.tasty"),
                ("type_class_resources", f"{internal}.class"),
                ("companion_tasty_resources", f"{internal}$.tasty"),
                ("companion_class_resources", f"{internal}$.class"),
            ):
                item[label] = ";".join(resources.get(suffix, ())) or "absent"
            item["module_classification"] = module["classification"]
            item["tasty_object_identity"] = module["object_identity"]
            item["tasty_module_class_identity"] = module["module_class_identity"]
            item["tasty_class_identity"] = module["class_identity"]
            item["tasty_visibility"] = module["visibility"]
            item["tasty_detail"] = module["detail"]
            writer.writerow(item)

    blockers = blocker_rows(text)
    blockers_path = Path(blockers_path)
    blockers_path.parent.mkdir(parents=True, exist_ok=True)
    with blockers_path.open("w", newline="") as output:
        writer = csv.DictWriter(output, BLOCKER_HEADER, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        writer.writerows(sorted(blockers, key=lambda row: tuple(row[field] for field in BLOCKER_HEADER)))

    key_counts = collections.Counter(row["classification"] for row in evidence.values())
    resource_targets = {
        "type_tasty": sum(bool(resources.get(f"{(p.replace('.', '/') + '/' if p else '')}{n}.tasty")) for p, n in targets.values()),
        "type_class": sum(bool(resources.get(f"{(p.replace('.', '/') + '/' if p else '')}{n}.class")) for p, n in targets.values()),
        "companion_tasty": sum(bool(resources.get(f"{(p.replace('.', '/') + '/' if p else '')}{n}$.tasty")) for p, n in targets.values()),
        "companion_class": sum(bool(resources.get(f"{(p.replace('.', '/') + '/' if p else '')}{n}$.class")) for p, n in targets.values()),
    }
    blocker_counts = collections.Counter(row["blocker"] for row in blockers)
    return (
        f"Frozen {len(observations)} package-term request observations across {len(targets)} distinct keys, "
        f"and {len(blockers)} module-related first blockers. Exact resource probes found "
        f"{resource_targets['type_tasty']} type-side `.tasty`, {resource_targets['type_class']} type-side `.class`, "
        f"{resource_targets['companion_tasty']} companion `.tasty`, and {resource_targets['companion_class']} companion `.class` target keys. "
        f"The semantic TASTy classifications are {dict(sorted(key_counts.items()))}. "
        f"The blocker cohort is {blocker_counts['ImportQualifierNotFound']} `ImportQualifierNotFound` and "
        f"{blocker_counts['TermNameNotFound']} `TermNameNotFound` observations."
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("report")
    parser.add_argument("observations")
    parser.add_argument("blockers")
    arguments = parser.parse_args()
    classpath = [Path(path) for path in os.environ["SCALA39_CLASSPATH"].split(os.pathsep)]
    for path in classpath:
        if not path.is_file():
            raise SystemExit(f"missing Scala classpath archive: {path}")
    jmods = Path(os.environ["JAVA_HOME"]) / "jmods"
    text = freeze(arguments.report, arguments.observations, arguments.blockers, classpath, jmods)
    print(text)


if __name__ == "__main__":
    main()
