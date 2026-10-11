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
cp "$build_dir/enclosing_generics/Outer.class" "$fixture_dir/Outer.class"
cp "$build_dir/enclosing_generics/Outer\$Inner.class" "$fixture_dir/Outer\$Inner.class"

javap -classpath "$build_dir" -p -v 'enclosing_generics.Outer$Inner' \
    | grep -A2 -E 'Signature:|InnerClasses:|EnclosingMethod:'
