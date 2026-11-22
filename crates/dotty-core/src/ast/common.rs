//! Node payloads shared by both AST phases.
//!
//! **Invariant:** every `TreeId` field here is `TreeId<P>` — never a
//! hard-coded `TreeId<Untyped>` or `TreeId<Typed>` regardless of the
//! enclosing phase `P`. Real Dotty's `Trees.scala` keeps a few fields
//! (`This.qual`, `Super.mix`, `Import`/`Export` selectors) permanently typed
//! as `untpd.*` because Dotty has no arena boundary between phases; `dotty-
//! core` does, so a hard-coded cross-phase `TreeId` would either be
//! unfillable (the TASTy adapter builds `AstArena<Typed>` with no
//! `AstArena<Untyped>` in existence at all) or a dangling-arena hazard. Where
//! Dotty's field is *only ever* a bare name with no children of its own
//! (`This.qual`, `Super.mix`), it becomes a plain `Option<Name>` here instead
//! of a tree reference. See `docs/dotty-core-design.md` §7, `[MAJOR 1]`.

use crate::ast::phase::AstPhase;
use crate::ids::TreeId;
use crate::names::{Name, TermName, TypeName};
use crate::types::{Constant, Variance};
use std::ops::Range;

/// `name`, preserving whether the source used backquotes around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ident {
    /// Namespace-aware name referenced by this identifier.
    pub name: Name,
    /// Whether the source spelling was enclosed in backquotes.
    pub backquoted: bool,
}

/// `qualifier.name`, or `qualifier#name` if `qualifier` is a type.
///
/// The `backquoted` flag preserves whether the selected name was written in
/// backquotes in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Select<P: AstPhase> {
    /// True for a source type projection (`T#Member`).
    pub is_type_projection: bool,
    /// Qualifier of the selected member or reference.
    pub qualifier: TreeId<P>,
    /// Namespace-aware name selected from the qualifier.
    pub name: Name,
    /// Whether the source spelling was enclosed in backquotes.
    pub backquoted: bool,
}

/// `qual.this`. `qual` is the optional enclosing class-name qualifier — a
/// bare name, not a tree, since it never has children of its own or a
/// semantic type independent of the enclosing class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct This {
    /// Optional enclosing class name qualifying this reference.
    pub qual: Option<Name>,
}

/// `C.super[mix]`, where `qual` is `C.this`. `mix` is the optional trait
/// qualifier, a bare name for the same reason as [`This::qual`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Super<P: AstPhase> {
    /// Current-instance tree qualifying the superclass reference.
    pub qual: TreeId<P>,
    /// Optional trait name qualifying a super reference.
    pub mix: Option<Name>,
}

/// A literal constant.
#[derive(Clone, Debug, PartialEq)]
pub struct Literal {
    /// Compile-time constant represented by this literal.
    pub value: Constant,
}

/// The kind of an [`Apply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyKind {
    /// Ordinary value argument application, such as `f(x)`.
    Regular,
    /// Explicit contextual argument application, such as `f(using x)`.
    Using,
}

/// `function(args)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Apply<P: AstPhase> {
    /// Callee expression in the same phase arena.
    pub function: TreeId<P>,
    /// Value argument trees in application order; contextual status is recorded separately by
    /// kind.
    pub args: Vec<TreeId<P>>,
    /// Whether the argument list is ordinary or explicitly contextual.
    pub kind: ApplyKind,
}

/// `function[args]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeApply<P: AstPhase> {
    /// Polymorphic callee or type constructor to which the explicit arguments apply.
    pub function: TreeId<P>,
    /// Explicit type-argument syntax in source order.
    pub args: Vec<TreeId<P>>,
}

/// `new tpt`, without a constructor call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct New<P: AstPhase> {
    /// Type syntax of the instance being constructed; constructor invocation is a separate
    /// application.
    pub tpt: TreeId<P>,
}

