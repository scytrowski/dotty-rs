#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <corpus-directory>" >&2
}

if [[ $# -ne 1 ]]; then
  usage
  exit 2
fi

corpus_dir=$(cd "$1" && pwd)
manifest_path="$corpus_dir/manifest.toml"

if [[ ! -f "$manifest_path" ]]; then
  echo "corpus manifest does not exist: $manifest_path" >&2
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
wire_expectation_file=$(manifest_value wire_expectation_file)

if [[ -z "$selection_file" || -z "$wire_expectation_file" ]]; then
  echo "manifest must define selection_file and wire_expectation_file" >&2
  exit 1
fi

selection_path="$corpus_dir/$selection_file"
output_path="$corpus_dir/$wire_expectation_file"
if [[ ! -f "$selection_path" ]]; then
  echo "selection file does not exist: $selection_path" >&2
  exit 1
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
temporary_output=$(mktemp "${TMPDIR:-/tmp}/tasty-wire-baseline.XXXXXX.json")
cleanup() {
  rm -f "$temporary_output"
}
trap cleanup EXIT

cargo run --manifest-path "$repo_root/Cargo.toml" --locked --quiet --bin tasty-wire-baseline -- \
  "$temporary_output" \
  "--select=$selection_path" \
  "$corpus_dir"

mkdir -p "$(dirname "$output_path")"
mv "$temporary_output" "$output_path"
