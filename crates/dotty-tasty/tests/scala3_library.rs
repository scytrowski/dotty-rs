use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/semantic.rs"]
mod semantic;

use dotty_tasty::tasty::{
    EncodedSection, PACKAGE_TAG, RawNode, RawNodes, Reader, StandardSection, TastyFile,
    TastyFileBuilder, Writer,
};

fn scala3_library_corpus() -> corpus::Corpus {
    corpus::Corpus::load(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scala3-library"),
    )
}

/// Returns canonical structured bytes used as a structural fingerprint.
///
/// Decoding removes wire-level Nat representation details and node offsets;
/// structured encoding then writes the same tree in canonical form.
fn normalized_structured_encoding(raw: &RawNode<'_>) -> Result<Vec<u8>, String> {
    let structured = raw.decode_structured().map_err(|error| error.to_string())?;
    let mut writer = Writer::new();
    structured
        .encode(&mut writer)
        .map_err(|error| error.to_string())?;
    Ok(writer.into_inner())
}

#[test]
fn scala3_library_fixture_inventory_is_complete() {
    let corpus = scala3_library_corpus();
    let fixtures = corpus.fixture_paths();
    assert_eq!(corpus.manifest().id, "scala3-library-3.9.0");
    assert_eq!(corpus.manifest().source_kind, "jar");
    assert_eq!(corpus.manifest().scala_version, "3.9.0");
    assert_eq!(corpus.manifest().tasty_format, "28.9.0");
    assert_eq!(
        corpus.manifest().artifact.as_deref(),
        Some("scala-library-3.9.0-bin-SNAPSHOT.jar")
    );
    assert_eq!(
        corpus.manifest().artifact_sha256.as_deref(),
        Some("8f4881ef32c90de03487555cf13a893e7f94e9f70361d000d5b8848842beecc7")
    );
    let total_bytes: u64 = fixtures
        .iter()
        .map(|path| {
            fs::metadata(path)
                .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()))
                .len()
        })
        .sum();

    assert_eq!(fixtures.len(), corpus.manifest().fixture_count);
    assert_eq!(total_bytes, corpus.manifest().fixture_bytes);
}

#[test]
fn scala3_library_semantic_selection_is_resolved_from_manifest() {
    let corpus = scala3_library_corpus();
    let selected = corpus.selected_fixture_paths();

    assert!(!selected.is_empty());
    assert!(selected.iter().all(|path| path.is_file()));
}

#[test]
fn scala3_library_semantic_expectations_cover_selected_fixtures() {
    let corpus = scala3_library_corpus();
    semantic::assert_expectations_cover_selected_fixtures(&corpus, 3, "3.9.0", (28, 9, 0));
}

#[test]
fn all_scala3_library_fixtures_decode_through_file_model() {
    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        assert!(!file.names().is_empty(), "fixture {}", path.display());
        assert!(!file.sections().is_empty(), "fixture {}", path.display());
        let asts = file
            .asts()
            .unwrap_or_else(|error| panic!("failed to decode ASTs in {}: {error}", path.display()));
        assert!(!asts.is_empty(), "fixture {}", path.display());
    }
}

#[test]
fn all_scala3_library_fixtures_round_trip_structurally() {
    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let encoded = file
            .encode()
            .unwrap_or_else(|error| panic!("failed to encode {}: {error}", path.display()));

        let reparsed = TastyFile::parse_scala_3_9(&encoded).unwrap_or_else(|error| {
            panic!("failed to reparse encoded {}: {error}", path.display())
        });
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
fn scala3_library_fixtures_have_package_ast_roots() {
    let mut tags = BTreeSet::new();

    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let nodes = file
            .asts()
            .unwrap_or_else(|error| panic!("failed to decode ASTs in {}: {error}", path.display()));

        tags.extend(nodes.iter().map(|node| node.tag));
    }

    assert_eq!(tags, BTreeSet::from([PACKAGE_TAG]));
}

