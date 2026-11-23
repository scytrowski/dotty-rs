//! Complete one source class header in its declaring unit.

use crate::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceDefinition, SourceSemanticIndex,
    SymbolId, SymbolKind, Typed, Untyped,
};
use crate::core::{ConflictingTypedTree, SourceId, SourceTypedIndex, TreeId};
use crate::source_declarations::named_class_template;
use crate::typer::{SignatureCompletion, SourceTyper, TyperError};

/// Source-to-typed bindings from successful class-header completions in one session.
#[derive(Debug, Default)]
pub struct ClassHeaderBindings {
    pub(crate) index: SourceTypedIndex,
}

impl ClassHeaderBindings {
    /// Retains all bindings from a successful attempt, including recursively
    /// completed headers. Continue typing into the arena returned by that typer.
    /// On a conflicting mapping, all previously recorded bindings are preserved.
    pub fn record(&mut self, index: SourceTypedIndex) -> Result<(), ConflictingTypedTree> {
        self.index.merge(index)
    }

    /// Looks up the exact typed node recorded for a tree in its source file.
    pub fn get(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.index.get(source, tree)
    }
}

/// Attempts one ordinary named class header, recording all typed parent applications
/// produced by the attempt (including recursively completed classes in this source).
/// Supply one session's arena and bindings throughout header completion and body typing.
/// Complete constructor signatures in their declaring units first. On `BlockedOn`,
/// complete the dependency in its own source, then retry. Both a blocked attempt
/// and an error preserve previous bindings and the arena.
#[allow(clippy::too_many_arguments)]
pub fn try_complete_class_header(
    class: SymbolId,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    arena: &mut AstArena<Typed>,
    bindings: &mut ClassHeaderBindings,
    in_batch: impl FnOnce(SymbolId) -> bool,
) -> Result<SignatureCompletion, TyperError> {
    if !store.symbols.contains(class) {
        return Err(TyperError::UnknownSymbol { symbol: class });
    }
    let Some(SourceDefinition::Canonical { source, tree }) = index.definition_of(class) else {
        return Err(TyperError::SourceProvenanceMissing { symbol: class });
    };
    let kind = store.symbols.get(class).kind;
    if kind != SymbolKind::Class {
        return Err(TyperError::SymbolSourceKindMismatch {
            source,
            tree_index: tree.index(),
            symbol: class,
            kind,
        });
    }
    if named_class_template(class, tree, ast, source, index, store).is_none() {
        return Err(TyperError::MalformedSourceAst {
            source,
            tree_index: tree.index(),
            symbol: class,
            kind,
        });
    }
    let typer = SourceTyper::new_with_typed_ast(
        ast,
        source,
        index,
        store,
        definitions,
        packages,
        std::mem::take(arena),
    );
    let (extended, result) =
        typer.try_complete_signature_into(class, in_batch, &mut bindings.index);
    *arena = extended;
    result
}
