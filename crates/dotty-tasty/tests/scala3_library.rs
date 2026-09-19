use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/semantic.rs"]
mod semantic;
#[path = "support/wire.rs"]
mod wire;

use dotty_tasty::tasty::{
    EncodedSection, PACKAGE_TAG, RawNode, RawNodes, Reader, StandardSection, TastyFile,
    TastyFileBuilder, Writer,
};

fn scala3_library_corpus() -> corpus::Corpus {
    corpus::Corpus::load(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scala3-library"),
    )
}

fn scala3_library_fixtures() -> &'static [corpus::ParsedFixture] {
    corpus::parsed_fixtures(&scala3_library_corpus(), 28, 9, 0)
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

fn manifest_corpora() -> Vec<corpus::Corpus> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    corpus::Corpus::discover(&root)
}

#[test]
fn all_manifest_corpora_have_consistent_fixture_inventories() {
    let corpora = manifest_corpora();
    assert!(!corpora.is_empty());

    for corpus in corpora {
        let fixtures = corpus.fixture_paths();
        let total_bytes: u64 = fixtures
            .iter()
            .map(|path| {
                fs::metadata(path)
                    .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()))
                    .len()
            })
            .sum();
        let selected = corpus.selected_fixture_paths();

        assert_eq!(
            fixtures.len(),
            corpus.manifest().fixture_count,
            "{}",
            corpus.manifest().id
        );
        assert_eq!(
            total_bytes,
            corpus.manifest().fixture_bytes,
            "{}",
            corpus.manifest().id
        );
        assert!(
            !selected.is_empty(),
            "{} has no selected fixtures",
            corpus.manifest().id
        );
        assert!(
            selected.iter().all(|path| path.is_file()),
            "{} has a missing selected fixture",
            corpus.manifest().id
        );
    }
}

#[test]
fn all_manifest_corpora_match_their_semantic_and_wire_baselines() {
    for corpus in manifest_corpora() {
        semantic::assert_expectations_cover_selected_fixtures(
            &corpus,
            3,
            &corpus.manifest().scala_version,
            (28, 9, 0),
        );
        wire::assert_expectations_match_selected_fixtures(&corpus, 1, (28, 9, 0));
    }
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
fn all_scala3_library_fixtures_decode_through_file_model() {
    for fixture in scala3_library_fixtures() {
        let file = &fixture.file;

        assert!(
            !file.names().is_empty(),
            "fixture {}",
            fixture.path.display()
        );
        assert!(
            !file.sections().is_empty(),
            "fixture {}",
            fixture.path.display()
        );
        let asts = fixture.asts();
        assert!(!asts.is_empty(), "fixture {}", fixture.path.display());
    }
}

#[test]
fn all_scala3_library_fixtures_round_trip_structurally() {
    for fixture in scala3_library_fixtures() {
        let file = &fixture.file;
        let encoded = file
            .encode()
            .unwrap_or_else(|error| panic!("failed to encode {}: {error}", fixture.path.display()));

        let reparsed = TastyFile::parse_scala_3_9(&encoded).unwrap_or_else(|error| {
            panic!(
                "failed to reparse encoded {}: {error}",
                fixture.path.display()
            )
        });
        assert_eq!(
            reparsed.header(),
            file.header(),
            "fixture {}",
            fixture.path.display()
        );
        assert_eq!(
            reparsed.names(),
            file.names(),
            "fixture {}",
            fixture.path.display()
        );
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
            fixture.path.display()
        );
    }
}

#[test]
fn scala3_library_fixtures_have_package_ast_roots() {
    let mut tags = BTreeSet::new();

    for fixture in scala3_library_fixtures() {
        let nodes = fixture.asts();

        tags.extend(nodes.iter().map(|node| node.tag));
    }

    assert_eq!(tags, BTreeSet::from([PACKAGE_TAG]));
}

#[test]
fn all_scala3_library_comment_sections_decode_with_ast_addresses() {
    let mut files_with_comments = 0;
    let mut comment_count = 0;

    for fixture in scala3_library_fixtures() {
        let file = &fixture.file;
        let Some(comments) = file.comments().unwrap_or_else(|error| {
            panic!(
                "failed to decode comments in {}: {error}",
                fixture.path.display()
            )
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
                fixture.path.display()
            );
        }
    }

    assert!(files_with_comments > 0);
    assert!(comment_count > 0);
}

#[test]
fn all_scala3_library_ast_indexes_build_with_qualified_modifiers() {
    for fixture in scala3_library_fixtures() {
        let index = &fixture.index;
        assert!(
            !index.is_empty(),
            "fixture {} has no indexed AST nodes",
            fixture.path.display()
        );
    }
}

#[test]
fn all_scala3_library_ast_references_resolve() {
    for fixture in scala3_library_fixtures() {
        let file = &fixture.file;

        file.validate_ast_reference_targets()
            .unwrap_or_else(|error| {
                panic!(
                    "invalid AST reference in {}: {error}",
                    fixture.path.display()
                )
            });
    }
}

