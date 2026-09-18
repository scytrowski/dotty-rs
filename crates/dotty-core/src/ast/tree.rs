//! [`Tree`] and [`TreeKind`]: the phase-indexed syntax/typed tree node.

use crate::ast::common::*;
use crate::ast::phase::AstPhase;
use crate::source::SourceSpan;

/// The kind of a [`Tree`] node.
///
/// Deliberately not exhaustive relative to Dotty: `Labeled` (desugared
/// gotos), `Hole` (quote-pickling only), `SeqLiteral`/`JavaSeqLiteral`
/// (desugared varargs), and the `InlineIf`/`InlineMatch`/`SubMatch`
/// boolean-flagged subclasses of `If`/`Match` are staged for when inline
/// handling and pattern desugaring are in scope — see
/// `docs/dotty-core-design.md` §7.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeKind<P: AstPhase> {
    Ident(Ident),
    Select(Select<P>),

    This(This),
    Super(Super<P>),

    Literal(Literal),

    Apply(Apply<P>),
    TypeApply(TypeApply<P>),

    New(New<P>),
    Typed(TypedExpr<P>),
    NamedArg(NamedArg<P>),

    Assign(Assign<P>),
    Block(Block<P>),
    If(If<P>),

    Match(Match<P>),
    CaseDef(CaseDef<P>),

    Return(Return<P>),
    While(While<P>),
    Try(Try<P>),

    Closure(Closure<P>),

    ValDef(ValDef<P>),
    DefDef(DefDef<P>),
    TypeDef(TypeDef<P>),
    Template(Template<P>),

    PackageDef(PackageDef<P>),

    Import(Import<P>),
    Export(Export<P>),

    TypeTree(TypeTree),
    SingletonTypeTree(SingletonTypeTree<P>),
    AppliedTypeTree(AppliedTypeTree<P>),
    RefinedTypeTree(RefinedTypeTree<P>),
    LambdaTypeTree(LambdaTypeTree<P>),
    MatchTypeTree(MatchTypeTree<P>),
    ByNameTypeTree(ByNameTypeTree<P>),
    TypeBoundsTree(TypeBoundsTree<P>),

    Bind(Bind<P>),
    Alternative(Alternative<P>),
    UnApply(UnApply<P>),

    Annotated(Annotated<P>),

    Quote(Quote<P>),
    Splice(Splice<P>),
    QuotePattern(QuotePattern<P>),
    SplicePattern(SplicePattern<P>),

    Inlined(Inlined<P>),

    /// Phase-only surface syntax: [`crate::ast::UntypedNode`] on the untyped
    /// phase, uninhabited (`Infallible`) on the typed phase — so a typed
    /// tree cannot be built from a real `UntypedNode` value:
    ///
    /// ```compile_fail
    /// use dotty_core::ast::{ErrorNode, ErrorNodeKind, Typed, TreeKind, UntypedNode};
    ///
    /// let node = UntypedNode::Error(ErrorNode {
    ///     kind: ErrorNodeKind::UnexpectedToken,
    /// });
    /// let _: TreeKind<Typed> = TreeKind::PhaseSpecific(node);
    /// ```
    PhaseSpecific(P::ExtraNode),
}

/// One node of a phase-indexed AST, allocated in an [`super::AstArena`].
///
/// `position` is `Option<SourceSpan>`, not a bare `SourceSpan`: not every
/// tree has a real source (compiler-generated trees, trees unpickled from
/// TASTy without position info, ...) — see `docs/dotty-core-design.md` §6,
/// `[BLOCKER 3]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Tree<P: AstPhase> {
    pub kind: TreeKind<P>,
    pub position: Option<SourceSpan>,
    pub ty: P::TypeInfo,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::phase::{Typed, Untyped};
    use crate::ids::{NameId, TypeId};
    use crate::names::{Name, Namespace};

    #[test]
    fn an_untyped_tree_carries_unit_type_info() {
        let tree = Tree::<Untyped> {
            kind: TreeKind::Ident(Ident {
                name: Name::new(NameId::new(1), Namespace::Term),
            }),
            position: None,
            ty: (),
        };

        assert_eq!(tree.ty, ());
    }

    #[test]
    fn a_typed_tree_always_carries_a_real_type_id() {
        let tree = Tree::<Typed> {
            kind: TreeKind::Ident(Ident {
                name: Name::new(NameId::new(1), Namespace::Term),
            }),
            position: None,
            ty: TypeId::new(7),
        };

        assert_eq!(tree.ty, TypeId::new(7));
    }
}
