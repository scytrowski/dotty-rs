use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{
    Apply, ApplyKind, Ident, If, Literal, Select, This, Tree, TreeKind, TypeApply, TypedAst,
};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId};
use dotty_core::names::{Name, Namespace, TermName};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{
    ClassInfo, Constant, MethodKind, MethodParam, MethodType, PolyType, TermRefTarget, Type,
};
use std::fs;
use std::process::Command;

use crate::{
    InvalidJvmClassInfoKind, InvalidTypeReason, JvmClassInitializationPlan, JvmClassPlan,
    JvmCodegenError, JvmCompilationUnit, JvmInvocationKind, JvmMethodPlan, JvmNameKind,
    JvmReturnType, JvmType, JvmValueCategory, SymbolRole, UnsupportedJvmCallShapeKind,
    UnsupportedJvmClassKind, UnsupportedJvmSymbolFlag, UnsupportedJvmTypeKind,
    UnsupportedMethodShapeKind, emit_class,
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
                initialization: JvmClassInitializationPlan::Unsupported,
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

struct ClassIdentityFixture {
    store: SemanticStore,
    class: SymbolId,
    declarations: dotty_core::ScopeId,
}

impl ClassIdentityFixture {
    fn new(package_segments: &[&str], class_name: &str) -> Self {
        let mut store = SemanticStore::new();
        let no_prefix = store.types.alloc(Type::NoPrefix);
        let root_name = store.names.intern("");
        let mut owner = store.symbols.alloc(Symbol {
            name: Name::new(root_name, Namespace::Term),
            owner: None,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        for segment in package_segments {
            let name = store.names.intern(segment);
            owner = store.symbols.alloc(Symbol {
                name: Name::new(name, Namespace::Term),
                owner: Some(owner),
                kind: SymbolKind::Package,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
        }

        let name = store.names.intern(class_name);
        let class = store.symbols.alloc(Symbol {
            name: Name::new(name, Namespace::Type),
            owner: Some(owner),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let declarations = store.scopes.alloc(Scope::new(Some(class)));
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: no_prefix,
            class,
            parents: Vec::new(),
            declarations,
            self_type: None,
        }));
        store
            .symbols
            .set_info(class, SymbolInfo::Complete(class_info));
        Self {
            store,
            class,
            declarations,
        }
    }

    fn identity(&self) -> Result<crate::JvmClassIdentity, JvmCodegenError> {
        JvmClassPlan {
            class: self.class,
            methods: Vec::new(),
            initialization: JvmClassInitializationPlan::Unsupported,
        }
        .identity(&self.store)
    }
}

fn alloc_symbol_named(
    store: &mut SemanticStore,
    name: &str,
    namespace: Namespace,
    owner: Option<SymbolId>,
    kind: SymbolKind,
    flags: SymbolFlags,
) -> SymbolId {
    let name = store.names.intern(name);
    store.symbols.alloc(Symbol {
        name: Name::new(name, namespace),
        owner,
        kind,
        flags,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    })
}

fn prepare_signature_class(fixture: &mut SignatureFixture, class_name: &str) -> SymbolId {
    let root = alloc_symbol_named(
        &mut fixture.store,
        "",
        Namespace::Term,
        None,
        SymbolKind::Package,
        SymbolFlags::EMPTY,
    );
    let class_name = fixture.store.names.intern(class_name);
    {
        let class = fixture.store.symbols.get_mut(fixture.class);
        class.name = Name::new(class_name, Namespace::Type);
        class.owner = Some(root);
    }
    let declarations = fixture.store.scopes.alloc(Scope::new(Some(fixture.class)));
    let class_info = fixture.store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix: fixture.definitions.no_prefix,
        class: fixture.class,
        parents: Vec::new(),
        declarations,
        self_type: None,
    }));
    fixture
        .store
        .symbols
        .set_info(fixture.class, SymbolInfo::Complete(class_info));
    root
}

#[test]
fn derives_default_and_package_owned_class_internal_names() {
    let default_package = ClassIdentityFixture::new(&[], "Answer");
    let default_identity = default_package.identity().unwrap();
    assert_eq!(default_identity.symbol(), default_package.class);
    assert_eq!(default_identity.internal_name(), "Answer");
    assert_eq!(default_identity.access_flags().0, 0x0021);

    let packaged = ClassIdentityFixture::new(&["example", "api"], "Answer");
    let package_identity = packaged.identity().unwrap();
    assert_eq!(package_identity.internal_name(), "example/api/Answer");

    let mut finalized = ClassIdentityFixture::new(&[], "FinalAnswer");
    finalized.store.symbols.get_mut(finalized.class).flags = SymbolFlags::FINAL;
    assert_eq!(finalized.identity().unwrap().access_flags().0, 0x0031);
}

#[test]
fn class_identity_is_deterministic_after_unrelated_symbol_allocations() {
    let mut ordinary = ClassIdentityFixture::new(&["example"], "Answer");
    let expected = ordinary.identity().unwrap();
    for index in 0..10 {
        let name = format!("unused{index}");
        alloc_symbol_named(
            &mut ordinary.store,
            &name,
            Namespace::Term,
            None,
            SymbolKind::Value,
            SymbolFlags::EMPTY,
        );
    }
    assert_eq!(ordinary.identity().unwrap(), expected);
}

#[test]
fn rejects_nested_and_invalid_package_owner_chains() {
    let mut nested = ClassIdentityFixture::new(&[], "Outer");
    let inner = alloc_symbol_named(
        &mut nested.store,
        "Inner",
        Namespace::Type,
        Some(nested.class),
        SymbolKind::Class,
        SymbolFlags::EMPTY,
    );
    let declarations = nested.store.scopes.alloc(Scope::new(Some(inner)));
    let prefix = nested.store.types.alloc(Type::NoPrefix);
    let class_info = nested.store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix,
        class: inner,
        parents: Vec::new(),
        declarations,
        self_type: None,
    }));
    nested
        .store
        .symbols
        .set_info(inner, SymbolInfo::Complete(class_info));
    assert_eq!(
        JvmClassPlan {
            class: inner,
            methods: Vec::new(),
            initialization: JvmClassInitializationPlan::Unsupported,
        }
        .identity(&nested.store),
        Err(JvmCodegenError::UnsupportedOwnerKind {
            class: inner,
            owner: nested.class,
            found: SymbolKind::Class,
        })
    );

    let mut ownerless = ClassIdentityFixture::new(&[], "Ownerless");
    ownerless.store.symbols.get_mut(ownerless.class).owner = None;
    assert_eq!(
        ownerless.identity(),
        Err(JvmCodegenError::InvalidPackageOwnerChain {
            class: ownerless.class,
            owner: ownerless.class,
        })
    );
}

