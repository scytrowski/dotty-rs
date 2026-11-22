//! Surface-syntax-only constructs, mirroring Dotty's actual `untpd`-only
//! node types (`ast/untpd.scala`).

use crate::ast::modifiers::Modifiers;
use crate::ast::phase::Untyped;
use crate::ids::{NameId, TreeId};
use crate::names::{Name, TermName, TypeName};

/// One parser-level entry in a template's `uses` clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UseRef {
    /// Source path syntax named by this uses-clause entry.
    pub reference: TreeId<Untyped>,
    /// Whether this use reference carries the initially marker.
    pub initially: bool,
}

/// Syntax-only metadata attached to an untyped template.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UntypedTemplateMetadata {
    /// Derived type syntax in source order.
    pub derives: Vec<TreeId<Untyped>>,
    /// Use-clause entries or service references in declaration order.
    pub uses: Vec<UseRef>,
}

/// The parser-level reason an expression, type, or pattern could not be
/// constructed. Diagnostics remain outside the AST; this enum only lets
/// recovery produce a structurally valid untyped tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorNodeKind {
    /// Recovery placeholder where an expression was required.
    MissingExpression,
    /// Recovery placeholder where type syntax was required.
    MissingType,
    /// Recovery placeholder where a pattern was required.
    MissingPattern,
    /// Recovery placeholder for a token invalid in this grammar position.
    UnexpectedToken,
}

/// A minimal placeholder inserted by parser recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorNode {
    /// Grammar form missing or rejected during parser recovery.
    pub kind: ErrorNodeKind,
}

/// `object Foo extends ... { ... }`, before desugaring into a synthetic
/// `ValDef` + module-class `TypeDef` pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleDef {
    /// Interned name in the term namespace; resolve its text through the session interner.
    pub name: TermName,
    /// Template tree defining the object body and parents.
    pub template: TreeId<Untyped>,
    /// Source modifiers and visibility before semantic lowering.
    pub metadata: Modifiers,
}

/// `(params) => body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// Original value-parameter declaration trees in source order.
    pub params: Vec<TreeId<Untyped>>,
    /// Original function body before typing and closure lowering.
    pub body: TreeId<Untyped>,
}

/// A function type whose parser-level modifiers or erased parameters must be
/// retained before typing, such as `(using A) ?=> B` or `(erased A) => B`.
///
/// `erased_params` has one entry per parameter and remains separate from
/// `modifiers`: erasure is positional parameter syntax, not a modifier on the
/// whole function type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionWithMods {
    /// Parameter type syntax in source order, aligned with erased_params.
    pub params: Vec<TreeId<Untyped>>,
    /// Result type syntax of the source function type.
    pub result: TreeId<Untyped>,
    /// Source modifiers retained on this syntax node.
    pub modifiers: Modifiers,
    /// Per-parameter erasure flags, aligned with the parameter list.
    pub erased_params: Vec<bool>,
}

/// Capture references attached to a function type's result before typing,
/// corresponding to Dotty's `untpd.CapturesAndResult` node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturesAndResult {
    /// Capture references attached to the function result.
    pub captures: Vec<TreeId<Untyped>>,
    /// Function result type syntax to which the capture references attach.
    pub result: TreeId<Untyped>,
}

/// `[type_params] => body`, a polymorphic function literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolyFunction {
    /// Type parameter declarations in source or signature order.
    pub type_params: Vec<TreeId<Untyped>>,
    /// Body tree or ordered body statements belonging to this node.
    pub body: TreeId<Untyped>,
}

/// `left op right`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InfixOp {
    /// Left operand of the enclosing relation or syntax node.
    pub left: TreeId<Untyped>,
    /// Interned operator name.
    pub op: Name,
    /// Right operand of the enclosing relation or syntax node.
    pub right: TreeId<Untyped>,
}

/// `op operand`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefixOp {
    /// Interned operator name.
    pub op: Name,
    /// Operand of the prefix or postfix operator.
    pub operand: TreeId<Untyped>,
}

