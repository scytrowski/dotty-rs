#!/usr/bin/env python3
"""Write the run-bound Markdown summary for the focused typer audit."""

from __future__ import annotations

import hashlib
import os
import subprocess
from pathlib import Path


ROOT = Path.cwd()
REPORT_DIR = Path(os.environ["DOTTY_AUDIT_REPORT_DIR"])


def command(*args: str, cwd: Path = ROOT) -> str:
    try:
        return subprocess.check_output(
            args, cwd=cwd, stderr=subprocess.DEVNULL, text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "missing"


def sha256(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError:
        return "missing"


status = os.environ.get("DOTTY_AUDIT_JOB_STATUS")
disposition = "PASS" if status == "success" else "SPRINT GOAL UNMET"
base_sha = "missing"
try:
    for line in (REPORT_DIR / "environment.tsv").read_text().splitlines():
        key, value = line.split("\t", maxsplit=1)
        if key == "pull_request_base_sha":
            base_sha = value
            break
except (OSError, ValueError):
    pass

scala_root = Path(os.environ.get("SCALA39_ROOT", ""))
java_home = Path(os.environ.get("JAVA_HOME", ""))
try:
    release_fields = (java_home / "release").read_text().splitlines()
    java_version = next(line for line in release_fields if line.startswith("JAVA_VERSION="))
    implementor = next(line for line in release_fields if line.startswith("IMPLEMENTOR_VERSION="))
    jdk_version = f"Eclipse Temurin {java_version.removeprefix('JAVA_VERSION=')} ({implementor})"
except (OSError, StopIteration):
    jdk_version = "missing"

rows = [
    ("Tested PR head", f"`{command('git', 'rev-parse', 'HEAD')}`"),
    ("Integrated PR base", f"`{base_sha}`"),
    (
        "Workflow run",
        f"{os.environ.get('GITHUB_SERVER_URL', '')}/{os.environ.get('GITHUB_REPOSITORY', '')}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}",
    ),
    ("Pull request", os.environ.get("DOTTY_PR_URL", "")),
    (
        "Scala source revision",
        f"`{command('git', '-C', str(scala_root), 'rev-parse', 'HEAD')}`",
    ),
    ("Rust toolchain", f"`{command('rustc', '+1.98.1', '--version')}`"),
    ("JDK", f"`{jdk_version}`"),
]
hashes = (
    ("Event.scala SHA-256", Path("crates/dotty-typer/tests/fixtures/source-sealed-adt-match-3.9.0/Event.scala")),
    ("Positive receipt SHA-256", Path("docs/typer-sealed-adt-e2e-positive-3.9.0.tsv")),
    ("Negative/rollback receipt SHA-256", Path("docs/typer-sealed-adt-e2e-negative-rollback-3.9.0.tsv")),
    ("Cohort contract SHA-256", Path("docs/typer-sealed-adt-e2e-cohorts-3.9.0.tsv")),
    ("Committed per-root cohort receipt SHA-256", Path("docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv")),
    ("Two-pass cohort receipt SHA-256", REPORT_DIR / "source-case-class-match-cohorts.tsv"),
    ("Recursive Expr.scala SHA-256", Path("crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala")),
    ("Recursive artifact manifest SHA-256", Path("crates/dotty-typer/tests/fixtures/source-recursive-adt-match-3.9.0/scala-artifacts.sha256.tsv")),
    ("Recursive positive receipt SHA-256", Path("docs/typer-recursive-adt-e2e-positive-3.9.0.tsv")),
    ("Recursive negative/rollback receipt SHA-256", Path("docs/typer-recursive-adt-e2e-negative-rollback-3.9.0.tsv")),
    ("Recursive protected-root cohort receipt SHA-256", Path("docs/typer-sealed-adt-e2e-cohort-receipt-3.9.0.tsv")),
)
rows.extend((label, f"`{sha256(path)}`") for label, path in hashes)
rows.extend(
    (
        ("Positive roots", "2 original roots × 2 fresh sessions (4/4 required)"),
        ("Box extractor negative matrix", "9 Scala-classified failures; 6 structural rollback controls"),
        ("Protected earlier roots", "Box 2/2 + Product2 2/2 + closures 6/6 + inherited 4/4"),
        ("Ordinary cohorts", "main 16/25 + analysis 15/25; two deterministic passes"),
        ("Recursive Expr positive roots", "unwrap and eval as full source-owned DefDefs in two independent sessions (4 exact rows)"),
        ("Recursive Expr negative and rollback", "15 Scala-classified controls; late binder/recursive-Apply rollback, repeated failure, same-session retry"),
        ("Prior source-root preservation", "Box 2/2 + Product2 2/2 + Event 2/2 + closures 6/6 + inherited members 4/4"),
        ("Prerequisites", "#2034 through #2039 merge ancestry is required by a fail-closed workflow step"),
        ("Full Scala corpus", "not run on hosted CI"),
    )
)

artifact_manifest = REPORT_DIR / "scala-artifacts.sha256.tsv"
artifact_text = (
    artifact_manifest.read_text()
    if artifact_manifest.is_file()
    else "UNMET: pinned artifact manifest was not produced\n"
)

report = [
    "# Sealed Event acceptance run",
    "",
    f"**Disposition:** {disposition}",
    "",
    "A failed job is UNMET; its first failing workflow step and full evidence logs identify the blocker.",
    "",
    "| Evidence | Value |",
    "| --- | --- |",
    *(f"| {label} | {value} |" for label, value in rows),
    "",
    "## Eight pinned Scala artifacts",
    "",
    "```text",
    artifact_text.rstrip("\n"),
    "```",
    "",
    "The four normalized positive receipts are in `source-case-class-match-rust.log`;",
    "the negative, rollback, and two-pass cohort TSVs/logs are adjacent artifacts.",
    "",
]
REPORT_DIR.mkdir(parents=True, exist_ok=True)
(REPORT_DIR / "sealed-adt-e2e-report.md").write_text("\n".join(report))

binary_required = (
    REPORT_DIR / "binary-recursive-adt-positive.tsv",
    REPORT_DIR / "binary-recursive-adt-negative-rollback.tsv",
    REPORT_DIR / "binary-recursive-adt-protected-cohorts.tsv",
)
environment = {}
try:
    for line in (REPORT_DIR / "environment.tsv").read_text(encoding="utf-8").splitlines():
        key, value = line.split("\t", maxsplit=1)
        environment[key] = value
except (OSError, ValueError):
    pass

binary_report = [
    "# Binary recursive ADT E2E acceptance (Scala 3.9.0)",
    "",
    "**Disposition:** SPRINT GOAL UNMET — independent review of this PR head is still required.",
    "",
    f"**Focused audit job:** {'PASS' if status == 'success' else 'SPRINT GOAL UNMET'}",
    "",
    "The issue is PASS only after all mandatory hosted checks and an independent review succeed. This report never marks the sprint PASS while review is pending.",
    "",
    "| Evidence | Value |",
    "| --- | --- |",
    f"| PR head | `{environment.get('issue2080_pr_head_sha', 'missing')}` |",
    f"| Tested source | `{environment.get('issue2080_tested_sha', command('git', 'rev-parse', 'HEAD'))}` |",
    f"| Integrated baseline | `{environment.get('issue2080_baseline_sha', 'missing')}` |",
    f"| Workflow run | {os.environ.get('GITHUB_SERVER_URL', '')}/{os.environ.get('GITHUB_REPOSITORY', '')}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')} |",
    f"| Pull request / independent review target | {os.environ.get('DOTTY_PR_URL', '')} |",
    f"| Frozen source SHA-256 | `{sha256(Path('crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/Expr.scala'))}` |",
    f"| Scala 3.9.0 source revision | `{command('git', '-C', str(scala_root), 'rev-parse', 'HEAD')}` |",
    f"| Rust toolchain | `{environment.get('rust', command('rustc', '+1.98.1', '--version'))}` |",
    f"| Scala artifact manifest SHA-256 | `{sha256(Path('crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/scala-artifacts.sha256.tsv'))}` |",
    f"| JDK | `{jdk_version}` |",
    "| Positive numerator | `Evaluator.selectLeft` and `Evaluator.eval`, exactly 2 roots × 2 fresh sessions |",
    "| Prior source roots | `18/18` required: Box 2 + Flagged 2 + Event 2 + Succ 2 + closures 6 + inherited members 4 |",
    "| Protected ordinary cohorts | `16/25` main + `15/25` analysis; two deterministic passes, full Scala corpus disabled |",
    "",
    "## Run-bound TSV sidecars",
    "",
]
for path in binary_required:
    binary_report.append(f"- `{path.name}` — SHA-256 `{sha256(path)}`")
binary_report.extend([
    "",
    "## Eight pinned Scala artifacts",
    "",
    "```text",
    Path("crates/dotty-typer/tests/fixtures/source-binary-recursive-adt-3.9.0/scala-artifacts.sha256.tsv").read_text(encoding="utf-8").rstrip("\n"),
    "```",
    "",
    "The sidecars are generated only after the verifier confirms all four positive rows, the named Scala-classified controls and late rollback/retry tests, exact protected roots, and matching baseline/tested/PR-head SHAs. Missing or malformed evidence fails the focused gate.",
    "",
])
(REPORT_DIR / "binary-recursive-adt-e2e-report.md").write_text("\n".join(binary_report))
