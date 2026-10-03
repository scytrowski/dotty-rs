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
    pub name: Name,
    pub backquoted: bool,
}

/// `qualifier.name`, or `qualifier#name` if `qualifier` is a type.
///
/// The `backquoted` flag preserves whether the selected name was written in
/// backquotes in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Select<P: AstPhase> {
    pub qualifier: TreeId<P>,
    pub name: Name,
    pub backquoted: bool,
}

/// `qual.this`. `qual` is the optional enclosing class-name qualifier — a
/// bare name, not a tree, since it never has children of its own or a
/// semantic type independent of the enclosing class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct This {
    pub qual: Option<Name>,
}

/// `C.super[mix]`, where `qual` is `C.this`. `mix` is the optional trait
/// qualifier, a bare name for the same reason as [`This::qual`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Super<P: AstPhase> {
    pub qual: TreeId<P>,
    pub mix: Option<Name>,
}

/// A literal constant.
#[derive(Clone, Debug, PartialEq)]
pub struct Literal {
    pub value: Constant,
}

/// The kind of an [`Apply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyKind {
    Regular,
    Using,
}

/// `function(args)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Apply<P: AstPhase> {
    pub function: TreeId<P>,
    pub args: Vec<TreeId<P>>,
    pub kind: ApplyKind,
}

/// `function[args]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeApply<P: AstPhase> {
    pub function: TreeId<P>,
    pub args: Vec<TreeId<P>>,
}

/// `new tpt`, without a constructor call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct New<P: AstPhase> {
    pub tpt: TreeId<P>,
}

/// `expr: tpt`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedExpr<P: AstPhase> {
    pub expr: TreeId<P>,
    pub tpt: TreeId<P>,
}

/// `name = arg`, in a parameter list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedArg<P: AstPhase> {
    pub name: Name,
    pub arg: TreeId<P>,
}

/// `lhs = rhs`, outside a parameter list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assign<P: AstPhase> {
    pub lhs: TreeId<P>,
    pub rhs: TreeId<P>,
}

/// `{ stats; expr }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block<P: AstPhase> {
    pub stats: Vec<TreeId<P>>,
    pub expr: TreeId<P>,
}

/// `if cond then then_branch else else_branch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct If<P: AstPhase> {
    pub cond: TreeId<P>,
    pub then_branch: TreeId<P>,
    pub else_branch: TreeId<P>,
}

/// `selector match { cases }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match<P: AstPhase> {
    pub selector: TreeId<P>,
    pub cases: Vec<TreeId<P>>,
}

/// `case pattern if guard => body`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseDef<P: AstPhase> {
    pub pattern: TreeId<P>,
    pub guard: Option<TreeId<P>>,
    pub body: TreeId<P>,
}

/// `return expr`, optionally naming the enclosing method/label it returns
/// from (absent means "the immediately enclosing method").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Return<P: AstPhase> {
    pub expr: Option<TreeId<P>>,
    pub from: Option<TreeId<P>>,
}

/// `while (cond) { body }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct While<P: AstPhase> {
    pub cond: TreeId<P>,
    pub body: TreeId<P>,
}

/// `try expr catch cases finally finalizer`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Try<P: AstPhase> {
    pub expr: TreeId<P>,
    pub cases: Vec<TreeId<P>>,
    pub finalizer: Option<TreeId<P>>,
}

/// A closure over `env`, referencing the method `method`; `tpt` is present
/// only when the closure's type is an explicit SAM type rather than a plain
/// function type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closure<P: AstPhase> {
    pub env: Vec<TreeId<P>>,
    pub method: TreeId<P>,
    pub tpt: Option<TreeId<P>>,
}

/// `mods val name: tpt = rhs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValDef<P: AstPhase> {
    pub name: TermName,
    pub tpt: TreeId<P>,
    pub rhs: Option<TreeId<P>>,
    pub metadata: P::DefMetadata,
}

/// An entry in a method's optional source-level parameter-clause ordering.
/// The ranges and indexes refer to the corresponding flattened fields on
/// [`DefDef`], avoiding duplicate tree references while preserving interleaved
/// type and term clauses (including empty clauses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefParamClauseOrder {
    TypeParams(Range<usize>),
    ValueParams(usize),
}

/// `mods def name[type_params](value_param_clauses): tpt = rhs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefDef<P: AstPhase> {
    pub name: TermName,
    pub type_params: Vec<TreeId<P>>,
    pub value_param_clauses: Vec<Vec<TreeId<P>>>,
    /// Source clause order when it cannot be reconstructed from the legacy
    /// type-first fields. `None` means one leading type clause, followed by
    /// all value clauses.
    pub source_param_clause_order: Option<Vec<DefParamClauseOrder>>,
    pub tpt: TreeId<P>,
    pub rhs: Option<TreeId<P>>,
    pub metadata: P::DefMetadata,
}

