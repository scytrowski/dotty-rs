#!/usr/bin/env bash
# Regenerates this directory's real JMOD fixture from the module
# sources in src/. Requires `javac` and `jmod` (JDK) on PATH; no
# network access.
#
# usage: tests/fixtures/pool_sample_jmod/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
src_dir="$fixture_dir/src"

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/pool-sample-jmod.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT

# `jmod create` refuses class files in the unnamed package, so this
# fixture's class lives in a named package (`pool`), unlike the
# dotty-classfile PoolSample.class fixture it otherwise mirrors.
javac -d "$work_dir/classes" "$src_dir/module-info.java" "$src_dir/pool/PoolSample.java"

# Remove any existing fixture first: `jmod create` refuses to
# overwrite an existing file.
rm -f "$fixture_dir/pool_sample.jmod"

jmod create --class-path "$work_dir/classes" --module-version 1.0 "$fixture_dir/pool_sample.jmod"

echo "regenerated pool_sample.jmod"
jmod list "$fixture_dir/pool_sample.jmod"
