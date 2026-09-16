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
