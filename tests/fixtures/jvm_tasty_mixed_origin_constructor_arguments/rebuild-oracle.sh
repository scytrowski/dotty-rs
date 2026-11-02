#!/usr/bin/env bash
set -euo pipefail

: "${SCALA39_ROOT:?set SCALA39_ROOT to the pinned Scala 3.9 checkout}"
: "${JAVA_HOME:?set JAVA_HOME to Temurin JDK 21}"
fixture_dir="$(cd "$(dirname "$0")" && pwd)"
work_dir="${DOTTY_ISSUE_2011_ORACLE_DIR:-$PWD/target/issue-2011/oracle}"
export TMPDIR="${TMPDIR:-$PWD/target/issue-2011/tmp}"
oracle_output="$work_dir/frozen"
scalac="$SCALA39_ROOT/dist/target/universal/stage/bin/scalac"
scala_lib="$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala3-library_3/3.9.0-bin-SNAPSHOT/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/dist/target/universal/stage/maven2/org/scala-lang/scala-library/3.9.0-bin-SNAPSHOT/scala-library-3.9.0-bin-SNAPSHOT.jar"
scala_cp="$(find "$SCALA39_ROOT/dist/target/universal/stage/maven2" -type f -name '*.jar' -print | paste -sd:):$SCALA39_ROOT/library/target/scala-library-bootstrapped/scala-library-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/library/target/scala3-library-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/scala3-library_3-3.9.0-bin-SNAPSHOT.jar:$SCALA39_ROOT/tasty/target/tasty-core-bootstrapped/scala-3.9.0-bin-SNAPSHOT-nonbootstrapped/tasty-core_3-3.9.0-bin-SNAPSHOT.jar"
export PATH="$JAVA_HOME/bin:$PATH"
mkdir -p "$TMPDIR"
export JAVA_TOOL_OPTIONS="-Djava.io.tmpdir=$TMPDIR -XX:-UsePerfData"
python3 "$fixture_dir/test_oracle_tools.py"
"$scalac" -version 2>&1 | grep -F '3.9.0-bin-SNAPSHOT-git-777528f' >/dev/null
"$JAVA_HOME/bin/java" -version 2>&1 | grep -F '21.0.13' >/dev/null
rm -rf "$work_dir"
mkdir -p "$work_dir" "$oracle_output"

for order in wrap-and-create create-and-wrap; do
  producer="$work_dir/$order/producer"
  consumer="$work_dir/$order/consumer"
  mkdir -p "$producer" "$consumer"
  test -z "$(find "$producer" -mindepth 1 -print -quit)"
  "$scalac" -release 21 -d "$producer" "$fixture_dir/$order/PairFactory.scala"
  test "$(find "$producer" -type f | wc -l)" -eq 6
  test -z "$(find "$producer" -type f ! -name '*.class' ! -name '*.tasty' -print -quit)"
  for name in Counter Pair PairFactory; do
    test -f "$producer/example/$name.class"
    test -f "$producer/example/$name.tasty"
  done
  client=Consumer
  if [[ "$order" == create-and-wrap ]]; then client=ConsumerReversed; fi
  "$scalac" -release 21 -classpath "$producer:$scala_lib" -d "$consumer" "$fixture_dir/$client.scala"
  "$JAVA_HOME/bin/java" -Xverify:all -ea -classpath "$consumer:$producer:$scala_lib" "$client"

  for name in Counter Pair PairFactory; do
    mkdir -p "$oracle_output/oracle/$order"
    class_file="$producer/example/$name.class"
    tasty_file="$producer/example/$name.tasty"
    "$JAVA_HOME/bin/javap" -v -p -c -classpath "$producer" "example.$name" \
      | python3 "$fixture_dir/normalize-dump.py" javap "$class_file" "<producer>/example/$name.class" \
      > "$oracle_output/oracle/$order/$name.javap.txt"
    "$JAVA_HOME/bin/java" -cp "$scala_cp" dotty.tools.MainGenericCompiler \
      -color:never -print-tasty "$tasty_file" \
      | python3 "$fixture_dir/normalize-dump.py" tasty "$tasty_file" "<producer>/example/$name.tasty" \
      > "$oracle_output/oracle/$order/$name.tasty.txt"
  done
  mkdir -p "$oracle_output/scala39/$order/example"
  cp "$producer"/example/* "$oracle_output/scala39/$order/example/"
done

python3 "$fixture_dir/write-manifest.py" "$SCALA39_ROOT" "$JAVA_HOME" "$fixture_dir" "$oracle_output"
for order in wrap-and-create create-and-wrap; do
  for name in Counter Pair PairFactory; do
    for suffix in class tasty; do
      cmp "$oracle_output/scala39/$order/example/$name.$suffix" \
        "$fixture_dir/scala39/$order/example/$name.$suffix"
    done
    for suffix in javap tasty; do
      cmp "$oracle_output/oracle/$order/$name.$suffix.txt" \
        "$fixture_dir/oracle/$order/$name.$suffix.txt"
    done
  done
done
python3 "$fixture_dir/compare-manifests.py" "$oracle_output/oracle.json" "$fixture_dir/oracle.json"
