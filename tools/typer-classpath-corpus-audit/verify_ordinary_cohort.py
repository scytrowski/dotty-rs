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
ACCEPTANCE_IV_BASELINE_REVISION = "89f81940fc538a7b24323b82b9415cdb6a7c5404"
ACCEPTANCE_IV_BASELINE_WORKFLOW_HEAD = "92a291efad106555b2be6aa5255ef2a45eac1985"
ACCEPTANCE_IV_BASELINE_RUN_ID = "42427679560"
ACCEPTANCE_IV_BASELINE_ARTIFACT_ID = "13595484615"
ACCEPTANCE_IV_BASELINE_ARTIFACT_SHA256 = "ca52bc903a18a45c3e408290a9728fc770fc2ceabb8e4d7c337e970182a43084"
ACCEPTANCE_IV_INTEGRATION_REVISION = "f9b94d2b2bb0480de32c4fd6689a4e7813148a1f"
ACCEPTANCE_IV_INTEGRATION_RECEIPT_SHA256 = "e783ece1710b7b271c3ca3f9dd12b566b212b655c28d89da35bc900fbccf486e"


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
    measured_baseline_path: Path,
    integration_receipt_path: Path,
    required_main_path: Path,
    required_analysis_path: Path,
    minimum_main: int = 17,
    main_manifest_path: Path = Path("docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv"),
    analysis_manifest_path: Path = Path("docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv"),
) -> tuple[list[str], list[dict[str, str]]]:
    """Enforce the frozen 25+25 acceptance-IV closeout without cohort reshaping."""
    failures: list[str] = []
    measured_rows = read_tsv(measured_baseline_path)
    integration_rows = read_tsv(integration_receipt_path)
    measured = {row["identity"]: row for row in measured_rows}
    integration = {row["identity"]: row for row in integration_rows}
    if len(measured) != len(measured_rows):
        failures.append(f"{measured_baseline_path}: duplicate measured #2179 identity")
    if len(integration) != len(integration_rows):
        failures.append(f"{integration_receipt_path}: duplicate integration identity")
    for row in measured_rows:
        expected = {
            "scala_revision": PINNED_SCALA_REVISION,
            "measured_revision": ACCEPTANCE_IV_BASELINE_REVISION,
            "workflow_head_sha": ACCEPTANCE_IV_BASELINE_WORKFLOW_HEAD,
            "workflow_run_id": ACCEPTANCE_IV_BASELINE_RUN_ID,
            "artifact_id": ACCEPTANCE_IV_BASELINE_ARTIFACT_ID,
            "artifact_sha256": ACCEPTANCE_IV_BASELINE_ARTIFACT_SHA256,
        }
        for key, value in expected.items():
            if row.get(key) != value:
                failures.append(f"{measured_baseline_path}: unexpected #2179 receipt {key}")
        if row.get("first_status") != row.get("second_status"):
            failures.append(f"{row.get('identity')}: #2179 measured attempts disagree")
        if row.get("first_actual_result") != row.get("second_actual_result"):
            failures.append(f"{row.get('identity')}: #2179 measured results disagree")
    for row in integration_rows:
        if row.get("first_status") != row.get("second_status") or row.get("two_pass_equal") != "true":
            failures.append(f"{row.get('identity')}: #2183 integration attempts disagree")
    if set(measured) != set(integration):
        failures.append("measured #2179 and integration receipts must contain the same exact roots")
    for cohort, expected_typed, expected_integrated in (("main", 16, 18), ("analysis", 15, 16)):
        rows = [row for row in measured_rows if row.get("cohort") == cohort]
        typed = 0
        if len(rows) != 25:
            failures.append(f"{measured_baseline_path}: #2179 {cohort} receipt must have 25 roots")
        for row in rows:
            identity_tree = _identity_tree(row.get("identity", ""))
            if identity_tree is None or row.get("source_tree") != identity_tree:
                failures.append(f"{row.get('identity')}: invalid measured #2179 source-tree mapping")
            status = row.get("first_status", "")
            if status == "typed":
                typed += 1
                if row.get("first_actual_result") in ("", "not_available"):
                    failures.append(f"{row.get('identity')}: typed #2179 root has no result")
            elif not status.startswith("blocked::") or row.get("first_actual_result") != "not_available":
                failures.append(f"{row.get('identity')}: malformed measured #2179 outcome")
        if typed != expected_typed:
            failures.append(
                f"{measured_baseline_path}: #2179 {cohort} measured {typed}/25 typed roots; expected {expected_typed}/25"
            )
        integrated = [row for row in integration_rows if row.get("cohort") == cohort]
        if len(integrated) != 25:
            failures.append(f"{integration_receipt_path}: integration {cohort} receipt must have 25 roots")
        integrated_typed = 0
        for row in integrated:
            identity_tree = _identity_tree(row.get("identity", ""))
            status = row.get("first_status", "")
            if identity_tree is None or row.get("source_tree") != identity_tree:
                failures.append(f"{row.get('identity')}: invalid integration source-tree mapping")
            if status != "typed" and not status.startswith("blocked::"):
                failures.append(f"{row.get('identity')}: malformed integration outcome")
            if status == "typed":
                integrated_typed += 1
                if row.get("actual_result") in ("", "not_available"):
                    failures.append(f"{row.get('identity')}: typed integration root has no result")
            elif row.get("actual_result") != "not_available":
                failures.append(f"{row.get('identity')}: blocked integration root has inconsistent result")
        if integrated_typed != expected_integrated:
            failures.append(
                f"{integration_receipt_path}: #2183 {cohort} receipt has {integrated_typed}/25 typed roots; expected {expected_integrated}/25"
            )
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
    for cohort, (_, protected_path, _) in inputs.items():
        required = read_tsv(protected_path)
        protected_ids = {
            row["identity"] for row in required if row.get("gate") == "required_typed"
        }
        measured_typed_ids = {
            row["identity"]
            for row in measured_rows
            if row.get("cohort") == cohort and row.get("first_status") == "typed"
        }
        if measured_typed_ids != protected_ids:
            failures.append(
                f"{measured_baseline_path}: measured #2179 {cohort} typed identities "
                "do not match the protected identity set"
            )
    output_rows: list[dict[str, str]] = []
    typed_main = 0
    for cohort, (report_path, protected_path, manifest_path) in inputs.items():
        failures.extend(verify(protected_path, report_path, manifest_path))
        report, summary = parse_report(report_path)
        cohort_ids = {row["identity"] for row in measured_rows if row.get("cohort") == cohort}
        if len(cohort_ids) != 25 or set(report) != cohort_ids or summary["selected"] != 25:
            failures.append(f"{cohort}: expected the unchanged exact 25-root cohort")
        integration_ids = {row["identity"] for row in integration_rows if row.get("cohort") == cohort}
        if integration_ids != cohort_ids:
            failures.append(f"{cohort}: integration receipt does not preserve the exact 25-root cohort")
        if any(row.get("cohort") not in inputs for row in measured_rows + integration_rows):
            failures.append("measured or integration receipt contains an unexpected cohort")
        typed_count = sum(row["status"] == "typed" for row in report.values())
        if cohort == "main":
            typed_main = typed_count
        for identity, row in report.items():
            prior = measured.get(identity, {})
            integrated = integration.get(identity, {})
            measured_status = prior.get("first_status", "")
            measured_result = prior.get("first_actual_result", "")
            integration_status = integrated.get("first_status", "")
            integration_result = integrated.get("actual_result", "")
            identity_tree = _identity_tree(identity)
            if identity_tree is None or prior.get("source_tree") != identity_tree:
                failures.append(f"{identity}: missing or malformed measured #2179 identity")
            if integrated.get("source_tree") != identity_tree:
                failures.append(f"{identity}: missing or malformed integration identity")
            if measured_status == "typed" and (
                integration_status != "typed" or integration_result != measured_result
            ):
                failures.append(f"{identity}: measured #2179 success changed in the integration receipt")
            if measured_status == "typed" and (
                row["status"] != "typed" or row["actual_result"] != measured_result
            ):
                failures.append(f"{identity}: protected identity or result changed")
            output_rows.append({
                "cohort": cohort,
                "identity": identity,
                "source_tree": row["source_tree"],
                "measured_first_status": prior.get("first_status", ""),
                "measured_second_status": prior.get("second_status", ""),
                "integration_status": integration_status,
                "after_status": row["status"],
                "measured_actual_result": measured_result,
                "integration_actual_result": integration_result,
                "expected_result": row["expected_result"],
                "result_comparison": (
                    "target two-session proof required"
                    if identity in ACCEPTANCE_IV_TARGETS
                    else "protected result retained"
                    if measured_status == "typed" and row["status"] == "typed"
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
        and measured.get(identity, {}).get("first_status") != "typed"
        and measured.get(identity, {}).get("second_status") != "typed"
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
        elif identity not in measured:
            failures.append(f"measured #2179 receipt is missing target identity {identity}")
        elif measured[identity].get("first_status") == "typed" or measured[identity].get("second_status") == "typed":
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
        "cohort", "identity", "source_tree", "measured_first_status", "measured_second_status",
        "integration_status", "after_status", "first_blocker", "measured_actual_result",
        "integration_actual_result", "expected_result", "actual_result", "result_comparison", "typed_tree", "scala_revision",
        "baseline_revision", "baseline_workflow_run", "baseline_artifact_id",
        "baseline_artifact_sha256", "integration_revision", "integration_receipt_sha256",
        "tested_revision",
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
                "baseline_revision": ACCEPTANCE_IV_BASELINE_REVISION,
                "baseline_workflow_run": ACCEPTANCE_IV_BASELINE_RUN_ID,
                "baseline_artifact_id": ACCEPTANCE_IV_BASELINE_ARTIFACT_ID,
                "baseline_artifact_sha256": ACCEPTANCE_IV_BASELINE_ARTIFACT_SHA256,
                "integration_revision": ACCEPTANCE_IV_INTEGRATION_REVISION,
                "integration_receipt_sha256": ACCEPTANCE_IV_INTEGRATION_RECEIPT_SHA256,
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
            if row["identity"] in ACCEPTANCE_IV_TARGETS
            and row["measured_first_status"] != "typed"
            and row["measured_second_status"] != "typed"
            and row["after_status"] == "typed"
        ),
        None,
    )
    target_integration = next(
        (
            row["integration_status"]
            for row in main_rows
            if row["identity"] == target
        ),
        "unavailable",
    )
    measured_main = sum(row["measured_first_status"] == "typed" for row in main_rows)
    measured_analysis = sum(row["measured_first_status"] == "typed" for row in analysis_rows)
    integration_main = sum(row["integration_status"] == "typed" for row in main_rows)
    integration_analysis = sum(row["integration_status"] == "typed" for row in analysis_rows)
    gate_status = "PASS" if passed else "FAIL"
    path.write_text(
        "# Stdlib Acceptance IV closeout (Scala 3.9.0)\n\n"
        f"**Outcome: SPRINT GOAL UNMET pending independent review.** Hosted acceptance gate: {gate_status}. "
        f"Main: {typed_main}/25; analysis: {typed_analysis}/25.\n\n"
        f"New unchanged-cohort target since measured #2179: {target or 'none'}; "
        f"its separate #2183 integration-receipt status is {target_integration}.\n\n"
        "Prepared `Tuple.toArray` proof is conditional on explicit owner preparation; "
        "only a typed original target row in the ordinary audit counts toward acceptance.\n\n"
        f"Source revision: `{PINNED_SCALA_REVISION}`; measured #2179: `89f81940fc538a7b24323b82b9415cdb6a7c5404` "
        f"({measured_main}/25 main, {measured_analysis}/25 analysis); "
        f"separate #2183 integration receipt: `f9b94d2b2bb0480de32c4fd6689a4e7813148a1f` "
        f"({integration_main}/25 main, {integration_analysis}/25 analysis); "
        f"tested revision: `{tested_revision}`; PR head: `{pr_head_sha}`; final merge SHA: pending.\n\n"
        f"Hosted run: {hosted_run}\n\n"
        "The gate verifies all 50 exact ordinary roots, complete attempted-root accounting, "
        "the measured first/second #2179 outcomes, the separate #2183 integration outcomes, "
        "the 16 main and 15 analysis protected results, and the 17/25 plus newly typed "
        "since #2179 target requirement. Historical manifest status is not a measured outcome.\n",
        encoding="utf-8",
    )


def main() -> int:
    if sys.argv[1:2] == ["acceptance-iv"]:
        parser = argparse.ArgumentParser()
        parser.add_argument("command")
        parser.add_argument("--main-report", type=Path, required=True)
        parser.add_argument("--analysis-report", type=Path, required=True)
        parser.add_argument("--measured-baseline", type=Path, required=True)
        parser.add_argument("--integration-receipt", type=Path, required=True)
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
                arguments.measured_baseline,
                arguments.integration_receipt,
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
