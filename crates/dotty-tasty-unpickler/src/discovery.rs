//! Pass-1 identity discovery (Milestone 5d2c): a mode-aware walker that
//! mirrors every structural route the semantic projection layer can take
//! from a declared type position, so far enough to pre-enter the two
//! identity-bearing tree forms pass 1 knows about (`LAMBDAtpt`'s type
//! parameters, Milestone 5c; `REFINEDtpt`'s synthetic `<refinement>` class,
//! Milestone 5d2b) before projection ever runs.
//!
//! ## Why a syntax whitelist was not enough
//!
//! Through Milestone 5d2b, pass 1 scanned a declared type tree with one
//! recursive walk that only descended through a fixed list of `TypeTree`
//! tags (`SHAREDterm`, `LAMBDAtpt`, `REFINEDtpt`, `APPLIEDtpt`, `BYNAMEtpt`,
//! `EXPLICITtpt`, `TYPEBOUNDStpt`, `ANNOTATEDtpt`). Real semantic projection
//! is not that shallow: [`type_of_tpt`](TastyUnpickler::type_of_tpt) can hand
//! off to [`type_of_term`](TastyUnpickler::type_of_term) (a `SELECTtpt`
//! qualifier, a `SINGLETONtpt` reference), which can hand off to
//! [`type_at`](TastyUnpickler::type_at)/`decode_type` (an embedded `IDENT`
//! type, a selection's semantic prefix), which can reach
//! [`this_class`](TastyUnpickler::this_class) (a `THIS`/`QUALTHIS` class
//! reference) — and any of those hops can land on a `SHAREDtype` link whose
//! target is, or contains, a `LAMBDAtpt`/`REFINEDtpt` identity the old
//! whitelist never visited. Two such real library self types were measured
//! failing with `MissingRefinementClass`/`InvalidRefinementClass` in
//! Milestone 5d2b's own corpus run (see [`crate::refinement`]).
//!
//! ## The fix: four interpretation modes, mirrored one for one
//!
//! [`DiscoveryMode`] names the four structural layers projection has:
//!
//! | mode | mirrors | entry examples |
//! |------|---------|-----------------|
//! | [`TypeTree`](DiscoveryMode::TypeTree) | [`type_of_tpt`](TastyUnpickler::type_of_tpt) | a `VALDEF`'s declared type, a `DEFDEF`'s result |
//! | [`TermType`](DiscoveryMode::TermType) | [`type_of_term`](TastyUnpickler::type_of_term) | a `SELECTtpt`/`SINGLETONtpt` reference |
//! | [`SemanticType`](DiscoveryMode::SemanticType) | `type_at`/`decode_type` | an `IDENTtpt`'s embedded type, a selection's prefix |
//! | [`ClassRef`](DiscoveryMode::ClassRef) | [`this_class`](TastyUnpickler::this_class) | a `THIS`/`QUALTHIS` class argument |
//!
//! [`discover_identities`](TastyUnpickler::discover_identities) dispatches on
//! the mode and follows *exactly* the child addresses the corresponding
//! projection function would read next, under the corresponding
//! interpretation — never more (an unsupported term body such as `APPLY` or
//! `BLOCK` is not a generic AST walk target; it simply has no matching arm
//! and the walk stops there, the same way projection itself would refuse or
//! ignore it) and never less (every route in the table above that can reach
//! a `LAMBDAtpt`/`REFINEDtpt` is mirrored).
//!
//! Discovery only ever *allocates* the two identity-bearing forms above
//! (through [`TastyUnpickler::enter_lambda_tpt`] and
//! [`TastyUnpickler::enter_refined_tpt`], both otherwise unchanged from
//! Milestone 5c/5d2b) and the ordinary symbols/scopes those already enter. It
//! never calls `type_of_tpt`, `type_of_term`, `type_at`, `complete_in` or the
//! `SymbolResolver`, never allocates a `TypeId`, and never resolves a member
//! or a package by name: it is reference reachability only, exactly as pass 1
//! always was.
//!
//! ## Direct/symbol reference targets (§11)
//!
//! An ordinary `TYPEREFdirect`/`TYPEREFsymbol`/`TERMREFdirect`/`TERMREFsymbol`
//! names another definition's own address. Discovery does not follow that
//! address as if it were a declared-type position of the current owner — a
//! reference is not a re-entry into its target's own scan, and doing so would
//! make discovery a general definition walker rather than a bounded
//! reachability check. The one exception is the address a `REFINEDtpt`
//! registers *for itself* (Dotty's `typeAtAddr(start) = refineCls.typeRef`,
//! Milestone 5d2b): when a reference's target address is itself a
//! `REFINEDtpt` node, that exact identity is entered for the *current*
//! semantic owner ([`enter_reference_target`](TastyUnpickler::enter_reference_target)),
//! never routed through `SemanticType`/`ClassRef` decoding — projection
//! itself treats it as the already-entered refinement class, not a type to
//! decode.
//!
//! ## Memoization includes the mode (§3)
//!
//! The walk is memoized by `(tree, owner, mode)`
//! ([`TastySemanticIndex::first_identity_scan`](crate::index::TastySemanticIndex::first_identity_scan)),
//! not just `(tree, owner)`: the same address can legitimately be visited
//! first under a shallow mode (say `SemanticType`, which does not follow a
//! reference's target) and later under a richer one (`ClassRef`, which does
//! reach `THIS`'s hidden `REFINEDtpt`) for the very same owner, and the first,
//! shallower visit must not suppress the second.
//!
//! ## Owner semantics (§16, §17)
//!
//! A `LAMBDAtpt`/`REFINEDtpt` found through any hidden route is owned by the
//! declared-type position discovery started from — never the selected
//! member, a qualifier's term symbol, a referenced class or a package.
//! Reaching one shared identity from two different owners keeps the first and
//! records the conflict, exactly as before broadened discovery (the
//! `lambda_owner`/`refined_owner` bookkeeping is untouched by this
//! milestone).

