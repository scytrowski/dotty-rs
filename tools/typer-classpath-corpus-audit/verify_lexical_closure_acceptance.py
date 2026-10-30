#!/usr/bin/env python3
"""Reject missing or changed inputs to the pinned lexical-closure acceptance gate."""

from __future__ import annotations

import argparse
import csv
import hashlib
import re
import sys
from pathlib import Path


INPUT_HASHES = {
    "docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv":
        "59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244",
    "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv":
        "afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/six-root-gate.tsv":
        "937b18923c5720a33fd438d30469e58f1e0bca9111181f48bd5d5ce726f953c6",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/scala-artifacts.sha256.tsv":
        "0b69527d4b2d2bb9d6cebba92b481f0a0f2d9aed7b2a8a2b15dbda3233128573",
    "crates/dotty-typer/src/typer/tests/lexical_closure_six_root_gate.rs":
        "ecc396a0f16c3039be22eaf4480e246516cfc00fc722a043a3a242d14f1fb635",
    "crates/dotty-typer/src/typer/tests/lexical_closure_rollback.rs":
        "817b20f0e3d92be4028c6e967842b5bfc38a5734ed0eabdc031f59f8b369ad4b",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/run":
        "d5ecfcc612bde2ac9eacd3b69a3da0a0e26466a7ae66f78e0c7dbb0409ad02e6",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/CombinedCaptures.scala":
        "862a0a0ef16adccdfd1227ef23562bce22428f43ef0be9d824ae6796a04c1dca",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/CombinedCaptures.scala39-typed-tree.txt":
        "7a06140879ad2f43ad34b800f242cce4525cd3169305697bed2ac10bafe5b316",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/MethodParameters.scala":
        "a2103c926f2d280b21d178e438a2735a214f9039cf62e1150f1cb5f0a3984744",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/LocalValues.scala":
        "4c6c73e81efdbdaa72f13437a4fa5000a0c7fa975ea8382640f508791e8fe65e",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/ThisCapture.scala":
        "2d5c29fc188904cbbd7409784cdb935bccc9f79d576e4d4099497e320ac259e2",
    "crates/dotty-typer/tests/fixtures/lexical-closure-contract/SameOwnerMembers.scala":
        "3ec081068d583a0aebfbc2faad83c048deb44bd614f6c942b112e71814743375",
    "docs/typer-lexical-closure-rollback-matrix-3.9.0.tsv":
        "902b32b60b9ade3f0f76fdbb2e29007f58c1651c25f1394ca206dd69a15fbf27",
    "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv":
        "05cdcf0f9185a32075ca8f6bfd3a37084e4127384c0940c9783ae2626dc9377f",
    "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-analysis.tsv":
        "021bb2d5a1e26ea575bf3f89b92c5a5e41acc2e124e753e43715608f0fe4bce4",
}

POSITIVE_ROOTS = {
    "strict-class-C",
    "lexical-closure-contract/MethodParameters.scala",
    "lexical-closure-contract/LocalValues.scala",
    "lexical-closure-contract/ThisCapture.scala",
    "lexical-closure-contract/SameOwnerMembers.scala",
    "lexical-closure-contract/CombinedCaptures.scala",
}
NEGATIVE_CASES = {
    "late_argument",
    "late_result",
    "unknown_name",
    "mutable_capture",
    "by_name_capture",
    "wrong_qualified_this",
    "inherited_member",
    "super_member",
    "shadowing",
}
TESTS = {
    "positive": (
        "typer::tests::lexical_closure_contract::six_root_gate::"
        "six_frozen_roots_type_as_complete_methods_in_two_independent_sessions",
    ),
    "negative": (
        "typer::tests::lexical_closure_rollback::failed_captured_methods_restore_state_and_allow_stable_retry",
        "typer::tests::lexical_closure_rollback::late_argument_and_result_errors_are_observed_after_both_capture_records",
        "typer::tests::lexical_closure_rollback::scala_invalid_qualification_and_deferred_member_forms_rollback",
    ),
}