#[test]
fn all_scala3_library_comment_sections_decode_with_ast_addresses() {
    let mut files_with_comments = 0;
    let mut comment_count = 0;

    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let Some(comments) = file.comments().unwrap_or_else(|error| {
            panic!("failed to decode comments in {}: {error}", path.display())
        }) else {
            continue;
        };
        files_with_comments += 1;
        comment_count += comments.len();

        let asts_length = file
            .section(StandardSection::Asts)
            .expect("comment-bearing fixture has an ASTs section")
            .payload
            .len();
        for comment in comments {
            assert!(
                (comment.address as usize) < asts_length,
                "comment address {} is outside ASTs payload in {}",
                comment.address,
                path.display()
            );
        }
    }

    assert!(files_with_comments > 0);
    assert!(comment_count > 0);
}

#[test]
fn all_scala3_library_ast_indexes_build_with_qualified_modifiers() {
    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let index = file
            .ast_address_index()
            .unwrap_or_else(|error| panic!("failed to index ASTs in {}: {error}", path.display()));
        assert!(
            !index.is_empty(),
            "fixture {} has no indexed AST nodes",
            path.display()
        );
    }
}

#[test]
fn all_scala3_library_ast_references_resolve() {
    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        file.validate_ast_reference_targets()
            .unwrap_or_else(|error| panic!("invalid AST reference in {}: {error}", path.display()));
    }
}

#[test]
fn all_scala3_library_indexed_structured_nodes_round_trip() {
    let mut node_count = 0;
    let mut byte_exact_count = 0;
    let mut normalized_count = 0;

    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
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
                    "failed to structurally decode AST node at address {address} in {}: {error}",
                    path.display()
                )
            });

            let mut reparsed_reader = Reader::new(encoded.as_slice());
            let reparsed_nodes = RawNodes::decode(&mut reparsed_reader).unwrap_or_else(|error| {
                panic!(
                    "failed to reparse encoded AST node at address {address} in {}: {error}",
                    path.display()
                )
            });
            assert!(
                reparsed_reader.is_at_end(),
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
            let reparsed_normalized = normalized_structured_encoding(reparsed_raw).unwrap_or_else(|error| {
                panic!(
                    "failed to structurally reparse AST node at address {address} in {}: {error}",
                    path.display()
                )
            });

            assert_eq!(
                encoded.as_slice(),
                reparsed_normalized.as_slice(),
                "normalized structured AST changed at address {address} in {}",
                path.display()
            );
            let mut original = Writer::new();
            raw.encode(&mut original).unwrap_or_else(|error| {
                panic!(
                    "failed to encode raw AST node at address {address} in {}: {error}",
                    path.display()
                )
            });
            if original.as_slice() == encoded.as_slice() {
                byte_exact_count += 1;
            } else {
                normalized_count += 1;
            }
        }
    }

    assert!(node_count > 0);
    assert_eq!(node_count, byte_exact_count + normalized_count);
}

#[test]
fn all_scala3_library_structured_files_validate_after_reencoding() {
    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let structured = file.structured_asts().unwrap_or_else(|error| {
            panic!(
                "failed to decode structured ASTs in {}: {error}",
                path.display()
            )
        });
        let original_ast_section = file
            .section(StandardSection::Asts)
            .expect("Scala library fixture must have an ASTs section");
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

        builder.validate_scala_3_9().unwrap_or_else(|error| {
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
        TastyFile::parse_and_validate_scala_3_9(&encoded).unwrap_or_else(|error| {
            panic!(
                "structured file output is not a valid Scala 3.9.0 TASTy file {}: {error}",
                path.display()
            )
        });
    }
}

#[test]
fn all_scala3_library_structured_relocated_files_validate() {
    for path in scala3_library_corpus().fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let structured = file.structured_asts().unwrap_or_else(|error| {
            panic!(
                "failed to decode structured ASTs in {}: {error}",
                path.display()
            )
        });
        let encoded = file
            .encode_structured_relocated(&structured)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to re-encode relocated file {}: {error}",
                    path.display()
                )
            });
        TastyFile::parse_and_validate_scala_3_9(&encoded).unwrap_or_else(|error| {
            panic!(
                "relocated file is not a valid Scala 3.9.0 TASTy file {}: {error}",
                path.display()
            )
        });
    }
}
