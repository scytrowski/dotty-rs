//! Recursive implicit arguments are fields of one compiler-generated holder.
//! Each by-name use inside an initializer selects that holder's own field.
//! The completed expression selects the same field on the allocated instance.

use super::*;
use crate::typer::synthetic_class::synthetic_symbol;
use dotty_core::ast::ValDef;
use dotty_core::{Namespace, Scope, TermName};

#[derive(Clone, Default)]
pub(super) struct Dictionary {
    holder: Option<SymbolId>,
    entries: Vec<Entry>,
    uses: Vec<FieldUse>,
}

#[derive(Clone)]
struct Entry {
    goal: TypeId,
    field: SymbolId,
    initializer: Option<TreeId<Typed>>,
}

#[derive(Clone)]
struct FieldUse {
    tree: TreeId<Typed>,
    field: SymbolId,
    inside_holder: bool,
}

impl Dictionary {
    pub(super) fn use_count(&self) -> usize {
        self.uses.len()
    }

    fn entry_for(
        &self,
        typer: &mut SourceTyper<'_>,
        goal: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<usize>, TyperError> {
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.goal == goal
                || (typer.contextual_conforms(entry.goal, goal, tree_index, journal)?
                    && typer.contextual_conforms(goal, entry.goal, tree_index, journal)?)
            {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn reference(&mut self, typer: &mut SourceTyper<'_>, index: usize) -> TreeId<Typed> {
        let field = self.entries[index].field;
        let name = typer.store.symbols.get(field).name;
        let ty = typer.store.types.alloc(Type::TermRef {
            prefix: typer.definitions.no_prefix,
            target: TermRefTarget::Symbol(field),
        });
        let tree =
            TypedAstBuilder::new(&mut typer.typed_arena, &typer.store.types).ident(name, ty, None);
        self.uses.push(FieldUse {
            tree,
            field,
            inside_holder: false,
        });
        tree
    }

    pub(super) fn backreference(
        &mut self,
        typer: &mut SourceTyper<'_>,
        goal: TypeId,
        owner: SymbolId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        if let Some(index) = self.entry_for(typer, goal, tree_index, journal)? {
            return Ok(self.reference(typer, index));
        }
        let holder = *self.holder.get_or_insert_with(|| {
            let name = Name::new(
                typer.store.names.intern("$lazyImplicit$dictionary"),
                Namespace::Type,
            );
            synthetic_symbol(
                typer,
                name,
                owner,
                SymbolKind::Class,
                SymbolFlags::FINAL,
                SymbolInfo::Missing,
            )
        });
        let index = self.entries.len();
        let name = Name::new(
            typer.store.names.intern(&format!("$lazyImplicit${index}")),
            Namespace::Term,
        );
        let field = synthetic_symbol(
            typer,
            name,
            holder,
            SymbolKind::Field,
            SymbolFlags::FINAL,
            SymbolInfo::Complete(goal),
        );
        self.entries.push(Entry {
            goal,
            field,
            initializer: None,
        });
        Ok(self.reference(typer, index))
    }

    pub(super) fn existing_reference(
        &mut self,
        typer: &mut SourceTyper<'_>,
        goal: TypeId,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<Option<TreeId<Typed>>, TyperError> {
        Ok(self
            .entry_for(typer, goal, tree_index, journal)?
            .map(|index| self.reference(typer, index)))
    }

    pub(super) fn define(
        &mut self,
        typer: &mut SourceTyper<'_>,
        goal: TypeId,
        initializer: TreeId<Typed>,
        first_use: usize,
        tree_index: u32,
        journal: &mut Vec<(SymbolId, SymbolInfo)>,
    ) -> Result<TreeId<Typed>, TyperError> {
        let Some(index) = self.entry_for(typer, goal, tree_index, journal)? else {
            return Ok(initializer);
        };
        self.entries[index].initializer = Some(initializer);
        for use_ in &mut self.uses[first_use..] {
            use_.inside_holder = true;
        }
        Ok(self.reference(typer, index))
    }

    pub(super) fn emit(
        self,
        typer: &mut SourceTyper<'_>,
        result: TreeId<Typed>,
        tree_index: u32,
    ) -> Result<TreeId<Typed>, TyperError> {
        let Some(holder) = self.holder else {
            return Ok(result);
        };
        let deferred = || TyperError::UsingApplicationDeferred {
            source: typer.source,
            tree_index,
        };
        let owner = typer.store.symbols.owner(holder).ok_or_else(deferred)?;
        let class_ref = typer
            .store
            .types
            .alloc(Type::type_ref(typer.definitions.no_prefix, holder));
        let instance_name = Name::new(
            typer.store.names.intern("$lazyImplicit$instance"),
            Namespace::Term,
        );
        let instance = synthetic_symbol(
            typer,
            instance_name,
            owner,
            SymbolKind::Local,
            SymbolFlags::FINAL,
            SymbolInfo::Complete(class_ref),
        );
        qualify_references(typer, self.uses, instance, holder);
        let (class_tree, init) = define_holder(typer, holder, class_ref, self.entries, tree_index)?;
        let instance_tree = value_definition(typer, instance, class_ref, init);
        let ty = typer.typed_arena.get(result).ty;
        Ok(
            TypedAstBuilder::new(&mut typer.typed_arena, &typer.store.types).block(
                vec![class_tree, instance_tree],
                result,
                ty,
                None,
            ),
        )
    }
}

fn qualify_references(
    typer: &mut SourceTyper<'_>,
    uses: Vec<FieldUse>,
    instance: SymbolId,
    holder: SymbolId,
) {
    let this_type = typer.store.types.alloc(Type::ThisType { class: holder });
    let instance_name = typer.store.symbols.get(instance).name;
    let instance_ref = typer.store.types.alloc(Type::TermRef {
        prefix: typer.definitions.no_prefix,
        target: TermRefTarget::Symbol(instance),
    });
    let mut builder = TypedAstBuilder::new(&mut typer.typed_arena, &typer.store.types);
    let this = builder.this(None, this_type, None);
    let external = builder.ident(instance_name, instance_ref, None);

    // Only trees allocated by this search are rewritten, after all probes
    // have finished. A pending field never enters ordinary implicit scope.
    for use_ in uses {
        let (qualifier, prefix) = if use_.inside_holder {
            (this, this_type)
        } else {
            (external, instance_ref)
        };
        let node = typer.typed_arena.get_mut(use_.tree);
        node.kind = TreeKind::Select(dotty_core::ast::Select {
            qualifier,
            name: typer.store.symbols.get(use_.field).name,
            backquoted: false,
        });
        node.ty = typer.store.types.alloc(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(use_.field),
        });
    }
}

fn define_holder(
    typer: &mut SourceTyper<'_>,
    holder: SymbolId,
    class_ref: TypeId,
    entries: Vec<Entry>,
    tree_index: u32,
) -> Result<(TreeId<Typed>, TreeId<Typed>), TyperError> {
    let scope = typer.store.scopes.alloc(Scope::new(Some(holder)));
    let mut body = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = typer.store.symbols.get(entry.field).name;
        let rhs = entry
            .initializer
            .ok_or(TyperError::MissingContextualArgument {
                source: typer.source,
                tree_index,
                expected: entry.goal,
            })?;
        typer.store.scopes.get_mut(scope).enter(name, entry.field);
        body.push(value_definition(typer, entry.field, entry.goal, rhs));
    }
    let trees = typer.finish_local_class(holder, class_ref, scope, body, None, None);
    Ok((trees.definition, trees.instance))
}

fn value_definition(
    typer: &mut SourceTyper<'_>,
    symbol: SymbolId,
    ty: TypeId,
    rhs: TreeId<Typed>,
) -> TreeId<Typed> {
    let name = TermName::new(typer.store.symbols.get(symbol).name.text());
    let tpt = TypedAstBuilder::new(&mut typer.typed_arena, &typer.store.types).type_tree(ty, None);
    typer.typed_index.alloc_synthetic_definition(
        &mut typer.typed_arena,
        symbol,
        Tree {
            kind: TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs: Some(rhs),
                metadata: (),
            }),
            position: None,
            ty,
        },
    )
}
