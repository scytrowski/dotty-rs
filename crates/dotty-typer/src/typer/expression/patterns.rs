//! Pattern typing entry points and pattern-specific type adaptation.

use super::{ExpressionContext, SourceTyper, TyperError};
use crate::typer::PatternKind;
use dotty_core::ast::{TreeKind, TypedAstBuilder, UntypedNode};
use dotty_core::types::Type;
use dotty_core::{SourceId, SymbolId, SymbolInfo, TreeId, TypeId, Typed, Untyped};

/// Classifies a pattern by its stable source-tree root category.
pub(super) fn pattern_kind(kind: &TreeKind<Untyped>) -> PatternKind {
    match kind {
        TreeKind::Ident(_) => PatternKind::Identifier,
        TreeKind::Select(_) => PatternKind::StableSelection,
        TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
            PatternKind::Literal
        }
        TreeKind::Typed(_) => PatternKind::Typed,
        TreeKind::Alternative(_) => PatternKind::Alternative,
        TreeKind::Apply(_) => PatternKind::Application,
        TreeKind::TypeApply(_) => PatternKind::TypeApplication,
        TreeKind::Bind(_) => PatternKind::Binding,
        TreeKind::UnApply(_) => PatternKind::Extractor,
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => PatternKind::Tuple,
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => PatternKind::Infix,
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => PatternKind::Parenthesized,
        _ => PatternKind::Other,
    }
}

