#!/usr/bin/env bash
# Regenerates this directory's real JAR fixtures, both containing the
# same entry: the existing PoolSample.class fixture from
# crates/dotty-classfile/tests/fixtures/pool_sample/. Requires `zip`
# (Info-Zip) on PATH; no network access.
#
# usage: tests/fixtures/pool_sample_jar/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
class_file="$fixture_dir/../../../../dotty-classfile/tests/fixtures/pool_sample/PoolSample.class"

if [[ ! -f "$class_file" ]]; then
  echo "PoolSample.class fixture not found at $class_file" >&2
  exit 1
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/pool-sample-jar.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
cp "$class_file" "$work_dir/PoolSample.class"

# -0: store, no compression. -X: no extra fields (extended timestamps
# etc.), so the fixture stays minimal and deterministic.
( cd "$work_dir" && zip -0 -X "$fixture_dir/pool_sample_stored.jar" PoolSample.class )

echo "regenerated pool_sample_stored.jar"
unzip -v "$fixture_dir/pool_sample_stored.jar"
