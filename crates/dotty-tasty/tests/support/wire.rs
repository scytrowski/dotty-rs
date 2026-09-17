use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use dotty_tasty::tasty::{
    DEFDEF_TAG, DefDefHeaderItem, DefinitionNode, RawNode, TYPEDEF_TAG, VALDEF_TAG,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct WireExpectation {
    pub schema_version: u32,
    pub files: Vec<WireExpectationFile>,
}

#[derive(Debug, Deserialize)]
pub struct WireExpectationFile {
    pub path: String,
    pub tasty_format: String,
    pub ast_tag_counts: BTreeMap<String, usize>,
    pub definitions: Vec<WireDefinition>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WireDefinition {
    pub kind: String,
    pub name_ref: u32,
    pub name_kind: String,
    pub name: Option<String>,
    pub parameter_tags: Option<Vec<u8>>,
    pub clause_tags: Option<Vec<u8>>,
}

pub fn load(path: &Path) -> Result<WireExpectation, String> {
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "failed to read wire expectations {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "failed to parse wire expectations {}: {error}",
            path.display()
        )
    })
}

pub fn assert_expectations_match_selected_fixtures(
    corpus: &super::corpus::Corpus,
    expected_schema_version: u32,
    compatible_compiler_version: (u32, u32, u32),
) {
    let expectation_path = corpus
        .wire_expectation_path()
        .expect("corpus manifest must identify its wire expectations");
    let expectation = load(&expectation_path).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(expectation.schema_version, expected_schema_version);

    let fixture_root = corpus.tasty_root_path();
    let selected = corpus.selected_fixture_paths();
    let selected_paths = selected
        .iter()
        .map(|path| relative_path(path, &fixture_root))
        .collect::<BTreeSet<_>>();
    let expectation_paths = expectation
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(expectation.files.len(), expectation_paths.len());
    assert_eq!(expectation_paths, selected_paths);

    let parsed = super::corpus::parsed_fixtures(
        corpus,
        compatible_compiler_version.0,
        compatible_compiler_version.1,
        compatible_compiler_version.2,
    );
    let parsed_by_path = parsed
        .iter()
        .map(|fixture| (relative_path(&fixture.path, &fixture_root), fixture))
        .collect::<BTreeMap<_, _>>();

    for expected in expectation.files {
        let fixture = parsed_by_path
            .get(&expected.path)
            .unwrap_or_else(|| panic!("wire expectation names unknown fixture {}", expected.path));
        assert_file_matches(&expected, fixture);
    }
}

fn assert_file_matches(expected: &WireExpectationFile, fixture: &super::corpus::ParsedFixture) {
    let actual_format = format!(
        "{}.{}.{}",
        fixture.file.header().major_version,
        fixture.file.header().minor_version,
        fixture.file.header().experimental_version
    );
    assert_eq!(
        expected.tasty_format, actual_format,
        "wire baseline format mismatch in {}",
        expected.path
    );

    let mut actual_tags = BTreeMap::new();
    for node in fixture.index.iter_nodes() {
        *actual_tags.entry(node.tag.to_string()).or_insert(0) += 1;
    }
    assert_eq!(
        expected.ast_tag_counts, actual_tags,
        "wire AST tag counts mismatch in {}",
        expected.path
    );

    let actual_definitions = fixture
        .index
        .iter()
        .filter(|node| matches!(node.tag, VALDEF_TAG | DEFDEF_TAG | TYPEDEF_TAG))
        .map(|node| actual_definition(&fixture.file, node, expected.path.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        expected.definitions.len(),
        actual_definitions.len(),
        "wire definition count mismatch in {}",
        expected.path
    );
    for (index, (expected_definition, actual_definition)) in expected
        .definitions
        .iter()
        .zip(actual_definitions.iter())
        .enumerate()
    {
        assert_eq!(
            expected_definition, actual_definition,
            "wire definition mismatch in {} at definition index {}",
            expected.path, index
        );
    }
}

fn actual_definition(
    file: &dotty_tasty::tasty::TastyFile<'_>,
    node: &RawNode<'_>,
    path: &str,
) -> WireDefinition {
    let definition = node.decode_definition().unwrap_or_else(|error| {
        panic!(
            "failed to decode wire definition in {path} at offset {}: {error}",
            node.offset
        )
    });
    let (kind, name_ref) = match definition {
        DefinitionNode::ValDef { name, .. } => ("ValDef", name),
        DefinitionNode::DefDef { name, .. } => ("DefDef", name),
        DefinitionNode::TypeDef { name, .. } => ("TypeDef", name),
    };
    let name_entry = file
        .names()
        .entries()
        .get(name_ref as usize)
        .unwrap_or_else(|| {
            panic!(
                "wire definition in {path} at offset {} has invalid name reference {name_ref}",
                node.offset
            )
        });

    let (parameter_tags, clause_tags) = if node.tag == DEFDEF_TAG {
        let body = node.decode_defdef_body().unwrap_or_else(|error| {
            panic!(
                "failed to decode wire DefDef in {path} at offset {}: {error}",
                node.offset
            )
        });
        let mut parameters = Vec::new();
        let mut clauses = Vec::new();
        for item in body.header_items {
            match item {
                DefDefHeaderItem::Parameter(parameter) => parameters.push(parameter.tag()),
                DefDefHeaderItem::Clause(clause) => clauses.push(clause),
            }
        }
        (Some(parameters), Some(clauses))
    } else {
        (None, None)
    };

    WireDefinition {
        kind: kind.to_owned(),
        name_ref,
        name_kind: format!("{:?}", name_entry.kind()),
        name: name_entry.as_utf8().map(str::to_owned),
        parameter_tags,
        clause_tags,
    }
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or_else(|error| {
            panic!(
                "fixture {} is outside {}: {error}",
                path.display(),
                root.display()
            )
        })
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[cfg(test)]
mod tests {
    use super::WireDefinition;

    #[test]
    fn definition_comparison_includes_wire_parameter_structure() {
        let first = WireDefinition {
            kind: "DefDef".to_owned(),
            name_ref: 1,
            name_kind: "Utf8".to_owned(),
            name: Some("method".to_owned()),
            parameter_tags: Some(vec![134]),
            clause_tags: Some(vec![45]),
        };
        let mut second = first.clone();
        second.clause_tags = Some(vec![46]);
        assert_ne!(second, first);
    }
}
