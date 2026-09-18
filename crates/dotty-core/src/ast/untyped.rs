//! Surface-syntax-only constructs, mirroring Dotty's actual `untpd`-only
//! node types (`ast/untpd.scala`).

use crate::ast::modifiers::Modifiers;
use crate::ast::phase::Untyped;
use crate::ids::{NameId, TreeId};
use crate::names::{Name, TermName};

/// One parser-level entry in a template's `uses` clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UseRef {
    pub reference: TreeId<Untyped>,
    pub initially: bool,
}

/// Syntax-only metadata attached to an untyped template.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UntypedTemplateMetadata {
    pub derives: Vec<TreeId<Untyped>>,
    pub uses: Vec<UseRef>,
}

/// The parser-level reason an expression, type, or pattern could not be
/// constructed. Diagnostics remain outside the AST; this enum only lets
/// recovery produce a structurally valid untyped tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorNodeKind {
    MissingExpression,
    MissingType,
    MissingPattern,
    UnexpectedToken,
}

/// A minimal placeholder inserted by parser recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorNode {
    pub kind: ErrorNodeKind,
}

/// `object Foo extends ... { ... }`, before desugaring into a synthetic
/// `ValDef` + module-class `TypeDef` pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleDef {
    pub name: TermName,
    pub template: TreeId<Untyped>,
}

/// `(params) => body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub params: Vec<TreeId<Untyped>>,
    pub body: TreeId<Untyped>,
}

/// `[type_params] => body`, a polymorphic function literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolyFunction {
    pub type_params: Vec<TreeId<Untyped>>,
    pub body: TreeId<Untyped>,
}

/// `left op right`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InfixOp {
    pub left: TreeId<Untyped>,
    pub op: Name,
    pub right: TreeId<Untyped>,
}

/// `op operand`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefixOp {
    pub op: Name,
    pub operand: TreeId<Untyped>,
}

/// `operand op` (legacy postfix syntax).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostfixOp {
    pub operand: TreeId<Untyped>,
    pub op: Name,
}

/// `(inner)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parens {
    pub inner: TreeId<Untyped>,
}

/// `(a, b, ...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tuple {
    pub elements: Vec<TreeId<Untyped>>,
}

/// `for (enums) yield body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForYield {
    pub enums: Vec<TreeId<Untyped>>,
    pub body: TreeId<Untyped>,
}

/// `for (enums) do body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForDo {
    pub enums: Vec<TreeId<Untyped>>,
    pub body: TreeId<Untyped>,
}

/// `pattern <- expr`, a for-comprehension generator.
///
/// Dotty's `GenFrom` also carries a `GenCheckMode` (whether the generator's
/// pattern match may fail and needs a filter inserted); that refinement is
/// omitted from the foundation model and can be added to this struct once
/// desugaring is implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenFrom {
    pub pattern: TreeId<Untyped>,
    pub expr: TreeId<Untyped>,
}

/// `pattern = expr`, a for-comprehension alias binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenAlias {
    pub pattern: TreeId<Untyped>,
    pub expr: TreeId<Untyped>,
}

/// `mods val (a, b) = rhs`, a pattern definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatDef {
    pub modifiers: Modifiers,
    pub patterns: Vec<TreeId<Untyped>>,
    pub tpt: TreeId<Untyped>,
    pub rhs: TreeId<Untyped>,
}

/// `extension (params) { methods }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionMethods {
    pub param_clauses: Vec<Vec<TreeId<Untyped>>>,
    pub methods: Vec<TreeId<Untyped>>,
}

/// `prefix"...${expr}..."`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpolatedString {
    pub prefix: Name,
    pub parts: Vec<TreeId<Untyped>>,
}

/// `def f[A: Show](...)`'s `: Show` context bound(s), before desugaring into
/// implicit parameters.
///
/// `context_bounds` is a `Vec`, not a single `TreeId`: Scala 3.9 allows more
/// than one bound on the same type parameter, either as `[A: Ord: Show]` or
/// the newer `[A: {Ord, Show}]` syntax — see
/// <https://docs.scala-lang.org/scala3/reference/contextual/context-bounds.html>.
/// Mirrors real Dotty's `untpd.ContextBounds(bounds: TypeBoundsTree, cxBounds: List[Tree])`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextBounds {
    pub bounds: TreeId<Untyped>,
    pub context_bounds: Vec<TreeId<Untyped>>,
}

