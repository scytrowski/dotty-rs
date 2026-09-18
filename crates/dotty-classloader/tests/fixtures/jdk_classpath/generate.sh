#!/usr/bin/env bash
# Regenerates this directory's fixture "jmods/" directory: two small
# real JMODs, standing in for a `$JAVA_HOME/jmods` directory without
# committing the real ~88 MB one. `pool_sample.jmod` is a copy of the
# fixture built by ../pool_sample_jmod/generate.sh (kept identical so
# both fixtures test against the same real bytes); `other_sample.jmod`
# is built fresh from src/ here, so JdkClassPath tests can prove they
# search across more than one jmod file. Requires `javac` and `jmod`
# (JDK) on PATH; no network access.
#
# usage: tests/fixtures/jdk_classpath/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
src_dir="$fixture_dir/src"
pool_sample_jmod="$fixture_dir/../pool_sample_jmod/pool_sample.jmod"

if [[ ! -f "$pool_sample_jmod" ]]; then
  echo "pool_sample.jmod fixture not found at $pool_sample_jmod; run ../pool_sample_jmod/generate.sh first" >&2
  exit 1
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/jdk-classpath-jmod.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT

javac -d "$work_dir/classes" "$src_dir/module-info.java" "$src_dir/other/OtherSample.java"

rm -f "$fixture_dir/pool_sample.jmod" "$fixture_dir/other_sample.jmod"

cp "$pool_sample_jmod" "$fixture_dir/pool_sample.jmod"
jmod create --class-path "$work_dir/classes" --module-version 1.0 "$fixture_dir/other_sample.jmod"

echo "copied pool_sample.jmod"
jmod list "$fixture_dir/pool_sample.jmod"
echo "regenerated other_sample.jmod"
jmod list "$fixture_dir/other_sample.jmod"