/// `expr: tpt`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedExpr<P: AstPhase> {
    /// Expression receiving the explicit type ascription.
    pub expr: TreeId<P>,
    /// Explicit ascribed type syntax.
    pub tpt: TreeId<P>,
}

/// `name = arg`, in a parameter list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedArg<P: AstPhase> {
    /// Parameter name targeted by this named argument.
    pub name: Name,
    /// Argument expression supplied for this named argument.
    pub arg: TreeId<P>,
}

/// `lhs = rhs`, outside a parameter list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assign<P: AstPhase> {
    /// Assignment target tree.
    pub lhs: TreeId<P>,
    /// Value expression assigned to the target.
    pub rhs: TreeId<P>,
}

/// `{ stats; expr }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block<P: AstPhase> {
    /// Statements in evaluation order preceding the block result.
    pub stats: Vec<TreeId<P>>,
    /// Final result expression evaluated after all statements.
    pub expr: TreeId<P>,
}

/// `if cond then then_branch else else_branch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct If<P: AstPhase> {
    /// Condition expression controlling this construct.
    pub cond: TreeId<P>,
    /// Expression evaluated when the condition is true.
    pub then_branch: TreeId<P>,
    /// Expression evaluated when the condition is false.
    pub else_branch: TreeId<P>,
}

/// `selector match { cases }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match<P: AstPhase> {
    /// Scrutinee expression matched against the ordered cases.
    pub selector: TreeId<P>,
    /// Case clauses in matching order.
    pub cases: Vec<TreeId<P>>,
}

/// `case pattern if guard => body`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseDef<P: AstPhase> {
    /// Pattern tested or bound by this construct.
    pub pattern: TreeId<P>,
    /// Optional case guard evaluated before the case body.
    pub guard: Option<TreeId<P>>,
    /// Expression evaluated after this case pattern and guard succeed.
    pub body: TreeId<P>,
}

/// `return expr`, optionally naming the enclosing method/label it returns
/// from (absent means "the immediately enclosing method").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Return<P: AstPhase> {
    /// Expression evaluated or described by this node.
    pub expr: Option<TreeId<P>>,
    /// Return target or starting point of the enclosing relation.
    pub from: Option<TreeId<P>>,
}

/// `while (cond) { body }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct While<P: AstPhase> {
    /// Condition expression controlling this construct.
    pub cond: TreeId<P>,
    /// Body tree or ordered body statements belonging to this node.
    pub body: TreeId<P>,
}

/// `try expr catch cases finally finalizer`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Try<P: AstPhase> {
    /// Expression evaluated or described by this node.
    pub expr: TreeId<P>,
    /// Case clauses in matching order.
    pub cases: Vec<TreeId<P>>,
    /// Optional finally expression evaluated after the protected body.
    pub finalizer: Option<TreeId<P>>,
}

/// A closure over `env`, referencing the method `method`; `tpt` is present
/// only when the closure's type is an explicit SAM type rather than a plain
/// function type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closure<P: AstPhase> {
    /// Captured environment trees in closure argument order.
    pub env: Vec<TreeId<P>>,
    /// Method-reference tree implementing the closure.
    pub method: TreeId<P>,
    /// Optional target function-type syntax supplied for this closure.
    pub tpt: Option<TreeId<P>>,
    /// Source-generated implementation and its ordered parameters, when available.
    pub implementation: Option<ClosureImplementation<P>>,
}

/// A closure's synthetic method definition in the same arena and session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosureImplementation<P: AstPhase> {
    /// Typed method definition referenced by the closure.
    pub definition: TreeId<P>,
    /// Formal symbols in the same order as the method definition.
    pub parameters: Vec<crate::SymbolId>,
}

/// `mods val name: tpt = rhs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValDef<P: AstPhase> {
    /// Interned name in the term namespace; resolve its text through the session interner.
    pub name: TermName,
    /// Declared type syntax, which can itself contain a parser-recovery placeholder.
    pub tpt: TreeId<P>,
    /// Right-hand-side tree, absent for a declaration without a definition.
    pub rhs: Option<TreeId<P>>,
    /// Definition metadata for the current phase: source modifiers before typing, unit after
    /// typing.
    pub metadata: P::DefMetadata,
}

