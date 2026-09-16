use std::fs;
use std::path::{Path, PathBuf};

use dotty_tasty::tasty::{
    Attribute, Comment, DEFDEF_TAG, EXPORT_TAG, Header, IMPORT_TAG, NameTable, PACKAGE_TAG,
    RawName, RawNodes, Reader, SectionTable, StandardSection, TYPEDEF_TAG, TastyFile, VALDEF_TAG,
    Writer,
};

const EXPECTED_FIXTURE_COUNT: usize = 35;
const TASTY_MAGIC: [u8; 4] = [0x5c, 0xa1, 0xab, 0x1f];

fn tasty_fixture_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
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
fn all_tasty_fixtures_are_readable_and_have_the_tasty_magic_header() {
    let fixtures = tasty_fixture_paths();

    assert_eq!(
        fixtures.len(),
        EXPECTED_FIXTURE_COUNT,
        "fixture inventory changed; update EXPECTED_FIXTURE_COUNT deliberately"
    );

    for path in fixtures {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

        assert!(
            bytes.len() >= TASTY_MAGIC.len(),
            "fixture {} is shorter than the TASTy magic header",
            path.display()
        );
        assert_eq!(
            bytes[..TASTY_MAGIC.len()],
            TASTY_MAGIC,
            "fixture {} has an invalid TASTy magic header",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_have_the_scala_3_9_header() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let header = Header::parse(&bytes)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));

        assert_eq!(header.major_version, 28, "fixture {}", path.display());
        assert_eq!(header.minor_version, 9, "fixture {}", path.display());
        assert_eq!(header.experimental_version, 0, "fixture {}", path.display());
        assert_eq!(
            header.tooling_version,
            "Scala 3.9.0",
            "fixture {}",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_have_a_valid_name_table() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let mut reader = Reader::new(&bytes);
        Header::decode(&mut reader).unwrap_or_else(|error| {
            panic!("failed to parse header in {}: {error}", path.display())
        });
        let names = NameTable::decode(&mut reader)
            .unwrap_or_else(|error| panic!("failed to parse names in {}: {error}", path.display()));

        assert!(
            !names.is_empty(),
            "fixture {} has an empty name table",
            path.display()
        );
        assert!(
            reader.position() < bytes.len(),
            "fixture {} has no bytes after its name table",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_iterate_name_entries_with_resolvable_references() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        for (reference, entry) in file.names().iter() {
            assert_eq!(
                file.name(reference),
                Some(entry),
                "fixture {} produced an invalid iterated name reference {}",
                path.display(),
                reference
            );
        }
    }
}

#[test]
fn all_tasty_fixtures_have_a_valid_section_table() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let mut reader = Reader::new(&bytes);
        Header::decode(&mut reader).unwrap_or_else(|error| {
            panic!("failed to parse header in {}: {error}", path.display())
        });
        let names = NameTable::decode(&mut reader)
            .unwrap_or_else(|error| panic!("failed to parse names in {}: {error}", path.display()));
        let sections = SectionTable::decode(&mut reader, names.len()).unwrap_or_else(|error| {
            panic!("failed to parse sections in {}: {error}", path.display())
        });

        assert!(
            !sections.is_empty(),
            "fixture {} has no sections",
            path.display()
        );
        assert!(
            reader.is_at_end(),
            "fixture {} was not fully consumed",
            path.display()
        );
        assert!(
            sections.iter().any(|section| section.length > 0),
            "fixture {} has no non-empty sections",
            path.display()
        );
        assert!(
            sections
                .iter()
                .any(|section| section.standard_kind(&names) == Some(StandardSection::Asts)),
            "fixture {} has no ASTs section",
            path.display()
        );

        if let Some(attributes) = sections
            .iter()
            .find(|section| section.standard_kind(&names) == Some(StandardSection::Attributes))
        {
            for attribute in attributes.decode_attributes().unwrap_or_else(|error| {
                panic!("failed to parse attributes in {}: {error}", path.display())
            }) {
                if let dotty_tasty::tasty::Attribute::SourceFile(name) = attribute {
                    assert!(
                        matches!(
                            names.entries().get(name as usize),
                            Some(dotty_tasty::tasty::RawName::Utf8(_))
                        ),
                        "fixture {} has a non-UTF-8 SOURCEFILE reference {}: {:?}",
                        path.display(),
                        name,
                        names.entries().get(name as usize)
                    );
                }
            }
        }

        if let Some(comments) = sections
            .iter()
            .find(|section| section.standard_kind(&names) == Some(StandardSection::Comments))
        {
            comments.decode_comments().unwrap_or_else(|error| {
                panic!("failed to parse comments in {}: {error}", path.display())
            });
        }

        if let Some(positions) = sections
            .iter()
            .find(|section| section.standard_kind(&names) == Some(StandardSection::Positions))
        {
            positions.decode_positions().unwrap_or_else(|error| {
                panic!("failed to parse positions in {}: {error}", path.display())
            });
        }

        let asts = sections
            .iter()
            .find(|section| section.standard_kind(&names) == Some(StandardSection::Asts))
            .expect("ASTs section was checked above");
        let mut ast_reader = asts.reader();
        let nodes = RawNodes::decode(&mut ast_reader)
            .unwrap_or_else(|error| panic!("failed to parse ASTs in {}: {error}", path.display()));

        assert!(
            !nodes.is_empty(),
            "fixture {} has no top-level AST nodes",
            path.display()
        );
        assert!(
            ast_reader.is_at_end(),
            "fixture {} has unread AST bytes",
            path.display()
        );

        let package = nodes
            .get(0)
            .expect("ASTs section has a first node")
            .decode_package()
            .unwrap_or_else(|error| {
                panic!("failed to parse package in {}: {error}", path.display())
            });
        assert!(
            names.get(package.path_name).is_some(),
            "fixture {} has an unresolved package path name",
            path.display()
        );
        assert!(
            !package.stats.is_empty(),
            "fixture {} has no package stats",
            path.display()
        );

        for stat in package.stats.iter() {
            if matches!(
                stat.tag,
                dotty_tasty::tasty::IMPORT_TAG | dotty_tasty::tasty::EXPORT_TAG
            ) {
                stat.decode_import_export().unwrap_or_else(|error| {
                    panic!(
                        "failed to parse import/export in {}: {error}",
                        path.display()
                    )
                });
            }
            if matches!(stat.tag, 129..=131) {
                let definition = stat.decode_definition().unwrap_or_else(|error| {
                    panic!("failed to parse definition in {}: {error}", path.display())
                });
                assert!(
                    names.get(definition.name()).is_some(),
                    "fixture {} has an unresolved definition name",
                    path.display()
                );
                let body = stat.decode_definition_body().unwrap_or_else(|error| {
                    panic!(
                        "failed to parse definition body in {}: {error}",
                        path.display()
                    )
                });
                if let dotty_tasty::tasty::DefinitionBody::TypeDef {
                    type_or_template: dotty_tasty::tasty::RawTree::LengthNode(template),
                    ..
                } = body
                    && template.tag == dotty_tasty::tasty::TEMPLATE_TAG
                {
                    let structure = template
                        .decode_template_structure()
                        .unwrap_or_else(|error| {
                            panic!("failed to parse template in {}: {error}", path.display())
                        });
                    for parameter in structure
                        .type_params
                        .iter()
                        .chain(structure.term_params.iter())
                    {
                        parameter.decode_body().unwrap_or_else(|error| {
                            panic!(
                                "failed to parse parameter body in {}: {error}",
                                path.display()
                            )
                        });
                    }
                    if let Some(self_def) = structure.self_def.as_ref() {
                        self_def.decode_self_def().unwrap_or_else(|error| {
                            panic!(
                                "failed to parse self definition in {}: {error}",
                                path.display()
                            )
                        });
                    }
                    for parent in &structure.parents {
                        if let dotty_tasty::tasty::RawTree::Ast { tag, .. } = parent
                            && matches!(
                                *tag,
                                dotty_tasty::tasty::THIS_TAG
                                    | dotty_tasty::tasty::NEW_TAG
                                    | dotty_tasty::tasty::THROW_TAG
                                    | dotty_tasty::tasty::ELIDED_TAG
                            )
                        {
                            parent.decode_ast_child(*tag).unwrap_or_else(|error| {
                                panic!(
                                    "failed to parse category-three tree in {}: {error}",
                                    path.display()
                                )
                            });
                        }
                        if let dotty_tasty::tasty::RawTree::LengthNode(raw) = parent {
                            match raw.tag {
                                dotty_tasty::tasty::APPLY_TAG => {
                                    raw.decode_apply().unwrap_or_else(|error| {
                                        panic!(
                                            "failed to parse apply in {}: {error}",
                                            path.display()
                                        )
                                    });
                                }
                                dotty_tasty::tasty::BLOCK_TAG => {
                                    raw.decode_block().unwrap_or_else(|error| {
                                        panic!(
                                            "failed to parse block in {}: {error}",
                                            path.display()
                                        )
                                    });
                                }
                                dotty_tasty::tasty::TYPEAPPLY_TAG => {
                                    raw.decode_type_apply().unwrap_or_else(|error| {
                                        panic!(
                                            "failed to parse type apply in {}: {error}",
                                            path.display()
                                        )
                                    });
                                }
                                dotty_tasty::tasty::TYPED_TAG => {
                                    raw.decode_typed().unwrap_or_else(|error| {
                                        panic!(
                                            "failed to parse typed tree in {}: {error}",
                                            path.display()
                                        )
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                    for nested in structure.stats.iter() {
                        if matches!(
                            nested.tag,
                            dotty_tasty::tasty::IMPORT_TAG | dotty_tasty::tasty::EXPORT_TAG
                        ) {
                            nested.decode_import_export().unwrap_or_else(|error| {
                                panic!(
                                    "failed to parse nested import/export in {}: {error}",
                                    path.display()
                                )
                            });
                        }
                        if nested.tag == dotty_tasty::tasty::DEFDEF_TAG {
                            nested.decode_defdef_body().unwrap_or_else(|error| {
                                panic!("failed to parse defdef body in {}: {error}", path.display())
                            });
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn all_tasty_fixtures_decode_through_the_complete_file_model() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        file.validate()
            .unwrap_or_else(|error| panic!("failed to validate {}: {error}", path.display()));

        assert_eq!(
            file.header().major_version,
            28,
            "fixture {}",
            path.display()
        );
        assert!(!file.names().is_empty(), "fixture {}", path.display());
        assert!(!file.sections().is_empty(), "fixture {}", path.display());
        let mut name_builder = NameTable::builder();
        for entry in file.names().entries() {
            name_builder.intern(entry.clone()).unwrap_or_else(|error| {
                panic!("failed to intern names in {}: {error}", path.display())
            });
        }
        let rebuilt_names = name_builder
            .finish()
            .unwrap_or_else(|error| panic!("failed to build names in {}: {error}", path.display()));
        let mut original_names_bytes = Writer::new();
        file.names().encode(&mut original_names_bytes).unwrap();
        let mut rebuilt_names_bytes = Writer::new();
        rebuilt_names.encode(&mut rebuilt_names_bytes).unwrap();
        assert_eq!(
            rebuilt_names_bytes.as_slice(),
            original_names_bytes.as_slice(),
            "fixture {}",
            path.display()
        );
        assert!(
            !file.asts().unwrap().is_empty(),
            "fixture {}",
            path.display()
        );
        assert_eq!(
            file.structured_asts().unwrap().len(),
            file.asts().unwrap().len(),
            "fixture {}",
            path.display()
        );
        for reference in file.ast_references().unwrap_or_else(|error| {
            panic!(
                "failed to collect AST references in {}: {error}",
                path.display()
            )
        }) {
            assert!(
                file.ast_at(reference.owner_address)
                    .unwrap_or_else(|error| {
                        panic!("failed to resolve AST owner in {}: {error}", path.display())
                    })
                    .is_some(),
                "fixture {} has an AST reference with a missing top-level owner {}",
                path.display(),
                reference.owner_address
            );
        }
        let ast_section = file.section(StandardSection::Asts).unwrap();
        let asts = file.asts().unwrap();
        let encoded_asts = asts.encode_with_addresses().unwrap_or_else(|error| {
            panic!(
                "failed to encode AST addresses in {}: {error}",
                path.display()
            )
        });
        assert_eq!(
            encoded_asts.as_slice(),
            ast_section.payload,
            "fixture {}",
            path.display()
        );
        for (node, address) in asts.iter().zip(encoded_asts.addresses()) {
            assert_eq!(*address as usize, node.offset, "fixture {}", path.display());
            assert_eq!(
                file.ast_at(*address).unwrap().as_ref().map(|node| node.tag),
                Some(node.tag),
                "fixture {}",
                path.display()
            );
        }
        file.attributes().unwrap_or_else(|error| {
            panic!("failed to decode attributes in {}: {error}", path.display())
        });
        file.comments().unwrap_or_else(|error| {
            panic!("failed to decode comments in {}: {error}", path.display())
        });
        file.positions().unwrap_or_else(|error| {
            panic!("failed to decode positions in {}: {error}", path.display())
        });
    }
}

#[test]
fn all_tasty_fixtures_pass_eager_compatible_validation() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_and_validate_compatible_with(&bytes, 28, 10, 0).unwrap_or_else(
            |error| {
                panic!(
                    "failed compatible validation for {}: {error}",
                    path.display()
                )
            },
        );

        assert_eq!(
            file.header().minor_version,
            9,
            "fixture {} did not preserve its original version",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixture_top_level_definitions_decode_structurally() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        for node in file.asts().unwrap().iter() {
            let result = match node.tag {
                PACKAGE_TAG => node.decode_package().map(|_| ()),
                VALDEF_TAG | TYPEDEF_TAG => node.decode_definition_body().map(|_| ()),
                DEFDEF_TAG => node.decode_defdef_body().map(|_| ()),
                IMPORT_TAG | EXPORT_TAG => node.decode_import_export().map(|_| ()),
                _ => Ok(()),
            };
            result.unwrap_or_else(|error| {
                panic!(
                    "failed to structurally decode top-level tag {} at offset {} in {}: {error}",
                    node.tag,
                    node.offset,
                    path.display()
                )
            });

            if matches!(
                node.tag,
                PACKAGE_TAG | VALDEF_TAG | DEFDEF_TAG | TYPEDEF_TAG | IMPORT_TAG | EXPORT_TAG
            ) {
                let structured = node.decode_structured().unwrap_or_else(|error| {
                    panic!(
                        "failed to dispatch top-level tag {} in {}: {error}",
                        node.tag,
                        path.display()
                    )
                });
                assert!(
                    !matches!(structured, dotty_tasty::tasty::StructuredNode::Raw(_)),
                    "known top-level tag {} was not dispatched structurally in {}",
                    node.tag,
                    path.display()
                );
            }
        }
    }
}

#[test]
fn all_tasty_fixtures_round_trip_top_level_structured_ast_nodes() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let ast_section = file.section(StandardSection::Asts).unwrap();
        let structured = file.structured_asts().unwrap_or_else(|error| {
            panic!(
                "failed to decode structured AST nodes in {}: {error}",
                path.display()
            )
        });
        let mut writer = Writer::new();

        for node in &structured {
            node.encode(&mut writer).unwrap_or_else(|error| {
                panic!(
                    "failed to encode structured AST node in {}: {error}",
                    path.display()
                )
            });
        }

        assert_eq!(
            writer.as_slice(),
            ast_section.payload,
            "structured AST round-trip changed {}",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_round_trip_every_indexed_structured_ast_node() {
    let mut nested_node_count = 0usize;

    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let top_level_count = file.asts().unwrap().len();
        let index = file.ast_address_index().unwrap_or_else(|error| {
            panic!("failed to index AST nodes in {}: {error}", path.display())
        });
        nested_node_count += index.len().saturating_sub(top_level_count);

        for address in index.addresses() {
            let raw = index.get(address).unwrap_or_else(|| {
                panic!(
                    "indexed category-five node at {} is not retrievable in {}",
                    address,
                    path.display()
                )
            });
            let structured = raw.decode_structured().unwrap_or_else(|error| {
                panic!(
                    "failed to decode indexed node at {} in {}: {error}",
                    address,
                    path.display()
                )
            });
            let mut structured_bytes = Writer::new();
            structured
                .encode(&mut structured_bytes)
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to encode indexed node at {} in {}: {error}",
                        address,
                        path.display()
                    )
                });
            let mut raw_bytes = Writer::new();
            raw.encode(&mut raw_bytes).unwrap_or_else(|error| {
                panic!(
                    "failed to encode raw indexed node at {} in {}: {error}",
                    address,
                    path.display()
                )
            });

            assert_eq!(
                structured_bytes.as_slice(),
                raw_bytes.as_slice(),
                "structured round-trip changed node at {} in {}",
                address,
                path.display()
            );
        }
    }

    assert!(
        nested_node_count > 0,
        "fixture corpus has no nested AST nodes"
    );
}

#[test]
fn all_tasty_fixture_top_level_nodes_expose_structured_ast_references() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        for node in file.asts().unwrap().iter() {
            node.ast_refs().unwrap_or_else(|error| {
                panic!(
                    "failed to collect AST references from tag {} at offset {} in {}: {error}",
                    node.tag,
                    node.offset,
                    path.display()
                )
            });
        }
    }
}

#[test]
fn all_tasty_fixture_name_references_resolve_in_the_name_table() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        for reference in file.name_references().unwrap_or_else(|error| {
            panic!(
                "failed to collect name references from {}: {error}",
                path.display()
            )
        }) {
            assert!(
                file.name(reference.reference).is_some(),
                "fixture {} has an unresolved name reference {}",
                path.display(),
                reference.reference
            );
            assert!(
                file.ast_at(reference.owner_address)
                    .unwrap_or_else(|error| {
                        panic!("failed to resolve AST owner in {}: {error}", path.display())
                    })
                    .is_some(),
                "fixture {} has a name reference with a missing owner {}",
                path.display(),
                reference.owner_address
            );
        }
    }
}

#[test]
fn all_tasty_fixture_name_reference_queries_match_the_complete_reference_list() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let references = file.name_references().unwrap_or_else(|error| {
            panic!(
                "failed to collect name references in {}: {error}",
                path.display()
            )
        });

        for reference in &references {
            assert!(
                file.name_references_from(reference.owner_address)
                    .unwrap_or_else(|error| {
                        panic!("failed to query name owner in {}: {error}", path.display())
                    })
                    .contains(reference),
                "fixture {} lost name reference from owner {}",
                path.display(),
                reference.owner_address
            );
            assert!(
                file.name_references_to(reference.reference)
                    .unwrap_or_else(|error| {
                        panic!("failed to query name target in {}: {error}", path.display())
                    })
                    .contains(reference),
                "fixture {} lost name reference to {}",
                path.display(),
                reference.reference
            );
        }
    }
}

