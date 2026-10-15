#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
jdk_home=${JAVA_HOME:?set JAVA_HOME to the pinned Eclipse Temurin 21.0.8+9 JDK}
java_bin="$jdk_home/bin/java"
javac_bin="$jdk_home/bin/javac"
jmod_bin="$jdk_home/bin/jmod"

runtime_version=$("$java_bin" -XshowSettings:properties -version 2>&1 \
  | awk -F'= ' '/^[[:space:]]*java.runtime.version = / { print $2; exit }')
if [[ "$runtime_version" != "21.0.8+9-LTS" ]]; then
  echo "expected Eclipse Temurin 21.0.8+9-LTS, got ${runtime_version:-unknown}" >&2
  exit 1
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/dotty-jpms-fixtures.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
mkdir -p "$work_dir/compiled" "$fixture_dir/jdk21" "$fixture_dir/generated"

for module in java.base java.net.http jdk.jshell; do
  python3 "$fixture_dir/extract_module_info.py" \
    "$jdk_home/jmods/$module.jmod" "$fixture_dir/jdk21/$module-module-info.class"
done

"$javac_bin" -g:none --module-source-path "$fixture_dir/source" \
  -d "$work_dir/compiled" \
  --module fixture.directives,fixture.consumer.one,fixture.consumer.two,fixture.main,fixture.minimal

"$jmod_bin" create --class-path "$work_dir/compiled/fixture.directives" \
  --module-version 1.0 "$work_dir/fixture-directives.jmod"
"$jmod_bin" create --class-path "$work_dir/compiled/fixture.main" \
  --module-version 1.0 --main-class fixture.app.Main "$work_dir/fixture-main.jmod"

python3 "$fixture_dir/extract_module_info.py" \
  "$work_dir/fixture-directives.jmod" "$fixture_dir/generated/directives-module-info.class"
python3 "$fixture_dir/extract_module_info.py" \
  "$work_dir/fixture-main.jmod" "$fixture_dir/generated/main-class-module-info.class"
cp "$work_dir/compiled/fixture.minimal/module-info.class" \
  "$fixture_dir/generated/minimal-module-info.class"
python3 "$fixture_dir/zero_optional_versions.py" \
  "$fixture_dir/generated/minimal-module-info.class" \
  "$fixture_dir/generated/zero-optional-versions-module-info.class"

(
  cd "$fixture_dir"
  sha256sum \
    jdk21/java.base-module-info.class \
    jdk21/java.net.http-module-info.class \
    jdk21/jdk.jshell-module-info.class \
    generated/directives-module-info.class \
    generated/main-class-module-info.class \
    generated/minimal-module-info.class \
    generated/zero-optional-versions-module-info.class \
    > SHA256SUMS
)

echo "generated JPMS module-info fixtures with Eclipse Temurin $runtime_version"
