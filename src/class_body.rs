//! Type the body of one ordinary named source class.

use std::fmt;

use crate::TypingDiagnostic;
use crate::core::{
    AstArena, Definitions, NoResolver, Packages, SemanticStore, SourceId, SourceSemanticIndex,
    SourceTypedIndex, SymbolId, SymbolResolver, Typed, Untyped,
};
use crate::jvm_class_plan::TypedClassBody;
use crate::source_declarations::{ClassMember, SourceClass};
use crate::typer::{SourceTyper, TypedLocalBinding, TyperDiagnostic, TyperError};

/// Failure to type a named class or one of its declared members.
#[derive(Debug)]
pub enum ClassBodyError {
    /// The class or its body declarations disagree with the supplied naming inputs.
    InvalidClass(SymbolId),
    /// The typer rejected this field initializer or method definition.
    Member {
        /// Canonical class symbol associated with this plan or failure.
        class: SymbolId,
        /// Declaration or member rejected by the current completion or typing operation.
        member: ClassMember,
        /// Underlying typed failure reported by the delegated compiler phase.
        error: TyperError,
    },
}

impl ClassBodyError {
    /// Captures the original typer error, its declaring class and member, and a
    /// validated source span. For an invalid declaration, returns the original
    /// phase failure without consulting the AST. Use files from the naming session.
    pub fn into_reported_error<'a>(
        self,
        ast_for: impl FnMut(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Result<crate::ReportedTyperError, Self> {
        self.capture(ast_for).map_err(Self::InvalidClass)
    }

    /// Captures a fail-fast body rejection against its original named sources.
    /// Its class and member remain available if the source lookup has no AST.
    pub fn into_semantic_diagnostic(
        self,
        sources: &crate::NamedClassSources<'_>,
    ) -> crate::SemanticDiagnostic {
        let class = match &self {
            Self::InvalidClass(id) => *id,
            Self::Member { class, .. } => *class,
        };
        let issue = match self.capture(|id| sources.file(id).map(|file| file.ast())) {
            Ok(error) => crate::SemanticIssue::Typing(Box::new(error.with_named_sources(sources))),
            Err(id) => crate::SemanticIssue::InvalidClass(id),
        };
        crate::SemanticDiagnostic {
            phase: crate::SemanticPhase::ClassBody,
            owner_source: sources.declarations().get(class).map(|d| d.source),
            class: Some(class),
            issue,
        }
    }

    fn capture<'a>(
        self,
        ast_for: impl FnMut(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Result<crate::ReportedTyperError, SymbolId> {
        let (class, member, error) = match self {
            Self::Member {
                class,
                member,
                error,
            } => (class, member.symbol(), error),
            Self::InvalidClass(class) => return Err(class),
        };
        Ok(crate::ReportedTyperError::from_class(
            error,
            class,
            Some(member),
            ast_for,
        ))
    }

    /// Retains an owned call failure with its canonical declaring member. Supply the
    /// original untyped arena for the requested source, or `None` if unavailable.
    /// Returns the original phase error intact for every other failure.
    pub fn into_call_diagnostic<'a>(
        self,
        ast_for: impl FnOnce(SourceId) -> Option<&'a AstArena<Untyped>>,
    ) -> Result<crate::CallDiagnostic, Self> {
        let Self::Member {
            class,
            member,
            error,
        } = self
        else {
            return Err(self);
        };
        let (ClassMember::Field(decl) | ClassMember::Method(decl)) = member;
        crate::CallDiagnostic::from_typer(error, Some(decl.symbol), ast_for).map_err(|error| {
            Self::Member {
                class,
                member,
                error,
            }
        })
    }

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
        BodyVisit::FirstError,
    )
    .body
    .map_err(|errors| {
        errors
            .into_iter()
            .next()
            .expect("fail-fast visit returned a member or class error")
    })
}

/// Outcome and non-fatal diagnostics of one class body check.
#[must_use]
#[derive(Debug)]
pub struct ClassBodyCheck {
    /// A complete body, or the errors that prevented it from being fully typed.
    pub body: Result<TypedClassBody, Vec<ClassBodyError>>,
    /// Diagnostics emitted by committed typing operations, with their source file.
    /// Their ranges are byte offsets into that file, in emission order. Match
    /// the payload or use [`crate::SourceDiagnostic::from_typer_warning`] for presentation.
    pub diagnostics: Vec<(SourceId, TyperDiagnostic)>,
    class: SymbolId,
    partial: Option<PartialClassBody>,
}

