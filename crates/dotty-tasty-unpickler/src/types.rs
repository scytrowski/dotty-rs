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
//! | `ANDtype`             | `Type Type`               | `And { left, right }`              |
//! | `ORtype`              | `Type Type`               | `Or { left, right }`               |
//! | `SUPERtype`           | `Type Type`               | `SuperType { this_type, super_type }` |
//! | `BYNAMEtype`          | `Type`                    | `ByName { result }`                |
//! | `UNITconst` .. `STRINGconst` | the constant itself | `Constant(..)`, losslessly        |
//! | `CLASSconst`          | `Type`                    | `Constant(Class(type))`            |
//! | `FLEXIBLEtype`        | `Type`                    | `Flexible { underlying }`          |
//! | `TYPEBOUNDS`          | `Type Type`               | `Bounds { low, high }`             |
//! | `TYPEBOUNDS`          | `Type` (no upper bound)   | `AliasingBounds { alias }`         |
//!
//! ## Recursive and refined types (Milestone 4a)
//!
//! | TASTy                 | wire shape                | semantic type                      |
//! |-----------------------|---------------------------|------------------------------------|
//! | `RECtype`             | `Type`                    | `Recursive { parent }`             |
//! | `RECthis`             | `ASTRef`                  | `RecThis { binder }`               |
//! | `REFINEDtype`         | `NameRef Type Type`       | `Refined { parent, name, info }`   |
//!
//! A `RECtype` is a binder like the lambdas (see the `recursive` module): its
//! id is reserved and published before the parent is decoded, and a `RECthis`
//! carries that exact id, found by address. Every `RECthis` naming one binder
//! shares one `RecThis` `TypeId`, as Dotty's `RecType` has one `recThis`. A
//! `REFINEDtype` name is a term name unless the info, after `SHAREDtype` links,
//! is `TYPEBOUNDS` (see the `refined` module). A refinement member has no
//! symbol, so a by-name reference through a refined or recursive prefix stays
//! `UnsupportedResolutionPrefix`.
//!
//! ## Variance-bearing `TYPEBOUNDS` (Milestone 3c)
//!
//! `TYPEBOUNDS` may end in variance markers (`STABLE`, `COVARIANT`,
//! `CONTRAVARIANT`), as Dotty's `readVariances` reads them: on an alias-only
//! node they apply to the one child (`AliasingBounds(readVariances(lo))`), on
//! a two-sided node to the *upper* bound only (`hi = readVariances(readType())`),
//! never to `low`. They only mean something on a `TypeLambda`: for any other
//! target Dotty's `readVariances` returns the type as it is (`case _ => tp`), and
//! so does this pass. For a lambda, a count different from its parameters is
//! [`BoundsVarianceArityMismatch`](UnpickleError::BoundsVarianceArityMismatch)
//! (nothing is dropped, padded or truncated), and a lambda still being decoded
//! is [`BoundsVarianceTargetPending`](UnpickleError::BoundsVarianceTargetPending).
//!
//! Dotty's `withVariances` does not change the lambda: it builds a *new* one
//! (`newLikeThis`) and substitutes the old binder in it. So does this pass,
//! through the format-agnostic `dotty_core::rebind_type_lambda`. The child
//! `TYPELAMBDAtype` is decoded normally first and stays the type cached at its
//! own AST address (and what a `SHAREDtype` to it returns), with no declared
//! variance; the bounds hold a fresh derived lambda, with
//! `declared_variance: Some(..)` per parameter (`STABLE` is
//! `Some(Invariant)`), no AST address, and its own `ParamRef`s. Decoding the
//! bounds again returns the cached bounds, so no second derived lambda is made.
//!
//! ## Binders (Milestones 3a and 3b)
//!
//! | TASTy                 | wire shape                | semantic type                      |
//! |-----------------------|---------------------------|------------------------------------|
//! | `TYPELAMBDAtype`      | `Type (Type NameRef)*`    | `TypeLambda { params, result }`    |
//! | `POLYtype`            | `Type (Type NameRef)*`    | `Poly { params, result }`          |
//! | `METHODtype`          | `Type (Type NameRef)* Modifier*` | `Method { params, result, kind }` |
//! | `PARAMtype`           | `ASTRef Nat`              | `ParamRef { binder, index }`       |
//!
//! A binder is the `TypeId` of its `TypeLambda`, `Poly` or `Method`, and a
//! `ParamRef` carries that exact id, found through the binder's AST address, never a name. The id is
//! reserved and its address published before the children are decoded, so a
//! `ParamRef` in a parameter's bounds, in the result or in a nested type
//! resolves to the binder under construction; the slot is filled last. See
//! the `binders` module for the sequence, the pending-binder state that lets
//! a `PARAMtype` validate a binder whose arena slot is still unfilled, and the
//! rules for a `PARAMtype` decoded before its binder.
//!
//! Each parameter's info must be `Bounds` or `AliasingBounds`, or the node is
//! [`InvalidTypeParameterBounds`](UnpickleError::InvalidTypeParameterBounds);
//! a standalone `TYPELAMBDAtype` or `POLYtype` has no declared variance of its
//! own (`declared_variance: None`, which is not the same as `Some(Invariant)`). A `POLYtype` and a `TYPELAMBDAtype` need at least
//! one parameter (as Dotty's `PolyType` and `HKTypeLambda` do), or the node is
//! [`MalformedType`](UnpickleError::MalformedType).
//!
//! A `METHODtype` parameter is a term parameter: its name is a `TermName`
//! and its type is any type, not bounds. `()` is a valid clause, so an empty
//! method is accepted. The modifier tail is the clause kind, as Dotty's
//! `methodTypeCompanion` reads it: none is `Plain`, `IMPLICIT` is `Implicit`,
//! `GIVEN` is `Contextual`. Both together are `Implicit`, as in Dotty, and any
//! other modifier is [`InvalidMethodModifier`](UnpickleError::InvalidMethodModifier).
//! A `PARAMtype` to a method is a reference to one of its term parameters,
//! which is how a dependent result (`(x: Box): x.Out`) names its own clause.
//!
//! `MethodParam.erased` and `MethodParam.varargs` are both `false`. Dotty
//! derives erasure from an `ErasedParamAnnot` on the parameter's *type*, which
//! is an `ANNOTATEDtype` this pass does not decode yet, so no decodable method
//! has an erased parameter; Milestone 4 must derive `erased` from the decoded
//! annotation. `varargs` is the JVM `ACC_VARARGS` distinction, which a
//! `METHODtype` does not carry (a repeated parameter is a `Seq`-like *type*),
//! so it is never inferred from position or name.
//!
//! `And` and `Or` keep the operand order and nesting the compiler wrote: no
//! commutative normalisation, no flattening. `BYNAMEtype` stays a wrapper; it
//! is never lowered to its result. `SUPERtype` is the type node, not the
//! term-level `SUPER`.
//!
//! Every child is decoded through the same entry point as a top-level type, so
//! it is cached, shared and resolved like any other, and a child's error is
//! the error of the whole node. Compound nodes are not interned: equal trees
//! at different addresses keep different ids.
//!
//! Every other form is `UnsupportedType`: `ANNOTATEDtype`, match types, and
//! `TYPEREFin`/`TERMREFin`.
//! Unsupported input is never lowered to `NoType`, `NoPrefix` or `Error`.

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::resolution::{MemberRequest, MemberSelector, ResolutionError};
use dotty_core::symbols::SymbolKind;
use dotty_core::types::{Constant, Type};
use dotty_tasty::tasty::{
    ANDTYPE_TAG, ANNOTATEDTYPE_TAG, APPLIEDTYPE_TAG, AstError, BYNAMETYPE_TAG, CLASSCONST_TAG,
    ConstantValue, FLEXIBLETYPE_TAG, METHODTYPE_TAG, ORTYPE_TAG, PARAMTYPE_TAG, POLYTYPE_TAG,
    RECTHIS_TAG, RECTYPE_TAG, REFINEDTYPE_TAG, RawTree, SHAREDTYPE_TAG, SUPERTYPE_TAG, TERMREF_TAG,
    TERMREFDIRECT_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG, THIS_TAG, TYPEBOUNDS_TAG,
    TYPELAMBDATYPE_TAG, TYPEREF_TAG, TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG,
    TermValue,
};

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::binders::declared_variances;
use crate::error::UnpickleError;
use crate::lookup::{LocalLookup, lookup_member};
use crate::names::{is_signed, package_segments, string_value, wire_name};
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
                (RECTHIS_TAG, TermValue::AstRef(target)) => {
                    return self.decode_rec_this(ast, at, *target, depth);
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
                _ => match term.constant_value().map_err(AstError::from)? {
                    Some(value) => Type::Constant(self.constant(value)?),
                    None => return Err(UnpickleError::UnsupportedType { tag, address: at }),
                },
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
                let shape = node.decode_applied_type()?;
                let ids = self.decode_children(ast, at, shape.arguments.len() + 1, depth)?;
                if shape.arguments.is_empty() {
                    // The grammar allows `Type*` to be empty, and Dotty's
                    // `appliedTo(Nil)` is the constructor itself. So is
                    // this node: it owns the constructor's `TypeId`, like a
                    // `SHAREDtype` link, rather than a second `Applied` spelling.
                    self.index.insert_type(at, ids[0])?;
                    return Ok(ids[0]);
                }
                Type::Applied {
                    tycon: ids[0],
                    args: ids[1..].to_vec(),
                }
            }
            RawTree::LengthNode(node) if tag == ANDTYPE_TAG => {
                node.decode_and_type()?;
                let [left, right] = self.decode_binary(ast, at, depth)?;
                Type::And { left, right }
            }
            RawTree::LengthNode(node) if tag == ORTYPE_TAG => {
                node.decode_or_type()?;
                let [left, right] = self.decode_binary(ast, at, depth)?;
                Type::Or { left, right }
            }
            RawTree::LengthNode(node) if tag == SUPERTYPE_TAG => {
                node.decode_super_type()?;
                let [this_type, super_type] = self.decode_binary(ast, at, depth)?;
                Type::SuperType {
                    this_type,
                    super_type,
                }
            }
            RawTree::LengthNode(node) if tag == TYPEBOUNDS_TAG => {
                let shape = node.decode_type_bounds()?;
                let variances = declared_variances(at, &shape.variances)?;
                if shape.high.is_some() {
                    // Dotty: `hi = readVariances(readType())`. The markers go
                    // to the upper bound, never to `low`.
                    let [low, high] = self.decode_binary(ast, at, depth)?;
                    let high = self.apply_declared_variances(at, high, &variances)?;
                    Type::Bounds { low, high }
                } else {
                    // Dotty: `AliasingBounds(readVariances(lo))`.
                    let ids = self.decode_children(ast, at, 1, depth)?;
                    let alias = self.apply_declared_variances(at, ids[0], &variances)?;
                    Type::AliasingBounds { alias }
                }
            }
            RawTree::Ast { .. } if tag == CLASSCONST_TAG => {
                // The constant holds the type the class literal denotes, not
                // a reference to a class symbol.
                let class = self.decode_type(ast, &tree.decode_class_const()?.child, depth)?;
                Type::Constant(Constant::Class(class))
            }
            // A binder-related node owns its identity: it records its own
            // address, possibly before its children are decoded.
            RawTree::LengthNode(node) if tag == TYPELAMBDATYPE_TAG => {
                return self.decode_type_lambda(ast, node, at, depth);
            }
            RawTree::LengthNode(node) if tag == POLYTYPE_TAG => {
                return self.decode_poly_type(ast, node, at, depth);
            }
            RawTree::LengthNode(node) if tag == METHODTYPE_TAG => {
                return self.decode_method_type(ast, node, at, depth);
            }
            RawTree::Ast { .. } if tag == RECTYPE_TAG => {
                tree.decode_rec_type()?;
                return self.decode_rec_type(ast, at, depth);
            }
            RawTree::LengthNode(node) if tag == REFINEDTYPE_TAG => {
                self.decode_refined_type(ast, node, at, depth)?
            }
            RawTree::LengthNode(node) if tag == ANNOTATEDTYPE_TAG => {
                self.decode_annotated_type(ast, node, at, depth)?
            }
            RawTree::LengthNode(node) if tag == PARAMTYPE_TAG => {
                return self.decode_param_type(ast, node, at, depth);
            }
            RawTree::LengthNode(node) if tag == FLEXIBLETYPE_TAG => {
                node.decode_flexible_type()?;
                let ids = self.decode_children(ast, at, 1, depth)?;
                Type::Flexible { underlying: ids[0] }
            }
            RawTree::Ast { .. } if tag == BYNAMETYPE_TAG => {
                let result = self.decode_type(ast, &tree.decode_by_name_type()?.child, depth)?;
                Type::ByName { result }
            }
            _ => return Err(UnpickleError::UnsupportedType { tag, address: at }),
        };

        let id = self.store.types.alloc(ty);
        self.index.insert_type(at, id)?;
        Ok(id)
    }

    /// The semantic constant for a wire constant, losslessly: floating-point
    /// bit patterns and 16-bit characters are carried as they were written.
    fn constant(&mut self, value: ConstantValue) -> Result<Constant, UnpickleError> {
        Ok(match value {
            ConstantValue::Unit => Constant::Unit,
            ConstantValue::Null => Constant::Null,
            ConstantValue::Boolean(value) => Constant::Boolean(value),
            ConstantValue::Byte(value) => Constant::Byte(value),
            ConstantValue::Short(value) => Constant::Short(value),
            ConstantValue::Char(unit) => Constant::Char(unit),
            ConstantValue::Int(value) => Constant::Int(value),
            ConstantValue::Long(value) => Constant::Long(value),
            ConstantValue::FloatBits(bits) => Constant::FloatBits(bits),
            ConstantValue::DoubleBits(bits) => Constant::DoubleBits(bits),
            ConstantValue::String(reference) => {
                let text = string_value(self.file.names(), reference)?;
                Constant::String(self.store.names.intern(&text))
            }
        })
    }

    /// Decodes the `count` child types of the length-prefixed node at `at`,
    /// in wire order, each through [`type_at`](Self::type_at), so a child is
    /// cached, shared and resolved like any other type.
    ///
    /// The children are taken from the AST index, not from the structural
    /// decoders' trees: those are relative to the node's payload, while a
    /// type is keyed by its absolute address. The structural decoder has
    /// already checked the node's shape; `count` guards the two agreeing.
    fn decode_children(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        count: usize,
        depth: usize,
    ) -> Result<Vec<TypeId>, UnpickleError> {
        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        if children.len() != count {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "the node's children do not match its shape",
            });
        }
        children
            .into_iter()
            .map(|child| self.type_at(ast, child, at, depth))
            .collect()
    }

    /// The two operands of a binary type node, left first. What they mean is
    /// up to the caller: `And` and `Or` operands, or the `this` and `super`
    /// parts of a `SuperType`.
    fn decode_binary(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        depth: usize,
    ) -> Result<[TypeId; 2], UnpickleError> {
        let ids = self.decode_children(ast, at, 2, depth)?;
        Ok([ids[0], ids[1]])
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

        // A prefix that is (or wraps) a binder still being decoded has no
        // readable slot yet, so its members cannot be looked up.
        let mut walk = prefix;
        loop {
            if self.is_pending(walk) {
                return Err(UnpickleError::UnsupportedResolutionPrefix {
                    address: at,
                    prefix,
                });
            }
            match self.store.types.get(walk) {
                Type::Flexible { underlying } => walk = *underlying,
                _ => break,
            }
        }

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
