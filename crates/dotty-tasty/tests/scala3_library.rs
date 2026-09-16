use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use dotty_tasty::tasty::{PACKAGE_TAG, RawNodes, Reader, StandardSection, TastyFile, Writer};

const EXPECTED_FIXTURE_COUNT: usize = 941;
const EXPECTED_TASTY_BYTES: u64 = 7_579_936;

fn scala3_library_fixture_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scala3-library");
    let mut directories = vec![root];
    let mut fixtures = Vec::new();

    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()));

        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!(
                    "failed to read an entry in {}: {error}",
                    directory.display()
                )
            });
            let path = entry.path();
            let file_type = entry
                .file_type()
                .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()));

            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "tasty") {
                fixtures.push(path);
            }
        }
    }

    fixtures.sort();
    fixtures
}

#[test]
fn scala3_library_fixture_inventory_is_complete() {
    let fixtures = scala3_library_fixture_paths();
    let total_bytes: u64 = fixtures
        .iter()
        .map(|path| {
            fs::metadata(path)
                .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()))
                .len()
        })
        .sum();

    assert_eq!(fixtures.len(), EXPECTED_FIXTURE_COUNT);
    assert_eq!(total_bytes, EXPECTED_TASTY_BYTES);
}

#[test]
fn all_scala3_library_fixtures_decode_through_file_model() {
    for path in scala3_library_fixture_paths() {
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
    for path in scala3_library_fixture_paths() {
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

    for path in scala3_library_fixture_paths() {
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

    for path in scala3_library_fixture_paths() {
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
    for path in scala3_library_fixture_paths() {
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
    for path in scala3_library_fixture_paths() {
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

    for path in scala3_library_fixture_paths() {
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
            let structured = raw.decode_structured().unwrap_or_else(|error| {
                panic!(
                    "failed to structurally decode AST node at address {address} in {}: {error}",
                    path.display()
                )
            });

            let mut encoded = Writer::new();
            structured.encode(&mut encoded).unwrap_or_else(|error| {
                panic!(
                    "failed to encode AST node at address {address} in {}: {error}",
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
            let reparsed_structured = reparsed_raw.decode_structured().unwrap_or_else(|error| {
                panic!(
                    "failed to structurally reparse AST node at address {address} in {}: {error}",
                    path.display()
                )
            });
            let mut normalized = Writer::new();
            reparsed_structured
                .encode(&mut normalized)
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to re-encode AST node at address {address} in {}: {error}",
                        path.display()
                    )
                });

            assert_eq!(
                encoded.as_slice(),
                normalized.as_slice(),
                "structured encoding is not stable for AST node at address {address} in {}",
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