#[test]
fn all_scala3_library_indexed_structured_nodes_round_trip() {
    let mut node_count = 0;
    let mut byte_exact_count = 0;
    let mut normalized_count = 0;

    for fixture in scala3_library_fixtures() {
        let index = &fixture.index;

        for address in index.addresses() {
            node_count += 1;
            let raw = index
                .get(address)
                .expect("indexed address must resolve to a raw node");
            let encoded = normalized_structured_encoding(raw).unwrap_or_else(|error| {
                panic!(
                    "failed to structurally decode AST node at address {address} in {}: {error}",
                    fixture.path.display()
                )
            });

            let mut reparsed_reader = Reader::new(encoded.as_slice());
            let reparsed_nodes = RawNodes::decode(&mut reparsed_reader).unwrap_or_else(|error| {
                panic!(
                    "failed to reparse encoded AST node at address {address} in {}: {error}",
                    fixture.path.display()
                )
            });
            assert!(
                reparsed_reader.is_at_end(),
                "structured encoding left trailing bytes for AST node at address {address} in {}",
                fixture.path.display()
            );
            assert_eq!(
                reparsed_nodes.len(),
                1,
                "address {address} in {}",
                fixture.path.display()
            );

            let reparsed_raw = reparsed_nodes
                .get(0)
                .expect("one encoded node must be present");
            let reparsed_normalized = normalized_structured_encoding(reparsed_raw).unwrap_or_else(|error| {
                panic!(
                    "failed to structurally reparse AST node at address {address} in {}: {error}",
                    fixture.path.display()
                )
            });

            assert_eq!(
                encoded.as_slice(),
                reparsed_normalized.as_slice(),
                "normalized structured AST changed at address {address} in {}",
                fixture.path.display()
            );
            let mut original = Writer::new();
            raw.encode(&mut original).unwrap_or_else(|error| {
                panic!(
                    "failed to encode raw AST node at address {address} in {}: {error}",
                    fixture.path.display()
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
    for fixture in scala3_library_fixtures() {
        let file = &fixture.file;
        let structured = fixture.structured_asts();
        let original_ast_section = file
            .section(StandardSection::Asts)
            .expect("Scala library fixture must have an ASTs section");
        let ast_section = EncodedSection::structured_asts(original_ast_section.name, structured)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to encode structured ASTs in {}: {error}",
                    fixture.path.display()
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
                fixture.path.display()
            )
        });

        let encoded = builder.encode().unwrap_or_else(|error| {
            panic!(
                "failed to encode the structured file {}: {error}",
                fixture.path.display()
            )
        });
        TastyFile::parse_and_validate_scala_3_9(&encoded).unwrap_or_else(|error| {
            panic!(
                "structured file output is not a valid Scala 3.9.0 TASTy file {}: {error}",
                fixture.path.display()
            )
        });
    }
}

#[test]
fn all_scala3_library_structured_relocated_files_validate() {
    for fixture in scala3_library_fixtures() {
        let file = &fixture.file;
        let structured = fixture.structured_asts();
        let encoded = file
            .encode_structured_relocated(structured)
            .unwrap_or_else(|error| {
                panic!(
                    "failed to re-encode relocated file {}: {error}",
                    fixture.path.display()
                )
            });
        TastyFile::parse_and_validate_scala_3_9(&encoded).unwrap_or_else(|error| {
            panic!(
                "relocated file is not a valid Scala 3.9.0 TASTy file {}: {error}",
                fixture.path.display()
            )
        });
    }
}

/// Regression test for issue #9. A name reference is the zero-based index of a
/// name-table entry, in composite entries as much as in AST payloads. Read
/// one-based, a qualified name would be built from the entry after each of its
/// parts, and would pick up the section names at the start of the table
/// (`ASTs`, `Positions`, `Comments`). Every corpus unit must therefore render its
/// qualified names without them, and a signed name must have a renderable
/// original.
#[test]
fn name_references_are_zero_based_in_every_scala3_library_unit() {
    use dotty_tasty::tasty::RawName;

    // Not `Attributes`: `java.util.jar.Attributes` is a real class.
    const SECTION_NAMES: [&str; 3] = ["ASTs", "Positions", "Comments"];
    let mut qualified = 0usize;
    let mut signed = 0usize;

    for fixture in scala3_library_fixtures() {
        let names = fixture.file.names();
        for (reference, entry) in names.iter() {
            match entry {
                RawName::Qualified { .. } => {
                    qualified += 1;
                    let text = names.render(reference).unwrap_or_else(|error| {
                        panic!(
                            "{} cannot render qualified name {reference}: {error}",
                            fixture.path.display()
                        )
                    });
                    assert!(
                        !text.split('.').any(|part| SECTION_NAMES.contains(&part)),
                        "{} renders qualified name {reference} as {text:?}",
                        fixture.path.display()
                    );
                }
                RawName::Signed { original, .. } | RawName::TargetSigned { original, .. } => {
                    signed += 1;
                    assert!(
                        names.render(*original).is_ok(),
                        "{} has a signed name {reference} whose original {original} does not render",
                        fixture.path.display()
                    );
                }
                _ => {}
            }
        }
    }

    assert!(
        qualified > 0 && signed > 0,
        "the corpus has no composite names"
    );
}
