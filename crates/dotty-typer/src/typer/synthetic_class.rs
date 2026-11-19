//! Typed definition and construction of a synthetic local class with an empty constructor.
use super::*;
use dotty_core::ast::{DefDef, Template, TypeDef};
use dotty_core::{Namespace, Symbol, SymbolLinks, TermName, TypeName};

pub(super) struct LocalClassTrees {
    pub(super) definition: TreeId<Typed>,
    pub(super) instance: TreeId<Typed>,
    pub(super) template: TreeId<Typed>,
    pub(super) constructor: TreeId<Typed>,
}

impl SourceTyper<'_> {
    pub(super) fn finish_local_class(
        &mut self,
        holder: SymbolId,
        class_ref: TypeId,
        scope: ScopeId,
        body: Vec<TreeId<Typed>>,
        mixin: Option<TreeId<Typed>>,
        position: Option<SourceSpan>,
    ) -> LocalClassTrees {
        let class_name = self.store.symbols.get(holder).name;
        let object = self.definitions.object_type;
        let class_info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: self.definitions.no_prefix,
            class: holder,
            parents: std::iter::once(object)
                .chain(mixin.map(|t| self.typed_arena.get(t).ty))
                .collect(),
            declarations: scope,
            self_type: None,
        }));
        self.store
            .symbols
            .set_info(holder, SymbolInfo::Complete(class_info));
        let constructor_name = Name::new(self.store.names.intern("<init>"), Namespace::Term);
        let constructor_type = self.store.types.alloc(Type::Method(MethodType {
            params: vec![],
            result: class_ref,
            kind: MethodKind::Plain,
        }));
        let constructor = synthetic_symbol(
            self,
            constructor_name,
            holder,
            SymbolKind::Constructor,
            SymbolFlags::EMPTY,
            SymbolInfo::Complete(constructor_type),
        );
        self.store
            .scopes
            .get_mut(scope)
            .enter(constructor_name, constructor);
        let tpt = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types)
            .type_tree(class_ref, None);
        let ctor = self.typed_index.alloc_synthetic_definition(
            &mut self.typed_arena,
            constructor,
            Tree {
                kind: TreeKind::DefDef(DefDef {
                    name: TermName::new(constructor_name.text()),
                    type_params: vec![],
                    value_param_clauses: vec![vec![]],
                    source_param_clause_order: None,
                    tpt,
                    rhs: None,
                    metadata: (),
                }),
                position,
                ty: constructor_type,
            },
        );
        let parent =
            TypedAstBuilder::new(&mut self.typed_arena, &self.store.types).type_tree(object, None);
        let template = self.typed_arena.alloc(Tree {
            kind: TreeKind::Template(Template {
                constructor: ctor,
                parents: std::iter::once(parent).chain(mixin).collect(),
                self_val: None,
                body,
                metadata: (),
            }),
            position,
            ty: class_ref,
        });
        let cls = self.typed_index.alloc_synthetic_definition(
            &mut self.typed_arena,
            holder,
            Tree {
                kind: TreeKind::TypeDef(TypeDef {
                    name: TypeName::new(class_name.text()),
                    rhs: template,
                    metadata: (),
                    variance: None,
                }),
                position,
                ty: class_ref,
            },
        );
        let ctor_ref = self.store.types.alloc(Type::TermRef {
            prefix: class_ref,
            target: TermRefTarget::Symbol(constructor),
        });
        let mut builder = TypedAstBuilder::new(&mut self.typed_arena, &self.store.types);
        let new = builder.new_expr(tpt, class_ref, position);
        let selected = builder.select(new, constructor_name, false, ctor_ref, None);
        let init = builder.apply(selected, vec![], class_ref, position);
        LocalClassTrees {
            definition: cls,
            instance: init,
            template,
            constructor: ctor,
        }
    }
}

pub(super) fn synthetic_symbol(
    typer: &mut SourceTyper<'_>,
    name: Name,
    owner: SymbolId,
    kind: SymbolKind,
    flags: SymbolFlags,
    info: SymbolInfo,
) -> SymbolId {
    typer.store.symbols.alloc(Symbol {
        name,
        owner: Some(owner),
        kind,
        flags: flags | SymbolFlags::SYNTHETIC,
        visibility: Visibility::Public,
        info,
        origin: SymbolOrigin::Synthetic,
        annotations: vec![],
        position: None,
        links: SymbolLinks::default(),
    })
}
