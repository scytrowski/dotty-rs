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

#[allow(dead_code)]
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
    #[allow(dead_code)]
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
