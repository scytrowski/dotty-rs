//! Binder identity: the state that lets a `PARAMtype` refer to a binder that
//! is still being decoded.
//!
//! A binder is the `TypeId` its `Type::TypeLambda`, `Type::Poly` or
//! `Type::Method` is stored under, and a `ParamRef` names that id. The id must
//! therefore exist before the binder's children are decoded, since they may
//! contain the `ParamRef`s. All three forms share one sequence (Dotty's
//! `readMethodic`); for a binder at address `B` it is:
//!
//! ```text
//! validate the envelope and parameter names
//! reserve a TypeId B'                 (TypeArena::reserve)
//! record  B -> B' in the index        (publish the address)
//! push    PendingBinder { B, B', .. } (decoding state, below)
//! decode parameter bounds and result  (a PARAMtype to B now resolves to B')
//! fill    B' with Type::TypeLambda    (TypeArena::fill)
//! pop     the pending binder
//! ```
//!
//! Between "record" and "fill" the address entry refers to an *unfilled*
//! arena slot, and reading such a slot panics. That window exists only inside
//! one active decode transaction ([`unpickle_type`](crate::tasty_unpickler::TastyUnpickler::unpickle_type)):
//! on failure the store and the index roll back together, discarding the
//! reservation. Code that runs during the window must not read a binder's
//! slot; it asks [`TastyUnpickler::is_pending`] first, and a `PARAMtype`
//! validates an in-progress binder from its [`PendingBinder`] (kind and arity)
//! rather than from the arena.
//!
//! The pending state is decoding state only. It is not part of the semantic
//! result, is not stored in `dotty-core`, and is empty whenever
//! `unpickle_type` returns.

use dotty_core::ids::TypeId;
use dotty_core::names::{TermName, TypeName};
use dotty_core::types::{
    MethodKind, MethodParam, MethodType, PolyType, Type, TypeLambda, TypeParam, Variance,
};
use dotty_tasty::tasty::{GIVEN_TAG, IMPLICIT_TAG, RawNode};

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
use crate::error::UnpickleError;
use crate::names::wire_name;
use crate::unpickler::TastyUnpickler;

/// What a pending binder will be once filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinderKind {
    TypeLambda,
    Poly,
    #[allow(dead_code)]
    Method,
}

/// The number of parameters of a completed binder type, or `None` if `ty` is
/// not one of the three binder forms a `PARAMtype` can name.
fn completed_arity(ty: &Type) -> Option<usize> {
    match ty {
        Type::TypeLambda(lambda) => Some(lambda.params.len()),
        Type::Poly(poly) => Some(poly.params.len()),
        Type::Method(method) => Some(method.params.len()),
        _ => None,
    }
}

/// A binder whose `TypeId` is reserved and published but not yet filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingBinder {
    /// The binder's AST address, the key a `PARAMtype` refers to.
    pub(crate) address: u32,
    /// The reserved id: the binder identity a `ParamRef` carries.
    pub(crate) id: TypeId,
    pub(crate) kind: BinderKind,
    /// The number of parameters, for checking a `PARAMtype` index.
    pub(crate) arity: usize,
}

