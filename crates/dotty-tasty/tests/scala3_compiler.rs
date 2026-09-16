use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/semantic.rs"]
mod semantic;

use dotty_tasty::tasty::{
    EncodedSection, RawNode, RawNodes, Reader, StandardSection, TastyFile, TastyFileBuilder, Writer,
};

fn scala3_compiler_corpus() -> corpus::Corpus {
    corpus::Corpus::load(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scala3-compiler"),
    )
}

/// Returns canonical structured bytes for a node, excluding its original
/// address and any wire-level integer representation details.
fn normalized_structured_encoding(raw: &RawNode<'_>) -> Result<Vec<u8>, String> {
    let structured = raw.decode_structured().map_err(|error| error.to_string())?;
    let mut writer = Writer::new();
    structured
        .encode(&mut writer)
        .map_err(|error| error.to_string())?;
    Ok(writer.into_inner())
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
fn all_scala3_compiler_indexed_structured_nodes_round_trip() {
    let mut node_count = 0;

    for path in scala3_compiler_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_and_validate_compatible_with(&bytes, 28, 9, 0)
            .unwrap_or_else(|error| panic!("failed to validate {}: {error}", path.display()));
        let index = file
            .ast_address_index()
            .unwrap_or_else(|error| panic!("failed to index {}: {error}", path.display()));

        for address in index.addresses() {
            node_count += 1;
            let raw = index
                .get(address)
                .expect("indexed address must resolve to a raw node");
            let encoded = normalized_structured_encoding(raw).unwrap_or_else(|error| {
                panic!(
                    "failed to structurally encode AST node at address {address} in {}: {error}",
                    path.display()
                )
            });

            let mut reader = Reader::new(&encoded);
            let reparsed_nodes = RawNodes::decode(&mut reader).unwrap_or_else(|error| {
                panic!(
                    "failed to structurally reparse AST node at address {address} in {}: {error}",
                    path.display()
                )
            });
            assert!(
                reader.is_at_end(),
                "structured encoding left trailing bytes for AST node at address {address} in {}",
                path.display()
            );
            assert_eq!(
                reparsed_nodes.len(),
                1,
                "address {address} in {}",
                path.display()
            );

            let reparsed_raw = reparsed_nodes
                .get(0)
                .expect("one encoded node must be present");
            let reparsed = normalized_structured_encoding(reparsed_raw).unwrap_or_else(|error| {
                panic!(
                    "failed to structurally reparse AST node at address {address} in {}: {error}",
                    path.display()
                )
            });
            assert_eq!(
                encoded,
                reparsed,
                "normalized structured AST changed at address {address} in {}",
                path.display()
            );
        }
    }

    assert!(node_count > 0);
}

#[test]
fn selected_scala3_compiler_corruptions_do_not_panic() {
    let mut state = 0x517c_c1b7_2722_0a95_u64;

    for path in scala3_compiler_corpus().selected_fixture_paths() {
        let original = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

        for case in 0..16 {
            let mut input = original.clone();
            let mutation_count = (next_random(&mut state) % 8 + 1) as usize;
            for _ in 0..mutation_count {
                let index = (next_random(&mut state) as usize) % input.len();
                input[index] ^= (next_random(&mut state) as u8).max(1);
            }

            let result = catch_unwind(AssertUnwindSafe(|| {
                let _ = TastyFile::parse(&input);
                let _ = TastyFile::parse_and_validate_compatible_with(&input, 28, 9, 0);
            }));
            assert!(
                result.is_ok(),
                "parser panicked for corrupted fixture {} case {case}",
                path.display()
            );
        }
    }
}

#[test]
fn selected_scala3_compiler_truncations_do_not_panic() {
    for path in scala3_compiler_corpus().selected_fixture_paths() {
        let original = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let prefix_sample_end = original.len().min(256);
        let suffix_sample_start = original.len().saturating_sub(256);

        for end in 0..prefix_sample_end {
            assert_truncated_compiler_fixture_does_not_panic(&path, &original[..end], end);
        }
        for end in suffix_sample_start..original.len() {
            assert_truncated_compiler_fixture_does_not_panic(&path, &original[..end], end);
        }
    }
}

fn assert_truncated_compiler_fixture_does_not_panic(path: &Path, input: &[u8], end: usize) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = TastyFile::parse(input);
        let _ = TastyFile::parse_scala_3_9(input);
        let _ = TastyFile::parse_and_validate_compatible_with(input, 28, 9, 0);
    }));
    assert!(
        result.is_ok(),
        "parser panicked for truncated fixture {} at {end} bytes",
        path.display()
    );
}

fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    *state
}

#[test]
fn scala3_compiler_semantic_expectations_cover_selected_fixtures() {
    let corpus = scala3_compiler_corpus();

    semantic::assert_expectations_cover_selected_fixtures(&corpus, 3, "3.9.0", (28, 9, 0));
}
