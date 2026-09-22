//! The post-hoc identity-reachability oracle (Milestone 5d2c's follow-up
//! review of issue #101): an independent cross-check, run after
//! [`TastyUnpickler::enter_symbols`], of whether pass-1 discovery
//! ([`crate::discovery`]) really has parity with semantic projection.
//!
//! Discovery's own tests exercise the routing table from the *inside*: they
//! build a fixture with a known hidden route and check that the identity at
//! its end is entered. This module instead starts from the *wire*, in two
//! independent steps:
//!
//! 1. [`reachable_identities`] structurally enumerates every declared-type
//!    root physically present in the file (a `VALDEF`/`TYPEDEF`'s own
//!    declared type, a `DEFDEF`'s result, a `TYPEPARAM`/`PARAM`'s bounds, a
//!    template's parents and self type), then walks from each root through
//!    the *same* tag-dispatch routing discovery's own four modes follow
//!    ([`crate::discovery`]'s module documentation) — mirrored here as a
//!    second, independent implementation, not by calling discovery's own
//!    functions, so a bug in discovery's dispatch cannot also hide from this
//!    check. The result is the address of every `LAMBDAtpt`/`REFINEDtpt`
//!    discovery *should* be able to reach.
//! 2. [`identity_reachability`] compares that set against every
//!    `LAMBDAtpt`/`REFINEDtpt` node physically present in the AST section and
//!    the real `index` a completed `enter_symbols` produced, and classifies
//!    each one as `Entered` (has an owner), `OutOfScope` (not in the
//!    reachable set — a local definition's declared type or a pattern
//!    binder's body, or a node only reachable through an unsupported term
//!    shape such as an `APPLY` argument, either way not a position pass 1
//!    documents as ever entering), or `Unaccounted` (in the reachable set but
//!    has no owner — always a genuine parity gap, never an expected shape).
//!
//! Run across a whole corpus, this is the "reachable vs. entered" measurement
//! issue #101's review asked for; the per-unit call is exact, not a sample.

use std::collections::HashSet;

use dotty_tasty::tasty::{
    ANDTYPE_TAG, ANNOTATEDTPT_TAG, ANNOTATEDTYPE_TAG, APPLIEDTPT_TAG, APPLIEDTYPE_TAG, APPLY_TAG,
    BLOCK_TAG, BYNAMETPT_TAG, BYNAMETYPE_TAG, CLASSCONST_TAG, DEFDEF_TAG, EMPTYCLAUSE_TAG,
    EXPLICITTPT_TAG, FLEXIBLETYPE_TAG, IDENT_TAG, IDENTTPT_TAG, LAMBDATPT_TAG, MATCHCASETYPE_TAG,
    MATCHTYPE_TAG, METHODTYPE_TAG, NEW_TAG, ORTYPE_TAG, PACKAGE_TAG, PARAM_TAG, PARAMTYPE_TAG,
    POLYTYPE_TAG, QUALTHIS_TAG, RECTHIS_TAG, RECTYPE_TAG, REFINEDTPT_TAG, REFINEDTYPE_TAG,
    SELECT_TAG, SELECTIN_TAG, SELECTTPT_TAG, SHAREDTERM_TAG, SHAREDTYPE_TAG, SINGLETONTPT_TAG,
    SPLITCLAUSE_TAG, SUPERTYPE_TAG, TEMPLATE_TAG, TERMREF_TAG, TERMREFDIRECT_TAG, TERMREFIN_TAG,
    TERMREFPKG_TAG, TERMREFSYMBOL_TAG, THIS_TAG, TYPEAPPLY_TAG, TYPEBOUNDS_TAG, TYPEBOUNDSTPT_TAG,
    TYPEDEF_TAG, TYPELAMBDATYPE_TAG, TYPEPARAM_TAG, TYPEREF_TAG, TYPEREFDIRECT_TAG, TYPEREFIN_TAG,
    TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG, TastyFile, VALDEF_TAG, is_compact_annot_type_tag,
};

use crate::ast_view::{AstView, address};
use crate::class::{parent_constructor_is_applied, template_parts};
use crate::discovery::{annotation_root, children_of, reference_target};
use crate::enter::MAX_TREE_DEPTH;
use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;
use crate::term_type::is_type_tree_tag;
use crate::type_tree::is_deferred_tree;

/// Which of discovery's four modes an address is walked under here, mirroring
/// [`crate::discovery::DiscoveryMode`] structurally — a separate type, even
/// though the two enums are identical, so this module never depends on
/// discovery's own definition changing shape under it unnoticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Mode {
    TypeTree,
    TermType,
    SemanticType,
    ClassRef,
}