#[allow(dead_code)] // The Match typer consumes this entry point in the next increment.
impl SourceTyper<'_> {
    /// Types the initial supported pattern subset: an unquoted term wildcard.
    ///
    /// The context is accepted here to keep the recursive pattern API aligned
    /// with expression typing; a wildcard neither resolves a name nor binds a
    /// symbol, so this first implementation does not inspect it.
    pub(super) fn type_pattern(
        &mut self,
        pattern: TreeId<Untyped>,
        selector_type: TypeId,
        _context: ExpressionContext,
        _info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, pattern) {
            return Ok(typed);
        }
        let Some(source_tree) = self.arena.try_get(pattern).cloned() else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: pattern.index(),
            });
        };
        let is_wildcard = match source_tree.kind {
            TreeKind::Ident(ident) => {
                !ident.backquoted
                    && ident.name.is_term()
                    && self.store.names.resolve(ident.name.text()) == "_"
            }
            _ => false,
        };
        if !is_wildcard {
            return Err(TyperError::UnsupportedPattern {
                source: self.source,
                tree_index: pattern.index(),
                pattern_kind: pattern_kind(&source_tree.kind),
            });
        }

        let TreeKind::Ident(ident) = source_tree.kind else {
            unreachable!("wildcard classification requires an identifier")
        };
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .ident_with_backquoted(ident.name, false, selector_type, source_tree.position);
        self.typed_index
            .insert(self.source, pattern, typed)
            .map_err(|conflict| TyperError::ConflictingTypedExpression {
                source: conflict.source,
                tree_index: conflict.untyped.index(),
                existing: conflict.existing.index(),
                attempted: conflict.attempted.index(),
            })?;
        new_mappings.push((self.source, pattern));
        Ok(typed)
    }

    /// Computes the selector prototype used by patterns, preserving literal
    /// singleton types and widening other expression types as usual.
    pub(super) fn pattern_selector_type(
        &mut self,
        selector_type: TypeId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        match self.store.types.try_get(selector_type) {
            Some(Type::Constant(_)) => Ok(selector_type),
            _ => self.widen_expression_type_journaled(selector_type, info_journal, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typer::{ExpressionContext, SourceTyper};
    use dotty_core::ast::{Ident, Tree};
    use dotty_core::{
        Definitions, Name, Namespace, Packages, SemanticStore, SourceSemanticIndex, SourceText,
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    use dotty_lexer::ContextualScanner;
    use dotty_namer::name_compilation_unit;

    fn setup(
        source_text: &str,
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
        let source = SourceId::from_index(7);
        let mut packages = Packages::new();
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Patterns.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        (parsed, store, packages, definitions, index, source)
    }

    fn method_and_pattern(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
        method_name: &str,
    ) -> (dotty_core::SymbolId, TreeId<Untyped>) {
        let mut method = None;
        let mut pattern = None;
        for (tree, node) in parsed.ast.iter() {
            match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == method_name =>
                {
                    method = Some(index.symbol_at(source, tree).unwrap());
                }
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "_" => {
                    pattern = Some(tree);
                }
                _ => {}
            }
        }
        (method.unwrap(), pattern.unwrap())
    }

    fn context_for<'a>(
        parsed: &'a dotty_parser::ParseResult,
        store: &'a mut SemanticStore,
        packages: &'a Packages,
        definitions: Definitions,
        index: &'a SourceSemanticIndex,
        source: SourceId,
        method: dotty_core::SymbolId,
    ) -> (SourceTyper<'a>, ExpressionContext) {
        let mut typer = SourceTyper::new(&parsed.ast, source, index, store, definitions, packages);
        let context = typer.expression_context_for(method).unwrap();
        (typer, context)
    }

    #[test]
    fn wildcard_uses_selector_type_and_repeated_typing_keeps_identity() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, pattern) = method_and_pattern(&parsed, &store, &index, source, "choose");
        let method_scope = index.scope_of(method).unwrap();
        let underscore = Name::new(store.names.intern("_"), Namespace::Term);
        let shadowed_wildcard = store.symbols.alloc(Symbol {
            name: underscore,
            owner: Some(method),
            kind: SymbolKind::Local,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(definitions.object_type),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(method_scope)
            .enter(underscore, shadowed_wildcard);
        let second_pattern = parsed.ast.alloc(Tree {
            kind: TreeKind::Ident(Ident {
                name: underscore,
                backquoted: false,
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
        let first = typer
            .run_expression_transaction(|typer, journal, mappings| {
                let typed =
                    typer.type_pattern(pattern, definitions.int, context, journal, mappings)?;
                Ok(typed)
            })
            .unwrap();
        let typed_tree = typer.typed_arena.get(first);
        assert!(matches!(typed_tree.kind, TreeKind::Ident(ident) if !ident.backquoted));
        assert_eq!(typed_tree.ty, definitions.int);
        assert_eq!(typer.typed_index.get(source, pattern), Some(first));

        let repeated = typer
            .run_expression_transaction(|typer, journal, mappings| {
                let before = mappings.len();
                let typed =
                    typer.type_pattern(pattern, definitions.int, context, journal, mappings)?;
                assert_eq!(mappings.len(), before);
                Ok(typed)
            })
            .unwrap();
        assert_eq!(repeated, first);

        let reference_pattern = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(
                    second_pattern,
                    definitions.object_type,
                    context,
                    journal,
                    mappings,
                )
            })
            .unwrap();
        assert_eq!(
            typer.typed_arena.get(reference_pattern).ty,
            definitions.object_type
        );
    }

    #[test]
    fn non_wildcard_identifier_pattern_is_explicitly_unsupported() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case value => 1 } }");
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
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "value" => {
                    Some(tree)
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
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap_err();
        assert!(matches!(
            error,
            TyperError::UnsupportedPattern {
                pattern_kind: PatternKind::Identifier,
                ..
            }
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn backquoted_underscore_is_not_a_wildcard() {
        let (mut parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, pattern) = method_and_pattern(&parsed, &store, &index, source, "choose");
        if let TreeKind::Ident(ident) = &mut parsed.ast.get_mut(pattern).kind {
            ident.backquoted = true;
        }
        let source_pattern = parsed.ast.get(pattern);
        assert!(matches!(source_pattern.kind, TreeKind::Ident(ident) if ident.backquoted));
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let error = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap_err();
        assert!(matches!(
            error,
            TyperError::UnsupportedPattern {
                pattern_kind: PatternKind::Identifier,
                ..
            }
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn pattern_selector_preserves_constant_types_and_widens_other_types() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case _ => 1 } }");
        let (method, pattern) = method_and_pattern(&parsed, &store, &index, source, "choose");
        let (mut typer, context) = context_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source,
            method,
        );
        let singleton = typer
            .store
            .types
            .alloc(Type::Constant(dotty_core::Constant::Int(42)));
        let mut journal = Vec::new();
        assert_eq!(
            typer
                .pattern_selector_type(singleton, &mut journal)
                .unwrap(),
            singleton
        );
        assert_eq!(
            typer
                .pattern_selector_type(definitions.int, &mut journal)
                .unwrap(),
            definitions.int
        );
        let typed_pattern = typer
            .run_expression_transaction(|typer, info_journal, mappings| {
                typer.type_pattern(pattern, singleton, context, info_journal, mappings)
            })
            .unwrap();
        assert_eq!(typer.typed_arena.get(typed_pattern).ty, singleton);
    }

    #[test]
    fn pattern_root_classifier_uses_stable_categories() {
        let mut store = SemanticStore::new();
        let underscore = store.names.intern("_");
        assert_eq!(
            pattern_kind(&TreeKind::Ident(dotty_core::ast::Ident {
                name: Name::new(underscore, Namespace::Term),
                backquoted: false,
            })),
            PatternKind::Identifier
        );
        assert_eq!(PatternKind::Identifier.as_str(), "identifier");
    }

    #[test]
    fn pattern_root_classifier_covers_common_unsupported_shapes() {
        let (parsed, _store, _packages, _definitions, _index, _source) = setup(
            "class C { def choose(x: Any): Int = x match { case 1 => 1; case y: Int => y; case Some(y) => 2; case 2 | 3 => 3 } }",
        );
        let categories = parsed
            .ast
            .iter()
            .map(|(_, tree)| pattern_kind(&tree.kind))
            .collect::<std::collections::HashSet<_>>();
        assert!(categories.contains(&PatternKind::Literal));
        assert!(categories.contains(&PatternKind::Typed));
        assert!(categories.contains(&PatternKind::Application));
        assert!(categories.contains(&PatternKind::Alternative));
    }
}
