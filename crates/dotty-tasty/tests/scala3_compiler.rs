use std::fs;
use std::path::Path;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/semantic.rs"]
mod semantic;

use dotty_tasty::tasty::{EncodedSection, StandardSection, TastyFile, TastyFileBuilder};

fn scala3_compiler_corpus() -> corpus::Corpus {
    corpus::Corpus::load(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scala3-compiler"),
    )
}

#[test]
fn scala3_compiler_fixture_inventory_is_complete() {
    let corpus = scala3_compiler_corpus();
    let fixtures = corpus.fixture_paths();
    let selected = corpus.selected_fixture_paths();
    assert!(!selected.is_empty());
    assert!(selected.iter().all(|path| path.is_file()));
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

#[test]
fn all_scala3_compiler_fixtures_decode_and_validate_as_compatible_tasty() {
    for path in scala3_compiler_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_and_validate_compatible_with(&bytes, 28, 9, 0)
            .unwrap_or_else(|error| panic!("failed to validate {}: {error}", path.display()));

        assert_eq!(
            file.header().major_version,
            28,
            "fixture {}",
            path.display()
        );
        assert_eq!(file.header().minor_version, 8, "fixture {}", path.display());
        assert_eq!(
            file.header().experimental_version,
            0,
            "fixture {}",
            path.display()
        );
        assert!(!file.names().is_empty(), "fixture {}", path.display());
        assert!(!file.sections().is_empty(), "fixture {}", path.display());
    }
}

#[test]
fn all_scala3_compiler_fixtures_round_trip_through_file_encoder() {
    for path in scala3_compiler_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_and_validate_compatible_with(&bytes, 28, 9, 0)
            .unwrap_or_else(|error| panic!("failed to validate {}: {error}", path.display()));
        let encoded = file
            .encode()
            .unwrap_or_else(|error| panic!("failed to encode {}: {error}", path.display()));
        let reparsed = TastyFile::parse_and_validate_compatible_with(&encoded, 28, 9, 0)
            .unwrap_or_else(|error| panic!("failed to reparse {}: {error}", path.display()));

        assert_eq!(
            reparsed.header(),
            file.header(),
            "fixture {}",
            path.display()
        );
        assert_eq!(reparsed.names(), file.names(), "fixture {}", path.display());
        let original_sections: Vec<_> = file
            .sections()
            .iter()
            .map(|section| (section.name, section.length, section.payload))
            .collect();
        let reparsed_sections: Vec<_> = reparsed
            .sections()
            .iter()
            .map(|section| (section.name, section.length, section.payload))
            .collect();
        assert_eq!(
            reparsed_sections,
            original_sections,
            "fixture {}",
            path.display()
        );
    }
}

#[test]
fn all_scala3_compiler_structured_files_validate_after_reencoding() {
    for path in scala3_compiler_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_and_validate_compatible_with(&bytes, 28, 9, 0)
            .unwrap_or_else(|error| panic!("failed to validate {}: {error}", path.display()));
        let structured = file.structured_asts().unwrap_or_else(|error| {
            panic!(
                "failed to decode structured ASTs in {}: {error}",
                path.display()
            )
        });
        let original_ast_section = file
            .section(StandardSection::Asts)
            .expect("Scala compiler fixture must have an ASTs section");
        let ast_section = EncodedSection::structured_asts(original_ast_section.name, &structured)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to encode structured ASTs in {}: {error}",
                    path.display()
                )
            });

        let mut builder = TastyFileBuilder::new(file.header().clone(), file.names().clone());
        for section in file.sections().iter() {
            if section.standard_kind(file.names()) == Some(StandardSection::Asts) {
                builder.push_section(ast_section.clone());
            } else {
                builder.push_section(EncodedSection::raw(section.name, section.payload));
            }
        }

        builder
            .validate_compatible_with(28, 9, 0)
            .unwrap_or_else(|error| {
                panic!(
                    "structured re-encoding produced invalid references in {}: {error}",
                    path.display()
                )
            });
        let encoded = builder.encode().unwrap_or_else(|error| {
            panic!(
                "failed to encode the structured file {}: {error}",
                path.display()
            )
        });
        TastyFile::parse_and_validate_compatible_with(&encoded, 28, 9, 0).unwrap_or_else(|error| {
            panic!(
                "structured file output is not compatible TASTy {}: {error}",
                path.display()
            )
        });
    }
}

#[test]
fn scala3_compiler_semantic_expectations_cover_selected_fixtures() {
    let corpus = scala3_compiler_corpus();

    semantic::assert_expectations_cover_selected_fixtures(&corpus, 3, "3.9.0", (28, 9, 0));
}
