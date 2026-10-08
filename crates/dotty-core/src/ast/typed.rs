//! Convenience aliases and a builder that cannot construct a typed tree
//! without a real `TypeId`.

use crate::ast::arena::AstArena;
use crate::ast::common::{
    Alternative, Apply, ApplyKind, Assign, Bind, Block, CaseDef, Closure, Ident, If, Literal,
    Match, New, Return, Select, This, TypeApply, TypeTree, TypedExpr, UnApply, While,
};
use crate::ast::phase::Typed;
use crate::ast::tree::{Tree, TreeKind};
use crate::ids::{TreeId, TypeId};
use crate::names::Name;
use crate::source::SourceSpan;
use crate::types::{Constant, Type, TypeArena};

pub type TypedTree = Tree<Typed>;
pub type TypedTreeId = TreeId<Typed>;
pub type TypedAst = AstArena<Typed>;

/// Builds `Tree<Typed>` nodes. Every constructor takes a `TypeId` argument,
/// so a typed tree cannot be built without one *by construction*, not just
/// by convention. CaseDef term-case types come from their bodies, while a
/// Match result type is computed by the Typer and merely stored here.
pub struct TypedAstBuilder<'a> {
    arena: &'a mut AstArena<Typed>,
    types: &'a TypeArena,
}

impl<'a> TypedAstBuilder<'a> {
    pub fn new(arena: &'a mut AstArena<Typed>, types: &'a TypeArena) -> Self {
        Self { arena, types }
    }

    fn assert_real_type(&self, ty: TypeId, description: &str) {
        debug_assert!(
            matches!(self.types.try_get(ty), Some(found) if !matches!(found, Type::NoType)),
            "{description} must not use a missing, reserved, or NoType type"
        );
    }

    fn assert_real_typed_tree(&self, id: TreeId<Typed>, description: &str) -> &TypedTree {
        let tree = self
            .arena
            .try_get(id)
            .unwrap_or_else(|| panic!("{description} must belong to the typed arena"));
        self.assert_real_type(tree.ty, description);
        tree
    }

    pub fn ident(&mut self, name: Name, ty: TypeId, position: Option<SourceSpan>) -> TreeId<Typed> {
        self.ident_with_backquoted(name, false, ty, position)
    }