#[test]
fn all_tasty_fixture_position_sections_resolve_without_overflow() {
    let mut section_count = 0;

    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        if let Some(positions) = file.positions().unwrap_or_else(|error| {
            panic!("failed to decode positions in {}: {error}", path.display())
        }) {
            section_count += 1;
            let resolved = positions.resolved_entries().unwrap_or_else(|error| {
                panic!("failed to resolve positions in {}: {error}", path.display())
            });
            assert_eq!(
                resolved.len(),
                positions.entries.len(),
                "fixture {} changed position-entry cardinality",
                path.display()
            );
        }
    }

    assert!(section_count > 0, "fixtures contain no Positions section");
}

#[test]
fn all_tasty_fixture_position_associations_have_resolved_coordinates() {
    let mut association_count = 0;

    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        if let Some(positions) = file.positions().unwrap_or_else(|error| {
            panic!("failed to decode positions in {}: {error}", path.display())
        }) {
            let associations = positions.resolved_associations().unwrap_or_else(|error| {
                panic!(
                    "failed to resolve position associations in {}: {error}",
                    path.display()
                )
            });
            association_count += associations.len();
            assert!(
                associations.len() <= positions.entries.len(),
                "fixture {} has more resolved associations than raw entries",
                path.display()
            );
        }
    }

    assert!(
        association_count > 0,
        "fixtures contain no position associations"
    );
}

