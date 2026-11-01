#!/usr/bin/env bash
set -euo pipefail

: "${SCALA39_ROOT:?set SCALA39_ROOT to the pinned Scala 3.9 source checkout}"
: "${JAVA_HOME:?set JAVA_HOME to Temurin 21}"
fixture_dir="$(cd "$(dirname "$0")" && pwd)"
work_dir="${DOTTY_ISSUE_1960_ORACLE_DIR:-${TMPDIR:-/var/tmp}/dotty-rs-issue-1960-oracle-rebuild}"
producer="$work_dir/producer"
consumer="$work_dir/consumer"
scalac="$SCALA39_ROOT/dist/target/universal/stage/bin/scalac"
export PATH="$JAVA_HOME/bin:$PATH"
export JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=${TMPDIR:-/var/tmp} -XX:-UsePerfData"
scala_lib="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"

rm -rf "$work_dir"
mkdir -p "$producer" "$consumer"
"$scalac" -release 21 -d "$producer" "$fixture_dir/PairFactory.scala"
"$scalac" -release 21 -classpath "$producer:$scala_lib" -d "$consumer" "$fixture_dir/Consumer.scala"
"$JAVA_HOME/bin/java" -Xverify:all -ea -classpath "$consumer:$producer:$scala_lib" example.PairFactoryConsumer
python3 "$fixture_dir/verify-controls.py" "$fixture_dir/PairFactory.scala" "$scalac" "$work_dir/controls"

for name in Counter Pair PairFactory; do
  cmp "$producer/example/$name.class" "$fixture_dir/scala39/example/$name.class"
  cmp "$producer/example/$name.tasty" "$fixture_dir/scala39/example/$name.tasty"
done
