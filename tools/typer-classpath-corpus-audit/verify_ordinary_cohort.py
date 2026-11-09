#!/usr/bin/env python3
"""Validate every frozen cohort attempt and require protected roots to stay typed."""

from __future__ import annotations

import argparse
import csv
import re
import sys
from pathlib import Path


PINNED_SCALA_REVISION = "777528f19a58e794c9954a42f433373472ec57f8"
REPORT_COLUMNS = (
    "method",
    "owner_path",
    "method_name",
    "line",
    "range",
    "modifiers",
    "expected_result",
    "actual_result",
    "complexity",
    "status",
    "source_tree",
    "typed_tree",
)
SUMMARY_KEYS = {
    "schema",
    "pinned_scala_revision",
    "selected",
    "attempted",
    "typed",
    "blocked",
    "excluded",
    "not_attempted",
}
ACCEPTANCE_IV_TARGETS = {
    "library/src/scala/Option.scala#tree=214": "scala.Option[typeparam#0]",
    "library/src/scala/Tuple.scala#tree=23": "scala.Array[java.lang.Object]",
}


def read_tsv(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as source:
        return list(csv.DictReader(source, delimiter="\t"))


def _identity_tree(identity: str) -> str | None:
    source_path, separator, tree = identity.rpartition("#tree=")
    if not separator or not source_path or not re.fullmatch(r"(?:0|[1-9][0-9]*)", tree):
        return None
    return tree


def _parse_summary(lines: list[str], header_index: int, path: Path) -> dict[str, int | str]:
    values: dict[str, int | str] = {}
    for line in lines[:header_index]:
        if not line:
            continue
        key, separator, value = line.partition("=")
        if not separator or not key or not value:
            raise ValueError(f"{path}: malformed ordinary audit summary row: {line!r}")
        if key not in SUMMARY_KEYS:
            raise ValueError(f"{path}: unexpected ordinary audit summary key: {key!r}")
        if key in values:
            raise ValueError(f"{path}: duplicate ordinary audit summary key: {key!r}")
        if key in {"schema", "pinned_scala_revision"}:
            values[key] = value
        elif not re.fullmatch(r"(?:0|[1-9][0-9]*)", value):
            raise ValueError(f"{path}: malformed ordinary audit counter {key}={value!r}")
        else:
            values[key] = int(value)
    missing = SUMMARY_KEYS - values.keys()
    if missing:
        raise ValueError(f"{path}: ordinary audit summary is missing {sorted(missing)}")
    if values["schema"] != "1":
        raise ValueError(f"{path}: unsupported ordinary audit schema {values['schema']!r}")
    if values["pinned_scala_revision"] != PINNED_SCALA_REVISION:
        raise ValueError(f"{path}: ordinary audit used an unexpected Scala source revision")
    return values


def parse_report(path: Path) -> tuple[dict[str, dict[str, str]], dict[str, int | str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    try:
        header_index = next(
            index
            for index, line in enumerate(lines)
            if line.split("\t") == list(REPORT_COLUMNS)
        )
    except (StopIteration, ValueError) as error:
        raise ValueError(f"{path}: ordinary audit header is missing") from error
    summary = _parse_summary(lines, header_index, path)
    rows = list(csv.DictReader(lines[header_index:], delimiter="\t"))
    for index, row in enumerate(rows, start=header_index + 2):
        malformed = (
            None in row
            or set(row) != set(REPORT_COLUMNS)
            or any(value is None for value in row.values())
        )
        if malformed:
            raise ValueError(f"{path}:{index}: malformed ordinary audit row width")
    by_identity = {row["method"]: row for row in rows}
    if len(by_identity) != len(rows):
        raise ValueError(f"{path}: duplicate ordinary-method identity")
    return by_identity, summary


def _validate_attempted_rows(
    report: dict[str, dict[str, str]], summary: dict[str, int | str], path: Path
) -> list[str]:
    failures: list[str] = []
    counts = {"typed": 0, "blocked": 0, "excluded": 0, "not_attempted": 0}
    for identity, row in report.items():
        identity_tree = _identity_tree(identity)
        if identity_tree is None:
            failures.append(f"{identity}: malformed method identity")
            continue
        if not row["owner_path"] or not row["method_name"]:
            failures.append(f"{identity}: attempted outcome is missing its method owner or name")
        status = row["status"]
        if status == "typed":
            counts["typed"] += 1
            if not re.fullmatch(r"(?:0|[1-9][0-9]*)", row["typed_tree"]):
                failures.append(f"{identity}: typed row has no valid typed-tree mapping")
            if not row["actual_result"] or row["actual_result"] == "not_available":
                failures.append(f"{identity}: typed row has no result shape")
        elif status.startswith("blocked::") and status.removeprefix("blocked::").strip():
            counts["blocked"] += 1
            if row["typed_tree"] not in ("", "-"):
                failures.append(f"{identity}: blocked row unexpectedly has a typed-tree mapping")
            if row["actual_result"] != "not_available":
                failures.append(f"{identity}: blocked row has an inconsistent result shape")
        else:
            counts["not_attempted"] += 1
            failures.append(
                f"{identity}: expected typed or blocked::* attempted outcome, got {status!r}"
            )
        if row["source_tree"] != identity_tree:
            failures.append(
                f"{identity}: invalid source-tree mapping; identity requires {identity_tree}, "
                f"got {row['source_tree']!r}"
            )

    selected = summary["selected"]
    attempted = summary["attempted"]
    for key, actual in counts.items():
        if summary[key] != actual:
            failures.append(
                f"{path}: {key} counter is {summary[key]}, but report rows contain {actual}"
            )
    if selected != len(report):
        failures.append(
            f"{path}: selected counter is {selected}, but report has {len(report)} rows"
        )
    if attempted != counts["typed"] + counts["blocked"]:
        failures.append(
            f"{path}: attempted counter {attempted} does not equal typed + blocked "
            f"({counts['typed']} + {counts['blocked']})"
        )
    if selected != attempted:
        failures.append(
            f"{path}: selected={selected} but attempted={attempted}; every root must be attempted"
        )
    if summary["excluded"] != 0:
        failures.append(f"{path}: excluded counter must be zero")
    if summary["not_attempted"] != 0:
        failures.append(f"{path}: not_attempted counter must be zero")
    return failures


def verify(
    cohort_path: Path,
    report_path: Path,
    manifest_path: Path | None = None,
) -> list[str]:
    cohort = read_tsv(cohort_path)
    report, summary = parse_report(report_path)
    failures = []
    failures.extend(_validate_attempted_rows(report, summary, report_path))
    required_rows = [row for row in cohort if row.get("gate") == "required_typed"]
    required_ids = [row["identity"] for row in required_rows]
    if len(required_ids) != len(set(required_ids)):
        failures.append(f"{cohort_path}: duplicate required_typed identity")

    if manifest_path is not None:
        manifest = read_tsv(manifest_path)
        manifest_ids = [row["identity"] for row in manifest]
        if len(manifest_ids) != len(set(manifest_ids)):
            failures.append(f"{manifest_path}: duplicate frozen manifest identity")
        for row in manifest:
            identity_tree = _identity_tree(row.get("identity", ""))
            manifest_tree = row.get("source_tree", "")
            if identity_tree is None or (manifest_tree and manifest_tree != identity_tree):
                failures.append(
                    f"{row.get('identity', '<missing identity>')}: "
                    "invalid frozen source-tree mapping"
                )
        expected = set(manifest_ids)
        actual = set(report)
        for identity in sorted(expected - actual):
            failures.append(f"{identity}: missing from ordinary audit report")
        for identity in sorted(actual - expected):
            failures.append(f"{identity}: unexpected in ordinary audit report")
        if summary["selected"] != len(manifest):
            failures.append(
                f"{manifest_path}: selected counter is {summary['selected']}, "
                f"expected {len(manifest)} frozen roots"
            )

    for row in cohort:
        if row.get("gate") != "required_typed":
            continue
        identity = row["identity"]
        audit = report.get(identity)
        if audit is None:
            failures.append(f"{identity}: missing from ordinary audit report")
            continue
        status = audit["status"]
        if status != "typed" or audit["typed_tree"] in ("", "-"):
            failures.append(f"{identity}: expected fully typed, got {status}")
            continue
        if audit["source_tree"] != row["source_tree"]:
            failures.append(
                f"{identity}: source-tree mapping changed; expected {row['source_tree']}, "
                f"got {audit['source_tree']}"
            )
        # TreeId values are arena-local diagnostics. New successful roots can
        # change allocation order without changing the canonical method-to-tree
        # mapping, so the gate requires an owned typed root but does not compare
        # its numeric ID across sessions.
        actual_result = audit["actual_result"]
        if actual_result != row["oracle_result"]:
            failures.append(
                f"{identity}: result shape changed; expected {row['oracle_result']}, "
                f"got {actual_result}"
            )
    if not any(row.get("gate") == "required_typed" for row in cohort):
        failures.append(f"{cohort_path}: no required_typed rows")
    return failures


def verify_acceptance_iv(
    main_report_path: Path,
    analysis_report_path: Path,
    baseline_receipt_path: Path,
    required_main_path: Path,
    required_analysis_path: Path,
    minimum_main: int = 17,
    main_manifest_path: Path = Path("docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv"),
    analysis_manifest_path: Path = Path("docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv"),
) -> tuple[list[str], list[dict[str, str]]]:
    """Enforce the frozen 25+25 acceptance-IV closeout without cohort reshaping."""
    failures: list[str] = []
    baseline_rows = read_tsv(baseline_receipt_path)
    baseline = {row["identity"]: row for row in baseline_rows}
    if len(baseline) != len(baseline_rows):
        failures.append(f"{baseline_receipt_path}: duplicate baseline identity")
    inputs = {
        "main": (
            main_report_path,
            required_main_path,
            main_manifest_path,
        ),
        "analysis": (
            analysis_report_path,
            required_analysis_path,
            analysis_manifest_path,
        ),
    }
    output_rows: list[dict[str, str]] = []
    typed_main = 0
    for cohort, (report_path, protected_path, manifest_path) in inputs.items():
        failures.extend(verify(protected_path, report_path, manifest_path))
        report, summary = parse_report(report_path)
        cohort_ids = {row["identity"] for row in baseline_rows if row.get("cohort") == cohort}
        if len(cohort_ids) != 25 or set(report) != cohort_ids or summary["selected"] != 25:
            failures.append(f"{cohort}: expected the unchanged exact 25-root cohort")
        if any(row.get("cohort") not in inputs for row in baseline_rows):
            failures.append(f"{baseline_receipt_path}: unexpected cohort")
        typed_count = sum(row["status"] == "typed" for row in report.values())
        if cohort == "main":
            typed_main = typed_count
        for identity, row in report.items():
            prior = baseline.get(identity, {})
            if prior.get("protected_success") == "true" and (
                row["status"] != "typed" or row["actual_result"] != prior.get("actual_result")
            ):
                failures.append(f"{identity}: protected identity or result changed")
            output_rows.append({
                "cohort": cohort,
                "identity": identity,
                "source_tree": row["source_tree"],
                "baseline_status": prior.get("baseline_status", ""),
                "after_status": row["status"],
                "expected_result": row["expected_result"],
                "result_comparison": (
                    "target two-session proof required"
                    if identity in ACCEPTANCE_IV_TARGETS
                    else "protected result retained"
                    if prior.get("protected_success") == "true" and row["status"] == "typed"
                    else "additional typed root; observed result recorded"
                    if row["status"] == "typed"
                    else "not typed"
                ),
                "first_blocker": row["status"].removeprefix("blocked::") if row["status"].startswith("blocked::") else "-",
                "actual_result": row["actual_result"],
                "typed_tree": row["typed_tree"] if row["typed_tree"] not in ("", "-") else "-",
            })
    if typed_main < minimum_main:
        failures.append(f"main cohort has {typed_main}/25 typed roots; requires at least {minimum_main}/25")
    main, _ = parse_report(main_report_path)
    typed_targets = [
        identity for identity, expected_result in ACCEPTANCE_IV_TARGETS.items()
        if identity in main
        and baseline.get(identity, {}).get("baseline_status") != "typed"
        and main[identity]["status"] == "typed"
        and main[identity]["typed_tree"] not in ("", "-")
        and main[identity]["actual_result"] == expected_result
    ]
    if not typed_targets:
        failures.append("no exact original Option.map or Tuple.toArray typed root in ordinary main audit")
    for identity, expected_result in ACCEPTANCE_IV_TARGETS.items():
        row = main.get(identity)
        if row is None:
            failures.append(f"main cohort is missing target identity {identity}")
        elif identity not in baseline:
            failures.append(f"baseline receipt is missing target identity {identity}")
        elif baseline[identity].get("baseline_status") == "typed" and row["status"] == "typed":
            failures.append(f"{identity}: target was already typed in the #2179 baseline")
        elif row["status"] == "typed" and row["actual_result"] != expected_result:
            failures.append(f"{identity}: typed target has unexpected result shape {row['actual_result']!r}")
    return failures, output_rows


def _write_acceptance_iv_tsv(
    path: Path,
    rows: list[dict[str, str]],
    tested_revision: str,
    pr_head_sha: str,
    hosted_run: str,
) -> None:
    columns = (
        "cohort", "identity", "source_tree", "baseline_status", "after_status",
        "first_blocker", "expected_result", "actual_result", "result_comparison", "typed_tree", "scala_revision",
        "baseline_revision", "integration_revision", "tested_revision",
        "pr_head_sha", "final_merge_sha", "focused_tests", "regression_guard", "hosted_run",
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=columns, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({
                **row,
                "scala_revision": PINNED_SCALA_REVISION,
                "baseline_revision": "89f81940fc538a7b24323b82b9415cdb6a7c5404",
                "integration_revision": "f9b94d2b2bb0480de32c4fd6689a4e7813148a1f",
                "tested_revision": tested_revision,
                "pr_head_sha": pr_head_sha,
                "final_merge_sha": "pending until merge",
                "focused_tests": "captures_acceptance_iv_relation_operands_before_transaction_rollback; original_tuple_to_array_types_after_explicit_canonical_owner_preparation; ordinary 25+25 audit",
                "regression_guard": "exact protected identities/results; complete attempts; >=17 main; exact target; both root-specific negative/rollback controls",
                "hosted_run": hosted_run,
            })


def _write_acceptance_iv_markdown(
    path: Path,
    rows: list[dict[str, str]],
    passed: bool,
    tested_revision: str,
    pr_head_sha: str,
    hosted_run: str,
) -> None:
    main_rows = [row for row in rows if row["cohort"] == "main"]
    analysis_rows = [row for row in rows if row["cohort"] == "analysis"]
    typed_main = sum(row["after_status"] == "typed" for row in main_rows)
    typed_analysis = sum(row["after_status"] == "typed" for row in analysis_rows)
    target = next(
        (
            row["identity"]
            for row in main_rows
            if row["identity"] in ACCEPTANCE_IV_TARGETS and row["after_status"] == "typed"
        ),
        None,
    )
    gate_status = "PASS" if passed else "FAIL"
    path.write_text(
        "# Stdlib Acceptance IV closeout (Scala 3.9.0)\n\n"
        f"**Outcome: SPRINT GOAL UNMET pending independent review.** Hosted acceptance gate: {gate_status}. "
        f"Main: {typed_main}/25; analysis: {typed_analysis}/25.\n\n"
        f"New unchanged-cohort target: {target or 'none'}.\n\n"
        "Prepared `Tuple.toArray` proof is conditional on explicit owner preparation; "
        "only a typed original target row in the ordinary audit counts toward acceptance.\n\n"
        f"Source revision: `{PINNED_SCALA_REVISION}`; baseline: `89f81940fc538a7b24323b82b9415cdb6a7c5404`; "
        "integration: `f9b94d2b2bb0480de32c4fd6689a4e7813148a1f`; "
        f"tested revision: `{tested_revision}`; PR head: `{pr_head_sha}`; final merge SHA: pending.\n\n"
        f"Hosted run: {hosted_run}\n\n"
        "The gate verifies all 50 exact ordinary roots, complete attempted-root accounting, "
        "the 16 main and 15 analysis protected identities and results, and the 17/25 plus "
        "new-target requirement.\n",
        encoding="utf-8",
    )


def main() -> int:
    if sys.argv[1:2] == ["acceptance-iv"]:
        parser = argparse.ArgumentParser()
        parser.add_argument("command")
        parser.add_argument("--main-report", type=Path, required=True)
        parser.add_argument("--analysis-report", type=Path, required=True)
        parser.add_argument("--baseline-receipt", type=Path, required=True)
        parser.add_argument("--required-main", type=Path, required=True)
        parser.add_argument("--required-analysis", type=Path, required=True)
        parser.add_argument("--minimum-main", type=int, default=17)
        parser.add_argument("--output", type=Path, required=True)
        parser.add_argument("--tested-revision", required=True)
        parser.add_argument("--pr-head-sha", required=True)
        parser.add_argument("--report", type=Path, required=True)
        parser.add_argument("--prior-checks-status", required=True)
        parser.add_argument("--hosted-run", default="pending")
        arguments = parser.parse_args()
    else:
        parser = argparse.ArgumentParser()
        parser.add_argument("cohort", type=Path)
        parser.add_argument("report", type=Path)
        parser.add_argument("--manifest", type=Path)
        arguments = parser.parse_args()
        arguments.command = "ordinary"
    try:
        if arguments.command == "acceptance-iv":
            failures, rows = verify_acceptance_iv(
                arguments.main_report,
                arguments.analysis_report,
                arguments.baseline_receipt,
                arguments.required_main,
                arguments.required_analysis,
                arguments.minimum_main,
            )
            _write_acceptance_iv_tsv(
                arguments.output,
                rows,
                arguments.tested_revision,
                arguments.pr_head_sha,
                arguments.hosted_run,
            )
            _write_acceptance_iv_markdown(
                arguments.report,
                rows,
                not failures and arguments.prior_checks_status == "success",
                arguments.tested_revision,
                arguments.pr_head_sha,
                arguments.hosted_run,
            )
        elif arguments.command == "ordinary":
            failures = verify(arguments.cohort, arguments.report, arguments.manifest)
        else:
            parser.error("select ordinary or acceptance-iv")
    except (OSError, ValueError, KeyError) as error:
        if getattr(arguments, "command", None) == "acceptance-iv":
            _write_acceptance_iv_markdown(
                arguments.report,
                [],
                False,
                arguments.tested_revision,
                arguments.pr_head_sha,
                arguments.hosted_run,
            )
        print(error, file=sys.stderr)
        return 2
    if failures:
        print("ordinary cohort gate failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("Stdlib Acceptance IV closeout passed" if arguments.command == "acceptance-iv" else "ordinary cohort gate passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
