#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 [--check] [--refresh-fixtures] <corpus-directory> <pinned-artifact.jar>" >&2
}

check_only=0
refresh_fixtures=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      check_only=1
      shift
      ;;
    --refresh-fixtures)
      refresh_fixtures=1
      shift
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
  esac
done

if [[ $# -ne 2 ]]; then
  usage
  exit 2
fi
if [[ "$check_only" == 1 && "$refresh_fixtures" == 1 ]]; then
  echo "--check and --refresh-fixtures cannot be used together" >&2
  exit 2
fi

corpus_dir=$(cd "$1" && pwd)
artifact_path=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
manifest_path="$corpus_dir/manifest.toml"

if [[ ! -f "$manifest_path" ]]; then
  echo "corpus manifest does not exist: $manifest_path" >&2
  exit 1
fi
if [[ ! -f "$artifact_path" ]]; then
  echo "artifact does not exist: $artifact_path" >&2
  exit 1
fi

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

selection_file=$(manifest_value selection_file)
expectation_file=$(manifest_value expectation_file)
wire_expectation_file=$(manifest_value wire_expectation_file)
expected_sha256=$(manifest_value artifact_sha256)
tasty_root=$(manifest_value tasty_root)
expected_fixture_count=$(manifest_value fixture_count)
expected_fixture_bytes=$(manifest_value fixture_bytes)
tasty_root=${tasty_root:-.}

if [[ -z "$selection_file" || -z "$expectation_file" || -z "$wire_expectation_file" \
  || -z "$expected_sha256" ]]; then
  echo "manifest must define selection_file, expectation_file, wire_expectation_file, and artifact_sha256" >&2
  exit 1
fi

selection_path="$corpus_dir/$selection_file"
semantic_output_path="$corpus_dir/$expectation_file"
wire_output_path="$corpus_dir/$wire_expectation_file"
if [[ ! -f "$selection_path" ]]; then
  echo "selection file does not exist: $selection_path" >&2
  exit 1
fi

actual_sha256=$(sha256sum "$artifact_path" | awk '{print $1}')
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "artifact checksum mismatch" >&2
  echo "  expected: $expected_sha256" >&2
  echo "  actual:   $actual_sha256" >&2
  exit 1
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
tool_dir="$repo_root/tools/tasty-baseline"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/tasty-baseline-all.XXXXXX")
staged_root="$work_dir/unpacked"
sbt_runtime_dir="$work_dir/sbt-runtime"
mkdir -p "$staged_root" "$sbt_runtime_dir"

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT

(
  cd "$staged_root"
  unzip -q "$artifact_path" '*.tasty'
)

fixture_count=$(find "$staged_root/$tasty_root" -type f -name '*.tasty' -printf . | wc -c)
fixture_bytes=$(find "$staged_root/$tasty_root" -type f -name '*.tasty' -printf '%s\n' \
  | awk '{ total += $1 } END { print total + 0 }')
if [[ -n "$expected_fixture_count" && "$fixture_count" != "$expected_fixture_count" ]]; then
  echo "fixture count mismatch: expected $expected_fixture_count, found $fixture_count" >&2
  exit 1
fi
if [[ -n "$expected_fixture_bytes" && "$fixture_bytes" != "$expected_fixture_bytes" ]]; then
  echo "fixture byte count mismatch: expected $expected_fixture_bytes, found $fixture_bytes" >&2
  exit 1
fi

if [[ "$refresh_fixtures" == 1 ]]; then
  fixture_output="$corpus_dir/$tasty_root"
  mkdir -p "$fixture_output"
  find "$fixture_output" -type f -name '*.tasty' -delete
  (
    cd "$staged_root/$tasty_root"
    find . -type f -name '*.tasty' -exec cp --parents '{}' "$fixture_output" \;
  )
fi

generate_semantic() {
  local output_path=$1
  local runtime_dir=$2
  local temporary_runtime="$runtime_dir/$(basename "$output_path").runtime"
  mkdir -p "$temporary_runtime"
  (
    cd "$tool_dir"
    XDG_RUNTIME_DIR="$temporary_runtime" sbt --server -batch \
      "run $output_path --select=$selection_path $artifact_path"
  )
  rmdir "$temporary_runtime" 2>/dev/null || true
}

semantic_output="$work_dir/semantic.json"
wire_output="$work_dir/wire.json"
generate_semantic "$semantic_output" "$sbt_runtime_dir"
cargo run --manifest-path "$repo_root/Cargo.toml" --locked --quiet --bin tasty-wire-baseline -- \
  "$wire_output" \
  "--select=$selection_path" \
  "$staged_root/$tasty_root"

if [[ "$check_only" == 1 ]]; then
  cmp -s "$semantic_output" "$semantic_output_path" || {
    echo "semantic baseline is not reproducible: $semantic_output_path" >&2
    exit 1
  }
  cmp -s "$wire_output" "$wire_output_path" || {
    echo "wire baseline is not reproducible: $wire_output_path" >&2
    exit 1
  }
  echo "baselines are reproducible for $corpus_dir"
  exit 0
fi

mkdir -p "$(dirname "$semantic_output_path")" "$(dirname "$wire_output_path")"
cp "$semantic_output" "$semantic_output_path"
cp "$wire_output" "$wire_output_path"
echo "wrote $semantic_output_path"
echo "wrote $wire_output_path"
