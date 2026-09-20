//! Recursive types: `RECtype` and `RECthis`.
//!
//! A `RECtype` is a binder, and the same knot-tying problem as a lambda:
//! its parent may contain `RECthis` nodes that name the `RECtype`, so the
//! `Type::Recursive`'s id must exist before the parent is decoded. The
//! `Recursive` value's `TypeId` *is* its binder identity, and a `RecThis`
//! carries exactly that id. The sequence for a `RECtype` at address `R` is
//! that of the other binders (see the `binders` module):
//!
//! ```text
//! validate the node and its one child
//! reserve a TypeId R'                 (TypeArena::reserve)
//! record  R -> R' in the index        (publish the address)
//! push    PendingBinder { R, R', Recursive, no arity }
//! decode the parent                   (a RECthis naming R now resolves to R')
//! fill    R' with Type::Recursive { parent }
//! pop     the pending binder
//! ```
//!
//! Between "record" and "fill" the slot is unfilled and must not be read; it
//! is guarded by [`TastyUnpickler::is_pending`] like every other pending
//! binder. A recursive binder binds no parameters, so it has no arity, and a
//! `PARAMtype` that names one is an `InvalidBinderKind`.

use dotty_core::ids::TypeId;
use dotty_core::types::Type;

use crate::ast_view::{AstView, address};
use crate::binders::{BinderKind, PendingBinder};
use crate::error::UnpickleError;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// `RECtype Type` at `at`, as a `Recursive` stored under its own address's
    /// `TypeId`. The id is reserved and published before the parent is
    /// decoded. The child comes from the AST index by absolute address.
    pub(crate) fn decode_rec_type(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let children: Vec<u32> = ast
            .children(at)
            .iter()
            .map(|child| address(child.offset))
            .collect();
        let [parent_at] = children[..] else {
            return Err(UnpickleError::MalformedType {
                address: at,
                reason: "a recursive type has exactly one child",
            });
        };

        let reserved = self.store.types.reserve();
        let id = reserved.id();
        self.index.insert_type(at, id)?;
        let binder = PendingBinder {
            address: at,
            id,
            kind: BinderKind::Recursive,
            arity: None,
        };
        let parent =
            self.with_pending_binder(binder, |this| this.type_at(ast, parent_at, at, depth))?;

        self.store.types.fill(reserved, Type::Recursive { parent });
        Ok(id)
    }
}
