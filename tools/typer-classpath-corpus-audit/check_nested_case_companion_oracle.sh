#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "$0")/../.." && pwd)"
scala_root="${SCALA39_ROOT:?set SCALA39_ROOT to the pinned Scala 3.9.0 checkout}"
java_home="${JAVA_HOME:?set JAVA_HOME to JDK 21}"
expected_revision=777528f19a58e794c9954a42f433373472ec57f8
actual_revision="$(git -C "$scala_root" rev-parse HEAD)"
if [[ "$actual_revision" != "$expected_revision" ]]; then
    printf 'SCALA39_ROOT revision is %s; expected %s\n' "$actual_revision" "$expected_revision" >&2
    exit 1
fi
if [[ ! -f "$java_home/release" || ! -d "$java_home/jmods" ]]; then
    printf 'JAVA_HOME must be a JDK with release metadata and jmods: %s\n' "$java_home" >&2
    exit 1
fi
jdk_version="$(sed -n 's/^JAVA_VERSION="\([^"]*\)".*/\1/p' "$java_home/release")"
if [[ "${jdk_version%%.*}" != 21 ]]; then
    printf 'JAVA_HOME must be JDK 21; found %s\n' "$jdk_version" >&2
    exit 1
fi

scalac="$scala_root/dist/target/universal/stage/bin/scalac"
fixture_dir="$repo_root/tools/typer-classpath-corpus-audit/fixtures/nested-case-companion"
output_dir="$repo_root/target/typer-nested-case-companion-oracle"
if [[ ! -x "$scalac" ]]; then
    printf 'pinned Scala compiler is not staged at %s\n' "$scalac" >&2
    exit 1
fi
compiler_version="$("$scalac" -version 2>&1)"
if [[ "$compiler_version" != *"3.9.0-bin-SNAPSHOT-git-777528f"* ]]; then
    printf 'unexpected compiler version: %s\n' "$compiler_version" >&2
    exit 1
fi

rm -rf "$output_dir"
mkdir -p "$output_dir/valid"
"$scalac" -color:never -d "$output_dir/valid" "$fixture_dir/valid-shapes.scala"

check_rejection() {
    local fixture="$1"
    local error_code="$2"
    local log="$output_dir/${fixture%.scala}.log"
    mkdir -p "$output_dir/${fixture%.scala}"
    if "$scalac" -color:never -d "$output_dir/${fixture%.scala}" "$fixture_dir/$fixture" >"$log" 2>&1; then
        printf 'Scala unexpectedly accepted %s\n' "$fixture" >&2
        exit 1
    fi
    if ! rg -q -- "\\[${error_code}\\]" "$log"; then
        cat "$log" >&2
        printf 'Scala rejected %s without expected %s\n' "$fixture" "$error_code" >&2
        exit 1
    fi
}

check_rejection invalid-by-name.scala E055
check_rejection invalid-contextual-only.scala E144
check_rejection invalid-contextual-leading-multiple.scala E144
check_rejection invalid-contextual-multiple.scala E144
check_rejection invalid-no-clause.scala E004
check_rejection invalid-name-collision.scala E083

printf 'Scala 3.9 oracle controls passed at %s with JDK %s\n' "$actual_revision" "$jdk_version"
