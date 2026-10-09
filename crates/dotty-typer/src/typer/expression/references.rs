//! Identifier, literal, `this`, and selection expression typing.

use super::super::{ExpressionContext, SourceTyper, TyperError};
use crate::typer::MemberLookupError;
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn non_value_term_mapping_symbol(
        &self,
        source_tree: TreeId<Untyped>,
        typed_tree: TreeId<Typed>,
    ) -> Option<SymbolId> {
        let mut source_kind = &self.arena.try_get(source_tree)?.kind;
        while let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) = source_kind {
            source_kind = &self.arena.try_get(parens.inner)?.kind;
        }
        if !matches!(source_kind, TreeKind::Ident(_) | TreeKind::Select(_)) {
            return None;
        }
        let Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) = self
            .store
            .types
            .try_get(self.typed_arena.get(typed_tree).ty)
        else {
            return None;
        };
        if !self.store.symbols.contains(*symbol) {
            return None;
        }
        let kind = self.store.symbols.get(*symbol).kind;
        let invalid_as_expression = match source_kind {
            TreeKind::Ident(_) => kind == SymbolKind::Package,
            TreeKind::Select(_) => matches!(
                kind,
                SymbolKind::Package
                    | SymbolKind::Class
                    | SymbolKind::Trait
                    | SymbolKind::ModuleClass
                    | SymbolKind::TypeParameter
                    | SymbolKind::TypeAlias
            ),
            _ => false,
        };
        invalid_as_expression.then_some(*symbol)
    }

    pub(in crate::typer) fn validate_value_expression(
        &self,
        source_tree: TreeId<Untyped>,
        typed_tree: TreeId<Typed>,
    ) -> Result<(), TyperError> {
        if let Some(symbol) = self.non_value_term_mapping_symbol(source_tree, typed_tree) {
            return Err(TyperError::UnsupportedTermReference {
                source: self.source,
                tree_index: source_tree.index(),
                symbol,
                kind: self.store.symbols.get(symbol).kind,
            });
        }
        Ok(())
    }

    pub(in crate::typer) fn type_value_expression_inner(
        &mut self,
        source_tree: TreeId<Untyped>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let typed = self.type_expression_inner(source_tree, context, info_journal, new_mappings)?;
        self.validate_value_expression(source_tree, typed)?;
        Ok(typed)
    }

    pub(in crate::typer) fn enclosing_this_owner(
        &self,
        qualifier: Option<dotty_core::Name>,
        expression_owner: SymbolId,
        tree_index: u32,
    ) -> Result<SymbolId, TyperError> {
        use std::collections::HashSet;
        let mut current = Some(expression_owner);
        let mut seen = HashSet::new();
        while let Some(symbol) = current {
            if !self.store.symbols.contains(symbol) {
                return Err(TyperError::UnknownSymbol { symbol });
            }
            if !seen.insert(symbol) {
                return Err(TyperError::ThisOwnerCycle {
                    source: self.source,
                    owner: symbol,
                });
            }
            let declaration = self.store.symbols.get(symbol);
            if matches!(
                declaration.kind,
                SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
            ) && qualifier.is_none_or(|name| declaration.name == name)
            {
                return Ok(symbol);
            }
            current = declaration.owner;
        }
        Err(TyperError::ThisOwnerNotEnclosing {
            source: self.source,
            tree_index,
            qualifier,
            owner: expression_owner,
        })
    }

    pub(in crate::typer) fn require_stable_selection_prefix(
        &self,
        qualifier_type: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let stable = match self.store.types.try_get(qualifier_type) {
            Some(Type::ThisType { .. } | Type::Constant(_)) => true,
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if self.store.symbols.contains(*symbol) => {
                let declaration = self.store.symbols.get(*symbol);
                let by_name = matches!(
                    declaration.info,
                    SymbolInfo::Complete(info)
                        if matches!(self.store.types.try_get(info), Some(Type::ByName { .. }))
                );
                matches!(
                    declaration.kind,
                    SymbolKind::Parameter
                        | SymbolKind::Field
                        | SymbolKind::Value
                        | SymbolKind::Local
                        | SymbolKind::Package
                        | SymbolKind::Object
                ) && !declaration.flags.contains(SymbolFlags::MUTABLE)
                    && !by_name
                    && (declaration.kind == SymbolKind::Package
                        || declaration.kind != SymbolKind::Object
                        || self.source_module_class_of_object(*symbol).is_ok())
            }
            _ => false,
        };
        if stable {
            Ok(())
        } else {
            Err(TyperError::UnstableSelectionPrefix {
                source: self.source,
                tree_index,
                qualifier_type,
            })
        }
    }

    pub(in crate::typer) fn expression_type_of_symbol(
        &mut self,
        symbol: SymbolId,
        expression_owner: SymbolId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        let kind = self.store.symbols.get(symbol).kind;
        match kind {
            SymbolKind::Parameter
            | SymbolKind::Field
            | SymbolKind::Value
            | SymbolKind::Variable
            | SymbolKind::Local
            | SymbolKind::Method => {}
            SymbolKind::Object => {
                if self.source_module_class_of_object(symbol).is_err() {
                    return Err(TyperError::ObjectTermReferenceDeferred {
                        source: self.source,
                        tree_index,
                        symbol,
                    });
                }
            }
            _ => {
                return Err(TyperError::UnsupportedTermReference {
                    source: self.source,
                    tree_index,
                    symbol,
                    kind,
                });
            }
        }
        if !matches!(kind, SymbolKind::Object | SymbolKind::Package) {
            if self.initializing_local_symbols.contains(&symbol) {
                return Err(TyperError::RecursiveLocalValueInitializer {
                    source: self.source,
                    tree_index,
                    symbol,
                });
            }
            let info = self.completed_expression_symbol_info(symbol, info_journal)?;
            if matches!(self.store.types.try_get(info), Some(Type::Repeated { .. })) {
                return Err(TyperError::VarargsParameterReferenceDeferred {
                    source: self.source,
                    tree_index,
                    symbol,
                });
            }
        }
        let prefix =
            self.expression_term_prefix(symbol, expression_owner, tree_index, info_journal)?;
        Ok(self.store.types.alloc(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
        }))
    }

    pub(in crate::typer) fn completed_expression_symbol_info(
        &mut self,
        symbol: SymbolId,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        match *self.store.symbols.info(symbol) {
            SymbolInfo::Complete(ty) => Ok(ty),
            SymbolInfo::Missing => self.complete_symbol_inner(symbol, info_journal),
            SymbolInfo::Deferred(_) => Err(TyperError::DeferredSymbolCompletion { symbol }),
            SymbolInfo::Error => Err(TyperError::SymbolAlreadyErrored { symbol }),
        }
    }

    pub(in crate::typer) fn expression_term_prefix(
        &mut self,
        symbol: SymbolId,
        expression_owner: SymbolId,
        tree_index: u32,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TypeId, TyperError> {
        if self.store.symbols.get(symbol).kind == SymbolKind::Local {
            return Ok(self.definitions.no_prefix);
        }
        let owner = self.store.symbols.get(symbol).owner;
        if !owner.is_some_and(|owner| {
            matches!(
                self.store.symbols.get(owner).kind,
                SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
            )
        }) {
            return Ok(self.definitions.no_prefix);
        }
        let current_class = self.enclosing_this_owner(None, expression_owner, tree_index)?;
        let prefix = self.store.types.alloc(Type::ThisType {
            class: current_class,
        });
        if owner == Some(current_class) {
            return Ok(prefix);
        }
        let mut enclosing = self.store.symbols.get(current_class).owner;
        let mut seen = std::collections::HashSet::new();
        while let Some(class) = enclosing {
            if !self.store.symbols.contains(class) {
                return Err(TyperError::UnknownSymbol { symbol: class });
            }
            if !seen.insert(class) {
                return Err(TyperError::ThisOwnerCycle {
                    source: self.source,
                    owner: class,
                });
            }
            let declaration = self.store.symbols.get(class);
            enclosing = declaration.owner;
            if Some(class) == owner
                && matches!(
                    declaration.kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                )
            {
                return Ok(self.store.types.alloc(Type::ThisType { class }));
            }
        }
        let name = self.store.symbols.get(symbol).name;
        match self.lookup_members_journaled(prefix, name, info_journal) {
            Ok(candidates)
                if candidates
                    .iter()
                    .any(|candidate| candidate.symbol == symbol) =>
            {
                Ok(prefix)
            }
            Ok(_) => Ok(self.definitions.no_prefix),
            Err(MemberLookupError::ClassInfoUnavailable { symbol, .. })
                if !self.is_current_source_symbol(symbol) =>
            {
                // Member lookup can reach an uncompleted external `Object`
                // parent after proving no current-source path to this symbol.
                Ok(self.definitions.no_prefix)
            }
            Err(error) => Err(TyperError::MemberLookup(Box::new(error))),
        }
    }

    pub(in crate::typer) fn literal_type(
        &self,
        value: &dotty_core::Constant,
        tree_index: u32,
    ) -> Result<TypeId, TyperError> {
        use dotty_core::Constant;
        match value {
            Constant::Unit => Ok(self.definitions.unit),
            Constant::Boolean(_) => Ok(self.definitions.boolean),
            Constant::Byte(_) => Ok(self.definitions.byte),
            Constant::Short(_) => Ok(self.definitions.short),
            Constant::Char(_) => Ok(self.definitions.char),
            Constant::Int(_) => Ok(self.definitions.int),
            Constant::Long(_) => Ok(self.definitions.long),
            Constant::FloatBits(_) => Ok(self.definitions.float),
            Constant::DoubleBits(_) => Ok(self.definitions.double),
            Constant::String(_) | Constant::StringUtf16(_) => {
                Err(TyperError::StringLiteralTypingDeferred {
                    source: self.source,
                    tree_index,
                })
            }
            Constant::Null => Err(TyperError::NullLiteralTypingDeferred {
                source: self.source,
                tree_index,
            }),
            Constant::Class(_) => Err(TyperError::UnsupportedExpression {
                source: self.source,
                tree_index,
                expression_kind: "class constant",
            }),
        }
    }

    pub(in crate::typer) fn type_number_literal(
        &self,
        number: NumberLiteral,
        tree_index: u32,
    ) -> Result<dotty_core::Constant, TyperError> {
        use dotty_core::Constant;
        let spelling = self.store.names.resolve(number.text).to_owned();
        let digits = spelling.replace('_', "");
        match number.kind {
            NumberKind::Whole(radix) => {
                let unsigned = digits
                    .strip_prefix("0x")
                    .or_else(|| digits.strip_prefix("0X"))
                    .or_else(|| digits.strip_prefix("0b"))
                    .or_else(|| digits.strip_prefix("0B"));
                let parsed = if radix == 10 {
                    digits.parse::<i32>().ok()
                } else if (2..=36).contains(&radix) {
                    unsigned
                        .and_then(|digits| u32::from_str_radix(digits, radix).ok())
                        .map(|bits| bits as i32)
                } else {
                    None
                };
                parsed
                    .map(Constant::Int)
                    .ok_or(TyperError::IntegerLiteralOutOfRange {
                        source: self.source,
                        tree_index,
                        spelling,
                    })
            }
            NumberKind::Decimal | NumberKind::Floating => digits
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(Constant::double)
                .ok_or(TyperError::FloatingLiteralInvalid {
                    source: self.source,
                    tree_index,
                    spelling,
                }),
        }
    }
    pub(in crate::typer) fn type_literal_expression(
        &mut self,
        tree: TreeId<Untyped>,
        literal: dotty_core::ast::Literal,
        position: Option<SourceSpan>,
    ) -> Result<TreeId<Typed>, TyperError> {
        self.literal_type(&literal.value, tree.index())?;
        let ty = self
            .store
            .types
            .alloc(Type::Constant(literal.value.clone()));
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).literal(
                literal.value,
                ty,
                position,
            ),
        )
    }

    pub(in crate::typer) fn type_number_literal_expression(
        &mut self,
        tree: TreeId<Untyped>,
        number: NumberLiteral,
        position: Option<SourceSpan>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let value = self.type_number_literal(number, tree.index())?;
        let ty = self.store.types.alloc(Type::Constant(value.clone()));
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .literal(value, ty, position),
        )
    }

    pub(in crate::typer) fn type_this_expression(
        &mut self,
        tree: TreeId<Untyped>,
        this: dotty_core::ast::This,
        position: Option<SourceSpan>,
        context: ExpressionContext,
    ) -> Result<TreeId<Typed>, TyperError> {
        if self.function_literals.is_method(context.owner) {
            return Err(TyperError::FunctionLiteralCaptureUnsupported {
                source: self.source,
                tree_index: tree.index(),
                symbol: None,
            });
        }
        let class = self.enclosing_this_owner(this.qual, context.owner, tree.index())?;
        let ty = self.store.types.alloc(Type::ThisType { class });
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .this(this.qual, ty, position),
        )
    }

    pub(in crate::typer) fn type_identifier_expression(
        &mut self,
        tree: TreeId<Untyped>,
        ident: Ident,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let symbol = self.resolve_expression_term(ident.name, context, tree.index(), position)?;
        let declaration = self.store.symbols.get(symbol);
        let shared_owner = declaration
            .owner
            .filter(|owner| self.store.symbols.contains(*owner))
            .is_some_and(|owner| {
                matches!(
                    self.store.symbols.get(owner).kind,
                    SymbolKind::Object | SymbolKind::ModuleClass | SymbolKind::Package
                )
            });
        if self.function_literals.is_method(context.owner)
            && declaration.owner != Some(context.owner)
            && (matches!(declaration.kind, SymbolKind::Parameter | SymbolKind::Local)
                || (matches!(
                    declaration.kind,
                    SymbolKind::Field | SymbolKind::Value | SymbolKind::Variable
                ) && !shared_owner))
        {
            return Err(TyperError::FunctionLiteralCaptureUnsupported {
                source: self.source,
                tree_index: tree.index(),
                symbol: Some(symbol),
            });
        }
        let ty =
            self.expression_type_of_symbol(symbol, context.owner, tree.index(), info_journal)?;
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).ident_with_backquoted(
                ident.name,
                ident.backquoted,
                ty,
                position,
            ),
        )
    }

    pub(in crate::typer) fn type_selection_expression(
        &mut self,
        tree: TreeId<Untyped>,
        selection: dotty_core::ast::Select<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if !selection.name.is_term() {
            return Err(TyperError::TypeSelectionInExpression {
                source: self.source,
                tree_index: tree.index(),
            });
        }
        let qualifier = if let Some(qualifier) = self.type_selection_qualifier(
            selection.qualifier,
            context,
            tree.index(),
            position,
            info_journal,
            new_mappings,
        )? {
            qualifier
        } else {
            self.type_expression_inner(selection.qualifier, context, info_journal, new_mappings)?
        };
        let receiver_type = self.typed_arena.get(qualifier).ty;
        self.require_stable_selection_prefix(receiver_type, tree.index())?;
        let package_receiver = matches!(
            self.store.types.try_get(receiver_type),
            Some(Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            }) if self.store.symbols.contains(*symbol)
                && self.store.symbols.get(*symbol).kind == SymbolKind::Package
        );
        if package_receiver {
            let candidate = self
                .resolve_qualifier_symbol(tree, context.lexical, tree.index(), position)?
                .ok_or(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index: tree.index(),
                    receiver: receiver_type,
                    name: selection.name,
                })?;
            let ty = self.store.types.alloc(Type::TermRef {
                prefix: receiver_type,
                target: TermRefTarget::Symbol(candidate),
            });
            return Ok(
                TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).select(
                    qualifier,
                    selection.name,
                    selection.backquoted,
                    ty,
                    position,
                ),
            );
        }
        self.type_selected_member_on_qualifier(
            tree.index(),
            qualifier,
            selection.name,
            selection.backquoted,
            position,
            info_journal,
        )
    }

    /// Selects a member from a qualifier already typed by the caller.
    pub(in crate::typer) fn type_selected_member_on_qualifier(
        &mut self,
        tree_index: u32,
        qualifier: TreeId<Typed>,
        name: Name,
        backquoted: bool,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let receiver_type = self.typed_arena.get(qualifier).ty;
        let receiver = self.widen_expression_type_journaled(receiver_type, info_journal, 0)?;
        let receiver = self.this_type_receiver_view(receiver)?;
        let candidates = self
            .lookup_members_journaled(receiver, name, info_journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        let candidate = match candidates.as_slice() {
            [] => {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver,
                    name,
                });
            }
            [candidate] => candidate,
            _ => {
                return Err(TyperError::OverloadedSelectionDeferred {
                    source: self.source,
                    tree_index,
                    name,
                });
            }
        };
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: receiver_type,
            target: TermRefTarget::Symbol(candidate.symbol),
        });
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .select(qualifier, name, backquoted, ty, position),
        )
    }

    /// Type a selection qualifier without treating a package path as a value.
    /// The resulting source mappings still retain every node in the path.
    fn type_selection_qualifier(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            return Ok(self
                .non_value_term_mapping_symbol(tree, typed)
                .is_some()
                .then_some(typed));
        }
        let Some(node) = self.arena.try_get(tree) else {
            return Err(TyperError::TreeOutsideArena {
                source: self.source,
                tree_index: tree.index(),
            });
        };
        if let TreeKind::Select(_) = node.kind {
            return self
                .type_expression_inner(tree, context, info_journal, new_mappings)
                .map(Some);
        }
        let TreeKind::Ident(ident) = &node.kind else {
            return Ok(None);
        };
        let symbol =
            match self.expression_term_candidates(ident.name, context, tree_index, position) {
                Ok(candidates) if candidates.len() == 1 => candidates[0],
                Ok(_) => return Ok(None),
                Err(TyperError::TermNameNotFound { .. }) => {
                    let Some(symbol) =
                        self.resolve_qualifier_symbol(tree, context.lexical, tree_index, position)?
                    else {
                        return Ok(None);
                    };
                    symbol
                }
                Err(error) => return Err(error),
            };
        if !self.store.symbols.contains(symbol)
            || self.store.symbols.get(symbol).kind != SymbolKind::Package
        {
            return Ok(None);
        }
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(symbol),
        });
        let typed = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .ident_with_backquoted(ident.name, ident.backquoted, ty, node.position);
        self.typed_index
            .insert(self.source, tree, typed)
            .map_err(|error| TyperError::ConflictingTypedExpression {
                source: error.source,
                tree_index: error.untyped.index(),
                existing: error.existing.index(),
                attempted: error.attempted.index(),
            })?;
        new_mappings.push((self.source, tree));
        Ok(Some(typed))
    }
}
