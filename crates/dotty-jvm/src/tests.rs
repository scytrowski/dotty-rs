use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_core::ast::{Tree, TreeKind, TypedAst};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId};
use dotty_core::names::{Name, Namespace, TermName};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{Constant, MethodKind, MethodParam, MethodType, PolyType, Type};

use crate::{
    InvalidTypeReason, JvmClassPlan, JvmCodegenError, JvmCompilationUnit, JvmMethodPlan,
    JvmReturnType, JvmType, JvmValueCategory, SymbolRole, UnsupportedJvmTypeKind,
    UnsupportedMethodShapeKind,
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

struct SignatureFixture {
    store: SemanticStore,
    definitions: Definitions,
    class: SymbolId,
    method: SymbolId,
    parameters: Vec<SymbolId>,
    plan: JvmMethodPlan,
}

impl SignatureFixture {
    fn new(parameter_count: usize) -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        Self::from_store(
            store,
            definitions,
            &vec![definitions.int; parameter_count],
            definitions.int,
            SymbolFlags::EMPTY,
            MethodKind::Plain,
        )
    }

    fn from_store(
        mut store: SemanticStore,
        definitions: Definitions,
        parameter_types: &[dotty_core::TypeId],
        result: dotty_core::TypeId,
        flags: SymbolFlags,
        method_kind: MethodKind,
    ) -> Self {
        let class = alloc_symbol(&mut store, SymbolKind::Class, None, definitions.object_type);
        let method = alloc_symbol(&mut store, SymbolKind::Method, Some(class), definitions.int);
        store.symbols.get_mut(method).flags = flags;

        let mut parameters = Vec::with_capacity(parameter_types.len());
        let mut method_params = Vec::with_capacity(parameter_types.len());
        for (index, ty) in parameter_types.iter().copied().enumerate() {
            let name = format!("p{index}");
            let name_id = store.names.intern(&name);
            let parameter = store.symbols.alloc(Symbol {
                name: Name::new(name_id, Namespace::Term),
                owner: Some(method),
                kind: SymbolKind::Parameter,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Complete(ty),
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            parameters.push(parameter);
            method_params.push(MethodParam {
                name: TermName::new(name_id),
                ty,
                erased: false,
                varargs: false,
            });
        }

        let method_type = store.types.alloc(Type::Method(MethodType {
            params: method_params,
            result,
            kind: method_kind,
        }));
        store
            .symbols
            .set_info(method, SymbolInfo::Complete(method_type));
        let plan = JvmMethodPlan {
            method,
            parameters: parameters.clone(),
            body: None,
        };
        Self {
            store,
            definitions,
            class,
            method,
            parameters,
            plan,
        }
    }

    fn descriptor(&self) -> Result<crate::JvmMethodDescriptor, JvmCodegenError> {
        self.plan
            .descriptor(&self.store, self.class, &self.definitions)
    }
}

#[test]
fn projects_primitive_method_signatures_to_valid_descriptors() {
    let mut value_store = SemanticStore::new();
    let value_definitions = Definitions::bootstrap(&mut value_store);
    let value_fixture = SignatureFixture::from_store(
        value_store,
        value_definitions,
        &[],
        value_definitions.int,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    assert_eq!(value_fixture.descriptor().unwrap().as_str(), "()I");

    let mut select_store = SemanticStore::new();
    let select_definitions = Definitions::bootstrap(&mut select_store);
    let select_fixture = SignatureFixture::from_store(
        select_store,
        select_definitions,
        &[
            select_definitions.boolean,
            select_definitions.int,
            select_definitions.int,
        ],
        select_definitions.int,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    let select = select_fixture.descriptor().unwrap();
    assert_eq!(select.as_str(), "(ZII)I");
    assert_eq!(
        select.parameters(),
        [JvmType::Boolean, JvmType::Int, JvmType::Int]
    );
    assert_eq!(select.parameters()[0].category(), JvmValueCategory::IntLike);
    assert_eq!(select.result(), JvmReturnType::Value(JvmType::Int));
    assert_eq!(
        select.parsed(),
        &MethodDescriptor {
            parameters: vec![FieldType::Boolean, FieldType::Int, FieldType::Int],
            return_type: Some(FieldType::Int),
        }
    );

    let mut noop_store = SemanticStore::new();
    let noop_definitions = Definitions::bootstrap(&mut noop_store);
    let noop_fixture = SignatureFixture::from_store(
        noop_store,
        noop_definitions,
        &[],
        noop_definitions.unit,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    let noop = noop_fixture.descriptor().unwrap();
    assert_eq!(noop.as_str(), "()V");
    assert_eq!(noop.result(), JvmReturnType::Void);
    assert_eq!(noop.result().category(), None);
    assert_eq!(noop.parsed().return_type, None);
}

#[test]
fn canonical_primitive_symbols_project_independently_of_type_id_allocation() {
    let mut ordinary_store = SemanticStore::new();
    let ordinary_definitions = Definitions::bootstrap(&mut ordinary_store);
    let ordinary = SignatureFixture::from_store(
        ordinary_store,
        ordinary_definitions,
        &[ordinary_definitions.int],
        ordinary_definitions.int,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    let ordinary_descriptor = ordinary.descriptor().unwrap();

    let mut shifted_store = SemanticStore::new();
    for _ in 0..9 {
        shifted_store.types.alloc(Type::Constant(Constant::Int(7)));
    }
    let shifted_definitions = Definitions::bootstrap(&mut shifted_store);
    let int_symbol = shifted_store
        .types
        .get(shifted_definitions.int)
        .reference_symbol()
        .unwrap();
    let equivalent_int = shifted_store
        .types
        .alloc(Type::type_ref(shifted_definitions.no_prefix, int_symbol));
    let shifted = SignatureFixture::from_store(
        shifted_store,
        shifted_definitions,
        &[equivalent_int],
        equivalent_int,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );

    assert_eq!(ordinary_descriptor.as_str(), "(I)I");
    assert_eq!(
        shifted.descriptor().unwrap().as_str(),
        ordinary_descriptor.as_str()
    );
}

#[test]
fn integer_and_boolean_constant_types_widen_without_mutating_semantic_store() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let int_literal = store.types.alloc(Type::Constant(Constant::Int(42)));
    let boolean_literal = store.types.alloc(Type::Constant(Constant::Boolean(true)));
    let fixture = SignatureFixture::from_store(
        store,
        definitions,
        &[int_literal],
        boolean_literal,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    let checkpoint = fixture.store.checkpoint();
    let original_parameter_type = fixture.store.types.get(int_literal).clone();
    let original_result_type = fixture.store.types.get(boolean_literal).clone();

    let signature = fixture.descriptor().unwrap();

    assert_eq!(signature.as_str(), "(I)Z");
    assert_eq!(fixture.store.checkpoint(), checkpoint);
    assert_eq!(
        fixture.store.types.get(int_literal),
        &original_parameter_type
    );
    assert_eq!(
        fixture.store.types.get(boolean_literal),
        &original_result_type
    );
}

#[test]
fn rejects_unit_parameters_polymorphic_methods_and_curried_results() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let unit_parameter = SignatureFixture::from_store(
        store,
        definitions,
        &[definitions.unit],
        definitions.int,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    assert_eq!(
        unit_parameter.descriptor(),
        Err(JvmCodegenError::UnitParameter {
            method: unit_parameter.method,
            parameter: unit_parameter.parameters[0],
            ty: unit_parameter.definitions.unit,
        })
    );

    let mut polymorphic = SignatureFixture::new(0);
    let poly = polymorphic.store.types.alloc(Type::Poly(PolyType {
        params: Vec::new(),
        result: polymorphic.definitions.int,
    }));
    polymorphic
        .store
        .symbols
        .set_info(polymorphic.method, SymbolInfo::Complete(poly));
    assert_eq!(
        polymorphic.descriptor(),
        Err(JvmCodegenError::UnsupportedMethodShape {
            method: polymorphic.method,
            ty: poly,
            kind: UnsupportedMethodShapeKind::Polymorphic,
        })
    );

    let mut curried = SignatureFixture::new(0);
    let inner = curried.store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: TermName::new(curried.store.names.intern("next")),
            ty: curried.definitions.boolean,
            erased: false,
            varargs: false,
        }],
        result: curried.definitions.int,
        kind: MethodKind::Plain,
    }));
    let outer = curried.store.types.alloc(Type::Method(MethodType {
        params: Vec::new(),
        result: inner,
        kind: MethodKind::Plain,
    }));
    curried
        .store
        .symbols
        .set_info(curried.method, SymbolInfo::Complete(outer));
    assert_eq!(
        curried.descriptor(),
        Err(JvmCodegenError::UnsupportedMethodShape {
            method: curried.method,
            ty: inner,
            kind: UnsupportedMethodShapeKind::NestedCurriedClause,
        })
    );
}

#[test]
fn rejects_alias_class_object_and_type_parameter_values_without_object_fallback() {
    let cases = [
        (SymbolKind::TypeAlias, UnsupportedJvmTypeKind::Alias),
        (SymbolKind::Class, UnsupportedJvmTypeKind::ReferenceClass),
        (SymbolKind::Object, UnsupportedJvmTypeKind::ScalaObject),
        (
            SymbolKind::TypeParameter,
            UnsupportedJvmTypeKind::TypeParameter,
        ),
    ];
    for (symbol_kind, expected_kind) in cases {
        let mut fixture = SignatureFixture::new(1);
        let semantic_target = alloc_symbol(
            &mut fixture.store,
            symbol_kind,
            None,
            fixture.definitions.int,
        );
        let reference_type = if symbol_kind == SymbolKind::Object {
            Type::term_ref(fixture.definitions.no_prefix, semantic_target)
        } else {
            Type::type_ref(fixture.definitions.no_prefix, semantic_target)
        };
        let reference = fixture.store.types.alloc(reference_type);
        fixture.store.symbols.get_mut(fixture.parameters[0]).info = SymbolInfo::Complete(reference);
        let method_type = fixture.store.types.alloc(Type::Method(MethodType {
            params: vec![MethodParam {
                name: TermName::new(fixture.store.names.intern("p0")),
                ty: reference,
                erased: false,
                varargs: false,
            }],
            result: fixture.definitions.int,
            kind: MethodKind::Plain,
        }));
        fixture
            .store
            .symbols
            .set_info(fixture.method, SymbolInfo::Complete(method_type));

        assert_eq!(
            fixture.descriptor(),
            Err(JvmCodegenError::UnsupportedJvmType {
                symbol: fixture.parameters[0],
                ty: reference,
                kind: expected_kind,
            })
        );
    }
}

#[test]
fn rejects_unresolved_method_parameter_info_shape_and_ownership_errors() {
    let mut missing = SignatureFixture::new(0);
    missing
        .store
        .symbols
        .set_info(missing.method, SymbolInfo::Missing);
    assert_eq!(
        missing.descriptor(),
        Err(JvmCodegenError::MissingSemanticInfo {
            symbol: missing.method
        })
    );

    let mut mismatch = SignatureFixture::new(1);
    mismatch.store.symbols.get_mut(mismatch.parameters[0]).info =
        SymbolInfo::Complete(mismatch.definitions.boolean);
    assert_eq!(
        mismatch.descriptor(),
        Err(JvmCodegenError::ParameterTypeMismatch {
            method: mismatch.method,
            parameter: mismatch.parameters[0],
            declared: mismatch.definitions.int,
            symbol_info: mismatch.definitions.boolean,
        })
    );

    let mut count_mismatch = SignatureFixture::new(1);
    count_mismatch.plan.parameters.clear();
    let method_type = count_mismatch.store.symbols.info(count_mismatch.method);
    let SymbolInfo::Complete(method_type) = method_type else {
        unreachable!()
    };
    assert_eq!(
        count_mismatch.descriptor(),
        Err(JvmCodegenError::UnsupportedMethodShape {
            method: count_mismatch.method,
            ty: *method_type,
            kind: UnsupportedMethodShapeKind::ParameterCountMismatch,
        })
    );

    let mut wrong_owner = SignatureFixture::new(0);
    let other_class = alloc_symbol(
        &mut wrong_owner.store,
        SymbolKind::Class,
        None,
        wrong_owner.definitions.object_type,
    );
    assert_eq!(
        wrong_owner
            .plan
            .descriptor(&wrong_owner.store, other_class, &wrong_owner.definitions),
        Err(JvmCodegenError::WrongOwner {
            symbol: wrong_owner.method,
            expected: other_class,
            actual: Some(wrong_owner.class),
        })
    );

    let mut wrong_kind = SignatureFixture::new(0);
    wrong_kind.store.symbols.get_mut(wrong_kind.method).kind = SymbolKind::Constructor;
    assert_eq!(
        wrong_kind.descriptor(),
        Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: wrong_kind.method,
            expected: SymbolRole::Method,
            found: SymbolKind::Constructor,
        })
    );
}