impl TastyUnpickler<'_, '_, '_> {
    /// The pending binder at AST address `address`, if one is being decoded.
    pub(crate) fn pending_at(&self, address: u32) -> Option<PendingBinder> {
        self.pending_binders
            .iter()
            .rev()
            .find(|binder| binder.address == address)
            .copied()
    }

    /// Whether `id` is a reserved binder slot that has not been filled yet.
    /// Such a slot must not be read from the arena.
    pub(crate) fn is_pending(&self, id: TypeId) -> bool {
        self.pending_binders.iter().any(|binder| binder.id == id)
    }

    /// The pending binder whose reserved id is `id`, if any.
    fn pending_with_id(&self, id: TypeId) -> Option<PendingBinder> {
        self.pending_binders
            .iter()
            .rev()
            .find(|binder| binder.id == id)
            .copied()
    }

    /// `PARAMtype Length binder_ASTRef paramNum_Nat` at `at`: a `ParamRef` to
    /// the binder at that AST address, by address only, never by name.
    ///
    /// The binder is, in this order: a pending one (validated from its
    /// recorded arity, never from the arena), one already decoded (checked to
    /// be a `TypeLambda`), or one not yet decoded, which is decoded now. That
    /// last decode can reach this very node through the binder's own
    /// children, so the address is looked up again afterwards instead of
    /// allocating a second `ParamRef`.
    pub(crate) fn decode_param_type(
        &mut self,
        ast: &AstView<'_>,
        node: &RawNode<'_>,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let shape = node.decode_param_type()?;
        let binder_address = shape.binder.address;
        let index = shape.parameter_number;
        // Decoding a binder on demand can lead back here (through the binder,
        // or through a `PARAMtype` that names another `PARAMtype`), so the
        // chain is bounded like a `SHAREDtype` chain.
        if depth >= MAX_SHARED_DEPTH {
            return Err(UnpickleError::InvalidReferenceTarget {
                from: at,
                to: binder_address,
            });
        }
        if !ast.is_node(binder_address) {
            return Err(UnpickleError::InvalidBinderReference {
                from: at,
                binder: binder_address,
            });
        }

        let (binder, arity) = if let Some(pending) = self.pending_at(binder_address) {
            (pending.id, pending.arity)
        } else {
            let id = match self.index.type_at(binder_address) {
                Some(id) => id,
                None => {
                    let id = self.type_at(ast, binder_address, at, depth + 1)?;
                    if let Some(existing) = self.index.type_at(at) {
                        return Ok(existing);
                    }
                    id
                }
            };
            match self.pending_with_id(id) {
                // The address is a `SHAREDtype` link to a binder that is
                // still being decoded.
                Some(pending) => (pending.id, pending.arity),
                None => match completed_arity(self.store.types.get(id)) {
                    Some(arity) => (id, arity),
                    None => {
                        return Err(UnpickleError::InvalidBinderKind {
                            from: at,
                            binder: id,
                        });
                    }
                },
            }
        };

        if usize::try_from(index).map_or(true, |index| index >= arity) {
            return Err(UnpickleError::InvalidParameterIndex {
                address: at,
                binder,
                index,
                arity,
            });
        }
        let id = self.store.types.alloc(Type::ParamRef { binder, index });
        self.index.insert_type(at, id)?;
        Ok(id)
    }

    /// `TYPELAMBDAtype Length result_Type (paramBounds_Type paramName_NameRef)*`
    /// at `at`, as a `TypeLambda` stored under its own address's `TypeId`.
    ///
    /// A `TYPELAMBDAtype` carries no variance of its own, so the parameters are
    /// invariant. Variance markers belong to an enclosing `TYPEBOUNDS`, which
    /// is deferred (see the module documentation of `types`).
    pub(crate) fn decode_type_lambda(
        &mut self,
        ast: &AstView<'_>,
        node: &RawNode<'_>,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let shape = node.decode_poly_type()?;
        let names = shape.type_names.iter().map(|entry| entry.name).collect();
        self.decode_methodic(ast, at, depth, Methodic::TypeLambda, names)
    }

    /// `POLYtype Length result_Type (paramBounds_Type paramName_NameRef)*` at
    /// `at`, as a `Poly` stored under its own address's `TypeId`. Its
    /// parameters are invariant: a `PolyType` has no variance markers.
    pub(crate) fn decode_poly_type(
        &mut self,
        ast: &AstView<'_>,
        node: &RawNode<'_>,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let shape = node.decode_poly_type()?;
        let names = shape.type_names.iter().map(|entry| entry.name).collect();
        self.decode_methodic(ast, at, depth, Methodic::Poly, names)
    }

    /// `METHODtype Length result_Type (paramType_Type paramName_NameRef)*
    /// Modifier*` at `at`, as a `Method` stored under its own address's
    /// `TypeId`. Zero parameters is a valid clause, `(): R`.
    ///
    /// A `ParamRef` to this binder is a *term* parameter reference, which is
    /// how a dependent result (`(x: Box): x.Out`) names the clause it is in.
    pub(crate) fn decode_method_type(
        &mut self,
        ast: &AstView<'_>,
        node: &RawNode<'_>,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let shape = node.decode_method_type()?;
        let kind = method_kind(at, &shape.modifiers)?;
        let names = shape.type_names.iter().map(|entry| entry.name).collect();
        self.decode_methodic(ast, at, depth, Methodic::Method(kind), names)
    }

    /// The shared body of every binder form (Dotty's `readMethodic`): the
    /// envelope and the parameter names are validated first. Then the id is
    /// reserved and published, and only then are the children decoded, so a
    /// `ParamRef` inside them (in a parameter's info, in the result, or in a
    /// nested type) resolves to the id under construction. The parameter
    /// infos are decoded before the result: either order works once the
    /// binder is published, and this one lets an info that is not a bounds
    /// type fail before the result is read. Every child comes from the AST
    /// index by absolute address (`children[0]` is the result, then one info
    /// per parameter); the structural decoder's trees are relative to the
    /// payload and only validate the shape.
    ///
    /// `names` are the wire `NameRef`s of the parameters, in order.
    fn decode_methodic(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        depth: usize,
        form: Methodic,
        names: Vec<u32>,
    ) -> Result<TypeId, UnpickleError> {
        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        if children.len() != names.len() + 1 {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "the binder's children disagree with its parameters",
            });
        }
        if names.is_empty() && form.requires_parameters() {
            // Dotty's `PolyType` and `HKTypeLambda` require at least one
            // parameter (unlike a `MethodType`, where `()` is a clause).
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "the binder has no parameters",
            });
        }
        let mut interned = Vec::with_capacity(names.len());
        for reference in &names {
            let text = wire_name(self.file.names(), *reference)?;
            interned.push(self.store.names.intern(&text));
        }

        let reserved = self.store.types.reserve();
        let id = reserved.id();
        self.index.insert_type(at, id)?;
        let binder = PendingBinder {
            address: at,
            id,
            kind: form.kind(),
            arity: interned.len(),
        };
        let (params, result) = self.with_pending_binder(binder, |this| {
            let mut params = Vec::with_capacity(interned.len());
            for (position, name) in interned.iter().enumerate() {
                let info = this.type_at(ast, children[position + 1], at, depth)?;
                if form.has_type_parameters() {
                    this.check_parameter_bounds(id, position, info)?;
                }
                params.push((*name, info));
            }
            let result = this.type_at(ast, children[0], at, depth)?;
            Ok::<_, UnpickleError>((params, result))
        })?;

        let type_params = || {
            params
                .iter()
                .map(|(name, info)| TypeParam {
                    name: TypeName::new(*name),
                    bounds: *info,
                    variance: Variance::Invariant,
                })
                .collect()
        };
        let ty = match form {
            Methodic::TypeLambda => Type::TypeLambda(TypeLambda {
                params: type_params(),
                result,
            }),
            Methodic::Poly => Type::Poly(PolyType {
                params: type_params(),
                result,
            }),
            // `erased` and `varargs` are not on the wire of a `METHODtype`:
            // see the module documentation.
            Methodic::Method(kind) => Type::Method(MethodType {
                params: params
                    .iter()
                    .map(|(name, info)| MethodParam {
                        name: TermName::new(*name),
                        ty: *info,
                        erased: false,
                        varargs: false,
                    })
                    .collect(),
                result,
                kind,
            }),
        };
        self.store.types.fill(reserved, ty);
        Ok(id)
    }

    /// A parameter's info must be a bounds type. `bounds` is read from the
    /// arena only when it is not a binder still being decoded.
    fn check_parameter_bounds(
        &self,
        binder: TypeId,
        position: usize,
        bounds: TypeId,
    ) -> Result<(), UnpickleError> {
        let is_bounds = !self.is_pending(bounds)
            && matches!(
                self.store.types.get(bounds),
                Type::Bounds { .. } | Type::AliasingBounds { .. }
            );
        if is_bounds {
            Ok(())
        } else {
            Err(UnpickleError::InvalidTypeParameterBounds {
                binder,
                index: u32::try_from(position).unwrap_or(u32::MAX),
                bounds,
            })
        }
    }

    /// Runs `decode` with `binder` registered as pending, and unregisters it
    /// however `decode` ends, so an error in a child cannot leave the binder
    /// (or a nested one) behind.
    pub(crate) fn with_pending_binder<T>(
        &mut self,
        binder: PendingBinder,
        decode: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let depth = self.pending_binders.len();
        self.pending_binders.push(binder);
        let result = decode(self);
        self.pending_binders.truncate(depth);
        result
    }
}

