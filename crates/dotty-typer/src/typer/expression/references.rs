//! Identifier, literal, `this`, and selection expression typing.

use super::super::{ExpressionContext, SourceTyper, TyperError};
use crate::typer::MemberLookupError;
use dotty_core::ast::*;
use dotty_core::types::*;
use dotty_core::*;

impl SourceTyper<'_> {
    pub(in crate::typer) fn validate_cached_parent_argument_receiver_access(
        &self,
        source_tree: TreeId<Untyped>,
        typed_tree: TreeId<Typed>,
    ) -> Result<(), TyperError> {
        let Some(receiver) = self.uninitialized_parent_receiver else {
            return Ok(());
        };
        let mut pending = vec![typed_tree];
        let mut seen = std::collections::HashSet::new();
        while let Some(tree) = pending.pop() {
            if !seen.insert(tree) {
                continue;
            }
            let Some(node) = self.typed_arena.try_get(tree) else {
                continue;
            };
            let accesses_receiver = match &node.kind {
                TreeKind::This(_) => matches!(
                    self.store.types.try_get(node.ty),
                    Some(Type::ThisType { class }) if *class == receiver
                ),
                TreeKind::Ident(_) => match self.store.types.try_get(node.ty) {
                    Some(Type::TermRef {
                        target: TermRefTarget::Symbol(symbol),
                        ..
                    }) => self.is_instance_class_member(*symbol),
                    _ => false,
                },
                _ => false,
            };
            if accesses_receiver {
                return Err(TyperError::UninitializedParentArgumentReceiver {
                    source: self.source,
                    tree_index: source_tree.index(),
                    receiver,
                });
            }
            pending.extend(typed_tree_children(&node.kind));
        }
        Ok(())
    }

    fn is_instance_class_member(&self, symbol: SymbolId) -> bool {
        self.store.symbols.contains(symbol)
            && self.store.symbols.get(symbol).owner.is_some_and(|owner| {
                self.store.symbols.contains(owner)
                    && matches!(
                        self.store.symbols.get(owner).kind,
                        SymbolKind::Class | SymbolKind::Trait
                    )
            })
    }

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
        let symbol = match self
            .store
            .types
            .try_get(self.typed_arena.get(typed_tree).ty)
        {
            Some(
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                }
                | Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                },
            ) => symbol,
            _ => return None,
        };
        if !self.store.symbols.contains(*symbol) {
            return None;
        }
        let kind = self.store.symbols.get(*symbol).kind;
        let invalid_as_expression = match source_kind {
            TreeKind::Ident(_) | TreeKind::Select(_) => matches!(
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
                let stable_term = matches!(
                    declaration.kind,
                    SymbolKind::Parameter
                        | SymbolKind::Field
                        | SymbolKind::Value
                        | SymbolKind::Local
                        | SymbolKind::Package
                        | SymbolKind::Object
                ) && !declaration.flags.contains(SymbolFlags::MUTABLE)
                    && !by_name;
                if declaration.kind == SymbolKind::Object && stable_term {
                    self.module_class_of_object(*symbol)?;
                    true
                } else {
                    stable_term
                }
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

    pub(in crate::typer) fn require_stable_application_selection_prefix(
        &self,
        qualifier: TreeId<Typed>,
        qualifier_type: TypeId,
        tree_index: u32,
    ) -> Result<(), TyperError> {
        let is_initialized_constructor_result = self
            .typed_arena
            .try_get(qualifier)
            .and_then(|tree| match &tree.kind {
                TreeKind::Apply(application) => self.typed_arena.try_get(application.function),
                _ => None,
            })
            .and_then(|function| match &function.kind {
                TreeKind::Select(selection) => self.typed_arena.try_get(selection.qualifier),
                _ => None,
            })
            .is_some_and(|tree| matches!(tree.kind, TreeKind::New(_)));
        if is_initialized_constructor_result {
            Ok(())
        } else if self
            .typed_arena
            .try_get(qualifier)
            .is_some_and(|tree| matches!(tree.kind, TreeKind::Apply(_)))
            && self.is_direct_current_source_class_result(qualifier_type)
        {
            // A direct method application returning a top-level class from
            // this source batch has a stable, exact class type. This permits
            // call chains such as `holder.value().get()` without treating a
            // mutable field reference like `holder.current.get()` as stable.
            Ok(())
        } else {
            self.require_stable_selection_prefix(qualifier_type, tree_index)
        }
    }

    pub(in crate::typer) fn is_mutable_local_selection_receiver(
        &self,
        qualifier: TreeId<Typed>,
        qualifier_type: TypeId,
    ) -> bool {
        if !self
            .typed_arena
            .try_get(qualifier)
            .is_some_and(|tree| matches!(tree.kind, TreeKind::Ident(_)))
        {
            return false;
        }
        let Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) = self.store.types.try_get(qualifier_type)
        else {
            return false;
        };
        if !self.store.symbols.contains(*symbol) {
            return false;
        }
        let declaration = self.store.symbols.get(*symbol);
        matches!(declaration.kind, SymbolKind::Local | SymbolKind::Variable)
            && declaration.flags.contains(SymbolFlags::MUTABLE)
    }

    fn is_direct_current_source_class_result(&self, qualifier_type: TypeId) -> bool {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) = self.store.types.try_get(qualifier_type)
        else {
            return false;
        };
        if !self.is_current_source_symbol(*symbol) {
            return false;
        }
        let declaration = self.store.symbols.get(*symbol);
        declaration.kind == SymbolKind::Class
            && declaration
                .owner
                .is_some_and(|owner| self.store.symbols.get(owner).kind == SymbolKind::Package)
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
                self.module_class_of_object(symbol)
                    .map_err(|error| match error {
                        TyperError::ObjectModuleClassUnavailable { .. } => {
                            TyperError::ObjectTermReferenceDeferred {
                                source: self.source,
                                tree_index,
                                symbol,
                            }
                        }
                        error => error,
                    })?;
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
                if self
                    .store
                    .symbols
                    .get(symbol)
                    .flags
                    .contains(SymbolFlags::LAZY)
                {
                    return Err(TyperError::RecursiveLazyLocalValueInitializer {
                        source: self.source,
                        tree_index,
                        symbol,
                    });
                }
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
            Constant::Null => Ok(self.definitions.null_type),
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
        let widened_type = self.literal_type(&literal.value, tree.index())?;
        let ty = if matches!(literal.value, dotty_core::Constant::Null) {
            widened_type
        } else {
            self.store
                .types
                .alloc(Type::Constant(literal.value.clone()))
        };
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
        if self.uninitialized_parent_receiver == Some(class) {
            return Err(TyperError::UninitializedParentArgumentReceiver {
                source: self.source,
                tree_index: tree.index(),
                receiver: class,
            });
        }
        let ty = self.store.types.alloc(Type::ThisType { class });
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .this(this.qual, ty, position),
        )
    }

    pub(in crate::typer) fn type_super_expression(
        &mut self,
        tree: TreeId<Untyped>,
        super_tree: dotty_core::ast::Super<Untyped>,
        position: Option<SourceSpan>,
        context: ExpressionContext,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let source = self.source;
        let tree_index = tree.index();
        let unsupported = |reason| TyperError::UnsupportedSuper {
            source,
            tree_index,
            reason,
        };
        if super_tree.mix.is_some() {
            return Err(unsupported("trait-qualified super is not supported"));
        }
        if !matches!(
            self.arena.try_get(super_tree.qual).map(|node| &node.kind),
            Some(TreeKind::This(dotty_core::ast::This { qual: None }))
        ) {
            return Err(unsupported(
                "only the current unqualified super is supported",
            ));
        }
        if !self.store.symbols.contains(context.owner)
            || self.store.symbols.get(context.owner).kind != SymbolKind::Method
        {
            return Err(unsupported("super is only supported in an instance method"));
        }
        if self
            .store
            .symbols
            .get(context.owner)
            .flags
            .contains(SymbolFlags::STATIC)
        {
            return Err(unsupported("super is unavailable in a static method"));
        }
        let Some(current_class) = self.store.symbols.get(context.owner).owner else {
            return Err(unsupported("the enclosing method has no class owner"));
        };
        if !self.store.symbols.contains(current_class)
            || self.store.symbols.get(current_class).kind != SymbolKind::Class
            || !self.is_current_source_symbol(current_class)
        {
            return Err(unsupported(
                "super requires a source-defined ordinary class",
            ));
        }
        let current_info = self
            .class_info(current_class, info_journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        let mut direct_parents = Vec::new();
        for parent_type in current_info.parents {
            let Some(Type::TypeRef {
                target: TypeRefTarget::Symbol(parent),
                ..
            }) = self.store.types.try_get(parent_type)
            else {
                return Err(unsupported(
                    "the direct parent type is not a plain class type",
                ));
            };
            if *parent != self.definitions.object_class {
                direct_parents.push((parent_type, *parent));
            }
        }
        let [(parent_type, parent)] = direct_parents.as_slice() else {
            return Err(unsupported(
                "super requires exactly one direct non-Object class parent",
            ));
        };
        if !self.store.symbols.contains(*parent)
            || self.store.symbols.get(*parent).kind != SymbolKind::Class
        {
            return Err(unsupported("super parent must be an ordinary class"));
        }
        let this_type = self.store.types.alloc(Type::ThisType {
            class: current_class,
        });
        let super_type = self.store.types.alloc(Type::SuperType {
            this_type,
            super_type: *parent_type,
        });
        let qualifier =
            self.type_expression_inner(super_tree.qual, context, info_journal, new_mappings)?;
        let qualifier_type = self.typed_arena.get(qualifier).ty;
        if !matches!(
            self.store.types.try_get(qualifier_type),
            Some(Type::ThisType { class }) if *class == current_class
        ) {
            return Err(unsupported(
                "super qualifier does not bind to the current class",
            ));
        }
        Ok(self.typed_arena.alloc(Tree {
            kind: TreeKind::Super(dotty_core::ast::Super {
                qual: qualifier,
                mix: None,
            }),
            position,
            ty: super_type,
        }))
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
        if let Some(receiver) = self.uninitialized_parent_receiver
            && self.is_instance_class_member(symbol)
        {
            return Err(TyperError::UninitializedParentArgumentReceiver {
                source: self.source,
                tree_index: tree.index(),
                receiver,
            });
        }
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
        let qualifier_candidate = self.type_selection_qualifier(
            selection.qualifier,
            context,
            tree.index(),
            position,
            info_journal,
            new_mappings,
        )?;
        let qualifier = if let Some(qualifier) = qualifier_candidate {
            qualifier
        } else {
            self.type_expression_inner(selection.qualifier, context, info_journal, new_mappings)?
        };
        let receiver_type = self.typed_arena.get(qualifier).ty;
        if matches!(self.typed_arena.get(qualifier).kind, TreeKind::Super(_)) {
            return self.type_super_selected_member(
                tree.index(),
                qualifier,
                selection.name,
                selection.backquoted,
                position,
                info_journal,
            );
        }
        let java_class_qualifier = self.is_java_class_qualifier(qualifier);
        if !java_class_qualifier {
            self.require_stable_application_selection_prefix(
                qualifier,
                receiver_type,
                tree.index(),
            )?;
        }
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

    fn type_super_selected_member(
        &mut self,
        tree_index: u32,
        qualifier: TreeId<Typed>,
        name: Name,
        backquoted: bool,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let receiver_type = self.typed_arena.get(qualifier).ty;
        let parent_type = match self.store.types.try_get(receiver_type) {
            Some(Type::SuperType { super_type, .. }) => *super_type,
            _ => {
                return Err(TyperError::UnsupportedSuper {
                    source: self.source,
                    tree_index,
                    reason: "typed super expression has no SuperType",
                });
            }
        };
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(_),
            ..
        }) = self.store.types.try_get(parent_type)
        else {
            return Err(TyperError::UnsupportedSuper {
                source: self.source,
                tree_index,
                reason: "direct superclass is not a plain class type",
            });
        };
        let candidates = self
            .lookup_members_journaled(parent_type, name, info_journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        let [candidate] = candidates.as_slice() else {
            if candidates.is_empty() {
                return Err(TyperError::MemberNotFound {
                    source: self.source,
                    tree_index,
                    receiver: parent_type,
                    name,
                });
            }
            return Err(TyperError::OverloadedSelectionDeferred {
                source: self.source,
                tree_index,
                name,
            });
        };
        let symbol = candidate.symbol;
        let declaring_class = candidate.declaring_class;
        if !self.store.symbols.contains(symbol) {
            return Err(TyperError::UnknownSymbol { symbol });
        }
        let declaration = self.store.symbols.get(symbol);
        let forbidden_flags = [
            SymbolFlags::ABSTRACT,
            SymbolFlags::STATIC,
            SymbolFlags::EXTENSION,
        ];
        if declaration.owner != Some(declaring_class)
            || declaration.kind != SymbolKind::Method
            || forbidden_flags
                .iter()
                .any(|flag| declaration.flags.contains(*flag))
            || declaration.visibility != Visibility::Public
        {
            return Err(TyperError::UnsupportedSuper {
                source: self.source,
                tree_index,
                reason: "selected parent member is not a public concrete instance method",
            });
        }
        if let Some(
            SourceDefinition::Canonical { source, tree }
            | SourceDefinition::Derived { source, tree },
        ) = self.index.definition_of(symbol)
            && source == self.source
            && !matches!(
                self.arena.try_get(tree).map(|node| &node.kind),
                Some(TreeKind::DefDef(definition)) if definition.rhs.is_some()
            )
        {
            return Err(TyperError::UnsupportedSuper {
                source: self.source,
                tree_index,
                reason: "selected source method has no implementation body",
            });
        }
        let method_type = self.completed_expression_symbol_info(symbol, info_journal)?;
        let supported_signature = match self.store.types.try_get(method_type) {
            Some(Type::Method(method)) => {
                method.kind == MethodKind::Plain
                    && method.params.iter().all(|parameter| {
                        !parameter.erased
                            && !parameter.varargs
                            && !matches!(
                                self.store.types.try_get(parameter.ty),
                                Some(Type::ByName { .. })
                            )
                    })
                    && !matches!(
                        self.store.types.try_get(method.result),
                        Some(Type::Method(_) | Type::Poly(_))
                    )
            }
            // Parameterless `def m: T` is represented by its result type.
            // Explicit `def m(): T` retains a zero-argument Method wrapper.
            Some(ty) if !matches!(ty, Type::Poly(_) | Type::ByName { .. }) => true,
            _ => false,
        };
        if !supported_signature {
            return Err(TyperError::UnsupportedSuper {
                source: self.source,
                tree_index,
                reason: "selected parent member does not have one plain monomorphic clause",
            });
        }
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: receiver_type,
            target: TermRefTarget::Symbol(symbol),
        });
        Ok(
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                .select(qualifier, name, backquoted, ty, position),
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
        let java_class_qualifier = self.is_java_class_qualifier(qualifier);

        // `AnyRef.eq/ne` are compiler semantic members, not methods loaded
        // from the JVM Object class. Keep their selection narrower than
        // ordinary member lookup until the source model distinguishes those
        // owners and supports a complete reference hierarchy.
        if !java_class_qualifier && self.any_ref_identity_operator_name(name) {
            if self
                .any_ref_identity_symbol_for_receiver(receiver, name)
                .is_some()
            {
                let candidates = self
                    .lookup_members_journaled(receiver, name, info_journal)
                    .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
                if candidates.is_empty()
                    && let Some(symbol) = self.any_ref_identity_symbol_for_receiver(receiver, name)
                {
                    let ty = self.store.types.alloc(Type::TermRef {
                        prefix: receiver_type,
                        target: TermRefTarget::Symbol(symbol),
                    });
                    return Ok(
                        TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
                            .select(qualifier, name, backquoted, ty, position),
                    );
                }
            }
            return Err(TyperError::MemberNotFound {
                source: self.source,
                tree_index,
                receiver,
                name,
            });
        }

        let mut candidates = self
            .lookup_members_journaled(receiver, name, info_journal)
            .map_err(|error| TyperError::MemberLookup(Box::new(error)))?;
        if java_class_qualifier {
            candidates.retain(|candidate| {
                self.store.symbols.contains(candidate.symbol)
                    && self
                        .store
                        .symbols
                        .get(candidate.symbol)
                        .flags
                        .contains(SymbolFlags::STATIC)
            });
        }
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

    pub(in crate::typer) fn any_ref_identity_operator_name(&self, name: Name) -> bool {
        matches!(self.store.names.resolve(name.text()), "eq" | "ne")
    }

    pub(in crate::typer) fn is_any_ref_identity_symbol(&self, symbol: SymbolId) -> bool {
        let Some(operators) = self
            .definitions
            .any_ref_identity_operator_symbols(self.store)
        else {
            return false;
        };
        symbol == operators.eq || symbol == operators.ne
    }

    /// Produces the nominal view used only for checking a `this` value against
    /// the canonical AnyRef identity parameter. The typed tree keeps its
    /// original ThisType and source identity.
    pub(in crate::typer) fn any_ref_identity_argument_view(
        &mut self,
        ty: TypeId,
    ) -> Option<TypeId> {
        let Some(Type::ThisType { class }) = self.store.types.try_get(ty).cloned() else {
            return None;
        };
        if !self.is_supported_any_ref_identity_receiver(ty) {
            return None;
        }
        let prefix = self.type_symbol_prefix(class);
        Some(self.store.types.alloc(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }))
    }

    pub(in crate::typer) fn any_ref_identity_symbol_for_receiver(
        &self,
        receiver: TypeId,
        name: Name,
    ) -> Option<SymbolId> {
        if !self.is_supported_any_ref_identity_receiver(receiver) {
            return None;
        }
        let operators = self
            .definitions
            .any_ref_identity_operator_symbols(self.store)?;
        match self.store.names.resolve(name.text()) {
            "eq" => Some(operators.eq),
            "ne" => Some(operators.ne),
            _ => None,
        }
    }

    fn is_supported_any_ref_identity_receiver(&self, receiver: TypeId) -> bool {
        let class = match self.store.types.try_get(receiver) {
            Some(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(class),
            }) => {
                if !self.store.symbols.contains(*class) {
                    return false;
                }
                let Some(owner) = self.store.symbols.get(*class).owner else {
                    return false;
                };
                if *prefix != self.definitions.no_prefix
                    && !matches!(
                        self.store.types.try_get(*prefix),
                        Some(Type::TypeRef {
                            prefix: package_prefix,
                            target: TypeRefTarget::Symbol(package),
                        }) if *package_prefix == self.definitions.no_prefix && *package == owner
                    )
                {
                    return false;
                }
                *class
            }
            Some(Type::ThisType { class }) => *class,
            _ => return false,
        };
        if !self.store.symbols.contains(class) {
            return false;
        }
        let declaration = self.store.symbols.get(class);
        if declaration.kind != SymbolKind::Class
            || declaration.origin != SymbolOrigin::Source(self.source)
            || declaration.owner.is_none_or(|owner| {
                !self.store.symbols.contains(owner)
                    || self.store.symbols.get(owner).kind != SymbolKind::Package
            })
            || !matches!(
                self.index.definition_of(class),
                Some(SourceDefinition::Canonical { source, .. }) if source == self.source
            )
        {
            return false;
        }
        let SymbolInfo::Complete(info_id) = declaration.info else {
            return false;
        };
        let Some(Type::ClassInfo(info)) = self.store.types.try_get(info_id) else {
            return false;
        };
        if info.class != class || !self.store.scopes.contains(info.declarations) {
            return false;
        }
        if self.index.scope_of(class) != Some(info.declarations) {
            return false;
        }
        let declarations = self.store.scopes.get(info.declarations);
        if declarations.owner != Some(class)
            || declarations.entered_symbols().any(|symbol| {
                !self.store.symbols.contains(symbol)
                    || self.store.symbols.get(symbol).kind == SymbolKind::TypeParameter
            })
        {
            return false;
        }
        info.parents.as_slice() == [self.definitions.object_type]
    }

    /// Type a selection qualifier without treating a package path as a value.
    /// The resulting source mappings still retain every node in the path.
    pub(in crate::typer) fn type_selection_qualifier(
        &mut self,
        tree: TreeId<Untyped>,
        context: ExpressionContext,
        tree_index: u32,
        position: Option<SourceSpan>,
        info_journal: &mut Vec<(SymbolId, SymbolInfo)>,
        new_mappings: &mut Vec<(SourceId, TreeId<Untyped>)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
        if let Some(typed) = self.typed_index.get(self.source, tree) {
            self.validate_cached_parent_argument_receiver_access(tree, typed)?;
            let is_class_qualifier = self.non_value_term_mapping_symbol(tree, typed).is_some()
                || self.is_java_class_type(self.typed_arena.get(typed).ty);
            return Ok(is_class_qualifier.then_some(typed));
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
        if !self.store.symbols.contains(symbol) {
            return Ok(None);
        }
        let declaration = self.store.symbols.get(symbol);
        let is_package = declaration.kind == SymbolKind::Package;
        let is_java_class = declaration.kind == SymbolKind::Class
            && declaration.flags.contains(SymbolFlags::JAVA_DEFINED);
        if !is_package && !is_java_class {
            return Ok(None);
        }
        let ty = if is_package {
            self.store.types.alloc(Type::TermRef {
                prefix: self.definitions.no_prefix,
                target: TermRefTarget::Symbol(symbol),
            })
        } else {
            let prefix = self.type_symbol_prefix(symbol);
            self.store.types.alloc(Type::TypeRef {
                prefix,
                target: TypeRefTarget::Symbol(symbol),
            })
        };
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

    pub(in crate::typer) fn is_java_class_type(&self, ty: TypeId) -> bool {
        let Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) = self.store.types.try_get(ty)
        else {
            return false;
        };
        self.store.symbols.contains(*symbol)
            && self.store.symbols.get(*symbol).kind == SymbolKind::Class
            && self
                .store
                .symbols
                .get(*symbol)
                .flags
                .contains(SymbolFlags::JAVA_DEFINED)
    }

    pub(in crate::typer) fn is_java_class_qualifier(&self, tree: TreeId<Typed>) -> bool {
        let Some(node) = self.typed_arena.try_get(tree) else {
            return false;
        };
        let symbol = match (&node.kind, self.store.types.try_get(node.ty)) {
            (
                TreeKind::Ident(_) | TreeKind::Select(_),
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                }),
            ) => Some(*symbol),
            (
                TreeKind::Select(_),
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                }),
            ) => Some(*symbol),
            _ => None,
        };
        symbol.is_some_and(|symbol| {
            self.store.symbols.contains(symbol)
                && self.store.symbols.get(symbol).kind == SymbolKind::Class
                && self
                    .store
                    .symbols
                    .get(symbol)
                    .flags
                    .contains(SymbolFlags::JAVA_DEFINED)
        })
    }
}

