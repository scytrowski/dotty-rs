use dotty_core::ast::{Tree, TreeKind, TypedAst};
use dotty_core::ids::{SymbolId, TreeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{Constant, Type};

use crate::{
    InvalidTypeReason, JvmClassPlan, JvmCodegenError, JvmCompilationUnit, JvmMethodPlan, SymbolRole,
};

struct Fixture {
    store: SemanticStore,
    ast: TypedAst,
    class: SymbolId,
    method: SymbolId,
    parameter: SymbolId,
    body: TreeId<dotty_core::ast::Typed>,
    value_type: dotty_core::TypeId,
}

impl Fixture {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let value_type = store.types.alloc(Type::Constant(Constant::Int(0)));
        let class = alloc_symbol(&mut store, SymbolKind::Class, None, value_type);
        let method = alloc_symbol(&mut store, SymbolKind::Method, Some(class), value_type);
        let parameter = alloc_symbol(&mut store, SymbolKind::Parameter, Some(method), value_type);
        let mut ast = TypedAst::new();
        let body = ast.alloc(Tree {
            kind: TreeKind::Literal(dotty_core::ast::Literal {
                value: Constant::Int(42),
            }),
            position: None,
            ty: value_type,
        });
        Self {
            store,
            ast,
            class,
            method,
            parameter,
            body,
            value_type,
        }
    }

    fn unit(&self) -> JvmCompilationUnit<'_> {
        JvmCompilationUnit {
            store: &self.store,
            typed_ast: &self.ast,
            classes: vec![JvmClassPlan {
                class: self.class,
                methods: vec![JvmMethodPlan {
                    method: self.method,
                    parameters: vec![self.parameter],
                    body: Some(self.body),
                }],
            }],
        }
    }
}

fn alloc_symbol(
    store: &mut SemanticStore,
    kind: SymbolKind,
    owner: Option<SymbolId>,
    ty: dotty_core::TypeId,
) -> SymbolId {
    let name_id = store.names.intern("sharedName");
    store.symbols.alloc(Symbol {
        name: Name::new(name_id, Namespace::Term),
        owner,
        kind,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(ty),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    })
}

#[test]
fn validates_a_borrowed_typed_unit_with_canonical_identities() {
    let fixture = Fixture::new();

    assert_eq!(fixture.unit().validate(), Ok(()));
}

#[test]
fn rejects_unknown_class_method_and_parameter_ids() {
    let mut fixture = Fixture::new();
    let checkpoint = fixture.store.checkpoint();
    let stale_class = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Class,
        None,
        fixture.value_type,
    );
    fixture.store.rollback_to(checkpoint);
    let mut unit = fixture.unit();
    unit.classes[0].class = stale_class;
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnknownClass { class: stale_class })
    );

    let mut fixture = Fixture::new();
    let checkpoint = fixture.store.checkpoint();
    let stale_method = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Method,
        Some(fixture.class),
        fixture.value_type,
    );
    fixture.store.rollback_to(checkpoint);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].method = stale_method;
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnknownMethod {
            method: stale_method
        })
    );

    let mut fixture = Fixture::new();
    let checkpoint = fixture.store.checkpoint();
    let stale_parameter = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Parameter,
        Some(fixture.method),
        fixture.value_type,
    );
    fixture.store.rollback_to(checkpoint);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].parameters[0] = stale_parameter;
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnknownParameter {
            parameter: stale_parameter
        })
    );
}

#[test]
fn rejects_wrong_kinds_for_class_method_and_parameter_roles() {
    let mut fixture = Fixture::new();
    fixture.store.symbols.get_mut(fixture.class).kind = SymbolKind::Trait;
    assert_eq!(
        fixture.unit().validate(),
        Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: fixture.class,
            expected: SymbolRole::Class,
            found: SymbolKind::Trait,
        })
    );

    let mut fixture = Fixture::new();
    fixture.store.symbols.get_mut(fixture.method).kind = SymbolKind::Constructor;
    assert_eq!(
        fixture.unit().validate(),
        Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: fixture.method,
            expected: SymbolRole::Method,
            found: SymbolKind::Constructor,
        })
    );

    let mut fixture = Fixture::new();
    fixture.store.symbols.get_mut(fixture.parameter).kind = SymbolKind::Value;
    assert_eq!(
        fixture.unit().validate(),
        Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: fixture.parameter,
            expected: SymbolRole::Parameter,
            found: SymbolKind::Value,
        })
    );
}

#[test]
fn rejects_a_method_owned_by_a_different_class_even_when_names_match() {
    let mut fixture = Fixture::new();
    let other_class = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Class,
        None,
        fixture.value_type,
    );
    let same_named_method = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Method,
        Some(other_class),
        fixture.value_type,
    );
    assert_eq!(
        fixture.store.symbols.get(same_named_method).name,
        fixture.store.symbols.get(fixture.method).name
    );

    let mut unit = fixture.unit();
    unit.classes[0].methods[0].method = same_named_method;
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::WrongOwner {
            symbol: same_named_method,
            expected: fixture.class,
            actual: Some(other_class),
        })
    );
}

