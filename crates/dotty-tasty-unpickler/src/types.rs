//! Semantic types: identity, references, name resolution, and compound types.
//!
//! One type node address owns at most one `TypeId`. Decoding is lazy and
//! cache-first: [`type_at`](TastyUnpickler::type_at) returns the id already
//! recorded for an address, or decodes the node, allocates its type, and
//! records the address. It is address identity, not structural interning:
//! equal trees at different addresses get different ids.
//!
//! ## Reference forms
//!
//! | TASTy                 | wire shape                | semantic type                      |
//! |-----------------------|---------------------------|------------------------------------|
//! | `TYPEREFdirect`       | `ASTRef`                  | `TypeRef { no_prefix, symbol }`    |
//! | `TERMREFdirect`       | `ASTRef`                  | `TermRef { no_prefix, symbol }`    |
//! | `TYPEREFsymbol`       | `ASTRef Type` (prefix)    | `TypeRef { prefix, symbol }`       |
//! | `TERMREFsymbol`       | `ASTRef Type` (prefix)    | `TermRef { prefix, symbol }`       |
//! | `TYPEREFpkg`          | `NameRef`                 | `TypeRef { no_prefix, package }`   |
//! | `TERMREFpkg`          | `NameRef`                 | `TermRef { no_prefix, package }`   |
//! | `TYPEREF`             | `NameRef Type` (prefix)   | `TypeRef { prefix, member }`       |
//! | `TERMREF`             | `NameRef Type` (prefix)   | `TermRef { prefix, member }`       |
//! | `THIS`                | `Type` (a class type ref) | `ThisType { class }`               |
//! | `SHAREDtype`          | `ASTRef`                  | the `TypeId` of the named node     |
//!
//! ## Name resolution
//!
//! `TYPEREF` and `TERMREF` name a member of their prefix rather than a
//! definition. The member is looked up in the prefix's own declaration scope
//! (an entered class, an object through its module class, a package), and
//! otherwise asked of the session's
//! [`SymbolResolver`](dotty_core::resolution::SymbolResolver). Nothing is
//! searched by text across owners, and a signed term reference or several
//! overloads is an explicit error, not a guess.
//!
//! ## Compound types
//!
//! | TASTy                 | wire shape                | semantic type                      |
//! |-----------------------|---------------------------|------------------------------------|
//! | `APPLIEDtype`         | `Type Type*`              | `Applied { tycon, args }`          |
//!
//! Every child is decoded through the same entry point as a top-level type, so
//! it is cached, shared and resolved like any other, and a child's error is
//! the error of the whole node. Compound nodes are not interned: equal trees
//! at different addresses keep different ids.
//!
//! Every other form is `UnsupportedType`: `TYPEBOUNDS`, `ANNOTATEDtype`,
//! `TYPELAMBDAtype`, `FLEXIBLEtype`, constants, method/poly/param types,
//! refinements, recursive and match types, and `TYPEREFin`/`TERMREFin`.
//! Unsupported input is never lowered to `NoType`, `NoPrefix` or `Error`.

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::resolution::{MemberRequest, MemberSelector, ResolutionError};
use dotty_core::symbols::SymbolKind;
use dotty_core::types::Type;
use dotty_tasty::tasty::{
    APPLIEDTYPE_TAG, RawTree, SHAREDTYPE_TAG, TERMREF_TAG, TERMREFDIRECT_TAG, TERMREFPKG_TAG,
    TERMREFSYMBOL_TAG, THIS_TAG, TYPEREF_TAG, TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG,
    TermValue,
};

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::error::UnpickleError;
use crate::lookup::{LocalLookup, lookup_member};
use crate::names::{is_signed, package_segments, wire_name};
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
                    let symbol = self.referenced_symbol(ast, at, *target, Namespace::Type)?;
                    let prefix = self.definitions.no_prefix;
                    Type::TypeRef { prefix, symbol }
                }
                (TERMREFDIRECT_TAG, TermValue::AstRef(target)) => {
                    let symbol = self.referenced_symbol(ast, at, *target, Namespace::Term)?;
                    let prefix = self.definitions.no_prefix;
                    Type::TermRef { prefix, symbol }
                }
                (TYPEREFPKG_TAG, TermValue::NameRef(name)) => {
                    let symbol = self.referenced_package(at, *name)?;
                    let prefix = self.definitions.no_prefix;
                    Type::TypeRef { prefix, symbol }
                }
                (TERMREFPKG_TAG, TermValue::NameRef(name)) => {
                    let symbol = self.referenced_package(at, *name)?;
                    let prefix = self.definitions.no_prefix;
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
                let namespace = if tag == TYPEREFSYMBOL_TAG {
                    Namespace::Type
                } else {
                    Namespace::Term
                };
                let symbol = self.referenced_symbol(ast, at, *target, namespace)?;
                if tag == TYPEREFSYMBOL_TAG {
                    Type::TypeRef { prefix, symbol }
                } else {
                    Type::TermRef { prefix, symbol }
                }
            }
            RawTree::NatAst {
                value: name, child, ..
            } if tag == TYPEREF_TAG || tag == TERMREF_TAG => {
                let prefix = self.decode_type(ast, child, depth)?;
                let namespace = if tag == TYPEREF_TAG {
                    Namespace::Type
                } else {
                    Namespace::Term
                };
                let symbol = self.resolved_member(at, *name, prefix, namespace)?;
                if tag == TYPEREF_TAG {
                    Type::TypeRef { prefix, symbol }
                } else {
                    Type::TermRef { prefix, symbol }
                }
            }
            RawTree::Ast { child, .. } if tag == THIS_TAG => Type::ThisType {
                class: self.this_class(ast, child, depth)?,
            },
            RawTree::LengthNode(node) if tag == APPLIEDTYPE_TAG => {
                let applied = node.decode_applied_type()?;
                let tycon = self.decode_type(ast, &applied.tycon, depth)?;
                let args = self.decode_types(ast, &applied.arguments, depth)?;
                Type::Applied { tycon, args }
            }
            _ => return Err(UnpickleError::UnsupportedType { tag, address: at }),
        };

        let id = self.store.types.alloc(ty);
        self.index.insert_type(at, id)?;
        Ok(id)
    }

    /// Decodes `trees` in wire order, each through [`decode_type`](Self::decode_type),
    /// so a child is cached, shared and resolved like any other type.
    fn decode_types(
        &mut self,
        ast: &AstView<'_>,
        trees: &[RawTree<'_>],
        depth: usize,
    ) -> Result<Vec<TypeId>, UnpickleError> {
        trees
            .iter()
            .map(|tree| self.decode_type(ast, tree, depth))
            .collect()
    }

    /// The member of `prefix` a name-based `TYPEREF` / `TERMREF` at `at`
    /// means: the prefix's own declaration scope first, then the resolver.
    /// Never a search by text across owners, never the first of several
    /// overloads.
    fn resolved_member(
        &mut self,
        at: u32,
        name_ref: u32,
        prefix: TypeId,
        namespace: Namespace,
    ) -> Result<SymbolId, UnpickleError> {
        let text = wire_name(self.file.names(), name_ref)?;
        if namespace == Namespace::Term && is_signed(self.file.names(), name_ref) {
            return Err(UnpickleError::UnsupportedSignedReference {
                address: at,
                name: text,
            });
        }
        let name = Name::new(self.store.names.intern(&text), namespace);

        let local = lookup_member(self.store, &self.index, &self.packages, prefix, &name);
        let unsupported_prefix = local == LocalLookup::UnsupportedPrefix;
        match local {
            LocalLookup::Found(symbol) => return Ok(symbol),
            LocalLookup::Ambiguous { candidates } => {
                return Err(UnpickleError::AmbiguousMember {
                    address: at,
                    prefix,
                    name: text,
                    candidates,
                });
            }
            LocalLookup::NotFound | LocalLookup::ScopeUnknown | LocalLookup::UnsupportedPrefix => {}
        }

        let request = MemberRequest {
            prefix,
            name,
            selector: MemberSelector::Unique,
        };
        let failure = |error| UnpickleError::ResolverFailure { address: at, error };
        match self
            .resolver
            .resolve_member(&*self.store, &request)
            .map_err(failure)?
        {
            Some(symbol) if self.store.symbols.get(symbol).name.namespace() == namespace => {
                Ok(symbol)
            }
            Some(_) => Err(failure(ResolutionError::Malformed {
                reason: format!("the symbol for `{text}` is in the wrong namespace"),
            })),
            None if unsupported_prefix => Err(UnpickleError::UnsupportedResolutionPrefix {
                address: at,
                prefix,
            }),
            None => Err(UnpickleError::UnresolvedMember {
                address: at,
                prefix,
                name: text,
                namespace,
            }),
        }
    }

    /// The symbol entered for the definition at `target`, named by the type
    /// node at `from`, which must be in the `expected` namespace. Resolved by
    /// address only.
    fn referenced_symbol(
        &self,
        ast: &AstView<'_>,
        from: u32,
        target: u32,
        expected: Namespace,
    ) -> Result<SymbolId, UnpickleError> {
        if let Some(symbol) = self.index.symbol_at(target) {
            // The index only says a symbol exists: a type reference to a
            // `val`, or a term reference to a `class`, would build an
            // inconsistent type from untrusted input.
            if self.store.symbols.get(symbol).name.namespace() != expected {
                return Err(UnpickleError::InvalidReferenceKind { from, to: target });
            }
            return Ok(symbol);
        }
        if ast.is_node(target) {
            Err(UnpickleError::MissingReferencedSymbol { from, to: target })
        } else {
            Err(UnpickleError::InvalidReferenceTarget { from, to: target })
        }
    }

    /// Like [`referenced_symbol`](Self::referenced_symbol), for the argument of
    /// `THIS`, which must be a class, trait or module class.
    fn referenced_class(
        &self,
        ast: &AstView<'_>,
        from: u32,
        target: u32,
    ) -> Result<SymbolId, UnpickleError> {
        let symbol = self.referenced_symbol(ast, from, target, Namespace::Type)?;
        match self.store.symbols.get(symbol).kind {
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => Ok(symbol),
            _ => Err(UnpickleError::InvalidReferenceKind { from, to: target }),
        }
    }

    /// The package symbol for a `TYPEREFpkg` / `TERMREFpkg` naming `name`.
    ///
    /// A package in the session's registry resolves. Any other is asked of the
    /// resolver, and is an error if it does not know it: a package is never
    /// created here, because a reference name is untrusted.
    fn referenced_package(&mut self, at: u32, name: u32) -> Result<SymbolId, UnpickleError> {
        let path = package_segments(self.file.names(), name)?;
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();
        if let Some(symbol) = self.packages.symbol(&segments) {
            return Ok(symbol);
        }
        let unresolved = || UnpickleError::UnresolvedPackage {
            address: at,
            package: path.join("."),
        };
        let failure = |error| UnpickleError::ResolverFailure { address: at, error };
        match self
            .resolver
            .resolve_package(&*self.store, &segments)
            .map_err(failure)?
        {
            Some(symbol) if self.store.symbols.get(symbol).kind == SymbolKind::Package => {
                Ok(symbol)
            }
            Some(_) => Err(failure(ResolutionError::Malformed {
                reason: format!(
                    "the symbol for package `{}` is not a package",
                    path.join(".")
                ),
            })),
            None => Err(unresolved()),
        }
    }

    /// The class named by the argument of a `THIS` node: a class reference by
    /// address or a package. Only its symbol is kept, so nothing is allocated
    /// for the argument itself.
    fn this_class(
        &mut self,
        ast: &AstView<'_>,
        class: &RawTree<'_>,
        depth: usize,
    ) -> Result<SymbolId, UnpickleError> {
        let (tag, at) = head(class);
        match class {
            RawTree::Leaf(term) => match (tag, &term.value) {
                (TYPEREFDIRECT_TAG, TermValue::AstRef(target)) => {
                    self.referenced_class(ast, at, *target)
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
                self.referenced_class(ast, at, *value)
            }
            // The class of an external `this`, named through its prefix.
            RawTree::NatAst { value, child, .. } if tag == TYPEREF_TAG => {
                let prefix = self.decode_type(ast, child, depth)?;
                let symbol = self.resolved_member(at, *value, prefix, Namespace::Type)?;
                match self.store.symbols.get(symbol).kind {
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass => Ok(symbol),
                    _ => Err(UnpickleError::InvalidReferenceKind { from: at, to: at }),
                }
            }
            _ => Err(UnpickleError::UnsupportedType { tag, address: at }),
        }
    }
}
