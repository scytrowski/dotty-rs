//! Compilation-unit naming entry point and traversal seam.

use std::error::Error;
use std::fmt;

use dotty_core::ast::Select;
use dotty_core::{
    AstArena, Packages, ScopeId, SemanticStore, SourceId, SymbolId, SymbolOrigin, TreeId, TreeKind,
    Untyped,
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
#[derive(Clone, Debug)]
struct NamingContext {
    owner: SymbolId,
    scope: ScopeId,
    package_path: Vec<String>,
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
        _source_file_name: source_file_name,
        store,
        packages,
        root,
        index: SourceSemanticIndex::new(),
    };
    namer.index(root)?;
    Ok(namer.index)
}

struct Namer<'a> {
    arena: &'a AstArena<Untyped>,
    source: SourceId,
    _source_file_name: &'a str,
    store: &'a mut SemanticStore,
    packages: &'a mut Packages,
    root: TreeId<Untyped>,
    index: SourceSemanticIndex,
}

impl Namer<'_> {
    fn index(&mut self, tree: TreeId<Untyped>) -> Result<(), NamerError> {
        self.expand(tree, &[], true)
    }

    /// Desugaring hook. It is a no-op until source constructs need expansion.
    fn expand(
        &mut self,
        tree: TreeId<Untyped>,
        enclosing_package: &[String],
        source_root: bool,
    ) -> Result<(), NamerError> {
        self.index_expanded(tree, enclosing_package, source_root)
    }

    fn index_expanded(
        &mut self,
        tree: TreeId<Untyped>,
        enclosing_package: &[String],
        source_root: bool,
    ) -> Result<(), NamerError> {
        let TreeKind::PackageDef(package) = &self.arena.get(tree).kind else {
            if tree == self.root {
                return Err(NamerError::RootIsNotPackage {
                    tree_index: tree.index(),
                });
            }
            return Ok(());
        };

        let package = package.clone();
        let segments = if source_root && self.is_empty_package_sentinel(package.name) {
            Vec::new()
        } else {
            self.flatten_package_name(package.name)?
        };
        let mut package_path = enclosing_package.to_vec();
        package_path.extend(segments);

        let entered =
            self.packages
                .enter(self.store, SymbolOrigin::Source(self.source), &package_path);
        let Some(leaf) = entered.last().copied() else {
            return Err(NamerError::MalformedAstShape {
                tree_index: tree.index(),
                expected: "package registry path result",
            });
        };
        self.index.record_symbol(self.source, tree, leaf.symbol)?;
        self.index.record_scope(leaf.symbol, leaf.scope)?;

        let context = NamingContext {
            owner: leaf.symbol,
            scope: leaf.scope,
            package_path,
        };
        let _active_package = (context.owner, context.scope);
        for stat in package.stats {
            if matches!(self.arena.get(stat).kind, TreeKind::PackageDef(_)) {
                self.expand(stat, &context.package_path, false)?;
            }
        }
        Ok(())
    }

    fn is_empty_package_sentinel(&self, name: TreeId<Untyped>) -> bool {
        matches!(
            &self.arena.get(name).kind,
            TreeKind::Ident(ident) if self.store.names.resolve(ident.name.text()) == "<empty>"
        )
    }

    fn flatten_package_name(&self, tree: TreeId<Untyped>) -> Result<Vec<String>, NamerError> {
        let mut segments = Vec::new();
        self.flatten_package_name_into(tree, &mut segments)?;
        Ok(segments)
    }

    fn flatten_package_name_into(
        &self,
        tree: TreeId<Untyped>,
        segments: &mut Vec<String>,
    ) -> Result<(), NamerError> {
        match &self.arena.get(tree).kind {
            TreeKind::Ident(ident) => {
                segments.push(self.store.names.resolve(ident.name.text()).to_owned());
                Ok(())
            }
            TreeKind::Select(Select {
                qualifier, name, ..
            }) => {
                self.flatten_package_name_into(*qualifier, segments)?;
                segments.push(self.store.names.resolve(name.text()).to_owned());
                Ok(())
            }
            _ => Err(NamerError::MalformedAstShape {
                tree_index: tree.index(),
                expected: "Ident or qualified Select package name",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use dotty_core::ast::{Ident, Literal, PackageDef};
    use dotty_core::{
        AstArena, Packages, SemanticStore, SourceId, SymbolOrigin, Tree, TreeKind, Untyped,
    };

    use super::*;

    fn ident(
        arena: &mut AstArena<Untyped>,
        store: &mut SemanticStore,
        text: &str,
    ) -> TreeId<Untyped> {
        let name = store.names.intern(text);
        arena.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: *dotty_core::TermName::new(name).as_name(),
                backquoted: false,
            }),
            position: None,
            ty: (),
        })
    }

    fn select(
        arena: &mut AstArena<Untyped>,
        qualifier: TreeId<Untyped>,
        store: &mut SemanticStore,
        text: &str,
    ) -> TreeId<Untyped> {
        let name = store.names.intern(text);
        arena.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier,
                name: *dotty_core::TermName::new(name).as_name(),
                backquoted: false,
            }),
            position: None,
            ty: (),
        })
    }

    fn package(
        arena: &mut AstArena<Untyped>,
        name: TreeId<Untyped>,
        stats: Vec<TreeId<Untyped>>,
    ) -> TreeId<Untyped> {
        arena.alloc(Tree {
            kind: TreeKind::PackageDef(PackageDef { name, stats }),
            position: None,
            ty: (),
        })
    }

    fn name_package(
        arena: &AstArena<Untyped>,
        root: TreeId<Untyped>,
        source: u32,
        store: &mut SemanticStore,
        packages: &mut Packages,
    ) -> Result<SourceSemanticIndex, NamerError> {
        name_compilation_unit(
            arena,
            root,
            SourceId::from_index(source),
            "Example.scala",
            store,
            packages,
        )
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

        assert_eq!(
            name_compilation_unit(
                &arena,
                root,
                SourceId::from_index(1),
                "Bad.scala",
                &mut SemanticStore::new(),
                &mut Packages::new(),
            )
            .unwrap_err(),
            NamerError::RootIsNotPackage { tree_index: 0 }
        );
    }

    #[test]
    fn synthetic_empty_package_uses_root_and_never_creates_an_empty_segment() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let sentinel = ident(&mut arena, &mut store, "<empty>");
        let root = package(&mut arena, sentinel, vec![]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 1, &mut store, &mut packages).unwrap();
        let root_package = packages.get::<&str>(&[]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(1), root),
            Some(root_package.symbol)
        );
        assert_eq!(
            index.scope_of(root_package.symbol),
            Some(root_package.scope)
        );
        assert!(packages.get(&["<empty>"]).is_none());
        assert_eq!(
            store.symbols.get(root_package.symbol).origin,
            SymbolOrigin::Source(SourceId::from_index(1))
        );
    }

    #[test]
    fn qualified_package_name_enters_each_segment_and_maps_the_leaf() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let foo = ident(&mut arena, &mut store, "foo");
        let bar = select(&mut arena, foo, &mut store, "bar");
        let baz = select(&mut arena, bar, &mut store, "baz");
        let root = package(&mut arena, baz, vec![]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 2, &mut store, &mut packages).unwrap();
        let leaf = packages.get(&["foo", "bar", "baz"]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(2), root),
            Some(leaf.symbol)
        );
        assert_eq!(index.scope_of(leaf.symbol), Some(leaf.scope));
        assert!(packages.get(&["foo"]).is_some());
        assert!(packages.get(&["foo", "bar"]).is_some());
    }

    #[test]
    fn nested_package_clauses_are_relative_to_the_enclosing_package() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let foo_name = ident(&mut arena, &mut store, "foo");
        let bar_name = ident(&mut arena, &mut store, "bar");
        let nested = package(&mut arena, bar_name, vec![]);
        let root = package(&mut arena, foo_name, vec![nested]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 3, &mut store, &mut packages).unwrap();
        let foo = packages.get(&["foo"]).unwrap();
        let foo_bar = packages.get(&["foo", "bar"]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(3), root),
            Some(foo.symbol)
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(3), nested),
            Some(foo_bar.symbol)
        );
        assert_eq!(store.symbols.owner(foo_bar.symbol), Some(foo.symbol));
    }

    #[test]
    fn separate_source_units_reuse_the_same_package_symbol_and_scope() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();

        let mut first_arena = AstArena::<Untyped>::new();
        let first_name = ident(&mut first_arena, &mut store, "shared");
        let first_root = package(&mut first_arena, first_name, vec![]);
        let first = name_package(&first_arena, first_root, 4, &mut store, &mut packages).unwrap();

        let mut second_arena = AstArena::<Untyped>::new();
        let second_name = ident(&mut second_arena, &mut store, "shared");
        let second_root = package(&mut second_arena, second_name, vec![]);
        let second =
            name_package(&second_arena, second_root, 5, &mut store, &mut packages).unwrap();
        let package = packages.get(&["shared"]).unwrap();

        assert_eq!(
            first.symbol_at(SourceId::from_index(4), first_root),
            Some(package.symbol)
        );
        assert_eq!(
            second.symbol_at(SourceId::from_index(5), second_root),
            Some(package.symbol)
        );
        assert_eq!(first.scope_of(package.symbol), Some(package.scope));
        assert_eq!(second.scope_of(package.symbol), Some(package.scope));
    }

    #[test]
    fn repeated_package_path_in_one_source_reuses_the_registered_scope() {
        let mut store = SemanticStore::new();
        let mut arena = AstArena::<Untyped>::new();
        let outer_name = ident(&mut arena, &mut store, "outer");
        let first_inner_name = ident(&mut arena, &mut store, "inner");
        let second_inner_name = ident(&mut arena, &mut store, "inner");
        let first_inner = package(&mut arena, first_inner_name, vec![]);
        let second_inner = package(&mut arena, second_inner_name, vec![]);
        let root = package(&mut arena, outer_name, vec![first_inner, second_inner]);
        let mut packages = Packages::new();

        let index = name_package(&arena, root, 10, &mut store, &mut packages).unwrap();
        let inner = packages.get(&["outer", "inner"]).unwrap();

        assert_eq!(
            index.symbol_at(SourceId::from_index(10), first_inner),
            Some(inner.symbol)
        );
        assert_eq!(
            index.symbol_at(SourceId::from_index(10), second_inner),
            Some(inner.symbol)
        );
        assert_eq!(index.scope_of(inner.symbol), Some(inner.scope));
    }

    #[test]
    fn package_created_by_another_adapter_keeps_its_existing_origin() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let prior_origin = SymbolOrigin::Builtin;
        packages.enter(&mut store, prior_origin, &["existing"]);

        let mut arena = AstArena::<Untyped>::new();
        let name = ident(&mut arena, &mut store, "existing");
        let root = package(&mut arena, name, vec![]);
        let _index = name_package(&arena, root, 6, &mut store, &mut packages).unwrap();
        let package = packages.get(&["existing"]).unwrap();

        assert_eq!(store.symbols.get(package.symbol).origin, prior_origin);
    }

    #[test]
    fn malformed_package_name_shape_is_rejected() {
        let mut arena = AstArena::<Untyped>::new();
        let name = arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: dotty_core::Constant::Unit,
            }),
            position: None,
            ty: (),
        });
        let root = package(&mut arena, name, vec![]);

        assert_eq!(
            name_package(
                &arena,
                root,
                7,
                &mut SemanticStore::new(),
                &mut Packages::new(),
            )
            .unwrap_err(),
            NamerError::MalformedAstShape {
                tree_index: name.index(),
                expected: "Ident or qualified Select package name",
            }
        );
    }
}
