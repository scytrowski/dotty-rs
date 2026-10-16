//! Fresh AST construction with symbolic labels and variable-width address fixups.
//!
//! Labels belong to one builder. Each reserved label must be bound and emitted
//! exactly once as a root or child; references do not emit their targets. Layout
//! starts with zero addresses and repeats codec-sized passes until stable. No
//! address bytes are patched in place. The result is an AST fragment, not a file;
//! callers remain responsible for the grammar of category-five payloads.

use std::fmt;

use crate::ast::*;
use crate::term::{SimpleTerm, TermEncodeError, TermValue};
use crate::writer::{WriteError, Writer};

/// A reserved node in one [`AstBuilder`], independent of its final byte address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AstLabel(usize);

/// One field in a length-delimited node's ordered wire payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AstField {
    /// An emitted child tree.
    Tree(AstLabel),
    /// A raw Nat, such as a name reference after a method parameter type.
    Nat(u32),
}

/// The Nat operand of a category-four node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AstOperand {
    /// A name reference or other literal Nat.
    Nat(u32),
    /// A declaration address, resolved during layout.
    Reference(AstLabel),
}

/// Wire-only construction schema. Child labels determine emission order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AstNode {
    /// A validated category-one or category-two leaf (no numeric AST addresses).
    Leaf(SimpleTerm),
    /// A shared or direct reference leaf.
    Reference {
        /// Reference tag.
        tag: u8,
        /// Target node label.
        target: AstLabel,
    },
    /// A category-three wrapper.
    Ast {
        /// Wrapper tag.
        tag: u8,
        /// Child node.
        child: AstLabel,
    },
    /// A category-four wrapper.
    NatAst {
        /// Wrapper tag.
        tag: u8,
        /// Literal Nat or symbolic declaration address.
        value: AstOperand,
        /// Child node.
        child: AstLabel,
    },
    /// A category-five node with a codec-computed length prefix.
    Length {
        /// Node tag.
        tag: u8,
        /// Ordered payload fields.
        fields: Vec<AstField>,
    },
}

/// Resource bounds for construction and iterative layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AstBuildLimits {
    /// Maximum labels plus payload fields.
    pub max_items: usize,
    /// Maximum child nesting; traversal is iterative.
    pub max_depth: usize,
    /// Maximum fragment bytes, also bounded by the wire u32 address range.
    pub max_bytes: usize,
    /// Maximum passes to resolve address and length widths.
    pub max_layout_passes: usize,
}

impl Default for AstBuildLimits {
    fn default() -> Self {
        Self {
            max_items: 1_000_000,
            max_depth: 256,
            max_bytes: 16 * 1024 * 1024,
            max_layout_passes: 64,
        }
    }
}

/// Typed construction failures. No partial fragment is returned on failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AstBuildError {
    /// A label was not reserved in this builder.
    InvalidLabel(AstLabel),
    /// A label has no node definition.
    Unbound(AstLabel),
    /// A label was bound twice.
    AlreadyBound(AstLabel),
    /// A child/root node was emitted twice.
    RepeatedNode(AstLabel),
    /// A reserved node is unreachable from the ordered roots.
    UnusedNode(AstLabel),
    /// Child edges contain a cycle.
    Cycle(AstLabel),
    /// A reference has an invalid tag, target kind, or sharing direction.
    InvalidReference { tag: u8, target: AstLabel },
    /// A configured budget was exceeded.
    Limit(&'static str),
    /// The wire address/length arithmetic overflowed.
    Overflow,
    /// Leaf validation/encoding failed.
    Term(TermEncodeError),
    /// Wire length encoding failed.
    Write(WriteError),
}

impl fmt::Display for AstBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AstBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Term(e) => Some(e),
            Self::Write(e) => Some(e),
            _ => None,
        }
    }
}
impl From<TermEncodeError> for AstBuildError {
    fn from(e: TermEncodeError) -> Self {
        Self::Term(e)
    }
}

/// Owns flat wire nodes and symbolic references; contains no compiler semantics.
#[derive(Debug)]
pub struct AstBuilder {
    nodes: Vec<Option<AstNode>>,
    limits: AstBuildLimits,
    items: usize,
}