#[test]
fn all_tasty_fixture_position_mappings_target_visible_ast_nodes() {
    let mut mapping_count = 0;

    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let index = file.ast_address_index().unwrap_or_else(|error| {
            panic!("failed to index AST nodes in {}: {error}", path.display())
        });

        if let Some(mapped) = file.ast_node_positions().unwrap_or_else(|error| {
            panic!("failed to map AST positions in {}: {error}", path.display())
        }) {
            mapping_count += mapped.len();
            for entry in mapped {
                assert_eq!(
                    index.get_node(entry.node.offset as u32),
                    Some(entry.node),
                    "position mapping targets a missing node in {}",
                    path.display()
                );
                assert_eq!(
                    entry.position.address,
                    entry.node.offset as i64,
                    "position mapping has a mismatched address in {}",
                    path.display()
                );
            }
        }
    }

    assert!(
        mapping_count > 0,
        "fixtures contain no position-to-AST mappings"
    );
}

#[test]
fn all_tasty_fixture_source_range_queries_return_only_overlapping_positions() {
    let mut matching_count = 0;

    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let Some(mapped) = file.ast_node_positions().unwrap_or_else(|error| {
            panic!("failed to map AST positions in {}: {error}", path.display())
        }) else {
            continue;
        };
        let Some(first) = mapped.first() else {
            continue;
        };
        let start = first.position.start;
        let end = first.position.end.max(start + 1);
        let filtered = file
            .ast_nodes_in_source_range(start, end)
            .unwrap_or_else(|error| panic!("failed to query {}: {error}", path.display()))
            .expect("Positions section was present");

        matching_count += filtered.len();
        assert!(
            filtered.iter().all(|entry| {
                if entry.position.start < entry.position.end {
                    entry.position.start < end && start < entry.position.end
                } else {
                    entry.position.point >= start && entry.position.point < end
                }
            }),
            "source range query returned a non-overlapping position in {}",
            path.display()
        );
        assert!(
            filtered.iter().any(|entry| entry.node == first.node),
            "source range query omitted the selected position in {}",
            path.display()
        );
    }

    assert!(
        matching_count > 0,
        "fixtures contain no source range matches"
    );
}

