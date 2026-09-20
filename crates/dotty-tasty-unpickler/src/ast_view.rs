//! The file's AST addressed by absolute offset, shared by every pass.
//!
//! An address in TASTy is only meaningful as the start of a node, and every
//! reference the unpickler follows comes from untrusted bytes. Both the
//! symbol pass and the type pass therefore reach the AST through this one
//! view, so there is a single definition of a valid AST reference: an address
//! that is the start of a visible indexed node. An address past the section,
//! or inside another node's payload, is rejected rather than decoded as an
//! unrelated tree.

use std::collections::HashMap;

use dotty_tasty::tasty::{
    AstAddressIndex, AstError, AstTreeNode, RawNode, RawTree, Reader, SHAREDTERM_TAG,
    SHAREDTYPE_TAG, StandardSection, TastyFile, TermValue,
};

use crate::error::UnpickleError;

/// How many `SHAREDtype` links may be followed, one inside another, before the
/// chain is treated as a cycle. A shared link always names an earlier tree, so
/// a real chain is short; a cyclic one must revisit a link.
pub(crate) const MAX_SHARED_DEPTH: usize = 16;

/// The AST address for an absolute offset. TASTy addresses fit `u32`; a
/// larger offset cannot name a node.
pub(crate) fn address(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

fn tree_tag(tree: &RawTree<'_>) -> u8 {
    match tree {
        RawTree::Leaf(term) => term.tag,
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => *tag,
        RawTree::LengthNode(node) => node.tag,
    }
}

/// The file's AST: nodes by absolute address, and each node's direct children
/// in wire order.
pub(crate) struct AstView<'bytes> {
    index: AstAddressIndex<'bytes>,
    children: HashMap<u32, Vec<AstTreeNode>>,
    /// The ASTs section payload, for decoding a tree shared at an address.
    payload: &'bytes [u8],
}

impl<'bytes> AstView<'bytes> {
    pub(crate) fn new(file: &TastyFile<'bytes>) -> Result<Self, UnpickleError> {
        let index = file.ast_address_index()?;
        let mut children: HashMap<u32, Vec<AstTreeNode>> = HashMap::new();
        for edge in index.iter_tree_edges() {
            children
                .entry(address(edge.parent.offset))
                .or_default()
                .push(edge.child);
        }
        let payload = file
            .section(StandardSection::Asts)
            .map_or(&[][..], |section| section.payload);
        Ok(Self {
            index,
            children,
            payload,
        })
    }