/// An entry in a method's optional source-level parameter-clause ordering.
/// The ranges and indexes refer to the corresponding flattened fields on
/// [`DefDef`], avoiding duplicate tree references while preserving interleaved
/// type and term clauses (including empty clauses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefParamClauseOrder {
    /// Range into DefDef::type_params for this source type-parameter clause.
    TypeParams(Range<usize>),
    /// Zero-based index into DefDef::value_param_clauses for this source value clause.
    ValueParams(usize),
}

/// `mods def name[type_params](value_param_clauses): tpt = rhs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefDef<P: AstPhase> {
    /// Interned name in the term namespace; resolve its text through the session interner.
    pub name: TermName,
    /// Type parameter declarations in source or signature order.
    pub type_params: Vec<TreeId<P>>,
    /// Value parameter clauses in source order.
    pub value_param_clauses: Vec<Vec<TreeId<P>>>,
    /// Source clause order when it cannot be reconstructed from the legacy
    /// type-first fields. `None` means one leading type clause, followed by
    /// all value clauses.
    pub source_param_clause_order: Option<Vec<DefParamClauseOrder>>,
    /// Declared method result type syntax.
    pub tpt: TreeId<P>,
    /// Right-hand-side tree, absent for a declaration without a definition.
    pub rhs: Option<TreeId<P>>,
    /// Definition metadata for the current phase: source modifiers before typing, unit after
    /// typing.
    pub metadata: P::DefMetadata,
}

/// `mods class/trait/type name = rhs` (a `Template` when `rhs` is a class
/// body, a type-bounds/alias tree otherwise).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDef<P: AstPhase> {
    /// Interned name in the type namespace; resolve its text through the session interner.
    pub name: TypeName,
    /// Class template, alias target, or bounds tree defining this type declaration.
    pub rhs: TreeId<P>,
    /// Definition metadata for the current phase: source modifiers before typing, unit after
    /// typing.
    pub metadata: P::DefMetadata,
    /// Declared variance when this `TypeDef` is used as a type parameter.
    ///
    /// Type parameters share the existing `TypeDef` representation in the
    /// untyped AST. Keeping the variance here prevents `+A` and `-A` from
    /// collapsing into the same source tree as `A`.
    pub variance: Option<Variance>,
}

/// `extends parents { self_val => body }`.
///
/// Carries parser-only metadata through `P::TemplateMetadata`: untyped
/// templates retain `derives` and `uses`, while typed templates carry `()`.
/// The shared `parents` field remains separate because `derives` is not a
/// parent and is consumed before a typed template is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template<P: AstPhase> {
    /// Constructor declaration tree in the same phase arena.
    pub constructor: TreeId<P>,
    /// Parent type or constructor syntax in original source order.
    pub parents: Vec<TreeId<P>>,
    /// Optional self declaration in the template.
    pub self_val: Option<TreeId<P>>,
    /// Template members and statements in original source order.
    pub body: Vec<TreeId<P>>,
    /// Source derives/uses metadata before typing; unit after typing.
    pub metadata: P::TemplateMetadata,
}

/// `package name { stats }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDef<P: AstPhase> {
    /// Name or package-path syntax in the same phase arena.
    pub name: TreeId<P>,
    /// Package declarations, imports, and statements in original source order.
    pub stats: Vec<TreeId<P>>,
}

/// One entry of an `import`/`export` selector list, e.g. `foo`, `foo as
/// bar`, or `given T`. `renamed`/`bound` are `TreeId<P>` like every other
/// child reference (see the module-level invariant above) even though their
/// own semantic type is never meaningful.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportSelector<P: AstPhase> {
    /// Name selected by this import selector.
    pub imported: Name,
    /// Whether the imported name was written in backquotes.
    pub imported_backquoted: bool,
    /// Optional import rename target.
    pub renamed: Option<TreeId<P>>,
    /// Optional type bound used by a typed given selector.
    pub bound: Option<TreeId<P>>,
}