use dotty_tasty::tasty::{
    ANDTYPE_TAG, ANNOTATEDTPT_TAG, ANNOTATEDTYPE_TAG, APPLIEDTPT_TAG, APPLIEDTYPE_TAG,
    BYNAMETPT_TAG, BYNAMETYPE_TAG, CLASSCONST_TAG, EXPLICITTPT_TAG, FLEXIBLETYPE_TAG, IDENT_TAG,
    IDENTTPT_TAG, LAMBDATPT_TAG, MATCHCASETYPE_TAG, MATCHTYPE_TAG, METHODTYPE_TAG, ORTYPE_TAG,
    PARAMTYPE_TAG, POLYTYPE_TAG, QUALTHIS_TAG, RECTHIS_TAG, RECTYPE_TAG, REFINEDTPT_TAG,
    REFINEDTYPE_TAG, SELECT_TAG, SELECTTPT_TAG, SHAREDTERM_TAG, SHAREDTYPE_TAG, SINGLETONTPT_TAG,
    SUPERTYPE_TAG, TERMREF_TAG, TERMREFDIRECT_TAG, TERMREFIN_TAG, TERMREFPKG_TAG,
    TERMREFSYMBOL_TAG, THIS_TAG, TYPEBOUNDS_TAG, TYPEBOUNDSTPT_TAG, TYPELAMBDATYPE_TAG,
    TYPEREF_TAG, TYPEREFDIRECT_TAG, TYPEREFIN_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG,
    is_compact_annot_type_tag,
};

use crate::ast_view::{AstView, address};
use crate::enter::MAX_TREE_DEPTH;
use crate::error::UnpickleError;
use crate::term_type::is_type_tree_tag;
use crate::type_tree::is_deferred_tree;
use crate::unpickler::TastyUnpickler;
use dotty_core::ids::SymbolId;

/// Which structural layer of semantic projection an address is being
/// visited under. Part of the discovery memo key (`(tree, owner, mode)`), so
/// the same address may be visited once per mode: see the module
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiscoveryMode {
    /// Mirrors [`type_of_tpt`](TastyUnpickler::type_of_tpt): a `TypeTree`
    /// position (a declared type, a parent, a self type, a lambda body/parent
    /// of a refinement).
    TypeTree,
    /// Mirrors [`type_of_term`](TastyUnpickler::type_of_term): the term tree a
    /// `SELECTtpt` qualifier or a `SINGLETONtpt` reference is made of.
    TermType,
    /// Mirrors `type_at`/`decode_type`: an ordinary semantic type wire node,
    /// reached without building the `Type` itself.
    SemanticType,
    /// Mirrors [`this_class`](TastyUnpickler::this_class): the narrower
    /// grammar of a `THIS`/`QUALTHIS` class argument.
    ClassRef,
}