#[test]
fn rejects_traits_objects_and_module_classes_as_emittable_classes() {
    for kind in [
        SymbolKind::Trait,
        SymbolKind::Object,
        SymbolKind::ModuleClass,
    ] {
        let mut fixture = ClassIdentityFixture::new(&[], "NotAClass");
        fixture.store.symbols.get_mut(fixture.class).kind = kind;
        assert_eq!(
            fixture.identity(),
            Err(JvmCodegenError::UnsupportedSymbolKind {
                symbol: fixture.class,
                expected: SymbolRole::Class,
                found: kind,
            })
        );
    }
}

#[test]
fn rejects_unsupported_class_shapes_and_generic_declarations() {
    for (flags, expected) in [
        (SymbolFlags::ABSTRACT, UnsupportedJvmClassKind::Abstract),
        (SymbolFlags::ENUM, UnsupportedJvmClassKind::Enum),
        (SymbolFlags::SYNTHETIC, UnsupportedJvmClassKind::Synthetic),
        (SymbolFlags::CASE, UnsupportedJvmClassKind::Case),
        (SymbolFlags::SEALED, UnsupportedJvmClassKind::Sealed),
    ] {
        let mut fixture = ClassIdentityFixture::new(&[], "Unsupported");
        fixture.store.symbols.get_mut(fixture.class).flags = flags;
        assert_eq!(
            fixture.identity(),
            Err(JvmCodegenError::UnsupportedJvmClass {
                class: fixture.class,
                kind: expected,
            })
        );
    }

    let mut generic = ClassIdentityFixture::new(&[], "Generic");
    let parameter = alloc_symbol_named(
        &mut generic.store,
        "A",
        Namespace::Type,
        Some(generic.class),
        SymbolKind::TypeParameter,
        SymbolFlags::EMPTY,
    );
    let name = generic.store.symbols.get(parameter).name;
    generic
        .store
        .scopes
        .get_mut(generic.declarations)
        .enter(name, parameter);
    assert_eq!(
        generic.identity(),
        Err(JvmCodegenError::UnsupportedJvmClass {
            class: generic.class,
            kind: UnsupportedJvmClassKind::Generic,
        })
    );

    let mut nonpublic = ClassIdentityFixture::new(&[], "Hidden");
    nonpublic.store.symbols.get_mut(nonpublic.class).visibility = Visibility::Private;
    assert_eq!(
        nonpublic.identity(),
        Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: nonpublic.class,
            visibility: Visibility::Private,
        })
    );
}

#[test]
fn rejects_invalid_class_and_package_segment_names() {
    for invalid in [
        "",
        "_",
        "bad/name",
        "bad.name",
        "bad;name",
        "Outer$Inner",
        "+",
    ] {
        let mut fixture = ClassIdentityFixture::new(&[], "Valid");
        let name = fixture.store.names.intern(invalid);
        fixture.store.symbols.get_mut(fixture.class).name = Name::new(name, Namespace::Type);
        assert_eq!(
            fixture.identity(),
            Err(JvmCodegenError::InvalidJvmName {
                symbol: fixture.class,
                kind: JvmNameKind::Class,
                text: invalid.to_owned(),
            })
        );
    }

    let mut fixture = ClassIdentityFixture::new(&["bad.segment"], "Valid");
    let invalid_package = fixture
        .store
        .symbols
        .get(fixture.class)
        .owner
        .expect("class has package owner");
    let bad_name = fixture.store.names.intern("bad.segment");
    fixture.store.symbols.get_mut(invalid_package).name = Name::new(bad_name, Namespace::Term);
    assert_eq!(
        fixture.identity(),
        Err(JvmCodegenError::InvalidJvmName {
            symbol: invalid_package,
            kind: JvmNameKind::PackageSegment,
            text: "bad.segment".to_owned(),
        })
    );
}

#[test]
fn rejects_malformed_class_info_instead_of_guessing_class_shape() {
    let mut missing = ClassIdentityFixture::new(&[], "MissingInfo");
    missing
        .store
        .symbols
        .set_info(missing.class, SymbolInfo::Missing);
    assert_eq!(
        missing.identity(),
        Err(JvmCodegenError::MissingSemanticInfo {
            symbol: missing.class,
        })
    );

    let mut fixture = ClassIdentityFixture::new(&[], "Broken");
    let not_class_info = fixture.store.types.alloc(Type::NoPrefix);
    fixture
        .store
        .symbols
        .set_info(fixture.class, SymbolInfo::Complete(not_class_info));
    assert_eq!(
        fixture.identity(),
        Err(JvmCodegenError::InvalidJvmClassInfo {
            class: fixture.class,
            ty: Some(not_class_info),
            kind: InvalidJvmClassInfoKind::NotClassInfo,
        })
    );
}

#[test]
fn method_metadata_preserves_parameter_identity_order_and_instance_slots() {
    let mut fixture = SignatureFixture::new(3);
    prepare_signature_class(&mut fixture, "Answer");
    let method_name = fixture.store.names.intern("select");
    fixture.store.symbols.get_mut(fixture.method).name = Name::new(method_name, Namespace::Term);
    let method_type = fixture.store.types.alloc(Type::Method(MethodType {
        params: vec![
            MethodParam {
                name: TermName::new(fixture.store.names.intern("first")),
                ty: fixture.definitions.boolean,
                erased: false,
                varargs: false,
            },
            MethodParam {
                name: TermName::new(fixture.store.names.intern("second")),
                ty: fixture.definitions.int,
                erased: false,
                varargs: false,
            },
            MethodParam {
                name: TermName::new(fixture.store.names.intern("third")),
                ty: fixture.definitions.int,
                erased: false,
                varargs: false,
            },
        ],
        result: fixture.definitions.int,
        kind: MethodKind::Plain,
    }));
    fixture
        .store
        .symbols
        .set_info(fixture.method, SymbolInfo::Complete(method_type));
    for (parameter, ty) in fixture.parameters.iter().copied().zip([
        fixture.definitions.boolean,
        fixture.definitions.int,
        fixture.definitions.int,
    ]) {
        fixture
            .store
            .symbols
            .set_info(parameter, SymbolInfo::Complete(ty));
    }

    let class_plan = JvmClassPlan {
        class: fixture.class,
        methods: vec![fixture.plan.clone()],
        initialization: JvmClassInitializationPlan::Unsupported,
    };
    let metadata = class_plan
        .metadata(&fixture.store, &fixture.definitions)
        .unwrap();
    assert_eq!(metadata.identity().internal_name(), "Answer");
    assert_eq!(metadata.methods().len(), 1);
    let method = &metadata.methods()[0];
    assert_eq!(method.symbol(), fixture.method);
    assert_eq!(method.name(), "select");
    assert_eq!(method.descriptor().as_str(), "(ZII)I");
    assert_eq!(method.access_flags().0, 0x0001);
    assert_eq!(
        method
            .parameters()
            .iter()
            .map(|parameter| (parameter.symbol(), parameter.slot(), parameter.width()))
            .collect::<Vec<_>>(),
        vec![
            (fixture.parameters[0], 1, 1),
            (fixture.parameters[1], 2, 1),
            (fixture.parameters[2], 3, 1),
        ]
    );
    assert_eq!(
        method
            .parameters()
            .iter()
            .map(|parameter| parameter.ty())
            .collect::<Vec<_>>(),
        vec![JvmType::Boolean, JvmType::Int, JvmType::Int]
    );
    for index in 0..7 {
        let name = format!("unrelated{index}");
        alloc_symbol_named(
            &mut fixture.store,
            &name,
            Namespace::Term,
            None,
            SymbolKind::Value,
            SymbolFlags::EMPTY,
        );
    }
    assert_eq!(
        class_plan
            .metadata(&fixture.store, &fixture.definitions)
            .unwrap(),
        metadata
    );
}

