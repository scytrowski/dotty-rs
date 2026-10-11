#!/usr/bin/env bash
# Regenerates the real nested generic classfile fixtures.
# Requires JDK 21 or later (`javac` and `javap`) on PATH.
#
# usage: tests/fixtures/enclosing_generics/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
build_dir=$(mktemp -d)
trap 'rm -rf "$build_dir"' EXIT

javac --release 21 -g:none -d "$build_dir" "$fixture_dir/Outer.java"
for class_file in "$build_dir/enclosing_generics/"*.class; do
    cp "$class_file" "$fixture_dir/$(basename "$class_file")"
done

javap -classpath "$build_dir" -p -v 'enclosing_generics.Outer$Inner' \
    | grep -A2 -E 'Signature:|InnerClasses:|EnclosingMethod:'