/// What the entered state and structural reachability of one
/// `LAMBDAtpt`/`REFINEDtpt` node say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityOutcome {
    /// Discovery entered it: it has an owner (`lambda_owner`/`refined_owner`
    /// / `symbol_at`), possibly alongside a recorded owner conflict.
    Entered,
    /// Not entered, and `reachable_identities` never reaches it either: a
    /// local definition's declared type or a pattern binder's (pass 1 never
    /// starts a walk there at all), or a node only reachable through an
    /// unsupported term shape — an `APPLY` argument, a `BLOCK` used as a
    /// tree, ... — that discovery's own routing tables have no arm for.
    /// Neither is a gap: discovery correctly never reaches this address.
    OutOfScope,
    /// Not entered, but `reachable_identities` does reach it: some
    /// supported declared-type route leads here and discovery's own
    /// dispatch, mirrored independently by this module, would follow it.
    /// Always a genuine parity gap when it occurs; a corpus run with any of
    /// these disproves the milestone's own claim, rather than merely leaving
    /// it unmeasured.
    Unaccounted,
}

/// One `LAMBDAtpt`/`REFINEDtpt` node found on the wire, and its
/// [`IdentityOutcome`].
#[derive(Debug, Clone, Copy)]
pub struct IdentityNode {
    pub address: u32,
    pub tag: u8,
    pub outcome: IdentityOutcome,
}

fn malformed(address: u32, reason: &'static str) -> UnpickleError {
    UnpickleError::MalformedType { address, reason }
}

/// Records `tree` as reachable and walks into it under `mode`, following
/// discovery's own memoization discipline (`(tree, mode)`, not just `tree`):
/// see [`crate::discovery::DiscoveryMode`]'s documentation for why the mode
/// is part of the key.
fn walk(
    ast: &AstView<'_>,
    tree: u32,
    mode: Mode,
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    if depth > MAX_TREE_DEPTH {
        return Err(malformed(tree, "a type tree nests too deeply"));
    }
    let Some(tag) = ast.tag_at(tree) else {
        return Err(UnpickleError::InvalidReferenceTarget {
            from: tree,
            to: tree,
        });
    };
    if !visited.insert((tree, mode)) {
        return Ok(());
    }
    match mode {
        Mode::TypeTree => walk_type_tree(ast, tree, tag, depth, visited, reachable),
        Mode::TermType => walk_term_type(ast, tree, tag, depth, visited, reachable),
        Mode::SemanticType => walk_semantic_type(ast, tree, tag, depth, visited, reachable),
        Mode::ClassRef => walk_class_ref(ast, tree, tag, depth, visited, reachable),
    }
}

/// Mirrors [`crate::unpickler::TastyUnpickler::discover_type_tree`].
fn walk_type_tree(
    ast: &AstView<'_>,
    tree: u32,
    tag: u8,
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    if tag == SHAREDTERM_TAG {
        let target = ast.resolve_shared_term(tree, tree)?;
        return walk(ast, target, Mode::TypeTree, depth + 1, visited, reachable);
    }
    let children = children_of(ast, tree);
    match tag {
        IDENTTPT_TAG => match children.first() {
            Some(&embedded) => walk(
                ast,
                embedded,
                Mode::SemanticType,
                depth + 1,
                visited,
                reachable,
            ),
            None => Ok(()),
        },
        EXPLICITTPT_TAG | BYNAMETPT_TAG | APPLIEDTPT_TAG | TYPEBOUNDSTPT_TAG => {
            for child in children {
                walk(ast, child, Mode::TypeTree, depth + 1, visited, reachable)?;
            }
            Ok(())
        }
        SELECTTPT_TAG | SINGLETONTPT_TAG => match children.first() {
            Some(&reference) => walk(
                ast,
                reference,
                Mode::TermType,
                depth + 1,
                visited,
                reachable,
            ),
            None => Ok(()),
        },
        ANNOTATEDTPT_TAG => match children.first() {
            Some(&base) => walk(ast, base, Mode::TypeTree, depth + 1, visited, reachable),
            None => Ok(()),
        },
        LAMBDATPT_TAG => walk_lambda_tpt(ast, tree, &children, depth, visited, reachable),
        REFINEDTPT_TAG => walk_refined_tpt(ast, tree, &children, depth, visited, reachable),
        tag if is_deferred_tree(tag) => Ok(()),
        _ => walk(ast, tree, Mode::SemanticType, depth + 1, visited, reachable),
    }
}