fn typed_tree_children(kind: &TreeKind<Typed>) -> Vec<TreeId<Typed>> {
    match kind {
        TreeKind::Select(selection) => vec![selection.qualifier],
        TreeKind::Super(super_tree) => vec![super_tree.qual],
        TreeKind::Apply(application) => std::iter::once(application.function)
            .chain(application.args.iter().copied())
            .collect(),
        TreeKind::TypeApply(application) => std::iter::once(application.function)
            .chain(application.args.iter().copied())
            .collect(),
        TreeKind::New(new) => vec![new.tpt],
        TreeKind::Typed(ascription) => vec![ascription.expr, ascription.tpt],
        TreeKind::NamedArg(argument) => vec![argument.arg],
        TreeKind::Assign(assignment) => vec![assignment.lhs, assignment.rhs],
        TreeKind::Block(block) => block
            .stats
            .iter()
            .copied()
            .chain(std::iter::once(block.expr))
            .collect(),
        TreeKind::If(if_expr) => vec![if_expr.cond, if_expr.then_branch, if_expr.else_branch],
        TreeKind::Match(matched) => std::iter::once(matched.selector)
            .chain(matched.cases.iter().copied())
            .collect(),
        TreeKind::CaseDef(case) => std::iter::once(case.pattern)
            .chain(case.guard)
            .chain(std::iter::once(case.body))
            .collect(),
        TreeKind::Return(return_tree) => return_tree
            .expr
            .into_iter()
            .chain(return_tree.from)
            .collect(),
        TreeKind::While(while_tree) => vec![while_tree.cond, while_tree.body],
        TreeKind::Try(try_tree) => std::iter::once(try_tree.expr)
            .chain(try_tree.cases.iter().copied())
            .chain(try_tree.finalizer)
            .collect(),
        TreeKind::Closure(closure) => closure
            .env
            .iter()
            .copied()
            .chain(std::iter::once(closure.method))
            .chain(closure.tpt)
            .collect(),
        TreeKind::ValDef(definition) => std::iter::once(definition.tpt)
            .chain(definition.rhs)
            .collect(),
        TreeKind::DefDef(definition) => definition
            .type_params
            .iter()
            .copied()
            .chain(definition.value_param_clauses.iter().flatten().copied())
            .chain(std::iter::once(definition.tpt))
            .chain(definition.rhs)
            .collect(),
        TreeKind::TypeDef(definition) => vec![definition.rhs],
        TreeKind::Template(template) => std::iter::once(template.constructor)
            .chain(template.parents.iter().copied())
            .chain(template.self_val)
            .chain(template.body.iter().copied())
            .collect(),
        TreeKind::PackageDef(package) => std::iter::once(package.name)
            .chain(package.stats.iter().copied())
            .collect(),
        TreeKind::Import(import) => std::iter::once(import.expr)
            .chain(
                import
                    .selectors
                    .iter()
                    .flat_map(|selector| selector.renamed),
            )
            .chain(import.selectors.iter().flat_map(|selector| selector.bound))
            .collect(),
        TreeKind::Export(export) => std::iter::once(export.expr)
            .chain(
                export
                    .selectors
                    .iter()
                    .flat_map(|selector| selector.renamed),
            )
            .chain(export.selectors.iter().flat_map(|selector| selector.bound))
            .collect(),
        TreeKind::SingletonTypeTree(singleton) => vec![singleton.reference],
        TreeKind::AppliedTypeTree(applied) => std::iter::once(applied.tpt)
            .chain(applied.args.iter().copied())
            .collect(),
        TreeKind::RefinedTypeTree(refined) => std::iter::once(refined.tpt)
            .chain(refined.refinements.iter().copied())
            .collect(),
        TreeKind::LambdaTypeTree(lambda) => lambda
            .type_params
            .iter()
            .copied()
            .chain(std::iter::once(lambda.body))
            .collect(),
        TreeKind::MatchTypeTree(matched) => matched
            .bound
            .into_iter()
            .chain(std::iter::once(matched.selector))
            .chain(matched.cases.iter().copied())
            .collect(),
        TreeKind::ByNameTypeTree(by_name) => vec![by_name.result],
        TreeKind::TypeBoundsTree(bounds) => bounds
            .low
            .into_iter()
            .chain(bounds.high)
            .chain(bounds.alias)
            .collect(),
        TreeKind::Bind(bind) => vec![bind.body],
        TreeKind::Alternative(alternative) => alternative.alternatives.clone(),
        TreeKind::UnApply(unapply) => std::iter::once(unapply.function)
            .chain(unapply.implicits.iter().copied())
            .chain(unapply.patterns.iter().copied())
            .collect(),
        TreeKind::Annotated(annotated) => vec![annotated.expr, annotated.annotation],
        TreeKind::Quote(quote) => std::iter::once(quote.body)
            .chain(quote.tags.iter().copied())
            .collect(),
        TreeKind::Splice(splice) => vec![splice.expr],
        TreeKind::QuotePattern(pattern) => pattern
            .bindings
            .iter()
            .copied()
            .chain(std::iter::once(pattern.body))
            .chain(std::iter::once(pattern.quotes))
            .collect(),
        TreeKind::SplicePattern(pattern) => std::iter::once(pattern.body)
            .chain(pattern.type_args.iter().copied())
            .chain(pattern.args.iter().copied())
            .collect(),
        TreeKind::Inlined(inlined) => inlined
            .bindings
            .iter()
            .copied()
            .chain(std::iter::once(inlined.expansion))
            .collect(),
        TreeKind::Ident(_) | TreeKind::This(_) | TreeKind::Literal(_) | TreeKind::TypeTree(_) => {
            Vec::new()
        }
        TreeKind::PhaseSpecific(never) => match *never {},
    }
}
