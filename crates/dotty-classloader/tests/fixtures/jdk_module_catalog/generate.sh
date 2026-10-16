#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
jdk_home=${JAVA_HOME:?set JAVA_HOME to a JDK with javac and jmod}
javac_bin="$jdk_home/bin/javac"
jmod_bin="$jdk_home/bin/jmod"

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/jdk-module-catalog.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
mkdir -p "$work_dir/compiled" "$work_dir/alpha" "$work_dir/beta"

"$javac_bin" -g:none --module-source-path "$fixture_dir/source" \
  -d "$work_dir/compiled" --module fixture.alpha,fixture.beta,fixture.minimal

"$jmod_bin" create --class-path "$work_dir/compiled/fixture.alpha" \
  --module-version 1.2 --main-class fixture.alpha.app.Main "$work_dir/fixture.alpha.jmod"
"$jmod_bin" create --class-path "$work_dir/compiled/fixture.beta" \
  "$work_dir/fixture.beta.jmod"

"$jmod_bin" extract --dir "$work_dir/alpha" "$work_dir/fixture.alpha.jmod"
"$jmod_bin" extract --dir "$work_dir/beta" "$work_dir/fixture.beta.jmod"
cp "$work_dir/alpha/classes/module-info.class" \
  "$fixture_dir/fixture.alpha-module-info.class"
cp "$work_dir/beta/classes/module-info.class" \
  "$fixture_dir/fixture.beta-module-info.class"
cp "$work_dir/compiled/fixture.minimal/module-info.class" \
  "$fixture_dir/fixture.minimal-module-info.class"

(
  cd "$fixture_dir"
  sha256sum fixture.alpha-module-info.class fixture.beta-module-info.class \
    fixture.minimal-module-info.class > SHA256SUMS
)

echo "generated module catalog fixtures with $($javac_bin -version 2>&1)"
