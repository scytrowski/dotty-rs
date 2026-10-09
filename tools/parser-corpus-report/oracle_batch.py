#!/usr/bin/env python3
"""Run a Scala parser oracle manifest in bounded, fully validated batches."""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile
from collections.abc import Callable, Sequence


def _read_manifest(path: pathlib.Path) -> list[str]:
    entries = path.read_text(encoding="utf-8").splitlines()
    for number, entry in enumerate(entries, start=1):
        fields = entry.split("\t")
        if len(fields) != 2 or not fields[0] or not fields[1]:
            raise ValueError(f"invalid manifest entry on line {number}: {entry!r}")
    return entries


def run_batches(
    manifest_path: pathlib.Path,
    counts_path: pathlib.Path,
    oracle_runner: str,
    batch_size: int = 512,
    run: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> tuple[int, int]:
    if batch_size < 1:
        raise ValueError("batch size must be positive")

    entries = _read_manifest(manifest_path)
    source_sets: list[list[str]] = []
    for line_number, line in enumerate(counts_path.read_text(encoding="utf-8").splitlines(), start=1):
        fields = line.split("\t")
        if len(fields) != 2:
            raise ValueError(f"invalid source-set count on line {line_number}: {line!r}")
        name, count_text = fields
        try:
            count = int(count_text)
        except ValueError as error:
            raise ValueError(f"invalid source-set file count on line {line_number}: {count_text!r}") from error
        if not name or count < 0:
            raise ValueError(f"invalid source-set count on line {line_number}: {line!r}")
        source_sets.append([name, str(count), "0"])

    expected_entries = sum(int(source_set[1]) for source_set in source_sets)
    if expected_entries != len(entries):
        raise ValueError(
            f"manifest/source-set count mismatch: {len(entries)} manifest entries, "
            f"source sets declare {expected_entries}"
        )

    failures = 0
    source_set_index = 0
    source_set_end = int(source_sets[0][1]) if source_sets else 0
    with tempfile.TemporaryDirectory(prefix="parser-oracle-batch-") as temporary_directory:
        temporary_root = pathlib.Path(temporary_directory)
        for start in range(0, len(entries), batch_size):
            batch_entries = entries[start : start + batch_size]
            batch_manifest = temporary_root / f"batch-{start // batch_size:04d}.manifest"
            batch_manifest.write_text("\n".join(batch_entries) + "\n", encoding="utf-8")
            completed = run(
                [oracle_runner, "--batch", str(batch_manifest)],
                check=False,
                capture_output=True,
                text=True,
            )
            if completed.returncode != 0:
                detail = completed.stderr.strip() or completed.stdout.strip()
                raise RuntimeError(
                    f"Scala oracle batch starting at entry {start} failed "
                    f"with exit code {completed.returncode}: {detail}"
                )

            # JSON strings may contain U+2028/U+2029. Python's splitlines()
            # treats those as separators even though the oracle protocol uses
            # only physical LF bytes between records.
            output_lines = completed.stdout.split("\n")
            if output_lines and output_lines[-1] == "":
                output_lines.pop()
            if len(output_lines) != len(batch_entries):
                extra_records = []
                for output_line in output_lines[len(batch_entries) :]:
                    try:
                        record = json.loads(output_line)
                    except json.JSONDecodeError:
                        extra_records.append(output_line[:160])
                    else:
                        extra_records.append(
                            f"{record.get('kind', '<unknown>')}: {record.get('path', '<no path>')}"
                        )
                raise ValueError(
                    f"Scala oracle batch starting at entry {start} returned "
                    f"{len(output_lines)} records for {len(batch_entries)} inputs; "
                    f"extra records: {extra_records}"
                )

            for offset, output_line in enumerate(output_lines):
                try:
                    record = json.loads(output_line)
                except json.JSONDecodeError as error:
                    raise ValueError(
                        f"invalid Scala oracle JSON for manifest entry {start + offset + 1}: "
                        f"{output_line[:160]!r}"
                    ) from error
                if not isinstance(record, dict) or not isinstance(record.get("kind"), str):
                    raise ValueError(
                        f"invalid Scala oracle record for manifest entry {start + offset + 1}"
                    )

                global_index = start + offset
                while global_index >= source_set_end and source_set_index + 1 < len(source_sets):
                    source_set_index += 1
                    source_set_end += int(source_sets[source_set_index][1])
                if global_index >= source_set_end:
                    raise ValueError(f"could not assign oracle record {global_index + 1} to a source set")
                if record["kind"] == "OracleFailure":
                    source_sets[source_set_index][2] = str(int(source_sets[source_set_index][2]) + 1)
                    failures += 1
            print(
                f"Scala oracle batch: {start + len(batch_entries)}/{len(entries)} files validated",
                file=sys.stderr,
                flush=True,
            )

    counts_path.write_text(
        "".join("\t".join(source_set) + "\n" for source_set in source_sets),
        encoding="utf-8",
    )
    return len(entries), failures


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=pathlib.Path)
    parser.add_argument("source_set_counts", type=pathlib.Path)
    parser.add_argument("oracle_runner")
    parser.add_argument("--batch-size", type=int, default=512)
    args = parser.parse_args()
    files, failures = run_batches(
        args.manifest,
        args.source_set_counts,
        args.oracle_runner,
        batch_size=args.batch_size,
    )
    print(f"Validated Scala oracle records: {files} files, {failures} oracle failures")


if __name__ == "__main__":
    main()