/// The absolute addresses of `at`'s direct children, in wire order.
fn children_of(ast: &AstView<'_>, at: u32) -> Vec<u32> {
    ast.children(at)
        .iter()
        .map(|child| address(child.offset))
        .collect()
}

fn malformed(address: u32, reason: &'static str) -> UnpickleError {
    UnpickleError::MalformedType { address, reason }
}

impl TastyUnpickler<'_, '_, '_> {
    /// Discovers the identities reachable from the declared type tree of the
    /// definition at `at` (its first child: a type, bounds or right-hand
    /// side), owned by `owner`. The successor of Milestone 5c/5d2b's
    /// `enter_declared_lambdas`.
    pub(crate) fn discover_declared_type_identities(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        if let Some(tree) = ast.children(at).first() {
            self.discover_identities(
                ast,
                address(tree.offset),
                owner,
                DiscoveryMode::TypeTree,
                depth,
            )?;
        }
        Ok(())
    }

    /// Visits `tree` under `mode`, owned by `owner`. `depth` bounds nesting,
    /// links and identity forms alike, exactly as the pre-5d2c walk did; a
    /// tree already visited under this exact `(owner, mode)` is not walked
    /// twice. An address that is not a visible node (an unresolved or
    /// malformed link) is left for projection to report; pass 1 simply stops.
    pub(crate) fn discover_identities(
        &mut self,
        ast: &AstView<'_>,
        tree: u32,
        owner: SymbolId,
        mode: DiscoveryMode,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        if depth > MAX_TREE_DEPTH {
            return Err(malformed(tree, "a type tree nests too deeply"));
        }
        let Some(tag) = ast.tag_at(tree) else {
            return Ok(());
        };
        if !self.index.first_identity_scan(tree, owner, mode) {
            return Ok(());
        }
        match mode {
            DiscoveryMode::TypeTree => self.discover_type_tree(ast, tree, tag, owner, depth),
            DiscoveryMode::TermType => self.discover_term_type(ast, tree, tag, owner, depth),
            DiscoveryMode::SemanticType => {
                self.discover_semantic_type(ast, tree, tag, owner, depth)
            }
            DiscoveryMode::ClassRef => self.discover_class_ref(ast, tree, tag, owner, depth),
        }
    }

    /// Mirrors [`type_of_tpt`](TastyUnpickler::type_of_tpt)'s routing.
    fn discover_type_tree(
        &mut self,
        ast: &AstView<'_>,
        tree: u32,
        tag: u8,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        use DiscoveryMode::{SemanticType, TermType, TypeTree};

        if tag == SHAREDTERM_TAG {
            let target = ast.resolve_shared_term(tree, tree)?;
            return self.discover_identities(ast, target, owner, TypeTree, depth + 1);
        }
        let children = children_of(ast, tree);
        match tag {
            IDENTTPT_TAG => match children.first() {
                Some(&embedded) => {
                    self.discover_identities(ast, embedded, owner, SemanticType, depth + 1)
                }
                None => Ok(()),
            },
            EXPLICITTPT_TAG | BYNAMETPT_TAG | APPLIEDTPT_TAG | TYPEBOUNDSTPT_TAG => {
                for child in children {
                    self.discover_identities(ast, child, owner, TypeTree, depth + 1)?;
                }
                Ok(())
            }
            SELECTTPT_TAG | SINGLETONTPT_TAG => match children.first() {
                Some(&reference) => {
                    self.discover_identities(ast, reference, owner, TermType, depth + 1)
                }
                None => Ok(()),
            },
            ANNOTATEDTPT_TAG => {
                // The base is walked as any other declared type tree. The
                // annotation itself is not: its only type-bearing portion
                // (the annotation class, when the constructor form is used)
                // is a plain class reference in every real annotation the
                // corpora contain (§13 of Milestone 5d2c) — never a
                // structural refinement or a type lambda — so mirroring it
                // would add a full constructor-spine walk for zero measured
                // gain. The corpus oracle (Milestone 5d2c, Commit 5) reports
                // this as zero missing identities, not assumed.
                match children.first() {
                    Some(&base) => self.discover_identities(ast, base, owner, TypeTree, depth + 1),
                    None => Ok(()),
                }
            }
            LAMBDATPT_TAG => self.enter_lambda_tpt(ast, tree, &children, owner, depth),
            REFINEDTPT_TAG => self.enter_refined_tpt(ast, tree, &children, owner, depth),
            // `MATCHtpt`, a `BLOCK` used as a tree, and `HOLE` are not
            // projected yet: their identities need not be discovered either.
            tag if is_deferred_tree(tag) => Ok(()),
            // `readTpt` falls back to `readType` for every other tag.
            _ => self.discover_identities(ast, tree, owner, SemanticType, depth + 1),
        }
    }

    /// Mirrors [`type_of_term`](TastyUnpickler::type_of_term)'s routing.
    fn discover_term_type(
        &mut self,
        ast: &AstView<'_>,
        tree: u32,
        tag: u8,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        use DiscoveryMode::{ClassRef, SemanticType, TermType, TypeTree};

        if tag == SHAREDTERM_TAG {
            let target = ast.resolve_shared_term(tree, tree)?;
            return self.discover_identities(ast, target, owner, TermType, depth + 1);
        }
        if is_type_tree_tag(tag) {
            return self.discover_identities(ast, tree, owner, TypeTree, depth + 1);
        }
        let children = children_of(ast, tree);
        match tag {
            IDENT_TAG => match children.first() {
                Some(&embedded) => {
                    self.discover_identities(ast, embedded, owner, SemanticType, depth + 1)
                }
                None => Ok(()),
            },
            SELECT_TAG => match children.first() {
                Some(&qualifier) => {
                    self.discover_identities(ast, qualifier, owner, TermType, depth + 1)
                }
                None => Ok(()),
            },
            QUALTHIS_TAG => {
                // `QUALTHIS (IDENTtpt Type)`: the class reference is the
                // qualifier `IDENTtpt`'s own embedded type child, exactly the
                // address `type_of_term`'s `QUALTHIS` arm decodes through
                // `this_class`.
                let Some(&qualifier) = children.first() else {
                    return Ok(());
                };
                if ast.tag_at(qualifier) != Some(IDENTTPT_TAG) {
                    return Ok(());
                }
                match children_of(ast, qualifier).first() {
                    Some(&embedded) => {
                        self.discover_identities(ast, embedded, owner, ClassRef, depth + 1)
                    }
                    None => Ok(()),
                }
            }
            // `readTree` falls back to `readType` for every other term tag
            // (`APPLY`, `BLOCK`, `INLINED`, ... are simply not matched here,
            // so the walk stops rather than descending into them — pass 1
            // never becomes a general term-body walker).
            _ => self.discover_identities(ast, tree, owner, SemanticType, depth + 1),
        }
    }

    /// Mirrors `type_at`/`decode_type`'s routing, structurally: it follows
    /// the same AST edges the real decoder would, without allocating a
    /// `Type` or resolving a name.
    fn discover_semantic_type(
        &mut self,
        ast: &AstView<'_>,
        tree: u32,
        tag: u8,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        use DiscoveryMode::{ClassRef, SemanticType};

        if tag == SHAREDTYPE_TAG {
            let Some(target) = shared_type_target(ast, tree)? else {
                return Ok(());
            };
            return self.discover_identities(ast, target, owner, SemanticType, depth + 1);
        }
        let children = children_of(ast, tree);
        match tag {
            // A direct/symbol reference's target is an ordinary definition
            // and is not recursively scanned as one — except when it is
            // itself a `REFINEDtpt` (§11 of the module documentation).
            TYPEREFDIRECT_TAG | TERMREFDIRECT_TAG => {
                self.enter_reference_target(ast, tree, owner, depth)
            }
            TYPEREFSYMBOL_TAG | TERMREFSYMBOL_TAG => {
                // `ASTRef Type` (prefix): the prefix is a semantic type, and
                // the target follows the same reference-target policy.
                if let Some(&prefix) = children.first() {
                    self.discover_identities(ast, prefix, owner, SemanticType, depth + 1)?;
                }
                self.enter_reference_target(ast, tree, owner, depth)
            }
            TYPEREF_TAG | TERMREF_TAG => match children.first() {
                // `NameRef Type` (prefix): the member is not resolved by
                // pass 1 (§15), only its prefix's own reachability.
                Some(&prefix) => {
                    self.discover_identities(ast, prefix, owner, SemanticType, depth + 1)
                }
                None => Ok(()),
            },
            TYPEREFPKG_TAG | TERMREFPKG_TAG => Ok(()),
            THIS_TAG => match children.first() {
                Some(&class) => self.discover_identities(ast, class, owner, ClassRef, depth + 1),
                None => Ok(()),
            },
            // `RECthis`/`PARAMtype` name a binder's own `TypeId`, reserved
            // and decoded by the type layer itself: not a pass-1 identity
            // (§12 of the module documentation).
            RECTHIS_TAG | PARAMTYPE_TAG => Ok(()),
            APPLIEDTYPE_TAG | ANDTYPE_TAG | ORTYPE_TAG | SUPERTYPE_TAG | MATCHCASETYPE_TAG
            | MATCHTYPE_TAG | TYPEBOUNDS_TAG | TYPELAMBDATYPE_TAG | POLYTYPE_TAG
            | METHODTYPE_TAG | RECTYPE_TAG | TYPEREFIN_TAG | TERMREFIN_TAG | REFINEDTYPE_TAG
            | FLEXIBLETYPE_TAG | BYNAMETYPE_TAG => {
                for child in children {
                    self.discover_identities(ast, child, owner, SemanticType, depth + 1)?;
                }
                Ok(())
            }
            CLASSCONST_TAG => match children.first() {
                Some(&class) => {
                    self.discover_identities(ast, class, owner, SemanticType, depth + 1)
                }
                None => Ok(()),
            },
            ANNOTATEDTYPE_TAG => {
                let [underlying, annotation] = children[..] else {
                    return Ok(());
                };
                self.discover_identities(ast, underlying, owner, SemanticType, depth + 1)?;
                // Only the compact (type-only) annotation form is a semantic
                // type; the full constructor form (`APPLY`/`NEW`, possibly
                // through `SHAREDterm`) is a term and is not walked, per the
                // same reasoning as `ANNOTATEDtpt` above.
                if let Some((root, root_tag)) = annotation_root(ast, annotation)?
                    && is_compact_annot_type_tag(root_tag)
                {
                    self.discover_identities(ast, root, owner, SemanticType, depth + 1)?;
                }
                Ok(())
            }
            // Every other tag (a constant, `PARAMtype`'s own envelope
            // already handled above, ...) has no reachability of its own.
            _ => Ok(()),
        }
    }

    /// Mirrors [`this_class`](TastyUnpickler::this_class)'s narrower grammar.
    fn discover_class_ref(
        &mut self,
        ast: &AstView<'_>,
        tree: u32,
        tag: u8,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        match tag {
            TYPEREFDIRECT_TAG | TYPEREFSYMBOL_TAG => {
                self.enter_reference_target(ast, tree, owner, depth)
            }
            TYPEREFPKG_TAG => Ok(()),
            SHAREDTYPE_TAG => {
                let Some(target) = shared_type_target(ast, tree)? else {
                    return Ok(());
                };
                self.discover_identities(ast, target, owner, DiscoveryMode::ClassRef, depth + 1)
            }
            TYPEREF_TAG => match children_of(ast, tree).first() {
                // The prefix of an external `this`; the member is not
                // resolved here (§15), the same policy `TYPEREF`/`TERMREF`
                // follow in `SemanticType` mode.
                Some(&prefix) => self.discover_identities(
                    ast,
                    prefix,
                    owner,
                    DiscoveryMode::SemanticType,
                    depth + 1,
                ),
                None => Ok(()),
            },
            // The critical case (§10 of the module documentation): a `THIS`
            // reached a `SHAREDtype` whose target is the `REFINEDtpt` node
            // itself (Dotty's `typeAtAddr(start) = refineCls.typeRef`). The
            // synthetic class is entered directly; it is never routed
            // through `SemanticType` decoding.
            REFINEDTPT_TAG => self.enter_referenced_refined_tpt(ast, tree, owner, depth),
            _ => Ok(()),
        }
    }

    /// A direct/symbol reference's target address is an ordinary
    /// definition, which discovery does not recursively scan as a second
    /// declared-type position (§11 of the module documentation) — except
    /// when the target is itself a `REFINEDtpt` node: Milestone 5d2b's pass 1
    /// registers that exact address as the synthetic refinement class's own
    /// identity (mirroring Dotty's `typeAtAddr(start) = refineCls.typeRef`),
    /// so a reference that resolves to it must cause that identity to be
    /// entered for the *current* owner, the same as reaching it through any
    /// other route.
    fn enter_reference_target(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        let Some(target) = reference_target(ast, at)? else {
            return Ok(());
        };
        if ast.tag_at(target) == Some(REFINEDTPT_TAG) {
            return self.enter_referenced_refined_tpt(ast, target, owner, depth);
        }
        Ok(())
    }

    /// Enters the `REFINEDtpt` at `tree` for `owner`, the way a *reference*
    /// to it — never a declared-type position of its own — is allowed to:
    /// Dotty's `symAtAddr.getOrElse(start, newRefinedClassSymbol(...))` gets
    /// the already-registered class when one exists and only creates a fresh
    /// one when none does. A member of a refinement referring to its own
    /// enclosing refinement (an explicit `this.type`, or a sibling member
    /// name resolved relative to the structural instance — both encoded as
    /// `THIS` of the refinement's own synthetic class, see [`crate::types`])
    /// is exactly this case: the identity was already entered, by the
    /// refinement's *own* declared-type position, before any member's type is
    /// scanned (Milestone 5d2b's two-stage entering), so this call must
    /// simply resolve to it — never re-derive ownership from the member that
    /// happens to mention it, which would misreport a genuine self-reference
    /// as a cross-owner conflict.
    ///
    /// Only when the address has *no* entry yet (a genuinely hidden route
    /// discovering it for the first time, e.g. the real library self-types
    /// Milestone 5d2c closes) is it entered fresh, owned by `owner` — the
    /// declared-type position discovery began from, per §16.
    fn enter_referenced_refined_tpt(
        &mut self,
        ast: &AstView<'_>,
        tree: u32,
        owner: SymbolId,
        depth: usize,
    ) -> Result<(), UnpickleError> {
        if self.index.symbol_at(tree).is_some() {
            return Ok(());
        }
        let children = children_of(ast, tree);
        self.enter_refined_tpt(ast, tree, &children, owner, depth)
    }
}