#[test]
fn rejects_non_simple_method_names_and_unsupported_instance_method_flags() {
    let mut invalid_name = SignatureFixture::new(0);
    prepare_signature_class(&mut invalid_name, "Answer");
    let operator = invalid_name.store.names.intern("$plus");
    invalid_name.store.symbols.get_mut(invalid_name.method).name =
        Name::new(operator, Namespace::Term);
    assert_eq!(
        JvmClassPlan {
            class: invalid_name.class,
            methods: vec![invalid_name.plan.clone()],
            initialization: JvmClassInitializationPlan::Unsupported,
        }
        .metadata(&invalid_name.store, &invalid_name.definitions),
        Err(JvmCodegenError::InvalidJvmName {
            symbol: invalid_name.method,
            kind: JvmNameKind::Method,
            text: "$plus".to_owned(),
        })
    );

    for (flags, unsupported) in [
        (SymbolFlags::STATIC, crate::UnsupportedJvmSymbolFlag::Static),
        (
            SymbolFlags::SYNTHETIC,
            crate::UnsupportedJvmSymbolFlag::Synthetic,
        ),
        (
            SymbolFlags::ABSTRACT,
            crate::UnsupportedJvmSymbolFlag::Abstract,
        ),
    ] {
        let mut fixture = SignatureFixture::new(0);
        prepare_signature_class(&mut fixture, "Answer");
        fixture.store.symbols.get_mut(fixture.method).flags = flags;
        assert_eq!(
            JvmClassPlan {
                class: fixture.class,
                methods: vec![fixture.plan.clone()],
                initialization: JvmClassInitializationPlan::Unsupported,
            }
            .metadata(&fixture.store, &fixture.definitions),
            Err(JvmCodegenError::UnsupportedJvmSymbolFlag {
                symbol: fixture.method,
                flag: unsupported,
            })
        );
    }

    let mut nonpublic = SignatureFixture::new(0);
    prepare_signature_class(&mut nonpublic, "Answer");
    nonpublic.store.symbols.get_mut(nonpublic.method).visibility = Visibility::Private;
    assert_eq!(
        JvmClassPlan {
            class: nonpublic.class,
            methods: vec![nonpublic.plan.clone()],
            initialization: JvmClassInitializationPlan::Unsupported,
        }
        .metadata(&nonpublic.store, &nonpublic.definitions),
        Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: nonpublic.method,
            visibility: Visibility::Private,
        })
    );

    let mut final_method = SignatureFixture::new(0);
    prepare_signature_class(&mut final_method, "Answer");
    final_method
        .store
        .symbols
        .get_mut(final_method.method)
        .flags = SymbolFlags::FINAL;
    let final_metadata = JvmClassPlan {
        class: final_method.class,
        methods: vec![final_method.plan.clone()],
        initialization: JvmClassInitializationPlan::Unsupported,
    }
    .metadata(&final_method.store, &final_method.definitions)
    .unwrap();
    assert_eq!(final_metadata.methods()[0].access_flags().0, 0x0011);
}

#[test]
fn same_named_method_from_a_different_owner_cannot_replace_the_planned_symbol() {
    let mut fixture = SignatureFixture::new(0);
    let root = prepare_signature_class(&mut fixture, "Answer");
    let other_class = alloc_symbol_named(
        &mut fixture.store,
        "Other",
        Namespace::Type,
        Some(root),
        SymbolKind::Class,
        SymbolFlags::EMPTY,
    );
    let class_scope = fixture.store.scopes.alloc(Scope::new(Some(other_class)));
    let class_info = fixture.store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix: fixture.definitions.no_prefix,
        class: other_class,
        parents: Vec::new(),
        declarations: class_scope,
        self_type: None,
    }));
    fixture
        .store
        .symbols
        .set_info(other_class, SymbolInfo::Complete(class_info));
    let mut other_method_symbol = fixture.store.symbols.get(fixture.method).clone();
    other_method_symbol.owner = Some(other_class);
    let other_method = fixture.store.symbols.alloc(other_method_symbol);
    let other_plan = JvmMethodPlan {
        method: other_method,
        parameters: Vec::new(),
        body: None,
    };

    assert_eq!(
        JvmClassPlan {
            class: fixture.class,
            methods: vec![other_plan],
            initialization: JvmClassInitializationPlan::Unsupported,
        }
        .metadata(&fixture.store, &fixture.definitions),
        Err(JvmCodegenError::WrongOwner {
            symbol: other_method,
            expected: fixture.class,
            actual: Some(other_class),
        })
    );
}

#[test]
fn rejects_duplicate_emitted_method_names_and_descriptors() {
    let mut fixture = SignatureFixture::new(0);
    prepare_signature_class(&mut fixture, "Answer");
    let duplicate = fixture
        .store
        .symbols
        .alloc(fixture.store.symbols.get(fixture.method).clone());
    let duplicate_plan = JvmMethodPlan {
        method: duplicate,
        parameters: Vec::new(),
        body: None,
    };

    assert_eq!(
        JvmClassPlan {
            class: fixture.class,
            methods: vec![fixture.plan.clone(), duplicate_plan],
            initialization: JvmClassInitializationPlan::Unsupported,
        }
        .metadata(&fixture.store, &fixture.definitions),
        Err(JvmCodegenError::DuplicateJvmMethodIdentity {
            class: fixture.class,
            method: duplicate,
            previous: fixture.method,
            name: "sharedName".to_owned(),
            descriptor: "()I".to_owned(),
        })
    );
}

