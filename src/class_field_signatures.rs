//! Signature completion for fields exposed by an ordinary named source class.

use std::fmt;

use crate::TypingDiagnostic;
use crate::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SymbolId,
    SymbolInfo, Untyped,
};
use crate::source_declarations::signature_fields;
use crate::typer::{SignatureCompletion, SourceTyper, TyperError};

/// Outcome of one pass over a class's field signatures.
#[must_use = "complete a blocking dependency in its owning source before retrying"]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassFieldSignatureCompletion {
    /// All selected field types are complete in the session store.
    Complete,
    /// This field needs the signature of a method or public field in another source of the batch.
    BlockedOn {
        field: SymbolId,
        dependency: SymbolId,
    },
}

/// Failure to identify a class or complete one of its field signatures.
#[derive(Debug)]
pub enum ClassFieldSignatureError {
    /// The supplied naming inputs do not identify a supported ordinary source class.
    InvalidClass(SymbolId),
    /// The typer rejected this field in its declaring class.
    Field {
        class: SymbolId,
        field: SymbolId,
        error: TyperError,
    },
}

impl ClassFieldSignatureError {
    /// Reports a field failure using [`TypingDiagnostic::from_typer`] and the original
    /// untyped arena for its source. An invalid class has no source location.
    pub fn diagnostic<'a>(
        &self,
        ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> TypingDiagnostic {
        match self {
            Self::InvalidClass(_) => TypingDiagnostic::error(self.to_string(), None),
            Self::Field { error, .. } => TypingDiagnostic::from_typer(error, ast_for),
        }
    }
}

impl fmt::Display for ClassFieldSignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClass(class) => write!(f, "invalid named source class {class:?}"),
            Self::Field {
                class,
                field,
                error,
            } => write!(f, "completing field {field:?} of {class:?}: {error}"),
        }
    }
}

impl std::error::Error for ClassFieldSignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Field { error, .. } => Some(error),
            Self::InvalidClass(_) => None,
        }
    }
}

/// After constructor and class-header completion, completes the class-owned constructor
/// fields followed by public body fields, in source order. All inputs belong to one session.
/// Earlier completed fields retain their types if a later field blocks or fails.
/// The caller schedules dependencies accepted by `in_batch` and retries the class.
/// Use [`crate::type_class_body`] to retain typed initializers for planning.
pub fn try_complete_class_field_signatures(
    class: SymbolId,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    mut in_batch: impl FnMut(SymbolId) -> bool,
) -> Result<ClassFieldSignatureCompletion, ClassFieldSignatureError> {
    let (source, fields) = signature_fields(class, ast, index, store)
        .ok_or(ClassFieldSignatureError::InvalidClass(class))?;
    if fields.is_empty() {
        return Ok(ClassFieldSignatureCompletion::Complete);
    }
    let mut typer = SourceTyper::new(ast, source, index, store, definitions, packages);
    for field in fields {
        if matches!(typer.store().symbols.info(field), SymbolInfo::Complete(_)) {
            continue;
        }
        let completion = typer
            .try_complete_signature(field, &mut in_batch)
            .map_err(|error| ClassFieldSignatureError::Field {
                class,
                field,
                error,
            })?;
        if let SignatureCompletion::BlockedOn(dependency) = completion {
            return Ok(ClassFieldSignatureCompletion::BlockedOn { field, dependency });
        }
    }
    Ok(ClassFieldSignatureCompletion::Complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::DiagnosticSeverity;

    #[test]
    fn invalid_class_reports_an_unlocated_error_without_an_ast_lookup() {
        let defs = Definitions::bootstrap(&mut SemanticStore::new());
        let failure = ClassFieldSignatureError::InvalidClass(defs.object_class);
        let report = failure.diagnostic(|_| panic!("invalid class has no source site"));
        assert_eq!(report.severity, DiagnosticSeverity::Error);
        assert_eq!(report.message, failure.to_string());
        assert_eq!(report.location, None);
    }
}
