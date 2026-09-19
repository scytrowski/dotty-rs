//! Pass 2a: the identity of semantic types, and reference types.
//!
//! One type node address owns at most one `TypeId`. Decoding is lazy and
//! cache-first: [`type_at`](TastyUnpickler::type_at) returns the id already
//! recorded for an address, or decodes the node, allocates its type, and
//! records the address. It is address identity, not structural interning:
//! equal trees at different addresses get different ids.
//!
//! The forms decoded here are the ones whose target is named by address (or by
//! a package path already entered), so no reference is resolved by name:
//!
//! | TASTy                 | wire shape                | semantic type                      |
//! |-----------------------|---------------------------|------------------------------------|
//! | `TYPEREFdirect`       | `ASTRef`                  | `TypeRef { NoPrefix, symbol }`     |
//! | `TERMREFdirect`       | `ASTRef`                  | `TermRef { NoPrefix, symbol }`     |
//! | `TYPEREFsymbol`       | `ASTRef Type` (prefix)    | `TypeRef { prefix, symbol }`       |
//! | `TERMREFsymbol`       | `ASTRef Type` (prefix)    | `TermRef { prefix, symbol }`       |
//! | `TYPEREFpkg`          | `NameRef`                 | `TypeRef { NoPrefix, package }`    |
//! | `TERMREFpkg`          | `NameRef`                 | `TermRef { NoPrefix, package }`    |
//! | `THIS`                | `Type` (a class type ref) | `ThisType { class }`               |
//! | `SHAREDtype`          | `ASTRef`                  | the `TypeId` of the named node     |
//!
//! `THIS` is decoded only because it is the prefix of most real references.
//!
//! Everything else is `UnsupportedType`, including the name-based
//! `TYPEREF`/`TERMREF` (they look a member up by name in a prefix, which needs
//! the future resolver) and `TYPEREFin`/`TERMREFin`. Unsupported input is
//! never lowered to `NoType`, `NoPrefix` or `Error`.

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    RawTree, SHAREDTYPE_TAG, TERMREFDIRECT_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG, THIS_TAG,
    TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG, TermValue,
};

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::error::UnpickleError;
use crate::names::qualified_segments;
use crate::unpickler::TastyUnpickler;

/// The tag and absolute address of a tree's root node.
fn head(tree: &RawTree<'_>) -> (u8, u32) {
    match tree {
        RawTree::Leaf(term) => (term.tag, address(term.offset)),
        RawTree::Ast { tag, offset, .. } | RawTree::NatAst { tag, offset, .. } => {
            (*tag, address(*offset))
        }
        RawTree::LengthNode(node) => (node.tag, address(node.offset)),
    }
}

