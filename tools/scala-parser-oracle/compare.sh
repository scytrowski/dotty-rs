#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd "${script_dir}/../.." && pwd)
fixture_dir=${1:-"${script_dir}/fixtures"}

if [[ "$#" -gt 1 ]]; then
  echo "usage: compare.sh [fixture-directory]" >&2
  exit 2
fi

fixture_dir=$(cd "${fixture_dir}" && pwd)

mapfile -t fixtures < <(find "${fixture_dir}" -type f -name '*.scala' | sort)
if [[ "${#fixtures[@]}" -eq 0 ]]; then
  echo "no Scala fixtures found in ${fixture_dir}" >&2
  exit 2
fi

manifest=$(mktemp)
scala_output=$(mktemp)
rust_output=$(mktemp)
trap 'rm -f "${manifest}" "${scala_output}" "${rust_output}"' EXIT

for fixture in "${fixtures[@]}"; do
  mode=expr
  if [[ "$(basename "$(dirname "${fixture}")")" == "patterns" ]]; then
    mode=pattern
  elif [[ "$(basename "$(dirname "${fixture}")")" == "definitions" ]]; then
    mode=block
    if [[ "$(basename "${fixture}")" == *function-erased-*.scala ]]; then
      mode=block-erased
    fi
  elif [[ "$(basename "$(dirname "${fixture}")")" == "compilation" ]]; then
    mode=compilation
  elif [[ "$(basename "$(dirname "${fixture}")")" == "oracle-only" ]]; then
    mode=oracle-only
  fi
  printf '%s\t%s\n' "${mode}" "${fixture}" >>"${manifest}"
done

"${script_dir}/run" --batch "${manifest}" >"${scala_output}"
cargo build -q -p dotty-parser-smoke-dump --locked
"${repo_dir}/target/debug/dotty-parser-smoke-dump" --batch "${manifest}" >"${rust_output}"
python3 "${script_dir}/compare.py" --batch "${manifest}" "${scala_output}" "${rust_output}"
