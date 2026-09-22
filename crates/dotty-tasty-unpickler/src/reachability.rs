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
    BLOCK_TAG, CASEDEF_TAG, DEFDEF_TAG, LAMBDATPT_TAG, REFINEDTPT_TAG, TastyFile, VALDEF_TAG,
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

/// Whether `at`'s structural ancestor chain passes through a `VALDEF`,
/// `DEFDEF`, `BLOCK`, `CASEDEF` or `LAMBDAtpt` before reaching a root: the
/// same tag set `tests/type_corpus.rs`'s own `inside_a_body` heuristic uses
/// for the analogous "should pass 1 have entered this" question about a
/// missing-symbol reference target. A local definition's own tag (`VALDEF`/
/// `DEFDEF`) already answers it without needing to reach the `BLOCK` it sits
/// in; `LAMBDAtpt` covers a type-lambda alias's own, separately-owned
/// parameters and body (`enter.rs`'s module documentation).
fn in_body(index: &dotty_tasty::tasty::AstAddressIndex<'_>, at: u32) -> bool {
    let mut current = at;
    while let Some(parent) = index.parent_of(current) {
        if matches!(
            parent.tag,
            VALDEF_TAG | DEFDEF_TAG | BLOCK_TAG | CASEDEF_TAG | LAMBDATPT_TAG
        ) {
            return true;
        }
        current = u32::try_from(parent.offset).unwrap_or(u32::MAX);
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
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};

    fn nat(value: u8) -> u8 {
        0x80 | value
    }

    fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag, nat(u8::try_from(payload.len()).unwrap())];
        bytes.extend(payload);
        bytes
    }

    /// Three top-level nodes: a `REFINEDtpt` this test enters (so it has a
    /// symbol), a `REFINEDtpt` nested inside a `BLOCK` that is never entered,
    /// and a bare `REFINEDtpt` that is neither — the one case the oracle must
    /// call out.
    fn file() -> Vec<u8> {
        let mut ast = node(REFINEDTPT_TAG, &[]); // address 0: entered
        ast.extend(node(BLOCK_TAG, &node(REFINEDTPT_TAG, &[]))); // nested in a BLOCK
        ast.extend(node(REFINEDTPT_TAG, &[])); // bare, never entered
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
            SectionTable::from_sections(vec![Section::new(0, &ast)]),
        )
        .unwrap()
        .encode()
        .unwrap()
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
}
