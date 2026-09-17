#!/usr/bin/env bash
# Regenerates this directory's real JDK .class fixtures via `jimage
# extract`, from the classes listed in classes.txt. Requires a JDK 25
# installation on PATH (or $JAVA_HOME set); no network access.
#
# usage: tests/fixtures/jdk_corpus/generate.sh
set -euo pipefail

corpus_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
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

classes_file=$(manifest_value classes_file)
expected_count=$(manifest_value fixture_count)
expected_bytes=$(manifest_value fixture_bytes)

if [[ -z "$classes_file" || -z "$expected_count" || -z "$expected_bytes" ]]; then
  echo "manifest must define classes_file, fixture_count, and fixture_bytes" >&2
  exit 1
fi

classes_path="$corpus_dir/$classes_file"
if [[ ! -f "$classes_path" ]]; then
  echo "classes file does not exist: $classes_path" >&2
  exit 1
fi

if [[ -z "${JAVA_HOME:-}" ]]; then
  java_bin=$(command -v java) || { echo "no java on PATH and JAVA_HOME unset" >&2; exit 1; }
  JAVA_HOME=$(cd "$(dirname "$java_bin")/.." && pwd)
fi
jimage_bin="$JAVA_HOME/bin/jimage"
modules_image="$JAVA_HOME/lib/modules"
if [[ ! -x "$jimage_bin" ]]; then
  echo "jimage not found at $jimage_bin" >&2
  exit 1
fi
if [[ ! -f "$modules_image" ]]; then
  echo "runtime image not found at $modules_image" >&2
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
  echo "classes.txt has no non-comment entries" >&2
  exit 1
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/jdk-classfile-corpus.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT

"$jimage_bin" extract --dir="$work_dir" --include "$includes" "$modules_image"

# Remove existing extracted classes (leave manifest.toml/classes.txt/this
# script alone) before copying the freshly extracted ones over.
while IFS= read -r class || [[ -n "$class" ]]; do
  class="${class%%#*}"
  class="$(echo "$class" | xargs)"
  [[ -z "$class" ]] && continue
  rm -f "$corpus_dir/${class}.class"
done < "$classes_path"

cp -R "$work_dir/java.base/." "$corpus_dir/"

actual_count=$(find "$corpus_dir" -name '*.class' | wc -l | tr -d '[:space:]')
actual_bytes=$(find "$corpus_dir" -name '*.class' -exec cat {} + | wc -c | tr -d '[:space:]')

if [[ "$actual_count" != "$expected_count" || "$actual_bytes" != "$expected_bytes" ]]; then
  echo "corpus drift detected against manifest.toml:" >&2
  echo "  fixture_count: expected $expected_count, got $actual_count" >&2
  echo "  fixture_bytes: expected $expected_bytes, got $actual_bytes" >&2
  echo "if this is an intentional JDK upgrade, update manifest.toml" \
       "(fixture_count, fixture_bytes, jdk_vendor, jdk_version, jdk_build)" >&2
  exit 1
fi

echo "regenerated $actual_count file(s), $actual_bytes byte(s) — matches manifest.toml"
"$JAVA_HOME/bin/java" -version
