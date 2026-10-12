"""Attribute frozen #976 incomplete-prefix requests without re-running the corpus."""

import argparse
import collections
import csv
import json
from pathlib import Path
import re
import sys


STATE = re.compile(r"^    state=(\w+)::origin=([^:]+)::identity=(.*?)=(\d+)$")
METHOD = re.compile(r"^    (.+): requests=\d+, distinct_request_outcomes=")
REQUEST = re.compile(
    r"^      prefix=(.*?)::namespace=(.*?)::name=(.*?)::space=(.*?)"
    r"::outcome=class member scope unavailable: incomplete class info=(\d+)$"
)


def classify(text):
    states = {}
    expected = collections.Counter()
    for line in text.splitlines():
        if match := STATE.match(line):
            state, origin, prefix, count = match.groups()
            attribution = (state, origin)
            if prefix in states and states[prefix] != attribution:
                raise ValueError(f"ambiguous origin/state attribution for {prefix}")
            states[prefix] = attribution
            expected[prefix] += int(count)
    if not states:
        raise ValueError("report has no attributed incomplete-prefix states")

    rows = []
    observed = collections.Counter()
    method = None
    for line in text.splitlines():
        if match := METHOD.match(line):
            method = match.group(1)
        if match := REQUEST.match(line):
            prefix, namespace, name, space, count = match.groups()
            if method is None or prefix not in states:
                raise ValueError(f"unattributed frozen request for {prefix}")
            state, origin = states[prefix]
            if state == "Missing" and origin == "source":
                outcome = "remains_unresolved_unowned_source_prefix"
            elif state == "Missing" and origin in ("classloader", "TASTy"):
                # Artifact origin alone does not establish LoadingSession ownership.
                outcome = "requires_owned_session_replay"
            else:
                outcome = "outside_missing_external_shell_boundary"
            rows.append((method, prefix, namespace, name, space, int(count), state, origin, outcome))
            observed[prefix] += int(count)
    if observed != expected:
        raise ValueError("frozen request counts do not match the attributed state counts")
    return sorted(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, default=Path(__file__).resolve().parents[2] / "docs/typer-classpath-corpus-audit-3.9.0.md")
    args = parser.parse_args()
    rows = classify(args.report.read_text(encoding="utf-8"))
    writer = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    writer.writerow(("method", "prefix", "namespace", "member", "space", "requests", "state", "origin", "frozen_state_outcome"))
    writer.writerows(rows)
    summary = collections.Counter()
    for row in rows:
        summary[row[-1]] += row[5]
    print(json.dumps({"rows": len(rows), "requests": sum(row[5] for row in rows),
                      "prefixes": len({row[1] for row in rows}), "outcomes": dict(sorted(summary.items()))}, sort_keys=True), file=sys.stderr)


if __name__ == "__main__":
    main()