/// The binder forms that share the reserve/publish/fill sequence.
#[derive(Clone, Copy)]
enum Methodic {
    TypeLambda,
    Poly,
    Method(MethodKind),
}

impl Methodic {
    fn kind(self) -> BinderKind {
        match self {
            Self::TypeLambda => BinderKind::TypeLambda,
            Self::Poly => BinderKind::Poly,
            Self::Method(_) => BinderKind::Method,
        }
    }

    /// Whether the parameters are type parameters (with bounds infos) rather
    /// than term parameters.
    fn has_type_parameters(self) -> bool {
        !matches!(self, Self::Method(_))
    }

    /// Whether an empty parameter list is malformed for this form. Only a
    /// method clause may be empty, `()`.
    fn requires_parameters(self) -> bool {
        self.has_type_parameters()
    }
}

/// The clause kind of a `METHODtype` from its modifier tail, as Dotty's
/// `methodTypeCompanion`: `IMPLICIT` is an implicit clause, `GIVEN` a
/// contextual one, neither a plain one. Both at once contradict each other,
/// and any other modifier has no meaning on a method type, so neither is
/// silently ignored.
fn method_kind(at: u32, modifiers: &[u8]) -> Result<MethodKind, UnpickleError> {
    let mut kind = MethodKind::Plain;
    for &modifier in modifiers {
        let next = match modifier {
            IMPLICIT_TAG => MethodKind::Implicit,
            GIVEN_TAG => MethodKind::Contextual,
            tag => return Err(UnpickleError::InvalidMethodModifier { address: at, tag }),
        };
        if kind != MethodKind::Plain && kind != next {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "the method type is both implicit and given",
            });
        }
        kind = next;
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::Definitions;
    use dotty_core::store::SemanticStore;
    use dotty_core::types::Type;
    use dotty_tasty::tasty::TastyFile;

    const FILE: &[u8] = include_bytes!("../tests/fixtures/semantic/Plain.tasty");

    fn binder(address: u32, id: TypeId) -> PendingBinder {
        PendingBinder {
            address,
            id,
            kind: BinderKind::TypeLambda,
            arity: 1,
        }
    }

    /// Runs `check` with an unpickler and two type ids to use as binders.
    fn with_unpickler(check: impl FnOnce(&mut TastyUnpickler<'_, '_, '_>, TypeId, TypeId)) {
        let file = TastyFile::parse_scala_3_9(FILE).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let first = store.types.alloc(Type::NoType);
        let second = store.types.alloc(Type::NoType);
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        check(&mut unpickler, first, second);
    }

    #[test]
    fn a_pending_binder_is_visible_inside_and_gone_after() {
        with_unpickler(|unpickler, first, second| {
            assert_eq!(unpickler.pending_at(10), None);

            unpickler.with_pending_binder(binder(10, first), |inner| {
                assert_eq!(inner.pending_at(10), Some(binder(10, first)));
                assert!(inner.is_pending(first));
                assert!(!inner.is_pending(second));
            });

            assert_eq!(unpickler.pending_at(10), None);
            assert!(!unpickler.is_pending(first));
        });
    }

    #[test]
    fn an_error_inside_still_unregisters_the_binder() {
        with_unpickler(|unpickler, first, _| {
            let result: Result<(), &str> =
                unpickler.with_pending_binder(binder(10, first), |_| Err("child failed"));

            assert_eq!(result, Err("child failed"));
            assert!(unpickler.pending_binders.is_empty());
        });
    }

    #[test]
    fn a_failed_nested_binder_restores_the_enclosing_stack() {
        with_unpickler(|unpickler, first, second| {
            unpickler.with_pending_binder(binder(10, first), |outer| {
                let failed: Result<(), &str> =
                    outer.with_pending_binder(binder(20, second), |_| Err("inner failed"));

                assert!(failed.is_err());
                // The outer binder is still pending, the inner one is not.
                assert_eq!(outer.pending_at(10), Some(binder(10, first)));
                assert_eq!(outer.pending_at(20), None);
                assert_eq!(outer.pending_binders.len(), 1);
            });
            assert!(unpickler.pending_binders.is_empty());
        });
    }

    #[test]
    fn lookup_by_address_finds_the_binder_at_that_address_not_the_latest() {
        with_unpickler(|unpickler, first, second| {
            unpickler.with_pending_binder(binder(10, first), |outer| {
                outer.with_pending_binder(binder(20, second), |inner| {
                    assert_eq!(inner.pending_at(10).map(|b| b.id), Some(first));
                    assert_eq!(inner.pending_at(20).map(|b| b.id), Some(second));
                });
            });
        });
    }
}