/// `operand op` (legacy postfix syntax).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostfixOp {
    /// Operand of the prefix or postfix operator.
    pub operand: TreeId<Untyped>,
    /// Interned operator name.
    pub op: Name,
}

/// `(inner)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parens {
    /// Expression inside the explicit parentheses.
    pub inner: TreeId<Untyped>,
}

/// `(a, b, ...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tuple {
    /// Elements in their original source or annotation order.
    pub elements: Vec<TreeId<Untyped>>,
}

/// `for (enums) yield body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForYield {
    /// For-comprehension enumerators in source order.
    pub enums: Vec<TreeId<Untyped>>,
    /// Body tree or ordered body statements belonging to this node.
    pub body: TreeId<Untyped>,
}

/// `for (enums) do body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForDo {
    /// For-comprehension enumerators in source order.
    pub enums: Vec<TreeId<Untyped>>,
    /// Body tree or ordered body statements belonging to this node.
    pub body: TreeId<Untyped>,
}

/// The parser/lowering policy for a for-comprehension generator pattern.
///
/// The modes mirror Scala 3.9's `untpd.GenCheckMode`. The source version and
/// an optional `case` prefix determine which mode the parser emits, so this
/// information must survive the parser boundary until for-comprehension
/// lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenCheckMode {
    /// Do not insert a generator pattern check.
    Ignore,
    /// Generator whose pattern has already been filtered.
    Filtered,
    /// Require an irrefutable generator pattern.
    Check,
    /// Check the pattern and retain filtering behavior.
    CheckAndFilter,
    /// Filter this generator pattern during desugaring.
    FilterNow,
    /// Retain an explicit filtering requirement for this pattern.
    FilterAlways,
}

/// `pattern <- expr`, a for-comprehension generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenFrom {
    /// Pattern tested or bound by this construct.
    pub pattern: TreeId<Untyped>,
    /// Collection or source expression supplying elements to this generator.
    pub expr: TreeId<Untyped>,
    /// Pattern-checking and filtering mode for this generator.
    pub check_mode: GenCheckMode,
}

/// `pattern = expr`, a for-comprehension alias binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenAlias {
    /// Pattern tested or bound by this construct.
    pub pattern: TreeId<Untyped>,
    /// Value expression bound by this enumerator alias.
    pub expr: TreeId<Untyped>,
}

/// `mods val (a, b) [= rhs]`, a pattern definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatDef {
    /// Source modifiers retained on this syntax node.
    pub modifiers: Modifiers,
    /// Patterns in their original source order.
    pub patterns: Vec<TreeId<Untyped>>,
    /// Type syntax attached to this expression or declaration.
    pub tpt: TreeId<Untyped>,
    /// The optional RHS preserves parser-level declarations such as `val x: A`.
    pub rhs: Option<TreeId<Untyped>>,
}

/// `extension (params) { methods }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionMethods {
    /// Parameter syntax grouped into clauses, in source order.
    pub param_clauses: Vec<Vec<TreeId<Untyped>>>,
    /// Declared methods in source or class-file order.
    pub methods: Vec<TreeId<Untyped>>,
}

/// `prefix"...${expr}..."`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpolatedString {
    /// Interned name of the selected string interpolator.
    pub prefix: Name,
    /// Literal string parts and embedded expression trees in source order.
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
    /// Ordinary lower/upper bounds tree, retained separately from context bounds.
    pub bounds: TreeId<Untyped>,
    /// Context-bound syntax attached to this parameter.
    pub context_bounds: Vec<TreeId<Untyped>>,
}

/// One Scala 3.9 context bound, including its optional `as` alias.
///
/// For example, `A: Show as show` is retained as a bound tree for `Show`,
/// `parameter = A`, and `name = Some(show)`. `ContextBounds` owns these
/// nodes in source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextBoundTypeTree {
    /// Bound type or syntax constraining this declaration.
    pub bound: TreeId<Untyped>,
    /// Type parameter name to which this context bound applies.
    pub parameter: TypeName,
    /// Optional explicit witness name introduced by an as clause.
    pub name: Option<TermName>,
}

