//! AST phase markers.
//!
//! `Untyped` and `Typed` are uninhabited marker types used only as the type
//! parameter `P` of [`crate::ids::TreeId`] and (once the rest of the `ast`
//! module lands) [`Tree`]/[`AstArena`]/[`TreeKind`]. Only the two marker
//! types are defined here for now — the full `AstPhase` trait (with its
//! `TypeInfo`/`ExtraNode`/`DefMetadata` associated types) is added once
//! `UntypedNode` and `Modifiers` exist to be its associated types; see
//! `docs/dotty-core-design.md` §7.
//!
//! [`Tree`]: super::Tree
//! [`AstArena`]: super::AstArena
//! [`TreeKind`]: super::TreeKind

/// Marker for a syntax tree produced by parsing, before typing.
pub enum Untyped {}

/// Marker for a syntax tree that has been type-checked.
pub enum Typed {}
