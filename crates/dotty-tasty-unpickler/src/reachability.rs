//! The post-hoc identity-reachability oracle (Milestone 5d2c's follow-up
//! review of issue #101): an independent cross-check, run after
//! [`TastyUnpickler::enter_symbols`], of whether pass-1 discovery
//! ([`crate::discovery`]) really has parity with semantic projection.
//!
//! Discovery's own tests exercise the routing table from the *inside*: they
//! build a fixture with a known hidden route and check that the identity at
//! its end is entered. This module instead starts from the *wire*: it finds
//! every `LAMBDAtpt`/`REFINEDtpt` node physically present in the AST section,
//! whether or not any discovery call ever visited it, and classifies each one
//! by what the entered state and the node's own structural position say about
//! it. A node discovery's routing table should have reached (it is not inside
//! a term body, the one class of position pass 1 documents as never entered)
//! but that has no owner is [`IdentityOutcome::Unaccounted`] — a genuine
//! parity gap, never an expected shape. Run across a whole corpus, this is
//! the "reachable vs. entered" measurement issue #101's review asked for; the
//! per-unit call is exact, not a sample.

use dotty_tasty::tasty::{
    AstAddressIndex, BLOCK_TAG, CASEDEF_TAG, DEFDEF_TAG, EMPTYCLAUSE_TAG, LAMBDATPT_TAG, PARAM_TAG,
    REFINEDTPT_TAG, SPLITCLAUSE_TAG, TYPEDEF_TAG, TYPEPARAM_TAG, TastyFile, VALDEF_TAG,
};

use crate::error::UnpickleError;
use crate::index::TastySemanticIndex;

/// What the entered state and wire position of one `LAMBDAtpt`/`REFINEDtpt`
/// node say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityOutcome {
    /// Discovery entered it: it has an owner (`lambda_owner`/`refined_owner`
    /// / `symbol_at`), possibly alongside a recorded owner conflict.
    Entered,
    /// Not entered, but nested under a `BLOCK` or a `CASEDEF`: a local
    /// definition's declared type, or a pattern binder's, which pass 1 never
    /// enters at all (`enter.rs`'s module documentation) — discovery
    /// correctly never starts a walk there, so this is not a gap.
    InBody,
    /// Neither of the above: a node discovery's routing table should have
    /// reached from some supported declared-type position but did not. Always
    /// a genuine parity gap when it occurs; a corpus run with any of these
    /// disproves the milestone's own claim, rather than merely leaving it
    /// unmeasured.
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

/// Whether `at`'s structural ancestor chain crosses into a genuinely skipped
/// body or local-definition context before reaching a root: a `BLOCK`'s or
/// `CASEDEF`'s child (a local statement or a pattern binder — pass 1 never
/// starts a walk there at all), or a `VALDEF`/`TYPEDEF`/`DEFDEF` child that is
/// its right-hand side rather than its declared type.
///
/// A `VALDEF`/`TYPEDEF`/`DEFDEF` node also being an *ancestor* proves nothing
/// on its own: `at` may just as well *be* that definition's own declared-type
/// child (its first child for `VALDEF`/`TYPEDEF`,
/// [`discover_declared_type_identities`](crate::unpickler::TastyUnpickler::discover_declared_type_identities);
/// its result — the first child that is not a parameter or clause marker, for
/// `DEFDEF`, `enter_definition_body`'s own rule) or one of its own
/// `TYPEPARAM`/`PARAM` children, in which case that child is a supported
/// declared-type position discovery should have entered directly, not a body.
/// Only the *other* child edge — the value or the method body — is a real
/// skipped position; this function checks that specific edge instead of
/// merely testing tag membership, so it cannot mistake a definition's own
/// declared type (or an unsupported term such as `APPLY` sitting directly in
/// its right-hand side, without an intervening `BLOCK`) for the other.
///
/// `LAMBDAtpt` needs no case of its own: every one of its children (each
/// `TYPEPARAM` and its trailing body) is itself a declared-type position
/// [`enter_lambda_tpt`](crate::unpickler::TastyUnpickler::enter_lambda_tpt)
/// enters directly, so climbing straight through it is always correct.
fn in_body(index: &AstAddressIndex<'_>, at: u32) -> bool {
    let mut current = at;
    while let Some(parent) = index.parent_of(current) {
        let parent_address = u32::try_from(parent.offset).unwrap_or(u32::MAX);
        match parent.tag {
            BLOCK_TAG | CASEDEF_TAG => return true,
            VALDEF_TAG | TYPEDEF_TAG => {
                let is_declared_type = index
                    .children_of(parent_address)
                    .next()
                    .is_some_and(|first| first.offset as u32 == current);
                if !is_declared_type {
                    return true;
                }
            }
            DEFDEF_TAG => {
                let children: Vec<_> = index.children_of(parent_address).collect();
                let is_parameter = children
                    .iter()
                    .find(|child| child.offset as u32 == current)
                    .is_some_and(|child| matches!(child.tag, TYPEPARAM_TAG | PARAM_TAG));
                if !is_parameter {
                    let is_result = children
                        .iter()
                        .find(|child| {
                            !matches!(
                                child.tag,
                                TYPEPARAM_TAG | PARAM_TAG | EMPTYCLAUSE_TAG | SPLITCLAUSE_TAG
                            )
                        })
                        .is_some_and(|result| result.offset as u32 == current);
                    if !is_result {
                        return true;
                    }
                }
            }
            _ => {}
        }
        current = parent_address;
    }
    false
}

