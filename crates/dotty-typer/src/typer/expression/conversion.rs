//! Value adaptation through the canonical scala.Conversion type.

use super::*;
use crate::typer::application::ApplicationRequest;

impl SourceTyper<'_> {
    pub(in crate::typer) fn try_implicit_conversion(
        &mut self,
        source_tree: TreeId<Untyped>,
        argument: TreeId<Typed>,
        actual: TypeId,
        expected: TypeId,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
        if self.searching_conversion {
            return Ok(None);
        }
        self.searching_conversion = true;
        let result = self.semantic_attempt(Some(journal), |typer, nested| {
            typer.apply_implicit_conversion(
                source_tree,
                argument,
                actual,
                expected,
                context,
                nested,
            )
        });
        self.searching_conversion = false;
        match result {
            Err(TyperError::MissingContextualArgument { .. }) => Ok(None),
            other => other.map(Some),
        }
    }

    fn apply_implicit_conversion(
        &mut self,
        source_tree: TreeId<Untyped>,
        argument: TreeId<Typed>,
        actual: TypeId,
        expected: TypeId,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let tree_index = source_tree.index();
        let missing = TyperError::MissingContextualArgument {
            source: self.source,
            tree_index,
            expected,
        };
        let name = Name::new(self.store.names.intern("Conversion"), Namespace::Type);
        let Some(class) = self.lookup_default_type_import(name, tree_index, None)? else {
            return Err(missing);
        };
        let entry = self.store.symbols.get(class);
        if entry.kind != SymbolKind::Class
            || entry.name != name
            || !entry
                .owner
                .is_some_and(|package| self.package_path(package) == ["scala"])
        {
            return Err(missing);
        }
        let prefix = self.type_symbol_prefix(class);
        let tycon = self.store.types.alloc(Type::type_ref(prefix, class));
        let goal = self.store.types.alloc(Type::Applied {
            tycon,
            args: vec![actual, expected],
        });
        let evidence = self.search_contextual_argument(
            &dotty_core::types::MethodParam {
                name: dotty_core::TermName::new(name.text()),
                ty: goal,
                erased: false,
                varargs: false,
            },
            context,
            tree_index,
            journal,
        )?;
        let evidence_type =
            self.widen_expression_type_journaled(self.typed_arena.get(evidence).ty, journal, 0)?;
        let request = ApplicationRequest {
            function_tree: source_tree,
            argument_trees: std::slice::from_ref(&source_tree),
            application_kind: ApplyKind::Regular,
            context,
            tree_index,
        };
        let (function, callable) =
            self.resolve_apply_function(evidence, evidence_type, request, journal)?;
        let Some(Type::Method(method)) = self.store.types.try_get(callable).cloned() else {
            return Err(missing);
        };
        let [parameter] = method.params.as_slice() else {
            return Err(missing);
        };
        if method.kind != MethodKind::Plain
            || parameter.erased
            || parameter.varargs
            || !self.conforms(actual, parameter.ty).map_err(|e| {
                TyperError::ExpectedExpressionConformanceUnsupported {
                    source: self.source,
                    tree_index,
                    actual,
                    expected: parameter.ty,
                    error: Box::new(e),
                }
            })?
            || !self.conforms(method.result, expected).map_err(|e| {
                TyperError::ExpectedExpressionConformanceUnsupported {
                    source: self.source,
                    tree_index,
                    actual: method.result,
                    expected,
                    error: Box::new(e),
                }
            })?
        {
            return Err(missing);
        }
        let converted = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply(
            function,
            vec![argument],
            method.result,
            self.arena.get(source_tree).position,
        );
        if self.typed_index.get(self.source, source_tree) == Some(argument) {
            self.typed_index.remove(self.source, source_tree);
            self.typed_index
                .insert(self.source, source_tree, converted)
                .map_err(|error| TyperError::ConflictingTypedExpression {
                    source: error.source,
                    tree_index: error.untyped.index(),
                    existing: error.existing.index(),
                    attempted: error.attempted.index(),
                })?;
        }
        Ok(converted)
    }
}