class AcceptanceInputError(ValueError):
    """An issue-frozen acceptance input is absent, changed, or malformed."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def check_hash(path: Path, expected: str) -> None:
    actual = sha256(path)
    if actual != expected:
        raise AcceptanceInputError(
            f"{path}: SHA-256 changed; expected {expected}, got {actual}"
        )


def read_tsv(path: Path) -> list[dict[str, str]]:
    try:
        with path.open(encoding="utf-8", newline="") as source:
            return list(csv.DictReader(source, delimiter="\t"))
    except OSError as error:
        raise AcceptanceInputError(f"cannot read {path}: {error}") from error


def validate_evidence_inputs(repo_root: Path) -> None:
    for relative, expected in INPUT_HASHES.items():
        check_hash(repo_root / relative, expected)

    main_path = repo_root / "docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv"
    analysis_path = repo_root / "docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv"
    main = read_tsv(main_path)
    analysis = read_tsv(analysis_path)
    main_ids = [row["identity"] for row in main]
    analysis_ids = [row["identity"] for row in analysis]
    if len(main_ids) != 25 or len(set(main_ids)) != 25:
        raise AcceptanceInputError(f"{main_path}: expected exactly 25 unique roots")
    if len(analysis_ids) != 25 or len(set(analysis_ids)) != 25:
        raise AcceptanceInputError(f"{analysis_path}: expected exactly 25 unique roots")
    overlap = set(main_ids) & set(analysis_ids)
    if overlap:
        raise AcceptanceInputError(f"frozen cohorts overlap at {sorted(overlap)[0]}")

    for cohort, rows, expected_count in (
        ("main", main, 16),
        ("analysis", analysis, 15),
    ):
        protection_path = repo_root / (
            f"docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-{cohort}.tsv"
        )
        protections = read_tsv(protection_path)
        identities = [row["identity"] for row in protections]
        cohort_ids = {row["identity"] for row in rows}
        if len(identities) != expected_count or len(set(identities)) != expected_count:
            raise AcceptanceInputError(
                f"{protection_path}: expected {expected_count} unique typed roots"
            )
        if not set(identities) <= cohort_ids:
            raise AcceptanceInputError(f"{protection_path}: root outside its frozen cohort")
        if any(row["gate"] != "required_typed" for row in protections):
            raise AcceptanceInputError(f"{protection_path}: only required_typed roots are allowed")

    main_protections = {
        row["identity"]
        for row in read_tsv(
            repo_root
            / "docs/typer-lexical-closure-acceptance-recovery-3.9.0/required-main.tsv"
        )
    }
    previous_main = read_tsv(
        repo_root / "docs/typer-stdlib-acceptance-iii-3.9.0/original-main-required-typed.tsv"
    )
    if not {row["identity"] for row in previous_main} <= main_protections:
        raise AcceptanceInputError("the prior 14 accepted main roots are not all protected")
    for identity in (
        "library/src/scala/Function1.scala#tree=111",
        "library/src/scala/Function1.scala#tree=86",
    ):
        if identity not in main_protections:
            raise AcceptanceInputError(f"required independent Function1 root missing: {identity}")

    fixture_dir = repo_root / "crates/dotty-typer/tests/fixtures/lexical-closure-contract"
    positive = read_tsv(fixture_dir / "six-root-gate.tsv")
    positive_paths = {row["source_path"] for row in positive}
    if len(positive) != 6 or positive_paths != POSITIVE_ROOTS:
        raise AcceptanceInputError("six-root contract must retain its exact six source identities")
    if any(
        row["status"] != "accepted"
        or row["mapping_ok"] != "true"
        or not row["session_1"]
        or not row["session_2"]
        for row in positive
    ):
        raise AcceptanceInputError("six-root receipts must retain both accepted session outcomes")

    negative_path = repo_root / "docs/typer-lexical-closure-rollback-matrix-3.9.0.tsv"
    negative = read_tsv(negative_path)
    negative_cases = [row["case"] for row in negative]
    if len(negative_cases) != len(NEGATIVE_CASES) or set(negative_cases) != NEGATIVE_CASES:
        raise AcceptanceInputError("rollback matrix must contain the exact nine #1890 cases")
    required_columns = (
        "source_root",
        "scala_3_9_0",
        "rust_error",
        "rollback_projection",
        "retry_and_followup",
        "validation",
    )
    if any(not all(row[column] for column in required_columns) for row in negative):
        raise AcceptanceInputError("rollback matrix contains an incomplete outcome row")


def validate_test_log(text: str, group: str) -> None:
    expected = TESTS[group]
    passed = re.findall(r"^test (.+) \.\.\. ok$", text, flags=re.MULTILINE)
    summary = re.findall(
        r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; "
        r"\d+ measured; \d+ filtered out(?:; finished in [0-9.]+s)?$",
        text,
        flags=re.MULTILINE,
    )
    if passed != list(expected):
        raise AcceptanceInputError(
            f"{group} gate executed {passed!r}; expected exactly {list(expected)!r}"
        )
    if summary != [(str(len(expected)), "0", "0")]:
        raise AcceptanceInputError(
            f"{group} gate did not report {len(expected)} passed, zero failed and zero ignored"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    inputs = subparsers.add_parser("inputs", help="validate all frozen acceptance inputs")
    inputs.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    results = subparsers.add_parser("test-log", help="reject skipped or incomplete Rust gates")
    results.add_argument("--group", choices=tuple(TESTS), required=True)
    results.add_argument("log", type=Path)
    arguments = parser.parse_args()
    try:
        if arguments.command == "inputs":
            validate_evidence_inputs(arguments.repo_root)
            print("frozen lexical closure acceptance inputs verified")
        else:
            validate_test_log(arguments.log.read_text(encoding="utf-8"), arguments.group)
            print(f"{arguments.group} Rust gate executed its complete fixed matrix")
    except (OSError, AcceptanceInputError, KeyError, ValueError) as error:
        print(f"lexical closure acceptance verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