#[test]
fn rejects_parameter_owned_by_a_different_method() {
    let mut fixture = Fixture::new();
    let other_method = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Method,
        Some(fixture.class),
        fixture.value_type,
    );
    let wrong_parameter = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Parameter,
        Some(other_method),
        fixture.value_type,
    );
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].parameters = vec![wrong_parameter];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::WrongOwner {
            symbol: wrong_parameter,
            expected: fixture.method,
            actual: Some(other_method),
        })
    );
}

#[test]
fn rejects_duplicate_method_plans_and_missing_or_unknown_body_roots() {
    let fixture = Fixture::new();
    let mut unit = fixture.unit();
    let repeated = unit.classes[0].methods[0].clone();
    unit.classes[0].methods.push(repeated);
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::DuplicateMethodPlan {
            method: fixture.method
        })
    );

    let mut unit = fixture.unit();
    unit.classes[0].methods[0].body = None;
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::MissingTypedBody {
            method: fixture.method
        })
    );

    let mut unit = fixture.unit();
    let mut other_arena = TypedAst::new();
    other_arena.alloc(Tree {
        kind: TreeKind::Literal(dotty_core::ast::Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: fixture.value_type,
    });
    let invalid_body = other_arena.alloc(Tree {
        kind: TreeKind::Literal(dotty_core::ast::Literal {
            value: Constant::Int(2),
        }),
        position: None,
        ty: fixture.value_type,
    });
    unit.classes[0].methods[0].body = Some(invalid_body);
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnknownBodyRoot {
            method: fixture.method,
            tree: invalid_body,
        })
    );
}

#[test]
fn rejects_duplicate_class_and_parameter_plans() {
    let fixture = Fixture::new();
    let mut unit = fixture.unit();
    unit.classes.push(unit.classes[0].clone());
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::DuplicateClassPlan {
            class: fixture.class
        })
    );

    let fixture = Fixture::new();
    let mut unit = fixture.unit();
    unit.classes[0].methods[0]
        .parameters
        .push(fixture.parameter);
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::DuplicateParameter {
            parameter: fixture.parameter,
            method: fixture.method,
        })
    );
}

#[test]
fn rejects_missing_and_invalid_semantic_or_typed_tree_types() {
    let mut fixture = Fixture::new();
    fixture.store.symbols.get_mut(fixture.parameter).info = SymbolInfo::Missing;
    assert_eq!(
        fixture.unit().validate(),
        Err(JvmCodegenError::MissingSemanticInfo {
            symbol: fixture.parameter
        })
    );

    let mut fixture = Fixture::new();
    fixture.store.symbols.get_mut(fixture.parameter).info = SymbolInfo::Error;
    assert_eq!(
        fixture.unit().validate(),
        Err(JvmCodegenError::ErrorSemanticInfo {
            symbol: fixture.parameter
        })
    );

    let mut fixture = Fixture::new();
    let no_type = fixture.store.types.alloc(Type::NoType);
    fixture.store.symbols.get_mut(fixture.parameter).info = SymbolInfo::Complete(no_type);
    assert_eq!(
        fixture.unit().validate(),
        Err(JvmCodegenError::InvalidSemanticType {
            symbol: fixture.parameter,
            ty: no_type,
            reason: InvalidTypeReason::NoType,
        })
    );

    let mut fixture = Fixture::new();
    let error_message = fixture.store.names.intern("type error");
    let error_type = fixture
        .store
        .types
        .alloc(Type::Error(dotty_core::types::ErrorType {
            message: error_message,
        }));
    fixture.ast.get_mut(fixture.body).ty = error_type;
    assert_eq!(
        fixture.unit().validate(),
        Err(JvmCodegenError::InvalidTreeType {
            tree: fixture.body,
            ty: error_type,
            reason: InvalidTypeReason::Error,
        })
    );
}

#[test]
fn validation_does_not_mutate_the_caller_owned_stores_or_arena() {
    let fixture = Fixture::new();
    let checkpoint = fixture.store.checkpoint();
    let trees = fixture
        .ast
        .iter()
        .map(|(id, tree)| (id, tree.clone()))
        .collect::<Vec<_>>();
    let symbols = fixture.store.symbols.get(fixture.class).clone();

    assert_eq!(fixture.unit().validate(), Ok(()));

    assert_eq!(fixture.store.checkpoint(), checkpoint);
    assert_eq!(fixture.store.symbols.get(fixture.class), &symbols);
    assert_eq!(
        fixture
            .ast
            .iter()
            .map(|(id, tree)| (id, tree.clone()))
            .collect::<Vec<_>>(),
        trees
    );
}