/// Every `LAMBDAtpt`/`REFINEDtpt` node in `file`'s AST section, classified
/// against `index` (the result of a completed
/// [`enter_symbols`](crate::unpickler::TastyUnpickler::enter_symbols) call).
pub fn identity_reachability(
    file: &TastyFile<'_>,
    index: &TastySemanticIndex,
) -> Result<Vec<IdentityNode>, UnpickleError> {
    let address_index = file.ast_address_index()?;
    let mut results = Vec::new();
    for tag in [LAMBDATPT_TAG, REFINEDTPT_TAG] {
        for node in address_index.iter_nodes_with_tag(tag) {
            let address = u32::try_from(node.offset).unwrap_or(u32::MAX);
            let entered = if tag == LAMBDATPT_TAG {
                index.lambda_owner(address).is_some()
            } else {
                index.symbol_at(address).is_some()
            };
            let outcome = if entered {
                IdentityOutcome::Entered
            } else if in_body(&address_index, address) {
                IdentityOutcome::InBody
            } else {
                IdentityOutcome::Unaccounted
            };
            results.push(IdentityNode {
                address,
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
    use dotty_tasty::tasty::{
        APPLY_TAG, Header, NameTable, RawName, Section, SectionTable, TERMREFPKG_TAG,
    };

    fn nat(value: u8) -> u8 {
        0x80 | value
    }

    fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag, nat(u8::try_from(payload.len()).unwrap())];
        bytes.extend(payload);
        bytes
    }

    /// A leaf tree: a tag directly followed by an inline `Nat`, no length
    /// prefix and no children (`TERMREFpkg`, ...).
    fn leaf(tag: u8, value: u8) -> Vec<u8> {
        vec![tag, nat(value)]
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
    /// and a bare `REFINEDtpt` that is neither — the one case the oracle must
    /// call out.
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
    fn an_unentered_refinedtpt_inside_a_block_is_reported_in_body() {
        let bytes = file();
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        // The nested `REFINEDtpt` is not at address 0 or the final bare one;
        // find it by exclusion.
        let bare_addresses: Vec<u32> = report
            .iter()
            .filter(|node| node.outcome != IdentityOutcome::InBody)
            .map(|node| node.address)
            .collect();
        let in_body = report
            .iter()
            .find(|node| !bare_addresses.contains(&node.address))
            .expect("one REFINEDtpt is nested in a BLOCK");
        assert_eq!(in_body.outcome, IdentityOutcome::InBody);
    }

    #[test]
    fn an_unentered_refinedtpt_with_no_body_ancestor_is_unaccounted() {
        let bytes = file();
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        assert!(
            report
                .iter()
                .any(|node| node.outcome == IdentityOutcome::Unaccounted),
            "expected the bare, never-entered REFINEDtpt to be unaccounted: {report:?}"
        );
    }

    /// Regression for the review of Commit 5's oracle (issue #101): a
    /// `REFINEDtpt` sitting exactly at a `VALDEF`'s declared-type child —
    /// precisely the position `discover_declared_type_identities` enters
    /// directly — must be `Unaccounted` when discovery never entered it, not
    /// `InBody`. The old heuristic tested only "is any ancestor a `VALDEF`",
    /// which is also true of this exact position, so a real regression here
    /// would have been silently reported as an expected shape instead of a
    /// parity gap.
    #[test]
    fn an_unentered_identity_at_a_valdefs_own_declared_type_position_is_unaccounted() {
        let ast = node(
            VALDEF_TAG,
            &[vec![nat(0)], node(REFINEDTPT_TAG, &[])].concat(),
        );
        let bytes = build_file(&ast);
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
            "a VALDEF's own declared-type child is a supported position, never a body: {report:?}"
        );
    }

    /// The same regression at a `DEFDEF`'s own result-type position — a
    /// `LAMBDAtpt` sitting exactly where `enter_definition_body` reads the
    /// result type from (the first child that is not a parameter or clause
    /// marker) — must likewise be `Unaccounted`, never `InBody`, when
    /// discovery never entered it.
    #[test]
    fn an_unentered_identity_at_a_defdefs_own_result_position_is_unaccounted() {
        let ast = node(
            DEFDEF_TAG,
            &[vec![nat(0)], node(LAMBDATPT_TAG, &[])].concat(),
        );
        let bytes = build_file(&ast);
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
            "a DEFDEF's own result position is a supported position, never a body: {report:?}"
        );
    }

    /// The same review's second case: an identity nested inside an
    /// unsupported `APPLY` that is a `DEFDEF`'s right-hand side (no `BLOCK`
    /// in between) is genuinely a skipped method body, so it must stay
    /// `InBody` — proving the fix above did not simply invert into always
    /// reporting a `DEFDEF`/`VALDEF` descendant as `Unaccounted`. The result
    /// must come from checking which child edge was actually crossed
    /// (declared result vs. right-hand side), not from `APPLY` or `DEFDEF`
    /// tag membership alone.
    #[test]
    fn an_unentered_identity_inside_an_unsupported_apply_in_a_defdefs_body_is_in_body() {
        let result_type = leaf(TERMREFPKG_TAG, 0);
        let rhs = node(APPLY_TAG, &node(REFINEDTPT_TAG, &[]));
        let ast = node(DEFDEF_TAG, &[vec![nat(0)], result_type, rhs].concat());
        let bytes = build_file(&ast);
        let parsed = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = TastySemanticIndex::new();

        let report = identity_reachability(&parsed, &index).unwrap();

        let identity = report
            .iter()
            .find(|node| node.tag == REFINEDTPT_TAG)
            .unwrap();
        assert_eq!(
            identity.outcome,
            IdentityOutcome::InBody,
            "a DEFDEF's right-hand side is a genuinely skipped method body: {report:?}"
        );
    }
}
