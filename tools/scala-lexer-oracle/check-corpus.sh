#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)

bash "$script_dir/compare.sh" --ignore-layout "$script_dir/src/main/scala"
bash "$script_dir/compare.sh" --ignore-layout "$repo_dir/tools/tasty-baseline/src/main/scala"