#[test]
fn all_tasty_fixture_signed_names_have_typed_views() {
    let mut signed_name_count = 0;

    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        for (index, name) in file.names().entries().iter().enumerate() {
            if matches!(name, RawName::Signed { .. } | RawName::TargetSigned { .. }) {
                signed_name_count += 1;
                let reference = index as u32 + 1;
                assert!(
                    name.signed_name().is_some(),
                    "fixture {} has a malformed typed signature at name {}",
                    path.display(),
                    reference
                );
                assert!(
                    file.names()
                        .render_signed_name(reference)
                        .unwrap_or_else(|error| {
                            panic!(
                                "fixture {} cannot render typed signature at name {}: {error}",
                                path.display(),
                                reference
                            )
                        })
                        .is_some()
                );
            }
        }
    }

    assert!(
        signed_name_count > 0,
        "fixtures contain no signature-bearing names"
    );
}

#[test]
fn all_tasty_fixtures_preserve_name_dependency_order() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let names = file.names();

        for root in 1..=names.len() as u32 {
            let order = names.dependency_order(root).unwrap_or_else(|| {
                panic!(
                    "fixture {} has no dependency order for name {}",
                    path.display(),
                    root
                )
            });
            let root_position = order
                .iter()
                .position(|reference| *reference == root)
                .unwrap();

            for dependency in &order[..root_position] {
                assert!(
                    names.get(*dependency).is_some(),
                    "fixture {} has an unresolved dependency {} of name {}",
                    path.display(),
                    dependency,
                    root
                );
            }
            for (position, current) in order.iter().enumerate() {
                let name = names.get(*current).unwrap();
                name.visit_references(&mut |dependency| {
                    let dependency_position = order
                        .iter()
                        .position(|reference| *reference == dependency)
                        .unwrap();
                    assert!(
                        dependency_position < position,
                        "fixture {} places dependency {} after name {}",
                        path.display(),
                        dependency,
                        current
                    );
                });
            }
            assert_eq!(order.last(), Some(&root));
        }
    }
}

