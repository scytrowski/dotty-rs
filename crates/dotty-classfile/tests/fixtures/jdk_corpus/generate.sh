#!/usr/bin/env bash
# Regenerates one real JDK class-file corpus via `jimage extract`.
#
# Usage:
#   JAVA_HOME=/path/to/jdk23 tests/fixtures/jdk_corpus/generate.sh jdk23
#   JAVA_HOME=/path/to/jdk24 tests/fixtures/jdk_corpus/generate.sh jdk24
#   JAVA_HOME=/path/to/jdk25 tests/fixtures/jdk_corpus/generate.sh jdk25
#   JAVA_HOME=/path/to/jdk26 tests/fixtures/jdk_corpus/generate.sh jdk26
#
# The selected JDK must match the version recorded by the target corpus
# manifest. Ordinary tests use the checked-in binaries and do not need Java.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: JAVA_HOME=/path/to/jdk tests/fixtures/jdk_corpus/generate.sh <jdk23|jdk24|jdk25|jdk26>" >&2
  exit 2
fi

corpus_root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
corpus_id=$1
corpus_dir="$corpus_root/$corpus_id"
manifest_path="$corpus_dir/manifest.toml"

manifest_value() {
  awk -F= -v wanted="$1" '
    /^[[:space:]]*#/ { next }
    {
      key = $1
      gsub(/[[:space:]]/, "", key)
      if (key != wanted) next
      value = substr($0, index($0, "=") + 1)
      sub(/^[[:space:]]*/, "", value)
      sub(/[[:space:]]*#.*/, "", value)
      if (value ~ /^".*"$/) value = substr(value, 2, length(value) - 2)
      print value
      exit
    }
  ' "$manifest_path"
}

if [[ ! -f "$manifest_path" ]]; then
  echo "corpus manifest does not exist: $manifest_path" >&2
  exit 1
fi

classes_file=$(manifest_value classes_file)
fixture_root=$(manifest_value fixture_root)
expected_jdk_major=$(manifest_value jdk_major)
expected_jdk_version=$(manifest_value jdk_version)
expected_jdk_build=$(manifest_value jdk_build)
expected_class_file_major=$(manifest_value class_file_major)
expected_count=$(manifest_value fixture_count)
expected_bytes=$(manifest_value fixture_bytes)

if [[ -z "$classes_file" || -z "$fixture_root" || -z "$expected_jdk_major" \
  || -z "$expected_jdk_version" || -z "$expected_jdk_build" \
  || -z "$expected_class_file_major" \
  || -z "$expected_count" || -z "$expected_bytes" ]]; then
  echo "manifest must define classes_file, fixture_root, jdk_major, jdk_version," \
    "jdk_build, class_file_major, fixture_count, and fixture_bytes" >&2
  exit 1
fi

classes_path="$corpus_dir/$classes_file"
fixture_dir="$corpus_dir/$fixture_root"
if [[ ! -f "$classes_path" ]]; then
  echo "classes file does not exist: $classes_path" >&2
  exit 1
fi

if [[ -z "${JAVA_HOME:-}" ]]; then
  java_bin=$(command -v java) || {
    echo "no java on PATH and JAVA_HOME unset" >&2
    exit 1
  }
  JAVA_HOME=$(cd "$(dirname "$java_bin")/.." && pwd)
fi

jimage_bin="$JAVA_HOME/bin/jimage"
java_bin="$JAVA_HOME/bin/java"
modules_image="$JAVA_HOME/lib/modules"
if [[ ! -x "$java_bin" ]]; then
  echo "java not found at $java_bin" >&2
  exit 1
fi
if [[ ! -x "$jimage_bin" ]]; then
  echo "jimage not found at $jimage_bin" >&2
  exit 1
fi
if [[ ! -f "$modules_image" ]]; then
  echo "runtime image not found at $modules_image" >&2
  exit 1
fi

java_settings=$("$java_bin" -XshowSettings:properties -version 2>&1)
actual_jdk_version=$(printf '%s\n' "$java_settings" | awk -F'= ' \
  '/^[[:space:]]*java.version = / { print $2; exit }')
actual_jdk_build=$(printf '%s\n' "$java_settings" | awk -F'= ' \
  '/^[[:space:]]*java.runtime.version = / { print $2; exit }')
actual_jdk_specification=$(printf '%s\n' "$java_settings" | awk -F'= ' \
  '/^[[:space:]]*java.specification.version = / { print $2; exit }')
actual_jdk_major=${actual_jdk_specification%%.*}
if [[ "$actual_jdk_major" == 1 ]]; then
  actual_jdk_major=${actual_jdk_specification#1.}
  actual_jdk_major=${actual_jdk_major%%.*}
fi

if [[ "$actual_jdk_major" != "$expected_jdk_major" ]]; then
  echo "wrong JDK major for $corpus_id: expected $expected_jdk_major, got $actual_jdk_major" >&2
  exit 1
fi
if [[ "$actual_jdk_version" != "$expected_jdk_version" ]]; then
  echo "wrong JDK version for $corpus_id: expected $expected_jdk_version, got $actual_jdk_version" >&2
  exit 1
fi
if [[ "$actual_jdk_build" != "$expected_jdk_build" ]]; then
  echo "wrong JDK build for $corpus_id: expected $expected_jdk_build, got $actual_jdk_build" >&2
  exit 1
fi

includes=""
while IFS= read -r class || [[ -n "$class" ]]; do
  class="${class%%#*}"
  class="$(echo "$class" | xargs)"
  [[ -z "$class" ]] && continue
  entry="glob:/java.base/${class}.class"
  includes="${includes:+$includes,}$entry"
done < "$classes_path"

if [[ -z "$includes" ]]; then
  echo "classes file has no non-comment entries: $classes_path" >&2
  exit 1
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/jdk-classfile-corpus.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
extract_root="$work_dir/java.base"

"$jimage_bin" extract --dir="$work_dir" --include "$includes" "$modules_image"

actual_count=$(find "$extract_root/$fixture_root" -type f -name '*.class' | wc -l | tr -d '[:space:]')
actual_bytes=$(find "$extract_root/$fixture_root" -type f -name '*.class' -exec cat {} + | wc -c | tr -d '[:space:]')
if [[ "$actual_count" != "$expected_count" || "$actual_bytes" != "$expected_bytes" ]]; then
  echo "corpus drift detected against $manifest_path:" >&2
  echo "  fixture_count: expected $expected_count, got $actual_count" >&2
  echo "  fixture_bytes: expected $expected_bytes, got $actual_bytes" >&2
  exit 1
fi

expected_paths="$work_dir/expected-paths"
actual_paths="$work_dir/actual-paths"
while IFS= read -r class || [[ -n "$class" ]]; do
  class="${class%%#*}"
  class="$(echo "$class" | xargs)"
  [[ -z "$class" ]] && continue
  class_path="$extract_root/$fixture_root/$class.class"
  if [[ ! -f "$class_path" ]]; then
    echo "requested class was not extracted: $class" >&2
    exit 1
  fi
  class_header=$(od -An -tx1 -j6 -N2 "$class_path" | tr -d '[:space:]')
  class_major=$((16#$class_header))
  if [[ "$class_major" != "$expected_class_file_major" ]]; then
    echo "wrong class-file major for $class: expected $expected_class_file_major, got $class_major" >&2
    exit 1
  fi
  echo "$class.class" >> "$expected_paths"
done < "$classes_path"
find "$extract_root/$fixture_root" -type f -name '*.class' -printf '%P\n' | sort > "$actual_paths"
sort -o "$expected_paths" "$expected_paths"
if ! diff -u "$expected_paths" "$actual_paths"; then
  echo "extracted class inventory differs from $classes_path" >&2
  exit 1
fi

# Replace only the selected corpus's materialized fixture directory after all
# validation has passed. Manifests and the shared classes file are preserved.
rm -rf "$fixture_dir/java"
mkdir -p "$fixture_dir"
cp -R "$extract_root/$fixture_root/." "$fixture_dir/"

echo "regenerated $corpus_id: $actual_count file(s), $actual_bytes byte(s), class-file major $expected_class_file_major"
"$java_bin" -version
