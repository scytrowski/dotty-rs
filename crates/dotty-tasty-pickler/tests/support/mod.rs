use std::collections::HashMap;

use dotty_core::ast::*;
use dotty_core::*;
use dotty_tasty_pickler::*;

#[derive(Default)]
pub struct Bindings(pub HashMap<(SourceId, TreeId<Untyped>), TreeId<Typed>>);

impl SourceTypedBindings for Bindings {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.0.get(&(source, tree)).copied()
    }
}

pub fn node(arena: &mut AstArena<Untyped>, kind: TreeKind<Untyped>) -> TreeId<Untyped> {
    arena.alloc(Tree {
        kind,
        position: None,
        ty: (),
    })
}

pub fn source_symbol(
    store: &mut SemanticStore,
    name: Name,
    kind: SymbolKind,
    owner: SymbolId,
    source: SourceId,
    info: SymbolInfo,
) -> SymbolId {
    store.symbols.alloc(Symbol {
        name,
        owner: Some(owner),
        kind,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info,
        origin: SymbolOrigin::Source(source),
        annotations: vec![],
        position: None,
        links: SymbolLinks::default(),
    })
}

pub struct Fixture {
    pub store: SemanticStore,
    pub definitions: Definitions,
    pub source_ast: AstArena<Untyped>,
    pub typed_ast: AstArena<Typed>,
    pub index: SourceSemanticIndex,
    pub bindings: Bindings,
    pub source: SourceId,
    pub package: SymbolId,
    pub package_root: SymbolId,
    pub class: SymbolId,
    pub scope: ScopeId,
    pub class_info: TypeId,
    pub root: TreeId<Untyped>,
    pub class_tree: TreeId<Untyped>,
    pub template: TreeId<Untyped>,
    pub rhs: TreeId<Untyped>,
    pub body: TreeId<Typed>,
    pub constructor: DefaultConstructorPlan,
    pub methods: Vec<MethodPicklePlan>,
    pub local_values: Vec<LocalValueBinding>,
}