/// Encoded fragment and final addresses of all labels in its builder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltAst {
    bytes: Vec<u8>,
    addresses: Vec<u32>,
}

impl BuiltAst {
    /// Borrows fragment bytes. Addresses are relative to byte zero of this payload.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Resolves a label from the same builder.
    pub fn address(&self, label: AstLabel) -> Option<u32> {
        self.addresses.get(label.0).copied()
    }
}

impl AstNode {
    fn children(&self) -> Vec<AstLabel> {
        match self {
            Self::Ast { child, .. } | Self::NatAst { child, .. } => vec![*child],
            Self::Length { fields, .. } => fields
                .iter()
                .filter_map(|f| match f {
                    AstField::Tree(l) => Some(*l),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }
    /// Returns the node's TASTy tag.
    pub fn tag(&self) -> u8 {
        match self {
            Self::Leaf(leaf) => leaf.tag,
            Self::Reference { tag, .. }
            | Self::Ast { tag, .. }
            | Self::NatAst { tag, .. }
            | Self::Length { tag, .. } => *tag,
        }
    }
}

impl AstBuilder {
    /// Creates a builder with explicit limits.
    pub fn new(limits: AstBuildLimits) -> Self {
        Self {
            nodes: vec![],
            limits,
            items: 0,
        }
    }
    fn check_items(&self, extra: usize) -> Result<usize, AstBuildError> {
        self.items
            .checked_add(extra)
            .filter(|n| *n <= self.limits.max_items)
            .ok_or(AstBuildError::Limit("items"))
    }
    /// Reserves an unbound label, including forward declaration targets.
    pub fn reserve(&mut self) -> Result<AstLabel, AstBuildError> {
        self.items = self.check_items(1)?;
        let label = AstLabel(self.nodes.len());
        self.nodes.push(None);
        Ok(label)
    }
    fn node(&self, label: AstLabel) -> Result<&AstNode, AstBuildError> {
        self.nodes
            .get(label.0)
            .ok_or(AstBuildError::InvalidLabel(label))?
            .as_ref()
            .ok_or(AstBuildError::Unbound(label))
    }
    /// Binds a label exactly once. Validation failures do not change the builder.
    pub fn bind(&mut self, label: AstLabel, node: AstNode) -> Result<(), AstBuildError> {
        if self
            .nodes
            .get(label.0)
            .ok_or(AstBuildError::InvalidLabel(label))?
            .is_some()
        {
            return Err(AstBuildError::AlreadyBound(label));
        }
        let extra = match &node {
            AstNode::Length { fields, .. } => fields.len(),
            _ => 0,
        };
        let items = self.check_items(extra)?;
        match &node {
            AstNode::Leaf(leaf) => {
                SimpleTerm::new(leaf.tag, leaf.value.clone())?;
                if matches!(leaf.value, TermValue::AstRef(_)) {
                    return Err(AstBuildError::InvalidReference {
                        tag: leaf.tag,
                        target: label,
                    });
                }
            }
            AstNode::Reference { tag, .. }
                if !matches!(
                    *tag,
                    SHAREDTERM_TAG
                        | SHAREDTYPE_TAG
                        | TERMREFDIRECT_TAG
                        | TYPEREFDIRECT_TAG
                        | RECTHIS_TAG
                ) =>
            {
                return Err(AstBuildError::InvalidReference {
                    tag: *tag,
                    target: label,
                });
            }
            AstNode::Ast { tag, .. } if !(90..=104).contains(tag) => {
                return Err(AstBuildError::Write(WriteError::InvalidTag { tag: *tag }));
            }
            AstNode::NatAst { tag, value, .. } => {
                if !(110..=119).contains(tag) {
                    return Err(AstBuildError::Write(WriteError::InvalidTag { tag: *tag }));
                }
                if matches!(value, AstOperand::Reference(_))
                    != matches!(*tag, TERMREFSYMBOL_TAG | TYPEREFSYMBOL_TAG)
                {
                    return Err(AstBuildError::InvalidReference {
                        tag: *tag,
                        target: label,
                    });
                }
            }
            AstNode::Length { tag, .. } if *tag < 128 => {
                return Err(AstBuildError::Write(WriteError::InvalidTag { tag: *tag }));
            }
            _ => {}
        }
        self.items = items;
        self.nodes[label.0] = Some(node);
        Ok(())
    }
    /// Allocates and binds one node. On failure no label is retained.
    pub fn push(&mut self, node: AstNode) -> Result<AstLabel, AstBuildError> {
        let label = self.reserve()?;
        if let Err(error) = self.bind(label, node) {
            self.nodes.pop();
            self.items -= 1;
            return Err(error);
        }
        Ok(label)
    }
    fn order(&self, roots: &[AstLabel]) -> Result<Vec<AstLabel>, AstBuildError> {
        if roots.len() > self.limits.max_items {
            return Err(AstBuildError::Limit("items"));
        }
        let mut stack: Vec<_> = roots.iter().rev().map(|l| (*l, 0usize, false)).collect();
        let mut state = vec![0; self.nodes.len()];
        let mut order = vec![];
        while let Some((label, depth, exit)) = stack.pop() {
            let node = self.node(label)?;
            if exit {
                state[label.0] = 2;
                continue;
            }
            if depth > self.limits.max_depth {
                return Err(AstBuildError::Limit("depth"));
            }
            match state[label.0] {
                1 => return Err(AstBuildError::Cycle(label)),
                2 => return Err(AstBuildError::RepeatedNode(label)),
                _ => {}
            }
            state[label.0] = 1;
            order.push(label);
            stack.push((label, depth, true));
            for child in node.children().into_iter().rev() {
                stack.push((
                    child,
                    depth.checked_add(1).ok_or(AstBuildError::Overflow)?,
                    false,
                ));
            }
        }
        for (i, seen) in state.into_iter().enumerate() {
            self.node(AstLabel(i))?;
            if seen == 0 {
                return Err(AstBuildError::UnusedNode(AstLabel(i)));
            }
        }
        Ok(order)
    }
    fn nat_size(value: u32) -> usize {
        let mut w = Writer::new();
        w.write_nat(value);
        w.position()
    }
    fn bounded(&self, size: usize) -> Result<usize, AstBuildError> {
        if size > self.limits.max_bytes {
            return Err(AstBuildError::Limit("bytes"));
        }
        u32::try_from(size).map_err(|_| AstBuildError::Overflow)?;
        Ok(size)
    }
    fn add(&self, a: usize, b: usize) -> Result<usize, AstBuildError> {
        self.bounded(a.checked_add(b).ok_or(AstBuildError::Overflow)?)
    }
    fn prefix(
        &self,
        node: &AstNode,
        addresses: &[u32],
        sizes: &[usize],
    ) -> Result<Writer, AstBuildError> {
        let mut writer = Writer::new();
        let address = |target: AstLabel| {
            addresses
                .get(target.0)
                .copied()
                .ok_or(AstBuildError::InvalidLabel(target))
        };
        match node {
            AstNode::Leaf(leaf) => leaf.encode(&mut writer)?,
            AstNode::Reference { tag, target } => {
                SimpleTerm::new(*tag, TermValue::AstRef(address(*target)?))?.encode(&mut writer)?
            }
            AstNode::Ast { tag, .. } => writer.write_u8(*tag),
            AstNode::NatAst { tag, value, .. } => {
                writer.write_u8(*tag);
                writer.write_nat(match value {
                    AstOperand::Nat(n) => *n,
                    AstOperand::Reference(l) => address(*l)?,
                });
            }
            AstNode::Length { tag, fields } => {
                let mut size = 0;
                for field in fields {
                    size = self.add(
                        size,
                        match field {
                            AstField::Tree(l) => {
                                *sizes.get(l.0).ok_or(AstBuildError::InvalidLabel(*l))?
                            }
                            AstField::Nat(n) => Self::nat_size(*n),
                        },
                    )?;
                }
                writer.write_u8(*tag);
                writer.write_nat(u32::try_from(size).map_err(|_| AstBuildError::Overflow)?);
            }
        }
        Ok(writer)
    }
    /// Resolves all labels and emits a fresh fragment. Order comes only from roots
    /// and child fields. Shared references must point backward; direct/symbol
    /// references must target a declaration of the correct namespace.
    pub fn finish(&self, roots: &[AstLabel]) -> Result<BuiltAst, AstBuildError> {
        let order = self.order(roots)?;
        let mut addresses = vec![0; self.nodes.len()];
        let mut sizes = vec![0; self.nodes.len()];
        for _ in 0..self.limits.max_layout_passes {
            for &label in order.iter().rev() {
                let node = self.node(label)?;
                let mut size = self.prefix(node, &addresses, &sizes)?.position();
                match node {
                    AstNode::Length { fields, .. } => {
                        for f in fields {
                            size = self.add(
                                size,
                                match f {
                                    AstField::Tree(l) => sizes[l.0],
                                    AstField::Nat(n) => Self::nat_size(*n),
                                },
                            )?;
                        }
                    }
                    _ => {
                        for child in node.children() {
                            size = self.add(size, sizes[child.0])?;
                        }
                    }
                }
                sizes[label.0] = self.bounded(size)?;
            }
            let mut next = vec![0; self.nodes.len()];
            let mut end = 0;
            for root in roots {
                next[root.0] = u32::try_from(end).map_err(|_| AstBuildError::Overflow)?;
                end = self.add(end, sizes[root.0])?;
            }
            for &label in &order {
                let node = self.node(label)?;
                let mut at = self.add(
                    next[label.0] as usize,
                    self.prefix(node, &addresses, &sizes)?.position(),
                )?;
                match node {
                    AstNode::Length { fields, .. } => {
                        for field in fields {
                            match field {
                                AstField::Tree(l) => {
                                    next[l.0] =
                                        u32::try_from(at).map_err(|_| AstBuildError::Overflow)?;
                                    at = self.add(at, sizes[l.0])?;
                                }
                                AstField::Nat(n) => {
                                    at = self.add(at, Self::nat_size(*n))?;
                                }
                            }
                        }
                    }
                    _ => {
                        for child in node.children() {
                            next[child.0] =
                                u32::try_from(at).map_err(|_| AstBuildError::Overflow)?;
                            at = self.add(at, sizes[child.0])?;
                        }
                    }
                }
            }
            if next != addresses {
                addresses = next;
                continue;
            }
            for &label in &order {
                let reference = match self.node(label)? {
                    AstNode::Reference { tag, target } => Some((*tag, *target)),
                    AstNode::NatAst {
                        tag,
                        value: AstOperand::Reference(target),
                        ..
                    } => Some((*tag, *target)),
                    _ => None,
                };
                if let Some((tag, target)) = reference {
                    let target_tag = self.node(target)?.tag();
                    let valid = match tag {
                        SHAREDTERM_TAG | SHAREDTYPE_TAG => addresses[target.0] < addresses[label.0],
                        TYPEREFDIRECT_TAG | TYPEREFSYMBOL_TAG => {
                            matches!(target_tag, TYPEDEF_TAG | TYPEPARAM_TAG)
                        }
                        TERMREFDIRECT_TAG | TERMREFSYMBOL_TAG => {
                            matches!(target_tag, VALDEF_TAG | DEFDEF_TAG | PARAM_TAG | BIND_TAG)
                        }
                        RECTHIS_TAG => target_tag == RECTYPE_TAG,
                        _ => false,
                    };
                    if !valid {
                        return Err(AstBuildError::InvalidReference { tag, target });
                    }
                }
            }
            let mut writer = Writer::with_capacity(end);
            let mut stack: Vec<_> = roots.iter().rev().map(|l| AstField::Tree(*l)).collect();
            while let Some(field) = stack.pop() {
                match field {
                    AstField::Nat(n) => writer.write_nat(n),
                    AstField::Tree(l) => {
                        let node = self.node(l)?;
                        writer.write_bytes(self.prefix(node, &addresses, &sizes)?.as_slice());
                        match node {
                            AstNode::Length { fields, .. } => {
                                stack.extend(fields.iter().rev().cloned())
                            }
                            _ => {
                                stack.extend(node.children().into_iter().rev().map(AstField::Tree))
                            }
                        }
                    }
                }
            }
            return Ok(BuiltAst {
                bytes: writer.into_inner(),
                addresses,
            });
        }
        Err(AstBuildError::Limit("layout passes"))
    }
}