#[test]
fn compilation_unit_rejects_distinct_symbols_with_the_same_jvm_class_name() {
    let mut fixture = SignatureFixture::new(0);
    prepare_signature_class(&mut fixture, "Answer");
    let duplicate = fixture
        .store
        .symbols
        .alloc(fixture.store.symbols.get(fixture.class).clone());
    let duplicate_scope = fixture.store.scopes.alloc(Scope::new(Some(duplicate)));
    let duplicate_info = fixture.store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix: fixture.definitions.no_prefix,
        class: duplicate,
        parents: Vec::new(),
        declarations: duplicate_scope,
        self_type: None,
    }));
    fixture
        .store
        .symbols
        .set_info(duplicate, SymbolInfo::Complete(duplicate_info));
    let ast = TypedAst::new();
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &ast,
        classes: vec![
            JvmClassPlan {
                class: fixture.class,
                methods: Vec::new(),
                initialization: JvmClassInitializationPlan::Unsupported,
            },
            JvmClassPlan {
                class: duplicate,
                methods: Vec::new(),
                initialization: JvmClassInitializationPlan::Unsupported,
            },
        ],
    };

    assert_eq!(
        unit.metadata(&fixture.definitions),
        Err(JvmCodegenError::DuplicateJvmClassIdentity {
            class: duplicate,
            previous: fixture.class,
            internal_name: "Answer".to_owned(),
        })
    );
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

struct EmissionFixture {
    store: SemanticStore,
    ast: TypedAst,
    definitions: Definitions,
    class: SymbolId,
    methods: Vec<SymbolId>,
    parameters: Vec<Vec<SymbolId>>,
    bodies: Vec<TreeId<dotty_core::ast::Typed>>,
}

impl EmissionFixture {
    fn new(allocation_noise: bool) -> Self {
        let mut identity = ClassIdentityFixture::new(&["example"], "Answer");
        if allocation_noise {
            for value in 0..7 {
                identity
                    .store
                    .types
                    .alloc(Type::Constant(Constant::Int(value)));
            }
        }
        let definitions = Definitions::bootstrap(&mut identity.store);
        let original = match identity.store.symbols.info(identity.class) {
            SymbolInfo::Complete(ty) => match identity.store.types.get(*ty) {
                Type::ClassInfo(info) => info.clone(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let class_type = identity.store.types.alloc(Type::ClassInfo(ClassInfo {
            parents: vec![definitions.object_type],
            ..original
        }));
        identity
            .store
            .symbols
            .set_info(identity.class, SymbolInfo::Complete(class_type));

        let mut ast = TypedAst::new();
        let value_body = ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(42),
            }),
            position: None,
            ty: definitions.int,
        });
        let (value, value_params) = add_emission_method(
            &mut identity.store,
            identity.class,
            identity.declarations,
            "value",
            &[],
            definitions.int,
        );

        let (select, select_params) = add_emission_method(
            &mut identity.store,
            identity.class,
            identity.declarations,
            "select",
            &[
                ("flag", definitions.boolean),
                ("left", definitions.int),
                ("right", definitions.int),
            ],
            definitions.int,
        );
        let mut ident = |store: &mut SemanticStore, parameter: SymbolId, _value_type| {
            let name = store.symbols.get(parameter).name;
            let reference_type = store.types.alloc(Type::TermRef {
                prefix: definitions.no_prefix,
                target: TermRefTarget::Symbol(parameter),
            });
            ast.alloc(Tree {
                kind: TreeKind::Ident(Ident {
                    name,
                    backquoted: false,
                }),
                position: None,
                ty: reference_type,
            })
        };
        let condition = ident(&mut identity.store, select_params[0], definitions.boolean);
        let left = ident(&mut identity.store, select_params[1], definitions.int);
        let right = ident(&mut identity.store, select_params[2], definitions.int);
        let select_body = ast.alloc(Tree {
            kind: TreeKind::If(If {
                cond: condition,
                then_branch: left,
                else_branch: right,
            }),
            position: None,
            ty: definitions.int,
        });

        Self {
            store: identity.store,
            ast,
            definitions,
            class: identity.class,
            methods: vec![value, select],
            parameters: vec![value_params, select_params],
            bodies: Vec::new(),
        }
        .with_bodies(vec![value_body, select_body])
    }

    fn with_bodies(mut self, bodies: Vec<TreeId<dotty_core::ast::Typed>>) -> Self {
        self.bodies = bodies;
        self
    }

    fn unit(&self) -> JvmCompilationUnit<'_> {
        let methods = self
            .methods
            .iter()
            .copied()
            .zip(self.parameters.iter())
            .zip(self.bodies.iter().copied())
            .map(|((method, parameters), body)| JvmMethodPlan {
                method,
                parameters: parameters.clone(),
                body: Some(body),
            })
            .collect();
        JvmCompilationUnit {
            store: &self.store,
            typed_ast: &self.ast,
            classes: vec![JvmClassPlan {
                class: self.class,
                methods,
                initialization: JvmClassInitializationPlan::ImplicitNoArg,
            }],
        }
    }

    fn emit(&self) -> Result<crate::GeneratedClass, JvmCodegenError> {
        emit_class(&self.unit(), self.class, &self.definitions)
    }
}

fn add_emission_parameter(
    store: &mut SemanticStore,
    method: SymbolId,
    text: &str,
    ty: dotty_core::TypeId,
) -> SymbolId {
    let name = store.names.intern(text);
    store.symbols.alloc(Symbol {
        name: Name::new(name, Namespace::Term),
        owner: Some(method),
        kind: SymbolKind::Parameter,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(ty),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    })
}