/// Owned diagnostics from one check, independent of the retained semantic data.
#[derive(Debug)]
pub struct ClassBodyReport {
    class: SymbolId,
    /// One typed error per rejected member in declaration order, or one invalid class error.
    pub errors: Vec<ClassBodyError>,
    /// Committed typer diagnostics with their source IDs, in emission order.
    pub diagnostics: Vec<(SourceId, TyperDiagnostic)>,
}

impl ClassBodyReport {
    /// Moves committed warnings in emission order, then direct member errors in
    /// declaration order, into owned semantic diagnostics. Supply the named
    /// files from this check's session to capture error ranges. Missing files
    /// leave errors without a range. Skipped members produce no body diagnostic.
    /// Each yielded item can outlive the iterator and the source arenas.
    pub fn into_semantic_diagnostics(
        self,
        sources: &crate::NamedClassSources<'_>,
    ) -> impl Iterator<Item = crate::SemanticDiagnostic> {
        let class = self.class;
        let owner_source = sources.declarations().get(class).map(|d| d.source);
        let warnings = self
            .diagnostics
            .into_iter()
            .map(move |(source, diagnostic)| crate::SemanticDiagnostic {
                phase: crate::SemanticPhase::ClassBody,
                owner_source,
                class: Some(class),
                issue: crate::SemanticIssue::Warning { source, diagnostic },
            });
        warnings.chain(
            self.errors
                .into_iter()
                .map(move |error| error.into_semantic_diagnostic(sources)),
        )
    }
}

/// Committed mappings from a partially checked class, identified by its original symbol.
#[derive(Debug)]
pub struct PartialClassBody {
    locals: Vec<TypedLocalBinding>,
    class: SymbolId,
    index: SourceTypedIndex,
}

impl PartialClassBody {
    /// Mappings for successful members in this class's original source and typed arenas.
    pub fn bindings(&self) -> &SourceTypedIndex {
        &self.index
    }
}

/// Semantic data retained after a class check, for use with its original session arena.
#[derive(Debug)]
pub enum CheckedClassBody {
    /// All members succeeded. Move this body into planning or borrow its navigation bindings.
    Complete(TypedClassBody),
    /// A valid class had rejected or skipped members. Retains committed tree
    /// mappings and ordinary local bindings from successful members.
    Partial(PartialClassBody),
    /// Class validation failed before typing began for the requested symbol.
    Invalid(SymbolId),
}

impl CheckedClassBody {
    /// Original owner, or the requested symbol for an invalid check. IDs are session-local.
    pub fn class(&self) -> SymbolId {
        match self {
            Self::Complete(body) => body.class(),
            Self::Partial(body) => body.class,
            Self::Invalid(class) => *class,
        }
    }

    /// Canonical locals in committed members, in typed declaration order.
    /// Complete bodies include ordinary ValDef locals and case-local Bind
    /// declarations; partial bodies expose their retained ordinary locals.
    /// Source and typed trees, symbols and scopes belong to this session.
    /// A rejected class or a class without committed locals yields an empty slice.
    pub fn local_bindings(&self) -> &[TypedLocalBinding] {
        match self {
            Self::Complete(body) => body.local_bindings(),
            Self::Partial(body) => &body.locals,
            Self::Invalid(_) => &[],
        }
    }

    /// Borrows the committed mappings for direct-call navigation after diagnostics have
    /// been consumed. A rejected class has no mappings. Use the original source and typed arenas.
    pub fn bindings(&self) -> Option<&SourceTypedIndex> {
        match self {
            Self::Complete(body) => Some(body.bindings()),
            Self::Partial(body) => Some(body.bindings()),
            Self::Invalid(_) => None,
        }
    }
}