#[test]
fn rejects_duplicate_parameter_symbol_identities_in_signature_plans() {
    let mut fixture = SignatureFixture::new(2);
    fixture.plan.parameters[1] = fixture.plan.parameters[0];

    assert_eq!(
        fixture.descriptor(),
        Err(JvmCodegenError::DuplicateParameter {
            parameter: fixture.parameters[0],
            method: fixture.method,
        })
    );
}

#[test]
fn enforces_the_jvms_method_parameter_slot_limit_including_instance_receiver() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let maximum_static = SignatureFixture::from_store(
        store,
        definitions,
        &vec![definitions.boolean; 255],
        definitions.int,
        SymbolFlags::STATIC,
        MethodKind::Plain,
    );
    assert_eq!(maximum_static.descriptor().unwrap().parameters().len(), 255);

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let too_many_instance = SignatureFixture::from_store(
        store,
        definitions,
        &vec![definitions.int; 255],
        definitions.int,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    assert_eq!(
        too_many_instance.descriptor(),
        Err(JvmCodegenError::TooManyParameterSlots {
            method: too_many_instance.method,
            slots: 256,
        })
    );

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let maximum_instance = SignatureFixture::from_store(
        store,
        definitions,
        &vec![definitions.int; 254],
        definitions.int,
        SymbolFlags::EMPTY,
        MethodKind::Plain,
    );
    assert_eq!(
        maximum_instance.descriptor().unwrap().parameters().len(),
        254
    );
}
