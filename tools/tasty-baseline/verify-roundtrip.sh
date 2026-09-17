#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 [--classpath=path/to/dependencies.jar] <corpus-directory>" >&2
}

classpath_path=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --classpath=*)
      classpath_path=${1#--classpath=}
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

tasty_root=$(manifest_value tasty_root)
selection_file=$(manifest_value selection_file)
expectation_file=$(manifest_value expectation_file)
tasty_root=${tasty_root:-.}

if [[ -z "$selection_file" || -z "$expectation_file" ]]; then
  echo "manifest must define selection_file and expectation_file" >&2
  exit 1
fi

input_root="$corpus_dir/$tasty_root"
selection_path="$corpus_dir/$selection_file"
expectation_path="$corpus_dir/$expectation_file"
if [[ ! -d "$input_root" ]]; then
  echo "TASTy root does not exist: $input_root" >&2
  exit 1
fi
if [[ ! -f "$selection_path" ]]; then
  echo "selection file does not exist: $selection_path" >&2
  exit 1
fi
if [[ ! -f "$expectation_path" ]]; then
  echo "semantic expectation does not exist: $expectation_path" >&2
  exit 1
fi
if [[ -n "$classpath_path" && ! -f "$classpath_path" && ! -d "$classpath_path" ]]; then
  echo "classpath path does not exist: $classpath_path" >&2
  exit 1
fi
if [[ -n "$classpath_path" ]]; then
  if [[ -d "$classpath_path" ]]; then
    classpath_path=$(cd "$classpath_path" && pwd)
  else
    classpath_path=$(cd "$(dirname "$classpath_path")" && pwd)/$(basename "$classpath_path")
  fi
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
tool_dir="$repo_root/tools/tasty-baseline"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/tasty-roundtrip.XXXXXX")
roundtrip_root="$work_dir/tasty"
semantic_output="$work_dir/semantic.json"
sbt_runtime_dir="$work_dir/sbt-runtime"
sbt_boot_dir="$work_dir/sbt-boot"
sbt_global_dir="$work_dir/sbt-global"
sbt_ivy_dir="$work_dir/sbt-ivy"
sbt_coursier_dir="$work_dir/sbt-coursier"
sbt_options="-Dsbt.boot.directory=$sbt_boot_dir -Dsbt.global.base=$sbt_global_dir -Dsbt.ivy.home=$sbt_ivy_dir -Dsbt.coursier.home=$sbt_coursier_dir"
mkdir -p "$roundtrip_root" "$sbt_runtime_dir" "$sbt_boot_dir" "$sbt_global_dir" \
  "$sbt_ivy_dir" "$sbt_coursier_dir"

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT

cargo run --manifest-path "$repo_root/Cargo.toml" --locked --quiet --bin tasty-roundtrip -- \
  --mode=structured \
  "$input_root" \
  "$roundtrip_root"

run_inspector() {
  local arguments=$1
  local runtime_name=$2
  local temporary_runtime="$sbt_runtime_dir/$runtime_name"
  mkdir -p "$temporary_runtime"
  (
    cd "$tool_dir"
    SBT_OPTS="${SBT_OPTS:-} $sbt_options" \
      COURSIER_HOME="$sbt_coursier_dir" \
      COURSIER_CACHE="$sbt_coursier_dir/cache" \
      XDG_RUNTIME_DIR="$temporary_runtime" sbt --server -batch "$arguments"
  )
}

full_output="$work_dir/full-semantic.json"
full_arguments="run $full_output $roundtrip_root"
if [[ -n "$classpath_path" ]]; then
  full_arguments+=" --classpath=$classpath_path"
fi
run_inspector "$full_arguments" full

selected_arguments="run $semantic_output --select=$selection_path $roundtrip_root"
if [[ -n "$classpath_path" ]]; then
  selected_arguments+=" --classpath=$classpath_path"
fi
run_inspector "$selected_arguments" selected

if ! cmp -s "$semantic_output" "$expectation_path"; then
  echo "semantic baseline mismatch after structured re-encoding: $corpus_dir" >&2
  diff -u "$expectation_path" "$semantic_output" | head -200 >&2 || true
  exit 1
fi

echo "Scala TastyInspector accepted all structured round-trip files and the selected semantic baseline for $corpus_dir"
