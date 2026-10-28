#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scala_root="${SCALA39_ROOT:?set SCALA39_ROOT to the pinned Scala checkout}"
java_home="${JAVA_HOME:?set JAVA_HOME to Temurin 21.0.8+9}"
output_root="${1:?pass an empty output directory outside the fixture}"
expected_revision=777528f19a58e794c9954a42f433373472ec57f8
actual_revision="$(git -C "$scala_root" rev-parse HEAD)"
test "$actual_revision" = "$expected_revision"
grep -q '^JAVA_VERSION="21.0.8"$' "$java_home/release"
grep -q '^IMPLEMENTOR_VERSION="Temurin-21.0.8+9"$' "$java_home/release"
compiler="$scala_root/dist/target/universal/stage/bin/scalac"
test -x "$compiler"
compiler_version="$("$compiler" -version 2>&1)"
[[ "$compiler_version" == *"3.9.0-bin-SNAPSHOT-git-777528f"* ]]
mkdir -p "$output_root"
test -z "$(find "$output_root" -mindepth 1 -print -quit)"
export JAVA_HOME="$java_home"
export PATH="$java_home/bin:$PATH"
export TMPDIR="$output_root/tmp"
mkdir -p "$TMPDIR"
mkdir "$output_root/oracle" "$output_root/client"
"$compiler" -release 21 -d "$output_root/oracle" \
  "$fixture_dir/NestedConstructorCalls.scala"
diff -ru "$fixture_dir/scala39" "$output_root/oracle"
"$compiler" -release 21 -classpath "$output_root/oracle" \
  -d "$output_root/client" "$fixture_dir/Consumer.scala"
"$java_home/bin/java" -Djava.io.tmpdir="$TMPDIR" -Xverify:all \
  -classpath "$output_root/client:$output_root/oracle:$scala_root/dist/target/universal/stage/lib/scala.jar" \
  client.Consumer
