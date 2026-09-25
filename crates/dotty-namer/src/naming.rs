//! Compilation-unit naming entry point and traversal seam.

use std::error::Error;
use std::fmt;

use dotty_core::{
    AstArena, Packages, ScopeId, SemanticStore, SourceId, SymbolId, TreeId, TreeKind, Untyped,
};

use crate::SourceSemanticIndex;

/// Internal structural error encountered while indexing a source tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamerError {
    /// A compilation-unit root must be a package definition.
    RootIsNotPackage { tree_index: u32 },
    /// A definition tree was assigned more than one semantic symbol.
    DuplicateSourceTreeSymbol { source: SourceId, tree_index: u32 },
    /// A semantic owner was assigned more than one declaration scope.
    DuplicateDeclarationScope { symbol: SymbolId },
    /// A tree did not have the shape required by a naming routine.
    MalformedAstShape {
        tree_index: u32,
        expected: &'static str,
    },
}

impl fmt::Display for NamerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootIsNotPackage { tree_index } => {
                write!(
                    f,
                    "compilation-unit root tree {tree_index} is not a PackageDef"
                )
            }
            Self::DuplicateSourceTreeSymbol { source, tree_index } => write!(
                f,
                "source {} tree {tree_index} already has a semantic symbol",
                source.index()
            ),
            Self::DuplicateDeclarationScope { symbol } => {
                write!(
                    f,
                    "symbol {} already has a declaration scope",
                    symbol.index()
                )
            }
            Self::MalformedAstShape {
                tree_index,
                expected,
            } => {
                write!(
                    f,
                    "tree {tree_index} does not have expected shape: {expected}"
                )
            }
        }
    }
}

impl Error for NamerError {}

/// Dynamic traversal state for naming declarations.
///
/// This is intentionally local to a naming traversal and is never stored in
/// [`SemanticStore`]. The first naming increment does not yet create owners
/// or declaration scopes, so it does not construct a context value.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
struct NamingContext {
    owner: SymbolId,
    scope: ScopeId,
}

/// Runs the source naming pass for one parsed compilation unit.
///
/// The input keeps the source ID and file name alongside arena-relative tree
/// IDs. The file name, including its extension, is retained by this API for
/// later top-level wrapper naming. This foundation increment validates the
/// package root but intentionally creates no symbols or scopes.
pub fn name_compilation_unit(
    arena: &AstArena<Untyped>,
    root: TreeId<Untyped>,
    source: SourceId,
    source_file_name: &str,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Result<SourceSemanticIndex, NamerError> {
    let mut namer = Namer {
        arena,
        source,
        source_file_name,
        store,
        packages,
        index: SourceSemanticIndex::new(),
    };
    namer.index(root)?;
    Ok(namer.index)
}

struct Namer<'a> {
    arena: &'a AstArena<Untyped>,
    source: SourceId,
    source_file_name: &'a str,
    store: &'a mut SemanticStore,
    packages: &'a mut Packages,
    index: SourceSemanticIndex,
}

impl Namer<'_> {
    fn index(&mut self, tree: TreeId<Untyped>) -> Result<(), NamerError> {
        self.expand(tree)
    }

    /// Desugaring hook. It is a no-op until source constructs need expansion.
    fn expand(&mut self, tree: TreeId<Untyped>) -> Result<(), NamerError> {
        self.index_expanded(tree)
    }

    fn index_expanded(&mut self, tree: TreeId<Untyped>) -> Result<(), NamerError> {
        if !matches!(self.arena.get(tree).kind, TreeKind::PackageDef(_)) {
            return Err(NamerError::RootIsNotPackage {
                tree_index: tree.index(),
            });
        }

        // Keep the full entry state available to subsequent naming routines.
        // None of it is persisted in SemanticStore in this foundation pass.
        let _ = (
            self.source,
            self.source_file_name,
            &mut self.store,
            &mut self.packages,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::ast::{Ident, Literal, PackageDef};
    use dotty_core::{AstArena, Packages, SemanticStore, SourceId, Tree, TreeKind, Untyped};

    use super::*;

    fn package_root(arena: &mut AstArena<Untyped>, store: &mut SemanticStore) -> TreeId<Untyped> {
        let name_id = store.names.intern("<empty>");
        let name = arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: *dotty_core::TermName::new(name_id).as_name(),
                backquoted: false,
            }),
            position: None,
            ty: (),
        });
        arena.alloc(Tree {
            kind: TreeKind::PackageDef(PackageDef {
                name,
                stats: vec![],
            }),
            position: None,
            ty: (),
        })
    }

    #[test]
    fn non_package_compilation_unit_root_is_rejected() {
        let mut arena = AstArena::<Untyped>::new();
        let root = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let error = name_compilation_unit(
            &arena,
            root,
            SourceId::from_index(1),
            "Bad.scala",
            &mut SemanticStore::new(),
            &mut Packages::new(),
        )
        .unwrap_err();

        assert_eq!(error, NamerError::RootIsNotPackage { tree_index: 0 });
    }

    #[test]
    fn empty_package_root_creates_no_symbols_or_scopes() {
        let mut arena = AstArena::<Untyped>::new();
        let mut store = SemanticStore::new();
        let root = package_root(&mut arena, &mut store);
        let before = store.checkpoint();
        let mut packages = Packages::new();

        let index = name_compilation_unit(
            &arena,
            root,
            SourceId::from_index(1),
            "Foo.scala",
            &mut store,
            &mut packages,
        )
        .expect("empty package should be accepted");

        assert!(index.symbol_at(SourceId::from_index(1), root).is_none());
        assert_eq!(store.checkpoint(), before);
        assert_eq!(packages.len(), 0);
    }
}
