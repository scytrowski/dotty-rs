#!/usr/bin/env python3
"""Fail unless both strict shards and all mandatory follow-up gates succeeded."""

from __future__ import annotations

import argparse
import os


REQUIRED_RESULTS = (
    "shard-1-job",
    "shard-1-tests",
    "shard-2-job",
    "shard-2-tests",
    "constructor-alias",
    "loader-rejection",
    "dual-reference-reader",
    "java-verifier",
)


def failed_results(results: dict[str, str]) -> dict[str, str]:
    """Return every missing or non-success result required by the aggregate."""
    return {
        name: results.get(name, "missing") or "missing"
        for name in REQUIRED_RESULTS
        if results.get(name) != "success"
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", action="store_true", help="print a concise status report")
    args = parser.parse_args()
    results = {name: os.environ.get(f"STRICT_TASTY_{name.upper().replace('-', '_')}", "") for name in REQUIRED_RESULTS}
    failed = failed_results(results)
    if failed:
        for name, result in failed.items():
            print(f"{name}: {result}")
        return 1
    if args.report:
        print("both strict TASTy shards and all four mandatory follow-up gates succeeded")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