/// `import expr.{selectors}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import<P: AstPhase> {
    /// Qualifier from which the named members are imported or exported.
    pub expr: TreeId<P>,
    /// Import selectors or selected names in source order.
    pub selectors: Vec<ImportSelector<P>>,
}

/// `export expr.{selectors}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export<P: AstPhase> {
    /// Qualifier from which the named members are imported or exported.
    pub expr: TreeId<P>,
    /// Import selectors or selected names in source order.
    pub selectors: Vec<ImportSelector<P>>,
}

/// A type tree representing an existing or inferred type. Carries no data of
/// its own: its meaning comes entirely from the enclosing `Tree<P>::ty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TypeTree;

/// `ref.type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SingletonTypeTree<P: AstPhase> {
    /// Stable term path designated by the singleton type.
    pub reference: TreeId<P>,
}

/// `tpt { refinements }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinedTypeTree<P: AstPhase> {
    /// Parent type syntax extended by the refinement declarations.
    pub tpt: TreeId<P>,
    /// Refinement declarations in source order.
    pub refinements: Vec<TreeId<P>>,
}

/// `tpt[args]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedTypeTree<P: AstPhase> {
    /// Type constructor syntax to which the arguments apply.
    pub tpt: TreeId<P>,
    /// Type-argument syntax in application order.
    pub args: Vec<TreeId<P>>,
}

/// `[type_params] =>> body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LambdaTypeTree<P: AstPhase> {
    /// Type parameter declarations in source or signature order.
    pub type_params: Vec<TreeId<P>>,
    /// Result type syntax within the type-parameter binder.
    pub body: TreeId<P>,
}

/// `[bound] selector match { cases }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchTypeTree<P: AstPhase> {
    /// Optional declared upper bound of the match type.
    pub bound: Option<TreeId<P>>,
    /// Type syntax used as the match-type scrutinee.
    pub selector: TreeId<P>,
    /// Type case clauses in source matching order.
    pub cases: Vec<TreeId<P>>,
}

/// `=> result`, a by-name parameter type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByNameTypeTree<P: AstPhase> {
    /// Result type syntax of the by-name type.
    pub result: TreeId<P>,
}

/// `>: low <: high`, optionally `= alias` for a bounded opaque type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeBoundsTree<P: AstPhase> {
    /// Optional explicit lower type-bound syntax.
    pub low: Option<TreeId<P>>,
    /// Optional explicit upper type-bound syntax.
    pub high: Option<TreeId<P>>,
    /// Optional alias target syntax, retained distinctly from ordinary bounds.
    pub alias: Option<TreeId<P>>,
}

/// `name @ body`, a pattern binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bind<P: AstPhase> {
    /// Interned text identity and term/type namespace of the declaration or reference.
    pub name: Name,
    /// Nested pattern whose successful match introduces this binding.
    pub body: TreeId<P>,
    /// Marks Scala's `given T` pattern form, which Dotty represents as a
    /// wildcard bind carrying the `Given` modifier.
    pub given: bool,
}

/// `alt_1 | ... | alt_n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alternative<P: AstPhase> {
    /// Alternative patterns in source order.
    pub alternatives: Vec<TreeId<P>>,
}

/// `extractor(patterns)` in a pattern, i.e. `extractor.unapply` applied to
/// the match selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnApply<P: AstPhase> {
    /// Extractor function tree selected for this pattern.
    pub function: TreeId<P>,
    /// Implicit argument trees associated with this extractor application.
    pub implicits: Vec<TreeId<P>>,
    /// Patterns in their original source order.
    pub patterns: Vec<TreeId<P>>,
}