/// The lexical classification of an unsuffixed numeric literal.
///
/// `Whole` retains the source radix because Scala's parser passes the token
/// base through to `untpd.Number`. `Decimal` and `Floating` distinguish the
/// parser's decimal and exponent forms before typing chooses a numeric type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberKind {
    /// Whole-number spelling with its source radix, before numeric type selection.
    Whole(u32),
    /// Numeric spelling containing a decimal point.
    Decimal,
    /// Unsuffixed exponent-form numeric spelling, before numeric type selection.
    Floating,
}

/// A raw numeric literal, before the exact numeric type and overflow
/// checking are resolved. The parser does not own numeric overflow checking
/// (see `AGENTS.md`); `text` is the literal exactly as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberLiteral {
    /// Interned numeric literal spelling exactly as written, before overflow checking.
    pub text: NameId,
    /// Numeric lexical form and, for whole numbers, source radix.
    pub kind: NumberKind,
}

/// `throw expr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Throw {
    /// Exception expression thrown by this source construct.
    pub expr: TreeId<Untyped>,
}

/// `try expr` with the parser's catch handler still intact.
///
/// The handler may be a single catch expression or a case clause. Converting
/// it to the shared `Try` node's `Vec<CaseDef>` belongs to a later lowering
/// step, not to source parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedTry {
    /// Expression evaluated or described by this node.
    pub expr: TreeId<Untyped>,
    /// Optional parsed exception handler expression.
    pub handler: Option<TreeId<Untyped>>,
    /// Optional finally expression evaluated after the protected body.
    pub finalizer: Option<TreeId<Untyped>>,
}

/// `inline if cond then thenBranch else elseBranch`.
///
/// Dotty represents this as an inline-marked `If` subclass. Keep the marker
/// in the untyped tree without widening the shared typed `If` contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InlineIf {
    /// Condition expression controlling this construct.
    pub cond: TreeId<Untyped>,
    /// Expression evaluated when the condition is true.
    pub then_branch: TreeId<Untyped>,
    /// Expression evaluated when the condition is false.
    pub else_branch: TreeId<Untyped>,
}

/// `inline selector match cases`.
///
/// Dotty represents this as an inline-marked `Match` subclass. The marker is
/// source syntax for later inline handling and is therefore untyped-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineMatch {
    /// Scrutinee expression or member selector for this construct.
    pub selector: TreeId<Untyped>,
    /// Case clauses in matching order.
    pub cases: Vec<TreeId<Untyped>>,
}

/// `macro expr`, before the macro implementation is expanded or desugared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacroTree {
    /// Source expression retained inside the macro form.
    pub expr: TreeId<Untyped>,
}

