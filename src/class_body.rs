//! Type the body of one ordinary named source class.

use std::fmt;

use crate::core::{
    AstArena, Definitions, NoResolver, Packages, SemanticStore, SourceSemanticIndex, SymbolId,
    SymbolResolver, Typed, Untyped,
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
    )
}

/// Checks each method and field initializer of a named class after signature completion.
/// Reports at most one error per member, in declaration order. An invalid class view
/// yields a single [`ClassBodyError::InvalidClass`] before typing begins.
/// Each failed member rolls back its typing attempt. Successful members can extend
/// the session arena and store. Only a completely checked class yields a body.
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
) -> Result<TypedClassBody, Vec<ClassBodyError>> {
    visit_class_body(
        class,
        ast,
        index,
        store,
        definitions,
        packages,
        arena,
        resolver,
        |first, rest| std::iter::once(first).chain(rest).collect(),
    )
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
    if let Some(first) = failures.next() {
        let errors = collect_errors(first, &mut failures);
        *arena = typer.into_typed_ast();
        return Err(errors);
    }
    let (extended, body) = TypedClassBody::from_output(class, typer.into_typed_output());
    *arena = extended;
    Ok(body)
}
