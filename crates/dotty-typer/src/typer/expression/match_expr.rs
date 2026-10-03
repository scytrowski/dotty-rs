//! Source match-case typing helpers.

use super::super::{ExpressionContext, SourceTyper, TyperError};
use dotty_core::ast::{TreeKind, TypedAstBuilder};
use dotty_core::{SourceId, SymbolId, SymbolInfo, TreeId, TypeId, Typed, Untyped};

#[allow(dead_code)] // The Match dispatcher consumes this helper in the next increment.
impl SourceTyper<'_> {
    /// Types one unguarded wildcard case without opening a nested transaction.
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

        let pattern = self.type_pattern(
            case_def.pattern,
            selector_type,
            context,
            info_journal,
            new_mappings,
        )?;
        if case_def.guard.is_some() {
            return Err(TyperError::MatchGuardDeferred {
                source: self.source,
                tree_index: case_tree.index(),
            });
        }
        let body =
            self.type_expression_inner(case_def.body, context, info_journal, new_mappings)?;
        let body_type = self.typed_arena.get(body).ty;
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).case_def(
            pattern,
            None,
            body,
            body_type,
            source_tree.position,
        );
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typer::SourceTyper;
    use dotty_core::ast::{Tree, TreeKind};
    use dotty_core::{
        Definitions, Name, Namespace, Packages, SemanticStore, SourceSemanticIndex, SourceText,
        Type,
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
    fn guarded_case_is_deferred_and_rolls_back_its_pattern() {
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
        assert!(matches!(
            type_one_case(&mut typer, case_tree, definitions.int, context),
            Err(TyperError::MatchGuardDeferred { tree_index, .. }) if tree_index == case_tree.index()
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn non_wildcard_pattern_keeps_pattern_specific_error() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case value => 1 } }");
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
            Err(TyperError::UnsupportedPattern { .. })
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
}
