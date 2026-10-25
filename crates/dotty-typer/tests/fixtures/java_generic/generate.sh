#!/usr/bin/env bash
# Regenerates the generic Java method fixture with the JDK 21 class-file target.
# Requires javac on PATH; no network access.
set -euo pipefail

fixture_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
javac --release 21 -d "$fixture_dir" "$fixture_dir/Opcodes.java"
