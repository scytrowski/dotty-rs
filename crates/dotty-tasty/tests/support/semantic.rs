use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SemanticExpectation {
    pub schema_version: u32,
    pub scala_version: String,
    pub files: Vec<SemanticExpectationFile>,
}

#[derive(Debug, Deserialize)]
pub struct SemanticExpectationFile {
    pub path: String,
    pub declarations: Vec<SemanticExpectationDeclaration>,
    pub shapes: BTreeMap<String, usize>,
    pub parameter_clause_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Deserialize)]
pub struct SemanticExpectationDeclaration {
    pub kind: String,
    pub name: String,
    pub parameter_clauses: Option<usize>,
}

pub fn load(path: &Path) -> Result<SemanticExpectation, String> {
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "failed to read semantic expectations {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "failed to parse semantic expectations {}: {error}",
            path.display()
        )
    })
}

pub fn assert_expectations_cover_selected_fixtures(
    corpus: &super::corpus::Corpus,
    expected_schema_version: u32,
    expected_scala_version: &str,
    compatible_compiler_version: (u32, u32, u32),
) {
    let expectation_path = corpus
        .expectation_path()
        .expect("corpus manifest must identify its semantic expectations");
    let expectation = load(&expectation_path).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(expectation.schema_version, expected_schema_version);
    assert_eq!(expectation.scala_version, expected_scala_version);
    assert_eq!(
        expectation.files.len(),
        corpus.selected_fixture_paths().len()
    );

    let fixture_root = corpus.tasty_root_path();
    let selected_paths: std::collections::BTreeSet<_> = corpus
        .selected_fixture_paths()
        .iter()
        .map(|path| {
            path.strip_prefix(&fixture_root)
                .expect("selected fixture must be inside corpus root")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
        .collect();
    let expectation_paths: std::collections::BTreeSet<_> = expectation
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    assert_eq!(expectation_paths, selected_paths);

    for expected in expectation.files {
        assert!(
            !expected.declarations.is_empty(),
            "{} has no semantic-lite declarations",
            expected.path
        );
        assert!(
            expected
                .declarations
                .iter()
                .all(|declaration| !declaration.kind.is_empty()),
            "semantic expectation for {} contains an empty declaration kind",
            expected.path
        );
        assert!(
            expected
                .declarations
                .iter()
                .all(|declaration| !declaration.name.contains('\0')),
            "semantic expectation for {} contains an invalid declaration name",
            expected.path
        );
        assert!(
            expected.declarations.iter().all(|declaration| {
                (declaration.kind == "DefDef") == declaration.parameter_clauses.is_some()
            }),
            "semantic expectation for {} has inconsistent parameter-clause metadata",
            expected.path
        );
        let fixture_path = fixture_root.join(&expected.path);
        let fixture = super::corpus::parsed_fixtures(
            corpus,
            compatible_compiler_version.0,
            compatible_compiler_version.1,
            compatible_compiler_version.2,
        )
        .iter()
        .find(|fixture| fixture.path == fixture_path)
        .unwrap_or_else(|| panic!("missing cached fixture {}", fixture_path.display()));
        let file = &fixture.file;
        let index = &fixture.index;
        let mut actual_shapes = BTreeMap::new();
        for node in index.iter_nodes() {
            if let Some(shape) = semantic_shape(node.tag) {
                *actual_shapes.entry(shape.to_owned()).or_insert(0) += 1;
            }
        }
        for (shape, expected_count) in &expected.shapes {
            let actual_count = actual_shapes.get(shape).copied().unwrap_or_default();
            assert!(
                actual_count > 0,
                "{} has no wire-level {shape} node (oracle has {expected_count})",
                expected.path
            );
        }

        let mut actual_parameter_clause_counts = BTreeMap::new();
        for node in index
            .iter()
            .filter(|node| node.tag == dotty_tasty::tasty::DEFDEF_TAG)
        {
            let body = node.decode_defdef_body().unwrap_or_else(|error| {
                panic!(
                    "failed to decode DefDef in {}: {error}",
                    fixture_path.display()
                )
            });
            let count = if body.parameters.is_empty() && body.clauses.is_empty() {
                0
            } else {
                body.clauses.len() + 1
            };
            *actual_parameter_clause_counts
                .entry(count.to_string())
                .or_insert(0) += 1;
        }
        let mut matched_parameter_clause_arity = false;
        for clause_count in expected.parameter_clause_counts.keys() {
            let actual_occurrences = actual_parameter_clause_counts
                .get(clause_count)
                .copied()
                .unwrap_or_default();
            matched_parameter_clause_arity |= actual_occurrences > 0;
        }
        if !expected.parameter_clause_counts.is_empty() {
            assert!(
                matched_parameter_clause_arity,
                "{} has no wire-level method with a parameter-clause arity present in the oracle",
                expected.path
            );
        }
        let actual_names: std::collections::BTreeSet<_> = file
            .names()
            .iter()
            .filter_map(|(reference, _)| {
                let name = file.render_name(reference).ok().or_else(|| {
                    match file.render_signed_name(reference).ok()? {
                        Some(dotty_tasty::tasty::RenderedSignedName::Signed {
                            original, ..
                        })
                        | Some(dotty_tasty::tasty::RenderedSignedName::TargetSigned {
                            original,
                            ..
                        }) => Some(original),
                        None => None,
                    }
                })?;
                let name = name.rsplit('.').next().unwrap_or(&name);
                Some(name.strip_suffix('$').unwrap_or(name).to_owned())
            })
            .collect();
        let actual_definition_kinds: std::collections::BTreeSet<_> = index
            .iter()
            .filter_map(|node| match node.tag {
                dotty_tasty::tasty::DEFDEF_TAG => Some("DefDef"),
                dotty_tasty::tasty::TYPEDEF_TAG => Some("TypeDef"),
                dotty_tasty::tasty::VALDEF_TAG => Some("ValDef"),
                _ => None,
            })
            .collect();
        let expected_definition_kinds: std::collections::BTreeSet<_> = expected
            .declarations
            .iter()
            .map(|declaration| declaration.kind.as_str())
            .collect();
        for kind in expected_definition_kinds {
            assert!(
                actual_definition_kinds.contains(kind),
                "{} has no indexed {kind} declaration",
                expected.path
            );
        }
        for declaration in &expected.declarations {
            assert!(
                actual_names.contains(&declaration.name),
                "{} is missing semantic-lite name {} ({})",
                expected.path,
                declaration.name,
                declaration.kind
            );
        }
        assert!(
            !file
                .asts()
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to decode ASTs in {}: {error}",
                        fixture_path.display()
                    )
                })
                .is_empty(),
            "{} has no AST roots",
            expected.path
        );
    }
}

