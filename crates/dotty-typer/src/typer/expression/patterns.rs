//! Pattern typing entry points and pattern-specific type adaptation.

use super::{ExpressionContext, SourceTyper, TyperError};
use crate::typer::PatternKind;
use dotty_core::ast::{TreeKind, TypedAstBuilder, UntypedNode};
use dotty_core::types::Type;
use dotty_core::{
    Name, Namespace, SemanticStore, SourceId, SymbolFlags, SymbolId, SymbolInfo, SymbolKind,
    SymbolOrigin, TermRefTarget, TreeId, TypeId, Typed, Untyped,
};

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

impl SourceTyper<'_> {
    /// Enters one pattern binding into the active case scope.
    pub(super) fn enter_pattern_binding(
        &mut self,
        source_tree: TreeId<Untyped>,
        name: Name,
        binding_type: TypeId,
        case_context: ExpressionContext,
    ) -> Result<(SymbolId, TypeId), TyperError> {
        let Some(tree) = self.arena.try_get(source_tree) else {
            return Err(TyperError::MalformedPatternBinding {
                source: self.source,
                tree_index: source_tree.index(),
            });
        };
        let valid_source = match &tree.kind {
            TreeKind::Bind(binding) => binding.name == name,
            TreeKind::Ident(ident) => {
                ident.name == name
                    && !ident.backquoted
                    && is_variable_pattern_name(self.store, name)
            }
            _ => false,
        };
        if !valid_source || !name.is_term() {
            return Err(TyperError::MalformedPatternBinding {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }
        if self.store.names.resolve(name.text()) == "_" {
            return Err(TyperError::WildcardPatternBindingRejected {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }
        let stack =
            case_context
                .local_scopes
                .ok_or(TyperError::PatternBindingOutsideCaseScope {
                    source: self.source,
                    tree_index: source_tree.index(),
                })?;
        self.validate_expression_scope_stack(Some(stack))?;
        let frame = self
            .expression_scopes
            .get(stack.index())
            .ok_or(TyperError::ExpressionLocalScopeStackMissing { stack })?;
        if !frame.is_case_scope
            || self.store.scopes.get(frame.scope).owner != Some(case_context.owner)
        {
            return Err(TyperError::PatternBindingOutsideCaseScope {
                source: self.source,
                tree_index: source_tree.index(),
            });
        }
        let scope = frame.scope;
        if let Some(existing) = self
            .pattern_bindings
            .by_tree
            .get(&(self.source, source_tree))
            .copied()
        {
            let Some(existing_scope) = self
                .pattern_bindings
                .scope_by_symbol
                .get(&existing)
                .copied()
            else {
                return Err(TyperError::PatternBindingScopeConflict {
                    source: self.source,
                    tree_index: source_tree.index(),
                    existing_scope: scope,
                    attempted_scope: scope,
                });
            };
            if existing_scope != scope {
                return Err(TyperError::PatternBindingScopeConflict {
                    source: self.source,
                    tree_index: source_tree.index(),
                    existing_scope,
                    attempted_scope: scope,
                });
            }
            return Ok((existing, self.pattern_binding_term_ref(existing)));
        }
        if !self.store.scopes.get(scope).lookup_all(&name).is_empty() {
            return Err(TyperError::DuplicatePatternBinding {
                source: self.source,
                tree_index: source_tree.index(),
                name,
            });
        }

        let symbol = self.store.symbols.alloc(dotty_core::Symbol {
            name,
            owner: Some(case_context.owner),
            kind: SymbolKind::Local,
            flags: SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Complete(binding_type),
            origin: SymbolOrigin::Source(self.source),
            annotations: Vec::new(),
            position: tree.position,
            links: dotty_core::SymbolLinks::default(),
        });
        self.store.scopes.get_mut(scope).enter(name, symbol);
        self.pattern_bindings
            .by_tree
            .insert((self.source, source_tree), symbol);
        self.pattern_bindings.scope_by_symbol.insert(symbol, scope);
        Ok((symbol, self.pattern_binding_term_ref(symbol)))
    }

    fn pattern_binding_term_ref(&mut self, symbol: SymbolId) -> TypeId {
        self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(symbol),
        })
    }

    /// Types wildcard and variable-pattern roots. Other pattern families are
    /// introduced in later Match increments.
    pub(super) fn type_pattern(
        &mut self,
        pattern: TreeId<Untyped>,
        selector_type: TypeId,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
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
        let typed = match &source_tree.kind {
            TreeKind::Ident(ident) => {
                if !ident.name.is_term() {
                    return Err(TyperError::MalformedVariablePattern {
                        source: self.source,
                        tree_index: pattern.index(),
                    });
                }
                let spelling = self.store.names.resolve(ident.name.text());
                if !ident.backquoted && spelling == "_" {
                    TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                        .ident_with_backquoted(
                            ident.name,
                            false,
                            selector_type,
                            source_tree.position,
                        )
                } else if !ident.backquoted && is_variable_pattern_name(self.store, ident.name) {
                    let wildcard_name = Name::new(self.store.names.intern("_"), Namespace::Term);
                    let wildcard = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                        .ident_with_backquoted(
                            wildcard_name,
                            false,
                            selector_type,
                            source_tree.position,
                        );
                    let (_symbol, binding_type) =
                        self.enter_pattern_binding(pattern, ident.name, selector_type, context)?;
                    TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).bind(
                        ident.name,
                        wildcard,
                        binding_type,
                        false,
                        source_tree.position,
                    )
                } else {
                    return Err(TyperError::UnsupportedPattern {
                        source: self.source,
                        tree_index: pattern.index(),
                        pattern_kind: pattern_kind(&source_tree.kind),
                    });
                }
            }
            TreeKind::Literal(literal) => {
                let typed =
                    self.type_literal_expression(pattern, literal.clone(), source_tree.position)?;
                let actual = self.widen_expression_type_journaled(
                    self.typed_arena.get(typed).ty,
                    info_journal,
                    0,
                )?;
                self.require_pattern_compatible(actual, selector_type, pattern.index())?;
                typed
            }
            TreeKind::PhaseSpecific(UntypedNode::Number(number)) => {
                let typed =
                    self.type_number_literal_expression(pattern, *number, source_tree.position)?;
                let actual = self.widen_expression_type_journaled(
                    self.typed_arena.get(typed).ty,
                    info_journal,
                    0,
                )?;
                self.require_pattern_compatible(actual, selector_type, pattern.index())?;
                typed
            }
            TreeKind::Bind(binding) => {
                if binding.given {
                    return Err(TyperError::UnsupportedBindPatternBody {
                        source: self.source,
                        tree_index: binding.body.index(),
                    });
                }
                let body_is_wildcard = self.arena.try_get(binding.body).is_some_and(|tree| {
                    matches!(
                        &tree.kind,
                        TreeKind::Ident(body_ident)
                            if !body_ident.backquoted
                                && body_ident.name.is_term()
                                && self.store.names.resolve(body_ident.name.text()) == "_"
                    )
                });
                if !body_is_wildcard {
                    return Err(TyperError::UnsupportedBindPatternBody {
                        source: self.source,
                        tree_index: binding.body.index(),
                    });
                }
                let typed_body = self.type_pattern(
                    binding.body,
                    selector_type,
                    context,
                    info_journal,
                    new_mappings,
                )?;
                let (_symbol, binding_type) =
                    self.enter_pattern_binding(pattern, binding.name, selector_type, context)?;
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).bind(
                    binding.name,
                    typed_body,
                    binding_type,
                    false,
                    source_tree.position,
                )
            }
            _ => {
                return Err(TyperError::UnsupportedPattern {
                    source: self.source,
                    tree_index: pattern.index(),
                    pattern_kind: pattern_kind(&source_tree.kind),
                });
            }
        };
        if let Some(existing) = self.typed_index.get(self.source, pattern) {
            if existing != typed {
                return Err(TyperError::ConflictingTypedExpression {
                    source: self.source,
                    tree_index: pattern.index(),
                    existing: existing.index(),
                    attempted: typed.index(),
                });
            }
        } else {
            self.typed_index
                .insert(self.source, pattern, typed)
                .map_err(|conflict| TyperError::ConflictingTypedExpression {
                    source: conflict.source,
                    tree_index: conflict.untyped.index(),
                    existing: conflict.existing.index(),
                    attempted: conflict.attempted.index(),
                })?;
            new_mappings.push((self.source, pattern));
        }
        Ok(typed)
    }

    fn require_pattern_compatible(
        &mut self,
        actual: TypeId,
        selector: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let forward = self.conforms(actual, selector);
        if matches!(forward, Ok(true)) {
            return Ok(());
        }
        let reverse = self.conforms(selector, actual);
        if matches!(reverse, Ok(true)) {
            return Ok(());
        }
        if let Err(error) = forward {
            return Err(TyperError::PatternTypeRelationDeferred {
                source: self.source,
                tree_index,
                actual,
                selector,
                error: Box::new(error),
            });
        }
        if let Err(error) = reverse {
            return Err(TyperError::PatternTypeRelationDeferred {
                source: self.source,
                tree_index,
                actual,
                selector,
                error: Box::new(error),
            });
        }
        Err(TyperError::PatternTypeMismatch {
            source: self.source,
            tree_index,
            actual,
            selector,
        })
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

/// Mirrors the parser's pattern-variable first-character rule. Backquotes are
/// checked by the caller; literal keywords and the wildcard are never binders.
fn is_variable_pattern_name(store: &SemanticStore, name: Name) -> bool {
    let spelling = store.names.resolve(name.text());
    if matches!(spelling, "_" | "false" | "true" | "null") {
        return false;
    }
    spelling
        .chars()
        .next()
        .is_some_and(|first| first == '_' || (first.is_alphabetic() && first.is_lowercase()))
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

    fn method_symbol(
        parsed: &dotty_parser::ParseResult,
        store: &SemanticStore,
        index: &SourceSemanticIndex,
        source: SourceId,
    ) -> dotty_core::SymbolId {
        parsed
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
            .unwrap()
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
    fn uppercase_identifier_pattern_is_not_a_variable_binding() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case Value => 1 } }");
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
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == "Value" => {
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
    fn backquoted_lowercase_identifier_is_not_a_variable_binding() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case `value` => 1 } }");
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
                TreeKind::Ident(ident)
                    if store.names.resolve(ident.name.text()) == "value" && ident.backquoted =>
                {
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
        assert!(typer.pattern_bindings.by_tree.is_empty());
    }

    #[test]
    fn literal_patterns_keep_constant_types_and_check_selector_compatibility() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
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
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.int, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::Constant(dotty_core::Constant::Int(1)))
        ));
        assert!(matches!(
            typer.typed_arena.get(typed).kind,
            TreeKind::Literal(dotty_core::ast::Literal {
                value: dotty_core::Constant::Int(1)
            })
        ));

        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
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
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.boolean, context, journal, mappings)
            }),
            Err(TyperError::PatternTypeMismatch { actual, selector, .. })
                if actual == definitions.int && selector == definitions.boolean
        ));
        assert!(typer.typed_arena.iter().next().is_none());
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn boolean_literal_pattern_uses_boolean_selector_prototype() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Boolean): Int = x match { case true => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
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
        let typed = typer
            .run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, definitions.boolean, context, journal, mappings)
            })
            .unwrap();
        assert!(matches!(
            typer.store.types.try_get(typer.typed_arena.get(typed).ty),
            Some(Type::Constant(dotty_core::Constant::Boolean(true)))
        ));
    }

    #[test]
    fn unsupported_pattern_selector_relation_is_deferred() {
        let (parsed, mut store, packages, definitions, index, source) =
            setup("class C { def choose(x: Int): Int = x match { case 1 => 1 } }");
        let method = method_symbol(&parsed, &store, &index, source);
        let pattern = parsed
            .ast
            .iter()
            .find_map(|(_, node)| match node.kind {
                TreeKind::CaseDef(case_def) => Some(case_def.pattern),
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
        let unsupported_selector = typer.store.types.alloc(Type::And {
            left: definitions.int,
            right: definitions.object_type,
        });
        assert!(matches!(
            typer.run_expression_transaction(|typer, journal, mappings| {
                typer.type_pattern(pattern, unsupported_selector, context, journal, mappings)
            }),
            Err(TyperError::PatternTypeRelationDeferred { .. })
        ));
        assert_eq!(typer.typed_arena.iter().count(), 0);
        assert!(typer.typed_index.is_empty());
    }

    #[test]
    fn true_false_and_null_are_not_variable_patterns() {
        for keyword in ["true", "false", "null"] {
            let source_text = format!(
                "class C {{ def choose(x: Int): Int = x match {{ case {keyword} => 1 }} }}"
            );
            let (parsed, mut store, packages, definitions, index, source) = setup(&source_text);
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
                .find_map(|(_tree, node)| match node.kind {
                    TreeKind::CaseDef(case_def) => Some(case_def.pattern),
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
                TyperError::UnsupportedPattern { .. }
                    | TyperError::PatternTypeMismatch { .. }
                    | TyperError::NullLiteralTypingDeferred { .. }
            ));
            assert!(typer.pattern_bindings.by_tree.is_empty());
            assert_eq!(typer.typed_arena.iter().count(), 0);
        }
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
