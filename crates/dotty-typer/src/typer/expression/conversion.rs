//! Value adaptation through scala.Conversion instances and unary implicit methods.

use super::*;
use crate::typer::application::ApplicationRequest;
use crate::typer::contextual::{ConversionGoal, ConversionTarget, MemberConversionGoal};

#[derive(Clone, Copy)]
pub(super) enum ConversionUse {
    Expected(TypeId),
    Member { name: Name, site: u32 },
}

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
        self.try_conversion(
            source_tree,
            argument,
            actual,
            ConversionUse::Expected(expected),
            context,
            journal,
        )
    }

    pub(super) fn try_conversion(
        &mut self,
        source_tree: TreeId<Untyped>,
        argument: TreeId<Typed>,
        actual: TypeId,
        purpose: ConversionUse,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
        if self.searching_conversion {
            return Ok(None);
        }
        self.searching_conversion = true;
        let result = self.semantic_attempt(Some(journal), |typer, nested| {
            typer.apply_implicit_conversion(source_tree, argument, actual, purpose, context, nested)
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
        purpose: ConversionUse,
        context: ExpressionContext,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let tree_index = match purpose {
            ConversionUse::Member { site, .. } => site,
            ConversionUse::Expected(_) => source_tree.index(),
        };
        let name = Name::new(self.store.names.intern("Conversion"), Namespace::Type);
        let class = self
            .lookup_default_type_import(name, tree_index, None)?
            .filter(|class| {
                let entry = self.store.symbols.get(*class);
                entry.kind == SymbolKind::Class
                    && entry.name == name
                    && entry
                        .owner
                        .is_some_and(|package| self.package_path(package) == ["scala"])
            });
        let source = self.source;
        let missing = |expected| TyperError::MissingContextualArgument {
            source,
            tree_index,
            expected,
        };
        let (target, expected) = match purpose {
            ConversionUse::Expected(expected) => {
                let instance = class.map(|class| {
                    let prefix = self.type_symbol_prefix(class);
                    let tycon = self.store.types.alloc(Type::type_ref(prefix, class));
                    self.store.types.alloc(Type::Applied {
                        tycon,
                        args: vec![actual, expected],
                    })
                });
                let target = self
                    .conversion_target(
                        ConversionGoal {
                            instance,
                            actual,
                            expected,
                        },
                        context,
                        tree_index,
                        journal,
                    )?
                    .ok_or_else(|| missing(expected))?;
                (target, expected)
            }
            ConversionUse::Member { name, .. } => {
                let class = class.ok_or_else(|| missing(actual))?;
                let (evidence, output) = self
                    .member_conversion_target(
                        MemberConversionGoal {
                            class,
                            receiver: actual,
                            name,
                        },
                        context,
                        tree_index,
                        journal,
                    )?
                    .ok_or_else(|| missing(actual))?;
                (ConversionTarget::Instance(evidence), output)
            }
        };
        let missing = missing(expected);
        let (function, callable) = match target {
            ConversionTarget::Method {
                function,
                signature,
            } => (function, signature),
            ConversionTarget::Instance(evidence) => {
                let evidence_type = self.widen_expression_type_journaled(
                    self.typed_arena.get(evidence).ty,
                    journal,
                    0,
                )?;
                let request = ApplicationRequest {
                    function_tree: source_tree,
                    argument_trees: std::slice::from_ref(&source_tree),
                    application_kind: ApplyKind::Regular,
                    context,
                    tree_index,
                };
                self.resolve_apply_function(evidence, evidence_type, request, journal)?
            }
        };
        let Some(Type::Method(method)) = self.store.types.try_get(callable).cloned() else {
            return Err(missing);
        };
        let [parameter] = method.params.as_slice() else {
            return Err(missing);
        };
        if method.kind != MethodKind::Plain
            || parameter.erased
            || parameter.varargs
            || !self.contextual_conforms(actual, parameter.ty, tree_index, journal)?
            || !self.contextual_conforms(method.result, expected, tree_index, journal)?
        {
            return Err(missing);
        }
        let converted = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).apply(
            function,
            vec![argument],
            method.result,
            self.arena.get(source_tree).position,
        );
        if matches!(purpose, ConversionUse::Expected(_))
            && self.typed_index.get(self.source, source_tree) == Some(argument)
        {
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