/// The target address a `TYPEREFdirect`/`TERMREFdirect`/`TYPEREFsymbol`/
/// `TERMREFsymbol` at `at` names, if any. Never itself a declared-type scan
/// target (see [`TastyUnpickler::enter_reference_target`]); only used to
/// detect the one case (§11) where the target is a `REFINEDtpt`'s own
/// address.
fn reference_target(ast: &AstView<'_>, at: u32) -> Result<Option<u32>, UnpickleError> {
    use dotty_tasty::tasty::{RawTree, TermValue};
    Ok(match ast.tree_at(at, at)? {
        RawTree::Leaf(term) => match term.value {
            TermValue::AstRef(target)
                if matches!(term.tag, TYPEREFDIRECT_TAG | TERMREFDIRECT_TAG) =>
            {
                Some(target)
            }
            _ => None,
        },
        RawTree::NatAst {
            value,
            tag: TYPEREFSYMBOL_TAG | TERMREFSYMBOL_TAG,
            ..
        } => Some(value),
        _ => None,
    })
}

/// The target address a `SHAREDtype` at `at` names, if it decodes to one
/// (the format's only valid shape for it).
fn shared_type_target(ast: &AstView<'_>, at: u32) -> Result<Option<u32>, UnpickleError> {
    use dotty_tasty::tasty::{RawTree, TermValue};
    Ok(match ast.tree_at(at, at)? {
        RawTree::Leaf(term) if term.tag == SHAREDTYPE_TAG => match term.value {
            TermValue::AstRef(target) => Some(target),
            _ => None,
        },
        _ => None,
    })
}

/// The address and tag of an annotation tree's root, following a
/// `SHAREDterm` link when there is one — the same classification
/// [`crate::annotated`]'s `decode_annotation_tree` makes, without decoding
/// anything. `None` when `at` is not a visible node.
fn annotation_root(ast: &AstView<'_>, at: u32) -> Result<Option<(u32, u8)>, UnpickleError> {
    let Some(tag) = ast.tag_at(at) else {
        return Ok(None);
    };
    if tag != SHAREDTERM_TAG {
        return Ok(Some((at, tag)));
    }
    let target = ast.resolve_shared_term(at, at)?;
    Ok(ast.tag_at(target).map(|tag| (target, tag)))
}
