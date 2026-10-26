"""Evidence-backed policy for classloader completion fanout."""

from __future__ import annotations

import csv
import re
from collections import Counter, defaultdict, deque
from dataclasses import dataclass
from pathlib import Path


LEGACY_COMPLETION_LIMIT = 16
MAX_PROFILE_IDENTITIES = 256
MAX_PROFILE_EDGES = 512
MAX_HIERARCHY_DEPTH = 256
CURRENT_SOURCE_SHA = "5a0503dc0ed4d0fdf7dd2d19add27dfdd3361acb"
SCALA_SOURCE_SHA = "777528f19a58e794c9954a42f433373472ec57f8"

DOCS = Path(__file__).resolve().parents[2] / "docs"
ROOTS_PATH = DOCS / "classloader-completion-fanout-3.9.0-current-roots.tsv"
EDGES_PATH = DOCS / "classloader-completion-fanout-3.9.0-current-edges.tsv"
CLASSES_PATH = DOCS / "classloader-completion-fanout-3.9.0-current-classes.tsv"

PROFILE_RE = re.compile(
    r"^root=(?P<root>.*?)::file=(?P<file>.*?)::method=(?P<method>.*?)"
    r"::request=(?P<request>.*?)::depth=(?P<depth>\d+)"
    r"::distinct_classes=(?P<distinct_classes>\d+)"
    r"::full_completions=(?P<full_completions>\d+)"
    r"::shells_created=(?P<shells_created>\d+)"
    r"::shells_reused=(?P<shells_reused>\d+)"
    r"::member_type_references=(?P<member_type_references>\d+)"
    r"::hierarchy_loads=(?P<hierarchy_loads>\d+)"
    r"::completed_cache_hits=(?P<completed_cache_hits>\d+)"
    r"::module_completion_calls=(?P<module_completion_calls>\d+)"
    r"::module_completion_successes=(?P<module_completion_successes>\d+)"
    r"::module_completion_errors=(?P<module_completion_errors>\d+)"
    r"::identity_shells=\[(?P<identity_shells>.*?)\]"
    r"::completed_classes=\[(?P<completed_classes>.*?)\]"
    r"::classes=\[(?P<classes>.*?)\]"
    r"::dependency_edges=\[(?P<dependency_edges>.*?)\]$"
)


@dataclass(frozen=True)
class VerifiedProfile:
    method: str
    completed: int
    proven_hierarchy: int
    unexpected: int
    depth: int
    identities: int


