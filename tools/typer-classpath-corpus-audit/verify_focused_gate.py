#!/usr/bin/env python3
"""Verify the bounded typer audit output and write timing sidecars."""

from __future__ import annotations

import argparse
import csv
import hashlib
from pathlib import Path
import re
import subprocess
import sys


REPORT_START = "## Selected audit output\n\n```text\n"
REPORT_END = "\n```\n\n"
TIMING_FIELDS = (
    "files",
    "wall_ms",
    "classpath_setup_ms",
    "file_discovery_ms",
    "source_read_ms",
    "source_processing_and_merge_ms",
)
TIMING_PATTERN = re.compile(r"^FOCUSED_TIMING (.+)$", re.MULTILINE)
NESTED_FIXTURE = Path("crates/dotty-typer/tests/fixtures/source-nested-recursive-adt-3.9.0")
NESTED_SOURCE_SHA256 = "f700d4e27f11365a08291ae15babc65e673eb91a0bdeee45ea5dc2cf7e8c8607"
NESTED_ROLLBACK_SOURCE_SHA256 = "23fd51007477c572b49c2ef0165482173f0b7265fbf23d89cbf069dd444d0c8f"
NESTED_ROLLBACK_SOURCE_BYTES = 870
NESTED_ROLLBACK_OWNER = "RollbackControls$"
NESTED_BASELINE_SHA = "0fc457776374f69d6b16187c007721a85b2557a0"
NESTED_TEST = "nested_recursive_adt_original_roots_are_attempted_in_two_fresh_sessions"
NESTED_RECEIPT_HASHES = {
    "rust-peelleft-complete.tsv": "c365d35c8fe0df1c45dd0052b80c6e7e6f86e365f326c7404c4508514856001d",
    "rust-sum-complete.tsv": "f75fc10eac428ff81324cefecf46607b31f6bf78470d06bdfe4b03029e868e3c",
    "negative-cases.tsv": "966f67c7eaff94084eb58b759f69f72db0c2481d36b6e16f8fc080ab5e5206fd",
    "rust-late-failure.tsv": "b23431067a439b5a1a997e90023e2301e3eaeb0656968209cf8a7a16222bc916",
    "source-hashes.tsv": "7ac1df0416cc785b14441a4c2f05a5dbaf67cb37cf16f5c7ae1d503ac363c718",
}
NESTED_ROWS = {
    ("session-a", "peelLeft"): ("Expr.scala#tree40", "158:266", 20, "Expr"),
    ("session-a", "sum"): ("Expr.scala#tree78", "270:468", 38, "Int"),
    ("session-b", "peelLeft"): ("Expr.scala#tree40", "158:266", 20, "Expr"),
    ("session-b", "sum"): ("Expr.scala#tree78", "270:468", 38, "Int"),
}
NESTED_SOURCE_IDS = {"session-a": "21321", "session-b": "21322"}
NESTED_NEGATIVE_CLASSIFICATIONS = {
    "WrongNestedArity": "rejected",
    "WrongNestedLiteralType": "rejected",
    "EscapedCaseBinder": "rejected",
    "ForeignSameSpelledExtractor": "accepted",
    "InvalidRecursiveArgument": "rejected",
    "WrongMethodResult": "rejected",
    "RenamedReorderedFields": "accepted",
    "LatePeelLeftResult": "rejected",
    "LateSumResult": "rejected",
    "WrongOuterPatternArity": "rejected",
}
NESTED_CLASSIFIED_CASES = {
    "WrongNestedArity": ("WrongNestedArity.scala", "21380", "sum", "WrongNestedArity.scala#tree79", "270:475", "Evaluator$", "rejected", "rejected;error=ExtractorProductSelectorCountMismatch;failure=WrongNestedArity.scala#tree55@358:375;typed_root=absent"),
    "WrongNestedLiteralType": ("WrongNestedLiteralType.scala", "21381", "sum", "WrongNestedLiteralType.scala#tree74", "270:451", "Evaluator$", "rejected", "rejected;error=PatternTypeMismatch;failure=WrongNestedLiteralType.scala#tree52@358:359;typed_root=absent"),
    "EscapedCaseBinder": ("EscapedCaseBinder.scala", "21382", "sum", "EscapedCaseBinder.scala#tree78", "270:463", "Evaluator$", "rejected", "rejected;error=TermNameNotFound;failure=EscapedCaseBinder.scala#tree48@333:337;typed_root=absent"),
    "ForeignSameSpelledExtractor": ("ForeignSameSpelledExtractor.scala", "21383", "peelLeft", "ForeignSameSpelledExtractor.scala#tree66", "265:381", "Evaluator$", "accepted", "accepted;typed_root=complete_DefDef;return=Expr;mappings=complete(21):all_spans_preserved"),
    "InvalidRecursiveArgument": ("InvalidRecursiveArgument.scala", "21384", "sum", "InvalidRecursiveArgument.scala#tree78", "270:464", "Evaluator$", "rejected", "rejected;error=ApplicationArgumentTypeMismatch;failure=InvalidRecursiveArgument.scala#tree60@396:402;typed_root=absent"),
    "WrongMethodResult": ("WrongMethodResult.scala", "21385", "sum", "WrongMethodResult.scala#tree78", "270:469", "Evaluator$", "rejected", "rejected;error=OverloadApplicationNoApplicable;failure=WrongMethodResult.scala#tree61@389:407;typed_root=absent"),
    "RenamedReorderedFields": ("RenamedReorderedFields.scala", "21386", "peelLeft", "RenamedReorderedFields.scala#tree40", "158:266", "Evaluator$", "accepted", "accepted;typed_root=complete_DefDef;return=Expr;mappings=complete(20):all_spans_preserved"),
    "LatePeelLeftResult": ("LatePeelLeftResult.scala", "21387", "peelLeftFails", "LatePeelLeftResult.scala#tree102", "496:612", "LatePeelControl$", "rejected", "rejected;error=ExpectedExpressionTypeMismatch;failure=LatePeelLeftResult.scala#tree101@541:612;typed_root=absent"),
    "LateSumResult": ("LateSumResult.scala", "21388", "sumFails", "LateSumResult.scala#tree123", "495:711", "LateSumControl$", "rejected", "rejected;error=ExpectedExpressionTypeMismatch;failure=LateSumResult.scala#tree122@535:711;typed_root=absent"),
    "WrongOuterPatternArity": ("WrongOuterPatternArity.scala", "21389", "peelLeftFails", "WrongOuterPatternArity.scala#tree103", "503:619", "WrongOuterArityControl$", "rejected", "rejected;error=ExtractorProductSelectorCountMismatch;failure=WrongOuterPatternArity.scala#tree94@567:590;typed_root=absent"),
}
NESTED_CONTROL_TESTS = {
    "nested_peelleft_controls_keep_bounded_rejections_and_positional_identity",
    "nested_sum_bounded_invalid_roots_fail_without_partial_mappings",
    "nested_sum_rejects_a_sibling_case_binder_escape_then_types_original_sum",
    "nested_adt_classifications_execute_all_pinned_case_roots",
}
NESTED_ROLLBACK_TEST = "nested_late_failures_restore_and_retry_before_original_roots_recover"