/// Mirrors [`crate::unpickler::TastyUnpickler::discover_term_type`].
fn walk_term_type(
    ast: &AstView<'_>,
    tree: u32,
    tag: u8,
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    if tag == SHAREDTERM_TAG {
        let target = ast.resolve_shared_term(tree, tree)?;
        return walk(ast, target, Mode::TermType, depth + 1, visited, reachable);
    }
    if is_type_tree_tag(tag) {
        return walk(ast, tree, Mode::TypeTree, depth + 1, visited, reachable);
    }
    let children = children_of(ast, tree);
    match tag {
        IDENT_TAG => match children.first() {
            Some(&embedded) => walk(
                ast,
                embedded,
                Mode::SemanticType,
                depth + 1,
                visited,
                reachable,
            ),
            None => Ok(()),
        },
        SELECT_TAG => match children.first() {
            Some(&qualifier) => walk(
                ast,
                qualifier,
                Mode::TermType,
                depth + 1,
                visited,
                reachable,
            ),
            None => Ok(()),
        },
        QUALTHIS_TAG => {
            let Some(&qualifier) = children.first() else {
                return Ok(());
            };
            if ast.tag_at(qualifier) != Some(IDENTTPT_TAG) {
                return Ok(());
            }
            match children_of(ast, qualifier).first() {
                Some(&embedded) => {
                    walk(ast, embedded, Mode::ClassRef, depth + 1, visited, reachable)
                }
                None => Ok(()),
            }
        }
        _ => walk(ast, tree, Mode::SemanticType, depth + 1, visited, reachable),
    }
}