    /// Allocates a typed identifier while retaining its source backquotes.
    pub fn ident_with_backquoted(
        &mut self,
        name: Name,
        backquoted: bool,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Ident(Ident { name, backquoted }),
            position,
            ty,
        })
    }

    /// Allocates a typed pattern binding whose type is the bound symbol's
    /// canonical term reference.
    pub fn bind(
        &mut self,
        name: Name,
        body: TreeId<Typed>,
        ty: TypeId,
        given: bool,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.assert_real_typed_tree(body, "a typed pattern binding body");
        self.assert_real_type(ty, "a typed pattern binding");
        debug_assert!(
            matches!(self.types.try_get(ty), Some(Type::TermRef { .. })),
            "a typed pattern binding must use its bound symbol's term reference"
        );
        self.arena.alloc(Tree {
            kind: TreeKind::Bind(Bind { name, body, given }),
            position,
            ty,
        })
    }

    /// Allocates a typed literal with its already-determined semantic type.
    pub fn literal(
        &mut self,
        value: Constant,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Literal(Literal { value }),
            position,
            ty,
        })
    }

    /// Allocates a typed pattern alternative with its already-computed type.
    /// The alternatives are kept in source order.
    pub fn alternative(
        &mut self,
        alternatives: Vec<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        assert!(
            !alternatives.is_empty(),
            "a typed alternative must have a branch"
        );
        for alternative in &alternatives {
            self.assert_real_typed_tree(*alternative, "each typed alternative branch");
        }
        self.assert_real_type(ty, "a typed alternative");
        self.arena.alloc(Tree {
            kind: TreeKind::Alternative(Alternative { alternatives }),
            position,
            ty,
        })
    }

    /// Allocates a typed extractor pattern with its already-determined
    /// selector prototype. This builder does not interpret the extractor
    /// result or type nested patterns.
    pub fn unapply(
        &mut self,
        function: TreeId<Typed>,
        implicits: Vec<TreeId<Typed>>,
        patterns: Vec<TreeId<Typed>>,
        prototype: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.assert_real_typed_tree(function, "a typed extractor function");
        for implicit in &implicits {
            self.assert_real_typed_tree(*implicit, "each typed extractor implicit argument");
        }
        for pattern in &patterns {
            self.assert_real_typed_tree(*pattern, "each typed extractor pattern");
        }
        self.assert_real_type(prototype, "a typed extractor pattern");
        self.arena.alloc(Tree {
            kind: TreeKind::UnApply(UnApply {
                function,
                implicits,
                patterns,
            }),
            position,
            ty: prototype,
        })
    }

    /// Allocates a typed member selection referencing a typed qualifier.
    pub fn select(
        &mut self,
        qualifier: TreeId<Typed>,
        name: Name,
        backquoted: bool,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier,
                name,
                backquoted,
            }),
            position,
            ty,
        })
    }

    /// Allocates a typed `this` reference with an optional source qualifier.
    pub fn this(
        &mut self,
        qual: Option<Name>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::This(This { qual }),
            position,
            ty,
        })
    }

    pub fn apply(
        &mut self,
        function: TreeId<Typed>,
        args: Vec<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.apply_with_kind(function, args, ApplyKind::Regular, ty, position)
    }

    /// Allocates a typed application while preserving its source application kind.
    pub fn apply_with_kind(
        &mut self,
        function: TreeId<Typed>,
        args: Vec<TreeId<Typed>>,
        kind: ApplyKind,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Apply(Apply {
                function,
                args,
                kind,
            }),
            position,
            ty,
        })
    }

    /// Allocates a typed explicit type application with its instantiated
    /// semantic result type.
    pub fn type_apply(
        &mut self,
        function: TreeId<Typed>,
        args: Vec<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::TypeApply(TypeApply { function, args }),
            position,
            ty,
        })
    }

    /// Allocates a typed `new` expression with its semantic instance type.
    pub fn new_expr(
        &mut self,
        tpt: TreeId<Typed>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        debug_assert!(
            matches!(self.arena.get(tpt).kind, TreeKind::TypeTree(_)),
            "a typed new expression must reference a typed type tree"
        );
        self.arena.alloc(Tree {
            kind: TreeKind::New(New { tpt }),
            position,
            ty,
        })
    }

    /// Allocates a typed closure over its environment and method reference.
    pub fn closure(
        &mut self,
        env: Vec<TreeId<Typed>>,
        method: TreeId<Typed>,
        tpt: Option<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        for captured in &env {
            self.assert_real_typed_tree(*captured, "each typed closure environment value");
        }
        self.assert_real_typed_tree(method, "a typed closure method reference");
        if let Some(tpt) = tpt {
            self.assert_real_typed_tree(tpt, "a typed closure target type");
        }
        self.assert_real_type(ty, "a typed closure");
        self.arena.alloc(Tree {
            kind: TreeKind::Closure(Closure { env, method, tpt }),
            position,
            ty,
        })
    }

    /// Allocates a typed block whose type is the final expression's own type.
    pub fn block(
        &mut self,
        stats: Vec<TreeId<Typed>>,
        expr: TreeId<Typed>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        debug_assert_eq!(
            self.arena.get(expr).ty,
            ty,
            "a typed block must carry its final expression's own type"
        );
        self.arena.alloc(Tree {
            kind: TreeKind::Block(Block { stats, expr }),
            position,
            ty,
        })
    }

    /// Allocates a typed ascription. The child expression keeps its own type;
    /// the enclosing node carries the ascribed type.
    pub fn typed_expr(
        &mut self,
        expr: TreeId<Typed>,
        tpt: TreeId<Typed>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        debug_assert!(
            matches!(self.arena.get(tpt).kind, TreeKind::TypeTree(_)),
            "a typed ascription must reference a typed type tree"
        );
        self.arena.alloc(Tree {
            kind: TreeKind::Typed(TypedExpr { expr, tpt }),
            position,
            ty,
        })
    }

    /// Allocates a typed assignment whose result has the canonical `Unit` type.
    pub fn assign(
        &mut self,
        lhs: TreeId<Typed>,
        rhs: TreeId<Typed>,
        unit: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Assign(Assign { lhs, rhs }),
            position,
            ty: unit,
        })
    }

    /// Allocates a typed conditional with the caller-computed branch join type.
    pub fn if_expr(
        &mut self,
        cond: TreeId<Typed>,
        then_branch: TreeId<Typed>,
        else_branch: TreeId<Typed>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::If(If {
                cond,
                then_branch,
                else_branch,
            }),
            position,
            ty,
        })
    }

    /// Allocates a typed term case. For ordinary term matches, Scala 3.9's
    /// `TypeAssigner.assignType(CaseDef, ...)` gives the case the type of its
    /// typed body; type-match cases are outside this builder's contract.
    pub fn case_def(
        &mut self,
        pattern: TreeId<Typed>,
        guard: Option<TreeId<Typed>>,
        body: TreeId<Typed>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.assert_real_typed_tree(pattern, "a typed case pattern");
        if let Some(guard) = guard {
            self.assert_real_typed_tree(guard, "a typed case guard");
        }
        let body_tree = self.assert_real_typed_tree(body, "a typed case body");
        self.assert_real_type(ty, "a typed case");
        debug_assert_eq!(
            ty, body_tree.ty,
            "a typed term case must carry its body's type"
        );

        self.arena.alloc(Tree {
            kind: TreeKind::CaseDef(CaseDef {
                pattern,
                guard,
                body,
            }),
            position,
            ty,
        })
    }

    /// Allocates a typed match with the result type already computed by the
    /// Typer. This builder stores that type and does not perform a join.
    pub fn match_expr(
        &mut self,
        selector: TreeId<Typed>,
        cases: Vec<TreeId<Typed>>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.assert_real_typed_tree(selector, "a typed match selector");
        self.assert_real_type(ty, "a typed match");
        for case in &cases {
            let case_tree = self.assert_real_typed_tree(*case, "each typed match case");
            let TreeKind::CaseDef(case_def) = &case_tree.kind else {
                panic!("each typed match case must be a CaseDef")
            };
            self.assert_real_typed_tree(case_def.pattern, "each typed case pattern");
            if let Some(guard) = case_def.guard {
                self.assert_real_typed_tree(guard, "each typed case guard");
            }
            self.assert_real_typed_tree(case_def.body, "each typed case body");
        }

        self.arena.alloc(Tree {
            kind: TreeKind::Match(Match { selector, cases }),
            position,
            ty,
        })
    }

    /// Allocates a typed loop with its already-determined result type.
    pub fn while_expr(
        &mut self,
        cond: TreeId<Typed>,
        body: TreeId<Typed>,
        ty: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::While(While { cond, body }),
            position,
            ty,
        })
    }

    /// Allocates a typed return with the caller-supplied canonical `Nothing` type.
    pub fn return_expr(
        &mut self,
        expr: Option<TreeId<Typed>>,
        from: Option<TreeId<Typed>>,
        nothing: TypeId,
        position: Option<SourceSpan>,
    ) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::Return(Return { expr, from }),
            position,
            ty: nothing,
        })
    }

    /// Reifies a projected source type as a typed type tree. Type trees carry
    /// their meaning in `Tree::ty`; the source typer records the source mapping.
    pub fn type_tree(&mut self, ty: TypeId, position: Option<SourceSpan>) -> TreeId<Typed> {
        self.arena.alloc(Tree {
            kind: TreeKind::TypeTree(TypeTree),
            position,
            ty,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{NameId, SourceId};
    use crate::names::Namespace;
    use crate::source::{Span, TextRange};

    fn position(start: u32, end: u32) -> SourceSpan {
        SourceSpan::new(
            SourceId::new(1),
            Span::without_point(TextRange::new(start, end).expect("valid range")),
        )
    }

    #[test]
    fn ident_produces_a_tree_with_the_given_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);

        let id = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );

        assert_eq!(arena.get(id).ty, TypeId::new(2));
    }

    #[test]
    fn apply_references_its_function_and_args_with_the_given_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);

        let function = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let arg = builder.ident(
            Name::new(NameId::new(3), Namespace::Term),
            TypeId::new(4),
            None,
        );
        let call = builder.apply(function, vec![arg], TypeId::new(5), None);

        assert_eq!(arena.get(call).ty, TypeId::new(5));
        let TreeKind::Apply(apply) = &arena.get(call).kind else {
            panic!("expected an Apply node");
        };
        assert_eq!(apply.function, function);
        assert_eq!(apply.args, vec![arg]);
        assert_eq!(apply.kind, ApplyKind::Regular);
    }

    #[test]
    fn unapply_uses_the_caller_prototype_and_preserves_all_children() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let prototype = types.alloc(Type::NoPrefix);
        let function_type = types.alloc(Type::NoPrefix);
        let pattern_type = types.alloc(Type::NoPrefix);
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let function = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            function_type,
            None,
        );
        let implicit = builder.ident(
            Name::new(NameId::new(2), Namespace::Term),
            function_type,
            None,
        );
        let pattern = builder.ident(
            Name::new(NameId::new(3), Namespace::Term),
            pattern_type,
            None,
        );
        let position = position(2, 10);

        let unapply = builder.unapply(
            function,
            vec![implicit],
            vec![pattern],
            prototype,
            Some(position),
        );

        assert_eq!(arena.get(unapply).ty, prototype);
        assert_eq!(arena.get(unapply).position, Some(position));
        let TreeKind::UnApply(unapply) = &arena.get(unapply).kind else {
            panic!("expected an UnApply node");
        };
        assert_eq!(unapply.function, function);
        assert_eq!(unapply.implicits, vec![implicit]);
        assert_eq!(unapply.patterns, vec![pattern]);
    }

    #[test]
    fn apply_with_kind_preserves_using_applications() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let function = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );

        let call =
            builder.apply_with_kind(function, Vec::new(), ApplyKind::Using, TypeId::new(3), None);

        let TreeKind::Apply(apply) = &arena.get(call).kind else {
            panic!("expected an Apply node");
        };
        assert_eq!(apply.kind, ApplyKind::Using);
    }

    #[test]
    fn block_references_typed_stats_and_uses_the_final_expression_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let stat = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let expr = builder.ident(
            Name::new(NameId::new(3), Namespace::Term),
            TypeId::new(4),
            None,
        );

        let block = builder.block(vec![stat], expr, TypeId::new(4), None);

        assert_eq!(arena.get(block).ty, arena.get(expr).ty);
        let TreeKind::Block(block) = &arena.get(block).kind else {
            panic!("expected a Block node");
        };
        assert_eq!(block.stats, vec![stat]);
        assert_eq!(block.expr, expr);
    }

    #[test]
    fn typed_expr_references_its_child_and_ascribed_type_tree() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let child_type = TypeId::new(2);
        let ascribed_type = TypeId::new(3);
        let expr = builder.literal(Constant::Int(1), child_type, None);
        let tpt = builder.type_tree(ascribed_type, None);

        let ascription = builder.typed_expr(expr, tpt, ascribed_type, None);

        assert_eq!(arena.get(expr).ty, child_type);
        assert_eq!(arena.get(ascription).ty, ascribed_type);
        let TreeKind::Typed(typed_expr) = arena.get(ascription).kind else {
            panic!("expected a typed expression ascription");
        };
        assert_eq!(typed_expr.expr, expr);
        assert_eq!(typed_expr.tpt, tpt);
    }

    #[test]
    fn assign_references_its_operands_and_uses_unit_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let lhs = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let rhs = builder.literal(Constant::Int(1), TypeId::new(3), None);
        let unit = TypeId::new(4);

        let assign = builder.assign(lhs, rhs, unit, None);

        assert_eq!(arena.get(assign).ty, unit);
        let TreeKind::Assign(assign) = arena.get(assign).kind else {
            panic!("expected an assignment");
        };
        assert_eq!(assign.lhs, lhs);
        assert_eq!(assign.rhs, rhs);
    }

    #[test]
    fn if_expr_references_all_three_children_and_uses_join_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let condition = builder.literal(Constant::Boolean(true), TypeId::new(2), None);
        let then_branch = builder.literal(Constant::Int(1), TypeId::new(3), None);
        let else_branch = builder.literal(Constant::Int(2), TypeId::new(4), None);
        let joined_type = TypeId::new(5);

        let conditional = builder.if_expr(condition, then_branch, else_branch, joined_type, None);

        assert_eq!(arena.get(conditional).ty, joined_type);
        let TreeKind::If(if_expr) = arena.get(conditional).kind else {
            panic!("expected a typed if expression");
        };
        assert_eq!(if_expr.cond, condition);
        assert_eq!(if_expr.then_branch, then_branch);
        assert_eq!(if_expr.else_branch, else_branch);
    }

    #[test]
    fn case_def_keeps_pattern_guard_body_position_and_body_type() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let body_type = types.alloc(Type::Constant(Constant::Int(0)));
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let pattern = builder.ident(Name::new(NameId::new(1), Namespace::Term), body_type, None);
        let body = builder.literal(Constant::Int(1), body_type, None);
        let source_position = position(10, 20);

        let case: TypedTreeId =
            builder.case_def(pattern, None, body, body_type, Some(source_position));

        let case_tree = arena.get(case);
        assert_eq!(case_tree.ty, body_type);
        assert_eq!(case_tree.position, Some(source_position));
        let TreeKind::CaseDef(case_def) = &case_tree.kind else {
            panic!("expected a typed CaseDef");
        };
        assert_eq!(case_def.pattern, pattern);
        assert_eq!(case_def.guard, None);
        assert_eq!(case_def.body, body);
        assert_eq!(arena.get(case_def.body).ty, body_type);
    }

    #[test]
    fn bind_keeps_its_body_name_and_exact_bound_symbol_reference() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let binding_type = types.alloc(Type::Constant(Constant::Int(1)));
        let no_prefix = types.alloc(Type::NoPrefix);
        let term_ref = types.alloc(Type::TermRef {
            prefix: no_prefix,
            target: crate::types::TermRefTarget::Symbol(crate::SymbolId::new(7)),
        });
        let name = Name::new(NameId::new(1), Namespace::Term);
        let position = position(2, 8);
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let body = builder.literal(Constant::Int(1), binding_type, Some(position));

        let bind = builder.bind(name, body, term_ref, false, Some(position));
        let given_bind = builder.bind(name, body, term_ref, true, Some(position));

        let node = arena.get(bind);
        assert_eq!(node.ty, term_ref);
        assert_eq!(node.position, Some(position));
        let TreeKind::Bind(bind) = &node.kind else {
            panic!("expected a typed Bind")
        };
        assert_eq!(bind.name, name);
        assert_eq!(bind.body, body);
        assert!(!bind.given);
        assert!(matches!(
            types.try_get(node.ty),
            Some(Type::TermRef {
                target: crate::types::TermRefTarget::Symbol(symbol),
                ..
            }) if *symbol == crate::SymbolId::new(7)
        ));
        assert!(matches!(
            &arena.get(given_bind).kind,
            TreeKind::Bind(bind) if bind.given
        ));
    }

    #[test]
    fn case_def_retains_a_present_guard() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let node_type = types.alloc(Type::Constant(Constant::Int(0)));
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let pattern = builder.ident(Name::new(NameId::new(1), Namespace::Term), node_type, None);
        let guard = builder.literal(Constant::Boolean(true), node_type, None);
        let body_type = node_type;
        let body = builder.literal(Constant::Int(1), body_type, None);

        let case = builder.case_def(pattern, Some(guard), body, body_type, None);

        let TreeKind::CaseDef(case_def) = &arena.get(case).kind else {
            panic!("expected a typed CaseDef");
        };
        assert_eq!(case_def.guard, Some(guard));
    }

    #[test]
    #[should_panic(expected = "a typed case body must not use a missing, reserved, or NoType type")]
    fn case_def_rejects_a_no_type_body() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let real_type = types.alloc(Type::Constant(Constant::Int(0)));
        let no_type = types.alloc(Type::NoType);
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let pattern = builder.ident(Name::new(NameId::new(1), Namespace::Term), real_type, None);
        let body = builder.literal(Constant::Int(1), no_type, None);

        builder.case_def(pattern, None, body, no_type, None);
    }

    #[test]
    fn match_expr_keeps_selector_ordered_cases_and_caller_result_type() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let selector_type = types.alloc(Type::Constant(Constant::Int(0)));
        let first_body_type = types.alloc(Type::Constant(Constant::Int(1)));
        let second_body_type = types.alloc(Type::Constant(Constant::Int(2)));
        let result_type = types.alloc(Type::Constant(Constant::Int(3)));
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let selector = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            selector_type,
            None,
        );
        let first_pattern = builder.ident(
            Name::new(NameId::new(3), Namespace::Term),
            selector_type,
            None,
        );
        let first_body = builder.literal(Constant::Int(1), first_body_type, None);
        let first_case = builder.case_def(first_pattern, None, first_body, first_body_type, None);
        let second_pattern = builder.ident(
            Name::new(NameId::new(6), Namespace::Term),
            selector_type,
            None,
        );
        let second_body = builder.literal(Constant::Int(2), second_body_type, None);
        let second_case =
            builder.case_def(second_pattern, None, second_body, second_body_type, None);
        let source_position = position(0, 30);

        let matched: TypedTreeId = builder.match_expr(
            selector,
            vec![first_case, second_case],
            result_type,
            Some(source_position),
        );

        let match_tree = arena.get(matched);
        assert_eq!(match_tree.ty, result_type);
        assert_eq!(match_tree.position, Some(source_position));
        let TreeKind::Match(match_expr) = &match_tree.kind else {
            panic!("expected a typed Match");
        };
        assert_eq!(match_expr.selector, selector);
        assert_eq!(match_expr.cases, vec![first_case, second_case]);
    }

    #[test]
    #[should_panic(expected = "each typed match case must be a CaseDef")]
    fn match_expr_rejects_non_case_trees() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let ty = types.alloc(Type::Constant(Constant::Int(0)));
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let selector = builder.ident(Name::new(NameId::new(1), Namespace::Term), ty, None);
        let not_a_case = builder.ident(Name::new(NameId::new(2), Namespace::Term), ty, None);

        builder.match_expr(selector, vec![not_a_case], ty, None);
    }

    #[test]
    #[should_panic(expected = "each typed case body must belong to the typed arena")]
    fn match_expr_rejects_a_case_with_a_body_outside_the_arena() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let ty = types.alloc(Type::Constant(Constant::Int(0)));
        let selector = {
            let mut builder = TypedAstBuilder::new(&mut arena, &types);
            builder.ident(Name::new(NameId::new(1), Namespace::Term), ty, None)
        };
        let case = arena.alloc(Tree {
            kind: TreeKind::CaseDef(CaseDef {
                pattern: selector,
                guard: None,
                body: TreeId::new(99),
            }),
            position: None,
            ty,
        });

        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        builder.match_expr(selector, vec![case], ty, None);
    }

    #[test]
    #[should_panic(expected = "a typed match must not use a missing, reserved, or NoType type")]
    fn match_expr_rejects_a_no_type_result() {
        let mut arena = TypedAst::new();
        let mut types = TypeArena::new();
        let ty = types.alloc(Type::Constant(Constant::Int(0)));
        let no_type = types.alloc(Type::NoType);
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let selector = builder.ident(Name::new(NameId::new(1), Namespace::Term), ty, None);

        builder.match_expr(selector, Vec::new(), no_type, None);
    }

    #[test]
    fn while_expr_references_condition_and_body_and_uses_result_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let condition = builder.literal(Constant::Boolean(true), TypeId::new(2), None);
        let body = builder.literal(Constant::Int(1), TypeId::new(3), None);
        let unit = TypeId::new(4);

        let loop_tree = builder.while_expr(condition, body, unit, None);

        assert_eq!(arena.get(loop_tree).ty, unit);
        let TreeKind::While(while_expr) = arena.get(loop_tree).kind else {
            panic!("expected a typed while expression");
        };
        assert_eq!(while_expr.cond, condition);
        assert_eq!(while_expr.body, body);
    }

    #[test]
    fn return_expr_preserves_optional_expression_and_target_and_uses_nothing() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let expr = builder.literal(Constant::Int(1), TypeId::new(2), None);
        let from = builder.ident(
            Name::new(NameId::new(3), Namespace::Term),
            TypeId::new(4),
            None,
        );
        let nothing = TypeId::new(5);

        let returned = builder.return_expr(Some(expr), Some(from), nothing, None);

        assert_eq!(arena.get(returned).ty, nothing);
        let TreeKind::Return(return_tree) = arena.get(returned).kind else {
            panic!("expected a typed return expression");
        };
        assert_eq!(return_tree.expr, Some(expr));
        assert_eq!(return_tree.from, Some(from));
    }

    #[test]
    fn literal_keeps_the_constant_and_explicit_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let ty = TypeId::new(2);

        let literal = builder.literal(Constant::Int(42), ty, None);

        assert_eq!(
            arena.get(literal).kind,
            TreeKind::Literal(Literal {
                value: Constant::Int(42)
            })
        );
        assert_eq!(arena.get(literal).ty, ty);
    }

    #[test]
    fn select_keeps_the_typed_qualifier_and_explicit_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let qualifier = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let selected_name = Name::new(NameId::new(3), Namespace::Term);
        let selected_type = TypeId::new(4);

        let select = builder.select(qualifier, selected_name, true, selected_type, None);

        assert_eq!(
            arena.get(select).kind,
            TreeKind::Select(Select {
                qualifier,
                name: selected_name,
                backquoted: true,
            })
        );
        assert_eq!(arena.get(select).ty, selected_type);
    }

    #[test]
    fn this_keeps_its_optional_qualifier_and_explicit_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let qualifier = Name::new(NameId::new(3), Namespace::Type);
        let ty = TypeId::new(4);

        let this = builder.this(Some(qualifier), ty, None);

        assert_eq!(
            arena.get(this).kind,
            TreeKind::This(This {
                qual: Some(qualifier)
            })
        );
        assert_eq!(arena.get(this).ty, ty);
    }

    #[test]
    fn type_apply_preserves_function_arguments_and_instantiated_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let function = builder.ident(
            Name::new(NameId::new(1), Namespace::Term),
            TypeId::new(2),
            None,
        );
        let argument = builder.type_tree(TypeId::new(3), None);
        let application = builder.type_apply(function, vec![argument], TypeId::new(4), None);

        assert_eq!(arena.get(application).ty, TypeId::new(4));
        let TreeKind::TypeApply(type_apply) = &arena.get(application).kind else {
            panic!("expected TypeApply")
        };
        assert_eq!(type_apply.function, function);
        assert_eq!(type_apply.args, vec![argument]);
    }

    #[test]
    fn type_tree_carries_the_projected_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let ty = TypeId::new(7);

        let tree = builder.type_tree(ty, None);

        assert_eq!(arena.get(tree).ty, ty);
        assert!(matches!(arena.get(tree).kind, TreeKind::TypeTree(TypeTree)));
    }

    #[test]
    fn new_references_a_typed_type_tree_and_keeps_the_instance_type() {
        let mut arena = TypedAst::new();
        let types = TypeArena::new();
        let mut builder = TypedAstBuilder::new(&mut arena, &types);
        let instance_type = TypeId::new(8);
        let tpt = builder.type_tree(instance_type, None);

        let tree = builder.new_expr(tpt, instance_type, None);

        assert_eq!(arena.get(tree).ty, instance_type);
        let TreeKind::New(new) = arena.get(tree).kind else {
            panic!("expected New")
        };
        assert_eq!(new.tpt, tpt);
    }
}
