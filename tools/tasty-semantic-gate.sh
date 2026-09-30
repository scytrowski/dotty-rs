#!/usr/bin/env bash
# Reproduce the semantic TASTy compatibility gate over the pinned corpora.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo test --locked -p dotty-tasty-unpickler --lib
cargo test --release --locked -p dotty-tasty-unpickler --test type_corpus -- --ignored --nocapture
cargo test --release --locked -p dotty-tasty-unpickler --test definition_tail_corpus -- --ignored --nocapture
cargo test --release --locked -p dotty-tasty-unpickler --features corpus --test symbol_annotation_corpus -- --ignored --nocapture
cargo test --release --locked -p dotty-tasty-unpickler --test companion_corpus -- --ignored --nocapture
cargo test --release --locked -p dotty-tasty-unpickler --test opaque_corpus -- --ignored --nocapture