#[test]
fn all_tasty_fixture_ast_references_resolve_in_the_global_ast_index() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let top_level_count = file.asts().unwrap().len();
        let index = file.ast_address_index().unwrap_or_else(|error| {
            panic!("failed to index AST nodes in {}: {error}", path.display())
        });

        assert!(
            index.len() >= top_level_count,
            "global AST index in {} lost top-level nodes",
            path.display()
        );

        for reference in file.ast_references().unwrap_or_else(|error| {
            panic!(
                "failed to collect AST references from {}: {error}",
                path.display()
            )
        }) {
            assert!(
                index.resolve_node(reference.reference).is_some(),
                "AST reference {} from owner {} does not resolve in {}",
                reference.reference.address,
                reference.owner_address,
                path.display()
            );
            assert_eq!(
                file.resolve_ast_reference(reference.reference).unwrap(),
                index.resolve_node(reference.reference)
            );
        }

        file.validate_ast_reference_targets()
            .unwrap_or_else(|error| {
                panic!(
                    "AST reference target validation failed in {}: {error}",
                    path.display()
                )
            });
    }
}

#[test]
fn all_tasty_fixtures_iterate_indexed_ast_nodes_in_address_order() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let index = file.ast_address_index().unwrap_or_else(|error| {
            panic!("failed to index AST nodes in {}: {error}", path.display())
        });

        let category_five_addresses = index.addresses().collect::<Vec<_>>();
        let iterated_category_five_addresses = index
            .iter()
            .map(|node| node.offset as u32)
            .collect::<Vec<_>>();
        assert_eq!(
            iterated_category_five_addresses,
            category_five_addresses,
            "category-five iterator disagrees with addresses in {}",
            path.display()
        );

        let visible_addresses = index.node_addresses().collect::<Vec<_>>();
        let iterated_visible_addresses = index
            .iter_nodes()
            .map(|node| node.offset as u32)
            .collect::<Vec<_>>();
        assert_eq!(
            iterated_visible_addresses,
            visible_addresses,
            "visible-node iterator disagrees with node_addresses in {}",
            path.display()
        );
        assert!(
            visible_addresses.windows(2).all(|pair| pair[0] <= pair[1]),
            "visible AST addresses are not ordered in {}",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_preserve_ast_parent_child_relationships() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let index = file.ast_address_index().unwrap_or_else(|error| {
            panic!("failed to index AST nodes in {}: {error}", path.display())
        });
        let edges = index.iter_tree_edges().collect::<Vec<_>>();

        for edge in &edges {
            assert_eq!(
                index.get_node(edge.parent.offset as u32),
                Some(edge.parent),
                "edge parent is not indexed in {}",
                path.display()
            );
            assert_eq!(
                index.get_node(edge.child.offset as u32),
                Some(edge.child),
                "edge child is not indexed in {}",
                path.display()
            );
            assert_eq!(
                index.parent_of(edge.child.offset as u32),
                Some(edge.parent),
                "parent lookup disagrees in {}",
                path.display()
            );
        }

        for parent in index.iter_nodes() {
            let expected = edges
                .iter()
                .filter(|edge| edge.parent == parent)
                .map(|edge| edge.child)
                .collect::<Vec<_>>();
            let actual = index.children_of(parent.offset as u32).collect::<Vec<_>>();
            assert_eq!(
                actual,
                expected,
                "children lookup disagrees for address {} in {}",
                parent.offset,
                path.display()
            );
        }
    }
}

