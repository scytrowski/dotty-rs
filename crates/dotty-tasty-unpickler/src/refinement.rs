//! `REFINEDtpt` projection (Milestone 5d2b): the final `Type::Refined` /
//! `Type::Recursive` graph of a refined type tree, reusing the synthetic
//! `<refinement>` class [`crate::enter`] entered for it in pass 1.
//!
//! Scala 3.9's `TreeUnpickler` reads it as:
//!
//! ```scala
//! val refineCls = symAtAddr.getOrElse(start, newRefinedClassSymbol(...)).asClass
//! registerSym(start, refineCls)
//! typeAtAddr(start) = refineCls.typeRef
//! val parent = readTpt()
//! val refinements = readStats(refineCls, end)(using localContext(refineCls))
//! RefinedTypeTree(parent, refinements, refineCls)
//! ```
//!
//! and `TypeAssigner` folds the refinement stats over the parent's type and
//! closes the result over the refinement class's own `ThisType`:
//!
//! ```scala
//! val refined = refinements.foldLeft(parent.tpe)(addRefinement)
//! tree.withType(RecType.closeOver(rt => refined.substThis(refineCls, rt.recThis)))
//! ```
//!
//! This module mirrors that, on top of the state pass 1 already built:
//!
//! 1. the pass-1 synthetic class is looked up and validated, never allocated
//!    again ([`refinement_class`](TastyUnpickler::refinement_class));
//! 2. the parent type tree is projected through the ordinary `type_of_tpt`
//!    path — its `TypeId` is the base of the chain, unchanged;
//! 3. every immediate member's info is completed through the ordinary
//!    per-symbol completion (`complete_in`), in wire order, and folded into
//!    an ordered `Type::Refined` chain (never sorted, never flattened, a
//!    repeated name an explicit `UnsupportedRefinementOverload` rather than
//!    silently kept or shadowed);
//! 4. the chain is closed over the synthetic class's `ThisType` with
//!    [`dotty_core::close_over_this`], which allocates a `Type::Recursive`
//!    only when the class is actually still referred to (Dotty's
//!    `RecType.closeOver`) — most non-recursive refinements therefore return
//!    the plain `Refined` chain, not a `Recursive` of one.
//!
//! A member whose completion fails fails the whole projection: nothing of
//! the chain is published (the public entry points' transaction rolls back
//! everything this call did), and an already-complete member is reused as it
//! is. The synthetic class, its scope and its members' identities are pass-1
//! state and are never touched here, on success or failure.

use std::collections::HashSet;

use dotty_core::close_over_this;
use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::Type;
use dotty_tasty::tasty::{DEFDEF_TAG, TYPEDEF_TAG, VALDEF_TAG};

use crate::ast_view::AstView;
use crate::error::UnpickleError;
use crate::mapping::REFINEMENT_CLASS_NAME;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// The semantic type of the `REFINEDtpt` at `at`: `Refined`/`Recursive`,
    /// never `ClassInfo` (the synthetic class's `ClassInfo` is reconstruction
    /// support state, not the tree's public type). Not atomic by itself; the
    /// public entry points roll back.
    pub(crate) fn type_of_refined_tpt(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        children: &[u32],
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        ast.node(at)?.decode_refined_tpt()?;
        let Some((parent, stats)) = children.split_first() else {
            return Err(UnpickleError::MalformedRefinedTypeTree {
                address: at,
                reason: "a refined type tree has a parent",
            });
        };

        let refine_cls = self.refinement_class(at)?;

        // Dotty reads the parent first (`readTpt()`), through the ordinary
        // type-tree projection; its `TypeId` is preserved exactly.
        let mut current = self.type_of_tpt(ast, *parent, at, depth)?;

        let mut seen = HashSet::new();
        for &stat in stats {
            if !matches!(
                ast.tag_at(stat),
                Some(TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG)
            ) {
                return Err(UnpickleError::MalformedRefinedTypeTree {
                    address: stat,
                    reason: "a refinement stat is not a supported member definition",
                });
            }
            let Some(member) = self.index.symbol_at(stat) else {
                return Err(UnpickleError::MissingEnteredSymbol { address: stat });
            };
            let name = self.store.symbols.get(member).name;
            if !seen.insert(name) {
                return Err(UnpickleError::UnsupportedRefinementOverload {
                    address: at,
                    name: self.store.names.resolve(name.text()).to_owned(),
                });
            }
            let info = self.complete_in(ast, stat, depth)?;
            current = self.store.types.alloc(Type::Refined {
                parent: current,
                name,
                info,
            });
        }

        close_over_this(self.store, current, refine_cls)
            .map_err(|error| UnpickleError::CloseOverThis { address: at, error })
    }

    /// The pass-1 synthetic refinement class entered for the `REFINEDtpt` at
    /// `at`, validated: it must exist, be a `Class` named `<refinement>`, own
    /// a declaration scope whose own `owner` names it back, and already hold
    /// the complete, parent-less, self-type-less `ClassInfo` pass 1 publishes
    /// for it. Nothing is allocated here; a second synthetic class is never
    /// made during projection.
    fn refinement_class(&self, at: u32) -> Result<SymbolId, UnpickleError> {
        let symbol = self
            .index
            .symbol_at(at)
            .ok_or(UnpickleError::MissingRefinementClass { address: at })?;
        let invalid = UnpickleError::InvalidRefinementClass {
            address: at,
            symbol,
        };
        let entered = self.store.symbols.get(symbol);
        if entered.kind != SymbolKind::Class
            || self.store.names.resolve(entered.name.text()) != REFINEMENT_CLASS_NAME
        {
            return Err(invalid);
        }
        let scope = self
            .index
            .scope_of(symbol)
            .ok_or(UnpickleError::MissingRefinementScope {
                address: at,
                symbol,
            })?;
        if self.store.scopes.get(scope).owner != Some(symbol) {
            return Err(invalid);
        }
        match entered.info {
            SymbolInfo::Complete(info_ty) => match self.store.types.get(info_ty) {
                Type::ClassInfo(info)
                    if info.class == symbol
                        && info.declarations == scope
                        && info.parents.is_empty()
                        && info.self_type.is_none()
                        && info.prefix == self.definitions.no_prefix =>
                {
                    Ok(symbol)
                }
                _ => Err(invalid),
            },
            _ => Err(invalid),
        }
    }
}
