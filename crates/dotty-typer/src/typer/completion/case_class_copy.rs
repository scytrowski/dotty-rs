//! Case-class copy defaults read the field of the original receiver.
use super::*;

impl<'a> SourceTyper<'a> {
    pub(in crate::typer) fn copy_default_field(
        &self,
        getter: SymbolId,
    ) -> Option<(SymbolId, &'a dotty_core::ast::DefDef<Untyped>)> {
        let identity = self.index.generated_case_class_method_identity(getter)?;
        let dotty_core::GeneratedCaseClassMethodRole::CopyDefault(ordinal) = identity.role else {
            return None;
        };
        let (copy, position) = self.store.default_getter_origin(getter)?;
        if position != ordinal
            || self.generated_case_class_for_copy(copy)? != identity.class
            || self.store.symbols.owner(getter) != Some(identity.class)
        {
            return None;
        }
        let SourceDefinition::Canonical { source, tree } =
            self.index.definition_of(identity.class)?
        else {
            return None;
        };
        if source != self.source {
            return None;
        }
        let TreeKind::TypeDef(class) = &self.arena.try_get(tree)?.kind else {
            return None;
        };
        let TreeKind::Template(template) = &self.arena.try_get(class.rhs)?.kind else {
            return None;
        };
        let TreeKind::DefDef(ctor) = &self.arena.try_get(template.constructor)?.kind else {
            return None;
        };
        let field = self
            .index
            .symbol_at(source, *ctor.value_param_clauses.first()?.get(ordinal)?)?;
        let entry = self.store.symbols.get(field);
        (entry.kind == SymbolKind::Field && entry.owner == Some(identity.class))
            .then_some((field, ctor))
    }

    /// An omitted value constrains the fresh copy binder through the getter's
    /// result on the original receiver. No argument tree is evaluated here.
    pub(in crate::typer) fn copy_default_inference_types(
        &mut self,
        function_type: TypeId,
        poly: &PolyType,
        supplied: usize,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<Vec<TypeId>>, TyperError> {
        let Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(copy),
        } = *self.store.types.get(function_type)
        else {
            return Ok(None);
        };
        if self.generated_case_class_for_copy(copy).is_none() {
            return Ok(None);
        }
        let Some(Type::Method(first)) = self.store.types.try_get(poly.result) else {
            return Err(TyperError::MalformedMethodClause {
                method: copy,
                method_tree_index: tree_index,
                clause_index: 1,
            });
        };
        let count = first.params.len();
        let mut defaults = Vec::new();
        for ordinal in supplied..count {
            let getter = self.store.default_getter(copy, ordinal).ok_or(
                TyperError::ApplicationArityMismatch {
                    source: self.source,
                    tree_index,
                    expected: count,
                    actual: supplied,
                },
            )?;
            let reference = self.store.types.alloc(Type::term_ref(prefix, getter));
            let signature = self.widen_expression_type_journaled(reference, journal, 0)?;
            let Some(Type::Poly(getter_type)) = self.store.types.try_get(signature) else {
                return Err(TyperError::MemberTypeUnavailable { symbol: getter });
            };
            defaults.push(getter_type.result);
        }
        Ok(Some(defaults))
    }

    /// Types the right-hand side of a generated copy default getter, `this.field`.
    pub fn type_generated_copy_default_body(
        &mut self,
        getter: SymbolId,
    ) -> Result<TreeId<Typed>, TyperError> {
        let (field, _) = self
            .copy_default_field(getter)
            .ok_or(TyperError::SourceProvenanceMissing { symbol: getter })?;
        if let Some(&body) = self.generated_factory_bodies.get(&getter) {
            return Ok(body);
        }
        let body = self.run_atomic(|typer, journal| {
            typer.complete_signature_parameter(getter, journal)?;
            let class = typer
                .store
                .symbols
                .owner(field)
                .ok_or(TyperError::SourceProvenanceMissing { symbol: field })?;
            let prefix = typer.store.types.alloc(Type::ThisType { class });
            let ty = typer.store.types.alloc(Type::term_ref(prefix, field));
            let entry = typer.store.symbols.get(field);
            let mut builder = TypedAstBuilder::new(&mut typer.typed_arena, &typer.store.types);
            let receiver = builder.this(None, prefix, entry.position);
            Ok(builder.select(receiver, entry.name, false, ty, entry.position))
        })?;
        self.generated_factory_bodies.insert(getter, body);
        Ok(body)
    }
}
