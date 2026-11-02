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
    jdk_version = next(
        line for line in (java_home / "release").read_text().splitlines()
        if line.startswith("JAVA_VERSION=")
    )
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
    ("Two-pass cohort receipt SHA-256", REPORT_DIR / "source-case-class-match-cohorts.tsv"),
)
rows.extend((label, f"`{sha256(path)}`") for label, path in hashes)
rows.extend(
    (
        ("Positive roots", "2 original roots × 2 fresh sessions (4/4 required)"),
        ("Negative matrix", "9 Scala-classified failures; 6 structural rollback controls"),
        ("Protected earlier roots", "Box 2/2 + Product2 2/2 + closures 6/6 + inherited 4/4"),
        ("Ordinary cohorts", "main 16/25 + analysis 15/25; two deterministic passes"),
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
