#!/usr/bin/env bash
# Regenerates this directory's real JAR fixtures, both containing the
# same entry: the existing PoolSample.class fixture from
# crates/dotty-classfile/tests/fixtures/pool_sample/. Requires `zip`
# (Info-Zip) and `jar` (JDK) on PATH; no network access.
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

# Remove any existing fixtures first: both `zip` and `jar` update an
# existing archive in place rather than recreating it, which can leave
# stale entries or non-deterministic byte layout behind.
rm -f "$fixture_dir/pool_sample_stored.jar" "$fixture_dir/pool_sample_deflate.jar"

# -0: store, no compression. -X: no extra fields (extended timestamps
# etc.), so the fixture stays minimal and deterministic.
( cd "$work_dir" && zip -0 -X "$fixture_dir/pool_sample_stored.jar" PoolSample.class )

echo "regenerated pool_sample_stored.jar"
unzip -v "$fixture_dir/pool_sample_stored.jar"

# jar's default compression is DEFLATE. This also picks up a
# META-INF/MANIFEST.MF entry (and a META-INF/ directory entry) that
# `jar` always adds; tests only look at the PoolSample.class entry.
( cd "$work_dir" && jar cf "$fixture_dir/pool_sample_deflate.jar" PoolSample.class )

echo "regenerated pool_sample_deflate.jar"
unzip -v "$fixture_dir/pool_sample_deflate.jar"
