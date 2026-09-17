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

type DeclarationKey = (String, String, Option<usize>);

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
        let declaration_keys = expected
            .declarations
            .iter()
            .map(|declaration| {
                (
                    declaration.kind.clone(),
                    declaration.name.clone(),
                    declaration.parameter_clauses,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            canonical_declarations(&declaration_keys).len(),
            declaration_keys.len(),
            "{} contains duplicate semantic-lite declarations",
            expected.path
        );
        assert!(
            expected.declarations.iter().all(|declaration| matches!(
                declaration.kind.as_str(),
                "DefDef" | "TypeDef" | "ValDef"
            )),
            "semantic expectation for {} contains an unknown declaration kind",
            expected.path
        );
        assert!(
            expected.declarations.iter().all(|declaration| {
                !declaration.name.is_empty()
                    && !declaration.name.contains('\0')
                    && declaration
                        .name
                        .chars()
                        .all(|character| character.is_alphanumeric() || character == '_')
            }),
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
        assert!(
            expected
                .shapes
                .keys()
                .all(|shape| is_known_semantic_shape(shape)),
            "semantic expectation for {} contains an unknown AST shape",
            expected.path
        );
        assert!(
            expected
                .parameter_clause_counts
                .keys()
                .all(|count| count.parse::<usize>().is_ok()),
            "semantic expectation for {} contains an invalid parameter-clause count",
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
        assert_expected_count_coverage(&expected.path, "shape", &expected.shapes, &actual_shapes);

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
        assert_expected_count_coverage(
            &expected.path,
            "parameter-clause",
            &expected.parameter_clause_counts,
            &actual_parameter_clause_counts,
        );

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
            !fixture.asts().is_empty(),
            "{} has no AST roots",
            expected.path
        );
    }
}

fn assert_expected_count_coverage(
    path: &str,
    label: &str,
    expected: &BTreeMap<String, usize>,
    actual: &BTreeMap<String, usize>,
) {
    let expected_positive = expected.values().filter(|count| **count > 0).count();
    let matched = expected
        .iter()
        .filter(|(key, expected_count)| {
            **expected_count > 0 && actual.get(*key).copied().unwrap_or_default() > 0
        })
        .count();
    assert!(
        expected_positive == 0 || matched > 0,
        "{path} has no wire-level {label} represented in the oracle; expected keys: {}",
        expected.keys().cloned().collect::<Vec<_>>().join(", ")
    );
}

fn canonical_declarations(declarations: &[DeclarationKey]) -> Vec<DeclarationKey> {
    let mut declarations = declarations.to_vec();
    declarations.sort();
    declarations.dedup();
    declarations
}

fn is_known_semantic_shape(shape: &str) -> bool {
    matches!(
        shape,
        "Apply"
            | "Block"
            | "If"
            | "Lambda"
            | "Match"
            | "New"
            | "Return"
            | "Try"
            | "TypeApply"
            | "Typed"
            | "WhileDo"
    )
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
    use super::{canonical_declarations, load};
    use std::fs;

    #[test]
    fn canonicalizes_declarations_in_generator_order_without_duplicates() {
        let declarations = vec![
            ("ValDef".to_owned(), "z".to_owned(), None),
            ("DefDef".to_owned(), "run".to_owned(), Some(1)),
            ("ValDef".to_owned(), "z".to_owned(), None),
        ];

        assert_eq!(
            canonical_declarations(&declarations),
            vec![
                ("DefDef".to_owned(), "run".to_owned(), Some(1)),
                ("ValDef".to_owned(), "z".to_owned(), None),
            ]
        );
    }

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
