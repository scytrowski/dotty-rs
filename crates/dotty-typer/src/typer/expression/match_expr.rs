//! Source match-case typing helpers.

use super::super::{ExpressionContext, SourceTyper, TyperError};
use dotty_core::ast::{TreeKind, TypedAstBuilder};
use dotty_core::{SourceId, SymbolId, SymbolInfo, TreeId, TypeId, Typed, Untyped};

impl SourceTyper<'_> {
    /// Types one supported match case without opening a nested transaction.
    pub(super) fn type_case_def(
        &mut self,
        case_tree: TreeId<Untyped>,
        selector_type: TypeId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, case_tree) {
            return Ok(typed);
        }
        let Some(source_tree) = self.arena.try_get(case_tree).cloned() else {
            return Err(TyperError::MalformedCaseDef {
                source: self.source,
                tree_index: case_tree.index(),
                actual_kind: "tree outside arena",
            });
        };
        let TreeKind::CaseDef(case_def) = source_tree.kind else {
            return Err(TyperError::MalformedCaseDef {
                source: self.source,
                tree_index: case_tree.index(),
                actual_kind: "non-CaseDef tree",
            });
        };

        let expression_scope_depth = self.expression_scopes.len();
        let case_context = self.push_case_scope(context)?;
        let result =
            (|| {
                let pattern = self.type_pattern(
                    case_def.pattern,
                    selector_type,
                    case_context,
                    info_journal,
                    new_mappings,
                )?;
                let guard = case_def
                    .guard
                    .map(|guard| {
                        self.type_expression_expected_inner(
                            guard,
                            case_context,
                            self.definitions.boolean,
                            info_journal,
                            new_mappings,
                        )
                    })
                    .transpose()?;
                let body = self.type_value_expression_inner(
                    case_def.body,
                    case_context,
                    info_journal,
                    new_mappings,
                )?;
                let body_type = self.typed_arena.get(body).ty;
                let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                    .case_def(pattern, guard, body, body_type, source_tree.position);
                self.typed_index
                    .insert(self.source, case_tree, typed)
                    .map_err(|error| TyperError::ConflictingTypedExpression {
                        source: error.source,
                        tree_index: error.untyped.index(),
                        existing: error.existing.index(),
                        attempted: error.attempted.index(),
                    })?;
                new_mappings.push((self.source, case_tree));
                Ok(typed)
            })();
        self.expression_scopes.truncate(expression_scope_depth);
        result
    }

    pub(in crate::typer) fn type_match_expression(
        &mut self,
        tree: TreeId<Untyped>,
        matched: dotty_core::ast::Match<Untyped>,
        position: Option<dotty_core::SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if matched.cases.is_empty() {
            return Err(TyperError::EmptyMatchCases {
                source: self.source,
                tree_index: tree.index(),
            });
        }

        let selector = self.type_value_expression_inner(
            matched.selector,
            context,
            info_journal,
            new_mappings,
        )?;
        let selector_type = self.typed_arena.get(selector).ty;
        let pattern_type = self
            .pattern_selector_type(selector_type, info_journal)
            .map_err(|error| TyperError::MatchSelectorTypeCannotBeAdapted {
                source: self.source,
                tree_index: tree.index(),
                error: Box::new(error),
            })?;

        let mut typed_cases = Vec::with_capacity(matched.cases.len());
        let mut result_type = None;
        for case_tree in matched.cases {
            let typed_case =
                self.type_case_def(case_tree, pattern_type, context, info_journal, new_mappings)?;
            let case_type = self.typed_arena.get(typed_case).ty;
            let case_type = self
                .widen_expression_type_journaled(case_type, info_journal, 0)
                .map_err(|error| TyperError::MatchCaseResultTypeCannotBeWidened {
                    source: self.source,
                    tree_index: tree.index(),
                    case_tree_index: case_tree.index(),
                    error: Box::new(error),
                })?;
            result_type = Some(match result_type {
                None => case_type,
                Some(current) => {
                    self.join_expression_types(current, case_type)
                        .map_err(|error| TyperError::MatchCaseJoinUnsupported {
                            source: self.source,
                            tree_index: tree.index(),
                            left: current,
                            right: case_type,
                            error: Box::new(error),
                        })?
                }
            });
            typed_cases.push(typed_case);
        }

        let result_type = result_type.expect("non-empty Match has a case result");
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).match_expr(
                selector,
                typed_cases,
                result_type,
                position,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typer::SourceTyper;
    use dotty_core::ast::{Bind, Tree, TreeKind};
    use dotty_core::{
        Definitions, Name, Namespace, Packages, SemanticStore, SourceSemanticIndex, SourceText,
        TermRefTarget, Type,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;

    fn setup(
        text: &str,
    ) -> (
        dotty_parser::ParseResult,
        SemanticStore,
        Packages,
        Definitions,
        SourceSemanticIndex,
        SourceId,
    ) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let source = SourceId::from_index(19);
        let mut packages = Packages::new();
        let scanner = ContextualScanner::new(text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "MatchCases.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        (parsed, store, packages, definitions, index, source)
    }

    fn method_and_case(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
    ) -> (SymbolId, TreeId<Untyped>) {
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let case_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::CaseDef(_)).then_some(tree))
            .unwrap();
        (method, case_tree)
    }

    fn method_and_match(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
    ) -> (SymbolId, TreeId<Untyped>) {
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "choose" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let match_tree = parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| matches!(node.kind, TreeKind::Match(_)).then_some(tree))
            .max_by_key(|tree| tree.index())
            .unwrap();
        (method, match_tree)
    }

    fn context_for<'a>(
        parsed: &'a dotty_parser::ParseResult,
        store: &'a mut SemanticStore,
        packages: &'a Packages,
        definitions: Definitions,
        index: &'a SourceSemanticIndex,
        source: SourceId,
        method: SymbolId,
    ) -> (SourceTyper<'a>, ExpressionContext) {
        let mut typer = SourceTyper::new(&parsed.ast, source, index, store, definitions, packages);
        let context = typer.expression_context_for(method).unwrap();
        (typer, context)
    }

    fn type_one_case(
        typer: &mut SourceTyper<'_>,
        case_tree: TreeId<Untyped>,
        selector_type: TypeId,
        context: ExpressionContext,
    ) -> Result<TreeId<Typed>, TyperError> {
        typer.run_expression_transaction(|typer, journal, mappings| {
            typer.type_case_def(case_tree, selector_type, context, journal, mappings)
        })
    }

    fn synthetic_bind(
        parsed: &mut dotty_parser::ParseResult,
        name: Name,
        body: TreeId<Untyped>,
    ) -> TreeId<Untyped> {
        parsed.ast.alloc(Tree {
            kind: TreeKind::Bind(Bind {
                name,
                body,
                given: false,
            }),
            position: None,
            ty: (),
        })
    }

    #[test]
    fn wildcard_case_uses_selector_type_and_case_body_constant_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case =
            type_one_case(&mut typer, case_tree, definitions.object_type, context).unwrap();
        let case_node = typer.typed_arena.get(typed_case);
        let TreeKind::CaseDef(case_def) = case_node.kind else {
            panic!("typed case helper must produce CaseDef")
        };
        assert_eq!(
            typer.typed_arena.get(case_def.pattern).ty,
            definitions.object_type
        );
        assert!(matches!(
            typer
                .store
                .types
                .try_get(typer.typed_arena.get(case_def.body).ty),
            Some(Type::Constant(dotty_core::Constant::Int(1)))
        ));
        assert_eq!(case_node.ty, typer.typed_arena.get(case_def.body).ty);
        assert_eq!(typer.typed_index.get(source, case_tree), Some(typed_case));

        let repeated =
            type_one_case(&mut typer, case_tree, definitions.object_type, context).unwrap();
        assert_eq!(repeated, typed_case);
    }

    #[test]
    fn case_body_can_reference_outer_parameters_and_use_supported_expressions() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def id(v: Int): Int = v; def choose(x: Int): Int = x match { case _ => if true then id(1) else { x } } }",
        );
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = type_one_case(&mut typer, case_tree, definitions.int, context).unwrap();
        let TreeKind::CaseDef(case_def) = typer.typed_arena.get(typed_case).kind else {
            panic!("typed case helper must produce CaseDef")
        };
        let TreeKind::Block(block) = &typer.typed_arena.get(case_def.body).kind else {
            panic!("case body should retain the source block wrapper")
        };
        assert!(matches!(
            typer.typed_arena.get(block.expr).kind,
            TreeKind::If(_)
        ));
    }

    #[test]
    fn wildcard_guard_is_typed_as_boolean_and_retained_on_the_case() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ if true => 1 } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = type_one_case(&mut typer, case_tree, definitions.int, context).unwrap();
        let typed_node = typer.typed_arena.get(typed_case);
        let TreeKind::CaseDef(case) = &typed_node.kind else {
            panic!("expected typed CaseDef")
        };
        let guard = case.guard.expect("typed CaseDef should retain the guard");
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(guard).ty),
            Some(Type::Constant(dotty_core::Constant::Boolean(true)))
        ));
        assert_eq!(typed_node.ty, typer.typed_arena.get(case.body).ty);
        assert_eq!(typer.typed_arena.get(case.pattern).ty, definitions.int);
    }

    #[test]
    fn variable_pattern_binding_is_visible_in_guard_and_body() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def predicate(value: Int): Boolean = true; def choose(x: Int): Int = x match { case item if predicate(item) => item } }",
        );
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let TreeKind::CaseDef(source_case) = parsed.ast.get(case_tree).kind else {
            panic!("expected source CaseDef")
        };
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = type_one_case(&mut typer, case_tree, definitions.int, context).unwrap();
        let TreeKind::CaseDef(case) = typer.typed_arena.get(typed_case).kind.clone() else {
            panic!("expected typed CaseDef")
        };
        let symbol = typer
            .pattern_binding_symbol_at(source, source_case.pattern)
            .unwrap();
        let guard = case.guard.unwrap();
        let TreeKind::Apply(application) = typer.typed_arena.get(guard).kind.clone() else {
            panic!("guard should type as a method application")
        };
        let argument = application.args[0];
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(argument).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                if *actual == symbol
        ));
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(case.body).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                if *actual == symbol
        ));
    }

    #[test]
    fn explicit_bind_and_literal_patterns_can_have_guards() {
        for (case_pattern, selector, expected_pattern) in [
            ("item @ _", "Int", "bind"),
            ("1", "Int", "literal"),
            ("Stable", "Int", "stable"),
        ] {
            let source_text = format!(
                "class C {{ val Stable: Int = 1; def choose(x: {selector}): Int = x match {{ case {case_pattern} if true => 1 }} }}"
            );
            let (parsed, mut store, packages, definitions, index, source) = setup(&source_text);
            let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
            let (mut typer, context) = context_for(
                &parsed,
                &mut store,
                &packages,
                definitions,
                &index,
                source,
                method,
            );
            let typed_case =
                type_one_case(&mut typer, case_tree, definitions.int, context).unwrap();
            let TreeKind::CaseDef(case) = typer.typed_arena.get(typed_case).kind.clone() else {
                panic!("expected typed CaseDef for {expected_pattern}")
            };
            assert!(case.guard.is_some(), "{expected_pattern} guard is present");
        }
    }

    #[test]
    fn non_boolean_guard_fails_before_the_body_is_typed() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ if 1 => missing } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            type_one_case(&mut typer, case_tree, definitions.int, context),
            Err(TyperError::ExpectedExpressionTypeMismatch { expected, .. })
                if expected == definitions.boolean
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn unsupported_pattern_fails_before_its_guard_is_typed() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case (1, _) if missing => 1 } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            type_one_case(&mut typer, case_tree, definitions.int, context),
            Err(TyperError::TuplePatternResolutionDeferred { .. })
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn guard_cannot_resolve_a_binding_from_a_sibling_case() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def choose(x: Int): Int = x match { case first if true => 1; case other if first => other } }",
        );
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.type_expression(match_tree, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert!(typer.pattern_bindings.by_tree.is_empty());
        assert!(typer.typed_index.is_empty());
        assert_eq!(typer.typed_arena.iter().count(), 0);
    }

    #[test]
    fn body_failure_rolls_back_previously_typed_guard_and_pattern() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case item if true => missing } }");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.type_expression(match_tree, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert!(typer.pattern_bindings.by_tree.is_empty());
        assert!(typer.typed_index.is_empty());
        assert_eq!(typer.typed_arena.iter().count(), 0);
    }

    #[test]
    fn unresolved_stable_pattern_keeps_resolution_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case Value => 1 } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            type_one_case(&mut typer, case_tree, definitions.int, context),
            Err(TyperError::TermNameNotFound { .. })
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn malformed_case_ids_return_a_structural_error() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, _) = method_and_case(&parsed, &store, &index, source);
        let non_case_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| matches!(node.kind, TreeKind::DefDef(_)).then_some(tree))
            .unwrap();
        let missing = {
            let checkpoint = parsed.ast.checkpoint();
            let temporary = parsed.ast.alloc(Tree {
                kind: TreeKind::Ident(dotty_core::ast::Ident {
                    name: Name::new(store.names.intern("bad"), Namespace::Term),
                    backquoted: false,
                }),
                position: None,
                ty: (),
            });
            parsed.ast.rollback_to(checkpoint);
            temporary
        };
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            type_one_case(&mut typer, non_case_tree, definitions.int, context),
            Err(TyperError::MalformedCaseDef {
                actual_kind: "non-CaseDef tree",
                ..
            })
        ));
        assert!(matches!(
            type_one_case(&mut typer, missing, definitions.int, context),
            Err(TyperError::MalformedCaseDef {
                actual_kind: "tree outside arena",
                ..
            })
        ));
    }

    #[test]
    fn body_failure_rolls_back_case_pattern_body_and_mappings() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def choose(x: Int): Int = x match { case _ => { val local = 1; missing } } }",
        );
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let local_tree = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                matches!(&node.kind, TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "local")
                .then_some(tree)
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let before = typer.store.types.alloc(Type::NoType);
        assert!(type_one_case(&mut typer, case_tree, definitions.int, context).is_err());
        let after = typer.store.types.alloc(Type::NoType);
        assert_eq!(after.index(), before.index() + 1);
        assert_eq!(typer.local_symbol_at(source, local_tree), None);
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn wildcard_match_types_selector_once_and_joins_case_results() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Any = x match { case _ => 1; case _ => 2 } }");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let TreeKind::Match(source_match) = &parsed.ast.get(match_tree).kind else {
            panic!("expected a source Match")
        };
        let selector_tree = source_match.selector;
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );

        let typed = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(typed_match) = &typer.typed_arena.get(typed).kind else {
            panic!("expected a typed Match")
        };
        assert_eq!(typed_match.cases.len(), 2);
        assert_eq!(
            typer.typed_index.get(source, selector_tree),
            Some(typed_match.selector)
        );
        let selector_type = typer.typed_arena.get(typed_match.selector).ty;
        assert!(typer.store.types.contains(selector_type));
        let TreeKind::CaseDef(first) = &typer.typed_arena.get(typed_match.cases[0]).kind else {
            panic!("expected a typed CaseDef")
        };
        assert_eq!(typer.typed_arena.get(first.pattern).ty, definitions.int);
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.int);

        let repeated = typer.type_expression(match_tree, context).unwrap();
        assert_eq!(repeated, typed);
    }

    #[test]
    fn wildcard_match_joins_unrelated_results_with_or_and_nests() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def choose(x: Int, y: Int): Any = x match { case _ => y match { case _ => 1 }; case _ => false } }",
        );
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(_) = &typer.typed_arena.get(typed).kind else {
            panic!("expected a typed outer Match")
        };
        let typed_matches = typer
            .typed_arena
            .iter()
            .filter(|(_, node)| matches!(node.kind, TreeKind::Match(_)))
            .count();
        assert!(typed_matches >= 2, "expected nested typed matches");
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::Or { .. })
        ));
    }

    #[test]
    fn constant_selector_keeps_its_type_and_empty_match_is_rejected() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = 1 match { case 1 => 1 } }");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let TreeKind::Match(source_match) = parsed.ast.get(match_tree).kind.clone() else {
            panic!("expected source Match")
        };
        let empty_match = parsed.ast.alloc(Tree {
            kind: TreeKind::Match(dotty_core::ast::Match {
                selector: source_match.selector,
                cases: Vec::new(),
            }),
            position: None,
            ty: (),
        });
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(typed_match) = &typer.typed_arena.get(typed).kind else {
            panic!("expected typed Match")
        };
        let selector_type = typer.typed_arena.get(typed_match.selector).ty;
        assert!(matches!(
            typer.store.types.try_get(selector_type),
            Some(Type::Constant(dotty_core::Constant::Int(1)))
        ));
        let TreeKind::CaseDef(case_def) = &typer.typed_arena.get(typed_match.cases[0]).kind else {
            panic!("expected typed CaseDef")
        };
        assert!(matches!(
            typer
                .store
                .types
                .try_get(typer.typed_arena.get(case_def.pattern).ty),
            Some(Type::Constant(dotty_core::Constant::Int(1)))
        ));
        assert!(matches!(
            typer.type_expression(empty_match, context),
            Err(TyperError::EmptyMatchCases { tree_index, .. })
                if tree_index == empty_match.index()
        ));
    }

    #[test]
    fn different_literal_constant_is_rejected_for_constant_selector() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = 1 match { case 2 => 1 } }");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let error = typer.type_expression(match_tree, context).unwrap_err();
        assert!(matches!(error, TyperError::PatternTypeMismatch { .. }));
        assert!(typer.typed_index.is_empty());
        assert_eq!(typer.typed_arena.iter().count(), 0);
    }

    #[test]
    fn match_result_uses_the_supertype_when_case_results_are_related() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class Parent; class Child extends Parent; class C { def choose(child: Child, parent: Parent): Any = child match { case _ => child; case _ => parent } }",
        );
        let symbol_named = |wanted: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == wanted =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let parent = symbol_named("Parent");
        let child = symbol_named("Child");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        typer.complete_symbol(parent).unwrap();
        typer.complete_symbol(child).unwrap();
        let typed = typer.type_expression(match_tree, context).unwrap();
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::TypeRef { target, .. }) if target.symbol() == Some(parent)
        ));
    }

    #[test]
    fn nothing_case_joins_with_the_ordinary_case_result() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def choose(x: Int): Any = x match { case _ => return 1; case _ => false } }",
        );
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed = typer.type_expression(match_tree, context).unwrap();
        assert_eq!(typer.typed_arena.get(typed).ty, definitions.boolean);
    }

    #[test]
    fn a_later_match_case_failure_rolls_back_the_entire_match() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { def choose(x: Int): Any = x match { case _ => 1; case _ if 1 => 2 } }",
        );
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            typer.type_expression(match_tree, context),
            Err(TyperError::ExpectedExpressionTypeMismatch { .. })
        ));
        assert!(typer.typed_index.is_empty());
        assert_eq!(typer.typed_arena.iter().count(), 0);
    }

    #[test]
    fn pattern_bindings_are_case_local_canonical_symbols() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1; case _ => 2 } }");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let TreeKind::Match(matched) = parsed.ast.get(match_tree).kind.clone() else {
            panic!("expected source Match")
        };
        let TreeKind::CaseDef(first_case) = parsed.ast.get(matched.cases[0]).kind else {
            panic!("expected first source CaseDef")
        };
        let TreeKind::CaseDef(second_case) = parsed.ast.get(matched.cases[1]).kind else {
            panic!("expected second source CaseDef")
        };
        let item = Name::new(store.names.intern("item"), Namespace::Term);
        let x_name = Name::new(store.names.intern("x"), Namespace::Term);
        let wildcard_name = Name::new(store.names.intern("_"), Namespace::Term);
        let bind_first = synthetic_bind(&mut parsed, item, first_case.pattern);
        let bind_second = synthetic_bind(&mut parsed, item, second_case.pattern);
        let bind_duplicate = synthetic_bind(&mut parsed, item, first_case.pattern);
        let bind_shadow = synthetic_bind(&mut parsed, x_name, first_case.pattern);
        let bind_wildcard = synthetic_bind(&mut parsed, wildcard_name, first_case.pattern);
        let outer_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "x" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let original_scope_depth = typer.expression_scopes.len();
        typer.type_expression(match_tree, context).unwrap();
        assert_eq!(typer.expression_scopes.len(), original_scope_depth);

        let case_one = typer.push_case_scope(context).unwrap();
        let case_one_stack = case_one.local_scopes.unwrap();
        let case_one_scope = typer.expression_scopes[case_one_stack.index()].scope;
        assert_eq!(typer.store.scopes.get(case_one_scope).owner, Some(method));
        let (first_symbol, first_ref) = typer
            .enter_pattern_binding(bind_first, item, definitions.int, case_one)
            .unwrap();
        assert_eq!(
            typer
                .resolve_expression_term(item, case_one, bind_first.index(), None)
                .unwrap(),
            first_symbol
        );
        assert_eq!(
            typer
                .resolve_expression_term(x_name, case_one, bind_first.index(), None)
                .unwrap(),
            outer_parameter
        );
        assert_eq!(
            typer.pattern_binding_symbol_at(source, bind_first),
            Some(first_symbol)
        );
        assert_eq!(
            typer.pattern_binding_scope(first_symbol),
            Some(case_one_scope)
        );
        assert!(matches!(
            typer.store.types.try_get(first_ref),
            Some(Type::TermRef {
                prefix,
                target: TermRefTarget::Symbol(symbol),
            }) if *prefix == definitions.no_prefix && *symbol == first_symbol
        ));
        let declaration = typer.store.symbols.get(first_symbol);
        assert_eq!(declaration.kind, dotty_core::SymbolKind::Local);
        assert_eq!(declaration.owner, Some(method));
        assert_eq!(declaration.origin, dotty_core::SymbolOrigin::Source(source));
        assert_eq!(declaration.visibility, dotty_core::Visibility::Public);
        assert_eq!(declaration.flags, dotty_core::SymbolFlags::EMPTY);
        assert_eq!(declaration.info, SymbolInfo::Complete(definitions.int));
        assert!(matches!(
            typer.enter_pattern_binding(bind_duplicate, item, definitions.int, case_one),
            Err(TyperError::DuplicatePatternBinding { name, .. }) if name == item
        ));
        assert!(matches!(
            typer.enter_pattern_binding(bind_wildcard, wildcard_name, definitions.int, case_one),
            Err(TyperError::WildcardPatternBindingRejected { .. })
        ));
        let (shadow_symbol, _) = typer
            .enter_pattern_binding(bind_shadow, x_name, definitions.int, case_one)
            .unwrap();
        assert_ne!(shadow_symbol, outer_parameter);
        assert_eq!(
            typer
                .resolve_expression_term(x_name, case_one, bind_shadow.index(), None)
                .unwrap(),
            shadow_symbol
        );
        typer.expression_scopes.truncate(original_scope_depth);

        let case_two = typer.push_case_scope(context).unwrap();
        let case_two_stack = case_two.local_scopes.unwrap();
        let case_two_scope = typer.expression_scopes[case_two_stack.index()].scope;
        assert_ne!(case_one_scope, case_two_scope);
        assert!(matches!(
            typer.resolve_expression_term(item, case_two, bind_second.index(), None),
            Err(TyperError::TermNameNotFound { .. })
        ));
        let (second_symbol, _) = typer
            .enter_pattern_binding(bind_second, item, definitions.int, case_two)
            .unwrap();
        assert_ne!(first_symbol, second_symbol);
        assert_eq!(
            typer
                .resolve_expression_term(item, case_two, bind_second.index(), None)
                .unwrap(),
            second_symbol
        );
        assert_eq!(
            typer.pattern_binding_symbol_at(source, bind_second),
            Some(second_symbol)
        );
    }

    #[test]
    fn variable_patterns_lower_to_binds_and_shadow_outer_locals_per_case() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case x => x; case x => x } }");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let TreeKind::Match(source_match) = parsed.ast.get(match_tree).kind.clone() else {
            panic!("expected source Match")
        };
        let source_cases = source_match.cases;
        let source_patterns = source_cases
            .iter()
            .map(|case_tree| {
                let TreeKind::CaseDef(case_def) = parsed.ast.get(*case_tree).kind else {
                    panic!("expected source CaseDef")
                };
                (case_def.pattern, case_def.body)
            })
            .collect::<Vec<_>>();
        let outer_parameter = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "x" =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_match = typer.type_expression(match_tree, context).unwrap();
        let TreeKind::Match(typed_match) = typer.typed_arena.get(typed_match).kind.clone() else {
            panic!("expected typed Match")
        };
        let mut pattern_symbols = Vec::new();
        for ((source_pattern, source_body), typed_case) in
            source_patterns.into_iter().zip(typed_match.cases)
        {
            let TreeKind::CaseDef(typed_case) = typer.typed_arena.get(typed_case).kind.clone()
            else {
                panic!("expected typed CaseDef")
            };
            let TreeKind::Bind(typed_bind) = typer.typed_arena.get(typed_case.pattern).kind.clone()
            else {
                panic!("variable patterns lower to Typed Bind")
            };
            let symbol = typer
                .pattern_binding_symbol_at(source, source_pattern)
                .unwrap();
            pattern_symbols.push(symbol);
            assert_ne!(symbol, outer_parameter);
            assert_eq!(
                typer.store.symbols.get(symbol).info,
                SymbolInfo::Complete(definitions.int)
            );
            assert_eq!(
                typer.typed_index.get(source, source_pattern),
                Some(typed_case.pattern)
            );
            let repeated = typer
                .run_expression_transaction(|typer, journal, mappings| {
                    let typed = typer.type_pattern(
                        source_pattern,
                        definitions.int,
                        context,
                        journal,
                        mappings,
                    )?;
                    assert!(mappings.is_empty());
                    Ok(typed)
                })
                .unwrap();
            assert_eq!(repeated, typed_case.pattern);
            assert!(matches!(
                typer.store.types.try_get(typer.typed_arena.get(typed_case.pattern).ty),
                Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                    if *actual == symbol
            ));
            assert_eq!(typer.typed_arena.get(typed_bind.body).ty, definitions.int);
            assert_eq!(
                typer.typed_index.get(source, source_body),
                Some(typed_case.body)
            );
            assert!(matches!(
                typer.store.types.try_get(typer.typed_arena.get(typed_case.body).ty),
                Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                    if *actual == symbol
            ));
        }
        assert_ne!(pattern_symbols[0], pattern_symbols[1]);
    }

    #[test]
    fn explicit_wildcard_bind_shares_binding_semantics_and_source_mappings() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case item @ _ => item } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let TreeKind::CaseDef(source_case) = parsed.ast.get(case_tree).kind else {
            panic!("expected source CaseDef")
        };
        let TreeKind::Bind(source_bind) = parsed.ast.get(source_case.pattern).kind else {
            panic!("expected explicit source Bind")
        };
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = type_one_case(&mut typer, case_tree, definitions.int, context).unwrap();
        let TreeKind::CaseDef(typed_case) = typer.typed_arena.get(typed_case).kind.clone() else {
            panic!("expected typed CaseDef")
        };
        let TreeKind::Bind(typed_bind) = typer.typed_arena.get(typed_case.pattern).kind.clone()
        else {
            panic!("expected typed Bind")
        };
        assert_eq!(typed_bind.name, source_bind.name);
        let symbol = typer
            .pattern_binding_symbol_at(source, source_case.pattern)
            .unwrap();
        assert_eq!(
            typer.typed_index.get(source, source_case.pattern),
            Some(typed_case.pattern)
        );
        assert!(typer.typed_index.get(source, source_bind.body).is_some());
        assert_eq!(
            typer.store.symbols.get(symbol).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed_case.body).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                if *actual == symbol
        ));
    }

    #[test]
    fn explicit_bind_rejects_non_wildcard_bodies_with_a_focused_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case item @ other => item } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            type_one_case(&mut typer, case_tree, definitions.int, context),
            Err(TyperError::UnsupportedBindPatternBody { .. })
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn explicit_bind_over_literal_keeps_selector_type_and_literal_child_type() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case item @ 1 => item } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let TreeKind::CaseDef(source_case) = parsed.ast.get(case_tree).kind else {
            panic!("expected source CaseDef")
        };
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = type_one_case(&mut typer, case_tree, definitions.int, context).unwrap();
        let TreeKind::CaseDef(typed_case) = typer.typed_arena.get(typed_case).kind.clone() else {
            panic!("expected typed CaseDef")
        };
        let TreeKind::Bind(typed_bind) = typer.typed_arena.get(typed_case.pattern).kind.clone()
        else {
            panic!("expected typed Bind")
        };
        let symbol = typer
            .pattern_binding_symbol_at(source, source_case.pattern)
            .unwrap();
        assert_eq!(
            typer.store.symbols.get(symbol).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert!(matches!(
            typer
                .store
                .types
                .try_get(typer.typed_arena.get(typed_bind.body).ty),
            Some(Type::Constant(dotty_core::Constant::Int(1)))
        ));
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed_case.pattern).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                if *actual == symbol
        ));
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed_case.body).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                if *actual == symbol
        ));
    }

    #[test]
    fn explicit_bind_over_stable_value_uses_selector_type() {
        let (parsed, mut store, packages, definitions, index, source) = setup(
            "class C { val Stable: Int = 1; def choose(x: Int): Int = x match { case item @ Stable => item } }",
        );
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let TreeKind::CaseDef(source_case) = parsed.ast.get(case_tree).kind else {
            panic!("expected source CaseDef")
        };
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let typed_case = type_one_case(&mut typer, case_tree, definitions.int, context).unwrap();
        let TreeKind::CaseDef(typed_case) = typer.typed_arena.get(typed_case).kind.clone() else {
            panic!("expected typed CaseDef")
        };
        let TreeKind::Bind(typed_bind) = typer.typed_arena.get(typed_case.pattern).kind.clone()
        else {
            panic!("expected typed Bind")
        };
        let symbol = typer
            .pattern_binding_symbol_at(source, source_case.pattern)
            .unwrap();
        assert_eq!(
            typer.store.symbols.get(symbol).info,
            SymbolInfo::Complete(definitions.int)
        );
        assert!(matches!(
            typer.typed_arena.get(typed_bind.body).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed_case.body).ty),
            Some(Type::TermRef { target: TermRefTarget::Symbol(actual), .. })
                if *actual == symbol
        ));
    }

    #[test]
    fn failed_bind_pattern_compatibility_rolls_back_the_binder() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case item @ 1 => item } }");
        let (method, case_tree) = method_and_case(&parsed, &store, &index, source);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        assert!(matches!(
            type_one_case(&mut typer, case_tree, definitions.boolean, context),
            Err(TyperError::PatternTypeMismatch { .. })
        ));
        assert!(typer.pattern_bindings.by_tree.is_empty());
        assert!(typer.typed_index.is_empty());
        assert_eq!(typer.typed_arena.iter().count(), 0);
    }

    #[test]
    fn failed_pattern_binding_transaction_rolls_back_scope_symbol_index_and_type() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, match_tree) = method_and_match(&parsed, &store, &index, source);
        let TreeKind::Match(matched) = parsed.ast.get(match_tree).kind.clone() else {
            panic!("expected source Match")
        };
        let TreeKind::CaseDef(case_def) = parsed.ast.get(matched.cases[0]).kind else {
            panic!("expected source CaseDef")
        };
        let name = Name::new(store.names.intern("temporary"), Namespace::Term);
        let binding = synthetic_bind(&mut parsed, name, case_def.pattern);
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let original_scope_depth = typer.expression_scopes.len();
        let mut allocated = None;
        let result: Result<TreeId<Typed>, TyperError> =
            typer.run_expression_transaction(|typer, _, _| {
                let case_context = typer.push_case_scope(context)?;
                let (symbol, term_ref) =
                    typer.enter_pattern_binding(binding, name, definitions.int, case_context)?;
                let scope = typer.pattern_binding_scope(symbol).unwrap();
                allocated = Some((symbol, term_ref, scope));
                Err(TyperError::PatternTypeMismatch {
                    source,
                    tree_index: matched.cases[0].index(),
                    actual: definitions.int,
                    selector: definitions.boolean,
                })
            });
        assert!(matches!(
            result,
            Err(TyperError::PatternTypeMismatch { .. })
        ));
        let (symbol, term_ref, scope) = allocated.unwrap();
        assert!(!typer.store.symbols.contains(symbol));
        assert!(!typer.store.scopes.contains(scope));
        assert!(!typer.store.types.contains(term_ref));
        assert_eq!(typer.pattern_binding_symbol_at(source, binding), None);
        assert_eq!(typer.expression_scopes.len(), original_scope_depth);
    }
}
