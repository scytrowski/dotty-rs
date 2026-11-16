#!/usr/bin/env python3
"""Validate every frozen cohort attempt and require protected roots to stay typed."""

from __future__ import annotations

import argparse
import csv
import json
import os
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
ACCEPTANCE_IV_TUPLE_IDENTITY = "library/src/scala/Tuple.scala#tree=23"
ACCEPTANCE_IV_HISTORICAL_MAIN_RESULTS = {
    "library/src/scala/Option.scala#tree=118": "Boolean",
    "library/src/scala/Option.scala#tree=122": "Boolean",
    "library/src/scala/Option.scala#tree=129": "Int",
    "library/src/scala/Option.scala#tree=214": "scala.Option[typeparam#0]",
    "library/src/scala/Option.scala#tree=329": "Boolean",
    "library/src/scala/Option.scala#tree=56": "scala.Option[typeparam#0]",
    "library/src/scala/util/Either.scala#tree=19": "scala.util.Either$.LeftProjection[scala.util.Either.A,scala.util.Either.B]",
    "library/src/scala/util/Either.scala#tree=31": "scala.util.Either$.RightProjection[scala.util.Either.A,scala.util.Either.B]",
    "library/src/scala/util/Either.scala#tree=505": "Boolean",
    "library/src/scala/util/Either.scala#tree=508": "Boolean",
    "library/src/scala/util/Try.scala#tree=1098": "scala.util.Success.T",
    "library/src/scala/util/Try.scala#tree=626": "Boolean",
    "library/src/scala/util/Try.scala#tree=629": "Boolean",
    "library/src/scala/Enumeration.scala#tree=172": "Int",
    "library/src/scala/Predef.scala#tree=204": "typeparam#0",
    "library/src/scala/Predef.scala#tree=239": "typeparam#0",
    "library/src/scala/Function1.scala#tree=86": "scala.Function1[typeparam#0,scala.Function1.R]",
    "library/src/scala/Function1.scala#tree=111": "scala.Function1[scala.Function1.T1,typeparam#0]",
}
ACCEPTANCE_IV_HISTORICAL_ANALYSIS_RESULTS = {
    "library/src/scala/runtime/LazyRef.scala#tree=140": "Byte",
    "library/src/scala/Function21.scala#tree=297": "java.lang.String",
    "library/src/scala/Product16.scala#tree=119": "Int",
    "library/src/scala/concurrent/BlockContext.scala#tree=35": "scala.concurrent.BlockContext",
    "library/src/scala/Function4.scala#tree=95": "java.lang.String",
    "library/src/scala/App.scala#tree=36": "scala.Array[java.lang.String]",
    "library/src/scala/collection/mutable/ArrayBuilder.scala#tree=48": "Int",
    "library/src/scala/util/matching/Regex.scala#tree=656": "scala.collection.immutable.Seq[java.lang.String]",
    "library/src/scala/collection/immutable/Set.scala#tree=406": "Boolean",
    "library/src/scala/sys/BooleanProp.scala#tree=116": "java.lang.String",
    "library/src/scala/collection/mutable/PriorityQueue.scala#tree=250": "scala.collection.mutable.PriorityQueue.this",
    "library/src/scala/Short.scala#tree=504": "Float",
    "library/src/scala/Byte.scala#tree=516": "Double",
    "library/src/scala/Char.scala#tree=510": "Double",
    "library/src/scala/collection/mutable/HashMap.scala#tree=125": "Int",
    "library/src/scala/io/Codec.scala#tree=483": "scala.io.Codec",
}
ACCEPTANCE_IV_BASELINE_REVISION = "89f81940fc538a7b24323b82b9415cdb6a7c5404"
ACCEPTANCE_IV_BASELINE_WORKFLOW_HEAD = "92a291efad106555b2be6aa5255ef2a45eac1985"
ACCEPTANCE_IV_BASELINE_RUN_ID = "42427679560"
ACCEPTANCE_IV_BASELINE_ARTIFACT_ID = "13595484615"
ACCEPTANCE_IV_BASELINE_ARTIFACT_SHA256 = "ca52bc903a18a45c3e408290a9728fc770fc2ceabb8e4d7c337e970182a43084"
ACCEPTANCE_IV_INTEGRATION_REVISION = "f9b94d2b2bb0480de32c4fd6689a4e7813148a1f"
ACCEPTANCE_IV_INTEGRATION_RECEIPT_SHA256 = "de376c62582388643436a6ad27a156d60ead77d1f0e7cdd8993c65764fb22c39"
ACCEPTANCE_V_TUPLE_IDENTITY = "library/src/scala/Tuple.scala#tree=23"
ACCEPTANCE_V_TUPLE_RESULT = "scala.Array[java.lang.Object]"
ACCEPTANCE_V_EITHER_IDENTITY = "library/src/scala/util/Either.scala#tree=84"
ACCEPTANCE_V_PROTECTED_SHA256 = "2a31b0b10cac7fe86d3c30f2d4e632ac93f2037e9a392efaf6e6b548e5284c4d"
ACCEPTANCE_VI_ROOTS_SHA256 = "b5ab8c17e1ff513ae772d4dd9c26c7a4f4e914f61678732b5f6b0e5087852d6f"
ACCEPTANCE_VI_SOURCE_INPUTS_SHA256 = "1c6477505122043fd3c72940197c4f678401f4f8e2e61f938d5fba92c0e7ddc6"
ACCEPTANCE_VI_ARTIFACTS_SHA256 = "0b69527d4b2d2bb9d6cebba92b481f0a0f2d9aed7b2a8a2b15dbda3233128573"
ACCEPTANCE_VI_TARGETS = {
    "main": {
        "identity": "library/src/scala/Enumeration.scala#tree=459",
        "owner": "scala.Enumeration.Value",
        "method": "hashCode",
        "result": "Int",
        "source_sha256": "7370a0d85123fd15c9b75c132a38b911ad71f2a41e851e3121b5a6253939c1e8",
        "source": "library/src/scala/Enumeration.scala",
    },
    "analysis": {
        "identity": "library/src/scala/sys/PropImpl.scala#tree=99",
        "owner": "scala.sys.PropImpl",
        "method": "clear",
        "result": "Unit",
        "source_sha256": "3e9946686782a9af7259a130cf8ee7467cac28a91469c0e3f59c5fc778c6157e",
        "source": "library/src/scala/sys/PropImpl.scala",
    },
}
ACCEPTANCE_VI_JDK_VERSION = "21.0.8"
ACCEPTANCE_VI_JDK_RELEASE_SHA256 = "93b52921a5efb10ac0dcf9e3c955fa209fc83687df897d6ab034af967e2f5088"
ACCEPTANCE_VI_JAVA_SHA256 = "90d0a6d25da103678013f193db078b90ed8aeb27b66ebe78e0149616c9c1b811"
ACCEPTANCE_VI_JDK_BASE_MODULE_SHA256 = "7e05214a446061c5543f3732f653f514d4e05fa6fd2545aa792c205b24362791"
ACCEPTANCE_VI_MANIFEST_SHA256 = {
    "main": "59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244",
    "analysis": "afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc",
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
    for cohort, expected_typed, expected_integrated in (("main", 16, 19), ("analysis", 15, 16)):
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
        if cohort == "main":
            historical_results = {
                row["identity"]: row["actual_result"]
                for row in integrated
                if row.get("first_status") == "typed"
                and row.get("identity") != ACCEPTANCE_IV_TUPLE_IDENTITY
            }
            if historical_results != ACCEPTANCE_IV_HISTORICAL_MAIN_RESULTS:
                failures.append(
                    f"{integration_receipt_path}: the original 18 main typed identities/results changed"
                )
            tuple_row = integration.get(ACCEPTANCE_IV_TUPLE_IDENTITY, {})
            if (
                tuple_row.get("cohort") != "main"
                or tuple_row.get("source_tree") != "23"
                or tuple_row.get("baseline_status") != "blocked"
                or tuple_row.get("gate") != "observe"
                or tuple_row.get("first_status") != "typed"
                or tuple_row.get("second_status") != "typed"
                or tuple_row.get("actual_result") != ACCEPTANCE_IV_TARGETS[ACCEPTANCE_IV_TUPLE_IDENTITY]
                or tuple_row.get("protected_success") != "false"
                or tuple_row.get("two_pass_equal") != "true"
            ):
                failures.append(
                    f"{integration_receipt_path}: the sole additive main success must be the observed Tuple.toArray root"
                )
        else:
            historical_results = {
                row["identity"]: row["actual_result"]
                for row in integrated
                if row.get("first_status") == "typed"
            }
            if historical_results != ACCEPTANCE_IV_HISTORICAL_ANALYSIS_RESULTS:
                failures.append(
                    f"{integration_receipt_path}: the original 16 analysis typed identities/results changed"
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


def verify_acceptance_v(
    main_first: Path, main_second: Path, analysis_first: Path, analysis_second: Path,
    protected_successes_path: Path, main_manifest_path: Path, analysis_manifest_path: Path,
    minimum_main: int = 19,
) -> tuple[list[str], list[dict[str, str]]]:
    """Enforce Acceptance V against the frozen #2233 successful identities/results."""
    failures: list[str] = []
    import hashlib
    if hashlib.sha256(protected_successes_path.read_bytes()).hexdigest() != ACCEPTANCE_V_PROTECTED_SHA256:
        failures.append(f"{protected_successes_path}: frozen #2233 protected-success receipt changed")
    reports = {
        "main": (main_first, main_second, main_manifest_path),
        "analysis": (analysis_first, analysis_second, analysis_manifest_path),
    }
    frozen_rows = read_tsv(protected_successes_path)
    frozen: dict[str, dict[str, str]] = {}
    for row in frozen_rows:
        identity = row.get("identity", "")
        cohort = row.get("cohort", "")
        if cohort not in reports or not identity or identity in frozen:
            failures.append(f"{protected_successes_path}: malformed or duplicate protected root {identity!r}")
        else:
            frozen[identity] = row
    for cohort in reports:
        protected = [row for row in frozen.values() if row.get("cohort") == cohort]
        expected_count = 18 if cohort == "main" else 16
        if len(protected) != expected_count:
            failures.append(f"{protected_successes_path}: expected {expected_count} protected {cohort} roots")

    output_rows: list[dict[str, str]] = []
    for cohort, (first_path, second_path, manifest_path) in reports.items():
        first_bytes, second_bytes = first_path.read_bytes(), second_path.read_bytes()
        if first_bytes != second_bytes:
            failures.append(f"{cohort}: normalized first and second ordinary reports differ")
        failures.extend(verify(Path("docs/typer-lexical-closure-acceptance-recovery-3.9.0") /
                               ("required-main.tsv" if cohort == "main" else "required-analysis.tsv"),
                               first_path, manifest_path))
        first, summary = parse_report(first_path)
        second, second_summary = parse_report(second_path)
        if first != second or summary != second_summary:
            failures.append(f"{cohort}: independent ordinary session outcomes differ")
        if summary["selected"] != 25 or summary["attempted"] != 25:
            failures.append(f"{cohort}: expected all 25 frozen roots attempted")
        typed = {identity: row for identity, row in first.items() if row["status"] == "typed"}
        expected_protected = {identity: row for identity, row in frozen.items() if row.get("cohort") == cohort}
        for identity, baseline in expected_protected.items():
            current = typed.get(identity)
            if current is None:
                failures.append(f"{cohort}: protected #2233 success regressed: {identity}")
            elif current["actual_result"] != baseline.get("result_type"):
                failures.append(f"{identity}: protected result changed from {baseline.get('result_type')!r} to {current['actual_result']!r}")
        for identity, row in first.items():
            output_rows.append({"cohort": cohort, "identity": identity,
                                "result_type": row["actual_result"],
                                "baseline": "protected #2233" if identity in expected_protected else "-",
                                "status": row["status"], "source_tree": row["source_tree"],
                                "typed_tree": row["typed_tree"] if row["typed_tree"] not in ("", "-") else "-"})
    main, _ = parse_report(main_first)
    typed_main = sum(row["status"] == "typed" for row in main.values())
    if typed_main < minimum_main:
        failures.append(f"main cohort has {typed_main}/25 typed roots; requires at least {minimum_main}/25")
    tuple_row = main.get(ACCEPTANCE_V_TUPLE_IDENTITY)
    if tuple_row is None or tuple_row["status"] != "typed" or tuple_row["typed_tree"] in ("", "-"):
        failures.append("original Tuple.toArray whole-method root is not complete in ordinary typing")
    elif tuple_row["actual_result"] != ACCEPTANCE_V_TUPLE_RESULT:
        failures.append(f"Tuple.toArray has unexpected result shape {tuple_row['actual_result']!r}")
    elif ACCEPTANCE_V_TUPLE_IDENTITY in frozen:
        failures.append("Tuple.toArray must be additive to the frozen #2233 success set")
    return failures, output_rows


def acceptance_v_provenance_failures(
    tested_revision: str,
    pr_head_sha: str,
    prior_checks_status: str,
    hosted_run: str,
    environment: dict[str, str] | None = None,
    allowed_events: tuple[str, ...] = ("pull_request",),
) -> list[str]:
    """Require the report to be emitted by the successful PR-head audit run."""
    env = os.environ if environment is None else environment
    failures: list[str] = []
    if not re.fullmatch(r"[0-9a-f]{40}", tested_revision) or not re.fullmatch(r"[0-9a-f]{40}", pr_head_sha):
        failures.append("tested revision and reviewed PR head must be full commit SHAs")
    elif tested_revision != pr_head_sha:
        failures.append("tested revision does not equal the reviewed PR head")
    if prior_checks_status != "success":
        failures.append("hosted focused audit prerequisites did not complete successfully")
    if env.get("GITHUB_ACTIONS") != "true" or env.get("GITHUB_EVENT_NAME") not in allowed_events:
        failures.append("final acceptance PASS requires a supported GitHub Actions pull request run")
    run_id = env.get("GITHUB_RUN_ID", "")
    server_url = env.get("GITHUB_SERVER_URL", "")
    repository = env.get("GITHUB_REPOSITORY", "")
    expected_source = env.get("DOTTY_EXPECTED_SOURCE_SHA", "")
    if not run_id.isdecimal() or not server_url or not repository:
        failures.append("hosted focused audit run identity is missing")
    else:
        expected_run = f"{server_url}/{repository}/actions/runs/{run_id}"
        if hosted_run != expected_run:
            failures.append("hosted-run link does not identify this GitHub Actions run")
    if expected_source != tested_revision:
        failures.append("tested revision does not match the workflow's expected PR source SHA")
    return failures


def _manifest_identities(path: Path, expected_sha256: str, cohort: str) -> tuple[list[str], list[str]]:
    import hashlib

    failures: list[str] = []
    if hashlib.sha256(path.read_bytes()).hexdigest() != expected_sha256:
        failures.append(f"{cohort}: frozen 25-root manifest changed")
    rows = read_tsv(path)
    identities = [row.get("identity", "") for row in rows]
    if len(identities) != 25 or any(not identity for identity in identities):
        failures.append(f"{cohort}: frozen manifest must contain exactly 25 named roots")
    if len(set(identities)) != len(identities):
        failures.append(f"{cohort}: frozen manifest contains duplicate roots")
    return identities, failures


def _published_review_result(
    evidence: dict[str, object] | None, pull_request_number: int | None, pr_head_sha: str
) -> tuple[bool, str, str, str, str, list[str]]:
    """Authenticate a current-head approval from the published GitHub PR review list."""
    failures: list[str] = []
    if evidence is None or pull_request_number is None:
        return False, "", "", "", "", ["published pull request review evidence is missing"]
    pull = evidence.get("pull_request")
    reviews = evidence.get("reviews")
    if not isinstance(pull, dict) or not isinstance(reviews, list):
        return False, "", "", "", "", ["published pull request review evidence has an invalid shape"]
    head = pull.get("head")
    base = pull.get("base")
    author = pull.get("user")
    if pull.get("number") != pull_request_number:
        failures.append("published review evidence names a different pull request")
    if not isinstance(head, dict) or head.get("sha") != pr_head_sha:
        failures.append("published pull request head does not equal the tested PR head")
    if not isinstance(base, dict) or base.get("ref") != "main":
        failures.append("published pull request does not target main")
    if not isinstance(author, dict) or not isinstance(author.get("login"), str) or not author["login"]:
        failures.append("published pull request author identity is missing")
        author_login = ""
    else:
        author_login = author["login"].casefold()

    latest_by_reviewer: dict[str, tuple[dict[str, object], str]] = {}
    for review in reviews:
        if not isinstance(review, dict) or review.get("commit_id") != pr_head_sha:
            continue
        user = review.get("user")
        if not isinstance(user, dict) or not isinstance(user.get("login"), str):
            continue
        reviewer = user["login"]
        if not reviewer:
            continue
        body = review.get("body", "")
        body = body if isinstance(body, str) else ""
        logical = re.search(
            r"(?m)^<!-- agent-review:v1 verdict=(READY_FOR_MERGE|CHANGES_REQUESTED) head=([0-9a-f]{40}) -->$",
            body,
        )
        verdict = logical.group(1) if logical is not None and logical.group(2) == pr_head_sha else ""
        if reviewer.casefold() == author_login and not verdict:
            # GitHub's authenticated reviewer may own the PR. In that case,
            # only a published structured agent-review verdict supplies the
            # delegated independent review signal.
            continue
        submitted = review.get("submitted_at")
        review_id = review.get("id")
        if not isinstance(submitted, str) or not isinstance(review_id, int):
            continue
        previous = latest_by_reviewer.get(reviewer.casefold())
        if previous is None or (submitted, review_id) > (
            str(previous[0].get("submitted_at", "")), int(previous[0].get("id", 0))
        ):
            latest_by_reviewer[reviewer.casefold()] = (review, verdict)

    latest_pairs = list(latest_by_reviewer.values())
    latest_reviews = [pair[0] for pair in latest_pairs]
    if not latest_reviews:
        failures.append("no independent published review is attached to the exact PR head")
    blocking = [
        (review, verdict) for review, verdict in latest_pairs
        if str(review.get("state", "")).upper() in {"CHANGES_REQUESTED", "DISMISSED"}
        or verdict == "CHANGES_REQUESTED"
    ]
    if blocking:
        failures.append("latest independent review on the exact PR head is dismissed or requests changes")
    approved: list[tuple[dict[str, object], str]] = []
    for review, verdict in latest_pairs:
        state = str(review.get("state", "")).upper()
        user = review.get("user")
        reviewer = user.get("login", "") if isinstance(user, dict) else ""
        is_pr_author = isinstance(reviewer, str) and reviewer.casefold() == author_login
        ready_comment = state == "COMMENTED" and verdict == "READY_FOR_MERGE"
        if (state == "APPROVED" and not is_pr_author) or ready_comment:
            approved.append((review, "READY_FOR_MERGE" if ready_comment else "APPROVED"))
    if not approved:
        failures.append("no independent exact-head APPROVED or READY_FOR_MERGE COMMENT review exists")
    if failures:
        return False, "", "", "", "", failures
    selected = max(
        approved,
        key=lambda pair: (str(pair[0].get("submitted_at", "")), int(pair[0].get("id", 0))),
    )
    selected, logical_verdict = selected
    user = selected["user"]
    assert isinstance(user, dict)
    url = selected.get("html_url", "")
    return True, pr_head_sha, logical_verdict, str(user["login"]), str(url), []


def _acceptance_vi_target_receipt(
    log_path: Path, cohort: str, tested_revision: str
) -> tuple[str, list[str]]:
    """Extract one normalized receipt from a successful original-root test log."""
    failures: list[str] = []
    target = ACCEPTANCE_VI_TARGETS[cohort]
    try:
        lines = log_path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        return "", [f"{cohort}: original-method receipt is missing: {error}"]
    if sum("test result: ok. 1 passed" in line for line in lines) != 1:
        failures.append(f"{cohort}: original-method test did not report one passing test")
    tested_rows = [line for line in lines if line.startswith("DOTTY_TESTED_SHA=")]
    if tested_rows != [f"DOTTY_TESTED_SHA={tested_revision}"]:
        failures.append(f"{cohort}: original-method test receipt is missing or bound to another tested SHA")
    if cohort == "main":
        log = "\n".join(lines)
        start_marker = "ENUMERATION_HASH_CODE_ACCEPTANCE_BEGIN"
        end_marker = "ENUMERATION_HASH_CODE_ACCEPTANCE_END"
        if log.count(start_marker) != 1 or log.count(end_marker) != 1:
            return "", failures + ["main: original hashCode acceptance receipt markers are missing or duplicated"]
        start = log.index(start_marker) + len(start_marker)
        end = log.index(end_marker)
        if start >= end:
            return "", failures + ["main: original hashCode acceptance receipt markers are reversed"]
        receipt = log[start:end].strip()
        required = (
            f"root={target['identity']} span=TextRange {{ start: 9430, end: 9466 }}",
            f"owner=.{target['owner']} method=.{target['owner']}.{target['method']} owner_provenance=canonical-source",
            "result=scala.Int typed_root_kind=DefDef typed_rhs_result=scala.Int source_to_typed_root=true",
            "id_tree=457 span=TextRange { start: 9461, end: 9463 } symbol=.scala.Enumeration.Value.id provenance=canonical-source",
            "selection_tree=458 span=TextRange { start: 9461, end: 9466 } typed_kind=Select member=Any.## provenance=Builtin signature=ByName[scala.Int] result=scala.Int",
            "rollback=store,typed-arena,type-index,typed-index,source-to-typed-index restored; same-session retry=success",
        )
    else:
        receipts = []
        for line in lines:
            marker = "root=library/src/scala/sys/PropImpl.scala#tree=99 "
            if marker in line:
                receipts.append(line[line.index(marker):])
        if len(receipts) != 1:
            return "", failures + ["analysis: original PropImpl.clear acceptance receipt is missing or duplicated"]
        receipt = receipts[0]
        required = (
            f"root={target['identity']} span=1297..1334 owner=.{target['owner']} result=Unit rhs=1317..1334",
            "operation=Shrinkable.-= operation_result=Shrinkable receiver=tree96 argument=tree97",
            "adaptation=Block(statements=1,final=())",
            "mappings=95:1310..1314,96:1317..1327,97:1331..1334,98:1317..1334,99:1297..1334",
        )
    for fragment in required:
        if fragment not in receipt:
            failures.append(f"{cohort}: original-method receipt is missing exact evidence {fragment!r}")
    return receipt, failures


def verify_acceptance_vi(
    main_first: Path,
    main_second: Path,
    analysis_first: Path,
    analysis_second: Path,
    baseline_roots_path: Path,
    main_manifest_path: Path,
    analysis_manifest_path: Path,
    source_inputs_path: Path,
    source_root: Path,
    artifact_manifest_path: Path,
    main_receipt_logs: tuple[Path, Path],
    analysis_receipt_logs: tuple[Path, Path],
    tested_revision: str,
    pr_head_sha: str,
    prior_checks_status: str,
    hosted_run: str,
    artifact_reference: str,
    reviewed_head_sha: str,
    review_state: str,
    jdk_version: str,
    jdk_release_sha256: str,
    java_binary_sha256: str,
    jdk_base_module_sha256: str,
    environment: dict[str, str] | None = None,
    review_evidence: dict[str, object] | None = None,
    pull_request_number: int | None = None,
) -> tuple[list[str], list[dict[str, str]], bool]:
    """Verify additive Acceptance VI against the immutable #2297 complete-root baseline."""
    import hashlib

    failures: list[str] = []
    if hashlib.sha256(baseline_roots_path.read_bytes()).hexdigest() != ACCEPTANCE_VI_ROOTS_SHA256:
        failures.append("frozen #2297 root baseline changed")
    if jdk_version != ACCEPTANCE_VI_JDK_VERSION:
        failures.append(f"pinned JDK must be {ACCEPTANCE_VI_JDK_VERSION}, got {jdk_version!r}")
    if jdk_release_sha256 != ACCEPTANCE_VI_JDK_RELEASE_SHA256:
        failures.append("pinned JDK release-file SHA-256 does not match Temurin 21.0.8")
    if java_binary_sha256 != ACCEPTANCE_VI_JAVA_SHA256:
        failures.append("pinned JDK java-binary SHA-256 does not match Temurin 21.0.8")
    if jdk_base_module_sha256 != ACCEPTANCE_VI_JDK_BASE_MODULE_SHA256:
        failures.append("pinned JDK java.base.jmod SHA-256 does not match Temurin 21.0.8")
    if hashlib.sha256(artifact_manifest_path.read_bytes()).hexdigest() != ACCEPTANCE_VI_ARTIFACTS_SHA256:
        failures.append("pinned Scala artifact manifest changed")

    manifests = {
        "main": (main_manifest_path, ACCEPTANCE_VI_MANIFEST_SHA256["main"]),
        "analysis": (analysis_manifest_path, ACCEPTANCE_VI_MANIFEST_SHA256["analysis"]),
    }
    expected_manifests: dict[str, set[str]] = {}
    for cohort, (path, expected_hash) in manifests.items():
        identities, manifest_failures = _manifest_identities(path, expected_hash, cohort)
        failures.extend(manifest_failures)
        expected_manifests[cohort] = set(identities)
    manifest_rows = {
        "main": {row.get("identity", ""): row for row in read_tsv(main_manifest_path)},
        "analysis": {row.get("identity", ""): row for row in read_tsv(analysis_manifest_path)},
    }

    baseline_rows = read_tsv(baseline_roots_path)
    baseline: dict[str, dict[str, str]] = {}
    for row in baseline_rows:
        identity = row.get("identity", "")
        cohort = row.get("cohort", "")
        if cohort not in manifests or not identity or identity in baseline:
            failures.append(f"#2297 baseline has malformed or duplicate identity {identity!r}")
        else:
            baseline[identity] = row
    if len(baseline) != 50:
        failures.append(f"#2297 baseline must retain all 50 exact roots; found {len(baseline)}")
    for cohort in manifests:
        baseline_cohort = {identity for identity, row in baseline.items() if row.get("cohort") == cohort}
        if baseline_cohort != expected_manifests[cohort]:
            failures.append(f"{cohort}: manifest membership/denominator differs from #2297 baseline")
        protected = [row for row in baseline.values()
                     if row.get("cohort") == cohort and row.get("current_status") == "typed"]
        expected_protected = 20 if cohort == "main" else 16
        if len(protected) != expected_protected:
            failures.append(f"#2297 baseline must protect {expected_protected} {cohort} typed roots")
    if sum(row.get("current_status") == "typed" and row.get("cohort") == "main" for row in baseline.values()) != 20:
        failures.append("#2297 baseline main typed count is not 20/25")
    if sum(row.get("current_status") == "typed" and row.get("cohort") == "analysis" for row in baseline.values()) != 16:
        failures.append("#2297 baseline analysis typed count is not 16/25")

    source_inputs = read_tsv(source_inputs_path)
    source_map = {row.get("source_path", ""): row.get("sha256", "") for row in source_inputs}
    if len(source_map) != len(source_inputs) or not source_map:
        failures.append("frozen Scala source input list is empty or duplicated")
    elif hashlib.sha256(source_inputs_path.read_bytes()).hexdigest() != ACCEPTANCE_VI_SOURCE_INPUTS_SHA256:
        failures.append("frozen Scala source input manifest changed")
    for source_path, expected_hash in source_map.items():
        try:
            actual_hash = hashlib.sha256((source_root / source_path).read_bytes()).hexdigest()
        except OSError:
            actual_hash = "missing"
        if not source_path or actual_hash != expected_hash:
            failures.append(f"pinned Scala source input changed or missing: {source_path}")
    for target in ACCEPTANCE_VI_TARGETS.values():
        if source_map.get(target["source"]) != target["source_sha256"]:
            failures.append(f"target source identity/hash is not frozen: {target['source']}")

    if prior_checks_status != "success":
        failures.append("hosted focused audit prerequisites did not complete successfully")
    provenance_failures = acceptance_v_provenance_failures(
        tested_revision,
        pr_head_sha,
        prior_checks_status,
        hosted_run,
        environment,
        allowed_events=("pull_request", "pull_request_review", "workflow_dispatch"),
    )
    failures.extend(provenance_failures)
    env = os.environ if environment is None else environment
    review_verified, authenticated_reviewed_head, authenticated_review_state, authenticated_reviewer, authenticated_review_url, review_failures = (
        _published_review_result(review_evidence, pull_request_number, pr_head_sha)
    )
    pull_for_pending = review_evidence.get("pull_request") if isinstance(review_evidence, dict) else None
    review_list_for_pending = review_evidence.get("reviews") if isinstance(review_evidence, dict) else None
    pending_pr_without_head_review = (
        env.get("GITHUB_EVENT_NAME") == "pull_request"
        and isinstance(pull_for_pending, dict)
        and pull_for_pending.get("number") == pull_request_number
        and isinstance(pull_for_pending.get("head"), dict)
        and pull_for_pending["head"].get("sha") == pr_head_sha
        and isinstance(pull_for_pending.get("user"), dict)
        and isinstance(review_list_for_pending, list)
        and not any(
            isinstance(review, dict)
            and review.get("commit_id") == pr_head_sha
            and isinstance(review.get("user"), dict)
            and isinstance(review["user"].get("login"), str)
            and review["user"]["login"].casefold() != str(pull_for_pending["user"].get("login", "")).casefold()
            for review in review_list_for_pending
        )
    )
    if pending_pr_without_head_review:
        # The ordinary PR run is allowed to publish an explicit unmet report
        # while approval is pending. Review and dispatch runs fail closed.
        review_failures = [
            failure for failure in review_failures
            if "no independent published review is attached" not in failure
            and "no independent exact-head APPROVED or READY_FOR_MERGE COMMENT review exists" not in failure
        ]
    failures.extend(review_failures)
    # CLI strings are output hints only. The TSV and PASS state use values resolved
    # from the published review evidence returned by GitHub's API.
    reviewed_head_sha = authenticated_reviewed_head
    review_state = authenticated_review_state
    review_event = env.get("GITHUB_EVENT_NAME") in {"pull_request_review", "workflow_dispatch"}
    if review_verified and not review_event:
        review_verified = False
        failures.append("final Acceptance VI PASS requires a pull_request_review or workflow_dispatch run")

    cohort_reports = {
        "main": (main_first, main_second),
        "analysis": (analysis_first, analysis_second),
    }
    output_rows: list[dict[str, str]] = []
    typed_counts: dict[str, int] = {}
    for cohort, (first_path, second_path) in cohort_reports.items():
        try:
            if first_path.read_bytes() != second_path.read_bytes():
                failures.append(f"{cohort}: normalized complete-root double attempts differ")
            first, summary = parse_report(first_path)
            second, second_summary = parse_report(second_path)
        except (OSError, ValueError) as error:
            failures.append(f"{cohort}: malformed or missing ordinary report: {error}")
            continue
        failures.extend(_validate_attempted_rows(first, summary, first_path))
        failures.extend(_validate_attempted_rows(second, second_summary, second_path))
        if first != second or summary != second_summary:
            failures.append(f"{cohort}: report identities or normalized outcomes differ")
        if set(first) != expected_manifests[cohort]:
            missing = sorted(expected_manifests[cohort] - set(first))
            extra = sorted(set(first) - expected_manifests[cohort])
            failures.append(f"{cohort}: report membership mismatch; missing={missing}, extra={extra}")
        if summary["selected"] != 25 or summary["attempted"] != 25:
            failures.append(f"{cohort}: all 25 original roots must be attempted")
        typed = {identity: row for identity, row in first.items() if row["status"] == "typed"}
        typed_counts[cohort] = len(typed)
        for identity, base_row in baseline.items():
            if base_row.get("cohort") != cohort or base_row.get("current_status") != "typed":
                continue
            current = typed.get(identity)
            expected_result = base_row.get("current_actual_result", "")
            if current is None:
                failures.append(f"{cohort}: protected #2297 typed root regressed: {identity}")
                continue
            if current["actual_result"] != expected_result:
                failures.append(f"{identity}: protected result changed from {expected_result!r} to {current['actual_result']!r}")
            if current["owner_path"] != base_row.get("owner", ""):
                failures.append(f"{identity}: protected owner changed")
        target = ACCEPTANCE_VI_TARGETS[cohort]
        target_row = typed.get(target["identity"])
        base_target = baseline.get(target["identity"])
        if base_target is None or base_target.get("current_status") == "typed":
            failures.append(f"{target['identity']}: target is not newly additive to the #2297 baseline")
        if target_row is None:
            failures.append(f"{target['identity']}: complete original DefDef is not typed")
        else:
            if target_row["actual_result"] != target["result"]:
                failures.append(f"{target['identity']}: expected exact result {target['result']}, got {target_row['actual_result']!r}")
            if target_row["owner_path"] != target["owner"] or target_row["method_name"] != target["method"]:
                failures.append(f"{target['identity']}: target source owner or method name changed")
            if target_row["source_tree"] != target["identity"].rsplit("#tree=", 1)[1]:
                failures.append(f"{target['identity']}: exact original source-tree mapping is missing")
        required_count = 21 if cohort == "main" else 17
        if typed_counts[cohort] < required_count:
            failures.append(f"{cohort} cohort has {typed_counts[cohort]}/25 typed roots; requires at least {required_count}/25")
        if len(typed) - expected_count_for(cohort) < 1:
            failures.append(f"{cohort} has no new complete original root beyond #2297")
        for identity in sorted(expected_manifests[cohort]):
            row = first.get(identity, {})
            base_row = baseline.get(identity, {})
            manifest_row = manifest_rows[cohort].get(identity, {})
            identity_fields = (
                ("owner_path", "owner"),
                ("method_name", "method"),
                ("line", "line"),
                ("range", "source_range"),
                ("expected_result", "declared_result"),
            )
            for report_field, baseline_field in identity_fields:
                if row.get(report_field, "") != base_row.get(baseline_field, ""):
                    failures.append(f"{identity}: report {report_field} differs from frozen root baseline")
            for report_field, manifest_field in (
                ("owner_path", "owner_path"),
                ("method_name", "method_name"),
            ):
                if report_field in row and manifest_field in manifest_row and row[report_field] != manifest_row[manifest_field]:
                    failures.append(f"{identity}: report {report_field} differs from frozen cohort manifest")
            output_rows.append({
                "cohort": cohort,
                "identity": identity,
                "owner": row.get("owner_path", ""),
                "source_range": row.get("range", ""),
                "source_sha256": source_map.get(identity.partition("#tree=")[0], ""),
                "result": row.get("actual_result", "not_available"),
                "baseline_status": base_row.get("current_status", "missing"),
                "baseline_result": base_row.get("current_actual_result", "not_available"),
                "status": row.get("status", "missing"),
                "source_tree": row.get("source_tree", ""),
                "typed_tree": row.get("typed_tree", ""),
            })

    receipt_pairs = {"main": main_receipt_logs, "analysis": analysis_receipt_logs}
    receipt_hashes: dict[str, str] = {}
    for cohort, paths in receipt_pairs.items():
        receipts = []
        for path in paths:
            receipt, receipt_failures = _acceptance_vi_target_receipt(path, cohort, tested_revision)
            failures.extend(receipt_failures)
            receipts.append(receipt)
        if not receipts[0] or receipts[0] != receipts[1]:
            failures.append(f"{cohort}: original-method receipts are missing or nondeterministic across fresh test processes")
        receipt_hashes[cohort] = hashlib.sha256(receipts[0].encode("utf-8")).hexdigest() if receipts[0] else "missing"

    for row in output_rows:
        target = next((target for target in ACCEPTANCE_VI_TARGETS.values() if row["identity"] == target["identity"]), None)
        if target:
            row.update({
                "target_owner": target["owner"],
                "target_result": target["result"],
                "source_sha256": target["source_sha256"],
                "receipt_sha256": receipt_hashes[row["cohort"]],
                "tested_revision": tested_revision,
                "reviewed_head_sha": reviewed_head_sha or "pending",
                "review_status": review_state or "pending",
                "reviewer_login": authenticated_reviewer or "pending",
                "review_url": authenticated_review_url or "pending",
                "hosted_run": hosted_run,
                "artifact_reference": artifact_reference,
                "final_merge_sha": "pending until merge",
            })
        else:
            row.update({
                "target_owner": "", "target_result": "",
                "receipt_sha256": "", "tested_revision": tested_revision,
                "reviewed_head_sha": reviewed_head_sha or "pending",
                "review_status": review_state or "pending", "hosted_run": hosted_run,
                "reviewer_login": authenticated_reviewer or "pending",
                "review_url": authenticated_review_url or "pending",
                "artifact_reference": artifact_reference,
                "final_merge_sha": "pending until merge",
            })

    # A technically green PR run is not the release closeout until a successful
    # independent approval names this same head; the review event reruns this gate.
    passed = not failures and review_verified
    return failures, output_rows, passed


def expected_count_for(cohort: str) -> int:
    return 20 if cohort == "main" else 16


def write_acceptance_vi_outputs(
    tsv_path: Path,
    report_path: Path,
    rows: list[dict[str, str]],
    failures: list[str],
    passed: bool,
    typed_counts: tuple[int, int],
    tested_revision: str,
    pr_head_sha: str,
    reviewed_head_sha: str,
    review_state: str,
    hosted_run: str,
    artifact_reference: str,
    jdk_version: str,
    jdk_release_sha256: str,
    java_binary_sha256: str,
    jdk_base_module_sha256: str,
) -> None:
    columns = (
        "cohort", "identity", "owner", "source_range", "source_sha256", "result", "baseline_status", "baseline_result",
        "status", "source_tree", "typed_tree", "target_owner", "target_result",
        "receipt_sha256", "tested_revision", "pr_head_sha", "reviewed_head_sha", "review_status", "reviewer_login", "review_url",
        "jdk_version", "jdk_release_sha256", "java_binary_sha256", "jdk_base_module_sha256",
        "hosted_run", "artifact_reference", "final_merge_sha",
    )
    tsv_path.parent.mkdir(parents=True, exist_ok=True)
    with tsv_path.open("w", encoding="utf-8", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=columns, delimiter="\t", lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({
                **row,
                "pr_head_sha": pr_head_sha,
                "jdk_version": jdk_version,
                "jdk_release_sha256": jdk_release_sha256,
                "java_binary_sha256": java_binary_sha256,
                "jdk_base_module_sha256": jdk_base_module_sha256,
            })
    report_path.parent.mkdir(parents=True, exist_ok=True)
    outcome = "PASS" if passed else "SPRINT GOAL UNMET"
    report_path.write_text(
        "# Typer Stdlib Acceptance VI closeout (Scala 3.9.0)\n\n"
        f"**Outcome: {outcome}.** Main: {typed_counts[0]}/25; analysis: {typed_counts[1]}/25.\n\n"
        f"#2297 baseline: 20/25 main and 16/25 analysis; both exact targets were blocked. "
        f"Protected successes retained: 20 main and 16 analysis with exact result shapes.\n\n"
        f"Tested revision: `{tested_revision}`; PR head: `{pr_head_sha}`; "
        f"reviewed head: `{reviewed_head_sha or 'pending'}`; independent review: `{review_state or 'pending'}`.\n\n"
        f"Published reviewer: `{next((row['reviewer_login'] for row in rows if row.get('reviewer_login') not in (None, '', 'pending')), 'pending')}`; "
        f"review record: {next((row['review_url'] for row in rows if row.get('review_url') not in (None, '', 'pending')), 'pending')}.\n\n"
        f"Pinned Scala source: `{PINNED_SCALA_REVISION}`; JDK: `{jdk_version}` "
        f"(release `{jdk_release_sha256}`, java `{java_binary_sha256}`, java.base.jmod `{jdk_base_module_sha256}`).\n\n"
        f"Hosted focused audit: {hosted_run}; artifact: `{artifact_reference}`. "
        "Final merge SHA: pending until merge.\n\n"
        f"Targets: `Enumeration.Value.hashCode` tree 459 => Int (receipt {next((r['receipt_sha256'] for r in rows if r['identity'] == ACCEPTANCE_VI_TARGETS['main']['identity']), 'missing')}); "
        f"`PropImpl.clear` tree 99 => Unit (receipt {next((r['receipt_sha256'] for r in rows if r['identity'] == ACCEPTANCE_VI_TARGETS['analysis']['identity']), 'missing')}).\n\n"
        + ("Closeout blockers:\n" + "".join(f"- {failure}\n" for failure in failures)
           if failures else ("The hosted workflow completed the release gate on the exact reviewed head.\n"
                             if passed else "The CI evidence passed, but an approved independent review of this exact head is pending.\n")),
        encoding="utf-8",
    )


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
        f"shared #2183 integration receipt plus the additive #2234 Tuple result: "
        f"`f9b94d2b2bb0480de32c4fd6689a4e7813148a1f` "
        f"({integration_main}/25 main, including the unchanged prior 18 plus Tuple; "
        f"{integration_analysis}/25 analysis); "
        f"tested revision: `{tested_revision}`; PR head: `{pr_head_sha}`; final merge SHA: pending.\n\n"
        f"Hosted run: {hosted_run}\n\n"
        "The gate verifies all 50 exact ordinary roots, complete attempted-root accounting, "
        "the measured first/second #2179 outcomes, the separate #2183 integration outcomes, "
        "the 16 main and 15 analysis protected results, the exact original 18 main "
        "integration results, the sole additive `Tuple.toArray` success, and the 17/25 plus newly typed "
        "since #2179 target requirement. Historical manifest status is not a measured outcome.\n",
        encoding="utf-8",
    )


def main() -> int:
    if sys.argv[1:2] == ["acceptance-vi"]:
        parser = argparse.ArgumentParser()
        parser.add_argument("command")
        parser.add_argument("--main-first", type=Path, required=True)
        parser.add_argument("--main-second", type=Path, required=True)
        parser.add_argument("--analysis-first", type=Path, required=True)
        parser.add_argument("--analysis-second", type=Path, required=True)
        parser.add_argument("--baseline-roots", type=Path, required=True)
        parser.add_argument("--main-manifest", type=Path, required=True)
        parser.add_argument("--analysis-manifest", type=Path, required=True)
        parser.add_argument("--source-inputs", type=Path, required=True)
        parser.add_argument("--source-root", type=Path, required=True)
        parser.add_argument("--artifact-manifest", type=Path, required=True)
        parser.add_argument("--main-receipt-first", type=Path, required=True)
        parser.add_argument("--main-receipt-second", type=Path, required=True)
        parser.add_argument("--analysis-receipt-first", type=Path, required=True)
        parser.add_argument("--analysis-receipt-second", type=Path, required=True)
        parser.add_argument("--output", type=Path, required=True)
        parser.add_argument("--report", type=Path, required=True)
        parser.add_argument("--tested-revision", required=True)
        parser.add_argument("--pr-head-sha", required=True)
        parser.add_argument("--pull-request-number", type=int, required=True)
        parser.add_argument("--review-evidence", type=Path, required=True)
        parser.add_argument("--prior-checks-status", default="unknown")
        parser.add_argument("--hosted-run", default="pending")
        parser.add_argument("--artifact-reference", default="pending")
        parser.add_argument("--jdk-version", default="unknown")
        parser.add_argument("--jdk-release-sha256", default="")
        parser.add_argument("--java-binary-sha256", default="")
        parser.add_argument("--jdk-base-module-sha256", default="")
        arguments = parser.parse_args()
    elif sys.argv[1:2] == ["acceptance-v"]:
        parser = argparse.ArgumentParser()
        parser.add_argument("command")
        parser.add_argument("--main-first", type=Path, required=True)
        parser.add_argument("--main-second", type=Path, required=True)
        parser.add_argument("--analysis-first", type=Path, required=True)
        parser.add_argument("--analysis-second", type=Path, required=True)
        parser.add_argument("--protected-successes", type=Path, required=True)
        parser.add_argument("--output", type=Path, required=True)
        parser.add_argument("--tested-revision", required=True)
        parser.add_argument("--pr-head-sha", required=True)
        parser.add_argument("--report", type=Path, required=True)
        parser.add_argument("--prior-checks-status", default="unknown")
        parser.add_argument("--hosted-run", default="pending")
        arguments = parser.parse_args()
    elif sys.argv[1:2] == ["acceptance-iv"]:
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
        if arguments.command == "acceptance-vi":
            review_evidence = json.loads(arguments.review_evidence.read_text(encoding="utf-8"))
            failures, rows, passed = verify_acceptance_vi(
                arguments.main_first,
                arguments.main_second,
                arguments.analysis_first,
                arguments.analysis_second,
                arguments.baseline_roots,
                arguments.main_manifest,
                arguments.analysis_manifest,
                arguments.source_inputs,
                arguments.source_root,
                arguments.artifact_manifest,
                (arguments.main_receipt_first, arguments.main_receipt_second),
                (arguments.analysis_receipt_first, arguments.analysis_receipt_second),
                arguments.tested_revision,
                arguments.pr_head_sha,
                arguments.prior_checks_status,
                arguments.hosted_run,
                arguments.artifact_reference,
                "",
                "",
                arguments.jdk_version,
                arguments.jdk_release_sha256,
                arguments.java_binary_sha256,
                arguments.jdk_base_module_sha256,
                review_evidence=review_evidence,
                pull_request_number=arguments.pull_request_number,
            )
            main_count = sum(row["cohort"] == "main" and row["status"] == "typed" for row in rows)
            analysis_count = sum(row["cohort"] == "analysis" and row["status"] == "typed" for row in rows)
            authenticated_reviewed_head = next(
                (row.get("reviewed_head_sha", "") for row in rows if row.get("reviewed_head_sha") not in (None, "", "pending")),
                "",
            )
            authenticated_review_state = next(
                (row.get("review_status", "") for row in rows if row.get("review_status") not in (None, "", "pending")),
                "",
            )
            write_acceptance_vi_outputs(
                arguments.output,
                arguments.report,
                rows,
                failures,
                passed,
                (main_count, analysis_count),
                arguments.tested_revision,
                arguments.pr_head_sha,
                authenticated_reviewed_head,
                authenticated_review_state,
                arguments.hosted_run,
                arguments.artifact_reference,
                arguments.jdk_version,
                arguments.jdk_release_sha256,
                arguments.java_binary_sha256,
                arguments.jdk_base_module_sha256,
            )
            for failure in failures:
                print(failure, file=sys.stderr)
            if not passed and not failures:
                print("independent review has not approved this exact PR head", file=sys.stderr)
            return 0 if not failures else 1
        if arguments.command == "acceptance-v":
            gate_failures, rows = verify_acceptance_v(
                arguments.main_first, arguments.main_second, arguments.analysis_first,
                arguments.analysis_second, arguments.protected_successes,
                Path("docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv"),
                Path("docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv"),
            )
            provenance_failures = acceptance_v_provenance_failures(
                arguments.tested_revision, arguments.pr_head_sha,
                arguments.prior_checks_status, arguments.hosted_run,
                allowed_events=("pull_request", "pull_request_review", "workflow_dispatch"),
            )
            failures = gate_failures + provenance_failures
            arguments.output.parent.mkdir(parents=True, exist_ok=True)
            with arguments.output.open("w", encoding="utf-8", newline="") as output:
                writer = csv.DictWriter(output, fieldnames=("cohort", "identity", "result_type", "baseline", "status", "source_tree", "typed_tree"), delimiter="\t", lineterminator="\n")
                writer.writeheader()
                writer.writerows(rows)
            passed = not failures
            main_count = sum(row["cohort"] == "main" and row["status"] == "typed" for row in rows)
            analysis_count = sum(row["cohort"] == "analysis" and row["status"] == "typed" for row in rows)
            either = next((row for row in rows if row["identity"] == ACCEPTANCE_V_EITHER_IDENTITY), None)
            stretch = (
                "not present in frozen report" if either is None
                else "blocked" if either["status"].startswith("blocked::")
                else either["status"]
            )
            arguments.report.parent.mkdir(parents=True, exist_ok=True)
            arguments.report.write_text(
                "# Typer Stdlib Acceptance V closeout (Scala 3.9.0)\n\n"
                f"**Outcome: {'PASS' if passed else 'SPRINT GOAL UNMET'}**; main {main_count}/25, analysis {analysis_count}/25.\n\n"
                f"Tested revision: `{arguments.tested_revision}`; PR head: `{arguments.pr_head_sha}`; final merge SHA: pending.\n\n"
                f"Hosted focused audit: {arguments.hosted_run}\n\n"
                "Both frozen 25-root cohorts were compared across two independent deterministic sessions. "
                "The exact #2233 successful identities and result shapes are protected, and the original "
                "unprepared Tuple.toArray DefDef must be complete with its exact result. Analysis retains its "
                f"original denominator and protected successes. Optional Either.swap stretch: {stretch}. "
                "Prepared-only Tuple evidence receives no credit.\n\n"
                + ("Closeout blockers:\n" + "".join(f"- {failure}\n" for failure in failures)
                   if failures else "Hosted provenance checks passed for this reviewed PR head.\n"),
                encoding="utf-8",
            )
        elif arguments.command == "acceptance-iv":
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
            parser.error("select ordinary, acceptance-iv, acceptance-v, or acceptance-vi")
    except (OSError, ValueError, KeyError) as error:
        if getattr(arguments, "command", None) == "acceptance-vi":
            write_acceptance_vi_outputs(
                arguments.output,
                arguments.report,
                [],
                [f"evidence could not be read or validated: {error}"],
                False,
                (0, 0),
                arguments.tested_revision,
                arguments.pr_head_sha,
                "",
                "",
                arguments.hosted_run,
                arguments.artifact_reference,
                arguments.jdk_version,
                arguments.jdk_release_sha256,
                arguments.java_binary_sha256,
                arguments.jdk_base_module_sha256,
            )
        elif getattr(arguments, "command", None) == "acceptance-iv":
            _write_acceptance_iv_markdown(
                arguments.report,
                [],
                False,
                arguments.tested_revision,
                arguments.pr_head_sha,
                arguments.hosted_run,
            )
        elif getattr(arguments, "command", None) == "acceptance-v":
            arguments.report.parent.mkdir(parents=True, exist_ok=True)
            arguments.report.write_text(
                "# Typer Stdlib Acceptance V closeout (Scala 3.9.0)\n\n"
                "**Outcome: SPRINT GOAL UNMET.** The fresh acceptance evidence could not be validated.\n\n"
                f"Tested revision: `{arguments.tested_revision}`; PR head: `{arguments.pr_head_sha}`.\n\n"
                f"Hosted focused audit: {arguments.hosted_run}\n",
                encoding="utf-8",
            )
        print(error, file=sys.stderr)
        return 2
    if failures:
        print(f"{getattr(arguments, 'command', 'ordinary')} gate failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("Stdlib Acceptance closeout passed" if arguments.command in {"acceptance-iv", "acceptance-v"} else "ordinary cohort gate passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
