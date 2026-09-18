//! AST phase markers and the [`AstPhase`] trait.

use std::convert::Infallible;
use std::fmt::Debug;

use crate::ast::modifiers::Modifiers;
use crate::ast::untyped::{UntypedNode, UntypedTemplateMetadata};
use crate::ids::TypeId;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Untyped {}
    impl Sealed for super::Typed {}
}

/// Distinguishes an untyped syntax tree from a typed one at the type level.
///
/// - `TypeInfo` is the type every [`Tree`](super::Tree) carries: `()` before
///   typing, a real `TypeId` after.
/// - `ExtraNode` is the phase-only node payload: [`UntypedNode`] before
///   typing, uninhabited (`Infallible`) after — so a typed tree cannot
///   contain surface-syntax-only constructs.
/// - `DefMetadata` is the source-level [`Modifiers`] before typing, `()`
///   after — a typed definition node cannot carry syntactic modifiers (see
///   `docs/dotty-core-design.md` §7, `[MAJOR 2]`).
/// - `TemplateMetadata` is parser-only template syntax before typing and `()`
///   after — a typed template cannot retain `derives` or `uses` metadata.
///
/// The associated types carry `Clone + Debug + PartialEq` bounds (and
/// `DefMetadata` additionally `Eq`) so that every generic node payload
/// parameterized over `P: AstPhase` can derive those traits itself: derive
/// macros generate a bound on the *generic parameter* (`P: Debug`), which
/// does not by itself imply an associated-type projection like
/// `P::TypeInfo: Debug` — these supertrait bounds are what closes that gap.
pub trait AstPhase: sealed::Sealed {
    type TypeInfo: Clone + Debug + PartialEq;
    type ExtraNode: Clone + Debug + PartialEq;
    type DefMetadata: Clone + Debug + PartialEq + Eq;
    type TemplateMetadata: Clone + Debug + PartialEq + Eq;
}

/// Marker for a syntax tree produced by parsing, before typing.
///
/// Derives are present (despite the type being uninhabited) purely so that
/// generic node payloads like `Select<P>` can `#[derive(PartialEq, ...)]`
/// without the derive macro's naive `P: PartialEq` bound going unsatisfied —
/// the impls it generates for an empty enum are vacuously trivial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Untyped {}

/// Marker for a syntax tree that has been type-checked. See [`Untyped`] for
/// why this derives traits despite being uninhabited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Typed {}

impl AstPhase for Untyped {
    type TypeInfo = ();
    type ExtraNode = UntypedNode;
    type DefMetadata = Modifiers;
    type TemplateMetadata = UntypedTemplateMetadata;
}

impl AstPhase for Typed {
    type TypeInfo = TypeId;
    type ExtraNode = Infallible;
    type DefMetadata = ();
    type TemplateMetadata = ();
}