fn add_emission_method(
    store: &mut SemanticStore,
    class: SymbolId,
    declarations: dotty_core::ScopeId,
    text: &str,
    parameters: &[(&str, dotty_core::TypeId)],
    result: dotty_core::TypeId,
) -> (SymbolId, Vec<SymbolId>) {
    let name_id = store.names.intern(text);
    let method = store.symbols.alloc(Symbol {
        name: Name::new(name_id, Namespace::Term),
        owner: Some(class),
        kind: SymbolKind::Method,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let mut parameter_ids = Vec::with_capacity(parameters.len());
    let method_params = parameters
        .iter()
        .map(|(name, ty)| {
            let id = store.names.intern(name);
            parameter_ids.push(add_emission_parameter(store, method, name, *ty));
            MethodParam {
                name: TermName::new(id),
                ty: *ty,
                erased: false,
                varargs: false,
            }
        })
        .collect();
    let method_type = store.types.alloc(Type::Method(MethodType {
        params: method_params,
        result,
        kind: MethodKind::Plain,
    }));
    store
        .symbols
        .set_info(method, SymbolInfo::Complete(method_type));
    let symbol_name = store.symbols.get(method).name;
    store
        .scopes
        .get_mut(declarations)
        .enter(symbol_name, method);
    (method, parameter_ids)
}

fn add_call_target_method(
    fixture: &mut EmissionFixture,
    name: &str,
    parameters: &[(&str, dotty_core::TypeId)],
    result: dotty_core::TypeId,
) -> SymbolId {
    let class_type = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => *ty,
        _ => panic!("emission fixture class info should be complete"),
    };
    let declarations = match fixture.store.types.get(class_type) {
        Type::ClassInfo(info) => info.declarations,
        _ => panic!("emission fixture class type should be ClassInfo"),
    };
    let (method, parameter_ids) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        name,
        parameters,
        result,
    );
    let literal = if result == fixture.definitions.unit {
        Constant::Unit
    } else if result == fixture.definitions.boolean {
        Constant::Boolean(true)
    } else {
        Constant::Int(0)
    };
    let body = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal { value: literal }),
        position: None,
        ty: result,
    });
    fixture.methods.push(method);
    fixture.parameters.push(parameter_ids);
    fixture.bodies.push(body);
    method
}

fn add_unplanned_call_target_method(
    fixture: &mut EmissionFixture,
    name: &str,
    parameters: &[(&str, dotty_core::TypeId)],
    result: dotty_core::TypeId,
) -> SymbolId {
    let class_type = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => *ty,
        _ => panic!("emission fixture class info should be complete"),
    };
    let declarations = match fixture.store.types.get(class_type) {
        Type::ClassInfo(info) => info.declarations,
        _ => panic!("emission fixture class type should be ClassInfo"),
    };
    add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        name,
        parameters,
        result,
    )
    .0
}

fn typed_method_ident(
    fixture: &mut EmissionFixture,
    method: SymbolId,
) -> TreeId<dotty_core::ast::Typed> {
    typed_method_ident_with_prefix(fixture, method, fixture.definitions.no_prefix)
}

fn typed_method_ident_with_prefix(
    fixture: &mut EmissionFixture,
    method: SymbolId,
    prefix: dotty_core::TypeId,
) -> TreeId<dotty_core::ast::Typed> {
    let name = fixture.store.symbols.get(method).name;
    let ty = fixture.store.types.alloc(Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(method),
    });
    fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name,
            backquoted: false,
        }),
        position: None,
        ty,
    })
}

fn typed_call(
    fixture: &mut EmissionFixture,
    function: TreeId<dotty_core::ast::Typed>,
    args: Vec<TreeId<dotty_core::ast::Typed>>,
    kind: ApplyKind,
    ty: dotty_core::TypeId,
) -> TreeId<dotty_core::ast::Typed> {
    fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args,
            kind,
        }),
        position: None,
        ty,
    })
}

#[test]
fn resolves_ident_this_calls_and_supported_zero_argument_results_by_symbol() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let unit_type = fixture.definitions.unit;
    let int_argument = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(7),
        }),
        position: None,
        ty: int_type,
    });
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let ident_function = typed_method_ident(&mut fixture, identity);
    let ident_call = typed_call(
        &mut fixture,
        ident_function,
        vec![int_argument],
        ApplyKind::Regular,
        int_type,
    );

    let this_type = fixture.store.types.alloc(Type::ThisType {
        class: fixture.class,
    });
    let this_tree = fixture.ast.alloc(Tree {
        kind: TreeKind::This(This { qual: None }),
        position: None,
        ty: this_type,
    });
    let identity_type = fixture.store.types.alloc(Type::TermRef {
        prefix: this_type,
        target: TermRefTarget::Symbol(identity),
    });
    let select_function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: this_tree,
            name: fixture.store.symbols.get(identity).name,
            backquoted: false,
        }),
        position: None,
        ty: identity_type,
    });
    let this_call = typed_call(
        &mut fixture,
        select_function,
        vec![int_argument],
        ApplyKind::Regular,
        int_type,
    );

    let checkpoint = fixture.store.checkpoint();
    let unit = fixture.unit();
    let ident_target = unit
        .resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            ident_call,
            &fixture.definitions,
        )
        .unwrap();
    let this_target = unit
        .resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            this_call,
            &fixture.definitions,
        )
        .unwrap();
    assert_eq!(fixture.store.checkpoint(), checkpoint);
    assert_eq!(ident_target, this_target);
    assert_eq!(ident_target.owner_internal_name(), "example/Answer");
    assert_eq!(ident_target.method_name(), "identity");
    assert_eq!(ident_target.method_symbol(), identity);
    assert_eq!(ident_target.descriptor().as_str(), "(I)I");
    assert_eq!(ident_target.result(), JvmReturnType::Value(JvmType::Int));
    assert_eq!(ident_target.invocation_kind(), JvmInvocationKind::Virtual);

    let noop = add_call_target_method(&mut fixture, "noop", &[], unit_type);
    let noop_function = typed_method_ident(&mut fixture, noop);
    let noop_call = typed_call(
        &mut fixture,
        noop_function,
        Vec::new(),
        ApplyKind::Regular,
        unit_type,
    );
    let value_method = fixture.methods[0];
    let value_function = typed_method_ident(&mut fixture, value_method);
    let value_call = typed_call(
        &mut fixture,
        value_function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    let checkpoint = fixture.store.checkpoint();
    let unit = fixture.unit();
    let noop_target = unit
        .resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            noop_call,
            &fixture.definitions,
        )
        .unwrap();
    let value_target = unit
        .resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            value_call,
            &fixture.definitions,
        )
        .unwrap();
    assert_eq!(noop_target.descriptor().as_str(), "()V");
    assert_eq!(noop_target.result(), JvmReturnType::Void);
    assert_eq!(value_target.descriptor().as_str(), "()I");
    assert_eq!(fixture.store.checkpoint(), checkpoint);
}