fn semantic_shape(tag: u8) -> Option<&'static str> {
    match tag {
        dotty_tasty::tasty::APPLY_TAG => Some("Apply"),
        dotty_tasty::tasty::BLOCK_TAG => Some("Block"),
        dotty_tasty::tasty::IF_TAG => Some("If"),
        dotty_tasty::tasty::LAMBDA_TAG => Some("Lambda"),
        dotty_tasty::tasty::MATCH_TAG => Some("Match"),
        dotty_tasty::tasty::NEW_TAG => Some("New"),
        dotty_tasty::tasty::RETURN_TAG => Some("Return"),
        dotty_tasty::tasty::TRY_TAG => Some("Try"),
        dotty_tasty::tasty::TYPEAPPLY_TAG => Some("TypeApply"),
        dotty_tasty::tasty::TYPED_TAG => Some("Typed"),
        dotty_tasty::tasty::WHILE_TAG => Some("WhileDo"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::load;
    use std::fs;

    #[test]
    fn loads_versioned_semantic_expectations() {
        let path = std::env::temp_dir().join(format!(
            "dotty-tasty-semantic-{}-{}.json",
            std::process::id(),
            "valid"
        ));
        fs::write(
            &path,
            r#"{"schema_version":3,"scala_version":"3.9.0","files":[]}"#,
        )
        .unwrap();

        let expectation = load(&path).unwrap();

        assert_eq!(expectation.schema_version, 3);
        assert_eq!(expectation.scala_version, "3.9.0");
        assert!(expectation.files.is_empty());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reports_invalid_semantic_expectations() {
        let path = std::env::temp_dir().join(format!(
            "dotty-tasty-semantic-{}-{}.json",
            std::process::id(),
            "invalid"
        ));
        fs::write(&path, b"not-json").unwrap();

        let error = load(&path).unwrap_err();

        assert!(error.contains("failed to parse semantic expectations"));
        fs::remove_file(path).unwrap();
    }
}
