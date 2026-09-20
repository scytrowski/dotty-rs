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
use dotty_tasty::tasty::RECTYPE_TAG;

use crate::ast_view::{AstView, MAX_SHARED_DEPTH, address};
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

    /// `RECthis ASTRef` at `at`, naming the `RECtype` at `target`: a `RecThis`
    /// whose binder is that `RECtype`'s exact `TypeId`, found by address only.
    ///
    /// The binder is, in this order: a pending one (never read from the
    /// arena, and it must be recursive); one already decoded (which must be a
    /// `Recursive`); or one not yet decoded, which is decoded now, and only if
    /// the node is a `RECtype`. That decode can reach this very node through
    /// the binder's own parent, so the address is looked up again afterwards
    /// instead of allocating a second `RecThis`. The chain is bounded like a
    /// `SHAREDtype` chain.
    ///
    /// Dotty's `RecType` keeps one `recThis` per binder, so every `RECthis`
    /// naming one binder, at whatever address, gets the same `TypeId`
    /// (address identity is kept: each address maps to exactly one type, and
    /// several addresses may map to the same one). `RecThis` values of
    /// different binders are never shared.
    pub(crate) fn decode_rec_this(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        target: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let invalid = UnpickleError::InvalidReferenceTarget {
            from: at,
            to: target,
        };
        if depth >= MAX_SHARED_DEPTH {
            return Err(invalid);
        }
        let Some(tag) = ast.tag_at(target) else {
            return Err(invalid);
        };

        let binder = if let Some(pending) = self.pending_at(target) {
            if pending.kind != BinderKind::Recursive {
                return Err(invalid);
            }
            pending.id
        } else {
            let id = match self.index.type_at(target) {
                Some(id) => id,
                None => {
                    // Only a `RECtype` is decoded on demand.
                    if tag != RECTYPE_TAG {
                        return Err(invalid);
                    }
                    let id = self.type_at(ast, target, at, depth + 1)?;
                    if let Some(existing) = self.index.type_at(at) {
                        return Ok(existing);
                    }
                    id
                }
            };
            match self.pending_with_id(id) {
                // The address is a `SHAREDtype` link to a binder that is
                // still being decoded.
                Some(pending) if pending.kind == BinderKind::Recursive => pending.id,
                Some(_) => return Err(invalid),
                None => match self.store.types.get(id) {
                    Type::Recursive { .. } => id,
                    _ => return Err(invalid),
                },
            }
        };

        let this = self.canonical_rec_this(binder);
        self.index.insert_type(at, this)?;
        Ok(this)
    }

    /// The one `RecThis` of the recursive binder `binder`, made on first use.
    fn canonical_rec_this(&mut self, binder: TypeId) -> TypeId {
        if let Some(&existing) = self.rec_this.get(&binder) {
            return existing;
        }
        let this = self.store.types.alloc(Type::RecThis { binder });
        self.rec_this.insert(binder, this);
        self.rec_this_journal.push(binder);
        this
    }

    /// Forgets the canonical `RecThis` entries added after the journal held
    /// `mark` entries. Called with the store rollback of a failed call: the
    /// ids of the rolled-back binder and `RecThis` are handed out again, so a
    /// surviving entry would pair a new binder with a stale `RecThis`.
    pub(crate) fn roll_back_rec_this(&mut self, mark: usize) {
        while self.rec_this_journal.len() > mark {
            if let Some(binder) = self.rec_this_journal.pop() {
                self.rec_this.remove(&binder);
            }
        }
    }
}
