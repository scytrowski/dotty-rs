//! [`Tree`] and [`TreeKind`]: the phase-indexed syntax/typed tree node.

use crate::ast::common::*;
use crate::ast::phase::AstPhase;
use crate::source::SourceSpan;

/// The kind of a [`Tree`] node.
///
/// Deliberately not exhaustive relative to Dotty: `Labeled` (desugared
/// gotos), `Hole` (quote-pickling only), and `SeqLiteral`/`JavaSeqLiteral`
/// (desugared varargs) are not modeled here. Inline-marked `If` and `Match`
/// source forms live in `UntypedNode`; the typed `If`/`Match` contracts remain
/// unchanged. `SubMatch` is staged for pattern desugaring — see
/// `docs/dotty-core-design.md` §7.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeKind<P: AstPhase> {
    /// Unqualified identifier reference, retaining its namespace and backquotes.
    Ident(Ident),
    /// Qualified member selection or type projection.
    Select(Select<P>),

    /// Current-instance reference with an optional enclosing-class qualifier.
    This(This),
    /// Superclass or trait-qualified super reference.
    Super(Super<P>),

    /// Literal constant value.
    Literal(Literal),

    /// Value application, with ordinary or explicit contextual arguments.
    Apply(Apply<P>),
    /// Explicit type argument application.
    TypeApply(TypeApply<P>),

    /// Instance construction type syntax; a constructor call is represented separately.
    New(New<P>),
    /// Expression with an explicit type ascription.
    Typed(TypedExpr<P>),
    /// Named argument within an application.
    NamedArg(NamedArg<P>),

    /// Assignment to a mutable target.
    Assign(Assign<P>),
    /// Ordered statements followed by a result expression.
    Block(Block<P>),
    /// Conditional expression with then and else branches.
    If(If<P>),

    /// Pattern match with cases in matching order.
    Match(Match<P>),
    /// One pattern, optional guard, and case body.
    CaseDef(CaseDef<P>),

    /// Return expression with an optional explicit return target.
    Return(Return<P>),
    /// Loop condition and body.
    While(While<P>),
    /// Protected expression, case handlers, and optional finalizer.
    Try(Try<P>),

    /// Closure method reference and captured environment.
    Closure(Closure<P>),

    /// Value, variable, or value-parameter declaration.
    ValDef(ValDef<P>),
    /// Method declaration with parameter clauses and optional body.
    DefDef(DefDef<P>),
    /// Type or class declaration.
    TypeDef(TypeDef<P>),
    /// Parents, constructor, self declaration, and ordered template members.
    Template(Template<P>),

    /// Package declaration and its ordered statements.
    PackageDef(PackageDef<P>),

    /// Import qualifier and selectors.
    Import(Import<P>),
    /// Export qualifier and selectors.
    Export(Export<P>),

    /// Explicit or inferred type syntax payload.
    TypeTree(TypeTree),
    /// Singleton type syntax referring to a stable term.
    SingletonTypeTree(SingletonTypeTree<P>),
    /// Type constructor applied to type arguments.
    AppliedTypeTree(AppliedTypeTree<P>),
    /// Parent type with structural member refinements.
    RefinedTypeTree(RefinedTypeTree<P>),
    /// Type lambda with bound parameters and result syntax.
    LambdaTypeTree(LambdaTypeTree<P>),
    /// Match type syntax with bound, scrutinee, and ordered cases.
    MatchTypeTree(MatchTypeTree<P>),
    /// By-name type syntax.
    ByNameTypeTree(ByNameTypeTree<P>),
    /// Lower and upper type bound syntax.
    TypeBoundsTree(TypeBoundsTree<P>),

    /// Pattern binding introducing a named value.
    Bind(Bind<P>),
    /// Alternative patterns tested against the same scrutinee.
    Alternative(Alternative<P>),
    /// Extractor pattern with type, contextual, and pattern arguments.
    UnApply(UnApply<P>),

    /// Expression or type carrying an annotation.
    Annotated(Annotated<P>),

    /// Quoted expression or type.
    Quote(Quote<P>),
    /// Spliced expression or type.
    Splice(Splice<P>),
    /// Quoted pattern syntax.
    QuotePattern(QuotePattern<P>),
    /// Spliced pattern syntax.
    SplicePattern(SplicePattern<P>),

    /// Inlined call, bindings, and resulting expansion.
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
    /// Phase-indexed syntax node payload; child IDs refer to the same arena.
    pub kind: TreeKind<P>,
    /// Optional source location; absent for nodes without source provenance.
    pub position: Option<SourceSpan>,
    /// Type payload for the current phase: unit before typing, a semantic type identity after
    /// typing.
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
                backquoted: false,
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
                backquoted: false,
            }),
            position: None,
            ty: TypeId::new(7),
        };

        assert_eq!(tree.ty, TypeId::new(7));
    }
}