impl ClassBodyCheck {
    /// Moves semantic data and typed reports into independent values. This preserves
    /// successful siblings after a member error without cloning trees or rerunning the typer.
    /// Only [`CheckedClassBody::Complete`] supplies a body suitable for class planning.
    pub fn into_parts(self) -> (CheckedClassBody, ClassBodyReport) {
        let (body, errors) = match self.body {
            Ok(body) => (CheckedClassBody::Complete(body), Vec::new()),
            Err(errors) => (
                self.partial.map_or(
                    CheckedClassBody::Invalid(self.class),
                    CheckedClassBody::Partial,
                ),
                errors,
            ),
        };
        (
            body,
            ClassBodyReport {
                class: self.class,
                errors,
                diagnostics: self.diagnostics,
            },
        )
    }

    pub(crate) fn invalid(class: SymbolId) -> Self {
        Self {
            class,
            body: Err(vec![ClassBodyError::InvalidClass(class)]),
            diagnostics: Vec::new(),
            partial: None,
        }
    }

    /// Committed bindings for navigation, including successful siblings of a failed member.
    pub(crate) fn bindings(&self) -> Option<&SourceTypedIndex> {
        self.body
            .as_ref()
            .ok()
            .map(TypedClassBody::bindings)
            .or_else(|| self.partial.as_ref().map(PartialClassBody::bindings))
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
    visit_class_body(
        class,
        ast,
        index,
        store,
        definitions,
        packages,
        arena,
        resolver,
        BodyVisit::All,
    )
}

pub(crate) enum BodyVisit<'a> {
    FirstError,
    All,
    Ready(&'a crate::ClassBodyWork<'a>),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn visit_class_body(
    class: &SourceClass,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    arena: &mut AstArena<Typed>,
    resolver: impl SymbolResolver,
    visit: BodyVisit<'_>,
) -> ClassBodyCheck {
    let Some(source) = class.body_source(ast, index, store) else {
        return ClassBodyCheck::invalid(class.symbol);
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

    // Constructor fields have signatures but no initializer in the template body.
    let mut incomplete = matches!(visit, BodyVisit::Ready(work)
        if class.constructor_fields.iter().any(|id|work.skip_reason(*id).is_some()));
    let mut errors = Vec::new();
    for &member in &class.members {
        if matches!(visit, BodyVisit::Ready(work) if work.skip_reason(member.symbol()).is_some()) {
            incomplete = true;
            continue;
        }
        let result = match member {
            ClassMember::Field(field) => typer.type_field_initializer(field.symbol),
            ClassMember::Method(method) => typer.type_method_definition(method.tree),
        };
        if let Err(error) = result {
            errors.push(ClassBodyError::Member {
                class: class.symbol,
                member,
                error,
            });
            if matches!(visit, BodyVisit::FirstError) {
                break;
            }
        }
    }
    let diagnostics = if matches!(visit, BodyVisit::FirstError) {
        Vec::new()
    } else {
        typer
            .take_diagnostics()
            .into_iter()
            .map(|d| (source, d))
            .collect()
    };
    let mut partial = None;
    let body = if incomplete || !errors.is_empty() {
        if matches!(visit, BodyVisit::FirstError) {
            *arena = typer.into_typed_ast_and_index().0;
        } else {
            let output = typer.into_typed_output();
            *arena = output.typed_ast;
            partial = Some(PartialClassBody {
                class: class.symbol,
                index: output.source_typed_index,
                locals: output.local_bindings,
            });
        }
        Err(errors)
    } else {
        let (extended, body) = TypedClassBody::from_output(class, typer.into_typed_output());
        *arena = extended;
        Ok(body)
    };
    ClassBodyCheck {
        class: class.symbol,
        body,
        diagnostics,
        partial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DiagnosticSeverity, SourceText, TextRange};
    use crate::source_declarations::source_classes;
    use crate::{SourceDeclarationIndex, try_complete_member_signature};

    fn check_call_diagnostic(text: &str, selected: &str, code: &str, message: &str) {
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
            assert_eq!(diagnostic.code, Some(code));
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
            "typer.application_argument_type_mismatch",
            "Expected Int, found Boolean",
        );
    }

    #[test]
    fn arity_diagnostic_selects_the_whole_call() {
        check_call_diagnostic(
            "class A { def value(): Int = 7; def bad() = this.value(1) }",
            "this.value(1)",
            "typer.application_arity_mismatch",
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