#[test]
fn all_tasty_fixtures_filter_visible_ast_nodes_by_tag() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let index = file.ast_address_index().unwrap_or_else(|error| {
            panic!("failed to index AST nodes in {}: {error}", path.display())
        });

        for tag in [crate::PACKAGE_TAG, crate::VALDEF_TAG, crate::DEFDEF_TAG] {
            let expected = index
                .iter_nodes()
                .filter(|node| node.tag == tag)
                .collect::<Vec<_>>();
            let actual = file.ast_nodes_with_tag(tag).unwrap_or_else(|error| {
                panic!("failed to query tag {tag} in {}: {error}", path.display())
            });
            assert_eq!(
                actual,
                expected,
                "tag query disagrees in {}",
                path.display()
            );
        }
    }
}

#[test]
fn all_tasty_fixture_ast_reference_queries_match_the_complete_reference_list() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let references = file.ast_references().unwrap_or_else(|error| {
            panic!(
                "failed to collect AST references in {}: {error}",
                path.display()
            )
        });

        for reference in &references {
            assert!(
                file.ast_references_from(reference.owner_address)
                    .unwrap_or_else(|error| {
                        panic!("failed to query owner in {}: {error}", path.display())
                    })
                    .contains(reference),
                "fixture {} lost reference from owner {}",
                path.display(),
                reference.owner_address
            );
            assert!(
                file.ast_references_to(reference.reference.address)
                    .unwrap_or_else(|error| {
                        panic!("failed to query target in {}: {error}", path.display())
                    })
                    .contains(reference),
                "fixture {} lost reference to address {}",
                path.display(),
                reference.reference.address
            );
        }
    }
}

