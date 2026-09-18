#!/usr/bin/env bash
# Regenerates this directory's real GenericSample.class fixture.
# Requires `javac` (JDK) on PATH; no network access.
#
# usage: tests/fixtures/generic_sample/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

javac -d "$fixture_dir" "$fixture_dir/GenericSample.java"

echo "regenerated GenericSample.class"
javap -p -v "$fixture_dir/GenericSample.class" | grep -A1 "Signature:"