#[test]
fn overload_targets_follow_the_typed_symbol_not_the_printed_name() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let boolean_type = fixture.definitions.boolean;
    let int_overload =
        add_call_target_method(&mut fixture, "choose", &[("value", int_type)], int_type);
    let boolean_overload = add_call_target_method(
        &mut fixture,
        "choose",
        &[("value", boolean_type)],
        boolean_type,
    );
    let int_function = typed_method_ident(&mut fixture, int_overload);
    let bool_function = typed_method_ident(&mut fixture, boolean_overload);
    let int_arg = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int_type,
    });
    let bool_arg = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean_type,
    });
    let int_call = typed_call(
        &mut fixture,
        int_function,
        vec![int_arg],
        ApplyKind::Regular,
        int_type,
    );
    let bool_call = typed_call(
        &mut fixture,
        bool_function,
        vec![bool_arg],
        ApplyKind::Regular,
        boolean_type,
    );
    let unit = fixture.unit();
    let int_target = unit
        .resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            int_call,
            &fixture.definitions,
        )
        .unwrap();
    let bool_target = unit
        .resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            bool_call,
            &fixture.definitions,
        )
        .unwrap();
    assert_eq!(int_target.method_symbol(), int_overload);
    assert_eq!(int_target.descriptor().as_str(), "(I)I");
    assert_eq!(bool_target.method_symbol(), boolean_overload);
    assert_eq!(bool_target.descriptor().as_str(), "(Z)Z");

    let duplicate =
        add_call_target_method(&mut fixture, "choose", &[("another", int_type)], int_type);
    let duplicate_function = typed_method_ident(&mut fixture, duplicate);
    let duplicate_call = typed_call(
        &mut fixture,
        duplicate_function,
        vec![int_arg],
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            duplicate_call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::DuplicateJvmMethodIdentity {
            class: fixture.class,
            method: duplicate,
            previous: int_overload,
            name: "choose".to_owned(),
            descriptor: "(I)I".to_owned(),
        })
    );
}

#[test]
fn rejects_unsupported_and_unplanned_call_targets_with_typed_errors() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let function = typed_method_ident(&mut fixture, identity);
    let call = typed_call(
        &mut fixture,
        function,
        Vec::new(),
        ApplyKind::Using,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            call,
            &fixture.definitions
        ),
        Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: call,
            kind: UnsupportedJvmCallShapeKind::UsingClause,
        })
    );

    let function = typed_method_ident(&mut fixture, identity);
    let type_application = fixture.ast.alloc(Tree {
        kind: TreeKind::TypeApply(TypeApply {
            function,
            args: Vec::new(),
        }),
        position: None,
        ty: fixture.store.types.alloc(Type::Method(MethodType {
            params: Vec::new(),
            result: int_type,
            kind: MethodKind::Plain,
        })),
    });
    let type_call = typed_call(
        &mut fixture,
        type_application,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            type_call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: type_application,
            kind: UnsupportedJvmCallShapeKind::TypeApplication,
        })
    );

    let outside_plan = add_unplanned_call_target_method(&mut fixture, "notPlanned", &[], int_type);
    let outside_function = typed_method_ident(&mut fixture, outside_plan);
    let outside_call = typed_call(
        &mut fixture,
        outside_function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            outside_call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::MethodNotInJvmPlan {
            tree: outside_function,
            class: fixture.class,
            method: outside_plan,
        })
    );
}

#[test]
fn rejects_unresolved_foreign_and_inconsistent_call_references() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let boolean_type = fixture.definitions.boolean;
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let mut external = EmissionFixture::new(false);
    let mut missing_symbol = alloc_symbol(
        &mut external.store,
        SymbolKind::Value,
        None,
        external.definitions.int,
    );
    while fixture.store.symbols.contains(missing_symbol) {
        missing_symbol = alloc_symbol(
            &mut external.store,
            SymbolKind::Value,
            None,
            external.definitions.int,
        );
    }
    let missing_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(missing_symbol),
    });
    let missing_function = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: fixture.store.symbols.get(identity).name,
            backquoted: false,
        }),
        position: None,
        ty: missing_type,
    });
    let missing_call = typed_call(
        &mut fixture,
        missing_function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            missing_call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::UnknownMethod {
            method: missing_symbol,
        })
    );

    let unresolved_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Name(TermName::new(fixture.store.names.intern("identity"))),
    });
    let unresolved_function = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: fixture.store.symbols.get(identity).name,
            backquoted: false,
        }),
        position: None,
        ty: unresolved_type,
    });
    let unresolved_call = typed_call(
        &mut fixture,
        unresolved_function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            unresolved_call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::UnresolvedJvmCallTarget {
            tree: unresolved_function,
        })
    );

    let foreign_class = alloc_symbol(&mut fixture.store, SymbolKind::Class, None, int_type);
    let foreign_method = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Method,
        Some(foreign_class),
        int_type,
    );
    let foreign_function = typed_method_ident(&mut fixture, foreign_method);
    let foreign_call = typed_call(
        &mut fixture,
        foreign_function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            foreign_call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::WrongOwner {
            symbol: foreign_method,
            expected: fixture.class,
            actual: Some(foreign_class),
        })
    );

    let function = typed_method_ident(&mut fixture, identity);
    let mismatched = typed_call(
        &mut fixture,
        function,
        Vec::new(),
        ApplyKind::Regular,
        boolean_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            mismatched,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::JvmCallResultTypeMismatch {
            tree: mismatched,
            method: identity,
            expected: JvmReturnType::Value(JvmType::Int),
            found: Some(JvmReturnType::Value(JvmType::Boolean)),
        })
    );
}

#[test]
fn rejects_visibility_flags_static_extension_and_constructor_call_targets() {
    let mut private = EmissionFixture::new(false);
    let int_type = private.definitions.int;
    let private_method = add_call_target_method(&mut private, "privateCall", &[], int_type);
    private.store.symbols.get_mut(private_method).visibility = Visibility::Private;
    let function = typed_method_ident(&mut private, private_method);
    let call = typed_call(
        &mut private,
        function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        private.unit().resolve_same_class_call(
            private.class,
            private.methods[1],
            call,
            &private.definitions,
        ),
        Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: private_method,
            visibility: Visibility::Private,
        })
    );

    for (flag, expected) in [
        (SymbolFlags::STATIC, UnsupportedJvmSymbolFlag::Static),
        (SymbolFlags::EXTENSION, UnsupportedJvmSymbolFlag::Extension),
    ] {
        let mut fixture = EmissionFixture::new(false);
        let int_type = fixture.definitions.int;
        let method = add_call_target_method(&mut fixture, "flagged", &[], int_type);
        fixture.store.symbols.get_mut(method).flags = flag;
        let function = typed_method_ident(&mut fixture, method);
        let call = typed_call(
            &mut fixture,
            function,
            Vec::new(),
            ApplyKind::Regular,
            int_type,
        );
        assert_eq!(
            fixture.unit().resolve_same_class_call(
                fixture.class,
                fixture.methods[1],
                call,
                &fixture.definitions,
            ),
            Err(JvmCodegenError::UnsupportedJvmSymbolFlag {
                symbol: method,
                flag: expected,
            })
        );
    }

    for clause_kind in [MethodKind::Implicit, MethodKind::Contextual] {
        let mut fixture = EmissionFixture::new(false);
        let int_type = fixture.definitions.int;
        let method = add_call_target_method(&mut fixture, "contextual", &[], int_type);
        let method_type = fixture.store.types.alloc(Type::Method(MethodType {
            params: Vec::new(),
            result: int_type,
            kind: clause_kind,
        }));
        fixture
            .store
            .symbols
            .set_info(method, SymbolInfo::Complete(method_type));
        let function = typed_method_ident(&mut fixture, method);
        let call = typed_call(
            &mut fixture,
            function,
            Vec::new(),
            ApplyKind::Regular,
            int_type,
        );
        assert_eq!(
            fixture.unit().resolve_same_class_call(
                fixture.class,
                fixture.methods[1],
                call,
                &fixture.definitions,
            ),
            Err(JvmCodegenError::UnsupportedMethodShape {
                method,
                ty: method_type,
                kind: UnsupportedMethodShapeKind::NonPlainClause,
            })
        );
    }

    let mut constructor = EmissionFixture::new(false);
    let int_type = constructor.definitions.int;
    let constructor_symbol = alloc_symbol(
        &mut constructor.store,
        SymbolKind::Constructor,
        Some(constructor.class),
        int_type,
    );
    let function = typed_method_ident(&mut constructor, constructor_symbol);
    let call = typed_call(
        &mut constructor,
        function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        constructor.unit().resolve_same_class_call(
            constructor.class,
            constructor.methods[1],
            call,
            &constructor.definitions,
        ),
        Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: constructor_symbol,
            expected: SymbolRole::Method,
            found: SymbolKind::Constructor,
        })
    );
}