#[test]
fn all_tasty_fixtures_round_trip_byte_for_byte_through_the_raw_encoder() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let encoded = file
            .encode()
            .unwrap_or_else(|error| panic!("failed to encode {}: {error}", path.display()));

        assert_eq!(
            encoded,
            bytes,
            "fixture {} changed during round-trip",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_round_trip_through_the_validated_file_encoder() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let encoded = file.encode_validated().unwrap_or_else(|error| {
            panic!("failed to validate and encode {}: {error}", path.display())
        });

        assert_eq!(
            encoded,
            bytes,
            "validated round-trip changed {}",
            path.display()
        );
    }
}

#[test]
fn all_tasty_fixtures_round_trip_through_the_file_encoder_with_ast_addresses() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let encoded = file.encode_with_ast_addresses().unwrap_or_else(|error| {
            panic!(
                "failed to encode {} with AST addresses: {error}",
                path.display()
            )
        });

        assert_eq!(encoded.as_slice(), bytes, "fixture {}", path.display());
        let asts = file.asts().unwrap();
        assert_eq!(
            encoded.ast_addresses().len(),
            asts.len(),
            "fixture {}",
            path.display()
        );
        for (node, address) in asts.iter().zip(encoded.ast_addresses()) {
            assert_eq!(*address as usize, node.offset, "fixture {}", path.display());
        }
    }
}