class GateError(ValueError):
    """Raised when a focused audit artifact is incomplete or differs."""


def expected_nested_proof(root: Path, method: str, mapping_count: int, result: str) -> str:
    receipt_name = "rust-peelleft-complete.tsv" if method == "peelLeft" else "rust-sum-complete.tsv"
    receipt_path = root / NESTED_FIXTURE / receipt_name
    try:
        receipt_bytes = receipt_path.read_bytes()
        lines = receipt_bytes.decode("utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        raise GateError(f"cannot read pinned nested ADT semantic receipt: {error}") from error
    if hashlib.sha256(receipt_bytes).hexdigest() != NESTED_RECEIPT_HASHES[receipt_name]:
        raise GateError(f"{receipt_path}: normalized semantic receipt pin changed")
    if len(lines) != 2:
        raise GateError(f"{receipt_path}: expected one normalized semantic receipt")
    normalized = lines[1].replace("\t", ";")
    if method == "peelLeft":
        pattern = "pattern=Add(Add(left,_),_)"
        identity = "identity=outer+inner.Add_use_canonical_unapply;left_unique_case_binder"
    else:
        pattern = "patterns=[Num(value),Add(Num(value),right),Add(left,right)]"
        identity = "identity=Evaluator.sum_same_canonical_method_at_3_distinct_Applies;5_distinct_case_binders"
    return (
        f"{normalized};signature=(Expr):{result};{pattern};"
        "canonical=Expr,Num$.unapply->Num,Add$.unapply->Add,"
        f"Add.Product2(_1:Expr,_2:Expr);{identity};source_mapping=complete({mapping_count}):"
        "all_spans_preserved;source_owned=true"
    )


def _git_head(root: Path) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise GateError(f"cannot read tested repository HEAD: {error}") from error
    return result.stdout.strip()


def _git_is_clean(root: Path) -> bool:
    try:
        result = subprocess.run(
            # The focused workflow checks out the pinned Scala source tree below
            # the repository root. It is intentionally untracked; only tracked
            # source changes can invalidate evidence for the tested commit.
            ["git", "-C", str(root), "diff", "--quiet", "HEAD", "--"],
            check=False,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise GateError(f"cannot verify tested repository cleanliness: {error}") from error
    return result.returncode == 0


def _validate_test_summary(log: str, expected_tests: set[str], label: str) -> None:
    # With --nocapture and RUST_TEST_THREADS=1, libtest emits `test NAME ... `,
    # then the test's own output, then the terminal status on a separate line.
    # Count test starts and reconcile their total with libtest's authoritative
    # summary; a successful summary with zero ignored/failed tests proves each
    # observed start passed without misreading arbitrary stdout as a status.
    events = re.findall(r"^test ([A-Za-z0-9_:]+) \.\.\..*$", log, re.MULTILINE)
    summaries = re.findall(
        r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",
        log, re.MULTILINE,
    )
    if len(summaries) != 1:
        raise GateError(f"{label} log must contain exactly one complete test summary")
    status, passed, failed, ignored = summaries[0]
    names: dict[str, list[str]] = {}
    for name in events:
        names.setdefault(name.split("::")[-1], []).append(name)
    if status != "ok" or int(failed) != 0 or int(ignored) != 0 or int(passed) != len(events):
        raise GateError(f"{label} test summary does not reconcile with observed test starts")
    if int(passed) == 0:
        raise GateError(f"{label} log must prove nonzero passing tests with no ignored or failed tests")
    for name in expected_tests:
        if len(names.get(name, [])) != 1:
            raise GateError(f"{label} mandatory test did not execute exactly once and pass: {name}")


def _validate_revision_marker(log: str, marker: str, tested_sha: str, label: str) -> None:
    matches = re.findall(rf"(?:^|\s){re.escape(marker)}\t([0-9a-f]{{40}})(?=\s|$)", log, re.MULTILINE)
    if matches != [tested_sha]:
        raise GateError(f"{label} log is missing, stale or mismatched for the exact tested revision")


def validate_nested_positive_log(
    root: Path, log_text: str, tested_sha: str, pr_head_sha: str | None = None,
    require_clean: bool = False,
) -> list[dict[str, str]]:
    if not re.fullmatch(r"[0-9a-f]{40}", tested_sha) or tested_sha != _git_head(root):
        raise GateError("nested ADT positive log tested SHA is stale or differs from repository HEAD")
    if require_clean and not _git_is_clean(root):
        raise GateError("nested ADT evidence must be generated from a clean tested revision")
    if pr_head_sha is not None and pr_head_sha != tested_sha:
        raise GateError("nested ADT positive evidence does not match the PR head")
    if not re.fullmatch(r"[0-9a-f]{40}", NESTED_BASELINE_SHA):
        raise GateError("nested ADT baseline pin is malformed")
    try:
        ancestry = subprocess.run(
            ["git", "-C", str(root), "merge-base", "--is-ancestor", NESTED_BASELINE_SHA, tested_sha],
            capture_output=True,
        )
    except OSError as error:
        raise GateError(f"cannot verify nested ADT baseline ancestry: {error}") from error
    if ancestry.returncode != 0:
        raise GateError("nested ADT tested revision is not based on the pinned prerequisite baseline")

    fixture = root / NESTED_FIXTURE
    source = fixture / "Expr.scala"
    try:
        source_digest = hashlib.sha256(source.read_bytes()).hexdigest()
    except (OSError, UnicodeError) as error:
        raise GateError(f"cannot read immutable nested ADT source: {error}") from error
    if source_digest != NESTED_SOURCE_SHA256:
        raise GateError("nested ADT source does not match the immutable #2132 pin")

    _validate_test_summary(log_text, {NESTED_TEST}, "nested ADT positive")
    _validate_revision_marker(log_text, "ISSUE2137_TESTED_SHA", tested_sha, "nested ADT positive")
    if log_text.count("ISSUE2137_EXECUTED_TESTS\t1") != 1:
        raise GateError("nested ADT positive log is missing its nonzero execution receipt")

    rows: list[dict[str, str]] = []
    for line in re.findall(r"^ISSUE2137_POSITIVE\t(.+)$", log_text, re.MULTILINE):
        fields = line.split("\t")
        if len(fields) != 12:
            raise GateError("nested ADT positive receipt has malformed columns")
        (session, nonce, source_id, store_ptr, arena_ptr, resolver_nonce, method,
         path_tree, span, owner, source_sha, proof) = fields
        key = session, method
        if key not in NESTED_ROWS or any((row["session"], row["root"]) == key for row in rows):
            raise GateError("nested ADT positive receipt contains an unexpected or duplicate root")
        expected_path, expected_span, mapping_count, result = NESTED_ROWS[key]
        if (path_tree, span, owner, source_sha) != (
            expected_path, expected_span, "Evaluator$", NESTED_SOURCE_SHA256
        ):
            raise GateError("nested ADT positive receipt substituted its pinned root, owner or source")
        if proof != expected_nested_proof(root, method, mapping_count, result):
            raise GateError(f"nested ADT {method} proof is partial or differs from the canonical semantic shape")
        if not re.fullmatch(r"[0-9]+-[0-9a-f]+", nonce):
            raise GateError("nested ADT session nonce is malformed")
        if not source_id.isdecimal() or not re.fullmatch(r"[0-9a-f]+", store_ptr) or not re.fullmatch(r"[0-9a-f]+", arena_ptr):
            raise GateError("nested ADT fresh-session identity witness is malformed")
        if not re.fullmatch(r"[0-9]+-[0-9]+", resolver_nonce):
            raise GateError("nested ADT resolver freshness witness is malformed")
        if source_id != NESTED_SOURCE_IDS[session] or resolver_nonce.split("-", 1)[0] != source_id:
            raise GateError("nested ADT session source/resolver witness differs from its pinned root attempt")
        rows.append({
            "session": session, "session_nonce": nonce, "source_id": source_id,
            "store_witness": store_ptr, "arena_witness": arena_ptr,
            "resolver_nonce": resolver_nonce, "root": method,
            "source_path_tree": path_tree, "source_span": span, "owner": owner,
            "source_sha256": source_sha, "proof": proof,
        })
    if len(rows) != 4 or {(row["session"], row["root"]) for row in rows} != set(NESTED_ROWS):
        raise GateError("nested ADT acceptance requires exactly the two original roots in two sessions")
    by_session: dict[str, list[dict[str, str]]] = {}
    for row in rows:
        by_session.setdefault(row["session"], []).append(row)
    if set(by_session) != {"session-a", "session-b"}:
        raise GateError("nested ADT evidence must contain exactly two fresh sessions")
    for session_rows in by_session.values():
        if len({row["session_nonce"] for row in session_rows}) != 1 or len({row["source_id"] for row in session_rows}) != 1:
            raise GateError("nested ADT roots do not share one execution-bound session witness")
        if len({row["store_witness"] for row in session_rows}) != 1 or len({row["arena_witness"] for row in session_rows}) != 1:
            raise GateError("nested ADT roots do not share one live semantic store and AST arena")
        if len({row["resolver_nonce"] for row in session_rows}) != 1:
            raise GateError("nested ADT roots do not share one fresh resolver session")
    sample_rows = [session_rows[0] for session_rows in by_session.values()]
    for field in ("session_nonce", "source_id", "store_witness", "arena_witness", "resolver_nonce"):
        if len({row[field] for row in sample_rows}) != 2:
            raise GateError(f"nested ADT sessions are not independent in {field}")
    if len({row["session_nonce"].split("-", 1)[0] for row in sample_rows}) != 1:
        raise GateError("nested ADT session witnesses do not come from one executed test process")
    return rows


def validate_nested_negative_sidecars(root: Path, rollback_log: str, tested_sha: str) -> list[dict[str, str]]:
    fixture = root / NESTED_FIXTURE
    try:
        negative_bytes = (fixture / "negative-cases.tsv").read_bytes()
        rollback_bytes = (fixture / "rust-late-failure.tsv").read_bytes()
        negative_lines = negative_bytes.decode("utf-8").splitlines()
        expected_rollback = rollback_bytes.decode("utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        raise GateError(f"cannot read #2136 nested ADT sidecars: {error}") from error
    if hashlib.sha256(negative_bytes).hexdigest() != NESTED_RECEIPT_HASHES["negative-cases.tsv"]:
        raise GateError("#2136 classified negative sidecar pin changed")
    if hashlib.sha256(rollback_bytes).hexdigest() != NESTED_RECEIPT_HASHES["rust-late-failure.tsv"]:
        raise GateError("#2136 late-failure sidecar pin changed")
    observed: dict[str, str] = {}
    for line in negative_lines[1:]:
        fields = line.split("\t")
        if len(fields) != 3 or fields[0] in observed:
            raise GateError("#2136 negative classification sidecar is malformed or duplicated")
        observed[fields[0]] = fields[1]
    if observed != NESTED_NEGATIVE_CLASSIFICATIONS:
        raise GateError("#2136 classified negative sidecar differs from its pinned ten-case inventory")
    if len(expected_rollback) != 3 or expected_rollback[0] != "control\tsource_id\tfresh_source_id\tsource_bytes\troot_span\tdiagnostic\trecovery":
        raise GateError("#2136 rollback sidecar has an unexpected schema")
    marker = "ISSUE2136_NESTED_ROLLBACK\t" + "\n".join(expected_rollback)
    if rollback_log.count(marker) != 1:
        raise GateError("#2136 actual late-failure receipt is absent or differs from its normalized sidecar")
    source_records = re.findall(r"(?:^|\s)ISSUE2136_ROLLBACK_SOURCE\t(.+)$", rollback_log, re.MULTILINE)
    if len(source_records) != 1:
        raise GateError("#2136 actual rollback source provenance must be emitted exactly once")
    source_fields = source_records[0].split("\t")
    if len(source_fields) != 5:
        raise GateError("#2136 rollback source provenance has a malformed schema")
    source_id, fresh_source_id, source_bytes, owner, suffix_hex = source_fields
    try:
        suffix = bytes.fromhex(suffix_hex)
        positive_source = (fixture / "Expr.scala").read_bytes()
    except (ValueError, OSError) as error:
        raise GateError(f"cannot decode #2136 rollback source provenance: {error}") from error
    rollback_source = positive_source + suffix
    if (
        source_id != "21361" or fresh_source_id != "21362"
        or source_id == fresh_source_id
        or source_bytes != str(NESTED_ROLLBACK_SOURCE_BYTES)
        or len(rollback_source) != NESTED_ROLLBACK_SOURCE_BYTES
        or hashlib.sha256(rollback_source).hexdigest() != NESTED_ROLLBACK_SOURCE_SHA256
        or owner != NESTED_ROLLBACK_OWNER
    ):
        raise GateError("#2136 rollback source id, bytes, hash or owner differs from the executed control source")
    _validate_revision_marker(rollback_log, "ISSUE2137_ROLLBACK_TESTED_SHA", tested_sha, "#2136 rollback")
    _validate_test_summary(rollback_log, {NESTED_ROLLBACK_TEST}, "#2136 rollback")
    expected_rows = [line.split("\t") for line in expected_rollback[1:]]
    rows = []
    for fields in expected_rows:
        span = re.fullmatch(r"tree[0-9]+@([0-9]+)\.\.([0-9]+)", fields[4])
        if (fields[1], fields[2], fields[3]) != (source_id, fresh_source_id, source_bytes):
            raise GateError("#2136 rollback row identity differs from its emitted control-source receipt")
        if span is None or int(span.group(2)) > len(rollback_source):
            raise GateError("#2136 rollback root span escapes its emitted control source")
        rows.append({
            "kind": "rollback", "case": fields[0], "fixture": "Expr.scala+RollbackControls",
            "source_sha256": NESTED_ROLLBACK_SOURCE_SHA256, "root": fields[0],
            "source_path_tree": "", "source_span": fields[4], "owner": owner,
            "classification": "rejected", "outcome": ";".join(fields[5:]),
            "source_id": source_id, "fresh_source_id": fresh_source_id,
            "source_bytes": source_bytes,
        })
    return rows


def validate_nested_control_log(root: Path, control_log: str, tested_sha: str) -> list[dict[str, str]]:
    _validate_revision_marker(control_log, "ISSUE2137_CONTROLS_TESTED_SHA", tested_sha, "nested ADT controls")
    _validate_test_summary(control_log, NESTED_CONTROL_TESTS, "nested ADT controls")
    fixture = root / NESTED_FIXTURE
    hashes = {}
    for line in (fixture / "source-hashes.tsv").read_text(encoding="utf-8").splitlines()[1:]:
        digest, filename = line.split("\t")
        hashes[filename] = digest
    if hashlib.sha256((fixture / "source-hashes.tsv").read_bytes()).hexdigest() != NESTED_RECEIPT_HASHES["source-hashes.tsv"]:
        raise GateError("nested ADT source hash inventory pin changed")
    rows = []
    records = re.findall(r"^ISSUE2137_CLASSIFIED_CASE\t(.+)$", control_log, re.MULTILINE)
    if len(records) != len(NESTED_CLASSIFIED_CASES) or control_log.count("ISSUE2137_CLASSIFIED_CASE_COUNT\t10") != 1:
        raise GateError("nested ADT controls must emit exactly ten executed classified case receipts")
    for record in records:
        fields = record.split("\t")
        if len(fields) != 9:
            raise GateError("nested ADT classified case receipt has malformed columns")
        case, fixture_name, source_id, method, path_tree, span, owner, classification, outcome = fields
        expected = NESTED_CLASSIFIED_CASES.get(case)
        if expected is None or any(row["case"] == case for row in rows):
            raise GateError("nested ADT classified case receipt is unexpected or duplicated")
        expected_tuple = (*expected[:6], NESTED_NEGATIVE_CLASSIFICATIONS[case], expected[7])
        if tuple(fields[1:]) != expected_tuple:
            raise GateError(f"nested ADT case execution differs from pinned source/root/outcome: {case}")
        try:
            source_digest = hashlib.sha256((fixture / fixture_name).read_bytes()).hexdigest()
        except OSError as error:
            raise GateError(f"cannot read classified source fixture {fixture_name}: {error}") from error
        if hashes.get(fixture_name) != source_digest:
            raise GateError(f"nested ADT case source differs from its pinned SHA-256: {fixture_name}")
        rows.append({"case": case, "fixture": fixture_name, "source_sha256": source_digest,
                     "root": method, "source_path_tree": path_tree, "source_span": span,
                     "owner": owner, "classification": classification, "outcome": outcome})
    if {row["case"] for row in rows} != set(NESTED_CLASSIFIED_CASES):
        raise GateError("nested ADT controls omitted one or more pinned classifications")
    return rows


def extract_selected_report(report: str) -> str:
    start = report.find(REPORT_START)
    if start < 0:
        raise GateError("focused report is missing the selected audit section")
    start += len(REPORT_START)
    end = report.find(REPORT_END, start)
    if end < 0:
        raise GateError("focused report is missing the selected audit section terminator")
    return report[start:end] + "\n"


def verify_baseline(report_path: Path, baseline_path: Path) -> None:
    try:
        report = report_path.read_text(encoding="utf-8")
        expected = baseline_path.read_text(encoding="utf-8")
    except OSError as error:
        raise GateError(f"cannot read focused audit input: {error}") from error
    actual = extract_selected_report(report)
    if actual != expected:
        expected_lines = expected.splitlines()
        actual_lines = actual.splitlines()
        first_difference = next(
            (
                index
                for index, pair in enumerate(zip(expected_lines, actual_lines), 1)
                if pair[0] != pair[1]
            ),
            min(len(expected_lines), len(actual_lines)) + 1,
        )
        raise GateError(
            "normalized focused output differs from the checked-in baseline "
            f"(first differing line: {first_difference}; update the baseline only "
            "with an intentional, reviewed semantic change)"
        )


def parse_key_values(line: str, source: Path) -> dict[str, int]:
    parsed: dict[str, int] = {}
    for field in line.split():
        key, separator, value = field.partition("=")
        if not separator or not key or not value:
            raise GateError(f"malformed timing field in {source}: {field!r}")
        try:
            parsed[key] = int(value)
        except ValueError as error:
            raise GateError(f"non-integer timing field in {source}: {field!r}") from error
    missing = set(TIMING_FIELDS) - parsed.keys()
    if missing:
        raise GateError(f"timing log {source} is missing fields: {', '.join(sorted(missing))}")
    if any(parsed[field] < 0 for field in TIMING_FIELDS):
        raise GateError(f"timing log {source} contains a negative measurement")
    return parsed


def timing_row(log_path: Path, rss_path: Path, sample: int) -> dict[str, int | str]:
    try:
        log = log_path.read_text(encoding="utf-8")
        rss = rss_path.read_text(encoding="utf-8")
    except OSError as error:
        raise GateError(f"cannot read timing sidecar: {error}") from error
    matches = TIMING_PATTERN.findall(log)
    if len(matches) != 1:
        raise GateError(f"expected one FOCUSED_TIMING row in {log_path}, found {len(matches)}")
    timing = parse_key_values(matches[0], log_path)
    rss_match = re.search(r"^MAX_RSS_KB=(\d+)\s*$", rss, re.MULTILINE)
    if rss_match is None:
        raise GateError(f"timed process-tree RSS is missing from {rss_path}")
    return {"sample": sample, **timing, "peak_rss_kb": int(rss_match.group(1))}


def write_timing_tsv(output: Path, log_paths: list[Path], rss_paths: list[Path]) -> None:
    if not log_paths or len(log_paths) != len(rss_paths):
        raise GateError("provide matching non-empty timing-log and timing-rss lists")
    rows = [timing_row(log, rss, index) for index, (log, rss) in enumerate(zip(log_paths, rss_paths), 1)]
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=("sample", *TIMING_FIELDS, "peak_rss_kb"), delimiter="\t")
        writer.writeheader()
        writer.writerows(rows)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--timing-output", type=Path)
    parser.add_argument("--timing-log", action="append", default=[], type=Path)
    parser.add_argument("--timing-rss", action="append", default=[], type=Path)
    parser.add_argument("--nested-positive-log", type=Path)
    parser.add_argument("--nested-controls-log", type=Path)
    parser.add_argument("--nested-rollback-log", type=Path)
    parser.add_argument("--nested-output", type=Path)
    parser.add_argument("--nested-classified-output", type=Path)
    parser.add_argument("--nested-report", type=Path)
    parser.add_argument("--nested-baseline-sha")
    parser.add_argument("--nested-tested-sha")
    parser.add_argument("--nested-pr-head-sha")
    args = parser.parse_args()
    try:
        if (args.report is None) != (args.baseline is None):
            raise GateError("focused report verification requires both --report and --baseline")
        nested_mode = any((args.nested_positive_log, args.nested_controls_log,
                           args.nested_rollback_log, args.nested_output, args.nested_classified_output,
                           args.nested_report, args.nested_baseline_sha, args.nested_tested_sha,
                           args.nested_pr_head_sha))
        if args.report is not None and args.baseline is not None:
            verify_baseline(args.report, args.baseline)
        elif not nested_mode:
            raise GateError("provide focused --report/--baseline or nested ADT receipt inputs")
        if args.timing_output or args.timing_log or args.timing_rss:
            if args.timing_output is None:
                raise GateError("timing sidecars require --timing-output")
            write_timing_tsv(args.timing_output, args.timing_log, args.timing_rss)
        nested_args = (args.nested_positive_log, args.nested_controls_log,
                       args.nested_rollback_log, args.nested_output, args.nested_classified_output,
                       args.nested_report, args.nested_baseline_sha, args.nested_tested_sha)
        if any(value is not None for value in nested_args) or args.nested_pr_head_sha is not None:
            if any(value is None for value in nested_args):
                raise GateError("nested ADT receipts require positive, controls and rollback logs, tested SHA, both TSV outputs and report")
            try:
                positive_log = args.nested_positive_log.read_text(encoding="utf-8")
                controls_log = args.nested_controls_log.read_text(encoding="utf-8")
                rollback_log = args.nested_rollback_log.read_text(encoding="utf-8")
            except OSError as error:
                raise GateError(f"cannot read nested ADT execution log: {error}") from error
            root = Path(__file__).resolve().parents[2]
            rows = validate_nested_positive_log(root, positive_log, args.nested_tested_sha,
                                                args.nested_pr_head_sha, require_clean=True)
            if not re.fullmatch(r"[0-9a-f]{40}", args.nested_baseline_sha):
                raise GateError("nested ADT integrated baseline SHA is malformed")
            try:
                baseline_ancestry = subprocess.run(
                    ["git", "-C", str(root), "merge-base", "--is-ancestor", args.nested_baseline_sha,
                     args.nested_tested_sha], capture_output=True,
                )
            except OSError as error:
                raise GateError(f"cannot verify integrated baseline ancestry: {error}") from error
            if baseline_ancestry.returncode != 0:
                raise GateError("nested ADT tested revision does not descend from the recorded integrated baseline")
            classified_rows = validate_nested_control_log(root, controls_log, args.nested_tested_sha)
            rollback_rows = validate_nested_negative_sidecars(root, rollback_log, args.nested_tested_sha)
            baseline_sha = args.nested_baseline_sha
            tested_sha = args.nested_tested_sha
            pr_head_sha = args.nested_pr_head_sha or tested_sha
            fields = ("session", "session_nonce", "source_id", "store_witness", "arena_witness",
                      "resolver_nonce", "root", "source_path_tree", "source_span", "owner",
                      "source_sha256", "normalized_semantic_sha256", "proof", "baseline_sha",
                      "tested_sha", "pr_head_sha")
            args.nested_output.parent.mkdir(parents=True, exist_ok=True)
            with args.nested_output.open("w", encoding="utf-8", newline="") as stream:
                writer = csv.DictWriter(stream, fieldnames=fields, delimiter="\t")
                writer.writeheader()
                for row in rows:
                    writer.writerow({**row, "normalized_semantic_sha256": hashlib.sha256(row["proof"].encode()).hexdigest(),
                                     "baseline_sha": baseline_sha, "tested_sha": tested_sha,
                                     "pr_head_sha": pr_head_sha})
            args.nested_classified_output.parent.mkdir(parents=True, exist_ok=True)
            with args.nested_classified_output.open("w", encoding="utf-8", newline="") as stream:
                writer = csv.DictWriter(stream, fieldnames=("kind", "case", "fixture", "source_sha256", "root",
                    "source_path_tree", "source_span", "owner", "classification", "outcome",
                    "source_id", "fresh_source_id", "source_bytes",
                    "baseline_sha", "tested_sha", "pr_head_sha"), delimiter="\t")
                writer.writeheader()
                for row in [*classified_rows, *rollback_rows]:
                    row.setdefault("kind", "classified")
                    writer.writerow({**row, "baseline_sha": baseline_sha, "tested_sha": tested_sha,
                                     "pr_head_sha": pr_head_sha})
            report = [
                "# Nested recursive ADT local receipts (Scala 3.9.0)", "",
                f"- Baseline: `{baseline_sha}`", f"- Tested revision: `{tested_sha}`",
                f"- PR head: `{pr_head_sha}`", f"- Immutable source SHA-256: `{NESTED_SOURCE_SHA256}`",
                "- Positive denominator: 4 original source roots across 2 live frontend sessions",
                "- Negative sidecar: ten executed Scala-classified cases plus two executed late rollback controls",
                "- Protected roots: exact 18/18 source-root contract; unchanged 16/25 main + 15/25 analysis cohorts, two passes", "",
                "| Session | Root | Source tree | Span | Normalized SHA-256 |",
                "| --- | --- | --- | --- | --- |",
            ]
            for row in rows:
                report.append(f"| {row['session']} | {row['root']} | {row['source_path_tree']} | {row['source_span']} | `{hashlib.sha256(row['proof'].encode()).hexdigest()}` |")
            report.extend(["", "**Execution PASS; final disposition SPRINT GOAL UNMET** — revision-bound positive, control, and rollback summaries reconciled with observed executions. Final acceptance remains unmet until hosted focused CI and independent review of this same PR head succeed.", ""])
            args.nested_report.parent.mkdir(parents=True, exist_ok=True)
            args.nested_report.write_text("\n".join(report), encoding="utf-8")
    except GateError as error:
        print(f"focused audit gate: {error}", file=sys.stderr)
        return 1
    if args.baseline is not None:
        print(f"normalized focused output matches {args.baseline}")
    if args.timing_output:
        print(f"wrote informational per-run timings to {args.timing_output}")
    if args.nested_output:
        print(f"verified four nested ADT receipts and wrote {args.nested_output} and {args.nested_classified_output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
