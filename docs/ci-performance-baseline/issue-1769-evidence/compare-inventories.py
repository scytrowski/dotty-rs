#!/usr/bin/env python3
"""Compare Cargo --list output with cargo-nextest list --message-format json."""
import json
from pathlib import Path
import re
import sys

cargo_log = Path(sys.argv[1])
nextest_json = Path(sys.argv[2])
output = Path(sys.argv[3]) if len(sys.argv) > 3 else Path(".")
output.mkdir(parents=True, exist_ok=True)
cargo_cases = set()
binary = None
run = re.compile(r"^\s*Running .+ \(([^)]+)\)$")
for line in cargo_log.read_text().splitlines():
    match = run.match(line)
    if match:
        binary = Path(match.group(1)).name
        continue
    name_kind = re.match(r"^(.*): (test|benchmark)$", line)
    if name_kind:
        if binary is None:
            raise SystemExit("test entry before Cargo's Running target header")
        cargo_cases.add((binary, name_kind.group(1), name_kind.group(2)))

payload = json.loads(nextest_json.read_text())
nextest_cases = set()
ignored = set()
for suite in payload["rust-suites"].values():
    binary = Path(suite["binary-path"]).name
    for name, case in suite["testcases"].items():
        key = (binary, name, case["kind"])
        nextest_cases.add(key)
        if case.get("ignored"):
            ignored.add(key)

missing = sorted(cargo_cases - nextest_cases)
additional = sorted(nextest_cases - cargo_cases)
for filename, cases, include_ignored in [
    ("cargo-inventory.tsv", cargo_cases, None),
    ("nextest-inventory.tsv", nextest_cases, None),
    ("ignored-inventory.tsv", ignored, None),
    ("cargo-only.tsv", missing, None),
    ("nextest-only.tsv", additional, None),
]:
    (output / filename).write_text("binary\ttest_name\tkind\n" + "".join("\t".join(row) + "\n" for row in sorted(cases)))
(output / "inventory-summary.txt").write_text(
    f"Cargo identities: {len(cargo_cases)}\n"
    f"Nextest identities: {len(nextest_cases)}\n"
    f"Ignored identities: {len(ignored)}\n"
    f"Cargo-only identities: {len(missing)}\n"
    f"Nextest-only identities: {len(additional)}\n"
)
print((output / "inventory-summary.txt").read_text(), end="")
if missing or additional:
    raise SystemExit(1)
