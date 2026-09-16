use std::fs;
use std::path::Path;

#[path = "support/corpus.rs"]
mod corpus;

fn scala3_compiler_corpus() -> corpus::Corpus {
    corpus::Corpus::load(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scala3-compiler"),
    )
}

#[test]
fn scala3_compiler_fixture_inventory_is_complete() {
    let corpus = scala3_compiler_corpus();
    let fixtures = corpus.fixture_paths();
    assert_eq!(corpus.selected_fixture_paths(), fixtures);
    let total_bytes: u64 = fixtures
        .iter()
        .map(|path| {
            fs::metadata(path)
                .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()))
                .len()
        })
        .sum();

    assert_eq!(corpus.manifest().id, "scala3-compiler-3.9.0");
    assert_eq!(corpus.manifest().source_kind, "jar");
    assert_eq!(corpus.manifest().scala_version, "3.9.0");
    assert_eq!(corpus.manifest().tasty_format, "28.8.0");
    assert_eq!(
        corpus.manifest().artifact.as_deref(),
        Some("scala3-compiler_3-3.9.0-bin-SNAPSHOT-nonbootstrapped.jar")
    );
    assert_eq!(
        corpus.manifest().artifact_sha256.as_deref(),
        Some("83b18f07cccac91f502f5083d2e59c7d162c6bc5b9737334d6423328ac170c58")
    );
    assert_eq!(fixtures.len(), corpus.manifest().fixture_count);
    assert_eq!(total_bytes, corpus.manifest().fixture_bytes);
}
