use std::fs;
use std::path::{Path, PathBuf};

use tasty_rs::{Header, NameTable, RawNodes, Reader, SectionTable, StandardSection};

const EXPECTED_FIXTURE_COUNT: usize = 33;
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
                if let tasty_rs::Attribute::SourceFile(name) = attribute {
                    assert!(
                        matches!(
                            names.entries().get(name as usize),
                            Some(tasty_rs::RawName::Utf8(_))
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
            if matches!(stat.tag, tasty_rs::IMPORT_TAG | tasty_rs::EXPORT_TAG) {
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
                if let tasty_rs::DefinitionBody::TypeDef {
                    type_or_template: tasty_rs::RawTree::LengthNode(template),
                    ..
                } = body
                {
                    if template.tag == tasty_rs::TEMPLATE_TAG {
                        let structure =
                            template
                                .decode_template_structure()
                                .unwrap_or_else(|error| {
                                    panic!(
                                        "failed to parse template in {}: {error}",
                                        path.display()
                                    )
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
                            if let tasty_rs::RawTree::LengthNode(raw) = parent {
                                match raw.tag {
                                    tasty_rs::APPLY_TAG => {
                                        raw.decode_apply().unwrap_or_else(|error| {
                                            panic!(
                                                "failed to parse apply in {}: {error}",
                                                path.display()
                                            )
                                        });
                                    }
                                    tasty_rs::BLOCK_TAG => {
                                        raw.decode_block().unwrap_or_else(|error| {
                                            panic!(
                                                "failed to parse block in {}: {error}",
                                                path.display()
                                            )
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                        for nested in structure.stats.iter() {
                            if matches!(nested.tag, tasty_rs::IMPORT_TAG | tasty_rs::EXPORT_TAG) {
                                nested.decode_import_export().unwrap_or_else(|error| {
                                    panic!(
                                        "failed to parse nested import/export in {}: {error}",
                                        path.display()
                                    )
                                });
                            }
                            if nested.tag == tasty_rs::DEFDEF_TAG {
                                nested.decode_defdef_body().unwrap_or_else(|error| {
                                    panic!(
                                        "failed to parse defdef body in {}: {error}",
                                        path.display()
                                    )
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}