/// A raw numeric literal, before the exact numeric type and overflow
/// checking are resolved. The parser does not own numeric overflow
/// checking (see `AGENTS.md`); `text` is the literal exactly as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberLiteral {
    pub text: NameId,
}

/// `throw expr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Throw {
    pub expr: TreeId<Untyped>,
}

/// `try expr` with the parser's catch handler still intact.
///
/// The handler may be a single catch expression or a case clause. Converting
/// it to the shared `Try` node's `Vec<CaseDef>` belongs to a later lowering
/// step, not to source parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedTry {
    pub expr: TreeId<Untyped>,
    pub handler: Option<TreeId<Untyped>>,
    pub finalizer: Option<TreeId<Untyped>>,
}

/// Surface-syntax-only constructs. This set mirrors Dotty's actual
/// `untpd`-only node types; it is not arbitrary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UntypedNode {
    Error(ErrorNode),

    ModuleDef(ModuleDef),

    Function(Function),
    PolyFunction(PolyFunction),

    InfixOp(InfixOp),
    PrefixOp(PrefixOp),
    PostfixOp(PostfixOp),

    Parens(Parens),
    Tuple(Tuple),

    ForYield(ForYield),
    ForDo(ForDo),

    GenFrom(GenFrom),
    GenAlias(GenAlias),

    PatDef(PatDef),

    ExtensionMethods(ExtensionMethods),

    InterpolatedString(InterpolatedString),

    ContextBounds(ContextBounds),

    Number(NumberLiteral),

    Throw(Throw),

    ParsedTry(ParsedTry),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::Namespace;

    fn tree_id(raw: u32) -> TreeId<Untyped> {
        TreeId::new(raw)
    }

    fn name(raw: u32) -> Name {
        Name::new(NameId::new(raw), Namespace::Term)
    }

    #[test]
    fn error_node_preserves_its_exact_recovery_kind() {
        let node = UntypedNode::Error(ErrorNode {
            kind: ErrorNodeKind::MissingExpression,
        });

        assert_eq!(
            node,
            UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::MissingExpression,
            })
        );
        assert_ne!(
            node,
            UntypedNode::Error(ErrorNode {
                kind: ErrorNodeKind::UnexpectedToken,
            })
        );
    }

    #[test]
    fn parsed_try_preserves_absent_handler_and_finalizer() {
        let parsed_try = ParsedTry {
            expr: tree_id(1),
            handler: None,
            finalizer: None,
        };

        assert_eq!(parsed_try.expr, tree_id(1));
        assert_eq!(parsed_try.handler, None);
        assert_eq!(parsed_try.finalizer, None);
    }

    #[test]
    fn parsed_try_preserves_a_handler_without_a_finalizer() {
        let parsed_try = ParsedTry {
            expr: tree_id(1),
            handler: Some(tree_id(2)),
            finalizer: None,
        };

        assert_eq!(parsed_try.handler, Some(tree_id(2)));
        assert_eq!(parsed_try.finalizer, None);
    }

    #[test]
    fn parsed_try_preserves_a_finalizer_without_a_handler() {
        let parsed_try = ParsedTry {
            expr: tree_id(1),
            handler: None,
            finalizer: Some(tree_id(3)),
        };

        assert_eq!(parsed_try.handler, None);
        assert_eq!(parsed_try.finalizer, Some(tree_id(3)));
    }

    #[test]
    fn parsed_try_preserves_both_handler_and_finalizer() {
        let parsed_try = ParsedTry {
            expr: tree_id(1),
            handler: Some(tree_id(2)),
            finalizer: Some(tree_id(3)),
        };

        assert_eq!(parsed_try.handler, Some(tree_id(2)));
        assert_eq!(parsed_try.finalizer, Some(tree_id(3)));
    }

    #[test]
    fn module_def_carries_its_name_and_template() {
        let node = UntypedNode::ModuleDef(ModuleDef {
            name: TermName::new(NameId::new(1)),
            template: tree_id(2),
        });

        assert_eq!(
            node,
            UntypedNode::ModuleDef(ModuleDef {
                name: TermName::new(NameId::new(1)),
                template: tree_id(2),
            })
        );
    }

    #[test]
    fn function_and_poly_function_are_distinguishable_despite_sharing_shape() {
        let function = UntypedNode::Function(Function {
            params: vec![tree_id(1)],
            body: tree_id(2),
        });
        let poly = UntypedNode::PolyFunction(PolyFunction {
            type_params: vec![tree_id(1)],
            body: tree_id(2),
        });

        assert_ne!(function, poly);
    }

    #[test]
    fn infix_prefix_and_postfix_ops_carry_their_operator_name() {
        let infix = InfixOp {
            left: tree_id(1),
            op: name(2),
            right: tree_id(3),
        };
        let prefix = PrefixOp {
            op: name(2),
            operand: tree_id(1),
        };
        let postfix = PostfixOp {
            operand: tree_id(1),
            op: name(2),
        };

        assert_ne!(UntypedNode::InfixOp(infix), UntypedNode::PrefixOp(prefix));
        assert_ne!(
            UntypedNode::PrefixOp(prefix),
            UntypedNode::PostfixOp(postfix)
        );
    }

    #[test]
    fn parens_and_tuple_carry_their_element_trees() {
        let parens = UntypedNode::Parens(Parens { inner: tree_id(1) });
        let tuple = UntypedNode::Tuple(Tuple {
            elements: vec![tree_id(1), tree_id(2)],
        });

        assert_ne!(parens, tuple);
    }

    #[test]
    fn for_yield_and_for_do_are_distinguishable_despite_sharing_shape() {
        let yield_form = UntypedNode::ForYield(ForYield {
            enums: vec![tree_id(1)],
            body: tree_id(2),
        });
        let do_form = UntypedNode::ForDo(ForDo {
            enums: vec![tree_id(1)],
            body: tree_id(2),
        });

        assert_ne!(yield_form, do_form);
    }

    #[test]
    fn gen_from_and_gen_alias_are_distinguishable_despite_sharing_shape() {
        let from = UntypedNode::GenFrom(GenFrom {
            pattern: tree_id(1),
            expr: tree_id(2),
        });
        let alias = UntypedNode::GenAlias(GenAlias {
            pattern: tree_id(1),
            expr: tree_id(2),
        });

        assert_ne!(from, alias);
    }

    #[test]
    fn pat_def_carries_its_modifiers_patterns_and_rhs() {
        let node = UntypedNode::PatDef(PatDef {
            modifiers: Modifiers::default(),
            patterns: vec![tree_id(1)],
            tpt: tree_id(2),
            rhs: tree_id(3),
        });

        assert_eq!(
            node,
            UntypedNode::PatDef(PatDef {
                modifiers: Modifiers::default(),
                patterns: vec![tree_id(1)],
                tpt: tree_id(2),
                rhs: tree_id(3),
            })
        );
    }

    #[test]
    fn extension_methods_carries_param_clauses_and_methods() {
        let node = UntypedNode::ExtensionMethods(ExtensionMethods {
            param_clauses: vec![vec![tree_id(1)]],
            methods: vec![tree_id(2)],
        });

        assert_eq!(
            node,
            UntypedNode::ExtensionMethods(ExtensionMethods {
                param_clauses: vec![vec![tree_id(1)]],
                methods: vec![tree_id(2)],
            })
        );
    }

    #[test]
    fn interpolated_string_carries_its_prefix_and_parts() {
        let node = UntypedNode::InterpolatedString(InterpolatedString {
            prefix: name(1),
            parts: vec![tree_id(2)],
        });

        assert_eq!(
            node,
            UntypedNode::InterpolatedString(InterpolatedString {
                prefix: name(1),
                parts: vec![tree_id(2)],
            })
        );
    }

    #[test]
    fn context_bounds_carries_its_bound_trees() {
        let node = UntypedNode::ContextBounds(ContextBounds {
            bounds: tree_id(1),
            context_bounds: vec![tree_id(2)],
        });

        assert_eq!(
            node,
            UntypedNode::ContextBounds(ContextBounds {
                bounds: tree_id(1),
                context_bounds: vec![tree_id(2)],
            })
        );
    }

    /// `def f[A: Ord: Show]` / `def f[A: {Ord, Show}]` — more than one
    /// context bound on the same type parameter.
    #[test]
    fn context_bounds_preserves_multiple_bounds_in_order() {
        let node = UntypedNode::ContextBounds(ContextBounds {
            bounds: tree_id(1),
            context_bounds: vec![tree_id(2), tree_id(3)],
        });

        let UntypedNode::ContextBounds(context_bounds) = &node else {
            panic!("expected a ContextBounds node");
        };
        assert_eq!(context_bounds.context_bounds, vec![tree_id(2), tree_id(3)]);
    }

    #[test]
    fn number_literal_and_throw_carry_their_payload() {
        let number = UntypedNode::Number(NumberLiteral {
            text: NameId::new(1),
        });
        let throw = UntypedNode::Throw(Throw { expr: tree_id(1) });

        assert_ne!(number, throw);
    }
}
