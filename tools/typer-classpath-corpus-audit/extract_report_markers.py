#!/usr/bin/env python3
"""Extract a non-empty report between markers in captured command output."""

from pathlib import Path
import sys


class ReportMarkerError(ValueError):
    """The output does not contain a complete, non-empty marked report."""


def extract_report(text: str, start_marker: str, end_marker: str) -> str:
    start = text.find(start_marker)
    if start < 0:
        raise ReportMarkerError(f"missing start marker: {start_marker}")
    start_line_end = text.find("\n", start + len(start_marker))
    if start_line_end < 0:
        raise ReportMarkerError("start marker has no following report line")

    end = text.find(end_marker, start_line_end + 1)
    if end < 0:
        raise ReportMarkerError(f"missing end marker: {end_marker}")
    end_line_start = text.rfind("\n", start_line_end + 1, end)
    if end_line_start < 0:
        end_line_start = start_line_end
    report = text[start_line_end + 1 : end_line_start + 1]
    if not report.strip():
        raise ReportMarkerError("marked report is empty")
    return report


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: extract_report_markers.py LOG START_MARKER END_MARKER", file=sys.stderr)
        return 2
    log_path, start_marker, end_marker = sys.argv[1:]
    try:
        report = extract_report(Path(log_path).read_text(encoding="utf-8"), start_marker, end_marker)
    except (OSError, UnicodeError, ReportMarkerError) as error:
        print(error, file=sys.stderr)
        return 1
    sys.stdout.write(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
