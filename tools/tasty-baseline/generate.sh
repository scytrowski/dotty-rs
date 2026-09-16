#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <corpus-directory> <pinned-artifact.jar>" >&2
}

if [[ $# -ne 2 ]]; then
  usage
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
expected_sha256=$(manifest_value artifact_sha256)

if [[ -z "$selection_file" || -z "$expectation_file" || -z "$expected_sha256" ]]; then
  echo "manifest must define selection_file, expectation_file, and artifact_sha256" >&2
  exit 1
fi

selection_path="$corpus_dir/$selection_file"
output_path="$corpus_dir/$expectation_file"
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

tool_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
temporary_output=$(mktemp "${TMPDIR:-/tmp}/tasty-baseline.XXXXXX.json")
cleanup() {
  rm -f "$temporary_output"
}
trap cleanup EXIT

(
  cd "$tool_dir"
  sbt -batch "run $temporary_output --select=$selection_path $artifact_path"
)

mkdir -p "$(dirname "$output_path")"
mv "$temporary_output" "$output_path"
trap - EXIT
echo "wrote $output_path"
