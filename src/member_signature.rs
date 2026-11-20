//! One signature completion attempt for an indexed field or method.

use std::fmt;

use crate::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceDefinition, SourceSemanticIndex,
    SymbolId, SymbolKind, TreeKind, Untyped,
};
use crate::source_declarations::canonical_symbol;
use crate::typer::{SignatureCompletion, SourceTyper, TyperError};
use crate::{SourceDeclaration, SourceDeclarationIndex};

/// Failure to locate or type the requested member.
#[derive(Debug)]
pub enum MemberSignatureError {
    /// The symbol is outside the member batch or disagrees with the supplied naming inputs.
    InvalidMember(SymbolId),
    /// The typer rejected the member in its declaring class and source.
    Typing {
        member: SymbolId,
        declaration: SourceDeclaration,
        error: TyperError,
    },
}

impl fmt::Display for MemberSignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMember(id) => write!(f, "invalid source member {id:?}"),
            Self::Typing {
                member,
                declaration,
                error,
            } => {
                write!(
                    f,
                    "completing member {member:?} of {:?}: {error}",
                    declaration.class
                )
            }
        }
    }
}

impl std::error::Error for MemberSignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Typing { error, .. } => Some(error),
            Self::InvalidMember(_) => None,
        }
    }
}

/// Attempts one indexed field or method after header completion. Use the AST and
/// naming index from its declaring unit, including when its signature is cached.
/// All inputs belong to the same session. On `BlockedOn`, the caller can complete
/// the dependency in its own unit and retry. An unsuccessful attempt rolls back.
pub fn try_complete_member_signature(
    member: SymbolId,
    declarations: &SourceDeclarationIndex,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
) -> Result<SignatureCompletion, MemberSignatureError> {
    let invalid = || MemberSignatureError::InvalidMember(member);
    let declaration = declarations.member(member).ok_or_else(invalid)?;
    let Some(SourceDefinition::Canonical { source, tree }) = index.definition_of(member) else {
        return Err(invalid());
    };
    if source != declaration.source
        || canonical_symbol(store, index, source, tree, declaration.class) != Some(member)
    {
        return Err(invalid());
    }
    let node = ast.try_get(tree).ok_or_else(invalid)?;
    let name = match (&node.kind, declaration.kind) {
        (TreeKind::ValDef(field), SymbolKind::Field) => field.name,
        (TreeKind::DefDef(method), SymbolKind::Method) => method.name,
        _ => return Err(invalid()),
    };
    let symbol = store.symbols.get(member);
    if symbol.kind != declaration.kind
        || symbol.name != *name.as_name()
        || symbol.position != node.position
        || node.position.is_some_and(|pos| pos.source() != source)
    {
        return Err(invalid());
    }

    SourceTyper::new(ast, source, index, store, definitions, packages)
        .try_complete_signature(member, |dep| declarations.member(dep).is_some())
        .map_err(|error| MemberSignatureError::Typing {
            member,
            declaration,
            error,
        })
}
