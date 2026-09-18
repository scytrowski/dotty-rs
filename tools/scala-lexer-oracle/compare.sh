#!/usr/bin/env bash
set -euo pipefail

ignore_layout=false
if [[ ${1:-} == "--ignore-layout" ]]; then
  ignore_layout=true
  shift
fi

if [[ $# -gt 1 ]]; then
  echo "usage: compare.sh [--ignore-layout] [fixture-directory]" >&2
  exit 2
fi

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
fixture_dir=${1:-"$script_dir/fixtures"}
fixture_dir=$(cd "$fixture_dir" && pwd)

mapfile -t fixtures < <(find "$fixture_dir" -type f -name '*.scala' | sort)
if [[ ${#fixtures[@]} -eq 0 ]]; then
  echo "no Scala fixtures found in $fixture_dir" >&2
  exit 2
fi

for fixture in "${fixtures[@]}"; do
  oracle_output=$(mktemp)
  rust_output=$(mktemp)
  trap 'rm -f "$oracle_output" "$rust_output"' EXIT

  if ! (
    cd "$script_dir"
    set +u
    source "${HOME}/.sdkman/bin/sdkman-init.sh"
    set -u
    sbt --error "run $fixture"
  ) >"$oracle_output" 2>&1; then
    cat "$oracle_output" >&2
    exit 1
  fi

  if ! (
    cd "$repo_dir"
    cargo run -q -p dotty-lexer --example dump -- "$fixture"
  ) >"$rust_output"; then
    cat "$rust_output" >&2
    exit 1
  fi

  if "$ignore_layout"; then
    python3 "$script_dir/compare.py" --ignore-layout "$fixture" "$oracle_output" "$rust_output"
  else
    python3 "$script_dir/compare.py" "$fixture" "$oracle_output" "$rust_output"
  fi
  rm -f "$oracle_output" "$rust_output"
  trap - EXIT
done
