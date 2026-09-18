#!/usr/bin/env bash
# Regenerates this directory's real multi-release JAR fixtures (JEP 238 /
# the JAR File Specification's "Multi-release JAR files" section) from
# the sources in base/, v11/, and v17/. Requires `javac`, `jar`, and
# `zip` (Info-Zip) on PATH; no network access.
#
# usage: tests/fixtures/multi_release_jar/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# Each variant declares the same class name (MrSample); v11 and v17
# each carry a distinct `@Deprecated(since = ...)` annotation, so tests
# can tell which variant was selected by comparing raw class bytes, or
# by decoding and checking the annotation - `jar --release` rejects
# public API differences (extra interfaces/methods/fields) between
# versioned entries, but an annotation isn't part of that check.
#
# `jar` validates that a multi-release JAR's class-file versions are
# non-decreasing from the base entry through each higher versions/N
# entry, so the base variant is compiled for release 9 (the lowest
# release multi-release JARs support) rather than this JDK's default
# target, keeping base(9) < v11(11) < v17(17).
javac --release 9 -d "$fixture_dir/base" "$fixture_dir/base/MrSample.java"
javac --release 11 -d "$fixture_dir/v11" "$fixture_dir/v11/MrSample.java"
javac --release 17 -d "$fixture_dir/v17" "$fixture_dir/v17/MrSample.java"

rm -f "$fixture_dir/mr_sample.jar" "$fixture_dir/incidental_versions_no_manifest_flag.jar"

# `jar --release N` also stamps `Multi-Release: true` into the manifest
# automatically - no hand-editing needed.
( cd "$fixture_dir" && jar --create --file mr_sample.jar \
    -C base MrSample.class \
    --release 11 -C v11 MrSample.class \
    --release 17 -C v17 MrSample.class )

echo "regenerated mr_sample.jar"
unzip -v "$fixture_dir/mr_sample.jar"

# A second JAR with an entry that merely happens to live at a
# META-INF/versions/11/ path, but carries no Multi-Release manifest
# attribute at all (built with plain `zip`, which adds no manifest of
# its own) - proving the manifest gate makes that entry inert.
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/mr-sample-incidental.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
cp "$fixture_dir/base/MrSample.class" "$work_dir/MrSample.class"
mkdir -p "$work_dir/META-INF/versions/11"
cp "$fixture_dir/v11/MrSample.class" "$work_dir/META-INF/versions/11/MrSample.class"

( cd "$work_dir" && zip -X -r "$fixture_dir/incidental_versions_no_manifest_flag.jar" \
    MrSample.class META-INF )

echo "regenerated incidental_versions_no_manifest_flag.jar"
unzip -v "$fixture_dir/incidental_versions_no_manifest_flag.jar"
