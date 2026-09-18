#!/usr/bin/env bash
# Regenerates this directory's real fixtures: Ping.class and Pong.class,
# two ordinary top-level classes each declaring a field typed as the
# other - a legitimate mutual reference `javac` compiles without
# complaint (see docs/classloader.md §9, Milestone 6). Requires `javac`
# (JDK) on PATH; no network access.
#
# usage: tests/fixtures/ping_pong/generate.sh
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

javac -d "$fixture_dir" "$fixture_dir/Ping.java" "$fixture_dir/Pong.java"

echo "regenerated Ping.class and Pong.class"
javap -p "$fixture_dir/Ping.class"
javap -p "$fixture_dir/Pong.class"
