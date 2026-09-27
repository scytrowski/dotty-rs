//! Source generic-class parameter recovery and receiver-specific adaptation.

use dotty_core::ast::TreeKind;
use dotty_core::types::{Type, TypeRefTarget, substitute_type_symbols};
use dotty_core::{SymbolId, SymbolInfo, SymbolKind, TypeId, Untyped};
use dotty_namer::SourceDefinition;

use crate::types::TypeNormalizer;
use crate::{MemberCandidate, SourceTyper, TyperError};

impl SourceTyper<'_> {
    /// Instantiates a source generic `ThisType` with its own type parameters
    /// for receiver lookup and member adaptation.
    pub(super) fn this_type_receiver_view(&mut self, ty: TypeId) -> Result<TypeId, TyperError> {
        let Type::ThisType { class } = *self.store.types.get(ty) else {
            return Ok(ty);
        };
        if !self.is_current_source_symbol(class) {
            return Ok(ty);
        }
        let parameters = self.source_class_type_parameters(class)?;
        if parameters.is_empty() {
            return Ok(ty);
        }
        let class_prefix = self.type_symbol_prefix(class);
        let tycon = self.store.types.alloc(Type::TypeRef {
            prefix: class_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let parameter_prefix = self.store.types.alloc(Type::ThisType { class });
        let arguments = parameters
            .into_iter()
            .map(|parameter| {
                self.store.types.alloc(Type::TypeRef {
                    prefix: parameter_prefix,
                    target: TypeRefTarget::Symbol(parameter),
                })
            })
            .collect();
        Ok(self.store.types.alloc(Type::Applied {
            tycon,
            args: arguments,
        }))
    }

    /// Adapts a completed declaration type for this candidate's instantiated
    /// declaring-class view. The canonical declaration info is never changed.
    pub fn member_type_on(&mut self, candidate: &MemberCandidate) -> Result<TypeId, TyperError> {
        let store_checkpoint = self.store.checkpoint();
        let mut journal = Vec::new();
        let checkpoint = self.type_index.checkpoint();
        let result = self.member_type_on_journaled(candidate, &mut journal);
        if result.is_err() {
            for (symbol, previous) in journal.into_iter().rev() {
                if self.store.symbols.contains(symbol) {
                    self.store.symbols.set_info(symbol, previous);
                }
            }
            self.store.rollback_to(store_checkpoint);
            self.type_index.restore(checkpoint);
        }
        result
    }

    pub(in crate::typer) fn member_type_on_journaled(
        &mut self,
        candidate: &MemberCandidate,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        // The lookup candidate owns the instantiated declaring-class view.
        // Taking no separate receiver avoids implying we can validate how it
        // was reached from an original receiver type.
        let view = self.class_view(candidate.receiver_view)?;
        if view.class != Some(candidate.declaring_class) {
            return Err(TyperError::ReceiverDoesNotDenoteExpectedClass {
                expected: candidate.declaring_class,
                actual: view.class,
            });
        }

        if !self.store.symbols.contains(candidate.symbol) {
            return Err(TyperError::UnknownSymbol {
                symbol: candidate.symbol,
            });
        }
        if matches!(
            *self.store.symbols.info(candidate.symbol),
            SymbolInfo::Missing
        ) && self.is_current_source_symbol(candidate.symbol)
        {
            self.complete_symbol_inner(candidate.symbol, journal)?;
        }
        let declaration = match *self.store.symbols.info(candidate.symbol) {
            SymbolInfo::Complete(info) => info,
            SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => {
                return Err(TyperError::MemberTypeUnavailable {
                    symbol: candidate.symbol,
                });
            }
        };

        if self.is_current_source_symbol(candidate.declaring_class) {
            let parameters = self.source_class_type_parameters(candidate.declaring_class)?;
            if view.applied {
                if parameters.len() != view.arguments.len() {
                    return Err(TyperError::ReceiverGenericArityMismatch {
                        class: candidate.declaring_class,
                        expected: parameters.len(),
                        actual: view.arguments.len(),
                    });
                }
                let substitutions: Vec<_> = parameters.into_iter().zip(view.arguments).collect();
                substitute_type_symbols(self.store, declaration, &substitutions)
                    .map_err(TyperError::TypeSubstitution)
            } else if !parameters.is_empty() {
                Err(TyperError::RawGenericSourceReceiverUnsupported {
                    class: candidate.declaring_class,
                    expected: parameters.len(),
                })
            } else {
                Ok(declaration)
            }
        } else if view.applied && !view.arguments.is_empty() {
            Err(TyperError::ExternalGenericInstantiationDeferred {
                class: candidate.declaring_class,
            })
        } else {
            Ok(declaration)
        }
    }

    /// Substitutes a source class's receiver arguments into one of its
    /// declaration-time parent views before lookup proceeds through it.
    pub(super) fn adapt_parent_view(
        &mut self,
        class: SymbolId,
        receiver_view: TypeId,
        parent_view: TypeId,
    ) -> Result<TypeId, TyperError> {
        let current = self.class_view(receiver_view)?;
        if current.class != Some(class) {
            return Err(TyperError::ReceiverDoesNotDenoteExpectedClass {
                expected: class,
                actual: current.class,
            });
        }
        if !self.is_current_source_symbol(class) {
            if current.applied && !current.arguments.is_empty() {
                return Err(TyperError::ExternalGenericInstantiationDeferred { class });
            }
            return Ok(parent_view);
        }
        let parameters = self.source_class_type_parameters(class)?;
        if current.applied {
            if parameters.len() != current.arguments.len() {
                return Err(TyperError::ReceiverGenericArityMismatch {
                    class,
                    expected: parameters.len(),
                    actual: current.arguments.len(),
                });
            }
            let substitutions: Vec<_> = parameters.into_iter().zip(current.arguments).collect();
            substitute_type_symbols(self.store, parent_view, &substitutions)
                .map_err(TyperError::TypeSubstitution)
        } else if !parameters.is_empty() {
            Err(TyperError::RawGenericSourceReceiverUnsupported {
                class,
                expected: parameters.len(),
            })
        } else {
            Ok(parent_view)
        }
    }

    pub(super) fn source_class_type_parameters(
        &self,
        class: SymbolId,
    ) -> Result<Vec<SymbolId>, TyperError> {
        if !self.store.symbols.contains(class) {
            return Err(TyperError::UnknownSymbol { symbol: class });
        }
        match self.store.symbols.get(class).kind {
            // Ordinary objects have no source class parameters in this model.
            SymbolKind::ModuleClass => return Ok(Vec::new()),
            SymbolKind::Class | SymbolKind::Trait => {}
            _ => {
                return Err(TyperError::MalformedSourceClassTypeParameters {
                    class,
                    tree_index: self
                        .index
                        .definition_of(class)
                        .map(source_tree)
                        .map_or(u32::MAX, |tree| tree.index()),
                });
            }
        }

        let Some(SourceDefinition::Canonical { source, tree }) = self.index.definition_of(class)
        else {
            return Err(TyperError::SourceClassTypeParametersProvenanceMissing { symbol: class });
        };
        if source != self.source {
            return Err(TyperError::SourceClassTypeParametersProvenanceMissing { symbol: class });
        }
        let source_node =
            self.arena
                .try_get(tree)
                .ok_or(TyperError::MalformedSourceClassTypeParameters {
                    class,
                    tree_index: tree.index(),
                })?;
        let TreeKind::TypeDef(definition) = &source_node.kind else {
            return Err(TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: tree.index(),
            });
        };
        let template_node = self.arena.try_get(definition.rhs).ok_or(
            TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: definition.rhs.index(),
            },
        )?;
        let TreeKind::Template(template) = &template_node.kind else {
            return Err(TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: definition.rhs.index(),
            });
        };
        let constructor_node = self.arena.try_get(template.constructor).ok_or(
            TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: template.constructor.index(),
            },
        )?;
        let TreeKind::DefDef(constructor) = &constructor_node.kind else {
            return Err(TyperError::MalformedSourceClassTypeParameters {
                class,
                tree_index: template.constructor.index(),
            });
        };

        let mut parameters = Vec::with_capacity(constructor.type_params.len());
        for parameter_tree in &constructor.type_params {
            let valid_tree = self
                .arena
                .try_get(*parameter_tree)
                .is_some_and(|node| matches!(node.kind, TreeKind::TypeDef(_)));
            if !valid_tree {
                return Err(TyperError::MalformedSourceClassTypeParameters {
                    class,
                    tree_index: parameter_tree.index(),
                });
            }
            let parameter = self.index.symbol_at(source, *parameter_tree).ok_or(
                TyperError::ClassTypeParameterSymbolMissing {
                    class,
                    tree_index: parameter_tree.index(),
                },
            )?;
            if !self.store.symbols.contains(parameter)
                || self.store.symbols.get(parameter).kind != SymbolKind::TypeParameter
                || self.store.symbols.get(parameter).owner != Some(class)
            {
                return Err(TyperError::ClassTypeParameterSymbolMissing {
                    class,
                    tree_index: parameter_tree.index(),
                });
            }
            parameters.push(parameter);
        }
        Ok(parameters)
    }

    fn class_view(&self, ty: TypeId) -> Result<ClassView, TyperError> {
        let normalized = TypeNormalizer::new(self.store)
            .normalize_for_lookup(ty)
            .map_err(TyperError::TypeNormalization)?;
        let (tycon, arguments, applied) = match self.store.types.get(normalized) {
            Type::Applied { tycon, args } => (*tycon, args.clone(), true),
            _ => (normalized, Vec::new(), false),
        };
        let normalized_tycon = TypeNormalizer::new(self.store)
            .normalize_for_lookup(tycon)
            .map_err(TyperError::TypeNormalization)?;
        let class = match self.store.types.try_get(normalized_tycon) {
            Some(Type::ThisType { class }) => Some(*class),
            Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(class),
                ..
            }) => Some(*class),
            _ => None,
        };
        Ok(ClassView {
            class,
            arguments,
            applied,
        })
    }
}

#[derive(Debug)]
struct ClassView {
    class: Option<SymbolId>,
    arguments: Vec<TypeId>,
    applied: bool,
}

fn source_tree(definition: SourceDefinition) -> dotty_core::TreeId<Untyped> {
    match definition {
        SourceDefinition::Canonical { tree, .. } | SourceDefinition::Derived { tree, .. } => tree,
    }
}
