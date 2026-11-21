//! Type the body of one ordinary named source class.

use std::fmt;

use crate::TypingDiagnostic;
use crate::core::{
    AstArena, Definitions, Diagnostic, NoResolver, Packages, SemanticStore, SourceId,
    SourceSemanticIndex, SourceTypedIndex, SymbolId, SymbolResolver, Typed, Untyped,
};
use crate::jvm_class_plan::TypedClassBody;
use crate::source_declarations::{ClassMember, SourceClass};
use crate::typer::{SourceTyper, TyperError};

/// Failure to type a named class or one of its declared members.
#[derive(Debug)]
pub enum ClassBodyError {
    /// The class or its body declarations disagree with the supplied naming inputs.
    InvalidClass(SymbolId),
    /// The typer rejected this field initializer or method definition.
    Member {
        class: SymbolId,
        member: ClassMember,
        error: TyperError,
    },
}

impl ClassBodyError {
    /// Formats this failure after typing, including after a member rollback.
    /// Supply the original untyped arena for the requested source, or `None` if unavailable.
    /// Location follows [`crate::typer_error_span_with`]. An invalid class has no source site.
    pub fn diagnostic<'a>(
        &self,
        ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> TypingDiagnostic {
        match self {
            Self::InvalidClass(_) => TypingDiagnostic::error(self.to_string(), None),
            Self::Member { error, .. } => TypingDiagnostic::from_typer(error, ast_for),
        }
    }
}

impl fmt::Display for ClassBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClass(class) => write!(f, "invalid source class body for {class:?}"),
            Self::Member {
                class,
                member,
                error,
            } => write!(f, "typing {member:?} of {class:?}: {error}"),
        }
    }
}

impl std::error::Error for ClassBodyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Member { error, .. } => Some(error),
            Self::InvalidClass(_) => None,
        }
    }
}

/// Types every field initializer and method of a canonical named class in declaration order.
/// All inputs belong to one session. The arena stays available on either outcome.
/// On a member error, earlier successful members may have extended the arena and semantic
/// store. Only a fully typed class yields a body for [`crate::jvm_class_plan::TypedClassBodies`].
pub fn type_class_body(
    class: &SourceClass,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    arena: &mut AstArena<Typed>,
) -> Result<TypedClassBody, ClassBodyError> {
    type_class_body_with_resolver(
        class,
        ast,
        index,
        store,
        definitions,
        packages,
        arena,
        NoResolver,
    )
}

/// Types the same members as [`type_class_body`] with a session's external symbol resolver.
/// Borrow the same resolver for successive classes. A failed member restores its resolver
/// checkpoint. Earlier successful members retain their state and typed trees.
#[allow(clippy::too_many_arguments)]
pub fn type_class_body_with_resolver(
    class: &SourceClass,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    arena: &mut AstArena<Typed>,
    resolver: impl SymbolResolver,
) -> Result<TypedClassBody, ClassBodyError> {
    visit_class_body(
        class,
        ast,
        index,
        store,
        definitions,
        packages,
        arena,
        resolver,
        |first, _| first,
        None,
        None,
    )
}

/// Outcome and non-fatal diagnostics of one class body check.
#[must_use]
#[derive(Debug)]
pub struct ClassBodyCheck {
    /// A complete body, or the errors that prevented it from being fully typed.
    pub body: Result<TypedClassBody, Vec<ClassBodyError>>,
    /// Diagnostics emitted by committed typing operations, with their source file.
    /// Their ranges are byte offsets into that file, in emission order.
    pub diagnostics: Vec<(SourceId, Diagnostic)>,
    partial_bindings: Option<SourceTypedIndex>,
}

impl ClassBodyCheck {
    /// Committed bindings for navigation, including successful siblings of a failed member.
    pub(crate) fn bindings(&self) -> Option<&SourceTypedIndex> {
        self.body
            .as_ref()
            .ok()
            .map(TypedClassBody::bindings)
            .or(self.partial_bindings.as_ref())
    }
}

