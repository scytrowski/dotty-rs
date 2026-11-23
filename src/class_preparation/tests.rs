use super::*;

#[test]
fn an_unlocated_namer_failure_retains_its_original_file_and_payload() {
    let source = SourceId::from_index(47);
    let error = NamerError::RootIsNotPackage { tree_index: 3 };
    let diagnostic = PreparationDiagnostic::NamerFailure {
        source,
        error: error.clone(),
    };
    assert_eq!(diagnostic.source(), source);
    assert_eq!(diagnostic.location(), None);
    assert_eq!(diagnostic.code(), None);
    assert_eq!(
        diagnostic.render(),
        SourceDiagnostic::error(error.to_string(), None)
    );
    assert!(
        matches!(diagnostic, PreparationDiagnostic::NamerFailure {error: original, ..} if original == error)
    );
}
