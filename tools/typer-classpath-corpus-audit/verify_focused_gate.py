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
NESTED_BASELINE_SHA = "0fc457776374f69d6b16187c007721a85b2557a0"
NESTED_TEST = "nested_recursive_adt_original_roots_are_attempted_in_two_fresh_sessions"
NESTED_RECEIPT_HASHES = {
    "rust-peelleft-complete.tsv": "c365d35c8fe0df1c45dd0052b80c6e7e6f86e365f326c7404c4508514856001d",
    "rust-sum-complete.tsv": "f75fc10eac428ff81324cefecf46607b31f6bf78470d06bdfe4b03029e868e3c",
    "negative-cases.tsv": "966f67c7eaff94084eb58b759f69f72db0c2481d36b6e16f8fc080ab5e5206fd",
    "rust-late-failure.tsv": "b23431067a439b5a1a997e90023e2301e3eaeb0656968209cf8a7a16222bc916",
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
            ["git", "-C", str(root), "status", "--porcelain=v1", "--untracked-files=all"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise GateError(f"cannot verify tested repository cleanliness: {error}") from error
    return not result.stdout


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

    if not re.search(rf"^test {re.escape(NESTED_TEST)} \.\.\. ok$", log_text, re.MULTILINE):
        raise GateError("nested ADT positive test was not observed as executed and passing")
    result_rows = re.findall(r"^test result: ok\. 1 passed; 0 failed; 0 ignored;", log_text, re.MULTILINE)
    if len(result_rows) != 1:
        raise GateError("nested ADT positive log must prove exactly one executed test")
    if log_text.count(f"ISSUE2137_TESTED_SHA\t{tested_sha}") != 1:
        raise GateError("nested ADT positive log is missing the exact tested revision marker")
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


def validate_nested_negative_sidecars(root: Path, rollback_log: str) -> None:
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
    test_name = "nested_late_failures_restore_and_retry_before_original_roots_recover"
    if len(re.findall(rf"^test (?:[A-Za-z0-9_:]+::)?{re.escape(test_name)} \.\.\. ok$", rollback_log, re.MULTILINE)) != 1:
        raise GateError("#2136 nested rollback test was not observed as executed and passing")
    if len(re.findall(r"^test result: ok\. 1 passed; 0 failed;", rollback_log, re.MULTILINE)) != 1:
        raise GateError("#2136 rollback log must prove exactly one executed test")


def validate_nested_control_log(control_log: str) -> None:
    expected = {
        "nested_peelleft_controls_keep_bounded_rejections_and_positional_identity",
        "nested_sum_bounded_invalid_roots_fail_without_partial_mappings",
        "nested_sum_rejects_a_sibling_case_binder_escape_then_types_original_sum",
    }
    observed = set(re.findall(r"^test ([A-Za-z0-9_]+) \.\.\. ok$", control_log, re.MULTILINE))
    if expected - observed:
        raise GateError(f"nested ADT classified controls did not execute: {sorted(expected - observed)}")
    for name in expected:
        if len(re.findall(rf"^test {re.escape(name)} \.\.\. ok$", control_log, re.MULTILINE)) != 1:
            raise GateError(f"nested ADT classified control duplicated execution: {name}")
    if len(re.findall(r"^test result: ok\. \d+ passed; 0 failed;", control_log, re.MULTILINE)) != 1:
        raise GateError("nested ADT classified control tests did not pass")


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
    parser.add_argument("--nested-report", type=Path)
    parser.add_argument("--nested-tested-sha")
    parser.add_argument("--nested-pr-head-sha")
    args = parser.parse_args()
    try:
        if (args.report is None) != (args.baseline is None):
            raise GateError("focused report verification requires both --report and --baseline")
        nested_mode = any((args.nested_positive_log, args.nested_controls_log,
                           args.nested_rollback_log, args.nested_output,
                           args.nested_report, args.nested_tested_sha, args.nested_pr_head_sha))
        if args.report is not None and args.baseline is not None:
            verify_baseline(args.report, args.baseline)
        elif not nested_mode:
            raise GateError("provide focused --report/--baseline or nested ADT receipt inputs")
        if args.timing_output or args.timing_log or args.timing_rss:
            if args.timing_output is None:
                raise GateError("timing sidecars require --timing-output")
            write_timing_tsv(args.timing_output, args.timing_log, args.timing_rss)
        nested_args = (args.nested_positive_log, args.nested_controls_log,
                       args.nested_rollback_log, args.nested_output,
                       args.nested_report, args.nested_tested_sha)
        if any(value is not None for value in nested_args) or args.nested_pr_head_sha is not None:
            if any(value is None for value in nested_args):
                raise GateError("nested ADT receipts require positive, controls and rollback logs, tested SHA, TSV output and report")
            try:
                positive_log = args.nested_positive_log.read_text(encoding="utf-8")
                controls_log = args.nested_controls_log.read_text(encoding="utf-8")
                rollback_log = args.nested_rollback_log.read_text(encoding="utf-8")
            except OSError as error:
                raise GateError(f"cannot read nested ADT execution log: {error}") from error
            root = Path(__file__).resolve().parents[2]
            rows = validate_nested_positive_log(root, positive_log, args.nested_tested_sha,
                                                args.nested_pr_head_sha, require_clean=True)
            validate_nested_control_log(controls_log)
            validate_nested_negative_sidecars(root, rollback_log)
            baseline_sha = NESTED_BASELINE_SHA
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
            report = [
                "# Nested recursive ADT local receipts (Scala 3.9.0)", "",
                f"- Baseline: `{baseline_sha}`", f"- Tested revision: `{tested_sha}`",
                f"- PR head: `{pr_head_sha}`", f"- Immutable source SHA-256: `{NESTED_SOURCE_SHA256}`",
                "- Positive denominator: 4 original source roots across 2 live frontend sessions",
                "- Negative classified cases: 10; late rollback receipts: 2 executed controls", "",
                "| Session | Root | Source tree | Span | Normalized SHA-256 |",
                "| --- | --- | --- | --- | --- |",
            ]
            for row in rows:
                report.append(f"| {row['session']} | {row['root']} | {row['source_path_tree']} | {row['source_span']} | `{hashlib.sha256(row['proof'].encode()).hexdigest()}` |")
            report.extend(["", "**PASS** — local evidence was emitted only after the executed positive test, four canonical receipts, fresh-session witnesses, pinned classifications, and actual rollback receipt all verified.", ""])
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
        print(f"verified four nested ADT receipts and wrote {args.nested_output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