#[test]
fn rejects_non_this_receivers_non_current_this_and_nullary_methods() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let select_parameters = fixture.parameters[1].clone();
    let receiver_symbol = select_parameters[0];
    let receiver_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(receiver_symbol),
    });
    let receiver = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: fixture.store.symbols.get(receiver_symbol).name,
            backquoted: false,
        }),
        position: None,
        ty: receiver_type,
    });
    let identity_type = fixture.store.types.alloc(Type::TermRef {
        prefix: receiver_type,
        target: TermRefTarget::Symbol(identity),
    });
    let select = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name: fixture.store.symbols.get(identity).name,
            backquoted: false,
        }),
        position: None,
        ty: identity_type,
    });
    let call = typed_call(
        &mut fixture,
        select,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: select,
            kind: UnsupportedJvmCallShapeKind::NonThisReceiver,
        })
    );

    let ident_with_foreign_prefix =
        typed_method_ident_with_prefix(&mut fixture, identity, receiver_type);
    let call = typed_call(
        &mut fixture,
        ident_with_foreign_prefix,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: ident_with_foreign_prefix,
            kind: UnsupportedJvmCallShapeKind::UnsupportedReceiverPrefix,
        })
    );

    let other_class = alloc_symbol(&mut fixture.store, SymbolKind::Class, None, int_type);
    let other_this_type = fixture
        .store
        .types
        .alloc(Type::ThisType { class: other_class });
    let other_this = fixture.ast.alloc(Tree {
        kind: TreeKind::This(This { qual: None }),
        position: None,
        ty: other_this_type,
    });
    let selected_type = fixture.store.types.alloc(Type::TermRef {
        prefix: other_this_type,
        target: TermRefTarget::Symbol(identity),
    });
    let selected = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: other_this,
            name: fixture.store.symbols.get(identity).name,
            backquoted: false,
        }),
        position: None,
        ty: selected_type,
    });
    let call = typed_call(
        &mut fixture,
        selected,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: selected,
            kind: UnsupportedJvmCallShapeKind::NonCurrentThis,
        })
    );

    let value = fixture.methods[0];
    fixture
        .store
        .symbols
        .set_info(value, SymbolInfo::Complete(int_type));
    let function = typed_method_ident(&mut fixture, value);
    let nullary_call = typed_call(
        &mut fixture,
        function,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture.unit().resolve_same_class_call(
            fixture.class,
            fixture.methods[1],
            nullary_call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::UnsupportedMethodShape {
            method: value,
            ty: int_type,
            kind: UnsupportedMethodShapeKind::NotMethodType,
        })
    );
}