impl TastyUnpickler<'_, '_, '_> {
    /// The type of the node at `at`, which comes from an untrusted reference
    /// made by the node at `from`. `depth` counts the `SHAREDtype` links being
    /// followed, one inside another.
    ///
    /// Not atomic by itself: [`unpickle_type`](Self::unpickle_type) rolls back
    /// what a failed call allocated.
    pub(crate) fn type_at(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        from: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        if let Some(existing) = self.index.type_at(at) {
            return Ok(existing);
        }
        let tree = ast.tree_at(at, from)?;
        self.decode_type(ast, &tree, depth)
    }

    /// Decodes the type tree `tree`, whose offsets are absolute.
    fn decode_type(
        &mut self,
        ast: &AstView<'_>,
        tree: &RawTree<'_>,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let (tag, at) = head(tree);
        if let Some(existing) = self.index.type_at(at) {
            return Ok(existing);
        }

        let ty = match tree {
            RawTree::Leaf(term) => match (tag, &term.value) {
                (SHAREDTYPE_TAG, TermValue::AstRef(target)) => {
                    // An indirection, not a type: it allocates nothing and
                    // has no entry of its own.
                    if depth >= MAX_SHARED_DEPTH {
                        return Err(UnpickleError::InvalidReferenceTarget {
                            from: at,
                            to: *target,
                        });
                    }
                    return self.type_at(ast, *target, at, depth + 1);
                }
                (TYPEREFDIRECT_TAG, TermValue::AstRef(target)) => {
                    let symbol = self.referenced_symbol(ast, at, *target)?;
                    let prefix = self.store.types.alloc(Type::NoPrefix);
                    Type::TypeRef { prefix, symbol }
                }
                (TERMREFDIRECT_TAG, TermValue::AstRef(target)) => {
                    let symbol = self.referenced_symbol(ast, at, *target)?;
                    let prefix = self.store.types.alloc(Type::NoPrefix);
                    Type::TermRef { prefix, symbol }
                }
                (TYPEREFPKG_TAG, TermValue::NameRef(name)) => {
                    let symbol = self.referenced_package(at, *name)?;
                    let prefix = self.store.types.alloc(Type::NoPrefix);
                    Type::TypeRef { prefix, symbol }
                }
                (TERMREFPKG_TAG, TermValue::NameRef(name)) => {
                    let symbol = self.referenced_package(at, *name)?;
                    let prefix = self.store.types.alloc(Type::NoPrefix);
                    Type::TermRef { prefix, symbol }
                }
                _ => return Err(UnpickleError::UnsupportedType { tag, address: at }),
            },
            RawTree::NatAst {
                value: target,
                child,
                ..
            } if tag == TYPEREFSYMBOL_TAG || tag == TERMREFSYMBOL_TAG => {
                let prefix = self.decode_type(ast, child, depth)?;
                let symbol = self.referenced_symbol(ast, at, *target)?;
                if tag == TYPEREFSYMBOL_TAG {
                    Type::TypeRef { prefix, symbol }
                } else {
                    Type::TermRef { prefix, symbol }
                }
            }
            RawTree::Ast { child, .. } if tag == THIS_TAG => Type::ThisType {
                class: self.this_class(ast, child, depth)?,
            },
            _ => return Err(UnpickleError::UnsupportedType { tag, address: at }),
        };

        let id = self.store.types.alloc(ty);
        self.index.insert_type(at, id)?;
        Ok(id)
    }

    /// The symbol entered for the definition at `target`, named by the type
    /// node at `from`. Resolved by address only.
    fn referenced_symbol(
        &self,
        ast: &AstView<'_>,
        from: u32,
        target: u32,
    ) -> Result<SymbolId, UnpickleError> {
        if let Some(symbol) = self.index.symbol_at(target) {
            return Ok(symbol);
        }
        if ast.is_node(target) {
            Err(UnpickleError::MissingReferencedSymbol { from, to: target })
        } else {
            Err(UnpickleError::InvalidReferenceTarget { from, to: target })
        }
    }

    /// The package symbol for a `TYPEREFpkg` / `TERMREFpkg` naming `name`.
    ///
    /// Only a package already entered into the registry resolves; one that is
    /// not is an error, never created here, because a reference name is
    /// untrusted and resolving beyond the entered units is the resolver's job.
    fn referenced_package(&self, at: u32, name: u32) -> Result<SymbolId, UnpickleError> {
        let path = qualified_segments(self.file.names(), name)?;
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();
        self.packages
            .symbol(&segments)
            .ok_or_else(|| UnpickleError::UnresolvedPackage {
                address: at,
                package: path.join("."),
            })
    }

    /// The class named by the argument of a `THIS` node: a class reference by
    /// address or a package. Only its symbol is kept, so nothing is allocated
    /// for the argument itself.
    fn this_class(
        &self,
        ast: &AstView<'_>,
        class: &RawTree<'_>,
        depth: usize,
    ) -> Result<SymbolId, UnpickleError> {
        let (tag, at) = head(class);
        match class {
            RawTree::Leaf(term) => match (tag, &term.value) {
                (TYPEREFDIRECT_TAG, TermValue::AstRef(target)) => {
                    self.referenced_symbol(ast, at, *target)
                }
                (TYPEREFPKG_TAG, TermValue::NameRef(name)) => self.referenced_package(at, *name),
                (SHAREDTYPE_TAG, TermValue::AstRef(target)) => {
                    if depth >= MAX_SHARED_DEPTH {
                        return Err(UnpickleError::InvalidReferenceTarget {
                            from: at,
                            to: *target,
                        });
                    }
                    let shared = ast.tree_at(*target, at)?;
                    self.this_class(ast, &shared, depth + 1)
                }
                _ => Err(UnpickleError::UnsupportedType { tag, address: at }),
            },
            RawTree::NatAst { value, .. } if tag == TYPEREFSYMBOL_TAG => {
                self.referenced_symbol(ast, at, *value)
            }
            _ => Err(UnpickleError::UnsupportedType { tag, address: at }),
        }
    }
}