/// `expr: @annotation` (an annotated expression or type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Annotated<P: AstPhase> {
    /// Expression or type tree carrying the annotation.
    pub expr: TreeId<P>,
    /// Annotation syntax tree attached to the expression or type.
    pub annotation: TreeId<P>,
}

/// `'{ body }` or `'[ body ]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote<P: AstPhase> {
    /// Body tree or ordered body statements belonging to this node.
    pub body: TreeId<P>,
    /// Quoted type tags in source order.
    pub tags: Vec<TreeId<P>>,
}

/// `${ expr }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splice<P: AstPhase> {
    /// Expression evaluated or described by this node.
    pub expr: TreeId<P>,
}

/// `'{ bindings; body }` or `'[ bindings; body ]` in a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotePattern<P: AstPhase> {
    /// Pattern bindings in source order.
    pub bindings: Vec<TreeId<P>>,
    /// Pattern tree inside the quote.
    pub body: TreeId<P>,
    /// Tree providing the contextual Quotes value.
    pub quotes: TreeId<P>,
}

/// `${ body }`, `$ident`, or `$ident(args*)` in a quote pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplicePattern<P: AstPhase> {
    /// Body of the splice pattern.
    pub body: TreeId<P>,
    /// Type arguments in application order.
    pub type_args: Vec<TreeId<P>>,
    /// Term argument trees in splice application order.
    pub args: Vec<TreeId<P>>,
}