/// Mirrors [`crate::unpickler::TastyUnpickler::discover_semantic_type`].
fn walk_semantic_type(
    ast: &AstView<'_>,
    tree: u32,
    tag: u8,
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    if tag == SHAREDTYPE_TAG {
        let target = ast.resolve_shared_type(tree, tree)?;
        return walk(
            ast,
            target,
            Mode::SemanticType,
            depth + 1,
            visited,
            reachable,
        );
    }
    let children = children_of(ast, tree);
    match tag {
        TYPEREFDIRECT_TAG | TERMREFDIRECT_TAG => {
            walk_reference_target(ast, tree, depth, visited, reachable)
        }
        TYPEREFSYMBOL_TAG | TERMREFSYMBOL_TAG => {
            if let Some(&prefix) = children.first() {
                walk(
                    ast,
                    prefix,
                    Mode::SemanticType,
                    depth + 1,
                    visited,
                    reachable,
                )?;
            }
            walk_reference_target(ast, tree, depth, visited, reachable)
        }
        TYPEREF_TAG | TERMREF_TAG => match children.first() {
            Some(&prefix) => walk(
                ast,
                prefix,
                Mode::SemanticType,
                depth + 1,
                visited,
                reachable,
            ),
            None => Ok(()),
        },
        TYPEREFPKG_TAG | TERMREFPKG_TAG => Ok(()),
        THIS_TAG => match children.first() {
            Some(&class) => walk(ast, class, Mode::ClassRef, depth + 1, visited, reachable),
            None => Ok(()),
        },
        RECTHIS_TAG | PARAMTYPE_TAG => Ok(()),
        APPLIEDTYPE_TAG | ANDTYPE_TAG | ORTYPE_TAG | SUPERTYPE_TAG | MATCHCASETYPE_TAG
        | MATCHTYPE_TAG | TYPEBOUNDS_TAG | TYPELAMBDATYPE_TAG | POLYTYPE_TAG | METHODTYPE_TAG
        | RECTYPE_TAG | TYPEREFIN_TAG | TERMREFIN_TAG | REFINEDTYPE_TAG | FLEXIBLETYPE_TAG
        | BYNAMETYPE_TAG => {
            for child in children {
                walk(
                    ast,
                    child,
                    Mode::SemanticType,
                    depth + 1,
                    visited,
                    reachable,
                )?;
            }
            Ok(())
        }
        CLASSCONST_TAG => match children.first() {
            Some(&class) => walk(
                ast,
                class,
                Mode::SemanticType,
                depth + 1,
                visited,
                reachable,
            ),
            None => Ok(()),
        },
        ANNOTATEDTYPE_TAG => {
            let [underlying, annotation] = children[..] else {
                return Ok(());
            };
            walk(
                ast,
                underlying,
                Mode::SemanticType,
                depth + 1,
                visited,
                reachable,
            )?;
            if let Some((root, root_tag)) = annotation_root(ast, annotation)?
                && is_compact_annot_type_tag(root_tag)
            {
                walk(ast, root, Mode::SemanticType, depth + 1, visited, reachable)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Mirrors [`crate::unpickler::TastyUnpickler::discover_class_ref`].
fn walk_class_ref(
    ast: &AstView<'_>,
    tree: u32,
    tag: u8,
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    match tag {
        TYPEREFDIRECT_TAG | TYPEREFSYMBOL_TAG => {
            walk_reference_target(ast, tree, depth, visited, reachable)
        }
        TYPEREFPKG_TAG => Ok(()),
        SHAREDTYPE_TAG => {
            let target = ast.resolve_shared_type(tree, tree)?;
            walk(ast, target, Mode::ClassRef, depth + 1, visited, reachable)
        }
        TYPEREF_TAG => match children_of(ast, tree).first() {
            Some(&prefix) => walk(
                ast,
                prefix,
                Mode::SemanticType,
                depth + 1,
                visited,
                reachable,
            ),
            None => Ok(()),
        },
        REFINEDTPT_TAG => {
            let children = children_of(ast, tree);
            walk_refined_tpt(ast, tree, &children, depth, visited, reachable)
        }
        _ => Ok(()),
    }
}

/// Mirrors [`crate::unpickler::TastyUnpickler::enter_reference_target`] (§11
/// of `discovery.rs`'s module documentation): a direct/symbol reference's
/// target is not itself a declared-type position, except when it is a
/// `REFINEDtpt` node, which is entered for the referencing owner exactly as
/// if reached by any other route.
fn walk_reference_target(
    ast: &AstView<'_>,
    at: u32,
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    let target = reference_target(ast, at)?;
    if ast.tag_at(target) == Some(REFINEDTPT_TAG) {
        let children = children_of(ast, target);
        return walk_refined_tpt(ast, target, &children, depth, visited, reachable);
    }
    Ok(())
}

/// Mirrors [`crate::unpickler::TastyUnpickler::enter_lambda_tpt`]: every
/// `TYPEPARAM`'s bounds, then the trailing body, both declared-type positions
/// of their own.
fn walk_lambda_tpt(
    ast: &AstView<'_>,
    tree: u32,
    children: &[u32],
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    reachable.insert(tree);
    let Some((body, params)) = children.split_last() else {
        return Ok(());
    };
    for param in params {
        if let Some(&bounds) = children_of(ast, *param).first() {
            walk(ast, bounds, Mode::TypeTree, depth + 1, visited, reachable)?;
        }
    }
    walk(ast, *body, Mode::TypeTree, depth + 1, visited, reachable)
}

/// Mirrors [`crate::unpickler::TastyUnpickler::enter_refined_tpt`]'s
/// reachability: the parent type tree, a declared-type position of its own.
/// The refinement's own members are not walked from here — every `VALDEF`/
/// `DEFDEF`/`TYPEDEF` in the file is already its own root (see
/// [`declared_type_roots`]), refinement member or not.
fn walk_refined_tpt(
    ast: &AstView<'_>,
    tree: u32,
    children: &[u32],
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    reachable.insert(tree);
    let Some((&parent, _stats)) = children.split_first() else {
        return Ok(());
    };
    walk(ast, parent, Mode::TypeTree, depth + 1, visited, reachable)
}

/// Mirrors [`crate::unpickler::TastyUnpickler::enter_parent_lambdas`]'s routing
/// exactly (a call's function only, a type application's function and type
/// arguments, a constructor selection's `NEW` type, anything else read as an
/// ordinary declared type tree), without entering anything — only marking
/// what it would reach.
fn walk_parent(
    ast: &AstView<'_>,
    at: u32,
    depth: usize,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    if depth > MAX_TREE_DEPTH {
        return Err(malformed(at, "a parent tree nests too deeply"));
    }
    let Ok(at) = ast.resolve_shared_term(at, at) else {
        return Ok(());
    };
    let children = children_of(ast, at);
    match ast.tag_at(at) {
        Some(APPLY_TAG | BLOCK_TAG) => match children.first() {
            Some(&function) => walk_parent(ast, function, depth + 1, visited, reachable),
            None => Ok(()),
        },
        Some(TYPEAPPLY_TAG) => {
            let Some((&function, arguments)) = children.split_first() else {
                return Ok(());
            };
            walk_parent(ast, function, depth + 1, visited, reachable)?;
            if !parent_constructor_is_applied(ast, function) {
                for &argument in arguments {
                    walk(ast, argument, Mode::TypeTree, depth + 1, visited, reachable)?;
                }
            }
            Ok(())
        }
        Some(SELECTIN_TAG) => match children.first() {
            Some(&new) if ast.tag_at(new) == Some(NEW_TAG) => {
                for tpt in children_of(ast, new) {
                    walk(ast, tpt, Mode::TypeTree, depth + 1, visited, reachable)?;
                }
                Ok(())
            }
            _ => Ok(()),
        },
        _ => walk(ast, at, Mode::TypeTree, depth + 1, visited, reachable),
    }
}

/// The `TYPEPARAM`/`PARAM` nodes directly under `parent_at`, each walked
/// under its own bounds as a declared-type position — mirrors
/// [`crate::unpickler::TastyUnpickler::enter_parameters`]'s flat, immediate-
/// children-only scan (never a deep search: a parameter belongs to the
/// definition or template that lists it directly, never one further down).
fn walk_parameters(
    ast: &AstView<'_>,
    parent_at: u32,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    for child in ast.children(parent_at) {
        if !matches!(child.tag, TYPEPARAM_TAG | PARAM_TAG) {
            continue;
        }
        if let Some(&bounds) = children_of(ast, address(child.offset)).first() {
            walk(ast, bounds, Mode::TypeTree, 0, visited, reachable)?;
        }
    }
    Ok(())
}

/// A `TYPEDEF`/`VALDEF`/`DEFDEF` member's own declared-type root(s), mirroring
/// [`crate::unpickler::TastyUnpickler::enter_definition_body`]'s dispatch
/// exactly: a class (`TYPEDEF` with a `TEMPLATE` first child) recurses into
/// [`walk_template`]; a type alias or `VALDEF` walks its first child; a
/// `DEFDEF` walks its own parameters, then its result (the first child that
/// is not a parameter or clause marker). Never walks a member's right-hand
/// side/body — the position `enter_definition_body` itself never reads.
fn walk_member(
    ast: &AstView<'_>,
    at: u32,
    tag: u8,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    match tag {
        TYPEDEF_TAG => {
            let first = ast.children(at).first().copied();
            if first.is_some_and(|child| child.tag == TEMPLATE_TAG) {
                walk_template(ast, address(first.unwrap().offset), visited, reachable)
            } else if let Some(child) = first {
                walk(
                    ast,
                    address(child.offset),
                    Mode::TypeTree,
                    0,
                    visited,
                    reachable,
                )
            } else {
                Ok(())
            }
        }
        VALDEF_TAG => match children_of(ast, at).first() {
            Some(&first) => walk(ast, first, Mode::TypeTree, 0, visited, reachable),
            None => Ok(()),
        },
        DEFDEF_TAG => {
            walk_parameters(ast, at, visited, reachable)?;
            if let Some(result) = ast
                .children(at)
                .iter()
                .find(|child| {
                    !matches!(
                        child.tag,
                        TYPEPARAM_TAG | PARAM_TAG | EMPTYCLAUSE_TAG | SPLITCLAUSE_TAG
                    )
                })
                .map(|child| address(child.offset))
            {
                walk(ast, result, Mode::TypeTree, 0, visited, reachable)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A class's template: its own header parameters, then every immediate
/// `TYPEDEF`/`VALDEF`/`DEFDEF` member (recursively, so a nested class's own
/// members are reached too), then its parents and an explicit self type.
/// Mirrors [`crate::unpickler::TastyUnpickler::enter_template`] exactly —
/// only the members [`template_parts`] itself would iterate, never a
/// statement or a local definition buried inside one of those members' own
/// right-hand side.
fn walk_template(
    ast: &AstView<'_>,
    template_at: u32,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    walk_parameters(ast, template_at, visited, reachable)?;
    for child in ast.children(template_at) {
        if matches!(child.tag, TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG) {
            walk_member(ast, address(child.offset), child.tag, visited, reachable)?;
        }
    }
    let parts = template_parts(ast, template_at);
    for parent in parts.parents {
        walk_parent(ast, parent, 0, visited, reachable)?;
    }
    if let Some(self_def) = parts.self_def
        && let Some(&tree) = children_of(ast, self_def).first()
    {
        walk(ast, tree, Mode::TypeTree, 0, visited, reachable)?;
    }
    Ok(())
}

/// A package's own members, mirroring
/// [`crate::unpickler::TastyUnpickler::enter_package`]'s dispatch: a nested
/// `PACKAGE` recurses, a `TYPEDEF`/`VALDEF`/`DEFDEF` is a member walked by
/// [`walk_member`]. Anything else (an import, a non-definition statement) is
/// not itself a declared-type position and is skipped, exactly as
/// `enter_package` skips it.
fn walk_package(
    ast: &AstView<'_>,
    at: u32,
    visited: &mut HashSet<(u32, Mode)>,
    reachable: &mut HashSet<u32>,
) -> Result<(), UnpickleError> {
    for child in ast.children(at) {
        let child_at = address(child.offset);
        match child.tag {
            PACKAGE_TAG => walk_package(ast, child_at, visited, reachable)?,
            TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG => {
                walk_member(ast, child_at, child.tag, visited, reachable)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Every `LAMBDAtpt`/`REFINEDtpt` address discovery's own dispatch tables —
/// mirrored independently above, not called — can reach, starting from the
/// file's top-level `PACKAGE` nodes and following exactly the member/
/// parameter/parent/self-type topology
/// [`crate::unpickler::TastyUnpickler::enter_all`]/`enter_package`/
/// `enter_template`/`enter_definition_body` themselves walk. A local
/// definition buried inside some member's own right-hand side is never
/// walked into by any of those real functions, so it is correctly never a
/// root here either — unlike scanning the whole file for every `VALDEF`/
/// `TYPEDEF`/`DEFDEF`/`TYPEPARAM`/`PARAM` tag regardless of position, which
/// would wrongly treat a local definition's own declared type as reachable.
fn reachable_identities(
    ast: &AstView<'_>,
    file: &TastyFile<'_>,
) -> Result<HashSet<u32>, UnpickleError> {
    let mut visited = HashSet::new();
    let mut reachable = HashSet::new();
    for node in file.asts()?.iter() {
        if node.tag == PACKAGE_TAG {
            walk_package(ast, address(node.offset), &mut visited, &mut reachable)?;
        }
    }
    Ok(reachable)
}

/// Every `LAMBDAtpt`/`REFINEDtpt` node in `file`'s AST section, classified
/// against `index` (the result of a completed
/// [`enter_symbols`](crate::unpickler::TastyUnpickler::enter_symbols) call)
/// and the independently, structurally computed `reachable_identities`.
pub fn identity_reachability(
    file: &TastyFile<'_>,
    index: &TastySemanticIndex,
) -> Result<Vec<IdentityNode>, UnpickleError> {
    let ast = AstView::new(file)?;
    let reachable = reachable_identities(&ast, file)?;
    let address_index = file.ast_address_index()?;
    let mut results = Vec::new();
    for tag in [LAMBDATPT_TAG, REFINEDTPT_TAG] {
        for node in address_index.iter_nodes_with_tag(tag) {
            let node_address = address(node.offset);
            let entered = if tag == LAMBDATPT_TAG {
                index.lambda_owner(node_address).is_some()
            } else {
                index.symbol_at(node_address).is_some()
            };
            let outcome = if entered {
                IdentityOutcome::Entered
            } else if reachable.contains(&node_address) {
                IdentityOutcome::Unaccounted
            } else {
                IdentityOutcome::OutOfScope
            };
            results.push(IdentityNode {
                address: node_address,
                tag,
                outcome,
            });
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::names::{Name, Namespace};
    use dotty_core::store::SemanticStore;
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};

    fn nat(value: u8) -> u8 {
        0x80 | value
    }

    fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag, nat(u8::try_from(payload.len()).unwrap())];
        bytes.extend(payload);
        bytes
    }

    /// A leaf tree: a tag directly followed by an inline `Nat`, no length
    /// prefix and no children (`TERMREFpkg`, `SHAREDterm`, ...).
    fn leaf(tag: u8, value: u8) -> Vec<u8> {
        vec![tag, nat(value)]
    }

    /// A top-level `PACKAGE` wrapping `member` as its sole statement: the
    /// real entry point [`reachable_identities`] walks from
    /// (`crate::unpickler::TastyUnpickler::enter_all`'s own top-level scan),
    /// since only a `TYPEDEF`/`VALDEF`/`DEFDEF` reached through a `PACKAGE`
    /// this way is a genuine declared-type root.
    fn package(member: &[u8]) -> Vec<u8> {
        let path = leaf(TERMREFPKG_TAG, 0);
        node(PACKAGE_TAG, &[path, member.to_vec()].concat())
    }

    /// Wraps a hand-built AST section into a minimal, otherwise-empty TASTy
    /// file, the way each fixture below needs.
    fn build_file(ast: &[u8]) -> Vec<u8> {
        let names = NameTable::from_entries(vec![RawName::Utf8("ASTs".to_owned())]).unwrap();
        TastyFile::from_parts(
            Header {
                major_version: 28,
                minor_version: 9,
                experimental_version: 0,
                tooling_version: "Scala 3.9.0".to_owned(),
                uuid: [0; 16],
            },
            names,
            SectionTable::from_sections(vec![Section::new(0, ast)]),
        )
        .unwrap()
        .encode()
        .unwrap()
    }

    /// Three top-level nodes: a `REFINEDtpt` this test enters (so it has a
    /// symbol), a `REFINEDtpt` nested inside a `BLOCK` that is never entered,
    /// and a bare `REFINEDtpt` that is neither — none reachable from any root
    /// (there are no `VALDEF`/`DEFDEF`/`TEMPLATE` nodes in this fixture at
    /// all), so both unentered ones are `OutOfScope`.
    fn file() -> Vec<u8> {
        let mut ast = node(REFINEDTPT_TAG, &[]); // address 0: entered
        ast.extend(node(BLOCK_TAG, &node(REFINEDTPT_TAG, &[]))); // nested in a BLOCK
        ast.extend(node(REFINEDTPT_TAG, &[])); // bare, never entered
        build_file(&ast)
    }

    fn allocate_symbol(store: &mut SemanticStore) -> dotty_core::ids::SymbolId {
        let name = Name::new(store.names.intern("Refinement"), Namespace::Type);
        store.symbols.alloc(Symbol {
            name,
            owner: None,
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    #[test]
    fn an_entered_refinedtpt_is_reported_entered() {
        let bytes = file();
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let mut store = SemanticStore::new();
        let symbol = allocate_symbol(&mut store);
        let mut index = TastySemanticIndex::new();
        index.insert_symbol(0, symbol).unwrap();

        let report = identity_reachability(&parsed, &index).unwrap();

        let entered = report.iter().find(|node| node.address == 0).unwrap();
        assert_eq!(entered.outcome, IdentityOutcome::Entered);
    }

    #[test]
    fn an_unentered_refinedtpt_with_no_root_reaching_it_is_out_of_scope() {
        let bytes = file();
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        // Every unentered `REFINEDtpt` in `file()` — nested in the `BLOCK`
        // and the bare final one alike — is `OutOfScope`: neither is
        // reachable, since the fixture has no `VALDEF`/`DEFDEF`/`TEMPLATE`
        // root that could reach either of them.
        let unentered: Vec<_> = report.iter().filter(|node| node.address != 0).collect();
        assert_eq!(unentered.len(), 2, "{report:?}");
        for node in unentered {
            assert_eq!(node.outcome, IdentityOutcome::OutOfScope, "{report:?}");
        }
    }

    /// Regression for the review of Commit 5's oracle (issue #101): a
    /// `REFINEDtpt` sitting exactly at a `VALDEF`'s declared-type child —
    /// precisely the position `discover_declared_type_identities` enters
    /// directly — must be `Unaccounted` when discovery never entered it, not
    /// `OutOfScope`. The old heuristic tested only "is any ancestor a
    /// `VALDEF`", which is also true of this exact position, so a real
    /// regression here would have been silently reported as an expected
    /// shape instead of a parity gap.
    #[test]
    fn an_unentered_identity_at_a_valdefs_own_declared_type_position_is_unaccounted() {
        let valdef = node(
            VALDEF_TAG,
            &[vec![nat(0)], node(REFINEDTPT_TAG, &[])].concat(),
        );
        let bytes = build_file(&package(&valdef));
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        let identity = report
            .iter()
            .find(|node| node.tag == REFINEDTPT_TAG)
            .unwrap();
        assert_eq!(
            identity.outcome,
            IdentityOutcome::Unaccounted,
            "a VALDEF's own declared-type child is a supported position, never out of scope: {report:?}"
        );
    }

    /// The same regression at a `DEFDEF`'s own result-type position — a
    /// `LAMBDAtpt` sitting exactly where `enter_definition_body` reads the
    /// result type from (the first child that is not a parameter or clause
    /// marker) — must likewise be `Unaccounted`, never `OutOfScope`, when
    /// discovery never entered it.
    #[test]
    fn an_unentered_identity_at_a_defdefs_own_result_position_is_unaccounted() {
        let defdef = node(
            DEFDEF_TAG,
            &[vec![nat(0)], node(LAMBDATPT_TAG, &[])].concat(),
        );
        let bytes = build_file(&package(&defdef));
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        let identity = report
            .iter()
            .find(|node| node.tag == LAMBDATPT_TAG)
            .unwrap();
        assert_eq!(
            identity.outcome,
            IdentityOutcome::Unaccounted,
            "a DEFDEF's own result position is a supported position, never out of scope: {report:?}"
        );
    }

    /// An identity nested inside an unsupported `APPLY` that is a `DEFDEF`'s
    /// right-hand side (no `BLOCK` in between) is genuinely a skipped method
    /// body, so it must stay `OutOfScope` — proving the fix above did not
    /// simply invert into always reporting a `DEFDEF`/`VALDEF` descendant as
    /// `Unaccounted`. `APPLY` has no arm in any of the four mirrored dispatch
    /// tables, so the walk that reaches it (as the `DEFDEF`'s own "result"
    /// slot, since this `DEFDEF` has no separate result type before it) stops
    /// there rather than descending into the `REFINEDtpt` nested in it.
    #[test]
    fn an_unentered_identity_inside_an_unsupported_apply_in_a_defdefs_body_is_out_of_scope() {
        let rhs = node(APPLY_TAG, &node(REFINEDTPT_TAG, &[]));
        let defdef = node(DEFDEF_TAG, &[vec![nat(0)], rhs].concat());
        let bytes = build_file(&package(&defdef));
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        let identity = report
            .iter()
            .find(|node| node.tag == REFINEDTPT_TAG)
            .unwrap();
        assert_eq!(
            identity.outcome,
            IdentityOutcome::OutOfScope,
            "a DEFDEF's right-hand side is a genuinely skipped method body: {report:?}"
        );
    }

    /// The review's own case (issue #101 PR #102): a physically present,
    /// unentered `REFINEDtpt` reachable *only* through a `SHAREDterm` link
    /// sitting inside an unsupported `APPLY` argument — exactly
    /// `tests/discovery.rs`'s `poison`/`poisoned` fixture, which discovery
    /// correctly never enters because `APPLY` has no arm in any dispatch
    /// table. Before this fix, an oracle that only tested physical ancestor
    /// tags reported this as `Unaccounted` (no `VALDEF`/`DEFDEF`/`BLOCK`/
    /// `CASEDEF` ancestor at all, since `poisoned` is a loose top-level tree
    /// only named by a link), a false positive that would have failed the
    /// corpus's `unaccounted == 0` assertion on a shape discovery is
    /// *correct* to skip. `reachable_identities` gets this right because it
    /// never even visits the `SHAREDterm` link: nothing in any dispatch table
    /// descends into an `APPLY`'s arguments.
    #[test]
    fn a_refinedtpt_reachable_only_through_a_shared_link_inside_an_unsupported_apply_is_out_of_scope()
     {
        // `poisoned` is a loose top-level tree, sitting right after the
        // top-level `PACKAGE` that holds `poison` — its address must track
        // that `PACKAGE`'s own encoded length.
        let poisoned_address = 13;
        let poison_fn = leaf(TERMREFPKG_TAG, 0);
        let poison_arg = leaf(SHAREDTERM_TAG, poisoned_address);
        let rhs = node(APPLY_TAG, &[poison_fn, poison_arg].concat());
        let poison = node(DEFDEF_TAG, &[vec![nat(0)], rhs].concat());
        let package = package(&poison);
        assert_eq!(
            u8::try_from(package.len()).unwrap(),
            poisoned_address,
            "poisoned_address must track the PACKAGE's actual encoded length"
        );
        let poisoned = node(REFINEDTPT_TAG, &[]);
        let ast = [package, poisoned].concat();
        let bytes = build_file(&ast);
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        let identity = report
            .iter()
            .find(|node| node.address == u32::from(poisoned_address))
            .unwrap();
        assert_eq!(
            identity.outcome,
            IdentityOutcome::OutOfScope,
            "poisoned is reachable only through an unsupported APPLY argument: {report:?}"
        );
    }

    /// A self-check on `reachable_identities`'s own root enumeration: a local
    /// `DEFDEF` nested inside an *enclosing* `DEFDEF`'s `BLOCK` body — the
    /// textbook case pass 1 never enters (`enter.rs`'s module documentation)
    /// — must not become a root just because it is, itself, a `DEFDEF` node
    /// somewhere in the file. An earlier version of this function found every
    /// `VALDEF`/`TYPEDEF`/`DEFDEF` node in the whole AST section by tag alone,
    /// regardless of position, which wrongly treated a local definition's own
    /// declared type as reachable and turned two real `scala3-library` files
    /// ($eq$colon$eq, $less$colon$less: nineteen `LAMBDAtpt`s each reached
    /// only from within a local method's own `BLOCK`) into false-positive
    /// `Unaccounted` corpus failures. The fix walks only from the file's
    /// top-level `PACKAGE`s, through `TEMPLATE`/`DEFDEF`/`VALDEF` exactly as
    /// `enter_package`/`enter_template`/`enter_definition_body` themselves
    /// do, so a member nested inside another member's *body* rather than its
    /// declared-type position is never visited at all.
    #[test]
    fn a_local_defdefs_own_result_position_nested_in_an_enclosing_blocks_body_is_out_of_scope() {
        let inner = node(
            DEFDEF_TAG,
            &[vec![nat(0)], node(LAMBDATPT_TAG, &[])].concat(),
        );
        let block = node(BLOCK_TAG, &[leaf(TERMREFPKG_TAG, 0), inner].concat());
        let outer = node(DEFDEF_TAG, &[vec![nat(0)], block].concat());
        let bytes = build_file(&package(&outer));
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        let identity = report
            .iter()
            .find(|node| node.tag == LAMBDATPT_TAG)
            .unwrap();
        assert_eq!(
            identity.outcome,
            IdentityOutcome::OutOfScope,
            "a local DEFDEF nested in an enclosing DEFDEF's BLOCK body is never entered, real or mirrored: {report:?}"
        );
    }
}
