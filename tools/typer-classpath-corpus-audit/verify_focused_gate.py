#!/usr/bin/env python3
"""Verify the bounded typer audit output and write timing sidecars."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path
import re
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


class GateError(ValueError):
    """Raised when a focused audit artifact is incomplete or differs."""


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
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--timing-output", type=Path)
    parser.add_argument("--timing-log", action="append", default=[], type=Path)
    parser.add_argument("--timing-rss", action="append", default=[], type=Path)
    args = parser.parse_args()
    try:
        verify_baseline(args.report, args.baseline)
        if args.timing_output or args.timing_log or args.timing_rss:
            if args.timing_output is None:
                raise GateError("timing sidecars require --timing-output")
            write_timing_tsv(args.timing_output, args.timing_log, args.timing_rss)
    except GateError as error:
        print(f"focused audit gate: {error}", file=sys.stderr)
        return 1
    print(f"normalized focused output matches {args.baseline}")
    if args.timing_output:
        print(f"wrote informational per-run timings to {args.timing_output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