#[test]
fn all_tasty_fixtures_round_trip_through_the_validated_address_encoder() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
        let encoded = file
            .encode_validated_with_ast_addresses()
            .unwrap_or_else(|error| {
                panic!(
                    "failed to validate and encode {} with AST addresses: {error}",
                    path.display()
                )
            });
        let asts = file.asts().unwrap();

        assert_eq!(
            encoded.as_slice(),
            bytes,
            "validated address round-trip changed {}",
            path.display()
        );
        assert_eq!(
            encoded.ast_addresses().len(),
            asts.len(),
            "fixture {}",
            path.display()
        );
        for (node, address) in asts.iter().zip(encoded.ast_addresses()) {
            assert_eq!(*address as usize, node.offset, "fixture {}", path.display());
        }
    }
}

#[test]
fn all_tasty_fixture_standard_sections_round_trip_through_structured_encoders() {
    for path in tasty_fixture_paths() {
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let file = TastyFile::parse_scala_3_9(&bytes)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        for kind in [
            StandardSection::Attributes,
            StandardSection::Comments,
            StandardSection::Positions,
        ] {
            let Some(section) = file.section(kind) else {
                continue;
            };
            let mut writer = Writer::new();
            match kind {
                StandardSection::Attributes => {
                    let attributes = section.decode_attributes().unwrap_or_else(|error| {
                        panic!("failed to decode attributes in {}: {error}", path.display())
                    });
                    Attribute::encode_all(&attributes, &mut writer).unwrap_or_else(|error| {
                        panic!("failed to encode attributes in {}: {error}", path.display())
                    });
                }
                StandardSection::Comments => {
                    let comments = section.decode_comments().unwrap_or_else(|error| {
                        panic!("failed to decode comments in {}: {error}", path.display())
                    });
                    Comment::encode_all(&comments, &mut writer).unwrap_or_else(|error| {
                        panic!("failed to encode comments in {}: {error}", path.display())
                    });
                }
                StandardSection::Positions => {
                    let positions = section.decode_positions().unwrap_or_else(|error| {
                        panic!("failed to decode positions in {}: {error}", path.display())
                    });
                    positions.encode(&mut writer).unwrap_or_else(|error| {
                        panic!("failed to encode positions in {}: {error}", path.display())
                    });
                }
                StandardSection::Asts => unreachable!("ASTs are tested by the raw round-trip"),
            }

            assert_eq!(
                writer.as_slice(),
                section.payload,
                "{} section changed during structured round-trip for {}",
                kind.as_str(),
                path.display()
            );
        }
    }
}