    /// Decodes the independent tree rooted at `at`. This is how a
    /// `SHAREDtype` reference is followed: the compiler writes a repeated
    /// subtree once and every other occurrence names its address.
    ///
    /// `at` comes from an untrusted reference, so it must be the start of a
    /// visible AST node. `from` is the referring node, for the error. Offsets
    /// inside the returned tree are absolute.
    pub(crate) fn tree_at(&self, at: u32, from: u32) -> Result<RawTree<'bytes>, UnpickleError> {
        if self.index.get_node(at).is_none() {
            return Err(UnpickleError::InvalidReferenceTarget { from, to: at });
        }
        let mut reader = Reader::with_range(self.payload, at as usize, self.payload.len())
            .map_err(AstError::from)?;
        Ok(RawTree::decode_with_base_offset(&mut reader, 0).map_err(AstError::from)?)
    }

    /// Whether `at` is the start of a visible AST node.
    pub(crate) fn is_node(&self, at: u32) -> bool {
        self.index.get_node(at).is_some()
    }

    /// The tag of the node at `at`, if `at` is the start of a visible node.
    pub(crate) fn tag_at(&self, at: u32) -> Option<u8> {
        self.index.get_node(at).map(|node| node.tag)
    }

    /// The tag of the first node at or after `at` that is not a shared link:
    /// Dotty's `nextUnsharedTag`. A `SHAREDtype` (or `SHAREDterm`) is followed
    /// to its target, however many are chained, up to the same bound as any
    /// other chain of links, and every target must be a visible node. `from`
    /// is the referring node, for the error.
    pub(crate) fn next_unshared_tag(&self, at: u32, from: u32) -> Result<u8, UnpickleError> {
        let mut current = at;
        for _ in 0..=MAX_SHARED_DEPTH {
            match self.tree_at(current, from)? {
                RawTree::Leaf(term) if matches!(term.tag, SHAREDTYPE_TAG | SHAREDTERM_TAG) => {
                    match term.value {
                        TermValue::AstRef(target) => current = target,
                        _ => return Ok(term.tag),
                    }
                }
                tree => return Ok(tree_tag(&tree)),
            }
        }
        Err(UnpickleError::InvalidReferenceTarget { from, to: current })
    }

    pub(crate) fn node(&self, at: u32) -> Result<&RawNode<'bytes>, UnpickleError> {
        self.index
            .get(at)
            .ok_or(UnpickleError::MissingDefinition { address: at })
    }

    pub(crate) fn children(&self, at: u32) -> &[AstTreeNode] {
        self.children.get(&at).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::{Header, NameTable, RawName, Section, SectionTable};

    const TYPEREFPKG: u8 = 65;

    fn nat(value: u8) -> u8 {
        0x80 | value
    }

    /// An `APPLIEDtype` (address 0) over: a `TYPEREFpkg` at 2, a `SHAREDtype`
    /// to it at 4, a `SHAREDterm` to that link at 6, a `SHAREDtype` to itself at
    /// 8, and a `SHAREDtype` to address 1 (not a node) at 10.
    fn file() -> Vec<u8> {
        let mut payload = vec![TYPEREFPKG, nat(1)];
        payload.extend([SHAREDTYPE_TAG, nat(2)]);
        payload.extend([SHAREDTERM_TAG, nat(4)]);
        payload.extend([SHAREDTYPE_TAG, nat(8)]);
        payload.extend([SHAREDTYPE_TAG, nat(1)]);
        let mut ast = vec![161, nat(u8::try_from(payload.len()).unwrap())];
        ast.extend(payload);
        let names = NameTable::from_entries(vec![
            RawName::Utf8("ASTs".to_owned()),
            RawName::Utf8("p".to_owned()),
        ])
        .unwrap();
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

    fn view(bytes: &[u8]) -> AstView<'_> {
        AstView::new(&TastyFile::parse_scala_3_9(bytes).unwrap()).unwrap()
    }

    #[test]
    fn an_unshared_node_reports_its_own_tag() {
        let bytes = file();
        assert_eq!(view(&bytes).next_unshared_tag(2, 0), Ok(TYPEREFPKG));
    }

    #[test]
    fn a_shared_type_link_is_followed_to_its_target() {
        let bytes = file();
        assert_eq!(view(&bytes).next_unshared_tag(4, 0), Ok(TYPEREFPKG));
    }

    #[test]
    fn a_chain_through_shared_term_and_shared_type_links_is_followed() {
        let bytes = file();
        assert_eq!(view(&bytes).next_unshared_tag(6, 0), Ok(TYPEREFPKG));
    }

    #[test]
    fn a_link_to_itself_is_an_invalid_reference_not_a_loop() {
        let bytes = file();
        assert!(matches!(
            view(&bytes).next_unshared_tag(8, 0),
            Err(UnpickleError::InvalidReferenceTarget { from: 0, .. })
        ));
    }

    #[test]
    fn a_link_to_no_node_is_an_invalid_reference() {
        let bytes = file();
        assert_eq!(
            view(&bytes).next_unshared_tag(10, 0),
            Err(UnpickleError::InvalidReferenceTarget { from: 0, to: 1 })
        );
    }

    #[test]
    fn a_start_that_is_no_node_is_an_invalid_reference() {
        let bytes = file();
        assert_eq!(
            view(&bytes).next_unshared_tag(3, 0),
            Err(UnpickleError::InvalidReferenceTarget { from: 0, to: 3 })
        );
    }
}
