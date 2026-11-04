#!/usr/bin/env python3
"""Summarize per-binary test timing from existing Cargo/libtest output."""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from pathlib import Path


RUNNING = re.compile(r"^\s*Running (?:`[^`]*` )?([^\s]+\.rs)(?: \([^)]*\))?\s*$")
FINISHED = re.compile(r"^\s*Finished .* in (.+)$")
DURATION = re.compile(r"(?:(\d+)m)?\s*(\d+(?:\.\d+)?)s\s*$")
RESULT = re.compile(r"^test result: (ok|FAILED)\. (.*) finished in (\d+(?:\.\d+)?)s\s*$")


@dataclass(frozen=True)
class Target:
    name: str
    seconds: float
    result: str
    tests: str


def seconds(value: str) -> float | None:
    match = DURATION.fullmatch(value.strip())
    if not match:
        return None
    return (int(match.group(1) or 0) * 60) + float(match.group(2))


def parse_output(output: str) -> tuple[list[Target], float | None]:
    targets: list[Target] = []
    current: str | None = None
    build_seconds: float | None = None
    for line in output.splitlines():
        finished = FINISHED.match(line)
        if finished and build_seconds is None:
            build_seconds = seconds(finished.group(1))
        running = RUNNING.match(line)
        if running:
            current = Path(running.group(1)).stem
        result = RESULT.match(line)
        if result and current is not None:
            targets.append(Target(current, float(result.group(3)), result.group(1), result.group(2)))
            current = None
    return targets, build_seconds


def render(output: str, expected: list[str], step_seconds: float, cargo_exit: int) -> str:
    targets, build_seconds = parse_output(output)
    by_name = {target.name: target for target in targets}
    accounted = sum(target.seconds for target in targets)
    rows = ["### Strict Scala/TASTy test timing", "", f"- Expected targets: {len(expected)}; measured targets: {len(by_name)}", f"- Cargo build phase: {build_seconds:.2f}s" if build_seconds is not None else "- Cargo build phase: unparsed (see live log)", f"- Accounted test execution: {accounted:.2f}s", f"- Outer step elapsed: {step_seconds:.2f}s", f"- Cargo exit code: {cargo_exit}", "", "| Target | Test execution | Result |", "| --- | ---: | --- |"]
    for name in expected:
        target = by_name.get(name)
        if target is None:
            rows.append(f"| `{name}` | missing | unparsed or no result |")
        else:
            rows.append(f"| `{name}` | {target.seconds:.2f}s | {target.result} ({target.tests}) |")
    unexpected = sorted(set(by_name) - set(expected))
    for name in unexpected:
        target = by_name[name]
        rows.append(f"| `{name}` | {target.seconds:.2f}s | unexpected target: {target.result} ({target.tests}) |")
    rows.extend(["", "#### Five slowest measured targets", ""])
    for target in sorted(targets, key=lambda item: (-item.seconds, item.name))[:5]:
        rows.append(f"- `{target.name}` — {target.seconds:.2f}s")
    if len(by_name) != len(expected) or set(by_name) != set(expected):
        rows.extend(["", "> **Warning:** measured binaries do not exactly match the current strict target list; missing and unexpected targets are shown above."])
    return "\n".join(rows) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--expected-targets", nargs="+", required=True)
    parser.add_argument("--step-seconds", type=float, required=True)
    parser.add_argument("--cargo-exit-code", type=int, required=True)
    args = parser.parse_args()
    print(render(args.log.read_text(encoding="utf-8", errors="replace"), args.expected_targets, args.step_seconds, args.cargo_exit_code), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