#[test]
fn emits_a_deterministic_complete_class_with_constructor_methods_and_branch_frames() {
    let ordinary = EmissionFixture::new(false).emit().unwrap();
    let shifted = EmissionFixture::new(true).emit().unwrap();
    assert_eq!(ordinary.internal_name(), "example/Answer");
    assert_eq!(ordinary.bytes(), shifted.bytes());

    let mut reader = Reader::new(ordinary.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    assert!(reader.is_at_end());
    class.validate_references().unwrap();
    assert_eq!(class.version.major, 65);
    assert_eq!(
        class.constant_pool.class_name(class.this_class),
        Ok("example/Answer")
    );
    assert_eq!(class.fields.len(), 0);
    assert_eq!(class.methods.len(), 3);
    let names = class
        .methods
        .iter()
        .map(|method| class.constant_pool.utf8(method.name_index).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["<init>", "value", "select"]);
    assert_eq!(
        class.constant_pool.utf8(class.methods[0].descriptor_index),
        Ok("()V")
    );
    assert_eq!(
        class.constant_pool.utf8(class.methods[1].descriptor_index),
        Ok("()I")
    );
    assert_eq!(
        class.constant_pool.utf8(class.methods[2].descriptor_index),
        Ok("(ZII)I")
    );
    assert_eq!(
        class
            .methods
            .iter()
            .filter(|method| class.constant_pool.utf8(method.name_index) == Ok("<init>"))
            .count(),
        1
    );
    let Attribute::Code(constructor) = &class.methods[0].attributes[0] else {
        panic!("constructor code missing")
    };
    assert_eq!(constructor.code.len(), 5);
    assert_eq!(&constructor.code[..2], &[0x2a, 0xb7]);
    assert_eq!(constructor.code[4], 0xb1);
    let Attribute::Code(select) = &class.methods[2].attributes[0] else {
        panic!("select code missing")
    };
    assert!(select.code.contains(&0x99));
    assert!(select.attributes.iter().any(|attribute| matches!(attribute, Attribute::Other(raw) if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable"))));

    if require_jdk_or_skip() {
        verify_emitted_class_with_jdk(ordinary.bytes());
    }
}

fn require_jdk_or_skip() -> bool {
    let unavailable = ["java", "javac", "javap"]
        .into_iter()
        .filter_map(|tool| match jdk_tool_major_version(tool) {
            Some(major) if major >= 21 => None,
            Some(major) => Some(format!("{tool} {major} (requires JDK 21+)")),
            None => Some(format!("{tool} (missing or version unavailable)")),
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        return true;
    }

    let message = format!(
        "dotty-jvm emission gate requires java, javac, and javap from JDK 21+ on PATH; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}");
    }
    eprintln!("skipping dotty-jvm emitted class verifier gate: {message}");
    false
}

fn jdk_tool_major_version(tool: &str) -> Option<u32> {
    let output = Command::new(tool).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    text.split(|character: char| !character.is_ascii_digit())
        .find_map(|part| part.parse::<u32>().ok())
}

fn verify_emitted_class_with_jdk(bytes: &[u8]) {
    let directory = std::env::temp_dir().join(format!("dotty-jvm-emission-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    let class_directory = directory.join("example");
    fs::create_dir_all(&class_directory).unwrap();
    fs::write(class_directory.join("Answer.class"), bytes).unwrap();
    let classpath = directory.to_str().unwrap();

    let javap = Command::new("javap")
        .args(["-v", "-classpath", classpath, "example.Answer"])
        .output()
        .unwrap();
    assert!(
        javap.status.success(),
        "javap rejected emitted Answer.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    let javap_output = String::from_utf8_lossy(&javap.stdout);
    for expected in [
        "major version: 65",
        "example/Answer",
        "descriptor: ()V",
        "value()",
        "select(boolean, int, int)",
    ] {
        assert!(
            javap_output.contains(expected),
            "javap output omitted {expected:?}:\n{javap_output}"
        );
    }

    let harness = directory.join("EmissionHarness.java");
    fs::write(
        &harness,
        "public final class EmissionHarness { public static void main(String[] args) { example.Answer answer = new example.Answer(); System.out.println(answer.value() + \":\" + answer.select(true, 7, 9) + \":\" + answer.select(false, 7, 9)); } }\n",
    )
    .unwrap();
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .unwrap();
    assert!(
        javac.status.success(),
        "javac rejected emitted Answer.class harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );

    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath", classpath, "EmissionHarness"])
        .output()
        .unwrap();
    assert!(
        java.status.success(),
        "java -Xverify:all rejected emitted Answer.class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "42:7:9");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn class_emission_fails_closed_for_unknown_constructor_fields_and_superclasses() {
    let mut fixture = EmissionFixture::new(false);
    let mut unit = fixture.unit();
    unit.classes[0].initialization = JvmClassInitializationPlan::Unsupported;
    assert_eq!(
        emit_class(&unit, fixture.class, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedConstructor {
            class: fixture.class
        })
    );

    let mut parameterized = EmissionFixture::new(false);
    let declarations = match parameterized.store.symbols.info(parameterized.class) {
        SymbolInfo::Complete(ty) => match parameterized.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let int_type = parameterized.definitions.int;
    let unit_type = parameterized.definitions.unit;
    let (constructor, _) = add_emission_method(
        &mut parameterized.store,
        parameterized.class,
        declarations,
        "<init>",
        &[("seed", int_type)],
        unit_type,
    );
    parameterized.store.symbols.get_mut(constructor).kind = SymbolKind::Constructor;
    assert_eq!(
        parameterized.emit(),
        Err(JvmCodegenError::UnsupportedConstructor {
            class: parameterized.class
        })
    );

    let mut nested_clause = EmissionFixture::new(false);
    let declarations = match nested_clause.store.symbols.info(nested_clause.class) {
        SymbolInfo::Complete(ty) => match nested_clause.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let (constructor, _) = add_emission_method(
        &mut nested_clause.store,
        nested_clause.class,
        declarations,
        "<init>",
        &[],
        nested_clause.definitions.unit,
    );
    nested_clause.store.symbols.get_mut(constructor).kind = SymbolKind::Constructor;
    let inner_clause = nested_clause.store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: TermName::new(nested_clause.store.names.intern("seed")),
            ty: nested_clause.definitions.int,
            erased: false,
            varargs: false,
        }],
        result: nested_clause.definitions.unit,
        kind: MethodKind::Plain,
    }));
    let empty_outer_clause = nested_clause.store.types.alloc(Type::Method(MethodType {
        params: Vec::new(),
        result: inner_clause,
        kind: MethodKind::Plain,
    }));
    nested_clause
        .store
        .symbols
        .set_info(constructor, SymbolInfo::Complete(empty_outer_clause));
    assert_eq!(
        nested_clause.emit(),
        Err(JvmCodegenError::UnsupportedConstructor {
            class: nested_clause.class
        })
    );

    let field_name = fixture.store.names.intern("field");
    let field = fixture.store.symbols.alloc(Symbol {
        name: Name::new(field_name, Namespace::Term),
        owner: Some(fixture.class),
        kind: SymbolKind::Field,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(fixture.definitions.int),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let class_info_type = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => *ty,
        _ => unreachable!(),
    };
    let Type::ClassInfo(info) = fixture.store.types.get(class_info_type).clone() else {
        unreachable!()
    };
    fixture
        .store
        .scopes
        .get_mut(info.declarations)
        .enter(Name::new(field_name, Namespace::Term), field);
    assert_eq!(
        fixture.emit(),
        Err(JvmCodegenError::UnsupportedJvmClassMember {
            class: fixture.class,
            member: field,
            kind: SymbolKind::Field
        })
    );

    let mut superclass = EmissionFixture::new(false);
    let Type::ClassInfo(mut info) = (match superclass.store.symbols.info(superclass.class) {
        SymbolInfo::Complete(ty) => superclass.store.types.get(*ty).clone(),
        _ => unreachable!(),
    }) else {
        unreachable!()
    };
    info.parents[0] = superclass.definitions.int;
    let changed = superclass.store.types.alloc(Type::ClassInfo(info));
    superclass
        .store
        .symbols
        .set_info(superclass.class, SymbolInfo::Complete(changed));
    assert_eq!(
        superclass.emit(),
        Err(JvmCodegenError::UnsupportedJvmSuperclass {
            class: superclass.class,
            parent: Some(superclass.definitions.int)
        })
    );
}

#[test]
fn missing_class_declaration_errors_follow_method_plan_order() {
    let mut fixture = EmissionFixture::new(false);
    let class_info_type = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => *ty,
        _ => unreachable!(),
    };
    let Type::ClassInfo(info) = fixture.store.types.get(class_info_type) else {
        unreachable!()
    };
    let declarations = info.declarations;
    let expected_method = fixture.methods[0];
    for method in &fixture.methods {
        fixture.store.scopes.get_mut(declarations).remove(*method);
    }

    let expected = Err(JvmCodegenError::MethodNotInClassDeclarations {
        class: fixture.class,
        method: expected_method,
    });
    assert_eq!(fixture.emit(), expected);
    assert_eq!(fixture.emit(), expected);
}