/// Inlined code: `bindings` are proxies for the inlined call's arguments,
/// `expansion` is the inlined body.
///
/// Does not carry Dotty's `call: tpd.Tree` (a permanently-typed back-
/// reference to the original call, used for diagnostics). Inline expansion
/// is out of scope for the foundation; adding call provenance back is
/// deferred to whichever future change implements the inliner — see
/// `docs/dotty-core-design.md` §7.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inlined<P: AstPhase> {
    /// Pattern bindings in source order.
    pub bindings: Vec<TreeId<P>>,
    /// Expanded tree retained by the inlined node.
    pub expansion: TreeId<P>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::phase::{Typed, Untyped};
    use crate::ids::NameId;
    use crate::names::Namespace;

    fn tree_id(raw: u32) -> TreeId<Untyped> {
        TreeId::new(raw)
    }

    fn name(raw: u32) -> Name {
        Name::new(NameId::new(raw), Namespace::Term)
    }

    #[test]
    fn ident_and_select_carry_a_name() {
        let ident = Ident {
            name: name(1),
            backquoted: false,
        };
        let select = Select {
            qualifier: tree_id(2),
            name: name(1),
            is_type_projection: false,
            backquoted: false,
        };

        assert_eq!(ident.name, select.name);
    }

    #[test]
    fn this_and_super_carry_an_optional_bare_name_qualifier() {
        let this_unqualified = This { qual: None };
        let this_qualified = This {
            qual: Some(name(1)),
        };
        let sup = Super {
            qual: tree_id(1),
            mix: Some(name(2)),
        };

        assert_ne!(this_unqualified, this_qualified);
        assert_eq!(sup.mix, Some(name(2)));
    }

    #[test]
    fn literal_wraps_a_constant() {
        let literal = Literal {
            value: Constant::Int(1),
        };

        assert_eq!(literal.value, Constant::Int(1));
    }

    #[test]
    fn apply_and_type_apply_are_distinguishable_despite_sharing_shape() {
        let apply = Apply {
            function: tree_id(1),
            args: vec![tree_id(2)],
            kind: ApplyKind::Regular,
        };
        let type_apply = TypeApply {
            function: tree_id(1),
            args: vec![tree_id(2)],
        };

        assert_eq!(apply.function, type_apply.function);
        assert_eq!(apply.kind, ApplyKind::Regular);
    }

    #[test]
    fn case_def_guard_is_optional() {
        let with_guard = CaseDef {
            pattern: tree_id(1),
            guard: Some(tree_id(2)),
            body: tree_id(3),
        };
        let without_guard = CaseDef {
            guard: None,
            ..with_guard
        };

        assert_ne!(with_guard, without_guard);
    }

    #[test]
    fn val_def_and_type_def_carry_phase_specific_metadata() {
        let val_def = ValDef::<Untyped> {
            name: TermName::new(NameId::new(1)),
            tpt: tree_id(2),
            rhs: Some(tree_id(3)),
            metadata: crate::ast::modifiers::Modifiers::default(),
        };
        let type_def = TypeDef::<Untyped> {
            name: TypeName::new(NameId::new(1)),
            rhs: tree_id(2),
            metadata: crate::ast::modifiers::Modifiers::default(),
            variance: None,
        };

        assert_eq!(val_def.tpt, tree_id(2));
        assert_eq!(type_def.rhs, tree_id(2));
    }

    #[test]
    fn def_def_separates_type_params_from_value_param_clauses() {
        let def_def = DefDef::<Untyped> {
            name: TermName::new(NameId::new(1)),
            type_params: vec![tree_id(2)],
            value_param_clauses: vec![vec![tree_id(3)], vec![tree_id(4)]],
            source_param_clause_order: None,
            tpt: tree_id(5),
            rhs: None,
            metadata: crate::ast::modifiers::Modifiers::default(),
        };

        assert_eq!(def_def.type_params, vec![tree_id(2)]);
        assert_eq!(def_def.value_param_clauses.len(), 2);
    }

    #[test]
    fn untyped_template_retains_derives_and_uses_metadata() {
        let template = Template::<Untyped> {
            constructor: tree_id(1),
            parents: vec![tree_id(2)],
            self_val: None,
            body: vec![tree_id(3)],
            metadata: crate::ast::untyped::UntypedTemplateMetadata {
                derives: vec![tree_id(4), tree_id(5)],
                uses: vec![
                    crate::ast::untyped::UseRef {
                        reference: tree_id(6),
                        initially: true,
                    },
                    crate::ast::untyped::UseRef {
                        reference: tree_id(7),
                        initially: false,
                    },
                ],
            },
        };

        assert_eq!(template.parents, vec![tree_id(2)]);
        assert_eq!(template.metadata.derives, vec![tree_id(4), tree_id(5)]);
        assert_eq!(template.metadata.uses[0].reference, tree_id(6));
        assert!(template.metadata.uses[0].initially);
        assert_eq!(template.metadata.uses[1].reference, tree_id(7));
        assert!(!template.metadata.uses[1].initially);
    }

    #[test]
    fn typed_template_has_no_untyped_template_metadata() {
        let template = Template::<Typed> {
            constructor: TreeId::new(1),
            parents: vec![TreeId::new(2)],
            self_val: None,
            body: vec![TreeId::new(3)],
            metadata: (),
        };

        assert_eq!(template.metadata, ());
    }

    #[test]
    fn import_and_export_selectors_use_phase_generic_tree_ids() {
        let selector = ImportSelector {
            imported: name(1),
            imported_backquoted: false,
            renamed: Some(tree_id(2)),
            bound: None,
        };
        let import = Import {
            expr: tree_id(3),
            selectors: vec![selector],
        };
        let export = Export {
            expr: tree_id(3),
            selectors: vec![selector],
        };

        assert_eq!(import.expr, export.expr);
        assert_eq!(import.selectors, export.selectors);
    }

    #[test]
    fn type_tree_carries_no_data_of_its_own() {
        assert_eq!(TypeTree, TypeTree);
    }

    #[test]
    fn quote_and_splice_are_distinguishable_despite_related_shapes() {
        let quote = Quote {
            body: tree_id(1),
            tags: vec![],
        };
        let splice = Splice { expr: tree_id(1) };

        assert_eq!(quote.body, splice.expr);
    }

    #[test]
    fn inlined_has_no_call_provenance_field() {
        let inlined = Inlined {
            bindings: vec![tree_id(1)],
            expansion: tree_id(2),
        };

        assert_eq!(inlined.expansion, tree_id(2));
    }
}