/// Surface-syntax-only constructs. This set mirrors Dotty's actual
/// `untpd`-only node types; it is not arbitrary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UntypedNode {
    /// Parser-recovery placeholder; consult the separate diagnostics.
    Error(ErrorNode),

    /// Object declaration before module-value and module-class desugaring.
    ModuleDef(ModuleDef),

    /// Function literal with ordered parameters and a body.
    Function(Function),
    /// Function type retaining source modifiers and positional erased parameters.
    FunctionWithMods(FunctionWithMods),
    /// Capture-set references attached to a function result.
    CapturesAndResult(CapturesAndResult),
    /// Polymorphic function literal with type parameters.
    PolyFunction(PolyFunction),

    /// Source infix operation before application desugaring.
    InfixOp(InfixOp),
    /// Source prefix operation before application desugaring.
    PrefixOp(PrefixOp),
    /// Source postfix operation before application desugaring.
    PostfixOp(PostfixOp),

    /// Explicitly parenthesized source tree.
    Parens(Parens),
    /// Tuple syntax before tuple construction or type desugaring.
    Tuple(Tuple),

    /// For-comprehension producing a result value.
    ForYield(ForYield),
    /// For-comprehension executed for effects.
    ForDo(ForDo),

    /// Generator pattern and source expression with its checking mode.
    GenFrom(GenFrom),
    /// Value alias within a for-comprehension.
    GenAlias(GenAlias),

    /// Pattern-based value or variable definition before expansion.
    PatDef(PatDef),

    /// Extension parameter clauses and member definitions.
    ExtensionMethods(ExtensionMethods),

    /// Interpolator prefix and ordered string parts and holes.
    InterpolatedString(InterpolatedString),

    /// Context-bound syntax associated with a type parameter.
    ContextBounds(ContextBounds),
    /// One context-bound type and its optional witness name.
    ContextBoundTypeTree(ContextBoundTypeTree),

    /// Numeric spelling retained before semantic constant interpretation.
    Number(NumberLiteral),

    /// Source throw expression.
    Throw(Throw),

    /// Conditional carrying the inline source marker.
    InlineIf(InlineIf),
    /// Pattern match carrying the inline source marker.
    InlineMatch(InlineMatch),
    /// Source macro syntax retained before later processing.
    MacroTree(MacroTree),

    /// Try expression whose handler has not yet been normalized to case clauses.
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
            metadata: Modifiers::default(),
        });

        assert_eq!(
            node,
            UntypedNode::ModuleDef(ModuleDef {
                name: TermName::new(NameId::new(1)),
                template: tree_id(2),
                metadata: Modifiers::default(),
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
    fn function_with_mods_preserves_modifiers_and_erased_parameters() {
        let node = UntypedNode::FunctionWithMods(FunctionWithMods {
            params: vec![tree_id(1), tree_id(2)],
            result: tree_id(3),
            modifiers: Modifiers {
                modifiers: vec![crate::ast::modifiers::Modifier::Given],
                ..Modifiers::default()
            },
            erased_params: vec![false, true],
        });

        let UntypedNode::FunctionWithMods(function) = node else {
            panic!("expected a FunctionWithMods node");
        };
        assert_eq!(function.params, vec![tree_id(1), tree_id(2)]);
        assert_eq!(function.result, tree_id(3));
        assert_eq!(
            function.modifiers.modifiers,
            vec![crate::ast::modifiers::Modifier::Given]
        );
        assert_eq!(function.erased_params, vec![false, true]);
    }

    #[test]
    fn captures_and_result_keeps_capture_refs_separate_from_the_result_type() {
        let node = UntypedNode::CapturesAndResult(CapturesAndResult {
            captures: vec![tree_id(1), tree_id(2)],
            result: tree_id(3),
        });

        let UntypedNode::CapturesAndResult(captures) = node else {
            panic!("expected a captures-and-result node");
        };
        assert_eq!(captures.captures, vec![tree_id(1), tree_id(2)]);
        assert_eq!(captures.result, tree_id(3));
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
            check_mode: GenCheckMode::Check,
        });
        let alias = UntypedNode::GenAlias(GenAlias {
            pattern: tree_id(1),
            expr: tree_id(2),
        });

        assert_ne!(from, alias);
    }

    fn gen_from_with_mode(check_mode: GenCheckMode) -> GenFrom {
        GenFrom {
            pattern: tree_id(1),
            expr: tree_id(2),
            check_mode,
        }
    }

    #[test]
    fn gen_from_preserves_ignore_mode() {
        assert_eq!(
            gen_from_with_mode(GenCheckMode::Ignore).check_mode,
            GenCheckMode::Ignore
        );
    }

    #[test]
    fn gen_from_preserves_filtered_mode() {
        assert_eq!(
            gen_from_with_mode(GenCheckMode::Filtered).check_mode,
            GenCheckMode::Filtered
        );
    }

    #[test]
    fn gen_from_preserves_check_mode() {
        assert_eq!(
            gen_from_with_mode(GenCheckMode::Check).check_mode,
            GenCheckMode::Check
        );
    }

    #[test]
    fn gen_from_preserves_check_and_filter_mode() {
        assert_eq!(
            gen_from_with_mode(GenCheckMode::CheckAndFilter).check_mode,
            GenCheckMode::CheckAndFilter
        );
    }

    #[test]
    fn gen_from_preserves_filter_now_mode() {
        assert_eq!(
            gen_from_with_mode(GenCheckMode::FilterNow).check_mode,
            GenCheckMode::FilterNow
        );
    }

    #[test]
    fn gen_from_preserves_filter_always_mode() {
        assert_eq!(
            gen_from_with_mode(GenCheckMode::FilterAlways).check_mode,
            GenCheckMode::FilterAlways
        );
    }

    #[test]
    fn pat_def_carries_its_modifiers_patterns_and_rhs() {
        let node = UntypedNode::PatDef(PatDef {
            modifiers: Modifiers::default(),
            patterns: vec![tree_id(1)],
            tpt: tree_id(2),
            rhs: Some(tree_id(3)),
        });

        assert_eq!(
            node,
            UntypedNode::PatDef(PatDef {
                modifiers: Modifiers::default(),
                patterns: vec![tree_id(1)],
                tpt: tree_id(2),
                rhs: Some(tree_id(3)),
            })
        );
    }

    #[test]
    fn pat_def_preserves_a_var_modifier() {
        let node = PatDef {
            modifiers: Modifiers {
                modifiers: vec![crate::ast::modifiers::Modifier::Var],
                ..Modifiers::default()
            },
            patterns: vec![tree_id(1)],
            tpt: tree_id(2),
            rhs: Some(tree_id(3)),
        };

        assert_eq!(
            node.modifiers.modifiers,
            vec![crate::ast::modifiers::Modifier::Var]
        );
    }

    #[test]
    fn pat_def_can_preserve_an_absent_rhs() {
        let node = PatDef {
            modifiers: Modifiers::default(),
            patterns: vec![tree_id(1)],
            tpt: tree_id(2),
            rhs: None,
        };

        assert_eq!(node.rhs, None);
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
    fn context_bound_type_tree_preserves_parameter_without_an_alias() {
        let node = UntypedNode::ContextBoundTypeTree(ContextBoundTypeTree {
            bound: tree_id(1),
            parameter: crate::names::TypeName::new(NameId::new(2)),
            name: None,
        });

        let UntypedNode::ContextBoundTypeTree(bound) = node else {
            panic!("expected a ContextBoundTypeTree node");
        };
        assert_eq!(bound.bound, tree_id(1));
        assert_eq!(bound.parameter, crate::names::TypeName::new(NameId::new(2)));
        assert_eq!(bound.name, None);
    }

    #[test]
    fn context_bound_type_tree_preserves_an_as_alias() {
        let node = ContextBoundTypeTree {
            bound: tree_id(1),
            parameter: crate::names::TypeName::new(NameId::new(2)),
            name: Some(TermName::new(NameId::new(3))),
        };

        assert_eq!(node.name, Some(TermName::new(NameId::new(3))));
    }

    #[test]
    fn number_literal_and_throw_carry_their_payload() {
        let number = UntypedNode::Number(NumberLiteral {
            text: NameId::new(1),
            kind: NumberKind::Whole(10),
        });
        let throw = UntypedNode::Throw(Throw { expr: tree_id(1) });

        assert_ne!(number, throw);
    }

    #[test]
    fn number_literal_preserves_a_non_decimal_whole_radix() {
        let number = NumberLiteral {
            text: NameId::new(1),
            kind: NumberKind::Whole(16),
        };

        assert_eq!(number.kind, NumberKind::Whole(16));
    }

    #[test]
    fn number_literal_preserves_decimal_classification() {
        let number = NumberLiteral {
            text: NameId::new(1),
            kind: NumberKind::Decimal,
        };

        assert_eq!(number.kind, NumberKind::Decimal);
    }

    #[test]
    fn number_literal_preserves_floating_classification() {
        let number = NumberLiteral {
            text: NameId::new(1),
            kind: NumberKind::Floating,
        };

        assert_eq!(number.kind, NumberKind::Floating);
    }
}