impl Fixture {
    pub fn new() -> Self {
        let source = SourceId::from_index(1);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let package = packages.enter(&mut store, SymbolOrigin::Source(source), &["example"])[0];
        let package_root = store.symbols.get(package.symbol).owner.unwrap();
        let class_name = Name::new(store.names.intern("Answer"), Namespace::Type);
        let class = source_symbol(
            &mut store,
            class_name,
            SymbolKind::Class,
            package.symbol,
            source,
            SymbolInfo::Missing,
        );
        let scope = store.scopes.alloc(Scope::new(Some(class)));
        store.scopes.get_mut(package.scope).enter(class_name, class);
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            class,
            prefix: definitions.no_prefix,
            parents: vec![definitions.object_type],
            declarations: scope,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(class_info));
        let constructor_name = TermName::new(store.names.intern("<init>"));
        let instance = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let constructor_type = store.types.alloc(Type::Method(MethodType {
            params: vec![],
            result: instance,
            kind: MethodKind::Plain,
        }));
        let constructor_symbol = source_symbol(
            &mut store,
            *constructor_name.as_name(),
            SymbolKind::Constructor,
            class,
            source,
            SymbolInfo::Complete(constructor_type),
        );
        let method_name = TermName::new(store.names.intern("value"));
        let method_symbol = source_symbol(
            &mut store,
            *method_name.as_name(),
            SymbolKind::Method,
            class,
            source,
            SymbolInfo::Complete(definitions.int),
        );
        store
            .scopes
            .get_mut(scope)
            .enter(*constructor_name.as_name(), constructor_symbol);
        store
            .scopes
            .get_mut(scope)
            .enter(*method_name.as_name(), method_symbol);
        let mut source_ast = AstArena::new();
        let int = store.types.get(definitions.int).reference_symbol().unwrap();
        let tpt = node(
            &mut source_ast,
            TreeKind::Ident(Ident {
                name: store.symbols.get(int).name,
                backquoted: false,
            }),
        );
        let rhs = node(
            &mut source_ast,
            TreeKind::Literal(Literal {
                value: Constant::Int(42),
            }),
        );
        let method_tree = node(
            &mut source_ast,
            TreeKind::DefDef(DefDef {
                name: method_name,
                type_params: vec![],
                value_param_clauses: vec![],
                source_param_clause_order: None,
                tpt,
                rhs: Some(rhs),
                metadata: Modifiers::default(),
            }),
        );
        let ctor_tpt = node(&mut source_ast, TreeKind::TypeTree(TypeTree));
        let constructor_tree = node(
            &mut source_ast,
            TreeKind::DefDef(DefDef {
                name: constructor_name,
                type_params: vec![],
                value_param_clauses: vec![],
                source_param_clause_order: None,
                tpt: ctor_tpt,
                rhs: None,
                metadata: Modifiers::default(),
            }),
        );
        let template = node(
            &mut source_ast,
            TreeKind::Template(Template {
                constructor: constructor_tree,
                parents: vec![],
                self_val: None,
                body: vec![method_tree],
                metadata: UntypedTemplateMetadata::default(),
            }),
        );
        let class_tree = node(
            &mut source_ast,
            TreeKind::TypeDef(TypeDef {
                name: TypeName::new(class_name.text()),
                rhs: template,
                metadata: Modifiers::default(),
                variance: None,
            }),
        );
        let package_name = node(
            &mut source_ast,
            TreeKind::Ident(Ident {
                name: store.symbols.get(package.symbol).name,
                backquoted: false,
            }),
        );
        let root = node(
            &mut source_ast,
            TreeKind::PackageDef(PackageDef {
                name: package_name,
                stats: vec![class_tree],
            }),
        );
        let mut index = SourceSemanticIndex::new();
        index
            .record_package_symbol(source, root, package.symbol)
            .unwrap();
        index.record_scope(package.symbol, package.scope).unwrap();
        index.record_scope(class, scope).unwrap();
        for (tree, symbol) in [
            (class_tree, class),
            (constructor_tree, constructor_symbol),
            (method_tree, method_symbol),
        ] {
            index.record_symbol(source, tree, symbol).unwrap();
        }
        let literal_type = store.types.alloc(Type::Constant(Constant::Int(42)));
        let mut typed_ast = AstArena::new();
        let body = TypedAstBuilder::new(&mut typed_ast, &store.types).literal(
            Constant::Int(42),
            literal_type,
            None,
        );
        let mut bindings = Bindings::default();
        bindings.0.insert((source, rhs), body);
        Self {
            store,
            definitions,
            source_ast,
            typed_ast,
            index,
            bindings,
            source,
            package: package.symbol,
            package_root,
            class,
            scope,
            class_info,
            root,
            class_tree,
            template,
            rhs,
            body,
            constructor: DefaultConstructorPlan {
                symbol: constructor_symbol,
                source_tree: constructor_tree,
                parameter_clauses: vec![],
            },
            methods: vec![MethodPicklePlan {
                symbol: method_symbol,
                source_tree: method_tree,
                parameter_clauses: vec![],
                body: Some(body),
            }],
            local_values: vec![],
        }
    }

    pub fn plan(&self) -> PicklePlan<'_> {
        PicklePlan {
            store: &self.store,
            definitions: &self.definitions,
            source_ast: &self.source_ast,
            typed_ast: &self.typed_ast,
            source_index: &self.index,
            typed_bindings: &self.bindings,
            source: SourceMetadata {
                id: self.source,
                name: "Answer.scala".into(),
            },
            source_root: self.root,
            class: self.class,
            class_tree: self.class_tree,
            constructor: self.constructor.clone(),
            methods: &self.methods,
            local_values: &self.local_values,
        }
    }

    pub fn snapshot(&self) -> String {
        format!(
            "{:?}{:?}{:?}{:?}{:?}",
            self.store, self.source_ast, self.typed_ast, self.index, self.bindings.0
        )
    }
}
