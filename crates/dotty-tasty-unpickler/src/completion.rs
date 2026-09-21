//! Simple symbol completion (Milestone 5a): `Missing` to `Complete(TypeId)`.
//!
//! Pass 1 enters every definition with `SymbolInfo::Missing`. This pass gives
//! the simple ones their declared type, from the type tree the wire carries
//! (see [`type_tree`](crate::type_tree)); nothing is inferred from a
//! right-hand side, as TASTy is post-typecheck.
//!
//! | definition | info |
//! |-----------|------|
//! | `VALDEF`, `PARAM` | the projected type of its type tree, as is (a by-name parameter stays `ByName`) |
//! | `TYPEPARAM` | its bounds tree, projected: `Bounds`/`AliasingBounds` as is, any other type wrapped in a fresh `AliasingBounds` |
//! | `TYPEDEF` without a template | the same `toBounds`: `type A = String` has info `AliasingBounds(String)` while its right-hand side still projects to `String` |
//!
//! Deferred, each with its own typed error and no info written: opaque
//! aliases (`OpaqueAliasDeferred`), methods and constructors (5c), and
//! classes, traits and modules (5d) (`UnsupportedSymbolCompletion`). No empty
//! `ClassInfo` is made to mark a class complete.
//!
//! Completion is *per symbol*: each public call is its own transaction, so an
//! unsupported definition never undoes a symbol another call completed. The
//! `SymbolInfo` a call overwrites is journaled and restored when the call
//! fails, which arena truncation could not do for a symbol that already
//! existed. `complete_symbols` is the one batch entry, all or nothing.
//!
//! Completion does not force other symbols: a stable term prefix takes part in
//! member lookup only if its own symbol was completed first (the read-only
//! lookup never completes anything).

use dotty_core::ids::TypeId;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind};
use dotty_core::types::Type;
use dotty_tasty::tasty::{PARAM_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, VALDEF_TAG};

use crate::ast_view::{AstView, address};
use crate::error::UnpickleError;
use crate::unpickler::TastyUnpickler;

impl TastyUnpickler<'_, '_, '_> {
    /// Completes the symbol entered for the definition at `address`, and
    /// returns its info type.
    ///
    /// `Complete(existing)` returns the existing type and allocates nothing.
    /// The call is atomic: on failure every allocation, cache entry and
    /// `SymbolInfo` change it made is undone, and symbols completed by earlier
    /// calls are untouched.
    pub fn complete_symbol(&mut self, address: u32) -> Result<TypeId, UnpickleError> {
        let ast = self.ast_view()?;
        self.declare_special_aliases();
        let transaction = self.begin_transaction();
        let result = self.complete_in(&ast, address, 0);
        self.finish_transaction(transaction, result)
    }

    /// Completes each of `addresses`, in order, as one transaction: if any
    /// fails, none of them stays completed.
    pub fn complete_symbols(&mut self, addresses: &[u32]) -> Result<Vec<TypeId>, UnpickleError> {
        let ast = self.ast_view()?;
        self.declare_special_aliases();
        let transaction = self.begin_transaction();
        let result = addresses
            .iter()
            .map(|address| self.complete_in(&ast, *address, 0))
            .collect();
        self.finish_transaction(transaction, result)
    }

    /// Completes the symbol at `at`; `depth` counts the links being followed
    /// when the completion is reached from inside a type-tree projection.
    pub(crate) fn complete_in(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        depth: usize,
    ) -> Result<TypeId, UnpickleError> {
        let Some(symbol) = self.index.symbol_at(at) else {
            return Err(UnpickleError::MissingEnteredSymbol { address: at });
        };
        let (kind, flags) = {
            let entered = self.store.symbols.get(symbol);
            match entered.info {
                SymbolInfo::Complete(ty) => return Ok(ty),
                SymbolInfo::Deferred(_) => {
                    return Err(UnpickleError::SymbolCompletionDeferred { address: at });
                }
                SymbolInfo::Error => return Err(UnpickleError::SymbolInfoError { address: at }),
                SymbolInfo::Missing => (entered.kind, entered.flags),
            }
        };
        let Some(tag) = ast.tag_at(at) else {
            return Err(UnpickleError::MissingDefinition { address: at });
        };
        let unsupported = UnpickleError::UnsupportedSymbolCompletion { address: at, kind };
        let tree = match tag {
            VALDEF_TAG | PARAM_TAG | TYPEPARAM_TAG => first_child(ast, at)?,
            TYPEDEF_TAG => {
                if kind != SymbolKind::TypeAlias {
                    return Err(unsupported);
                }
                if flags.contains(SymbolFlags::OPAQUE) {
                    return Err(UnpickleError::OpaqueAliasDeferred { address: at });
                }
                first_child(ast, at)?
            }
            _ => return Err(unsupported),
        };
        let projected = self.type_of_tpt(ast, tree, at, depth)?;
        let info = if matches!(tag, TYPEPARAM_TAG | TYPEDEF_TAG) {
            self.bounds_of(at, projected)?
        } else {
            projected
        };
        self.set_symbol_info(symbol, SymbolInfo::Complete(info));
        Ok(info)
    }

    /// Dotty's `toBounds`: bounds stay as they are, an ordinary type becomes
    /// the alias of a fresh `AliasingBounds`. A type that is not the kind of
    /// thing a type definition can be (a by-name or a methodic type) is
    /// refused rather than wrapped.
    fn bounds_of(&mut self, at: u32, ty: TypeId) -> Result<TypeId, UnpickleError> {
        match self.store.types.get(ty) {
            Type::Bounds { .. } | Type::AliasingBounds { .. } => Ok(ty),
            Type::ByName { .. } | Type::Method(_) | Type::Poly(_) => {
                Err(UnpickleError::InvalidCompletedBounds { address: at, ty })
            }
            _ => Ok(self.store.types.alloc(Type::AliasingBounds { alias: ty })),
        }
    }
}

/// The first child of a definition node: its type tree (a `VALDEF`'s and a
/// parameter's declared type, a `TYPEPARAM`'s bounds, a `TYPEDEF`'s
/// right-hand side).
fn first_child(ast: &AstView<'_>, at: u32) -> Result<u32, UnpickleError> {
    ast.children(at)
        .first()
        .map(|child| address(child.offset))
        .ok_or(UnpickleError::MissingDefinition { address: at })
}