/// `mods class/trait/type name = rhs` (a `Template` when `rhs` is a class
/// body, a type-bounds/alias tree otherwise).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDef<P: AstPhase> {
    pub name: TypeName,
    pub rhs: TreeId<P>,
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
    pub constructor: TreeId<P>,
    pub parents: Vec<TreeId<P>>,
    pub self_val: Option<TreeId<P>>,
    pub body: Vec<TreeId<P>>,
    pub metadata: P::TemplateMetadata,
}

/// `package name { stats }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDef<P: AstPhase> {
    pub name: TreeId<P>,
    pub stats: Vec<TreeId<P>>,
}

/// One entry of an `import`/`export` selector list, e.g. `foo`, `foo as
/// bar`, or `given T`. `renamed`/`bound` are `TreeId<P>` like every other
/// child reference (see the module-level invariant above) even though their
/// own semantic type is never meaningful.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportSelector<P: AstPhase> {
    pub imported: Name,
    pub imported_backquoted: bool,
    pub renamed: Option<TreeId<P>>,
    pub bound: Option<TreeId<P>>,
}

/// `import expr.{selectors}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import<P: AstPhase> {
    pub expr: TreeId<P>,
    pub selectors: Vec<ImportSelector<P>>,
}

/// `export expr.{selectors}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export<P: AstPhase> {
    pub expr: TreeId<P>,
    pub selectors: Vec<ImportSelector<P>>,
}

/// A type tree representing an existing or inferred type. Carries no data of
/// its own: its meaning comes entirely from the enclosing `Tree<P>::ty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TypeTree;

/// `ref.type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SingletonTypeTree<P: AstPhase> {
    pub reference: TreeId<P>,
}

/// `tpt { refinements }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinedTypeTree<P: AstPhase> {
    pub tpt: TreeId<P>,
    pub refinements: Vec<TreeId<P>>,
}

/// `tpt[args]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedTypeTree<P: AstPhase> {
    pub tpt: TreeId<P>,
    pub args: Vec<TreeId<P>>,
}

/// `[type_params] =>> body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LambdaTypeTree<P: AstPhase> {
    pub type_params: Vec<TreeId<P>>,
    pub body: TreeId<P>,
}

/// `[bound] selector match { cases }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchTypeTree<P: AstPhase> {
    pub bound: Option<TreeId<P>>,
    pub selector: TreeId<P>,
    pub cases: Vec<TreeId<P>>,
}

/// `=> result`, a by-name parameter type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByNameTypeTree<P: AstPhase> {
    pub result: TreeId<P>,
}

/// `>: low <: high`, optionally `= alias` for a bounded opaque type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeBoundsTree<P: AstPhase> {
    pub low: Option<TreeId<P>>,
    pub high: Option<TreeId<P>>,
    pub alias: Option<TreeId<P>>,
}

/// `name @ body`, a pattern binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bind<P: AstPhase> {
    pub name: Name,
    pub body: TreeId<P>,
    /// Marks Scala's `given T` pattern form, which Dotty represents as a
    /// wildcard bind carrying the `Given` modifier.
    pub given: bool,
}

/// `alt_1 | ... | alt_n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alternative<P: AstPhase> {
    pub alternatives: Vec<TreeId<P>>,
}

/// `extractor(patterns)` in a pattern, i.e. `extractor.unapply` applied to
/// the match selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnApply<P: AstPhase> {
    pub function: TreeId<P>,
    pub implicits: Vec<TreeId<P>>,
    pub patterns: Vec<TreeId<P>>,
}

/// `expr: @annotation` (an annotated expression or type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Annotated<P: AstPhase> {
    pub expr: TreeId<P>,
    pub annotation: TreeId<P>,
}

/// `'{ body }` or `'[ body ]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote<P: AstPhase> {
    pub body: TreeId<P>,
    pub tags: Vec<TreeId<P>>,
}

/// `${ expr }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splice<P: AstPhase> {
    pub expr: TreeId<P>,
}

/// `'{ bindings; body }` or `'[ bindings; body ]` in a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotePattern<P: AstPhase> {
    pub bindings: Vec<TreeId<P>>,
    pub body: TreeId<P>,
    pub quotes: TreeId<P>,
}

/// `${ body }`, `$ident`, or `$ident(args*)` in a quote pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplicePattern<P: AstPhase> {
    pub body: TreeId<P>,
    pub type_args: Vec<TreeId<P>>,
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
    pub bindings: Vec<TreeId<P>>,
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
