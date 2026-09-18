#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd "${script_dir}/../.." && pwd)
fixture_dir=${1:-"${script_dir}/fixtures"}

if [[ "$#" -gt 1 ]]; then
  echo "usage: compare.sh [fixture-directory]" >&2
  exit 2
fi

mapfile -t fixtures < <(find "${fixture_dir}" -maxdepth 1 -type f -name '*.scala' | sort)
if [[ "${#fixtures[@]}" -eq 0 ]]; then
  echo "no Scala fixtures found in ${fixture_dir}" >&2
  exit 2
fi

for fixture in "${fixtures[@]}"; do
  scala_output=$(mktemp)
  rust_output=$(mktemp)
  trap 'rm -f "${scala_output}" "${rust_output}"' EXIT

  "${script_dir}/run" "${fixture}" >"${scala_output}"
  cargo run -q -p dotty-parser-smoke-dump --locked -- "${fixture}" >"${rust_output}"
  python3 "${script_dir}/compare.py" "${scala_output}" "${rust_output}"

  rm -f "${scala_output}" "${rust_output}"
  trap - EXIT
done
