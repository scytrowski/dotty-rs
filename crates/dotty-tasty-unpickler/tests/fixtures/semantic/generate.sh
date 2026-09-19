#!/usr/bin/env bash
# Regenerates this directory's real .tasty fixtures (one per top-level
# definition in the .scala sources) with the Scala 3.9.0
# compiler, run straight from a Coursier cache (no sbt, no network access).
# Requires `java` on PATH and the Scala 3.9.0 compiler dependencies already
# present in the cache; set COURSIER_CACHE to use a non-default cache.
#
# usage: tests/fixtures/semantic/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cache=${COURSIER_CACHE:-$HOME/.cache/coursier}/v1/https/repo1.maven.org/maven2
scala_version=3.9.0

jars=(
  "org/scala-lang/scala3-compiler_3/$scala_version/scala3-compiler_3-$scala_version.jar"
  "org/scala-lang/scala3-interfaces/$scala_version/scala3-interfaces-$scala_version.jar"
  "org/scala-lang/tasty-core_3/$scala_version/tasty-core_3-$scala_version.jar"
  "org/scala-lang/scala3-library_3/$scala_version/scala3-library_3-$scala_version.jar"
  "org/scala-lang/scala-library/$scala_version/scala-library-$scala_version.jar"
  "org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar"
  "org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar"
)

compiler_class_path=""
library_class_path=""
for jar in "${jars[@]}"; do
  [[ -f "$cache/$jar" ]] || { echo "missing $cache/$jar" >&2; exit 1; }
  compiler_class_path+="${compiler_class_path:+:}$cache/$jar"
  case "$jar" in
    *scala3-library_3* | *scala-library-*) library_class_path+="${library_class_path:+:}$cache/$jar" ;;
  esac
done

work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

# Compile from the fixture directory so each SourceFile attribute is the
# stable relative path (`Foo.scala`, ...).
sources=$(cd "$fixture_dir" && ls -- *.scala)
# shellcheck disable=SC2086
(cd "$fixture_dir" && java -cp "$compiler_class_path" dotty.tools.dotc.Main \
  -usejavacp:false -classpath "$library_class_path" -d "$work_dir" $sources)

for tasty in "$work_dir"/me/cytrowski/tastyfixtures/semantic/*.tasty; do
  cp "$tasty" "$fixture_dir/"
  echo "regenerated $(basename "$tasty")"
done
