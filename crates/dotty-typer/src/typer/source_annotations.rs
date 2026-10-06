//! Projection of ordinary source annotation syntax into shared semantic annotations.

use std::collections::{HashSet, VecDeque};

use dotty_core::ast::{ApplyKind, TreeKind, UntypedNode};
use dotty_core::types::{Annotation, AnnotationArgument, AnnotationValue, MethodKind, Type};
use dotty_core::{
    AnnotationId, Name, Namespace, SourceId, SymbolId, SymbolInfo, SymbolKind, TermName, TreeId,
    Untyped,
};

use super::lookup::{MAX_MEMBER_LOOKUP_DEPTH, class_symbol_for_type};
use super::{ExpressionContext, SourceTyper, TyperError};

// The source expression dispatcher starts consuming this foundation in #770.
#[allow(dead_code)]
impl SourceTyper<'_> {
    /// Projects one parser-produced annotation atomically. Repeating a
    /// successful request returns the same canonical annotation ID.
    pub(in crate::typer) fn type_source_annotation(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
    ) -> Result<AnnotationId, TyperError> {
        self.run_expression_transaction(|typer, info_journal, new_mappings| {
            typer.type_source_annotation_inner(tree, context, info_journal, new_mappings)
        })
    }

    pub(in crate::typer) fn type_source_annotation_inner(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        _new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<AnnotationId, TyperError> {
        if let Some(annotation) = self.source_annotations.get(&tree).copied() {
            return Ok(annotation);
        }
        let source = self.source;
        let malformed = || TyperError::MalformedSourceAnnotation {
            source,
            tree_index: tree.index(),
        };
        let Some(TreeKind::Apply(application)) = self.arena.try_get(tree).map(|node| &node.kind)
        else {
            return Err(malformed());
        };
        if application.kind != ApplyKind::Regular {
            return Err(malformed());
        }
        let Some(TreeKind::Select(constructor)) = self
            .arena
            .try_get(application.function)
            .map(|node| &node.kind)
        else {
            return Err(malformed());
        };
        if self.store.names.resolve(constructor.name.text()) != "<init>" {
            return Err(malformed());
        }
        let Some(TreeKind::New(new)) = self
            .arena
            .try_get(constructor.qualifier)
            .map(|node| &node.kind)
        else {
            return Err(malformed());
        };
        let tpt = new.tpt;
        let arguments = application.args.clone();
        let ty = self.type_of_tpt_inner_journaled(tpt, context.lexical, info_journal)?;
        let mut projected = Vec::with_capacity(arguments.len());
        let mut value_trees = Vec::with_capacity(arguments.len());
        let mut names = HashSet::new();
        for argument in arguments {
            let Some(node) = self.arena.try_get(argument) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: argument.index(),
                });
            };
            let (name, value_tree) = match &node.kind {
                TreeKind::NamedArg(named) => {
                    if !named.name.is_term() {
                        return Err(malformed());
                    }
                    let name = TermName::new(named.name.text());
                    if !names.insert(name) {
                        return Err(TyperError::SourceAnnotationDuplicateNamedArgument {
                            source: self.source,
                            tree_index: argument.index(),
                            name,
                        });
                    }
                    (Some(name), named.arg)
                }
                _ => (None, argument),
            };
            let Some(value_node) = self.arena.try_get(value_tree) else {
                return Err(TyperError::TreeOutsideArena {
                    source: self.source,
                    tree_index: value_tree.index(),
                });
            };
            let value = match &value_node.kind {
                TreeKind::Literal(literal) => literal.value.clone(),
                TreeKind::PhaseSpecific(UntypedNode::Number(number)) => {
                    self.type_number_literal(*number, value_tree.index())?
                }
                _ => {
                    return Err(TyperError::SourceAnnotationArgumentNotConstant {
                        source: self.source,
                        tree_index: value_tree.index(),
                    });
                }
            };
            projected.push(AnnotationArgument {
                name,
                value: AnnotationValue::Constant(value),
            });
            value_trees.push(value_tree);
        }
        let class = self
            .store
            .annotation_class(&Annotation::compact(ty))
            .ok_or(TyperError::SourceAnnotationClassDeferred {
                source: self.source,
                tree_index: tree.index(),
                class: None,
            })?;
        if !self.store.symbols.contains(class)
            || self.store.symbols.get(class).kind != SymbolKind::Class
        {
            return Err(TyperError::SourceAnnotationClassDeferred {
                source: self.source,
                tree_index: tree.index(),
                class: Some(class),
            });
        }
        self.verify_annotation_class(class, tree.index(), info_journal)?;
        self.verify_annotation_constructor(
            ty,
            class,
            &projected,
            &value_trees,
            tree.index(),
            info_journal,
        )?;
        let annotation = Annotation::with_arguments(ty, projected);
        let id = self.store.annotations.alloc(annotation);
        self.source_annotations.insert(tree, id);
        Ok(id)
    }

    fn verify_annotation_constructor(
        &mut self,
        ty: dotty_core::TypeId,
        class: SymbolId,
        arguments: &[AnnotationArgument],
        value_trees: &[TreeId<Untyped>],
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let source = self.source;
        let deferred = || TyperError::SourceAnnotationConstructorDeferred {
            source,
            tree_index,
            class,
        };
        let candidates = self.constructors_of_inner(ty, info_journal)?;
        let [candidate] = candidates.as_slice() else {
            return Err(deferred());
        };
        let Some(Type::Method(method)) = self.store.types.try_get(candidate.callable).cloned()
        else {
            return Err(deferred());
        };
        if method.kind != MethodKind::Plain {
            return Err(deferred());
        }
        if method.params.len() != arguments.len() {
            return Err(TyperError::SourceAnnotationConstructorArgumentMismatch {
                source: self.source,
                tree_index,
                class,
            });
        }
        let mut assigned = vec![false; method.params.len()];
        let mut positional = 0;
        let mut saw_named = false;
        for (argument, value_tree) in arguments.iter().zip(value_trees) {
            let parameter_index = if let Some(name) = argument.name {
                saw_named = true;
                method
                    .params
                    .iter()
                    .position(|parameter| parameter.name == name)
            } else if saw_named {
                None
            } else {
                let index = positional;
                positional += 1;
                Some(index)
            };
            let Some(parameter_index) = parameter_index.filter(|index| *index < assigned.len())
            else {
                return Err(TyperError::SourceAnnotationConstructorArgumentMismatch {
                    source: self.source,
                    tree_index: value_tree.index(),
                    class,
                });
            };
            if assigned[parameter_index] {
                return Err(TyperError::SourceAnnotationConstructorArgumentMismatch {
                    source: self.source,
                    tree_index: value_tree.index(),
                    class,
                });
            }
            assigned[parameter_index] = true;
            let AnnotationValue::Constant(value) = &argument.value;
            let actual = self.literal_type(value, value_tree.index()).map_err(|_| {
                TyperError::SourceAnnotationArgumentTypeDeferred {
                    source: self.source,
                    tree_index: value_tree.index(),
                    class,
                }
            })?;
            match self.conforms(actual, method.params[parameter_index].ty) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(TyperError::SourceAnnotationConstructorArgumentMismatch {
                        source: self.source,
                        tree_index: value_tree.index(),
                        class,
                    });
                }
                Err(_) => {
                    return Err(TyperError::SourceAnnotationArgumentTypeDeferred {
                        source: self.source,
                        tree_index: value_tree.index(),
                        class,
                    });
                }
            }
        }
        Ok(())
    }

    fn verify_annotation_class(
        &mut self,
        class: SymbolId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<(), TyperError> {
        let source = self.source;
        let deferred = || TyperError::SourceAnnotationClassDeferred {
            source,
            tree_index,
            class: Some(class),
        };
        let base_package = self
            .packages
            .get(&["scala", "annotation"])
            .ok_or_else(deferred)?;
        let annotation_name = self.store.names.get("Annotation").ok_or_else(deferred)?;
        let base_name = Name::new(annotation_name, Namespace::Type);
        let base_candidates = self
            .store
            .scopes
            .get(base_package.scope)
            .lookup_all(&base_name);
        let [base] = base_candidates else {
            return Err(deferred());
        };
        if !self.store.symbols.contains(*base)
            || self.store.symbols.get(*base).kind != SymbolKind::Class
        {
            return Err(deferred());
        }
        let base = *base;
        let mut pending = VecDeque::from([class]);
        let mut visited = HashSet::new();
        while let Some(current) = pending.pop_front() {
            if !visited.insert(current) {
                continue;
            }
            if current == base {
                return Ok(());
            }
            if current == self.definitions.object_class || current == self.definitions.any_class {
                continue;
            }
            if visited.len() > MAX_MEMBER_LOOKUP_DEPTH {
                return Err(deferred());
            }
            let info = self
                .class_info(current, info_journal)
                .map_err(|_| deferred())?;
            for parent in info.parents {
                let parent_class =
                    class_symbol_for_type(self.store, parent, true).map_err(|_| deferred())?;
                pending.push_back(parent_class);
            }
        }
        Err(TyperError::SourceAnnotationNotAnnotationClass {
            source: self.source,
            tree_index,
            class,
        })
    }
}