def _read_tsv(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as stream:
        return list(csv.DictReader(stream, delimiter="\t"))


def _parse_names(value: str) -> set[str]:
    return set(value.split(",")) if value else set()


def _parse_edges(value: str) -> Counter[tuple[str, str, str, int]]:
    edges: Counter[tuple[str, str, str, int]] = Counter()
    if not value:
        return edges
    for item in value.split(","):
        match = re.fullmatch(r"([^:]+):([^>]+)>([^:]+):(\d+)", item)
        if not match:
            raise ValueError(f"malformed dependency edge: {item}")
        reason, owner, dependency, count = match.groups()
        edges[(reason, owner, dependency, int(count))] += 1
    return edges


def serialize_profile_record(
    root: dict[str, str],
    edge_rows: list[dict[str, str]],
    class_rows: list[dict[str, str]],
) -> str:
    """Render the stable profile row used by the local and corpus audit reports."""
    identity_shells = ",".join(
        row["class"] for row in class_rows if row["identity_shell_seen"] == "1"
    )
    completed = ",".join(
        row["class"] for row in class_rows if row["fully_completed"] == "1"
    )
    classes = ",".join(row["class"] for row in class_rows)
    edges = ",".join(
        f'{row["reason"]}:{row["owner"]}>{row["dependency"]}:{row["requests"]}'
        for row in edge_rows
    )
    method = root["method"]
    file = method.split("#tree=", 1)[0]
    return (
        f'root={root["root"]}::file={file}::method={method}'
        f'::request={root["request"]}::depth={root["depth"]}'
        f'::distinct_classes={root["distinct_classes"]}'
        f'::full_completions={root["full_completions"]}'
        f'::shells_created={root["shells_created"]}'
        f'::shells_reused={root["shells_reused"]}'
        f'::member_type_references={root["member_type_references"]}'
        f'::hierarchy_loads={root["hierarchy_loads"]}'
        f'::completed_cache_hits={root["completed_cache_hits"]}'
        f'::module_completion_calls={root["module_completion_calls"]}'
        f'::module_completion_successes={root["module_completion_successes"]}'
        f'::module_completion_errors={root["module_completion_errors"]}'
        f'::identity_shells=[{identity_shells}]::completed_classes=[{completed}]'
        f'::classes=[{classes}]::dependency_edges=[{edges}]'
    )


def _max_graph_depth(edges: set[tuple[str, str]]) -> int:
    children: dict[str, set[str]] = defaultdict(set)
    indegree: dict[str, int] = defaultdict(int)
    for owner, dependency in edges:
        children[owner].add(dependency)
        indegree[dependency] += 1
        indegree.setdefault(owner, 0)
    nodes = set(indegree)
    ready = deque(sorted(node for node in nodes if indegree[node] == 0))
    depths = {node: 0 for node in ready}
    visited = 0
    while ready:
        node = ready.popleft()
        visited += 1
        for parent in children[node]:
            depths[parent] = max(depths.get(parent, 0), depths[node] + 1)
            indegree[parent] -= 1
            if indegree[parent] == 0:
                ready.append(parent)
    if visited != len(nodes):
        raise ValueError("inheritance cycle in hierarchy profile")
    return max(depths.values(), default=0)


def verify_hierarchy_graph(
    *,
    root: str,
    classes: set[str],
    completed: set[str],
    observed_edges: Counter[tuple[str, str, str, int]],
    trusted_edges: Counter[tuple[str, str, str, int]],
    max_depth: int,
) -> tuple[int, int]:
    """Require exact trusted edges and prove every completion is in the root closure."""
    if observed_edges != trusted_edges:
        raise ValueError("dependency edges differ from trusted hierarchy/member oracle")
    if max_depth > MAX_HIERARCHY_DEPTH:
        raise ValueError(
            f"hierarchy depth budget exceeded: {max_depth}/{MAX_HIERARCHY_DEPTH}"
        )
    if len(classes) > MAX_PROFILE_IDENTITIES:
        raise ValueError(
            f"profile identity budget exceeded: {len(classes)}/{MAX_PROFILE_IDENTITIES}"
        )
    if len(observed_edges) > MAX_PROFILE_EDGES:
        raise ValueError(
            f"profile edge budget exceeded: {len(observed_edges)}/{MAX_PROFILE_EDGES}"
        )

    hierarchy = {
        (owner, dependency)
        for reason, owner, dependency, _ in observed_edges
        if reason == "hierarchy"
    }
    if not hierarchy:
        raise ValueError("verified high fanout has no hierarchy edges")
    hierarchy_nodes = {node for edge in hierarchy for node in edge}
    if not hierarchy_nodes <= classes:
        raise ValueError(
            f"hierarchy graph names identities absent from request profile: "
            f"{sorted(hierarchy_nodes - classes)}"
        )
    if root not in classes or not completed <= classes:
        raise ValueError("root or completed class identity is absent from the request profile")

    # The observed classloader depth omits completed-cache hits; the pinned
    # dependency graph can therefore contain a longer path than was traversed
    # eagerly in one request. Check both values against the independent ceiling.
    graph_depth = _max_graph_depth(hierarchy)
    if graph_depth > MAX_HIERARCHY_DEPTH:
        raise ValueError(
            f"hierarchy graph depth budget exceeded: {graph_depth}/{MAX_HIERARCHY_DEPTH}"
        )
    closure = {root}
    pending = [root]
    children: dict[str, set[str]] = defaultdict(set)
    for owner, dependency in hierarchy:
        children[owner].add(dependency)
    while pending:
        pending.extend(children[pending.pop()] - closure)
        closure.update(pending)

    unexpected = completed - closure
    if unexpected:
        raise ValueError(
            f"unexpected non-hierarchy completions: {sorted(unexpected)}"
        )
    missing = closure - completed
    if missing:
        raise ValueError(
            f"hierarchy closure includes classes not fully completed: {sorted(missing)}"
        )
    return len(completed & closure), len(unexpected)


def _expected_oracle() -> tuple[
    dict[str, dict[str, str]],
    dict[str, Counter[tuple[str, str, str, int]]],
    dict[str, set[tuple[str, str, str]]],
]:
    roots = {row["source_method"]: row for row in _read_tsv(ROOTS_PATH)}
    edges: dict[str, Counter[tuple[str, str, str, int]]] = defaultdict(Counter)
    for row in _read_tsv(EDGES_PATH):
        edges[row["source_method"]][
            (row["reason"], row["owner"], row["dependency"], int(row["requests"]))
        ] += 1
    classes: dict[str, set[tuple[str, str, str]]] = defaultdict(set)
    for row in _read_tsv(CLASSES_PATH):
        classes[row["source_method"]].add(
            (row["class"], row["identity_shell_seen"], row["fully_completed"])
        )
    if set(roots) != set(edges) or set(roots) != set(classes):
        raise ValueError("List fanout oracle roots differ between current TSVs")
    return roots, edges, classes


def verify_high_fanout_profiles(lines: list[str], expected_count: int) -> list[VerifiedProfile]:
    """Accept only exact, bounded high-fanout profiles in the pinned hierarchy oracle."""
    parsed = []
    for line in lines:
        match = PROFILE_RE.fullmatch(line.strip())
        if not match:
            raise ValueError(f"malformed high-fanout profile: {line.strip()[:120]}")
        record = match.groupdict()
        for metric in (
            "depth", "distinct_classes", "full_completions", "shells_created",
            "shells_reused", "member_type_references", "hierarchy_loads",
            "completed_cache_hits", "module_completion_calls",
            "module_completion_successes", "module_completion_errors",
        ):
            record[metric] = int(record[metric])
        parsed.append(record)
    if len(parsed) != expected_count:
        raise ValueError(
            f"high-fanout evidence count mismatch: metric={expected_count}, rows={len(parsed)}"
        )

    expected_roots, expected_edges, expected_classes = _expected_oracle()
    if expected_count != len(expected_roots):
        raise ValueError(
            f"high-fanout request count differs from the pinned oracle: "
            f"{expected_count}/{len(expected_roots)}"
        )
    by_method = {row["method"]: row for row in parsed}
    if len(by_method) != len(parsed):
        raise ValueError("duplicate high-fanout request profile")
    if set(by_method) != set(expected_roots):
        unknown = sorted(set(by_method) - set(expected_roots))
        missing = sorted(set(expected_roots) - set(by_method))
        raise ValueError(
            f"high fanout must match all pinned requests; "
            f"unknown={unknown}, missing={missing}"
        )

    verified = []
    for method in sorted(expected_roots):
        actual = by_method[method]
        expected = expected_roots[method]
        if actual["root"] != expected["root"] or actual["request"] != expected["request"]:
            raise ValueError(f"root request differs from pinned List oracle: {method}")
        if actual["depth"] != int(expected["depth"]):
            raise ValueError(f"hierarchy depth differs from pinned List oracle: {method}")
        for metric in (
            "distinct_classes", "full_completions", "shells_created", "shells_reused",
            "member_type_references", "hierarchy_loads", "completed_cache_hits",
            "module_completion_calls", "module_completion_successes",
            "module_completion_errors",
        ):
            if actual[metric] != int(expected[metric]):
                raise ValueError(f"{metric} differs from pinned List oracle: {method}")
        actual_classes = {
            (name, str(int(name in _parse_names(actual["identity_shells"]))),
             str(int(name in _parse_names(actual["completed_classes"]))))
            for name in _parse_names(actual["classes"])
        }
        if actual_classes != expected_classes[method]:
            raise ValueError(f"visited/completed identities differ from pinned List oracle: {method}")
        actual_edges = _parse_edges(actual["dependency_edges"])
        if actual_edges != expected_edges[method]:
            raise ValueError(f"dependency graph differs from pinned List oracle: {method}")
        completed = _parse_names(actual["completed_classes"])
        if len(completed) != actual["full_completions"]:
            raise ValueError(f"raw completed count differs from completed class set: {method}")
        proven, unexpected = verify_hierarchy_graph(
            root=actual["root"],
            classes=_parse_names(actual["classes"]),
            completed=completed,
            observed_edges=actual_edges,
            trusted_edges=expected_edges[method],
            max_depth=actual["depth"],
        )
        verified.append(
            VerifiedProfile(
                method=method,
                completed=len(completed),
                proven_hierarchy=proven,
                unexpected=unexpected,
                depth=actual["depth"],
                identities=actual["distinct_classes"],
            )
        )
    return verified
