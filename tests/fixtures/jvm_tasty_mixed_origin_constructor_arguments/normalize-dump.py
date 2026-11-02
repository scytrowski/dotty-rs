#!/usr/bin/env python3
"""Normalize machine-specific headers while preserving the javap/TASTy body."""
import pathlib
import re
import sys

kind, artifact_path, logical_path = sys.argv[1:]
text = sys.stdin.read()

# javap and -print-tasty print the input file's absolute path as their first
# line. Replace only that exact input path so offsets, CP references, and the
# remaining evidence are byte-for-byte comparable across checkout locations.
text = text.replace(artifact_path, logical_path)
resolved_artifact_path = str(pathlib.Path(artifact_path).resolve())
if resolved_artifact_path != artifact_path:
    text = text.replace(resolved_artifact_path, logical_path)

if kind == "javap":
    text = re.sub(
        r"(?m)^  Last modified .*; size ([0-9]+) bytes$",
        r"  Last modified <normalized>; size \1 bytes",
        text,
    )
elif kind != "tasty":
    raise SystemExit(f"unknown dump kind: {kind}")

sys.stdout.write(text)