/// Checks each method and field initializer of a named class after signature completion.
/// Reports at most one error per member, in declaration order. An invalid class view
/// yields a single [`ClassBodyError::InvalidClass`] before typing begins.
/// Each failed member rolls back its typing attempt. Successful members can extend
/// the session arena and store. Only a completely checked class yields a body.
/// Diagnostics from successful members are available on either outcome in
/// [`ClassBodyCheck::diagnostics`]. Failed attempts discard their own diagnostics.
/// Pass [`NoResolver`] for source-only checking, or borrow a session resolver.
#[allow(clippy::too_many_arguments)]
pub fn check_class_body(
    class: &SourceClass,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    arena: &mut AstArena<Typed>,
    resolver: impl SymbolResolver,
) -> ClassBodyCheck {
    let mut diagnostics = Vec::new();
    let mut partial_bindings = None;
    let body = visit_class_body(
        class,
        ast,
        index,
        store,
        definitions,
        packages,
        arena,
        resolver,
        |first, rest| std::iter::once(first).chain(rest).collect(),
        Some(&mut diagnostics),
        Some(&mut partial_bindings),
    );
    ClassBodyCheck {
        body,
        diagnostics,
        partial_bindings,
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_class_body<E>(
    class: &SourceClass,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    arena: &mut AstArena<Typed>,
    resolver: impl SymbolResolver,
    collect_errors: impl FnOnce(ClassBodyError, &mut dyn Iterator<Item = ClassBodyError>) -> E,
    diagnostics: Option<&mut Vec<(SourceId, Diagnostic)>>,
    partial_bindings: Option<&mut Option<SourceTypedIndex>>,
) -> Result<TypedClassBody, E> {
    let Some(source) = class.body_source(ast, index, store) else {
        return Err(collect_errors(
            ClassBodyError::InvalidClass(class.symbol),
            &mut std::iter::empty(),
        ));
    };
    let mut typer = SourceTyper::new_with_typed_ast(
        ast,
        source,
        index,
        store,
        definitions,
        packages,
        std::mem::take(arena),
    )
    .with_resolver(Box::new(resolver));
    // Advancing this iterator types the next members. The fail-fast caller stops
    // at the first failure. The diagnostic caller consumes the rest in source order.
    let mut failures = class.members.iter().filter_map(|&member| {
        let result = match member {
            ClassMember::Field(field) => typer.type_field_initializer(field.symbol),
            ClassMember::Method(method) => typer.type_method_definition(method.tree),
        };
        result.err().map(|error| ClassBodyError::Member {
            class: class.symbol,
            member,
            error,
        })
    });
    let errors = failures
        .next()
        .map(|first| collect_errors(first, &mut failures));
    if let Some(diagnostics) = diagnostics {
        diagnostics.extend(
            typer
                .take_diagnostics()
                .into_iter()
                .map(|item| (source, item)),
        );
    }
    if let Some(errors) = errors {
        if let Some(bindings) = partial_bindings {
            let output = typer.into_typed_output();
            *bindings = Some(output.source_typed_index);
            *arena = output.typed_ast;
        } else {
            *arena = typer.into_typed_ast();
        }
        return Err(errors);
    }
    let (extended, body) = TypedClassBody::from_output(class, typer.into_typed_output());
    *arena = extended;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DiagnosticSeverity, SourceText, TextRange};
    use crate::source_declarations::source_classes;
    use crate::{SourceDeclarationIndex, try_complete_member_signature};

    fn check_call_diagnostic(text: &str, selected: &str, message: &str) {
        let source = SourceId::from_index(7);
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let parse = |source, store: &mut SemanticStore| {
            crate::parser::parse_compilation_unit(
                SourceText::new(text).unwrap(),
                source,
                crate::lexer::ContextualScanner::new(text).unwrap(),
                &mut store.names,
            )
        };
        let parsed = parse(source, &mut store);
        let foreign = parse(SourceId::from_index(8), &mut store);
        let index = crate::namer::name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "A.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let classes = source_classes(&parsed.ast, parsed.root, source, &index, &store).unwrap();
        let class = &classes[0];
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
        typer.complete_symbol(class.constructor).unwrap();
        typer.complete_symbol(class.symbol).unwrap();
        let mut methods = class.methods();
        typer
            .complete_symbol(methods.next().unwrap().symbol)
            .unwrap();
        let bad = methods.next().unwrap().symbol;
        drop(typer);
        let declarations = SourceDeclarationIndex::from_classes(&store, [class]).unwrap();
        let signature = try_complete_member_signature(
            bad,
            &declarations,
            &parsed.ast,
            &index,
            &mut store,
            defs,
            &packages,
        )
        .unwrap_err();
        let failure = type_class_body(
            class,
            &parsed.ast,
            &index,
            &mut store,
            defs,
            &packages,
            &mut AstArena::new(),
        )
        .unwrap_err();
        let start = text.rfind(selected).unwrap() as u32;
        for (ast, expected) in [
            (
                Some(&parsed.ast),
                Some((
                    source,
                    TextRange::new(start, start + selected.len() as u32).unwrap(),
                )),
            ),
            (Some(&foreign.ast), None),
            (None, None),
        ] {
            let diagnostic = failure.diagnostic(|id| {
                assert_eq!(id, source);
                ast
            });
            assert_eq!(signature.diagnostic(|_| ast), diagnostic);
            let _: &crate::SourceDiagnostic = &diagnostic;
            assert_eq!(diagnostic.code, None);
            assert_eq!(diagnostic.message, message);
            assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
            assert_eq!(
                diagnostic.location.map(|p| (p.source(), p.span().range())),
                expected
            );
        }
    }

    #[test]
    fn argument_diagnostic_keeps_its_message_with_or_without_the_source_tree() {
        check_call_diagnostic(
            "class A { def only(n: Int): Int = n; def bad() = only(false) }",
            "false",
            "Expected Int, found Boolean",
        );
    }

    #[test]
    fn arity_diagnostic_selects_the_whole_call() {
        check_call_diagnostic(
            "class A { def value(): Int = 7; def bad() = this.value(1) }",
            "this.value(1)",
            "Expected 0 arguments, got 1",
        );
    }

    #[test]
    fn invalid_class_reports_its_error_without_looking_up_a_source_tree() {
        let defs = Definitions::bootstrap(&mut SemanticStore::new());
        let invalid = ClassBodyError::InvalidClass(defs.object_class);
        let diagnostic = invalid.diagnostic(|_| panic!("invalid class has no source site"));
        assert_eq!(diagnostic.message, invalid.to_string());
        assert_eq!(diagnostic.location, None);
    }
}
