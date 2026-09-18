//! Runs the symbol-entering pass over every small Scala 3.9.0 fixture that
//! `dotty-tasty` ships (its manifest-backed library corpora are excluded).
//! This checks that pass 1 accepts real compiler output of many shapes —
//! case classes, givens, extensions, inline, opaque types, match types,
//! refinements, package objects — not only the dedicated `semantic/`
//! fixtures. It is a corpus-wide smoke test; the semantic assertions live in
//! `enter_symbols.rs` and `scopes_and_identity.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use dotty_core::store::SemanticStore;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

fn dotty_tasty_fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dotty-tasty/tests/fixtures");
    let mut directories = vec![root];
    let mut fixtures = Vec::new();

    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        {
            let path = entry.unwrap().path();
            if path.is_dir() {
                // Manifest-backed corpora are large and target another
                // compiler baseline; they have their own tests.
                if !path.join("manifest.toml").is_file() {
                    directories.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "tasty") {
                fixtures.push(path);
            }
        }
    }

    fixtures.sort();
    fixtures
}

#[test]
fn every_small_fixture_enters_without_error_and_declares_at_least_one_symbol() {
    let fixtures = dotty_tasty_fixtures();
    assert!(!fixtures.is_empty(), "no fixtures found");

    for path in fixtures {
        let bytes = fs::read(&path).unwrap();
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("{} does not parse: {error}", path.display()));
        let mut store = SemanticStore::new();
        let mut unpickler = TastyUnpickler::new(&file, &mut store);

        let index = unpickler
            .enter_symbols()
            .unwrap_or_else(|error| panic!("{} failed to enter: {error:?}", path.display()));

        assert!(
            index.symbol_count() > 0,
            "{} entered nothing",
            path.display()
        );
    }
}
