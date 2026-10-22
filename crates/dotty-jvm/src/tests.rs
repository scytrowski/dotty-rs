use dotty_classfile::attribute::Attribute;
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::class_file::ClassFileVersion;
use dotty_classfile::class_file_builder::ClassFileBuilder;
use dotty_classfile::constant_pool::{ConstantPoolEntry, ConstantPoolIndex};
use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_classfile::reader::Reader;
use dotty_core::ast::{
    Apply, ApplyKind, Block, Ident, If, Literal, Select, This, Tree, TreeKind, TypeApply, TypedAst,
    ValDef,
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
    TypeRefTarget,
};
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::locals::JvmLocalAllocator;
use crate::{
    InvalidJvmClassInfoKind, InvalidTypeReason, JvmClassInitializationPlan, JvmClassPlan,
    JvmCodegenError, JvmCompilationUnit, JvmExpressionFlow, JvmFieldPlan, JvmIntIntrinsic,
    JvmIntIntrinsicError, JvmInvocationKind, JvmLocalBinding, JvmMethodPlan, JvmNameKind,
    JvmPrimaryConstructorPlan, JvmReturnType, JvmType, JvmValueCategory, SymbolRole,
    UnsupportedJvmCallShapeKind, UnsupportedJvmClassKind, UnsupportedJvmLocalFlag,
    UnsupportedJvmSymbolFlag, UnsupportedJvmTypeKind, UnsupportedMethodShapeKind, emit_class,
    resolve_int_intrinsic,
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
                fields: Vec::new(),
                methods: vec![JvmMethodPlan {
                    method: self.method,
                    parameters: vec![self.parameter],
                    locals: Vec::new(),
                    body: Some(self.body),
                }],
                initialization: JvmClassInitializationPlan::Unsupported,
            }],
        }
    }
}

fn add_local_binding(fixture: &mut Fixture, name: &str, mutable: bool) -> JvmLocalBinding {
    let name_id = fixture.store.names.intern(name);
    let symbol = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Local,
        Some(fixture.method),
        fixture.value_type,
    );
    if mutable {
        fixture.store.symbols.get_mut(symbol).flags = SymbolFlags::MUTABLE;
    }
    let declaration = fixture.ast.alloc(Tree {
        kind: TreeKind::ValDef(ValDef {
            name: TermName::new(name_id),
            tpt: fixture.body,
            rhs: Some(fixture.body),
            metadata: (),
        }),
        position: None,
        ty: fixture.value_type,
    });
    let previous_body = fixture.body;
    fixture.body = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![declaration],
            expr: previous_body,
        }),
        position: None,
        ty: fixture.value_type,
    });
    JvmLocalBinding {
        declaration,
        symbol,
        mutable,
    }
}

#[test]
fn validates_canonical_immutable_and_mutable_local_bindings() {
    let mut fixture = Fixture::new();
    let immutable = add_local_binding(&mut fixture, "saved", false);
    let mutable = add_local_binding(&mut fixture, "saved", true);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![immutable, mutable];

    assert_eq!(unit.validate(), Ok(()));
    assert_eq!(unit.classes[0].methods[0].locals(), &[immutable, mutable]);
}

#[test]
fn rejects_stale_duplicate_foreign_and_parameter_local_bindings() {
    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    let checkpoint = fixture.ast.checkpoint();
    let stale_declaration = fixture.ast.alloc(Tree {
        kind: TreeKind::ValDef(ValDef {
            name: TermName::new(fixture.store.names.intern("stale")),
            tpt: fixture.body,
            rhs: Some(fixture.body),
            metadata: (),
        }),
        position: None,
        ty: fixture.value_type,
    });
    fixture.ast.rollback_to(checkpoint);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![JvmLocalBinding {
        declaration: stale_declaration,
        ..binding
    }];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnknownLocalDeclaration {
            method: fixture.method,
            declaration: stale_declaration,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding, binding];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::DuplicateLocalDeclaration {
            declaration: binding.declaration,
            method: fixture.method,
        })
    );

    let mut fixture = Fixture::new();
    let first = add_local_binding(&mut fixture, "first", false);
    let second = add_local_binding(&mut fixture, "second", false);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![
        first,
        JvmLocalBinding {
            symbol: first.symbol,
            ..second
        },
    ];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::DuplicateLocalSymbol {
            symbol: first.symbol,
            method: fixture.method,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![JvmLocalBinding {
        declaration: fixture.body,
        ..binding
    }];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::InvalidLocalDeclaration {
            method: fixture.method,
            declaration: fixture.body,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    let other_type = fixture.store.types.alloc(Type::Constant(Constant::Int(7)));
    fixture
        .store
        .symbols
        .set_info(binding.symbol, SymbolInfo::Complete(other_type));
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::LocalTypeMismatch {
            declaration: binding.declaration,
            symbol: binding.symbol,
            declaration_type: fixture.value_type,
            symbol_type: other_type,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    let mut local = fixture.store.symbols.get(binding.symbol).clone();
    local.kind = SymbolKind::Value;
    let wrong_kind = fixture.store.symbols.alloc(local);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding];
    unit.classes[0].methods[0].locals[0].symbol = wrong_kind;
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: wrong_kind,
            expected: SymbolRole::Local,
            found: SymbolKind::Value,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    let checkpoint = fixture.store.checkpoint();
    let stale_symbol = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Local,
        Some(fixture.method),
        fixture.value_type,
    );
    fixture.store.rollback_to(checkpoint);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![JvmLocalBinding {
        symbol: stale_symbol,
        ..binding
    }];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnknownLocalSymbol {
            method: fixture.method,
            symbol: stale_symbol,
        })
    );

    let mut fixture = Fixture::new();
    let first = add_local_binding(&mut fixture, "first", false);
    let second = add_local_binding(&mut fixture, "second", false);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![second, first];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnorderedLocalBindings {
            method: fixture.method,
        })
    );

    let mut fixture = Fixture::new();
    let other_method = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Method,
        Some(fixture.class),
        fixture.value_type,
    );
    let foreign_local = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Local,
        Some(other_method),
        fixture.value_type,
    );
    let binding = add_local_binding(&mut fixture, "foreign", false);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![JvmLocalBinding {
        declaration: binding.declaration,
        symbol: foreign_local,
        mutable: false,
    }];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::WrongOwner {
            symbol: foreign_local,
            expected: fixture.method,
            actual: Some(other_method),
        })
    );

    let mut fixture = Fixture::new();
    let symbol = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Local,
        Some(fixture.method),
        fixture.value_type,
    );
    let declaration = fixture.ast.alloc(Tree {
        kind: TreeKind::ValDef(ValDef {
            name: TermName::new(fixture.store.names.intern("foreignTree")),
            tpt: fixture.body,
            rhs: Some(fixture.body),
            metadata: (),
        }),
        position: None,
        ty: fixture.value_type,
    });
    let binding = JvmLocalBinding {
        declaration,
        symbol,
        mutable: false,
    };
    let other_method = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Method,
        Some(fixture.class),
        fixture.value_type,
    );
    let other_body = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![binding.declaration],
            expr: fixture.body,
        }),
        position: None,
        ty: fixture.value_type,
    });
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding];
    unit.classes[0].methods.push(JvmMethodPlan {
        method: other_method,
        parameters: Vec::new(),
        locals: Vec::new(),
        body: Some(other_body),
    });
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::LocalDeclarationOutsideMethod {
            method: fixture.method,
            declaration,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "parameter", false);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![JvmLocalBinding {
        declaration: binding.declaration,
        symbol: fixture.parameter,
        mutable: false,
    }];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::LocalIsMethodParameter {
            method: fixture.method,
            symbol: fixture.parameter,
        })
    );
}

#[test]
fn rejects_invalid_local_type_info_flags_and_mutability() {
    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    fixture.store.symbols.get_mut(binding.symbol).info = SymbolInfo::Missing;
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::MissingSemanticInfo {
            symbol: binding.symbol,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    let no_type = fixture.store.types.alloc(Type::NoType);
    fixture.ast.get_mut(binding.declaration).ty = no_type;
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::InvalidTreeType {
            tree: binding.declaration,
            ty: no_type,
            reason: InvalidTypeReason::NoType,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", true);
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![JvmLocalBinding {
        mutable: false,
        ..binding
    }];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::LocalMutabilityMismatch {
            declaration: binding.declaration,
            symbol: binding.symbol,
            recorded_mutable: false,
            symbol_mutable: true,
        })
    );

    let mut fixture = Fixture::new();
    let binding = add_local_binding(&mut fixture, "saved", false);
    fixture.store.symbols.get_mut(binding.symbol).flags = SymbolFlags::LAZY;
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding];
    assert_eq!(
        unit.validate(),
        Err(JvmCodegenError::UnsupportedLocalFlag {
            symbol: binding.symbol,
            flag: UnsupportedJvmLocalFlag::Lazy,
        })
    );
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
            locals: Vec::new(),
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
            fields: Vec::new(),
            methods: Vec::new(),
            initialization: JvmClassInitializationPlan::Unsupported,
        }
        .identity(&self.store)
    }
}

struct ReferenceFixture {
    store: SemanticStore,
    ast: TypedAst,
    definitions: Definitions,
    classes: Vec<SymbolId>,
}

struct HierarchyFixture {
    store: SemanticStore,
    ast: TypedAst,
    definitions: Definitions,
    base: SymbolId,
    derived: SymbolId,
    third: SymbolId,
    fourth: SymbolId,
}

impl HierarchyFixture {
    fn new() -> Self {
        let mut identity = ClassIdentityFixture::new(&["example"], "Base");
        let definitions = Definitions::bootstrap(&mut identity.store);
        set_object_parent(
            &mut identity.store,
            identity.class,
            definitions.no_prefix,
            definitions.object_type,
        );
        let owner = identity.store.symbols.get(identity.class).owner.unwrap();
        let derived = add_reference_class(
            &mut identity.store,
            owner,
            "Derived",
            definitions.no_prefix,
            definitions.object_type,
            SymbolOrigin::Synthetic,
            SymbolFlags::EMPTY,
        );
        let third = add_reference_class(
            &mut identity.store,
            owner,
            "Third",
            definitions.no_prefix,
            definitions.object_type,
            SymbolOrigin::Synthetic,
            SymbolFlags::EMPTY,
        );
        let fourth = add_reference_class(
            &mut identity.store,
            owner,
            "Fourth",
            definitions.no_prefix,
            definitions.object_type,
            SymbolOrigin::Synthetic,
            SymbolFlags::EMPTY,
        );
        let base_type = identity.store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(identity.class),
        });
        set_class_parents(&mut identity.store, derived, vec![base_type]);
        let derived_type = identity.store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(derived),
        });
        set_class_parents(&mut identity.store, third, vec![derived_type]);
        let third_type = identity.store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(third),
        });
        set_class_parents(&mut identity.store, fourth, vec![third_type]);
        Self {
            store: identity.store,
            ast: TypedAst::new(),
            definitions,
            base: identity.class,
            derived,
            third,
            fourth,
        }
    }

    fn unit(&self, classes: &[SymbolId]) -> JvmCompilationUnit<'_> {
        JvmCompilationUnit {
            store: &self.store,
            typed_ast: &self.ast,
            classes: classes
                .iter()
                .copied()
                .map(|class| JvmClassPlan {
                    class,
                    fields: Vec::new(),
                    methods: Vec::new(),
                    initialization: JvmClassInitializationPlan::ImplicitNoArg,
                })
                .collect(),
        }
    }
}

fn set_class_parents(store: &mut SemanticStore, class: SymbolId, parents: Vec<dotty_core::TypeId>) {
    let info = match store.symbols.info(class) {
        SymbolInfo::Complete(ty) => match store.types.get(*ty) {
            Type::ClassInfo(info) => info.clone(),
            _ => panic!("hierarchy test class should have ClassInfo"),
        },
        _ => panic!("hierarchy test class should have completed info"),
    };
    let class_info = store
        .types
        .alloc(Type::ClassInfo(ClassInfo { parents, ..info }));
    store
        .symbols
        .set_info(class, SymbolInfo::Complete(class_info));
}

#[test]
fn compilation_unit_projects_one_hop_canonical_superclass_proofs_in_plan_order() {
    let mut fixture = HierarchyFixture::new();
    let unit = fixture.unit(&[fixture.base, fixture.derived]);
    let before = *fixture.store.symbols.info(fixture.base);
    let hierarchy = unit.class_hierarchy(&fixture.definitions).unwrap();
    assert_eq!(fixture.store.symbols.info(fixture.base), &before);
    assert_eq!(
        hierarchy
            .superclasses()
            .iter()
            .map(|plan| (
                plan.class(),
                plan.parent(),
                plan.class_internal_name(),
                plan.parent_internal_name(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                fixture.base,
                fixture.definitions.object_class,
                "example/Base",
                "java/lang/Object"
            ),
            (
                fixture.derived,
                fixture.base,
                "example/Derived",
                "example/Base"
            ),
        ]
    );
    assert_eq!(
        hierarchy
            .internal_name_edges()
            .iter()
            .map(|edge| (edge.child_internal_name(), edge.parent_internal_name()))
            .collect::<Vec<_>>(),
        vec![
            ("example/Base", "java/lang/Object"),
            ("example/Derived", "example/Base"),
        ]
    );

    let metadata = unit.metadata(&fixture.definitions).unwrap();
    assert_eq!(
        metadata[0].superclass().unwrap().parent(),
        fixture.definitions.object_class
    );
    assert_eq!(metadata[1].superclass().unwrap().parent(), fixture.base);
    assert_eq!(metadata[1].identity().symbol(), fixture.derived);

    let reversed = fixture.unit(&[fixture.derived, fixture.base]);
    let reversed_hierarchy = reversed.class_hierarchy(&fixture.definitions).unwrap();
    let normalized = |hierarchy: &crate::JvmClassHierarchy| {
        let mut edges = hierarchy
            .superclasses()
            .iter()
            .map(|plan| (plan.class(), plan.parent()))
            .collect::<Vec<_>>();
        edges.sort_by_key(|(class, _)| class.index());
        edges
    };
    assert_eq!(normalized(&hierarchy), normalized(&reversed_hierarchy));

    let package = fixture.store.symbols.get(fixture.base).owner.unwrap();
    let package_prefix = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(package),
    });
    let qualified_parent = fixture.store.types.alloc(Type::TypeRef {
        prefix: package_prefix,
        target: TypeRefTarget::Symbol(fixture.base),
    });
    set_class_parents(&mut fixture.store, fixture.derived, vec![qualified_parent]);
    assert!(
        fixture
            .unit(&[fixture.base, fixture.derived])
            .class_hierarchy(&fixture.definitions)
            .is_ok()
    );
}

#[test]
fn compilation_unit_hierarchy_accepts_three_user_classes_in_any_plan_order() {
    let fixture = HierarchyFixture::new();
    let before = (
        *fixture.store.symbols.info(fixture.base),
        *fixture.store.symbols.info(fixture.derived),
        *fixture.store.symbols.info(fixture.third),
    );
    let unit = fixture.unit(&[fixture.third, fixture.base, fixture.derived]);
    let hierarchy = unit.class_hierarchy(&fixture.definitions).unwrap();

    assert_eq!(
        (
            *fixture.store.symbols.info(fixture.base),
            *fixture.store.symbols.info(fixture.derived),
            *fixture.store.symbols.info(fixture.third),
        ),
        before
    );
    assert_eq!(
        hierarchy
            .superclasses()
            .iter()
            .map(|plan| (plan.class(), plan.parent()))
            .collect::<Vec<_>>(),
        vec![
            (fixture.third, fixture.derived),
            (fixture.base, fixture.definitions.object_class),
            (fixture.derived, fixture.base),
        ]
    );
    assert_eq!(
        hierarchy
            .internal_name_edges()
            .iter()
            .map(|edge| (edge.child_internal_name(), edge.parent_internal_name()))
            .collect::<Vec<_>>(),
        vec![
            ("example/Third", "example/Derived"),
            ("example/Base", "java/lang/Object"),
            ("example/Derived", "example/Base"),
        ]
    );
    let metadata = unit.metadata(&fixture.definitions).unwrap();
    assert_eq!(
        metadata
            .iter()
            .map(|class| (
                class.identity().symbol(),
                class.superclass().unwrap().parent(),
            ))
            .collect::<Vec<_>>(),
        [
            (fixture.third, fixture.derived),
            (fixture.base, fixture.definitions.object_class),
            (fixture.derived, fixture.base),
        ]
    );
}

#[test]
fn reference_projection_requires_the_complete_unit_proof_for_a_deep_class() {
    let mut fixture = HierarchyFixture::new();
    let third_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(fixture.third),
    });
    let unit = fixture.unit(&[fixture.third, fixture.derived, fixture.base]);

    assert_eq!(
        crate::reference::project_type(&unit, third_type, &fixture.definitions),
        Ok(JvmType::Reference(fixture.third))
    );

    let identities = crate::identity::unit_class_identities(&unit).unwrap();
    assert_eq!(
        crate::reference::project_method_signature_type(
            &fixture.store,
            third_type,
            &fixture.definitions,
            &identities,
            Some(&unit),
        ),
        Ok(JvmType::Reference(fixture.third))
    );
    assert!(matches!(
        crate::reference::project_method_signature_type(
            &fixture.store,
            third_type,
            &fixture.definitions,
            &identities,
            None,
        ),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            kind: crate::UnsupportedJvmReferenceTypeKind::NotObjectDerived,
            ..
        })
    ));
}

#[test]
fn reference_assignability_uses_only_the_validated_two_edge_ancestor_path() {
    let mut fixture = HierarchyFixture::new();
    let owner = fixture.store.symbols.get(fixture.base).owner.unwrap();
    let other = add_reference_class(
        &mut fixture.store,
        owner,
        "Other",
        fixture.definitions.no_prefix,
        fixture.definitions.object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    let foreign_base = add_reference_class(
        &mut fixture.store,
        owner,
        "Base",
        fixture.definitions.no_prefix,
        fixture.definitions.object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    let unit = fixture.unit(&[fixture.base, fixture.derived, fixture.third, other]);
    let reference = |class| JvmType::Reference(class);

    assert!(
        crate::reference::is_assignable(
            Some(&unit),
            reference(fixture.third),
            reference(fixture.base),
            &fixture.definitions,
        )
        .unwrap()
    );
    assert!(
        crate::reference::is_assignable(
            Some(&unit),
            reference(fixture.third),
            reference(fixture.derived),
            &fixture.definitions,
        )
        .unwrap()
    );
    assert!(
        crate::reference::is_assignable(
            Some(&unit),
            reference(fixture.derived),
            reference(fixture.base),
            &fixture.definitions,
        )
        .unwrap()
    );
    assert!(
        !crate::reference::is_assignable(
            Some(&unit),
            reference(fixture.base),
            reference(fixture.third),
            &fixture.definitions,
        )
        .unwrap()
    );
    assert!(
        !crate::reference::is_assignable(
            Some(&unit),
            reference(other),
            reference(fixture.base),
            &fixture.definitions,
        )
        .unwrap()
    );
    assert!(
        !crate::reference::is_assignable(
            None,
            reference(fixture.third),
            reference(fixture.base),
            &fixture.definitions,
        )
        .unwrap()
    );

    let unplanned_unit = fixture.unit(&[fixture.base, fixture.derived]);
    assert_eq!(
        crate::reference::is_assignable(
            Some(&unplanned_unit),
            reference(fixture.third),
            reference(fixture.base),
            &fixture.definitions,
        ),
        Err(JvmCodegenError::ReferenceClassNotInJvmPlan {
            class: fixture.third,
        })
    );

    assert_eq!(
        crate::reference::is_assignable(
            Some(&unit),
            reference(fixture.third),
            reference(foreign_base),
            &fixture.definitions,
        ),
        Err(JvmCodegenError::ReferenceClassNotInJvmPlan {
            class: foreign_base,
        })
    );
    assert_eq!(
        crate::reference::is_assignable(
            Some(&unit),
            reference(foreign_base),
            reference(foreign_base),
            &fixture.definitions,
        ),
        Err(JvmCodegenError::ReferenceClassNotInJvmPlan {
            class: foreign_base,
        })
    );
}

#[test]
fn whole_class_emission_uses_the_validated_parent_for_class_and_constructor() {
    let fixture = HierarchyFixture::new();
    let unit = fixture.unit(&[fixture.base, fixture.derived]);
    let base = emit_class(&unit, fixture.base, &fixture.definitions).unwrap();
    let derived = emit_class(&unit, fixture.derived, &fixture.definitions).unwrap();

    let mut base_reader = Reader::new(base.bytes());
    let base_class = ClassFile::decode(&mut base_reader).unwrap();
    let mut derived_reader = Reader::new(derived.bytes());
    let derived_class = ClassFile::decode(&mut derived_reader).unwrap();
    assert_eq!(
        derived_class
            .constant_pool
            .class_name(derived_class.super_class.unwrap()),
        Ok("example/Base")
    );
    let constructor = emitted_method_code(&derived_class, "<init>");
    assert_eq!(constructor[0], 0x2a);
    assert_eq!(constructor[1], 0xb7);
    assert_eq!(constructor[4], 0xb1);
    assert_special_method_ref(
        &derived_class,
        constructor,
        1,
        "example/Base",
        "<init>",
        "()V",
    );
    assert_eq!(
        base_class
            .constant_pool
            .class_name(base_class.super_class.unwrap()),
        Ok("java/lang/Object")
    );
    let base_constructor = emitted_method_code(&base_class, "<init>");
    assert_special_method_ref(
        &base_class,
        base_constructor,
        1,
        "java/lang/Object",
        "<init>",
        "()V",
    );
}

#[test]
fn common_planned_ancestor_is_nearest_unique_and_ignores_plan_order() {
    let fixture = HierarchyFixture::new();
    let chain = fixture.unit(&[fixture.base, fixture.derived, fixture.third]);
    let chain_hierarchy = chain.class_hierarchy(&fixture.definitions).unwrap();
    assert_eq!(
        chain_hierarchy.nearest_common_non_object_ancestor(fixture.third, fixture.derived),
        Some(fixture.derived)
    );
    assert_eq!(
        chain_hierarchy.nearest_common_non_object_ancestor(fixture.derived, fixture.third),
        Some(fixture.derived)
    );
    assert_eq!(
        chain_hierarchy.nearest_common_non_object_ancestor(fixture.third, fixture.base),
        Some(fixture.base)
    );
    assert_eq!(
        chain_hierarchy.nearest_common_non_object_ancestor(fixture.third, fixture.third),
        Some(fixture.third)
    );

    let reversed_chain = fixture.unit(&[fixture.third, fixture.derived, fixture.base]);
    let reversed_hierarchy = reversed_chain
        .class_hierarchy(&fixture.definitions)
        .unwrap();
    assert_eq!(
        reversed_hierarchy.nearest_common_non_object_ancestor(fixture.third, fixture.derived),
        Some(fixture.derived)
    );

    let mut siblings = HierarchyFixture::new();
    let sibling_parent = siblings.store.types.alloc(Type::TypeRef {
        prefix: siblings.definitions.no_prefix,
        target: TypeRefTarget::Symbol(siblings.base),
    });
    set_class_parents(&mut siblings.store, siblings.third, vec![sibling_parent]);
    let sibling_unit = siblings.unit(&[siblings.third, siblings.base, siblings.derived]);
    let sibling_hierarchy = sibling_unit.class_hierarchy(&siblings.definitions).unwrap();
    assert_eq!(
        sibling_hierarchy.nearest_common_non_object_ancestor(siblings.derived, siblings.third),
        Some(siblings.base)
    );
    assert_eq!(
        sibling_hierarchy.nearest_common_non_object_ancestor(siblings.third, siblings.derived),
        Some(siblings.base)
    );

    let classfile_hierarchy = dotty_classfile::bytecode::ClassHierarchy::from_edges(
        sibling_hierarchy
            .internal_name_edges()
            .into_iter()
            .map(|edge| {
                (
                    edge.child_internal_name().to_owned(),
                    edge.parent_internal_name().to_owned(),
                )
            }),
    )
    .expect("the checked sibling JVM names form a valid class-file hierarchy");
    assert!(classfile_hierarchy.is_assignable("example/Derived", "example/Base"));
    assert!(classfile_hierarchy.is_assignable("example/Third", "example/Base"));
}

#[test]
fn common_planned_ancestor_rejects_object_only_and_foreign_same_name_inputs() {
    let mut fixture = HierarchyFixture::new();
    set_class_parents(
        &mut fixture.store,
        fixture.third,
        vec![fixture.definitions.object_type],
    );
    let package = fixture.store.symbols.get(fixture.base).owner.unwrap();
    let foreign_base = add_reference_class(
        &mut fixture.store,
        package,
        "Base",
        fixture.definitions.no_prefix,
        fixture.definitions.object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    assert_ne!(foreign_base, fixture.base);
    assert_eq!(
        fixture
            .store
            .names
            .resolve(fixture.store.symbols.get(foreign_base).name.text()),
        fixture
            .store
            .names
            .resolve(fixture.store.symbols.get(fixture.base).name.text())
    );

    let unit = fixture.unit(&[fixture.base, fixture.derived, fixture.third]);
    let hierarchy = unit.class_hierarchy(&fixture.definitions).unwrap();
    assert_eq!(
        hierarchy.nearest_common_non_object_ancestor(fixture.base, fixture.third),
        None,
        "sharing only the builtin Object root is not an approved source ancestor"
    );
    assert_eq!(
        hierarchy.nearest_common_non_object_ancestor(foreign_base, fixture.derived),
        None,
        "same-spelled symbols are not substituted for the planned canonical class"
    );
    assert_eq!(
        hierarchy
            .nearest_common_non_object_ancestor(fixture.definitions.object_class, fixture.base),
        None,
        "the builtin Object class is not a planned common-ancestor result"
    );
}

#[test]
fn compilation_unit_hierarchy_rejects_unplanned_final_cyclic_and_deep_parents() {
    let fixture = HierarchyFixture::new();
    let missing = fixture.unit(&[fixture.derived]);
    assert!(matches!(
        missing.class_hierarchy(&fixture.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: crate::JvmSuperclassIssue::ParentNotPlanned,
        }) if class == fixture.derived && parent == fixture.base
    ));

    let mut final_parent = HierarchyFixture::new();
    final_parent.store.symbols.get_mut(final_parent.base).flags = SymbolFlags::FINAL;
    assert!(matches!(
        final_parent
            .unit(&[final_parent.base, final_parent.derived])
            .class_hierarchy(&final_parent.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: crate::JvmSuperclassIssue::ParentIsFinal,
        }) if class == final_parent.derived && parent == final_parent.base
    ));

    let mut cyclic = HierarchyFixture::new();
    let derived_parent = cyclic.store.types.alloc(Type::TypeRef {
        prefix: cyclic.definitions.no_prefix,
        target: TypeRefTarget::Symbol(cyclic.derived),
    });
    set_class_parents(&mut cyclic.store, cyclic.base, vec![derived_parent]);
    assert!(matches!(
        cyclic
            .unit(&[cyclic.base, cyclic.derived])
            .class_hierarchy(&cyclic.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            issue: crate::JvmSuperclassIssue::InheritanceCycle,
            ..
        })
    ));

    let too_deep = fixture.unit(&[fixture.base, fixture.derived, fixture.third, fixture.fourth]);
    assert!(matches!(
        too_deep.class_hierarchy(&fixture.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: crate::JvmSuperclassIssue::InheritanceTooDeep,
        }) if class == fixture.fourth && parent == fixture.third
    ));

    let mut self_parent = HierarchyFixture::new();
    let self_type = self_parent.store.types.alloc(Type::TypeRef {
        prefix: self_parent.definitions.no_prefix,
        target: TypeRefTarget::Symbol(self_parent.derived),
    });
    set_class_parents(&mut self_parent.store, self_parent.derived, vec![self_type]);
    assert!(matches!(
        self_parent
            .unit(&[self_parent.base, self_parent.derived])
            .class_hierarchy(&self_parent.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: crate::JvmSuperclassIssue::SelfParent,
        }) if class == self_parent.derived && parent == self_parent.derived
    ));

    let mut private_parent = HierarchyFixture::new();
    private_parent
        .store
        .symbols
        .get_mut(private_parent.base)
        .visibility = Visibility::Private;
    assert!(matches!(
        private_parent
            .unit(&[private_parent.base, private_parent.derived])
            .class_hierarchy(&private_parent.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: crate::JvmSuperclassIssue::ParentNotPublic,
        }) if class == private_parent.derived && parent == private_parent.base
    ));
}

#[test]
fn compilation_unit_hierarchy_rejects_multiple_parents_and_unavailable_super_constructor() {
    let mut fixture = HierarchyFixture::new();
    let base_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(fixture.base),
    });
    set_class_parents(
        &mut fixture.store,
        fixture.derived,
        vec![base_type, fixture.definitions.object_type],
    );
    assert!(matches!(
        fixture
            .unit(&[fixture.base, fixture.derived])
            .class_hierarchy(&fixture.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            issue: crate::JvmSuperclassIssue::MissingOrMultipleParents,
            ..
        }) if class == fixture.derived
    ));

    let fixture = HierarchyFixture::new();
    let mut unit = fixture.unit(&[fixture.base, fixture.derived]);
    unit.classes[0].initialization = JvmClassInitializationPlan::Unsupported;
    assert!(matches!(
        unit.class_hierarchy(&fixture.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: crate::JvmSuperclassIssue::ParentConstructorUnavailable,
        }) if class == fixture.derived && parent == fixture.base
    ));

    let mut malformed_prefix = HierarchyFixture::new();
    let foreign_package = malformed_prefix.store.symbols.alloc(Symbol {
        name: Name::new(
            malformed_prefix.store.names.intern("foreign"),
            Namespace::Term,
        ),
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
    let foreign_prefix = malformed_prefix.store.types.alloc(Type::TypeRef {
        prefix: malformed_prefix.definitions.no_prefix,
        target: TypeRefTarget::Symbol(foreign_package),
    });
    let wrong_parent = malformed_prefix.store.types.alloc(Type::TypeRef {
        prefix: foreign_prefix,
        target: TypeRefTarget::Symbol(malformed_prefix.base),
    });
    set_class_parents(
        &mut malformed_prefix.store,
        malformed_prefix.derived,
        vec![wrong_parent],
    );
    assert!(matches!(
        malformed_prefix
            .unit(&[malformed_prefix.base, malformed_prefix.derived])
            .class_hierarchy(&malformed_prefix.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            issue: crate::JvmSuperclassIssue::NonCanonicalParentType,
            ..
        }) if class == malformed_prefix.derived
    ));

    let mut curried_parent = HierarchyFixture::new();
    let declarations = match curried_parent.store.symbols.info(curried_parent.base) {
        SymbolInfo::Complete(ty) => match curried_parent.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("Base should have ClassInfo"),
        },
        _ => panic!("Base should have completed info"),
    };
    let constructor_name = curried_parent.store.names.intern("<init>");
    let result_clause = curried_parent.store.types.alloc(Type::Method(MethodType {
        params: Vec::new(),
        result: curried_parent.definitions.unit,
        kind: MethodKind::Plain,
    }));
    let constructor_type = curried_parent.store.types.alloc(Type::Method(MethodType {
        params: Vec::new(),
        result: result_clause,
        kind: MethodKind::Plain,
    }));
    let constructor = curried_parent.store.symbols.alloc(Symbol {
        name: Name::new(constructor_name, Namespace::Term),
        owner: Some(curried_parent.base),
        kind: SymbolKind::Constructor,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(constructor_type),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    curried_parent
        .store
        .scopes
        .get_mut(declarations)
        .enter(Name::new(constructor_name, Namespace::Term), constructor);
    assert!(matches!(
        curried_parent
            .unit(&[curried_parent.base, curried_parent.derived])
            .class_hierarchy(&curried_parent.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            parent: Some(parent),
            issue: crate::JvmSuperclassIssue::ParentConstructorUnavailable,
        }) if class == curried_parent.derived && parent == curried_parent.base
    ));
}

impl ReferenceFixture {
    fn new() -> Self {
        let mut identity = ClassIdentityFixture::new(&["example"], "Counter");
        let definitions = Definitions::bootstrap(&mut identity.store);
        set_object_parent(
            &mut identity.store,
            identity.class,
            definitions.no_prefix,
            definitions.object_type,
        );
        let owner = identity.store.symbols.get(identity.class).owner.unwrap();
        let other = add_reference_class(
            &mut identity.store,
            owner,
            "Other",
            definitions.no_prefix,
            definitions.object_type,
            SymbolOrigin::Synthetic,
            SymbolFlags::EMPTY,
        );
        Self {
            store: identity.store,
            ast: TypedAst::new(),
            definitions,
            classes: vec![identity.class, other],
        }
    }

    fn unit(&self) -> JvmCompilationUnit<'_> {
        JvmCompilationUnit {
            store: &self.store,
            typed_ast: &self.ast,
            classes: self
                .classes
                .iter()
                .copied()
                .map(|class| JvmClassPlan {
                    class,
                    fields: Vec::new(),
                    methods: Vec::new(),
                    initialization: JvmClassInitializationPlan::Unsupported,
                })
                .collect(),
        }
    }

    fn type_ref(&mut self, class: SymbolId) -> dotty_core::TypeId {
        self.store.types.alloc(Type::TypeRef {
            prefix: self.definitions.no_prefix,
            target: TypeRefTarget::Symbol(class),
        })
    }
}

fn set_object_parent(
    store: &mut SemanticStore,
    class: SymbolId,
    prefix: dotty_core::TypeId,
    object_type: dotty_core::TypeId,
) {
    let info = match store.symbols.info(class) {
        SymbolInfo::Complete(ty) => match store.types.get(*ty) {
            Type::ClassInfo(info) => info.clone(),
            _ => panic!("class identity fixture should have ClassInfo"),
        },
        _ => panic!("class identity fixture should have complete info"),
    };
    let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix,
        parents: vec![object_type],
        ..info
    }));
    store
        .symbols
        .set_info(class, SymbolInfo::Complete(class_info));
}

fn add_reference_class(
    store: &mut SemanticStore,
    owner: SymbolId,
    name: &str,
    prefix: dotty_core::TypeId,
    object_type: dotty_core::TypeId,
    origin: SymbolOrigin,
    flags: SymbolFlags,
) -> SymbolId {
    let name_id = store.names.intern(name);
    let class = store.symbols.alloc(Symbol {
        name: Name::new(name_id, Namespace::Type),
        owner: Some(owner),
        kind: SymbolKind::Class,
        flags,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let declarations = store.scopes.alloc(Scope::new(Some(class)));
    let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix,
        class,
        parents: vec![object_type],
        declarations,
        self_type: None,
    }));
    store
        .symbols
        .set_info(class, SymbolInfo::Complete(class_info));
    class
}

#[test]
fn reference_projection_keeps_exact_planned_class_identity() {
    let mut fixture = ReferenceFixture::new();
    let counter = fixture.classes[0];
    let other = fixture.classes[1];
    let counter_type = fixture.type_ref(counter);
    let same_counter_type = fixture.type_ref(counter);
    let other_type = fixture.type_ref(other);
    let unit = fixture.unit();
    let store_checkpoint = fixture.store.checkpoint();
    let ast_checkpoint = fixture.ast.checkpoint();

    assert_eq!(
        unit.reference_type(counter_type, &fixture.definitions),
        Ok(JvmType::Reference(counter))
    );
    assert_eq!(
        unit.reference_type(same_counter_type, &fixture.definitions),
        Ok(JvmType::Reference(counter))
    );
    assert_eq!(
        unit.reference_type(other_type, &fixture.definitions),
        Ok(JvmType::Reference(other))
    );
    assert_ne!(
        JvmType::Reference(counter),
        JvmType::Reference(other),
        "different canonical class identities must remain distinct"
    );
    assert_eq!(
        JvmType::Reference(counter).category(),
        JvmValueCategory::Reference
    );
    assert_eq!(JvmType::Reference(counter).slot_width(), 1);
    assert_eq!(JvmType::Reference(counter).descriptor_char(), None);
    assert_ne!(JvmType::Reference(counter), JvmType::Int);
    assert_ne!(JvmType::Reference(counter), JvmType::Boolean);

    let counter_flow = JvmExpressionFlow::Value {
        value_type: JvmType::Reference(counter),
        category: JvmValueCategory::Reference,
    };
    let other_flow = JvmExpressionFlow::Value {
        value_type: JvmType::Reference(other),
        category: JvmValueCategory::Reference,
    };
    assert_ne!(counter_flow, other_flow);
    assert_eq!(fixture.store.checkpoint(), store_checkpoint);
    assert_eq!(fixture.ast.checkpoint(), ast_checkpoint);
}

#[test]
fn reference_projection_rejects_unplanned_stale_and_noncanonical_targets() {
    let mut fixture = ReferenceFixture::new();
    let counter = fixture.classes[0];
    let owner = fixture.store.symbols.get(counter).owner.unwrap();
    let unplanned = add_reference_class(
        &mut fixture.store,
        owner,
        "Unplanned",
        fixture.definitions.no_prefix,
        fixture.definitions.object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    let unplanned_type = fixture.type_ref(unplanned);
    let object_type = fixture.definitions.object_type;
    let non_class = alloc_symbol_named(
        &mut fixture.store,
        "notAClass",
        Namespace::Type,
        Some(owner),
        SymbolKind::Value,
        SymbolFlags::EMPTY,
    );
    let non_class_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(non_class),
    });
    let term_ref_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(counter),
    });
    let mut foreign_session = ReferenceFixture::new();
    let foreign_owner = foreign_session
        .store
        .symbols
        .get(foreign_session.classes[0])
        .owner
        .unwrap();
    for index in 0..64 {
        let name = format!("foreign{index}");
        alloc_symbol_named(
            &mut foreign_session.store,
            &name,
            Namespace::Term,
            Some(foreign_owner),
            SymbolKind::Value,
            SymbolFlags::EMPTY,
        );
    }
    let foreign_class = add_reference_class(
        &mut foreign_session.store,
        foreign_owner,
        "Foreign",
        foreign_session.definitions.no_prefix,
        foreign_session.definitions.object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    let foreign_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(foreign_class),
    });
    let counter_parent = fixture.type_ref(counter);
    let other_parent = fixture.type_ref(fixture.classes[1]);
    let union_type = fixture.store.types.alloc(Type::Or {
        left: counter_parent,
        right: other_parent,
    });
    let qualified_prefix = fixture.type_ref(counter);
    let unsupported_prefix_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: qualified_prefix,
        target: TypeRefTarget::Symbol(counter),
    });
    let prefix_checkpoint = fixture.store.checkpoint();
    let _reused_prefix_slot = fixture.store.types.alloc(Type::NoPrefix);
    let missing_prefix = fixture.store.types.alloc(Type::NoPrefix);
    fixture.store.rollback_to(prefix_checkpoint);
    let dangling_prefix_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: missing_prefix,
        target: TypeRefTarget::Symbol(counter),
    });
    let checkpoint = fixture.store.checkpoint();
    let stale_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(counter),
    });
    fixture.store.rollback_to(checkpoint);
    let unit = fixture.unit();

    assert_eq!(
        unit.reference_type(unplanned_type, &fixture.definitions),
        Err(JvmCodegenError::ReferenceClassNotInJvmPlan { class: unplanned })
    );
    assert_eq!(
        unit.reference_type(stale_type, &fixture.definitions),
        Err(JvmCodegenError::InvalidReferenceType {
            ty: stale_type,
            reason: InvalidTypeReason::MissingOrUnfilled,
        })
    );
    assert_eq!(
        unit.reference_type(dangling_prefix_type, &fixture.definitions),
        Err(JvmCodegenError::InvalidReferenceType {
            ty: missing_prefix,
            reason: InvalidTypeReason::MissingOrUnfilled,
        })
    );
    assert_eq!(
        unit.reference_type(unsupported_prefix_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: unsupported_prefix_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::UnsupportedPrefix,
        })
    );
    assert_eq!(
        unit.reference_type(object_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: object_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::ObjectClass,
        })
    );
    assert_eq!(
        unit.reference_type(non_class_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: non_class_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::UnsupportedSymbolKind,
        })
    );
    assert_eq!(
        unit.reference_type(term_ref_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: term_ref_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        })
    );
    assert_eq!(
        unit.reference_type(foreign_type, &fixture.definitions),
        Err(JvmCodegenError::ReferenceClassNotInJvmPlan {
            class: foreign_class,
        })
    );
    assert_eq!(
        unit.reference_type(union_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: union_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
        })
    );
    assert_eq!(
        unit.project_sibling_union_to_expected_type(
            union_type,
            counter_parent,
            &fixture.definitions
        ),
        Err(JvmCodegenError::UnsupportedSiblingUnionProjection {
            source_type: union_type,
            expected_type: counter_parent,
        }),
        "an Object-only shared root or one union arm is not an expected sibling base"
    );
}

#[test]
fn method_signature_projection_requires_exact_class_and_package_identity() {
    let mut fixture = ReferenceFixture::new();
    let counter = fixture.classes[0];
    let package = fixture.store.symbols.get(counter).owner.unwrap();
    let package_prefix = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(package),
    });
    let counter_signature = fixture.store.types.alloc(Type::TypeRef {
        prefix: package_prefix,
        target: TypeRefTarget::Symbol(counter),
    });
    let unsupported_prefix = fixture.store.types.alloc(Type::NoType);
    let wrong_prefix = fixture.store.types.alloc(Type::TypeRef {
        prefix: unsupported_prefix,
        target: TypeRefTarget::Symbol(counter),
    });
    let foreign_counter = add_reference_class(
        &mut fixture.store,
        package,
        "Counter",
        fixture.definitions.no_prefix,
        fixture.definitions.object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    let foreign_signature = fixture.store.types.alloc(Type::TypeRef {
        prefix: package_prefix,
        target: TypeRefTarget::Symbol(foreign_counter),
    });
    let no_prefix_signature = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(counter),
    });
    let unit = fixture.unit();
    let identities = crate::identity::unit_class_identities(&unit).unwrap();

    assert_eq!(
        crate::reference::project_method_signature_type(
            unit.store,
            counter_signature,
            &fixture.definitions,
            &identities,
            None,
        ),
        Ok(JvmType::Reference(counter))
    );
    assert_eq!(
        crate::reference::project_method_signature_type(
            unit.store,
            wrong_prefix,
            &fixture.definitions,
            &identities,
            None,
        ),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: wrong_prefix,
            kind: crate::UnsupportedJvmReferenceTypeKind::UnsupportedPrefix,
        })
    );
    assert_eq!(
        crate::reference::project_method_signature_type(
            unit.store,
            no_prefix_signature,
            &fixture.definitions,
            &identities,
            None,
        ),
        Ok(JvmType::Reference(counter))
    );
    assert_eq!(
        crate::reference::project_method_signature_type(
            unit.store,
            foreign_signature,
            &fixture.definitions,
            &identities,
            None,
        ),
        Err(JvmCodegenError::ReferenceClassNotInJvmPlan {
            class: foreign_counter,
        })
    );
}

#[test]
fn reference_projection_rejects_external_anonymous_generic_and_inherited_classes() {
    let mut fixture = ReferenceFixture::new();
    let owner = fixture.store.symbols.get(fixture.classes[0]).owner.unwrap();
    let object_type = fixture.definitions.object_type;
    let no_prefix = fixture.definitions.no_prefix;

    let classfile_origin = SymbolOrigin::Classfile(fixture.store.origins.register_classfile());
    let external = add_reference_class(
        &mut fixture.store,
        owner,
        "External",
        no_prefix,
        object_type,
        classfile_origin,
        SymbolFlags::EMPTY,
    );
    let external_type = fixture.type_ref(external);
    fixture.classes.push(external);
    let unit = fixture.unit();
    assert_eq!(
        unit.reference_type(external_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: external_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::ExternalClass,
        })
    );
    drop(unit);

    let mut fixture = ReferenceFixture::new();
    let owner = fixture.store.symbols.get(fixture.classes[0]).owner.unwrap();
    let anonymous = add_reference_class(
        &mut fixture.store,
        owner,
        "Anonymous",
        no_prefix,
        object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::SYNTHETIC,
    );
    let anonymous_type = fixture.type_ref(anonymous);
    fixture.classes.push(anonymous);
    let unit = fixture.unit();
    assert_eq!(
        unit.reference_type(anonymous_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: anonymous_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::AnonymousClass,
        })
    );
    drop(unit);

    let mut fixture = ReferenceFixture::new();
    let generic = fixture.classes[1];
    let info = match fixture.store.symbols.info(generic) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.clone(),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let type_parameter = alloc_symbol_named(
        &mut fixture.store,
        "T",
        Namespace::Type,
        Some(generic),
        SymbolKind::TypeParameter,
        SymbolFlags::EMPTY,
    );
    fixture.store.scopes.get_mut(info.declarations).enter(
        fixture.store.symbols.get(type_parameter).name,
        type_parameter,
    );
    let generic_type = fixture.type_ref(generic);
    let unit = fixture.unit();
    assert_eq!(
        unit.reference_type(generic_type, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            ty: generic_type,
            kind: crate::UnsupportedJvmReferenceTypeKind::GenericClass,
        })
    );
    drop(unit);

    let mut fixture = ReferenceFixture::new();
    let inherited = fixture.classes[1];
    let info = match fixture.store.symbols.info(inherited) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.clone(),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let inherited_info = fixture.store.types.alloc(Type::ClassInfo(ClassInfo {
        parents: vec![fixture.definitions.boolean],
        ..info
    }));
    fixture
        .store
        .symbols
        .set_info(inherited, SymbolInfo::Complete(inherited_info));
    let inherited_type = fixture.type_ref(inherited);
    let unit = fixture.unit();
    assert!(matches!(
        unit.reference_type(inherited_type, &fixture.definitions),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class,
            issue: crate::JvmSuperclassIssue::ParentNotPlanned,
            ..
        }) if class == inherited
    ));
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
            fields: Vec::new(),
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
        fields: Vec::new(),
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
fn local_layout_allocates_after_parameters_by_canonical_identity() {
    let mut fixture = EmissionFixture::new(false);
    let method = fixture.methods[1];
    let mut bindings = Vec::new();
    let mut body = fixture.bodies[1];

    for (mutable, ty) in [
        (false, fixture.definitions.int),
        (true, fixture.definitions.boolean),
    ] {
        let name = fixture.store.names.intern("same");
        let symbol = fixture.store.symbols.alloc(Symbol {
            name: Name::new(name, Namespace::Term),
            owner: Some(method),
            kind: SymbolKind::Local,
            flags: if mutable {
                SymbolFlags::MUTABLE
            } else {
                SymbolFlags::EMPTY
            },
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(ty),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let declaration = fixture.ast.alloc(Tree {
            kind: TreeKind::ValDef(ValDef {
                name: TermName::new(name),
                tpt: body,
                rhs: Some(body),
                metadata: (),
            }),
            position: None,
            ty,
        });
        body = fixture.ast.alloc(Tree {
            kind: TreeKind::Block(Block {
                stats: vec![declaration],
                expr: body,
            }),
            position: None,
            ty: fixture.definitions.int,
        });
        bindings.push(JvmLocalBinding {
            declaration,
            symbol,
            mutable,
        });
    }
    fixture.bodies[1] = body;

    let mut unit = fixture.unit();
    unit.classes[0].methods[1].locals = bindings.clone();
    let layout = unit
        .local_layout(fixture.class, method, &fixture.definitions)
        .unwrap();
    let metadata = unit.metadata(&fixture.definitions).unwrap();
    let method_metadata = &metadata[0].methods()[1];
    assert_eq!(
        unit.local_layout_for(method_metadata, &bindings, &fixture.definitions)
            .unwrap(),
        layout
    );
    assert_eq!(
        unit.local_layout_for(method_metadata, &[], &fixture.definitions),
        Err(JvmCodegenError::LocalLayoutInputMismatch { method })
    );
    assert_eq!(layout.method(), method);
    assert_eq!(layout.max_locals(), 6);
    assert_eq!(
        layout
            .locals()
            .iter()
            .map(|local| (local.symbol(), local.slot(), local.ty(), local.is_mutable()))
            .collect::<Vec<_>>(),
        vec![
            (bindings[0].symbol, 4, JvmType::Int, false),
            (bindings[1].symbol, 5, JvmType::Boolean, true),
        ]
    );
    assert_eq!(
        layout.local(bindings[0].symbol).unwrap().declaration(),
        bindings[0].declaration
    );
    assert_eq!(layout.local(bindings[1].symbol).unwrap().slot(), 5);
    assert_eq!(
        layout.local(fixture.parameters[1][0]).unwrap_err(),
        JvmCodegenError::UnknownLocalSlot {
            method,
            symbol: fixture.parameters[1][0],
        }
    );
    assert_eq!(
        layout,
        unit.local_layout(fixture.class, method, &fixture.definitions)
            .unwrap()
    );
}

#[test]
fn methods_without_locals_keep_receiver_and_parameter_layout_unchanged() {
    let fixture = EmissionFixture::new(false);
    let layout = fixture
        .unit()
        .local_layout(fixture.class, fixture.methods[1], &fixture.definitions)
        .unwrap();
    assert!(layout.locals().is_empty());
    assert_eq!(layout.max_locals(), 4);
}

#[test]
fn local_layout_rejects_unsupported_semantic_types() {
    let mut fixture = EmissionFixture::new(false);
    let method = fixture.methods[0];
    let name = fixture.store.names.intern("reference");
    let symbol = fixture.store.symbols.alloc(Symbol {
        name: Name::new(name, Namespace::Term),
        owner: Some(method),
        kind: SymbolKind::Local,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(fixture.definitions.object_type),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let body = fixture.bodies[0];
    let declaration = fixture.ast.alloc(Tree {
        kind: TreeKind::ValDef(ValDef {
            name: TermName::new(name),
            tpt: body,
            rhs: Some(body),
            metadata: (),
        }),
        position: None,
        ty: fixture.definitions.object_type,
    });
    fixture.bodies[0] = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![declaration],
            expr: body,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let binding = JvmLocalBinding {
        declaration,
        symbol,
        mutable: false,
    };
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals.push(binding);

    assert!(matches!(
        unit.local_layout(fixture.class, method, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmReferenceType {
            kind: crate::UnsupportedJvmReferenceTypeKind::ObjectClass,
            ..
        })
    ));
}

#[test]
fn local_layout_reserves_exact_category_one_slots_for_planned_references() {
    let mut fixture = EmissionFixture::new(false);
    let class = fixture.class;
    let method = fixture.methods[0];
    let reference_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(class),
    });
    let name = fixture.store.names.intern("other");
    let symbol = fixture.store.symbols.alloc(Symbol {
        name: Name::new(name, Namespace::Term),
        owner: Some(method),
        kind: SymbolKind::Local,
        flags: SymbolFlags::MUTABLE,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(reference_type),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let old_body = fixture.bodies[0];
    let declaration = fixture.ast.alloc(Tree {
        kind: TreeKind::ValDef(ValDef {
            name: TermName::new(name),
            tpt: old_body,
            rhs: Some(old_body),
            metadata: (),
        }),
        position: None,
        ty: reference_type,
    });
    fixture.bodies[0] = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![declaration],
            expr: old_body,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let binding = JvmLocalBinding {
        declaration,
        symbol,
        mutable: true,
    };
    let mut unit = fixture.unit();
    unit.classes[0].methods[0].locals = vec![binding];

    let layout = unit
        .local_layout(class, method, &fixture.definitions)
        .unwrap();
    assert_eq!(layout.max_locals(), 2);
    assert_eq!(
        layout.local(symbol).unwrap().ty(),
        JvmType::Reference(class)
    );
    assert_eq!(layout.local(symbol).unwrap().slot(), 1);
    assert_eq!(layout.local(symbol).unwrap().ty().slot_width(), 1);
    assert!(layout.local(symbol).unwrap().is_mutable());
    assert_eq!(
        unit.local_value_type(symbol, reference_type, &fixture.definitions, None),
        Ok(JvmType::Reference(class))
    );
}

#[test]
fn reference_local_layout_keeps_same_named_symbols_tied_to_distinct_classes() {
    let mut fixture = ReferenceFixture::new();
    let owner_class = fixture.classes[0];
    let first_class = fixture.classes[0];
    let second_class = fixture.classes[1];
    let declarations = match fixture.store.symbols.info(owner_class) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("planned reference class should have ClassInfo"),
        },
        _ => panic!("planned reference class should be complete"),
    };
    let (method, _) = add_emission_method(
        &mut fixture.store,
        owner_class,
        declarations,
        "locals",
        &[],
        fixture.definitions.int,
    );
    let int_body = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let mut bindings = Vec::new();
    let mut declarations = Vec::new();
    for class in [first_class, second_class] {
        let reference_type = fixture.type_ref(class);
        let name = fixture.store.names.intern("same");
        let symbol = fixture.store.symbols.alloc(Symbol {
            name: Name::new(name, Namespace::Term),
            owner: Some(method),
            kind: SymbolKind::Local,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(reference_type),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let declaration = fixture.ast.alloc(Tree {
            kind: TreeKind::ValDef(ValDef {
                name: TermName::new(name),
                tpt: int_body,
                rhs: Some(int_body),
                metadata: (),
            }),
            position: None,
            ty: reference_type,
        });
        bindings.push(JvmLocalBinding {
            declaration,
            symbol,
            mutable: false,
        });
        declarations.push(declaration);
    }
    let nested_body = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![declarations[1]],
            expr: int_body,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let body = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![declarations[0]],
            expr: nested_body,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let mut unit = fixture.unit();
    unit.classes[0].methods.push(JvmMethodPlan {
        method,
        parameters: Vec::new(),
        locals: bindings.clone(),
        body: Some(body),
    });

    let layout = unit
        .local_layout(owner_class, method, &fixture.definitions)
        .unwrap();
    assert_eq!(layout.max_locals(), 3);
    assert_ne!(bindings[0].symbol, bindings[1].symbol);
    assert_eq!(
        bindings
            .iter()
            .map(|binding| {
                let local = layout.local(binding.symbol).unwrap();
                (local.slot(), local.ty())
            })
            .collect::<Vec<_>>(),
        vec![
            (1, JvmType::Reference(first_class)),
            (2, JvmType::Reference(second_class)),
        ]
    );
}

#[test]
fn reference_local_read_before_store_returns_a_typed_error() {
    let mut fixture = ReferenceFixture::new();
    let class = fixture.classes[0];
    let declarations = match fixture.store.symbols.info(class) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("planned reference class should have ClassInfo"),
        },
        _ => panic!("planned reference class should be complete"),
    };
    let (method, _) = add_emission_method(
        &mut fixture.store,
        class,
        declarations,
        "readBeforeStore",
        &[],
        fixture.definitions.int,
    );
    let reference_type = fixture.type_ref(class);
    let name = fixture.store.names.intern("counter");
    let local = fixture.store.symbols.alloc(Symbol {
        name: Name::new(name, Namespace::Term),
        owner: Some(method),
        kind: SymbolKind::Local,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(reference_type),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let local_reference = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(local),
    });
    let read = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: Name::new(name, Namespace::Term),
            backquoted: false,
        }),
        position: None,
        ty: local_reference,
    });
    let declaration = fixture.ast.alloc(Tree {
        kind: TreeKind::ValDef(ValDef {
            name: TermName::new(name),
            tpt: read,
            rhs: Some(read),
            metadata: (),
        }),
        position: None,
        ty: reference_type,
    });
    let result = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let body = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![declaration],
            expr: result,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let binding = JvmLocalBinding {
        declaration,
        symbol: local,
        mutable: false,
    };
    let mut unit = fixture.unit();
    unit.classes[0].methods.push(JvmMethodPlan {
        method,
        parameters: Vec::new(),
        locals: vec![binding],
        body: Some(body),
    });

    assert!(matches!(
        unit.lower_method_body(class, method, &fixture.definitions),
        Err(JvmCodegenError::UninitializedLocalRead {
            tree: found_tree,
            symbol: found_symbol,
        }) if found_tree == read && found_symbol == local
    ));
}

#[test]
fn assignments_reject_immutable_nonlocal_and_mistyped_targets_without_class_output() {
    let emit_case =
        |mutable: bool, parameter_target: bool, invalid_lhs: bool, boolean_rhs: bool| {
            let mut fixture = EmissionFixture::new(false);
            let method_index = 1;
            let int_ty = fixture.definitions.int;
            let local = add_emission_local_symbol(
                &mut fixture,
                method_index,
                "current",
                int_ty,
                mutable,
                SymbolFlags::EMPTY,
            );
            let initial_value = fixture.ast.alloc(Tree {
                kind: TreeKind::Literal(Literal {
                    value: Constant::Int(1),
                }),
                position: None,
                ty: int_ty,
            });
            let declaration = add_emission_local_valdef(
                &mut fixture,
                method_index,
                local,
                int_ty,
                Some(initial_value),
            );
            let local_binding = JvmLocalBinding {
                declaration,
                symbol: local,
                mutable,
            };
            let lhs = if invalid_lhs {
                initial_value
            } else if parameter_target {
                let parameter = fixture.parameters[method_index][1];
                typed_emission_symbol_ident(&mut fixture, parameter)
            } else {
                typed_emission_symbol_ident(&mut fixture, local)
            };
            let (rhs_value, rhs_ty) = if boolean_rhs {
                (Constant::Boolean(false), fixture.definitions.boolean)
            } else {
                (Constant::Int(2), int_ty)
            };
            let rhs = fixture.ast.alloc(Tree {
                kind: TreeKind::Literal(Literal { value: rhs_value }),
                position: None,
                ty: rhs_ty,
            });
            let assignment = fixture.ast.alloc(Tree {
                kind: TreeKind::Assign(dotty_core::ast::Assign { lhs, rhs }),
                position: None,
                ty: fixture.definitions.unit,
            });
            let result = typed_emission_symbol_ident(&mut fixture, local);
            fixture.bodies[method_index] =
                typed_block(&mut fixture, vec![declaration, assignment], result, int_ty);
            let mut unit = fixture.unit();
            unit.classes[0].methods[method_index].locals = vec![local_binding];
            emit_class(&unit, fixture.class, &fixture.definitions)
        };

    assert!(matches!(
        emit_case(false, false, false, false),
        Err(JvmCodegenError::ImmutableLocalAssignment { .. })
    ));
    assert!(matches!(
        emit_case(true, true, false, false),
        Err(JvmCodegenError::InvalidAssignmentTarget { .. })
    ));
    assert!(matches!(
        emit_case(true, false, false, true),
        Err(JvmCodegenError::AssignmentValueFlowMismatch { .. })
    ));
    assert!(matches!(
        emit_case(true, false, true, false),
        Err(JvmCodegenError::InvalidAssignmentTarget { .. })
    ));
}

#[test]
fn loop_body_initialization_is_not_assumed_after_a_zero_iteration_path() {
    let mut fixture = EmissionFixture::new(false);
    let method_index = 1;
    let int_ty = fixture.definitions.int;
    let unit_ty = fixture.definitions.unit;
    let local = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "insideLoop",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int_ty,
    });
    let declaration =
        add_emission_local_valdef(&mut fixture, method_index, local, int_ty, Some(initializer));
    let unit_value = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Unit,
        }),
        position: None,
        ty: unit_ty,
    });
    let loop_body = typed_block(&mut fixture, vec![declaration], unit_value, unit_ty);
    let false_condition = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(false),
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });
    let loop_tree = fixture.ast.alloc(Tree {
        kind: TreeKind::While(dotty_core::ast::While {
            cond: false_condition,
            body: loop_body,
        }),
        position: None,
        ty: unit_ty,
    });
    let result = typed_emission_symbol_ident(&mut fixture, local);
    fixture.bodies[method_index] = typed_block(&mut fixture, vec![loop_tree], result, int_ty);

    let mut unit = fixture.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol: local,
        mutable: false,
    }];
    assert!(matches!(
        emit_class(&unit, fixture.class, &fixture.definitions),
        Err(JvmCodegenError::UninitializedLocalRead { tree, symbol })
            if tree == result && symbol == local
    ));
}

#[test]
fn emits_local_initializers_once_in_declaration_order_with_canonical_slots() {
    let mut fixture = EmissionFixture::new(false);
    let method_index = 1;
    let int_ty = fixture.definitions.int;
    let parameter = fixture.parameters[method_index][1];
    let parameter_ty = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(parameter),
    });
    let parameter_name = fixture.store.symbols.get(parameter).name;
    let parameter_ref = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: parameter_name,
            backquoted: false,
        }),
        position: None,
        ty: parameter_ty,
    });
    let first_symbol = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "saved",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let first_decl = add_emission_local_valdef(
        &mut fixture,
        method_index,
        first_symbol,
        int_ty,
        Some(parameter_ref),
    );
    let second_symbol = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "current",
        int_ty,
        true,
        SymbolFlags::EMPTY,
    );
    let initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let second_decl = add_emission_local_valdef(
        &mut fixture,
        method_index,
        second_symbol,
        int_ty,
        Some(initializer),
    );
    let final_expr = fixture.bodies[method_index];
    fixture.bodies[method_index] = typed_block(
        &mut fixture,
        vec![first_decl, second_decl],
        final_expr,
        int_ty,
    );

    let mut unit = fixture.unit();
    unit.classes[0].methods[method_index].locals = vec![
        JvmLocalBinding {
            declaration: first_decl,
            symbol: first_symbol,
            mutable: false,
        },
        JvmLocalBinding {
            declaration: second_decl,
            symbol: second_symbol,
            mutable: true,
        },
    ];
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(
        &code[..7],
        &[0x1c, 0x36, 0x04, 0x10, 42, 0x36, 0x05],
        "parameter load/store and constant/store should each occur once in declaration order"
    );
    assert_eq!(code[7], 0x1b, "the following source body starts afterward");
    let select = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("select"))
        .unwrap();
    let Attribute::Code(select_code) = &select.attributes[0] else {
        panic!("select method has no Code attribute")
    };
    assert_eq!(select_code.max_locals, 6);
}

#[test]
fn loads_initialized_local_by_symbol_slot_when_it_shadows_a_parameter() {
    let mut fixture = EmissionFixture::new(false);
    let method_index = 1;
    let int_ty = fixture.definitions.int;
    let parameter = fixture.parameters[method_index][1];
    let initializer = typed_emission_symbol_ident(&mut fixture, parameter);
    let local = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "left",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let declaration =
        add_emission_local_valdef(&mut fixture, method_index, local, int_ty, Some(initializer));
    let local_ref = typed_emission_symbol_ident(&mut fixture, local);
    fixture.bodies[method_index] = typed_block(&mut fixture, vec![declaration], local_ref, int_ty);

    let mut unit = fixture.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol: local,
        mutable: false,
    }];
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let repeated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    assert_eq!(generated.bytes(), repeated.bytes());
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(
        code,
        &[0x1c, 0x36, 0x04, 0x15, 0x04, 0xac],
        "load parameter slot two, store local slot four, then load that exact local slot"
    );
}

#[test]
fn nested_same_named_local_references_keep_their_canonical_slots() {
    let mut fixture = EmissionFixture::new(false);
    let method_index = 1;
    let int_ty = fixture.definitions.int;
    let parameter = fixture.parameters[method_index][1];
    let outer_initializer = typed_emission_symbol_ident(&mut fixture, parameter);
    let outer = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "value",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let outer_declaration = add_emission_local_valdef(
        &mut fixture,
        method_index,
        outer,
        int_ty,
        Some(outer_initializer),
    );
    let inner_initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: int_ty,
    });
    let inner = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "value",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let inner_declaration = add_emission_local_valdef(
        &mut fixture,
        method_index,
        inner,
        int_ty,
        Some(inner_initializer),
    );
    let inner_ref = typed_emission_symbol_ident(&mut fixture, inner);
    let nested = typed_block(&mut fixture, vec![inner_declaration], inner_ref, int_ty);
    fixture.bodies[method_index] =
        typed_block(&mut fixture, vec![outer_declaration], nested, int_ty);

    let mut unit = fixture.unit();
    unit.classes[0].methods[method_index].locals = vec![
        JvmLocalBinding {
            declaration: outer_declaration,
            symbol: outer,
            mutable: false,
        },
        JvmLocalBinding {
            declaration: inner_declaration,
            symbol: inner,
            mutable: false,
        },
    ];
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(
        code,
        &[0x1c, 0x36, 0x04, 0x10, 42, 0x36, 0x05, 0x15, 0x05, 0xac],
        "same-text declarations use distinct slots and the inner symbol loads slot five"
    );
}

#[test]
fn rejects_local_identifiers_without_a_valid_method_binding() {
    let mut missing = EmissionFixture::new(false);
    let method_index = 1;
    let int_ty = missing.definitions.int;
    let local = add_emission_local_symbol(
        &mut missing,
        method_index,
        "unplanned",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let local_ref = typed_emission_symbol_ident(&mut missing, local);
    missing.bodies[method_index] = local_ref;
    assert_eq!(
        missing.emit(),
        Err(JvmCodegenError::MissingLocalBinding {
            tree: local_ref,
            symbol: local,
        })
    );

    let mut foreign = EmissionFixture::new(false);
    let int_ty = foreign.definitions.int;
    let local = add_emission_local_symbol(
        &mut foreign,
        0,
        "foreign",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let local_ref = typed_emission_symbol_ident(&mut foreign, local);
    foreign.bodies[1] = local_ref;
    assert_eq!(
        foreign.emit(),
        Err(JvmCodegenError::InvalidLocalReference {
            tree: local_ref,
            symbol: local,
        })
    );
}

#[test]
fn rejects_local_reads_when_a_control_flow_join_has_an_uninitialized_path() {
    let mut fixture = EmissionFixture::new(false);
    let method_index = 1;
    let int_ty = fixture.definitions.int;
    let unit_ty = fixture.definitions.unit;
    let condition_symbol = fixture.parameters[method_index][0];
    let condition = typed_emission_symbol_ident(&mut fixture, condition_symbol);
    let local = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "conditional",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int_ty,
    });
    let declaration =
        add_emission_local_valdef(&mut fixture, method_index, local, int_ty, Some(initializer));
    let unit_value = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Unit,
        }),
        position: None,
        ty: unit_ty,
    });
    let then_branch = typed_block(&mut fixture, vec![declaration], unit_value, unit_ty);
    let else_branch = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Unit,
        }),
        position: None,
        ty: unit_ty,
    });
    let conditional = fixture.ast.alloc(Tree {
        kind: TreeKind::If(If {
            cond: condition,
            then_branch,
            else_branch,
        }),
        position: None,
        ty: unit_ty,
    });
    let local_ref = typed_emission_symbol_ident(&mut fixture, local);
    fixture.bodies[method_index] = typed_block(&mut fixture, vec![conditional], local_ref, int_ty);

    let mut unit = fixture.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol: local,
        mutable: false,
    }];
    assert_eq!(
        emit_class(&unit, fixture.class, &fixture.definitions),
        Err(JvmCodegenError::UninitializedLocalRead {
            tree: local_ref,
            symbol: local,
        })
    );
}

#[test]
fn rejects_missing_initializer_type_mismatch_and_self_reference() {
    let mut missing = EmissionFixture::new(false);
    let method_index = 0;
    let missing_int = missing.definitions.int;
    let symbol = add_emission_local_symbol(
        &mut missing,
        method_index,
        "missing",
        missing_int,
        false,
        SymbolFlags::EMPTY,
    );
    let declaration =
        add_emission_local_valdef(&mut missing, method_index, symbol, missing_int, None);
    let final_expr = missing.bodies[method_index];
    missing.bodies[method_index] =
        typed_block(&mut missing, vec![declaration], final_expr, missing_int);
    let mut unit = missing.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol,
        mutable: false,
    }];
    assert_eq!(
        emit_class(&unit, missing.class, &missing.definitions),
        Err(JvmCodegenError::MissingLocalInitializer {
            method: missing.methods[method_index],
            declaration,
        })
    );

    let mut mismatch = EmissionFixture::new(false);
    let mismatch_int = mismatch.definitions.int;
    let symbol = add_emission_local_symbol(
        &mut mismatch,
        method_index,
        "wrongType",
        mismatch_int,
        false,
        SymbolFlags::EMPTY,
    );
    let boolean_rhs = mismatch.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: mismatch.definitions.boolean,
    });
    let declaration = add_emission_local_valdef(
        &mut mismatch,
        method_index,
        symbol,
        mismatch_int,
        Some(boolean_rhs),
    );
    let final_expr = mismatch.bodies[method_index];
    mismatch.bodies[method_index] =
        typed_block(&mut mismatch, vec![declaration], final_expr, mismatch_int);
    let mut unit = mismatch.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol,
        mutable: false,
    }];
    assert!(matches!(
        emit_class(&unit, mismatch.class, &mismatch.definitions),
        Err(JvmCodegenError::LocalInitializerFlowMismatch {
            method: found_method,
            declaration: found_declaration,
            symbol: found_symbol,
            expected: JvmType::Int,
            found: JvmExpressionFlow::Value {
                value_type: JvmType::Boolean,
                ..
            },
        }) if found_method == mismatch.methods[method_index]
            && found_declaration == declaration
            && found_symbol == symbol
    ));

    let mut recursive = EmissionFixture::new(false);
    let recursive_int = recursive.definitions.int;
    let symbol = add_emission_local_symbol(
        &mut recursive,
        method_index,
        "recursive",
        recursive_int,
        false,
        SymbolFlags::EMPTY,
    );
    let recursive_type = recursive.store.types.alloc(Type::TermRef {
        prefix: recursive.definitions.no_prefix,
        target: TermRefTarget::Symbol(symbol),
    });
    let recursive_ref = recursive.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: recursive.store.symbols.get(symbol).name,
            backquoted: false,
        }),
        position: None,
        ty: recursive_type,
    });
    let declaration = add_emission_local_valdef(
        &mut recursive,
        method_index,
        symbol,
        recursive_int,
        Some(recursive_ref),
    );
    let final_expr = recursive.bodies[method_index];
    recursive.bodies[method_index] =
        typed_block(&mut recursive, vec![declaration], final_expr, recursive_int);
    let mut unit = recursive.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol,
        mutable: false,
    }];
    assert_eq!(
        emit_class(&unit, recursive.class, &recursive.definitions),
        Err(JvmCodegenError::UninitializedLocalRead {
            tree: recursive_ref,
            symbol,
        })
    );
}

#[test]
fn lowers_conditional_initializer_before_its_single_store() {
    let mut fixture = EmissionFixture::new(false);
    let method_index = 1;
    let int_ty = fixture.definitions.int;
    let condition_symbol = fixture.parameters[method_index][0];
    let condition_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(condition_symbol),
    });
    let condition = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: fixture.store.symbols.get(condition_symbol).name,
            backquoted: false,
        }),
        position: None,
        ty: condition_type,
    });
    let then_value = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(11),
        }),
        position: None,
        ty: int_ty,
    });
    let else_value = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(12),
        }),
        position: None,
        ty: int_ty,
    });
    let initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::If(If {
            cond: condition,
            then_branch: then_value,
            else_branch: else_value,
        }),
        position: None,
        ty: int_ty,
    });
    let symbol = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "selected",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let declaration = add_emission_local_valdef(
        &mut fixture,
        method_index,
        symbol,
        int_ty,
        Some(initializer),
    );
    let final_expr = fixture.bodies[method_index];
    fixture.bodies[method_index] = typed_block(&mut fixture, vec![declaration], final_expr, int_ty);
    let mut unit = fixture.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol,
        mutable: false,
    }];

    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(code[0], 0x1b, "condition is evaluated first");
    assert_eq!(
        code.windows(2)
            .filter(|instruction| *instruction == [0x36, 0x04])
            .count(),
        1,
        "conditional initializer stores once to local slot four"
    );
    let select = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok("select"))
        .unwrap();
    assert!(select.attributes.iter().any(|attribute| matches!(attribute,
        Attribute::Code(code) if code.attributes.iter().any(|nested| matches!(nested, Attribute::Other(raw)
            if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable"))))));
}

#[test]
fn rejects_inline_local_modifiers_before_lowering() {
    let mut fixture = EmissionFixture::new(false);
    let method_index = 0;
    let int_ty = fixture.definitions.int;
    let initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int_ty,
    });
    let symbol = add_emission_local_symbol(
        &mut fixture,
        method_index,
        "inlineValue",
        int_ty,
        false,
        SymbolFlags::INLINE,
    );
    let declaration = add_emission_local_valdef(
        &mut fixture,
        method_index,
        symbol,
        int_ty,
        Some(initializer),
    );
    let final_expr = fixture.bodies[method_index];
    fixture.bodies[method_index] = typed_block(&mut fixture, vec![declaration], final_expr, int_ty);
    let mut unit = fixture.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol,
        mutable: false,
    }];
    assert_eq!(
        emit_class(&unit, fixture.class, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedLocalFlag {
            symbol,
            flag: UnsupportedJvmLocalFlag::Inline,
        })
    );
}

#[test]
fn local_slot_u16_boundary_does_not_publish_an_overflow_assignment() {
    let fixture = Fixture::new();
    let binding = JvmLocalBinding {
        declaration: fixture.body,
        symbol: fixture.parameter,
        mutable: false,
    };
    let mut allocator = JvmLocalAllocator::empty(fixture.method);
    allocator.next_slot = u16::MAX as usize - 1;

    allocator.reserve(binding, JvmType::Int).unwrap();
    assert_eq!(allocator.locals.last().unwrap().slot(), u16::MAX - 1);
    assert_eq!(allocator.next_slot, u16::MAX as usize);

    let before = allocator.locals.clone();
    assert_eq!(
        allocator.reserve(binding, JvmType::Int),
        Err(JvmCodegenError::TooManyLocalSlots {
            method: fixture.method,
        })
    );
    assert_eq!(allocator.locals, before);
    assert_eq!(allocator.by_symbol.get(&binding.symbol), Some(&0));
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
            fields: Vec::new(),
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
                fields: Vec::new(),
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
    let private_metadata = JvmClassPlan {
        class: nonpublic.class,
        fields: Vec::new(),
        methods: vec![nonpublic.plan.clone()],
        initialization: JvmClassInitializationPlan::Unsupported,
    }
    .metadata(&nonpublic.store, &nonpublic.definitions)
    .expect("plain private methods have valid JVM metadata");
    assert!(private_metadata.methods()[0].access_flags().is_private());

    let mut final_method = SignatureFixture::new(0);
    prepare_signature_class(&mut final_method, "Answer");
    final_method
        .store
        .symbols
        .get_mut(final_method.method)
        .flags = SymbolFlags::FINAL;
    let final_metadata = JvmClassPlan {
        class: final_method.class,
        fields: Vec::new(),
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
        locals: Vec::new(),
        body: None,
    };

    assert_eq!(
        JvmClassPlan {
            class: fixture.class,
            fields: Vec::new(),
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
        locals: Vec::new(),
        body: None,
    };

    assert_eq!(
        JvmClassPlan {
            class: fixture.class,
            fields: Vec::new(),
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
                fields: Vec::new(),
                methods: Vec::new(),
                initialization: JvmClassInitializationPlan::Unsupported,
            },
            JvmClassPlan {
                class: duplicate,
                fields: Vec::new(),
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

    let mut reference_result = SignatureFixture::new(0);
    let class = alloc_symbol(
        &mut reference_result.store,
        SymbolKind::Class,
        None,
        reference_result.definitions.int,
    );
    let reference = reference_result.store.types.alloc(Type::type_ref(
        reference_result.definitions.no_prefix,
        class,
    ));
    let method_type = reference_result.store.types.alloc(Type::Method(MethodType {
        params: Vec::new(),
        result: reference,
        kind: MethodKind::Plain,
    }));
    reference_result
        .store
        .symbols
        .set_info(reference_result.method, SymbolInfo::Complete(method_type));
    assert_eq!(
        reference_result.descriptor(),
        Err(JvmCodegenError::UnsupportedJvmType {
            symbol: reference_result.method,
            ty: reference,
            kind: UnsupportedJvmTypeKind::ReferenceClass,
        })
    );
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
                locals: Vec::new(),
                body: Some(body),
            })
            .collect();
        JvmCompilationUnit {
            store: &self.store,
            typed_ast: &self.ast,
            classes: vec![JvmClassPlan {
                class: self.class,
                fields: Vec::new(),
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

fn typed_block(
    fixture: &mut EmissionFixture,
    stats: Vec<TreeId<dotty_core::ast::Typed>>,
    expr: TreeId<dotty_core::ast::Typed>,
    ty: dotty_core::TypeId,
) -> TreeId<dotty_core::ast::Typed> {
    fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block { stats, expr }),
        position: None,
        ty,
    })
}

fn add_emission_local_symbol(
    fixture: &mut EmissionFixture,
    method_index: usize,
    text: &str,
    ty: dotty_core::TypeId,
    mutable: bool,
    extra_flags: SymbolFlags,
) -> SymbolId {
    let name_id = fixture.store.names.intern(text);
    fixture.store.symbols.alloc(Symbol {
        name: Name::new(name_id, Namespace::Term),
        owner: Some(fixture.methods[method_index]),
        kind: SymbolKind::Local,
        flags: if mutable {
            extra_flags | SymbolFlags::MUTABLE
        } else {
            extra_flags
        },
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(ty),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    })
}

fn add_emission_local_valdef(
    fixture: &mut EmissionFixture,
    method_index: usize,
    symbol: SymbolId,
    ty: dotty_core::TypeId,
    rhs: Option<TreeId<dotty_core::ast::Typed>>,
) -> TreeId<dotty_core::ast::Typed> {
    let name = fixture.store.symbols.get(symbol).name;
    fixture.ast.alloc(Tree {
        kind: TreeKind::ValDef(ValDef {
            name: TermName::new(name.text()),
            tpt: fixture.bodies[method_index],
            rhs,
            metadata: (),
        }),
        position: None,
        ty,
    })
}

fn typed_emission_symbol_ident(
    fixture: &mut EmissionFixture,
    symbol: SymbolId,
) -> TreeId<dotty_core::ast::Typed> {
    let name = fixture.store.symbols.get(symbol).name;
    let ty = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(symbol),
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

fn encode_receiver_code(code: dotty_classfile::bytecode::CodeBuilder) -> Vec<u8> {
    let mut class = ClassFileBuilder::new(
        "example/Receiver",
        ClassFileVersion {
            major: 65,
            minor: 0,
        },
    )
    .unwrap();
    class
        .add_instance_method("receiver", "()Ljava/lang/Object;", |output| {
            *output = code;
            output.areturn()?;
            Ok(())
        })
        .unwrap();
    let bytes = class.encode().unwrap();
    let mut reader = Reader::new(&bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    class.methods[0]
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(code.code.to_vec()),
            _ => None,
        })
        .unwrap()
}

fn emitted_method_code<'a>(class: &'a ClassFile, name: &str) -> &'a [u8] {
    let method = class
        .methods
        .iter()
        .find(|method| class.constant_pool.utf8(method.name_index) == Ok(name))
        .unwrap();
    method
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            Attribute::Code(code) => Some(&code.code),
            _ => None,
        })
        .unwrap()
}

fn assert_virtual_method_ref(
    class: &ClassFile,
    code: &[u8],
    opcode_offset: usize,
    owner: &str,
    method: &str,
    descriptor: &str,
) {
    assert_eq!(code[opcode_offset], 0xb6);
    let index = u16::from_be_bytes([code[opcode_offset + 1], code[opcode_offset + 2]]);
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class.constant_pool.get(ConstantPoolIndex(index)).unwrap()
    else {
        panic!("invokevirtual must reference a Methodref constant")
    };
    assert_eq!(class.constant_pool.class_name(*class_index), Ok(owner));
    assert_eq!(
        class.constant_pool.name_and_type(*name_and_type_index),
        Ok((method, descriptor))
    );
}

fn set_method_body_to_parameter(
    fixture: &mut EmissionFixture,
    method: SymbolId,
    parameter: SymbolId,
) {
    let method_index = fixture
        .methods
        .iter()
        .position(|candidate| *candidate == method)
        .unwrap();
    let prefix = fixture.definitions.no_prefix;
    let reference = fixture.store.types.alloc(Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(parameter),
    });
    let body = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: fixture.store.symbols.get(parameter).name,
            backquoted: false,
        }),
        position: None,
        ty: reference,
    });
    fixture.bodies[method_index] = body;
}

fn typed_call_to(
    fixture: &mut EmissionFixture,
    method: SymbolId,
    args: Vec<TreeId<dotty_core::ast::Typed>>,
    result: dotty_core::TypeId,
    explicit_this: bool,
) -> TreeId<dotty_core::ast::Typed> {
    let function = if explicit_this {
        let this_type = fixture.store.types.alloc(Type::ThisType {
            class: fixture.class,
        });
        let receiver = fixture.ast.alloc(Tree {
            kind: TreeKind::This(This { qual: None }),
            position: None,
            ty: this_type,
        });
        let ty = fixture.store.types.alloc(Type::TermRef {
            prefix: this_type,
            target: TermRefTarget::Symbol(method),
        });
        fixture.ast.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier: receiver,
                name: fixture.store.symbols.get(method).name,
                backquoted: false,
            }),
            position: None,
            ty,
        })
    } else {
        typed_method_ident(fixture, method)
    };
    typed_call(fixture, function, args, ApplyKind::Regular, result)
}

fn typed_boolean_intrinsic_call(
    fixture: &mut EmissionFixture,
    method: SymbolId,
    receiver: TreeId<dotty_core::ast::Typed>,
    argument: TreeId<dotty_core::ast::Typed>,
) -> TreeId<dotty_core::ast::Typed> {
    let prefix = fixture.ast.get(receiver).ty;
    let name = fixture.store.symbols.get(method).name;
    let function_type = fixture.store.types.alloc(Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(method),
    });
    let function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name,
            backquoted: false,
        }),
        position: None,
        ty: function_type,
    });
    typed_call(
        fixture,
        function,
        vec![argument],
        ApplyKind::Regular,
        fixture.definitions.boolean,
    )
}

fn typed_boolean_not(
    fixture: &mut EmissionFixture,
    receiver: TreeId<dotty_core::ast::Typed>,
) -> TreeId<dotty_core::ast::Typed> {
    let symbols = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let prefix = fixture.ast.get(receiver).ty;
    let name = fixture.store.symbols.get(symbols.unary_not).name;
    let ty = fixture.store.types.alloc(Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(symbols.unary_not),
    });
    fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name,
            backquoted: false,
        }),
        position: None,
        ty,
    })
}

fn typed_int_intrinsic_call(
    fixture: &mut EmissionFixture,
    method: SymbolId,
    receiver: TreeId<dotty_core::ast::Typed>,
    argument: TreeId<dotty_core::ast::Typed>,
    result: dotty_core::TypeId,
) -> TreeId<dotty_core::ast::Typed> {
    let prefix = fixture.ast.get(receiver).ty;
    let name = fixture.store.symbols.get(method).name;
    let function_type = fixture.store.types.alloc(Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(method),
    });
    let function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name,
            backquoted: false,
        }),
        position: None,
        ty: function_type,
    });
    typed_call(
        fixture,
        function,
        vec![argument],
        ApplyKind::Regular,
        result,
    )
}

#[test]
fn emits_canonical_boolean_intrinsics_and_evaluates_boolean_truth_tables() {
    let mut fixture = EmissionFixture::new(false);
    let boolean = fixture.definitions.boolean;
    let int = fixture.definitions.int;
    let symbols = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();

    let not_method =
        add_call_target_method(&mut fixture, "negateFlag", &[("value", boolean)], boolean);
    let not_parameter = fixture.parameters.last().unwrap()[0];
    let no_prefix = fixture.definitions.no_prefix;
    let not_receiver = typed_method_ident_with_prefix(&mut fixture, not_parameter, no_prefix);
    let not_body = typed_boolean_not(&mut fixture, not_receiver);
    let ignored_stat = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(0),
        }),
        position: None,
        ty: int,
    });
    let not_result_type = fixture.ast.get(not_body).ty;
    let inner_ignored_stat = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(0),
        }),
        position: None,
        ty: int,
    });
    let inner_not_block = typed_block(
        &mut fixture,
        vec![inner_ignored_stat],
        not_body,
        not_result_type,
    );
    let not_block = typed_block(
        &mut fixture,
        vec![ignored_stat],
        inner_not_block,
        not_result_type,
    );
    *fixture.bodies.last_mut().unwrap() = not_block;

    let mut operation_methods = Vec::new();
    for (name, method, operation) in [
        ("andFlag", symbols.eager_and, 0x7e),
        ("orFlag", symbols.eager_or, 0x80),
        ("xorFlag", symbols.xor, 0x82),
        ("equalFlag", symbols.equal, 0x9f),
        ("notEqualFlag", symbols.not_equal, 0xa0),
    ] {
        let method_symbol = add_call_target_method(
            &mut fixture,
            name,
            &[("left", boolean), ("right", boolean)],
            boolean,
        );
        let method_index = fixture.methods.len() - 1;
        let left_parameter = fixture.parameters[method_index][0];
        let right_parameter = fixture.parameters[method_index][1];
        let left = typed_method_ident_with_prefix(&mut fixture, left_parameter, no_prefix);
        let right = typed_method_ident_with_prefix(&mut fixture, right_parameter, no_prefix);
        let expression = typed_boolean_intrinsic_call(&mut fixture, method, left, right);
        fixture.bodies[method_index] = expression;
        operation_methods.push((name, method_symbol, operation));
    }

    let and_method = add_call_target_method(
        &mut fixture,
        "branchOnAnd",
        &[("left", boolean), ("right", boolean)],
        int,
    );
    let branch_index = fixture.methods.len() - 1;
    let left_parameter = fixture.parameters[branch_index][0];
    let right_parameter = fixture.parameters[branch_index][1];
    let left = typed_method_ident_with_prefix(&mut fixture, left_parameter, no_prefix);
    let right = typed_method_ident_with_prefix(&mut fixture, right_parameter, no_prefix);
    let condition = typed_boolean_intrinsic_call(&mut fixture, symbols.eager_and, left, right);
    let yes = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int,
    });
    let no = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(0),
        }),
        position: None,
        ty: int,
    });
    fixture.bodies[branch_index] = fixture.ast.alloc(Tree {
        kind: TreeKind::If(If {
            cond: condition,
            then_branch: yes,
            else_branch: no,
        }),
        position: None,
        ty: int,
    });

    let left_probe = add_call_target_method(&mut fixture, "leftProbe", &[], boolean);
    let right_probe = add_call_target_method(&mut fixture, "rightProbe", &[], boolean);
    let ordered_method = add_call_target_method(&mut fixture, "orderedAnd", &[], boolean);
    let left_call = typed_call_to(&mut fixture, left_probe, Vec::new(), boolean, false);
    let right_call = typed_call_to(&mut fixture, right_probe, Vec::new(), boolean, false);
    let ordered =
        typed_boolean_intrinsic_call(&mut fixture, symbols.eager_and, left_call, right_call);
    *fixture.bodies.last_mut().unwrap() = ordered;

    let int_symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    for (name, boolean_method, initial) in [
        ("falseAndDivide", symbols.eager_and, false),
        ("trueOrDivide", symbols.eager_or, true),
    ] {
        add_call_target_method(&mut fixture, name, &[], boolean);
        let left = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Boolean(initial),
            }),
            position: None,
            ty: boolean,
        });
        let one = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(1),
            }),
            position: None,
            ty: int,
        });
        let zero = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(0),
            }),
            position: None,
            ty: int,
        });
        let quotient = typed_int_intrinsic_call(&mut fixture, int_symbols.divide, one, zero, int);
        let zero_again = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(0),
            }),
            position: None,
            ty: int,
        });
        let comparison = typed_int_intrinsic_call(
            &mut fixture,
            int_symbols.greater_than,
            quotient,
            zero_again,
            boolean,
        );
        let body = typed_boolean_intrinsic_call(&mut fixture, boolean_method, left, comparison);
        *fixture.bodies.last_mut().unwrap() = body;
    }

    add_call_target_method(
        &mut fixture,
        "localBoolean",
        &[("left", boolean), ("right", boolean)],
        boolean,
    );
    let local_method_index = fixture.methods.len() - 1;
    let local_left_parameter = fixture.parameters[local_method_index][0];
    let local_right_parameter = fixture.parameters[local_method_index][1];
    let local_left = typed_method_ident_with_prefix(&mut fixture, local_left_parameter, no_prefix);
    let local_right =
        typed_method_ident_with_prefix(&mut fixture, local_right_parameter, no_prefix);
    let first_local = add_emission_local_symbol(
        &mut fixture,
        local_method_index,
        "andResult",
        boolean,
        false,
        SymbolFlags::EMPTY,
    );
    let initialized_with_and =
        typed_boolean_intrinsic_call(&mut fixture, symbols.eager_and, local_left, local_right);
    let first_declaration = add_emission_local_valdef(
        &mut fixture,
        local_method_index,
        first_local,
        boolean,
        Some(initialized_with_and),
    );
    let first_reference = typed_emission_symbol_ident(&mut fixture, first_local);
    let second_local = add_emission_local_symbol(
        &mut fixture,
        local_method_index,
        "result",
        boolean,
        true,
        SymbolFlags::EMPTY,
    );
    let second_declaration = add_emission_local_valdef(
        &mut fixture,
        local_method_index,
        second_local,
        boolean,
        Some(first_reference),
    );
    let assignment_target = typed_emission_symbol_ident(&mut fixture, second_local);
    let assignment_left =
        typed_method_ident_with_prefix(&mut fixture, local_left_parameter, no_prefix);
    let assignment_right =
        typed_method_ident_with_prefix(&mut fixture, local_right_parameter, no_prefix);
    let assigned_xor =
        typed_boolean_intrinsic_call(&mut fixture, symbols.xor, assignment_left, assignment_right);
    let assignment = fixture.ast.alloc(Tree {
        kind: TreeKind::Assign(dotty_core::ast::Assign {
            lhs: assignment_target,
            rhs: assigned_xor,
        }),
        position: None,
        ty: fixture.definitions.unit,
    });
    let result = typed_emission_symbol_ident(&mut fixture, second_local);
    let local_body = typed_block(
        &mut fixture,
        vec![first_declaration, second_declaration, assignment],
        result,
        boolean,
    );
    fixture.bodies[local_method_index] = local_body;

    add_call_target_method(
        &mut fixture,
        "loopUntilTrue",
        &[("initial", boolean)],
        boolean,
    );
    let loop_method_index = fixture.methods.len() - 1;
    let loop_parameter = fixture.parameters[loop_method_index][0];
    let loop_local = add_emission_local_symbol(
        &mut fixture,
        loop_method_index,
        "flag",
        boolean,
        true,
        SymbolFlags::EMPTY,
    );
    let initializer = typed_method_ident_with_prefix(&mut fixture, loop_parameter, no_prefix);
    let loop_declaration = add_emission_local_valdef(
        &mut fixture,
        loop_method_index,
        loop_local,
        boolean,
        Some(initializer),
    );
    let condition_operand = typed_emission_symbol_ident(&mut fixture, loop_local);
    let condition = typed_boolean_not(&mut fixture, condition_operand);
    let assignment_target = typed_emission_symbol_ident(&mut fixture, loop_local);
    let true_value = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean,
    });
    let loop_assignment = fixture.ast.alloc(Tree {
        kind: TreeKind::Assign(dotty_core::ast::Assign {
            lhs: assignment_target,
            rhs: true_value,
        }),
        position: None,
        ty: fixture.definitions.unit,
    });
    let loop_body = fixture.ast.alloc(Tree {
        kind: TreeKind::While(dotty_core::ast::While {
            cond: condition,
            body: loop_assignment,
        }),
        position: None,
        ty: fixture.definitions.unit,
    });
    let loop_result = typed_emission_symbol_ident(&mut fixture, loop_local);
    fixture.bodies[loop_method_index] = typed_block(
        &mut fixture,
        vec![loop_declaration, loop_body],
        loop_result,
        boolean,
    );

    let mut unit = fixture.unit();
    unit.classes[0].methods[local_method_index].locals = vec![
        JvmLocalBinding {
            declaration: first_declaration,
            symbol: first_local,
            mutable: false,
        },
        JvmLocalBinding {
            declaration: second_declaration,
            symbol: second_local,
            mutable: true,
        },
    ];
    unit.classes[0].methods[loop_method_index].locals = vec![JvmLocalBinding {
        declaration: loop_declaration,
        symbol: loop_local,
        mutable: true,
    }];
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    assert_eq!(
        emitted_method_code(&class, "negateFlag"),
        [0x03, 0x57, 0x03, 0x57, 0x1b, 0x04, 0x82, 0xac]
    );
    for (name, _, opcode) in operation_methods {
        let method_code = emitted_method_code(&class, name);
        match opcode {
            0x7e | 0x80 | 0x82 => assert_eq!(method_code, [0x1b, 0x1c, opcode, 0xac]),
            0x9f | 0xa0 => assert_eq!(
                method_code,
                [
                    0x1b, 0x1c, opcode, 0x00, 0x07, 0x03, 0xa7, 0x00, 0x04, 0x04, 0xac
                ]
            ),
            _ => unreachable!(),
        }
    }
    let ordered_code = emitted_method_code(&class, "orderedAnd");
    assert_eq!(&ordered_code[0..1], &[0x2a]);
    assert_eq!(&ordered_code[4..5], &[0x2a]);
    assert_eq!(ordered_code.len(), 10);
    assert_eq!(&ordered_code[8..], &[0x7e, 0xac]);
    assert_virtual_method_ref(
        &class,
        ordered_code,
        1,
        "example/Answer",
        "leftProbe",
        "()Z",
    );
    assert_virtual_method_ref(
        &class,
        ordered_code,
        5,
        "example/Answer",
        "rightProbe",
        "()Z",
    );
    let branch_code = emitted_method_code(&class, "branchOnAnd");
    assert!(branch_code.contains(&0x7e));
    assert!(class.methods.iter().any(|method| {
        class.constant_pool.utf8(method.name_index) == Ok("branchOnAnd")
            && method.attributes.iter().any(|attribute| matches!(attribute,
                Attribute::Code(code) if code.attributes.iter().any(|nested| matches!(nested,
                    Attribute::Other(raw) if class.constant_pool.utf8(raw.name_index) == Ok("StackMapTable")
                ))
            ))
    }));
    for name in ["falseAndDivide", "trueOrDivide"] {
        let code = emitted_method_code(&class, name);
        let divide = code.iter().position(|opcode| *opcode == 0x6c).unwrap();
        let boolean_bitwise = code
            .iter()
            .position(|opcode| matches!(*opcode, 0x7e | 0x80))
            .unwrap();
        assert!(
            divide < boolean_bitwise,
            "RHS division must run before {name}'s intrinsic"
        );
    }

    if require_jdk_or_skip() {
        verify_emitted_call_class_with_jdk(
            generated.bytes(),
            "public final class CallHarness { public static void main(String[] args) { example.Answer answer = new example.Answer(); for (boolean left : new boolean[] { false, true }) { if (answer.negateFlag(left) != !left) throw new AssertionError(\"not\"); for (boolean right : new boolean[] { false, true }) { if (answer.andFlag(left, right) != (left & right)) throw new AssertionError(\"and\"); if (answer.orFlag(left, right) != (left | right)) throw new AssertionError(\"or\"); if (answer.xorFlag(left, right) != (left ^ right)) throw new AssertionError(\"xor\"); if (answer.equalFlag(left, right) != (left == right)) throw new AssertionError(\"eq\"); if (answer.notEqualFlag(left, right) != (left != right)) throw new AssertionError(\"ne\"); if (answer.localBoolean(left, right) != (left ^ right)) throw new AssertionError(\"local init and assign\"); } } if (answer.loopUntilTrue(false) != true || answer.loopUntilTrue(true) != true) throw new AssertionError(\"while condition\"); if (answer.branchOnAnd(true, true) != 1 || answer.branchOnAnd(true, false) != 0 || !answer.orderedAnd()) throw new AssertionError(\"control flow and eager order\"); try { answer.falseAndDivide(); throw new AssertionError(\"false & rhs was short-circuited\"); } catch (ArithmeticException expected) { } try { answer.trueOrDivide(); throw new AssertionError(\"true | rhs was short-circuited\"); } catch (ArithmeticException expected) { } System.out.println(\"boolean intrinsics ok\"); } }\n",
            "boolean intrinsics ok",
        );
    }
    assert_ne!(and_method, not_method);
    assert_ne!(ordered_method, right_probe);
}

#[test]
fn rejects_mistyped_canonical_boolean_intrinsics_without_a_class_result() {
    let mut operand = EmissionFixture::new(false);
    let boolean = operand.definitions.boolean;
    let int = operand.definitions.int;
    let symbols = operand
        .definitions
        .boolean_primitive_operator_symbols(&operand.store)
        .unwrap();
    add_call_target_method(&mut operand, "badOperand", &[], boolean);
    let receiver = operand.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int,
    });
    let argument = operand.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean,
    });
    let call = typed_boolean_intrinsic_call(&mut operand, symbols.eager_and, receiver, argument);
    *operand.bodies.last_mut().unwrap() = call;
    assert_eq!(
        operand.emit().unwrap_err(),
        JvmCodegenError::BooleanIntrinsicResolution {
            tree: call,
            error: crate::JvmBooleanIntrinsicError::WrongOperandType {
                tree: call,
                operand: receiver,
            },
        }
    );

    let mut result = EmissionFixture::new(false);
    let boolean = result.definitions.boolean;
    let int = result.definitions.int;
    let symbols = result
        .definitions
        .boolean_primitive_operator_symbols(&result.store)
        .unwrap();
    add_call_target_method(&mut result, "badResult", &[], int);
    let receiver = result.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean,
    });
    let argument = result.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(false),
        }),
        position: None,
        ty: boolean,
    });
    let call = typed_boolean_intrinsic_call(&mut result, symbols.xor, receiver, argument);
    result.ast.get_mut(call).ty = int;
    *result.bodies.last_mut().unwrap() = call;
    assert_eq!(
        result.emit().unwrap_err(),
        JvmCodegenError::BooleanIntrinsicResolution {
            tree: call,
            error: crate::JvmBooleanIntrinsicError::WrongResultType {
                tree: call,
                expected: boolean,
                found: int,
            },
        }
    );
}

#[test]
fn emits_short_circuit_boolean_intrinsics_without_evaluating_skipped_rhs() {
    let mut fixture = EmissionFixture::new(false);
    let boolean = fixture.definitions.boolean;
    let int = fixture.definitions.int;
    let no_prefix = fixture.definitions.no_prefix;
    let symbols = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let int_symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();

    for (name, operator, left) in [
        ("andSkips", symbols.and_then, false),
        ("orSkips", symbols.or_else, true),
        ("andEvaluates", symbols.and_then, true),
        ("orEvaluates", symbols.or_else, false),
    ] {
        add_call_target_method(&mut fixture, name, &[], boolean);
        let left = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Boolean(left),
            }),
            position: None,
            ty: boolean,
        });
        let one = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(1),
            }),
            position: None,
            ty: int,
        });
        let zero = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(0),
            }),
            position: None,
            ty: int,
        });
        let quotient = typed_int_intrinsic_call(&mut fixture, int_symbols.divide, one, zero, int);
        let comparison = typed_int_intrinsic_call(
            &mut fixture,
            int_symbols.greater_than,
            quotient,
            zero,
            boolean,
        );
        let body = typed_boolean_intrinsic_call(&mut fixture, operator, left, comparison);
        *fixture.bodies.last_mut().unwrap() = body;
    }

    for (name, operator, params) in [
        ("andChain", symbols.and_then, ["a", "b", "c"]),
        ("orChain", symbols.or_else, ["a", "b", "c"]),
    ] {
        add_call_target_method(
            &mut fixture,
            name,
            &[
                (params[0], boolean),
                (params[1], boolean),
                (params[2], boolean),
            ],
            boolean,
        );
        let method_index = fixture.methods.len() - 1;
        let parameters = fixture.parameters[method_index].clone();
        let [first_symbol, second_symbol, third_symbol] = parameters.as_slice() else {
            unreachable!("the chain method has three parameters")
        };
        let prefix = fixture.definitions.no_prefix;
        let first = typed_method_ident_with_prefix(&mut fixture, *first_symbol, prefix);
        let second = typed_method_ident_with_prefix(&mut fixture, *second_symbol, prefix);
        let third = typed_method_ident_with_prefix(&mut fixture, *third_symbol, prefix);
        let first_two = typed_boolean_intrinsic_call(&mut fixture, operator, first, second);
        let chain = typed_boolean_intrinsic_call(&mut fixture, operator, first_two, third);
        fixture.bodies[method_index] = chain;
    }

    let probe = add_call_target_method(&mut fixture, "probeBoolean", &[], boolean);
    add_call_target_method(&mut fixture, "ordinaryProbe", &[], boolean);
    let ordinary_probe = typed_call_to(&mut fixture, probe, Vec::new(), boolean, false);
    *fixture.bodies.last_mut().unwrap() = ordinary_probe;

    add_call_target_method(
        &mut fixture,
        "nestedInIf",
        &[("left", boolean), ("right", boolean)],
        int,
    );
    let if_index = fixture.methods.len() - 1;
    let if_parameters = fixture.parameters[if_index].clone();
    let left = typed_method_ident_with_prefix(&mut fixture, if_parameters[0], no_prefix);
    let right = typed_method_ident_with_prefix(&mut fixture, if_parameters[1], no_prefix);
    let not_right = typed_boolean_not(&mut fixture, right);
    let inner_or = typed_boolean_intrinsic_call(&mut fixture, symbols.or_else, not_right, left);
    let left_again = typed_method_ident_with_prefix(&mut fixture, if_parameters[0], no_prefix);
    let if_condition =
        typed_boolean_intrinsic_call(&mut fixture, symbols.and_then, left_again, inner_or);
    let yes = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int,
    });
    let no = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(0),
        }),
        position: None,
        ty: int,
    });
    fixture.bodies[if_index] = fixture.ast.alloc(Tree {
        kind: TreeKind::If(dotty_core::ast::If {
            cond: if_condition,
            then_branch: yes,
            else_branch: no,
        }),
        position: None,
        ty: int,
    });

    add_call_target_method(&mut fixture, "whileAnd", &[("initial", boolean)], boolean);
    let while_index = fixture.methods.len() - 1;
    let initial = fixture.parameters[while_index][0];
    let flag_local = add_emission_local_symbol(
        &mut fixture,
        while_index,
        "flag",
        boolean,
        true,
        SymbolFlags::EMPTY,
    );
    let initial_ref = typed_method_ident_with_prefix(&mut fixture, initial, no_prefix);
    let flag_declaration = add_emission_local_valdef(
        &mut fixture,
        while_index,
        flag_local,
        boolean,
        Some(initial_ref),
    );
    let seen_local = add_emission_local_symbol(
        &mut fixture,
        while_index,
        "seen",
        boolean,
        true,
        SymbolFlags::EMPTY,
    );
    let false_value = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(false),
        }),
        position: None,
        ty: boolean,
    });
    let seen_declaration = add_emission_local_valdef(
        &mut fixture,
        while_index,
        seen_local,
        boolean,
        Some(false_value),
    );
    let flag_condition = typed_emission_symbol_ident(&mut fixture, flag_local);
    let seen_condition = typed_emission_symbol_ident(&mut fixture, seen_local);
    let not_seen = typed_boolean_not(&mut fixture, seen_condition);
    let while_condition =
        typed_boolean_intrinsic_call(&mut fixture, symbols.and_then, flag_condition, not_seen);
    let seen_target = typed_emission_symbol_ident(&mut fixture, seen_local);
    let true_value = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean,
    });
    let seen_assignment = fixture.ast.alloc(Tree {
        kind: TreeKind::Assign(dotty_core::ast::Assign {
            lhs: seen_target,
            rhs: true_value,
        }),
        position: None,
        ty: fixture.definitions.unit,
    });
    let while_tree = fixture.ast.alloc(Tree {
        kind: TreeKind::While(dotty_core::ast::While {
            cond: while_condition,
            body: seen_assignment,
        }),
        position: None,
        ty: fixture.definitions.unit,
    });
    let seen_result = typed_emission_symbol_ident(&mut fixture, seen_local);
    fixture.bodies[while_index] = typed_block(
        &mut fixture,
        vec![flag_declaration, seen_declaration, while_tree],
        seen_result,
        boolean,
    );

    let mut unit = fixture.unit();
    unit.classes[0].methods[while_index].locals = vec![
        JvmLocalBinding {
            declaration: flag_declaration,
            symbol: flag_local,
            mutable: true,
        },
        JvmLocalBinding {
            declaration: seen_declaration,
            symbol: seen_local,
            mutable: true,
        },
    ];
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let and_skip_code = emitted_method_code(&class, "andSkips");
    let or_skip_code = emitted_method_code(&class, "orSkips");
    assert!(
        and_skip_code.contains(&0x99),
        "&& uses ifeq for its skip path"
    );
    assert!(
        or_skip_code.contains(&0x9a),
        "|| uses ifne for its skip path"
    );
    for name in ["andSkips", "orSkips", "andEvaluates", "orEvaluates"] {
        let code = emitted_method_code(&class, name);
        let divide = code.iter().position(|opcode| *opcode == 0x6c).unwrap();
        let branch = code
            .iter()
            .position(|opcode| matches!(*opcode, 0x99 | 0x9a))
            .unwrap();
        assert!(branch < divide, "{name} must branch before evaluating RHS");
    }

    if require_jdk_or_skip() {
        verify_emitted_call_class_with_jdk(
            generated.bytes(),
            "public final class CallHarness { public static void main(String[] args) { example.Answer answer = new example.Answer(); if (!answer.ordinaryProbe()) throw new AssertionError(\"ordinary call baseline\"); if (answer.andSkips() || !answer.orSkips()) throw new AssertionError(\"short-circuit result\"); try { answer.andEvaluates(); throw new AssertionError(\"true && evaluated RHS should throw\"); } catch (ArithmeticException expected) { } try { answer.orEvaluates(); throw new AssertionError(\"false || evaluated RHS should throw\"); } catch (ArithmeticException expected) { } for (boolean a : new boolean[] { false, true }) for (boolean b : new boolean[] { false, true }) for (boolean c : new boolean[] { false, true }) { if (answer.andChain(a, b, c) != (a && b && c)) throw new AssertionError(\"and chain\"); if (answer.orChain(a, b, c) != (a || b || c)) throw new AssertionError(\"or chain\"); if (answer.nestedInIf(a, b) != (a ? 1 : 0)) throw new AssertionError(\"short circuit nested in if\"); } if (answer.whileAnd(false) || !answer.whileAnd(true)) throw new AssertionError(\"short circuit nested in while\"); System.out.println(\"short circuit ok\"); } }\n",
            "short circuit ok",
        );
    }
}

#[test]
fn short_circuit_rhs_local_effects_run_once_and_join_initialization_safely() {
    let mut fixture = EmissionFixture::new(false);
    let boolean = fixture.definitions.boolean;
    let int = fixture.definitions.int;
    let unit_type = fixture.definitions.unit;
    let no_prefix = fixture.definitions.no_prefix;
    let symbols = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let int_symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let mut planned_locals = Vec::new();

    for (name, operator, rhs_value) in [
        ("andHits", symbols.and_then, true),
        ("orHits", symbols.or_else, false),
    ] {
        add_call_target_method(&mut fixture, name, &[("flag", boolean)], int);
        let method_index = fixture.methods.len() - 1;
        let parameter = fixture.parameters[method_index][0];
        let hit_counter = add_emission_local_symbol(
            &mut fixture,
            method_index,
            "hits",
            int,
            true,
            SymbolFlags::EMPTY,
        );
        let zero = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(0),
            }),
            position: None,
            ty: int,
        });
        let initializer =
            add_emission_local_valdef(&mut fixture, method_index, hit_counter, int, Some(zero));
        let left = typed_method_ident_with_prefix(&mut fixture, parameter, no_prefix);
        let assignment_lhs = typed_emission_symbol_ident(&mut fixture, hit_counter);
        let current_hits = typed_emission_symbol_ident(&mut fixture, hit_counter);
        let one = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(1),
            }),
            position: None,
            ty: int,
        });
        let incremented_hits =
            typed_int_intrinsic_call(&mut fixture, int_symbols.plus, current_hits, one, int);
        let assignment = fixture.ast.alloc(Tree {
            kind: TreeKind::Assign(dotty_core::ast::Assign {
                lhs: assignment_lhs,
                rhs: incremented_hits,
            }),
            position: None,
            ty: unit_type,
        });
        let rhs_value = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Boolean(rhs_value),
            }),
            position: None,
            ty: boolean,
        });
        let rhs = typed_block(&mut fixture, vec![assignment], rhs_value, boolean);
        let short_circuit = typed_boolean_intrinsic_call(&mut fixture, operator, left, rhs);
        let hits_result = typed_emission_symbol_ident(&mut fixture, hit_counter);
        fixture.bodies[method_index] = typed_block(
            &mut fixture,
            vec![initializer, short_circuit],
            hits_result,
            int,
        );
        planned_locals.push((method_index, hit_counter, initializer));
    }

    add_call_target_method(&mut fixture, "leftOnce", &[], int);
    let left_once_index = fixture.methods.len() - 1;
    let left_hits = add_emission_local_symbol(
        &mut fixture,
        left_once_index,
        "hits",
        int,
        true,
        SymbolFlags::EMPTY,
    );
    let zero = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(0),
        }),
        position: None,
        ty: int,
    });
    let left_hits_declaration =
        add_emission_local_valdef(&mut fixture, left_once_index, left_hits, int, Some(zero));
    let left_assignment_target = typed_emission_symbol_ident(&mut fixture, left_hits);
    let left_current_hits = typed_emission_symbol_ident(&mut fixture, left_hits);
    let one = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: int,
    });
    let incremented_left_hits =
        typed_int_intrinsic_call(&mut fixture, int_symbols.plus, left_current_hits, one, int);
    let left_assignment = fixture.ast.alloc(Tree {
        kind: TreeKind::Assign(dotty_core::ast::Assign {
            lhs: left_assignment_target,
            rhs: incremented_left_hits,
        }),
        position: None,
        ty: unit_type,
    });
    let left_true = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean,
    });
    let left_operand = typed_block(&mut fixture, vec![left_assignment], left_true, boolean);
    let right_true = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean,
    });
    let left_once =
        typed_boolean_intrinsic_call(&mut fixture, symbols.and_then, left_operand, right_true);
    let left_once_result = typed_emission_symbol_ident(&mut fixture, left_hits);
    fixture.bodies[left_once_index] = typed_block(
        &mut fixture,
        vec![left_hits_declaration, left_once],
        left_once_result,
        int,
    );
    planned_locals.push((left_once_index, left_hits, left_hits_declaration));

    let probe = add_call_target_method(&mut fixture, "localEffectProbe", &[], boolean);
    add_call_target_method(&mut fixture, "ordinaryLocalEffectProbe", &[], boolean);
    let probe_call = typed_call_to(&mut fixture, probe, Vec::new(), boolean, false);
    *fixture.bodies.last_mut().unwrap() = probe_call;

    let mut unit = fixture.unit();
    for (method_index, symbol, declaration) in planned_locals {
        unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
            declaration,
            symbol,
            mutable: true,
        }];
    }
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    if require_jdk_or_skip() {
        verify_emitted_call_class_with_jdk(
            generated.bytes(),
            "public final class CallHarness { public static void main(String[] args) { example.Answer answer = new example.Answer(); if (!answer.ordinaryLocalEffectProbe()) throw new AssertionError(\"ordinary call baseline\"); if (answer.andHits(false) != 0 || answer.andHits(true) != 1) throw new AssertionError(\"and RHS count\"); if (answer.orHits(true) != 0 || answer.orHits(false) != 1) throw new AssertionError(\"or RHS count\"); if (answer.leftOnce() != 1) throw new AssertionError(\"left operand count\"); System.out.println(\"short-circuit local effects ok\"); } }\n",
            "short-circuit local effects ok",
        );
    }

    let mut uninitialized = EmissionFixture::new(false);
    let boolean = uninitialized.definitions.boolean;
    let int = uninitialized.definitions.int;
    let symbols = uninitialized
        .definitions
        .boolean_primitive_operator_symbols(&uninitialized.store)
        .unwrap();
    add_call_target_method(
        &mut uninitialized,
        "readConditionalLocal",
        &[("flag", boolean)],
        int,
    );
    let method_index = uninitialized.methods.len() - 1;
    let parameter = uninitialized.parameters[method_index][0];
    let conditional_local = add_emission_local_symbol(
        &mut uninitialized,
        method_index,
        "conditional",
        int,
        false,
        SymbolFlags::EMPTY,
    );
    let seven = uninitialized.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(7),
        }),
        position: None,
        ty: int,
    });
    let declaration = add_emission_local_valdef(
        &mut uninitialized,
        method_index,
        conditional_local,
        int,
        Some(seven),
    );
    let rhs_true = uninitialized.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: boolean,
    });
    let rhs = typed_block(&mut uninitialized, vec![declaration], rhs_true, boolean);
    let no_prefix = uninitialized.definitions.no_prefix;
    let left = typed_method_ident_with_prefix(&mut uninitialized, parameter, no_prefix);
    let short_circuit =
        typed_boolean_intrinsic_call(&mut uninitialized, symbols.and_then, left, rhs);
    let conditional_read = typed_emission_symbol_ident(&mut uninitialized, conditional_local);
    let body = typed_block(
        &mut uninitialized,
        vec![short_circuit],
        conditional_read,
        int,
    );
    uninitialized.bodies[method_index] = body;
    let mut unit = uninitialized.unit();
    unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
        declaration,
        symbol: conditional_local,
        mutable: false,
    }];
    assert_eq!(
        emit_class(&unit, uninitialized.class, &uninitialized.definitions).unwrap_err(),
        JvmCodegenError::UninitializedLocalRead {
            tree: conditional_read,
            symbol: conditional_local,
        }
    );
}

#[test]
fn rejects_unit_and_diverging_short_circuit_rhs_with_typed_errors() {
    for rhs_type in ["unit", "nothing"] {
        let mut fixture = EmissionFixture::new(false);
        let boolean = fixture.definitions.boolean;
        let symbols = fixture
            .definitions
            .boolean_primitive_operator_symbols(&fixture.store)
            .unwrap();
        add_call_target_method(&mut fixture, "badShortCircuit", &[], boolean);
        let left = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Boolean(true),
            }),
            position: None,
            ty: boolean,
        });
        let right_type = if rhs_type == "unit" {
            fixture.definitions.unit
        } else {
            fixture.definitions.nothing_type
        };
        let right = if rhs_type == "unit" {
            fixture.ast.alloc(Tree {
                kind: TreeKind::Literal(Literal {
                    value: Constant::Unit,
                }),
                position: None,
                ty: right_type,
            })
        } else {
            fixture.ast.alloc(Tree {
                kind: TreeKind::Return(dotty_core::ast::Return {
                    expr: None,
                    from: None,
                }),
                position: None,
                ty: right_type,
            })
        };
        let expression = typed_boolean_intrinsic_call(&mut fixture, symbols.and_then, left, right);
        *fixture.bodies.last_mut().unwrap() = expression;
        assert_eq!(
            fixture.emit().unwrap_err(),
            JvmCodegenError::BooleanIntrinsicResolution {
                tree: expression,
                error: crate::JvmBooleanIntrinsicError::WrongOperandType {
                    tree: expression,
                    operand: right,
                },
            },
            "{rhs_type} RHS is rejected before code publication"
        );
    }
}

#[test]
fn short_circuit_allows_verified_ordinary_calls_inside_the_rhs_branch() {
    let mut fixture = EmissionFixture::new(false);
    let boolean = fixture.definitions.boolean;
    let symbols = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let probe = add_call_target_method(&mut fixture, "probe", &[], boolean);
    let _caller = add_call_target_method(
        &mut fixture,
        "shortCircuitCall",
        &[("flag", boolean)],
        boolean,
    );
    let method_index = fixture.methods.len() - 1;
    let flag_symbol = fixture.parameters[method_index][0];
    let no_prefix = fixture.definitions.no_prefix;
    let flag = typed_method_ident_with_prefix(&mut fixture, flag_symbol, no_prefix);
    let rhs = typed_call_to(&mut fixture, probe, Vec::new(), boolean, false);
    let expression = typed_boolean_intrinsic_call(&mut fixture, symbols.and_then, flag, rhs);
    fixture.bodies[method_index] = expression;

    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "shortCircuitCall");
    assert!(code.contains(&0x99), "&& branches before its RHS call");
    assert!(code.contains(&0xb6), "the approved call remains virtual");
    assert!(code.contains(&0xac), "the Boolean result is returned");
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
fn lowers_explicit_and_implicit_same_class_call_receivers_once() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let target = add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let argument = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(3),
        }),
        position: None,
        ty: int_type,
    });

    let implicit_function = typed_method_ident(&mut fixture, target);
    let implicit_call = typed_call(
        &mut fixture,
        implicit_function,
        vec![argument],
        ApplyKind::Regular,
        int_type,
    );
    let implicit_code = fixture
        .unit()
        .lower_same_class_call_receiver(
            fixture.class,
            fixture.methods[1],
            implicit_call,
            &fixture.definitions,
        )
        .unwrap();
    assert_eq!(encode_receiver_code(implicit_code), vec![0x2a, 0xb0]);

    let this_type = fixture.store.types.alloc(Type::ThisType {
        class: fixture.class,
    });
    let this_tree = fixture.ast.alloc(Tree {
        kind: TreeKind::This(This { qual: None }),
        position: None,
        ty: this_type,
    });
    let selected_type = fixture.store.types.alloc(Type::TermRef {
        prefix: this_type,
        target: TermRefTarget::Symbol(target),
    });
    let explicit_function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: this_tree,
            name: fixture.store.symbols.get(target).name,
            backquoted: false,
        }),
        position: None,
        ty: selected_type,
    });
    let explicit_call = typed_call(
        &mut fixture,
        explicit_function,
        vec![argument],
        ApplyKind::Regular,
        int_type,
    );
    let explicit_code = fixture
        .unit()
        .lower_same_class_call_receiver(
            fixture.class,
            fixture.methods[1],
            explicit_call,
            &fixture.definitions,
        )
        .unwrap();
    assert_eq!(encode_receiver_code(explicit_code), vec![0x2a, 0xb0]);
}

#[test]
fn emits_explicit_and_implicit_virtual_calls_with_exact_owner_and_descriptor() {
    for explicit_this in [false, true] {
        let mut fixture = EmissionFixture::new(false);
        let int_type = fixture.definitions.int;
        let identity =
            add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
        let identity_parameter = fixture.parameters[2][0];
        set_method_body_to_parameter(&mut fixture, identity, identity_parameter);
        let left = fixture.parameters[1][1];
        let prefix = fixture.definitions.no_prefix;
        let argument = typed_method_ident_with_prefix(&mut fixture, left, prefix);
        let call = typed_call_to(
            &mut fixture,
            identity,
            vec![argument],
            int_type,
            explicit_this,
        );
        fixture.bodies[1] = call;
        let constant = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(42),
            }),
            position: None,
            ty: int_type,
        });
        let value_call = typed_call_to(&mut fixture, identity, vec![constant], int_type, false);
        fixture.bodies[0] = fixture.ast.alloc(Tree {
            kind: TreeKind::Return(dotty_core::ast::Return {
                expr: Some(value_call),
                from: None,
            }),
            position: None,
            ty: fixture.definitions.nothing_type,
        });

        let generated = fixture.emit().unwrap();
        let mut reader = Reader::new(generated.bytes());
        let class = ClassFile::decode(&mut reader).unwrap();
        class.validate_references().unwrap();
        let code = emitted_method_code(&class, "select");
        assert_eq!(code.len(), 6);
        assert_eq!(&code[..3], &[0x2a, 0x1c, 0xb6]);
        assert_eq!(code[5], 0xac);
        assert_virtual_method_ref(&class, code, 2, "example/Answer", "identity", "(I)I");
        let value_code = emitted_method_code(&class, "value");
        assert_eq!(value_code.len(), 7);
        assert_eq!(&value_code[..3], &[0x2a, 0x10, 42]);
        assert_eq!(value_code[3], 0xb6);
        assert_eq!(value_code[6], 0xac);
        assert_virtual_method_ref(&class, value_code, 3, "example/Answer", "identity", "(I)I");
        assert_eq!(generated.bytes(), fixture.emit().unwrap().bytes());
        if !explicit_this && require_jdk_or_skip() {
            verify_emitted_call_class_with_jdk(
                generated.bytes(),
                "public final class CallHarness { public static void main(String[] args) { example.Answer answer = new example.Answer(); System.out.println(answer.value() + \":\" + answer.select(true, 7, 9) + \":\" + answer.select(false, 11, 13)); } }\n",
                "42:7:11",
            );
        }
    }
}

#[test]
fn emits_nested_calls_and_evaluates_two_arguments_in_source_order() {
    let mut nested = EmissionFixture::new(false);
    let int_type = nested.definitions.int;
    let identity =
        add_call_target_method(&mut nested, "identity", &[("value", int_type)], int_type);
    let identity_parameter = nested.parameters[2][0];
    set_method_body_to_parameter(&mut nested, identity, identity_parameter);
    let left = nested.parameters[1][1];
    let prefix = nested.definitions.no_prefix;
    let inner_arg = typed_method_ident_with_prefix(&mut nested, left, prefix);
    let inner = typed_call_to(&mut nested, identity, vec![inner_arg], int_type, false);
    let outer = typed_call_to(&mut nested, identity, vec![inner], int_type, false);
    nested.bodies[1] = outer;
    let generated = nested.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(code.len(), 10);
    assert_eq!(&code[..4], &[0x2a, 0x2a, 0x1c, 0xb6]);
    assert_eq!(code[6], 0xb6);
    assert_eq!(code[9], 0xac);
    assert_virtual_method_ref(&class, code, 3, "example/Answer", "identity", "(I)I");
    assert_virtual_method_ref(&class, code, 6, "example/Answer", "identity", "(I)I");

    let mut pair = EmissionFixture::new(false);
    let int_type = pair.definitions.int;
    let combine = add_call_target_method(
        &mut pair,
        "combine",
        &[("first", int_type), ("second", int_type)],
        int_type,
    );
    let first = pair.parameters.last().unwrap()[0];
    set_method_body_to_parameter(&mut pair, combine, first);
    let right = pair.parameters[1][2];
    let left = pair.parameters[1][1];
    let prefix = pair.definitions.no_prefix;
    let first_argument = typed_method_ident_with_prefix(&mut pair, right, prefix);
    let second_argument = typed_method_ident_with_prefix(&mut pair, left, prefix);
    let call = typed_call_to(
        &mut pair,
        combine,
        vec![first_argument, second_argument],
        int_type,
        false,
    );
    pair.bodies[1] = call;
    let generated = pair.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(code.len(), 7);
    assert_eq!(&code[..4], &[0x2a, 0x1d, 0x1c, 0xb6]);
    assert_eq!(code[6], 0xac);
    assert_virtual_method_ref(&class, code, 3, "example/Answer", "combine", "(II)I");
}

#[test]
fn reference_call_values_are_forwarded_and_used_as_receivers_once() {
    let mut fixture = EmissionFixture::new(false);
    let class_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(fixture.class),
    });
    let declarations = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("fixture class info should be ClassInfo"),
        },
        _ => panic!("fixture class info should be complete"),
    };
    let int_type = fixture.definitions.int;
    let no_prefix = fixture.definitions.no_prefix;
    let get = add_call_target_method(&mut fixture, "getReference", &[], int_type);
    let identity = add_call_target_method(
        &mut fixture,
        "identityReference",
        &[("value", class_type)],
        class_type,
    );
    let identity_index = fixture
        .methods
        .iter()
        .position(|method| *method == identity)
        .unwrap();
    let identity_parameter = fixture.parameters[identity_index][0];
    set_method_body_to_parameter(&mut fixture, identity, identity_parameter);

    let (forward, forward_parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        "forwardReference",
        &[("value", class_type)],
        class_type,
    );
    let forward_argument =
        typed_method_ident_with_prefix(&mut fixture, forward_parameters[0], no_prefix);
    let forward_body = typed_call_to(
        &mut fixture,
        identity,
        vec![forward_argument],
        class_type,
        false,
    );
    fixture.methods.push(forward);
    fixture.parameters.push(forward_parameters);
    fixture.bodies.push(forward_body);

    let (read, read_parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        "readReference",
        &[("value", class_type)],
        int_type,
    );
    let receiver = typed_method_ident_with_prefix(&mut fixture, read_parameters[0], no_prefix);
    let select_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.ast.get(receiver).ty,
        target: TermRefTarget::Symbol(get),
    });
    let select = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name: fixture.store.symbols.get(get).name,
            backquoted: false,
        }),
        position: None,
        ty: select_type,
    });
    let int_type = fixture.definitions.int;
    let read_body = typed_call(
        &mut fixture,
        select,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    fixture.methods.push(read);
    fixture.parameters.push(read_parameters);
    fixture.bodies.push(read_body);

    let (nested_argument, nested_argument_parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        "nestedArgument",
        &[("value", class_type)],
        class_type,
    );
    let nested_argument_parameter =
        typed_method_ident_with_prefix(&mut fixture, nested_argument_parameters[0], no_prefix);
    let inner_argument_call = typed_call_to(
        &mut fixture,
        identity,
        vec![nested_argument_parameter],
        class_type,
        false,
    );
    let nested_argument_body = typed_call_to(
        &mut fixture,
        identity,
        vec![inner_argument_call],
        class_type,
        false,
    );
    fixture.methods.push(nested_argument);
    fixture.parameters.push(nested_argument_parameters);
    fixture.bodies.push(nested_argument_body);

    let (nested_receiver, nested_receiver_parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        "nestedReceiver",
        &[("value", class_type)],
        fixture.definitions.int,
    );
    let nested_receiver_parameter =
        typed_method_ident_with_prefix(&mut fixture, nested_receiver_parameters[0], no_prefix);
    let receiver_call = typed_call_to(
        &mut fixture,
        identity,
        vec![nested_receiver_parameter],
        class_type,
        false,
    );
    let get_select_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.ast.get(receiver_call).ty,
        target: TermRefTarget::Symbol(get),
    });
    let get_select = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver_call,
            name: fixture.store.symbols.get(get).name,
            backquoted: false,
        }),
        position: None,
        ty: get_select_type,
    });
    let nested_receiver_body = typed_call(
        &mut fixture,
        get_select,
        Vec::new(),
        ApplyKind::Regular,
        int_type,
    );
    fixture.methods.push(nested_receiver);
    fixture.parameters.push(nested_receiver_parameters);
    fixture.bodies.push(nested_receiver_body);

    let generated = fixture.emit().expect("exact planned reference calls lower");
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).expect("decode reference-call class");
    class
        .validate_references()
        .expect("reference call descriptors validate");
    let forward_code = emitted_method_code(&class, "forwardReference");
    assert_eq!(&forward_code[..2], &[0x2a, 0x2b]);
    assert_eq!(forward_code[2], 0xb6);
    assert_eq!(forward_code[5], 0xb0);
    assert_virtual_method_ref(
        &class,
        forward_code,
        2,
        "example/Answer",
        "identityReference",
        "(Lexample/Answer;)Lexample/Answer;",
    );
    let read_code = emitted_method_code(&class, "readReference");
    assert_eq!(read_code[0], 0x2b);
    assert_eq!(read_code[1], 0xb6);
    assert_eq!(read_code[4], 0xac);
    let nested_argument_code = emitted_method_code(&class, "nestedArgument");
    assert_eq!(&nested_argument_code[..3], &[0x2a, 0x2a, 0x2b]);
    assert_eq!(nested_argument_code[3], 0xb6);
    assert_eq!(nested_argument_code[6], 0xb6);
    assert_eq!(nested_argument_code[9], 0xb0);
    let nested_receiver_code = emitted_method_code(&class, "nestedReceiver");
    assert_eq!(&nested_receiver_code[..2], &[0x2a, 0x2b]);
    assert_eq!(nested_receiver_code[2], 0xb6);
    assert_eq!(nested_receiver_code[5], 0xb6);
    assert_eq!(nested_receiver_code[8], 0xac);
    assert_virtual_method_ref(
        &class,
        nested_receiver_code,
        5,
        "example/Answer",
        "getReference",
        "()I",
    );
}

#[test]
fn reference_call_receiver_resolution_rejects_cycles_and_excessive_depth() {
    let mut cyclic = EmissionFixture::new(false);
    let class_type = cyclic.store.types.alloc(Type::TypeRef {
        prefix: cyclic.definitions.no_prefix,
        target: TypeRefTarget::Symbol(cyclic.class),
    });
    let identity = add_call_target_method(
        &mut cyclic,
        "identityReference",
        &[("value", class_type)],
        class_type,
    );
    let this_type = cyclic.store.types.alloc(Type::ThisType {
        class: cyclic.class,
    });
    let this = cyclic.ast.alloc(Tree {
        kind: TreeKind::This(This { qual: None }),
        position: None,
        ty: this_type,
    });
    let selected_type = cyclic.store.types.alloc(Type::TermRef {
        prefix: this_type,
        target: TermRefTarget::Symbol(identity),
    });
    let select = cyclic.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: this,
            name: cyclic.store.symbols.get(identity).name,
            backquoted: false,
        }),
        position: None,
        ty: selected_type,
    });
    let call = typed_call(
        &mut cyclic,
        select,
        vec![this],
        ApplyKind::Regular,
        class_type,
    );
    let TreeKind::Select(select_tree) = &mut cyclic.ast.get_mut(select).kind else {
        unreachable!();
    };
    select_tree.qualifier = call;

    let unit = cyclic.unit();
    unit.validate()
        .expect("resolver cycles remain valid unit data");
    assert_eq!(
        unit.resolve_planned_instance_call(
            cyclic.class,
            cyclic.methods[1],
            call,
            &cyclic.definitions,
        ),
        Err(JvmCodegenError::UnsupportedJvmCallShape {
            tree: call,
            kind: UnsupportedJvmCallShapeKind::RecursiveReceiver,
        })
    );

    let mut deep = EmissionFixture::new(false);
    let class_type = deep.store.types.alloc(Type::TypeRef {
        prefix: deep.definitions.no_prefix,
        target: TypeRefTarget::Symbol(deep.class),
    });
    let identity = add_call_target_method(
        &mut deep,
        "identityReference",
        &[("value", class_type)],
        class_type,
    );
    let this_type = deep.store.types.alloc(Type::ThisType { class: deep.class });
    let mut receiver = deep.ast.alloc(Tree {
        kind: TreeKind::This(This { qual: None }),
        position: None,
        ty: this_type,
    });
    let mut innermost_call = None;
    for _ in 0..=crate::JVM_EXPRESSION_DEPTH_LIMIT {
        let selected_type = deep.store.types.alloc(Type::TermRef {
            prefix: deep.ast.get(receiver).ty,
            target: TermRefTarget::Symbol(identity),
        });
        let select = deep.ast.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier: receiver,
                name: deep.store.symbols.get(identity).name,
                backquoted: false,
            }),
            position: None,
            ty: selected_type,
        });
        let argument = deep.ast.alloc(Tree {
            kind: TreeKind::This(This { qual: None }),
            position: None,
            ty: this_type,
        });
        receiver = typed_call(
            &mut deep,
            select,
            vec![argument],
            ApplyKind::Regular,
            class_type,
        );
        innermost_call.get_or_insert(receiver);
    }
    let unit = deep.unit();
    unit.validate()
        .expect("deep call chain remains valid unit data");
    assert_eq!(
        unit.resolve_planned_instance_call(
            deep.class,
            deep.methods[1],
            receiver,
            &deep.definitions,
        ),
        Err(JvmCodegenError::ExpressionDepthExceeded {
            tree: innermost_call.unwrap(),
            position: None,
            limit: crate::JVM_EXPRESSION_DEPTH_LIMIT,
        })
    );
}

#[test]
fn rejects_reference_call_result_with_a_different_planned_class() {
    let mut fixture = EmissionFixture::new(false);
    let answer_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(fixture.class),
    });
    let owner = fixture.store.symbols.get(fixture.class).owner.unwrap();
    let no_prefix = fixture.definitions.no_prefix;
    let object_type = fixture.definitions.object_type;
    let other = add_reference_class(
        &mut fixture.store,
        owner,
        "OtherReference",
        no_prefix,
        object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    let other_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(other),
    });
    let identity = add_call_target_method(
        &mut fixture,
        "identityReference",
        &[("value", answer_type)],
        answer_type,
    );
    let this_type = fixture.store.types.alloc(Type::ThisType {
        class: fixture.class,
    });
    let argument = fixture.ast.alloc(Tree {
        kind: TreeKind::This(This { qual: None }),
        position: None,
        ty: this_type,
    });
    let call = typed_call_to(&mut fixture, identity, vec![argument], other_type, false);
    let mut unit = fixture.unit();
    unit.classes.push(JvmClassPlan {
        class: other,
        fields: Vec::new(),
        methods: Vec::new(),
        initialization: JvmClassInitializationPlan::ImplicitNoArg,
    });
    assert_eq!(
        unit.resolve_planned_instance_call(
            fixture.class,
            fixture.methods[1],
            call,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::JvmCallResultTypeMismatch {
            tree: call,
            method: identity,
            expected: JvmReturnType::Value(JvmType::Reference(fixture.class)),
            found: Some(JvmReturnType::Value(JvmType::Reference(other))),
        })
    );

    let identity_index = fixture
        .methods
        .iter()
        .position(|method| *method == identity)
        .unwrap();
    let identity_parameter = fixture.parameters[identity_index][0];
    set_method_body_to_parameter(&mut fixture, identity, identity_parameter);
    let class_type = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("fixture class info should be ClassInfo"),
        },
        _ => panic!("fixture class info should be complete"),
    };
    let (bad_forward, bad_forward_parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        class_type,
        "badForwardReference",
        &[("value", other_type)],
        answer_type,
    );
    let no_prefix = fixture.definitions.no_prefix;
    let bad_argument =
        typed_method_ident_with_prefix(&mut fixture, bad_forward_parameters[0], no_prefix);
    let bad_body = typed_call_to(
        &mut fixture,
        identity,
        vec![bad_argument],
        answer_type,
        false,
    );
    fixture.methods.push(bad_forward);
    fixture.parameters.push(bad_forward_parameters);
    fixture.bodies.push(bad_body);
    let mut unit = fixture.unit();
    unit.classes.push(JvmClassPlan {
        class: other,
        fields: Vec::new(),
        methods: Vec::new(),
        initialization: JvmClassInitializationPlan::ImplicitNoArg,
    });
    assert_eq!(
        emit_class(&unit, fixture.class, &fixture.definitions),
        Err(JvmCodegenError::JvmCallArgumentMismatch {
            tree: bad_body,
            argument: bad_argument,
            method: identity,
            index: 0,
            expected: JvmType::Reference(fixture.class),
            found: JvmExpressionFlow::Value {
                value_type: JvmType::Reference(other),
                category: JvmValueCategory::Reference,
            },
        })
    );
}

#[test]
fn lowers_block_call_stats_in_source_order_with_exact_pop_and_invocations() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let identity_parameter = fixture.parameters[2][0];
    set_method_body_to_parameter(&mut fixture, identity, identity_parameter);

    let select_left = fixture.parameters[1][1];
    let prefix = fixture.definitions.no_prefix;
    let left = typed_method_ident_with_prefix(&mut fixture, select_left, prefix);
    let first_call = typed_call_to(&mut fixture, identity, vec![left], int_type, false);
    let constant = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: int_type,
    });
    let final_call = typed_call_to(&mut fixture, identity, vec![constant], int_type, false);
    fixture.bodies[1] = typed_block(&mut fixture, vec![first_call], final_call, int_type);

    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(code.len(), 13);
    assert_eq!(&code[..2], &[0x2a, 0x1c]);
    assert_eq!(code[2], 0xb6);
    assert_eq!(code[5], 0x57);
    assert_eq!(&code[6..9], &[0x2a, 0x10, 42]);
    assert_eq!(code[9], 0xb6);
    assert_eq!(code[12], 0xac);
    assert_virtual_method_ref(&class, code, 2, "example/Answer", "identity", "(I)I");
    assert_virtual_method_ref(&class, code, 9, "example/Answer", "identity", "(I)I");

    if require_jdk_or_skip() {
        verify_emitted_call_class_with_jdk(
            generated.bytes(),
            "public class CallHarness { public static void main(String[] args) { example.Answer answer = new example.Answer(); System.out.println(answer.select(true, 7, 9) + \":\" + answer.select(false, 11, 13)); } }\n",
            "42:42",
        );
    }
}

#[test]
fn lowers_nested_blocks_and_discards_each_intermediate_call_result() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let identity_parameter = fixture.parameters[2][0];
    set_method_body_to_parameter(&mut fixture, identity, identity_parameter);

    let select_left = fixture.parameters[1][1];
    let prefix = fixture.definitions.no_prefix;
    let left = typed_method_ident_with_prefix(&mut fixture, select_left, prefix);
    let first_call = typed_call_to(&mut fixture, identity, vec![left], int_type, false);
    let twenty = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(20),
        }),
        position: None,
        ty: int_type,
    });
    let inner_result = typed_call_to(&mut fixture, identity, vec![twenty], int_type, false);
    let inner = typed_block(&mut fixture, vec![first_call], inner_result, int_type);
    let forty_two = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: int_type,
    });
    let final_call = typed_call_to(&mut fixture, identity, vec![forty_two], int_type, false);
    fixture.bodies[1] = typed_block(&mut fixture, vec![inner], final_call, int_type);

    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(code.len(), 20);
    assert_eq!(&code[..2], &[0x2a, 0x1c]);
    assert_eq!(code[2], 0xb6);
    assert_eq!(code[5], 0x57);
    assert_eq!(&code[6..9], &[0x2a, 0x10, 20]);
    assert_eq!(code[9], 0xb6);
    assert_eq!(code[12], 0x57);
    assert_eq!(&code[13..16], &[0x2a, 0x10, 42]);
    assert_eq!(code[16], 0xb6);
    assert_eq!(code[19], 0xac);
    for offset in [2, 9, 16] {
        assert_virtual_method_ref(&class, code, offset, "example/Answer", "identity", "(I)I");
    }
}

#[test]
fn unit_call_stat_needs_no_pop_and_unit_block_ends_with_return() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let unit_type = fixture.definitions.unit;
    let noop = add_call_target_method(&mut fixture, "noop", &[], unit_type);
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let identity_parameter = fixture.parameters[3][0];
    set_method_body_to_parameter(&mut fixture, identity, identity_parameter);

    let noop_call = typed_call_to(&mut fixture, noop, Vec::new(), unit_type, false);
    let forty_two = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: int_type,
    });
    let final_call = typed_call_to(&mut fixture, identity, vec![forty_two], int_type, false);
    fixture.bodies[1] = typed_block(&mut fixture, vec![noop_call], final_call, int_type);

    let declarations = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("fixture class info should be ClassInfo"),
        },
        _ => panic!("fixture class info should be complete"),
    };
    let (only_unit, parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        "onlyUnit",
        &[],
        unit_type,
    );
    fixture.methods.push(only_unit);
    fixture.parameters.push(parameters);
    let unit_literal = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Unit,
        }),
        position: None,
        ty: unit_type,
    });
    let only_unit_body = typed_block(&mut fixture, vec![noop_call], unit_literal, unit_type);
    fixture.bodies.push(only_unit_body);

    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let select_code = emitted_method_code(&class, "select");
    assert_eq!(select_code.len(), 11);
    assert_eq!(
        &select_code[..4],
        &[0x2a, 0xb6, select_code[2], select_code[3]]
    );
    assert_eq!(select_code[4], 0x2a);
    assert_eq!(&select_code[5..7], &[0x10, 42]);
    assert_eq!(select_code[7], 0xb6);
    assert_eq!(select_code[10], 0xac);
    assert_virtual_method_ref(&class, select_code, 1, "example/Answer", "noop", "()V");
    assert_virtual_method_ref(&class, select_code, 7, "example/Answer", "identity", "(I)I");
    assert_eq!(emitted_method_code(&class, "onlyUnit").len(), 5);
    assert_eq!(
        emitted_method_code(&class, "onlyUnit"),
        [0x2a, 0xb6, select_code[2], select_code[3], 0xb1]
    );

    if require_jdk_or_skip() {
        verify_emitted_call_class_with_jdk(
            generated.bytes(),
            "public class CallHarness { public static void main(String[] args) { example.Answer answer = new example.Answer(); answer.onlyUnit(); System.out.println(answer.select(true, 7, 9)); } }\n",
            "42",
        );
    }
}

struct ConstructorCallFixture {
    store: SemanticStore,
    ast: TypedAst,
    definitions: Definitions,
    class: SymbolId,
    caller: SymbolId,
    caller_parameters: Vec<SymbolId>,
    constructor: SymbolId,
    apply: TreeId<dotty_core::ast::Typed>,
    initialization: JvmClassInitializationPlan,
}

fn constructor_call_fixture(parameter_types: &[JvmType]) -> ConstructorCallFixture {
    use dotty_core::ast::{
        AstArena, DefDef, Modifier, Modifiers, Template, TypeDef, TypeTree, Untyped,
        UntypedTemplateMetadata, ValDef,
    };

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let source = dotty_core::SourceId::from_index(81);
    let package = store.symbols.alloc(Symbol {
        name: Name::new(store.names.intern(""), Namespace::Term),
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
    let class = store.symbols.alloc(Symbol {
        name: Name::new(store.names.intern("CtorTarget"), Namespace::Type),
        owner: Some(package),
        kind: SymbolKind::Class,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Source(source),
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let no_prefix = definitions.no_prefix;
    let package_type = store.types.alloc(Type::TypeRef {
        prefix: no_prefix,
        target: TypeRefTarget::Symbol(package),
    });
    let class_type = store.types.alloc(Type::TypeRef {
        prefix: package_type,
        target: TypeRefTarget::Symbol(class),
    });
    let declarations = store.scopes.alloc(Scope::new(Some(class)));
    let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
        prefix: no_prefix,
        class,
        parents: vec![definitions.object_type],
        declarations,
        self_type: None,
    }));
    store
        .symbols
        .set_info(class, SymbolInfo::Complete(class_info));

    let mut source_ast = AstArena::<Untyped>::new();
    let mut source_parameters = Vec::new();
    let mut parameter_symbols = Vec::new();
    let mut method_parameters = Vec::new();
    let parameter_type_ids = parameter_types
        .iter()
        .map(|ty| match ty {
            JvmType::Int => definitions.int,
            JvmType::Boolean => definitions.boolean,
            JvmType::Reference(_) => panic!("constructor fixture only accepts primitive types"),
        })
        .collect::<Vec<_>>();
    for (index, ty) in parameter_type_ids.iter().copied().enumerate() {
        let text = format!("arg{index}");
        let name_id = store.names.intern(&text);
        let parameter_tpt = source_ast.alloc(dotty_core::Tree {
            kind: TreeKind::TypeTree(TypeTree),
            position: None,
            ty: (),
        });
        let parameter_tree = source_ast.alloc(dotty_core::Tree {
            kind: TreeKind::ValDef(ValDef {
                name: TermName::new(name_id),
                tpt: parameter_tpt,
                rhs: None,
                metadata: Modifiers {
                    modifiers: vec![Modifier::ParamAccessor, Modifier::PrivateLocal],
                    ..Modifiers::default()
                },
            }),
            position: None,
            ty: (),
        });
        source_parameters.push(parameter_tree);
        let parameter = store.symbols.alloc(Symbol {
            name: Name::new(name_id, Namespace::Term),
            owner: None,
            kind: SymbolKind::Parameter,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(ty),
            origin: SymbolOrigin::Source(source),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        parameter_symbols.push(parameter);
        method_parameters.push(MethodParam {
            name: TermName::new(name_id),
            ty,
            erased: false,
            varargs: false,
        });
    }
    let constructor_name = store.names.intern("<init>");
    let constructor = store.symbols.alloc(Symbol {
        name: Name::new(constructor_name, Namespace::Term),
        owner: Some(class),
        kind: SymbolKind::Constructor,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Source(source),
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    for parameter in &mut parameter_symbols {
        store.symbols.get_mut(*parameter).owner = Some(constructor);
    }
    let constructor_type = store.types.alloc(Type::Method(MethodType {
        params: method_parameters,
        result: class_type,
        kind: MethodKind::Plain,
    }));
    store
        .symbols
        .set_info(constructor, SymbolInfo::Complete(constructor_type));
    let constructor_symbol_name = store.symbols.get(constructor).name;
    store
        .scopes
        .get_mut(declarations)
        .enter(constructor_symbol_name, constructor);

    let type_tpt = source_ast.alloc(dotty_core::Tree {
        kind: TreeKind::TypeTree(TypeTree),
        position: None,
        ty: (),
    });
    let constructor_tree = source_ast.alloc(dotty_core::Tree {
        kind: TreeKind::DefDef(DefDef {
            name: TermName::new(constructor_name),
            type_params: Vec::new(),
            value_param_clauses: vec![source_parameters.clone()],
            source_param_clause_order: None,
            tpt: type_tpt,
            rhs: None,
            metadata: Modifiers::default(),
        }),
        position: None,
        ty: (),
    });
    let template_tree = source_ast.alloc(dotty_core::Tree {
        kind: TreeKind::Template(Template {
            constructor: constructor_tree,
            parents: Vec::new(),
            self_val: None,
            body: Vec::new(),
            metadata: UntypedTemplateMetadata::default(),
        }),
        position: None,
        ty: (),
    });
    let class_tree = source_ast.alloc(dotty_core::Tree {
        kind: TreeKind::TypeDef(TypeDef {
            name: dotty_core::TypeName::new(store.names.intern("CtorTarget")),
            rhs: template_tree,
            metadata: Modifiers::default(),
            variance: None,
        }),
        position: None,
        ty: (),
    });
    let mut index = dotty_core::SourceSemanticIndex::new();
    index.record_symbol(source, class_tree, class).unwrap();
    index
        .record_symbol(source, constructor_tree, constructor)
        .unwrap();
    for (&tree, &parameter) in source_parameters.iter().zip(&parameter_symbols) {
        index
            .record_derived_symbol(constructor, source, tree, parameter)
            .unwrap();
    }
    let initialization = JvmClassInitializationPlan::PrimaryConstructor(
        JvmPrimaryConstructorPlan::from_source(
            class,
            constructor,
            parameter_symbols.clone(),
            &source_ast,
            &index,
        )
        .unwrap(),
    );

    let caller_name = store.names.intern("caller");
    let caller = store.symbols.alloc(Symbol {
        name: Name::new(caller_name, Namespace::Term),
        owner: Some(class),
        kind: SymbolKind::Method,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Source(source),
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let caller_signature = store.types.alloc(Type::Method(MethodType {
        params: Vec::new(),
        result: definitions.int,
        kind: MethodKind::Plain,
    }));
    store
        .symbols
        .set_info(caller, SymbolInfo::Complete(caller_signature));
    store
        .scopes
        .get_mut(declarations)
        .enter(store.symbols.get(caller).name, caller);

    let ten = store.types.alloc(Type::Constant(Constant::Int(10)));
    let truth = store.types.alloc(Type::Constant(Constant::Boolean(true)));
    let function_type = store.types.alloc(Type::TermRef {
        prefix: class_type,
        target: TermRefTarget::Symbol(constructor),
    });
    let mut ast = TypedAst::new();
    let mut builder = dotty_core::ast::TypedAstBuilder::new(&mut ast, &store.types);
    let tpt = builder.type_tree(class_type, None);
    let new_tree = builder.new_expr(tpt, class_type, None);
    let function = builder.select(
        new_tree,
        Name::new(constructor_name, Namespace::Term),
        false,
        function_type,
        None,
    );
    let argument_ids = parameter_types
        .iter()
        .map(|ty| {
            let (constant, literal_ty) = if *ty == JvmType::Boolean {
                (Constant::Boolean(true), truth)
            } else {
                (Constant::Int(10), ten)
            };
            builder.literal(constant, literal_ty, None)
        })
        .collect();
    let apply = builder.apply(function, argument_ids, class_type, None);

    ConstructorCallFixture {
        store,
        ast,
        definitions,
        class,
        caller,
        caller_parameters: Vec::new(),
        constructor,
        apply,
        initialization,
    }
}

fn constructor_fixture_body(
    fixture: &mut ConstructorCallFixture,
) -> TreeId<dotty_core::ast::Typed> {
    let zero = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(0),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![fixture.apply],
            expr: zero,
        }),
        position: None,
        ty: fixture.definitions.int,
    })
}

fn constructor_fixture_unit(
    fixture: &ConstructorCallFixture,
    body: TreeId<dotty_core::ast::Typed>,
) -> JvmCompilationUnit<'_> {
    JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: fixture.caller_parameters.clone(),
                locals: Vec::new(),
                body: Some(body),
            }],
            fields: Vec::new(),
            initialization: fixture.initialization.clone(),
        }],
    }
}

fn assert_special_method_ref(
    class: &ClassFile,
    code: &[u8],
    opcode_offset: usize,
    owner: &str,
    method: &str,
    descriptor: &str,
) {
    assert_eq!(code[opcode_offset], 0xb7);
    let index = u16::from_be_bytes([code[opcode_offset + 1], code[opcode_offset + 2]]);
    let ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    } = class.constant_pool.get(ConstantPoolIndex(index)).unwrap()
    else {
        panic!("invokespecial must reference a Methodref constant")
    };
    assert_eq!(class.constant_pool.class_name(*class_index), Ok(owner));
    assert_eq!(
        class.constant_pool.name_and_type(*name_and_type_index),
        Ok((method, descriptor))
    );
}

fn assert_new_class_ref(class: &ClassFile, code: &[u8], owner: &str) {
    assert_eq!(code[0], 0xbb);
    let index = u16::from_be_bytes([code[1], code[2]]);
    let ConstantPoolEntry::Class {
        name_index: class_index,
    } = class.constant_pool.get(ConstantPoolIndex(index)).unwrap()
    else {
        panic!("new must reference a Class constant")
    };
    assert_eq!(class.constant_pool.utf8(*class_index), Ok(owner));
}

#[test]
fn emits_initialized_object_expression_after_exact_constructor_sequence() {
    let mut fixture = constructor_call_fixture(&[JvmType::Boolean, JvmType::Int]);
    let body = constructor_fixture_body(&mut fixture);
    let unit = constructor_fixture_unit(&fixture, body);
    let flow_unit = constructor_fixture_unit(&fixture, fixture.apply);
    assert!(matches!(
        flow_unit.lower_method_body(fixture.class, fixture.caller, &fixture.definitions),
        Err(JvmCodegenError::MethodBodyTypeMismatch {
            found: Some(JvmType::Reference(class)),
            ..
        }) if class == fixture.class
    ));
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    class.validate_references().unwrap();
    let code = emitted_method_code(&class, "caller");
    assert_new_class_ref(&class, code, "CtorTarget");
    assert_eq!(&code[3..8], &[0x59, 0x04, 0x10, 10, 0xb7]);
    assert_special_method_ref(&class, code, 7, "CtorTarget", "<init>", "(ZI)V");
    assert_eq!(&code[10..], &[0x57, 0x03, 0xac]);
    if require_jdk_or_skip() {
        verify_constructor_class_with_jdk(generated.bytes());
    }
}

#[test]
fn emits_a_proven_no_argument_constructor_without_fabricating_a_target() {
    let mut fixture = constructor_call_fixture(&[]);
    let body = constructor_fixture_body(&mut fixture);
    let unit = constructor_fixture_unit(&fixture, body);
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "caller");
    assert_new_class_ref(&class, code, "CtorTarget");
    assert_eq!(&code[3..4], &[0x59]);
    assert_special_method_ref(&class, code, 4, "CtorTarget", "<init>", "()V");
    assert_eq!(&code[7..], &[0x57, 0x03, 0xac]);
}

#[test]
fn emits_parameter_arithmetic_once_before_the_approved_constructor_call() {
    let mut fixture = constructor_call_fixture(&[JvmType::Boolean, JvmType::Int]);
    let parameter_name = fixture.store.names.intern("seed");
    let parameter = fixture.store.symbols.alloc(Symbol {
        name: Name::new(parameter_name, Namespace::Term),
        owner: Some(fixture.caller),
        kind: SymbolKind::Parameter,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(fixture.definitions.int),
        origin: SymbolOrigin::Source(dotty_core::SourceId::from_index(81)),
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    fixture.caller_parameters.push(parameter);
    let signature = fixture.store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: TermName::new(parameter_name),
            ty: fixture.definitions.int,
            erased: false,
            varargs: false,
        }],
        result: fixture.definitions.int,
        kind: MethodKind::Plain,
    }));
    fixture
        .store
        .symbols
        .set_info(fixture.caller, SymbolInfo::Complete(signature));

    let parameter_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(parameter),
    });
    let seed = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(Ident {
            name: Name::new(parameter_name, Namespace::Term),
            backquoted: false,
        }),
        position: None,
        ty: parameter_type,
    });
    let operator = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .plus;
    let function_type = fixture.store.types.alloc(Type::TermRef {
        prefix: parameter_type,
        target: TermRefTarget::Symbol(operator),
    });
    let function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(dotty_core::ast::Select {
            qualifier: seed,
            name: fixture.store.symbols.get(operator).name,
            backquoted: false,
        }),
        position: None,
        ty: function_type,
    });
    let one_type = fixture.store.types.alloc(Type::Constant(Constant::Int(1)));
    let one = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: one_type,
    });
    let sum = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args: vec![one],
            kind: ApplyKind::Regular,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let application = fixture.ast.get(fixture.apply).clone();
    let TreeKind::Apply(mut application) = application.kind else {
        unreachable!()
    };
    application.args[1] = sum;
    fixture.ast.get_mut(fixture.apply).kind = TreeKind::Apply(application);

    let body = constructor_fixture_body(&mut fixture);
    let unit = constructor_fixture_unit(&fixture, body);
    let generated = emit_class(&unit, fixture.class, &fixture.definitions).unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "caller");
    assert_new_class_ref(&class, code, "CtorTarget");
    assert_eq!(&code[3..9], &[0x59, 0x04, 0x1b, 0x04, 0x60, 0xb7]);
    assert_special_method_ref(&class, code, 8, "CtorTarget", "<init>", "(ZI)V");
    assert_eq!(&code[11..], &[0x57, 0x03, 0xac]);
}

#[test]
fn rejects_branching_constructor_arguments_before_allocating() {
    let mut fixture = constructor_call_fixture(&[JvmType::Int]);
    let boolean = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });
    let one = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let two = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(2),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let conditional = fixture.ast.alloc(Tree {
        kind: TreeKind::If(dotty_core::ast::If {
            cond: boolean,
            then_branch: one,
            else_branch: two,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let application = fixture.ast.get(fixture.apply).clone();
    let TreeKind::Apply(mut application) = application.kind else {
        unreachable!()
    };
    application.args[0] = conditional;
    fixture.ast.get_mut(fixture.apply).kind = TreeKind::Apply(application);
    let unit = constructor_fixture_unit(&fixture, fixture.apply);
    assert!(matches!(
        unit.lower_method_body(fixture.class, fixture.caller, &fixture.definitions),
        Err(JvmCodegenError::UnsupportedJvmConstructorArgument { argument, .. })
            if argument == conditional
    ));
}

#[test]
fn allows_branches_before_or_after_a_complete_allocation_sequence() {
    for allocation_after_branch in [false, true] {
        let mut fixture = constructor_call_fixture(&[JvmType::Boolean, JvmType::Int]);
        let condition = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Boolean(true),
            }),
            position: None,
            ty: fixture.definitions.boolean,
        });
        let one = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(1),
            }),
            position: None,
            ty: fixture.definitions.int,
        });
        let two = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(2),
            }),
            position: None,
            ty: fixture.definitions.int,
        });
        let branch = fixture.ast.alloc(Tree {
            kind: TreeKind::If(dotty_core::ast::If {
                cond: condition,
                then_branch: one,
                else_branch: two,
            }),
            position: None,
            ty: fixture.definitions.int,
        });
        let body = fixture.ast.alloc(Tree {
            kind: TreeKind::Block(Block {
                stats: if allocation_after_branch {
                    vec![branch, fixture.apply]
                } else {
                    vec![fixture.apply, branch]
                },
                expr: one,
            }),
            position: None,
            ty: fixture.definitions.int,
        });
        let unit = constructor_fixture_unit(&fixture, body);
        emit_class(&unit, fixture.class, &fixture.definitions)
            .expect("a complete allocation sequence may coexist with verified branches");
    }
}

#[test]
fn allows_same_class_allocations_in_if_branches_and_joins_exact_references() {
    let mut fixture = constructor_call_fixture(&[JvmType::Boolean, JvmType::Int]);
    let condition = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Boolean(true),
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });
    let conditional = fixture.ast.alloc(Tree {
        kind: TreeKind::If(dotty_core::ast::If {
            cond: condition,
            then_branch: fixture.apply,
            else_branch: fixture.apply,
        }),
        position: None,
        ty: fixture.store.types.alloc(Type::TypeRef {
            prefix: fixture.definitions.no_prefix,
            target: TypeRefTarget::Symbol(fixture.class),
        }),
    });
    let one = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(1),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let body = fixture.ast.alloc(Tree {
        kind: TreeKind::Block(Block {
            stats: vec![conditional],
            expr: one,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let unit = constructor_fixture_unit(&fixture, body);
    emit_class(&unit, fixture.class, &fixture.definitions)
        .expect("both branches initialize the same exact class before their join");
}

#[test]
fn rejects_different_exact_reference_classes_at_an_if_expression_join() {
    let mut fixture = EmissionFixture::new(false);
    let class_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: fixture.definitions.no_prefix,
        target: TypeRefTarget::Symbol(fixture.class),
    });
    let owner = fixture.store.symbols.get(fixture.class).owner.unwrap();
    let no_prefix = fixture.definitions.no_prefix;
    let object_type = fixture.definitions.object_type;
    let other = add_reference_class(
        &mut fixture.store,
        owner,
        "OtherReference",
        no_prefix,
        object_type,
        SymbolOrigin::Synthetic,
        SymbolFlags::EMPTY,
    );
    let other_type = fixture.store.types.alloc(Type::TypeRef {
        prefix: no_prefix,
        target: TypeRefTarget::Symbol(other),
    });
    let declarations = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("fixture class info should be ClassInfo"),
        },
        _ => panic!("fixture class info should be complete"),
    };
    let (method, parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        "selectReference",
        &[
            ("condition", fixture.definitions.boolean),
            ("other", other_type),
        ],
        class_type,
    );
    let no_prefix = fixture.definitions.no_prefix;
    let condition = typed_method_ident_with_prefix(&mut fixture, parameters[0], no_prefix);
    let other_value = typed_method_ident_with_prefix(&mut fixture, parameters[1], no_prefix);
    let this_type = fixture.store.types.alloc(Type::ThisType {
        class: fixture.class,
    });
    let current = fixture.ast.alloc(Tree {
        kind: TreeKind::This(This { qual: None }),
        position: None,
        ty: this_type,
    });
    let body = fixture.ast.alloc(Tree {
        kind: TreeKind::If(dotty_core::ast::If {
            cond: condition,
            then_branch: current,
            else_branch: other_value,
        }),
        position: None,
        ty: class_type,
    });
    fixture.methods.push(method);
    fixture.parameters.push(parameters);
    fixture.bodies.push(body);
    let mut unit = fixture.unit();
    unit.classes.push(JvmClassPlan {
        class: other,
        fields: Vec::new(),
        methods: Vec::new(),
        initialization: JvmClassInitializationPlan::ImplicitNoArg,
    });

    assert_eq!(
        emit_class(&unit, fixture.class, &fixture.definitions),
        Err(JvmCodegenError::ReferenceTypeMismatch {
            expected: fixture.class,
            found: other,
        })
    );
}

#[test]
fn reference_locals_initialized_only_in_a_branch_or_loop_are_not_definite_after_exit() {
    let check = |inside_loop: bool| {
        let mut fixture = EmissionFixture::new(false);
        let class_type = fixture.store.types.alloc(Type::TypeRef {
            prefix: fixture.definitions.no_prefix,
            target: TypeRefTarget::Symbol(fixture.class),
        });
        let boolean_type = fixture.definitions.boolean;
        add_call_target_method(
            &mut fixture,
            if inside_loop {
                "readLoopOnlyReference"
            } else {
                "readBranchOnlyReference"
            },
            &[("condition", boolean_type)],
            class_type,
        );
        let method_index = fixture.methods.len() - 1;
        let condition_parameter = fixture.parameters[method_index][0];
        let no_prefix = fixture.definitions.no_prefix;
        let condition =
            typed_method_ident_with_prefix(&mut fixture, condition_parameter, no_prefix);
        let local = add_emission_local_symbol(
            &mut fixture,
            method_index,
            "conditionalReference",
            class_type,
            false,
            SymbolFlags::EMPTY,
        );
        let this_type = fixture.store.types.alloc(Type::ThisType {
            class: fixture.class,
        });
        let initializer = fixture.ast.alloc(Tree {
            kind: TreeKind::This(This { qual: None }),
            position: None,
            ty: this_type,
        });
        let declaration = add_emission_local_valdef(
            &mut fixture,
            method_index,
            local,
            class_type,
            Some(initializer),
        );
        let unit_type = fixture.definitions.unit;
        let unit_value = fixture.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Unit,
            }),
            position: None,
            ty: unit_type,
        });
        let control = if inside_loop {
            let loop_body = typed_block(&mut fixture, vec![declaration], unit_value, unit_type);
            fixture.ast.alloc(Tree {
                kind: TreeKind::While(dotty_core::ast::While {
                    cond: condition,
                    body: loop_body,
                }),
                position: None,
                ty: unit_type,
            })
        } else {
            let then_branch = typed_block(&mut fixture, vec![declaration], unit_value, unit_type);
            let else_branch = fixture.ast.alloc(Tree {
                kind: TreeKind::Literal(Literal {
                    value: Constant::Unit,
                }),
                position: None,
                ty: unit_type,
            });
            fixture.ast.alloc(Tree {
                kind: TreeKind::If(dotty_core::ast::If {
                    cond: condition,
                    then_branch,
                    else_branch,
                }),
                position: None,
                ty: unit_type,
            })
        };
        let read = typed_emission_symbol_ident(&mut fixture, local);
        fixture.bodies[method_index] = typed_block(&mut fixture, vec![control], read, class_type);
        let mut unit = fixture.unit();
        unit.classes[0].methods[method_index].locals = vec![JvmLocalBinding {
            declaration,
            symbol: local,
            mutable: false,
        }];

        assert_eq!(
            emit_class(&unit, fixture.class, &fixture.definitions),
            Err(JvmCodegenError::UninitializedLocalRead {
                tree: read,
                symbol: local
            })
        );
    };

    check(false);
    check(true);
}

#[test]
fn resolves_typed_constructor_application_to_canonical_metadata() {
    let fixture = constructor_call_fixture(&[JvmType::Boolean, JvmType::Int]);
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(fixture.apply),
            }],
            fields: Vec::new(),
            initialization: fixture.initialization.clone(),
        }],
    };
    let target = unit
        .resolve_constructor_call(
            fixture.class,
            fixture.caller,
            fixture.apply,
            &fixture.definitions,
        )
        .unwrap();
    assert_eq!(target.owner_class(), fixture.class);
    assert_eq!(target.constructor_symbol(), fixture.constructor);
    assert_eq!(target.owner_internal_name(), "CtorTarget");
    assert_eq!(target.descriptor().as_str(), "(ZI)V");
    assert_eq!(target.argument_types(), &[JvmType::Boolean, JvmType::Int]);
    assert_eq!(target.result_type(), JvmType::Reference(fixture.class));
    assert_eq!(
        target.invocation_kind(),
        crate::JvmConstructorInvocationKind::Special
    );
}

#[test]
fn rejects_constructor_calls_with_wrong_arity_or_argument_type() {
    let mut fixture = constructor_call_fixture(&[JvmType::Boolean, JvmType::Int]);
    let application = fixture.ast.get(fixture.apply).clone();
    let TreeKind::Apply(mut apply) = application.kind else {
        unreachable!("fixture contains an application")
    };
    apply.args.pop();
    fixture.ast.get_mut(fixture.apply).kind = TreeKind::Apply(apply);
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(fixture.apply),
            }],
            fields: Vec::new(),
            initialization: fixture.initialization.clone(),
        }],
    };
    assert!(matches!(
        unit.resolve_constructor_call(
            fixture.class,
            fixture.caller,
            fixture.apply,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::JvmConstructorCallArityMismatch {
            expected: 2,
            found: 1,
            ..
        })
    ));

    let mut fixture = constructor_call_fixture(&[JvmType::Boolean, JvmType::Int]);
    let application = fixture.ast.get(fixture.apply).clone();
    let TreeKind::Apply(apply) = application.kind else {
        unreachable!("fixture contains an application")
    };
    fixture.ast.get_mut(apply.args[0]).ty = fixture.definitions.int;
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(fixture.apply),
            }],
            fields: Vec::new(),
            initialization: fixture.initialization.clone(),
        }],
    };
    assert!(matches!(
        unit.resolve_constructor_call(
            fixture.class,
            fixture.caller,
            fixture.apply,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::JvmConstructorCallArgumentTypeMismatch {
            expected: JvmType::Boolean,
            found: Some(JvmType::Int),
            ..
        })
    ));
}

#[test]
fn rejects_a_constructor_reference_owned_by_another_class() {
    let mut fixture = constructor_call_fixture(&[]);
    let foreign_class = alloc_symbol(
        &mut fixture.store,
        SymbolKind::Class,
        Some(fixture.class),
        fixture.definitions.object_type,
    );
    let constructor_name = fixture.store.names.intern("<init>");
    let foreign_constructor = fixture.store.symbols.alloc(Symbol {
        name: Name::new(constructor_name, Namespace::Term),
        owner: Some(foreign_class),
        kind: SymbolKind::Constructor,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let application = fixture.ast.get(fixture.apply).clone();
    let TreeKind::Apply(apply) = application.kind else {
        unreachable!("fixture contains an application")
    };
    let function = fixture.ast.get(apply.function).clone();
    let TreeKind::Select(select) = function.kind else {
        unreachable!("fixture contains a constructor selection")
    };
    let class_type = fixture.ast.get(select.qualifier).ty;
    let function_type = fixture.store.types.alloc(Type::TermRef {
        prefix: class_type,
        target: TermRefTarget::Symbol(foreign_constructor),
    });
    fixture.ast.get_mut(apply.function).ty = function_type;
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(fixture.apply),
            }],
            fields: Vec::new(),
            initialization: fixture.initialization.clone(),
        }],
    };
    assert!(matches!(
        unit.resolve_constructor_call(
            fixture.class,
            fixture.caller,
            fixture.apply,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::JvmConstructorCallWrongOwner {
            expected,
            actual: Some(actual),
            ..
        }) if expected == fixture.class && actual == foreign_class
    ));
}

#[test]
fn rejects_a_constructor_application_with_a_nonclass_result_type() {
    let mut fixture = constructor_call_fixture(&[]);
    fixture.ast.get_mut(fixture.apply).ty = fixture.definitions.int;
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(fixture.apply),
            }],
            fields: Vec::new(),
            initialization: fixture.initialization.clone(),
        }],
    };
    assert!(matches!(
        unit.resolve_constructor_call(
            fixture.class,
            fixture.caller,
            fixture.apply,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::JvmConstructorCallResultTypeMismatch {
            expected,
            found: None,
            ..
        }) if expected == fixture.class
    ));
}

#[test]
fn implicit_no_arg_resolution_requires_the_selected_declared_constructor() {
    let fixture = constructor_call_fixture(&[]);
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(fixture.apply),
            }],
            fields: Vec::new(),
            initialization: JvmClassInitializationPlan::ImplicitNoArg,
        }],
    };
    let target = unit
        .resolve_constructor_call(
            fixture.class,
            fixture.caller,
            fixture.apply,
            &fixture.definitions,
        )
        .unwrap();
    assert_eq!(target.constructor_symbol(), fixture.constructor);
    assert_eq!(target.descriptor().as_str(), "()V");
    assert!(target.argument_types().is_empty());
}

#[test]
fn implicit_no_arg_resolution_rejects_unplanned_class_fields() {
    let mut fixture = constructor_call_fixture(&[]);
    let field_name = fixture.store.names.intern("stored");
    let field = fixture.store.symbols.alloc(Symbol {
        name: Name::new(field_name, Namespace::Term),
        owner: Some(fixture.class),
        kind: SymbolKind::Field,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Private,
        info: SymbolInfo::Complete(fixture.definitions.int),
        origin: SymbolOrigin::Source(dotty_core::SourceId::from_index(81)),
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let class_info = crate::identity::class_info(&fixture.store, fixture.class).unwrap();
    let field_name = fixture.store.symbols.get(field).name;
    fixture
        .store
        .scopes
        .get_mut(class_info.declarations)
        .enter(field_name, field);
    let unit = JvmCompilationUnit {
        store: &fixture.store,
        typed_ast: &fixture.ast,
        classes: vec![JvmClassPlan {
            class: fixture.class,
            methods: vec![JvmMethodPlan {
                method: fixture.caller,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(fixture.apply),
            }],
            fields: Vec::new(),
            initialization: JvmClassInitializationPlan::ImplicitNoArg,
        }],
    };
    assert_eq!(
        unit.resolve_constructor_call(
            fixture.class,
            fixture.caller,
            fixture.apply,
            &fixture.definitions,
        ),
        Err(JvmCodegenError::FieldEmissionUnsupported {
            class: fixture.class,
            field,
        })
    );
}

#[test]
fn emits_calls_in_control_flow_branches_with_reference_frames() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let identity =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let identity_parameter = fixture.parameters[2][0];
    set_method_body_to_parameter(&mut fixture, identity, identity_parameter);

    let prefix = fixture.definitions.no_prefix;
    let parameters = fixture.parameters[1].clone();
    let condition = typed_method_ident_with_prefix(&mut fixture, parameters[0], prefix);
    let left = typed_method_ident_with_prefix(&mut fixture, parameters[1], prefix);
    let right = typed_method_ident_with_prefix(&mut fixture, parameters[2], prefix);
    let then_call = typed_call_to(&mut fixture, identity, vec![left], int_type, false);
    let else_call = typed_call_to(&mut fixture, identity, vec![right], int_type, false);
    fixture.bodies[1] = fixture.ast.alloc(Tree {
        kind: TreeKind::If(If {
            cond: condition,
            then_branch: then_call,
            else_branch: else_call,
        }),
        position: None,
        ty: int_type,
    });

    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert!(code.contains(&0x99), "Boolean branch remains in the method");
    assert_eq!(code.iter().filter(|opcode| **opcode == 0xb6).count(), 2);
}

#[test]
fn emits_explicit_empty_clause_calls_and_unit_results() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let constant = add_call_target_method(&mut fixture, "constant", &[], int_type);
    let call = typed_call_to(&mut fixture, constant, Vec::new(), int_type, false);
    fixture.bodies[1] = call;
    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(code.len(), 5);
    assert_eq!(&code[..2], &[0x2a, 0xb6]);
    assert_eq!(code[4], 0xac);
    assert_virtual_method_ref(&class, code, 1, "example/Answer", "constant", "()I");

    let unit_type = fixture.definitions.unit;
    let noop = add_call_target_method(&mut fixture, "noop", &[], unit_type);
    let declarations = match fixture.store.symbols.info(fixture.class) {
        SymbolInfo::Complete(ty) => match fixture.store.types.get(*ty) {
            Type::ClassInfo(info) => info.declarations,
            _ => panic!("fixture class info should be ClassInfo"),
        },
        _ => panic!("fixture class info should be complete"),
    };
    let (run, run_parameters) = add_emission_method(
        &mut fixture.store,
        fixture.class,
        declarations,
        "run",
        &[],
        unit_type,
    );
    fixture.methods.push(run);
    fixture.parameters.push(run_parameters);
    let unit_body = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Unit,
        }),
        position: None,
        ty: unit_type,
    });
    fixture.bodies.push(unit_body);
    let call = typed_call_to(&mut fixture, noop, Vec::new(), unit_type, false);
    let run_index = fixture
        .methods
        .iter()
        .position(|method| *method == run)
        .unwrap();
    fixture.bodies[run_index] = call;
    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "run");
    assert_eq!(code.len(), 5);
    assert_eq!(&code[..2], &[0x2a, 0xb6]);
    assert_eq!(code[4], 0xb1);
    assert_virtual_method_ref(&class, code, 1, "example/Answer", "noop", "()V");
}

#[test]
fn emits_the_overload_selected_by_the_canonical_method_symbol() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let boolean_type = fixture.definitions.boolean;
    let _integer_overload =
        add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let boolean_overload = add_call_target_method(
        &mut fixture,
        "identity",
        &[("value", boolean_type)],
        int_type,
    );
    let flag = fixture.parameters[1][0];
    let prefix = fixture.definitions.no_prefix;
    let argument = typed_method_ident_with_prefix(&mut fixture, flag, prefix);
    let call = typed_call_to(
        &mut fixture,
        boolean_overload,
        vec![argument],
        int_type,
        false,
    );
    fixture.bodies[1] = call;

    let generated = fixture.emit().unwrap();
    let mut reader = Reader::new(generated.bytes());
    let class = ClassFile::decode(&mut reader).unwrap();
    let code = emitted_method_code(&class, "select");
    assert_eq!(&code[..3], &[0x2a, 0x1b, 0xb6]);
    assert_eq!(code[5], 0xac);
    assert_virtual_method_ref(&class, code, 2, "example/Answer", "identity", "(Z)I");
}

#[test]
fn rejects_call_arity_argument_and_result_mismatches_without_class_output() {
    let mut arity = EmissionFixture::new(false);
    let int_type = arity.definitions.int;
    let target = add_call_target_method(&mut arity, "identity", &[("value", int_type)], int_type);
    let call = typed_call_to(&mut arity, target, Vec::new(), int_type, false);
    arity.bodies[1] = call;
    assert_eq!(
        arity.emit().unwrap_err(),
        JvmCodegenError::JvmCallArityMismatch {
            tree: call,
            method: target,
            expected: 1,
            found: 0,
        }
    );

    let mut argument_type = EmissionFixture::new(false);
    let int_type = argument_type.definitions.int;
    let target = add_call_target_method(
        &mut argument_type,
        "identity",
        &[("value", int_type)],
        int_type,
    );
    let boolean_parameter = argument_type.parameters[1][0];
    let prefix = argument_type.definitions.no_prefix;
    let argument = typed_method_ident_with_prefix(&mut argument_type, boolean_parameter, prefix);
    let call = typed_call_to(&mut argument_type, target, vec![argument], int_type, false);
    argument_type.bodies[1] = call;
    assert_eq!(
        argument_type.emit().unwrap_err(),
        JvmCodegenError::JvmCallArgumentMismatch {
            tree: call,
            argument,
            method: target,
            index: 0,
            expected: JvmType::Int,
            found: JvmExpressionFlow::Value {
                value_type: JvmType::Boolean,
                category: JvmValueCategory::IntLike,
            },
        }
    );

    let mut unit_argument = EmissionFixture::new(false);
    let int_type = unit_argument.definitions.int;
    let target = add_call_target_method(
        &mut unit_argument,
        "identity",
        &[("value", int_type)],
        int_type,
    );
    let unit_type = unit_argument.definitions.unit;
    let argument = unit_argument.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Unit,
        }),
        position: None,
        ty: unit_type,
    });
    let call = typed_call_to(&mut unit_argument, target, vec![argument], int_type, false);
    unit_argument.bodies[1] = call;
    assert_eq!(
        unit_argument.emit().unwrap_err(),
        JvmCodegenError::JvmCallArgumentMismatch {
            tree: call,
            argument,
            method: target,
            index: 0,
            expected: JvmType::Int,
            found: JvmExpressionFlow::Unit,
        }
    );

    let mut diverging_argument = EmissionFixture::new(false);
    let int_type = diverging_argument.definitions.int;
    let target = add_call_target_method(
        &mut diverging_argument,
        "identity",
        &[("value", int_type)],
        int_type,
    );
    let value = diverging_argument.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(4),
        }),
        position: None,
        ty: int_type,
    });
    let argument = diverging_argument.ast.alloc(Tree {
        kind: TreeKind::Return(dotty_core::ast::Return {
            expr: Some(value),
            from: None,
        }),
        position: None,
        ty: diverging_argument.definitions.nothing_type,
    });
    let call = typed_call_to(
        &mut diverging_argument,
        target,
        vec![argument],
        int_type,
        false,
    );
    diverging_argument.bodies[1] = call;
    assert_eq!(
        diverging_argument.emit().unwrap_err(),
        JvmCodegenError::JvmCallArgumentMismatch {
            tree: call,
            argument,
            method: target,
            index: 0,
            expected: JvmType::Int,
            found: JvmExpressionFlow::Diverges,
        }
    );

    let mut result_type = EmissionFixture::new(false);
    let int_type = result_type.definitions.int;
    let target = add_call_target_method(
        &mut result_type,
        "identity",
        &[("value", int_type)],
        int_type,
    );
    let argument = result_type.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(4),
        }),
        position: None,
        ty: int_type,
    });
    let boolean_type = result_type.definitions.boolean;
    let call = typed_call_to(
        &mut result_type,
        target,
        vec![argument],
        boolean_type,
        false,
    );
    result_type.bodies[1] = call;
    assert_eq!(
        result_type.emit().unwrap_err(),
        JvmCodegenError::JvmCallResultTypeMismatch {
            tree: call,
            method: target,
            expected: JvmReturnType::Value(JvmType::Int),
            found: Some(JvmReturnType::Value(JvmType::Boolean)),
        }
    );
}

#[test]
fn rejects_a_missing_call_argument_tree_without_publishing_a_class() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let target = add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let mut foreign_ast = TypedAst::new();
    let missing = (0..100)
        .map(|_| {
            foreign_ast.alloc(Tree {
                kind: TreeKind::Literal(Literal {
                    value: Constant::Int(1),
                }),
                position: None,
                ty: int_type,
            })
        })
        .last()
        .unwrap();
    let call = typed_call_to(&mut fixture, target, vec![missing], int_type, false);
    fixture.bodies[1] = call;

    assert_eq!(
        fixture.emit().unwrap_err(),
        JvmCodegenError::UnsupportedTypedExpression {
            tree: missing,
            node: "missing tree",
        }
    );
}

#[test]
fn rejects_unsupported_call_receivers_and_static_caller_contexts() {
    let mut fixture = EmissionFixture::new(false);
    let int_type = fixture.definitions.int;
    let target = add_call_target_method(&mut fixture, "identity", &[("value", int_type)], int_type);
    let argument = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(3),
        }),
        position: None,
        ty: int_type,
    });

    let receiver_symbol = fixture.parameters[1][0];
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
    let selected_type = fixture.store.types.alloc(Type::TermRef {
        prefix: receiver_type,
        target: TermRefTarget::Symbol(target),
    });
    let selected = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name: fixture.store.symbols.get(target).name,
            backquoted: false,
        }),
        position: None,
        ty: selected_type,
    });
    let call = typed_call(
        &mut fixture,
        selected,
        vec![argument],
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture
            .unit()
            .lower_same_class_call_receiver(
                fixture.class,
                fixture.methods[1],
                call,
                &fixture.definitions,
            )
            .unwrap_err(),
        JvmCodegenError::UnsupportedJvmCallShape {
            tree: selected,
            kind: UnsupportedJvmCallShapeKind::NonThisReceiver,
        }
    );

    let this_type = fixture.store.types.alloc(Type::ThisType {
        class: fixture.class,
    });
    let qualified_this = fixture.ast.alloc(Tree {
        kind: TreeKind::This(This {
            qual: Some(fixture.store.symbols.get(fixture.class).name),
        }),
        position: None,
        ty: this_type,
    });
    let qualified_function_type = fixture.store.types.alloc(Type::TermRef {
        prefix: this_type,
        target: TermRefTarget::Symbol(target),
    });
    let qualified_function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: qualified_this,
            name: fixture.store.symbols.get(target).name,
            backquoted: false,
        }),
        position: None,
        ty: qualified_function_type,
    });
    let qualified_call = typed_call(
        &mut fixture,
        qualified_function,
        vec![argument],
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture
            .unit()
            .lower_same_class_call_receiver(
                fixture.class,
                fixture.methods[1],
                qualified_call,
                &fixture.definitions,
            )
            .unwrap_err(),
        JvmCodegenError::UnsupportedJvmCallShape {
            tree: qualified_this,
            kind: UnsupportedJvmCallShapeKind::QualifiedThisReceiver,
        }
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
    let other_selected_type = fixture.store.types.alloc(Type::TermRef {
        prefix: other_this_type,
        target: TermRefTarget::Symbol(target),
    });
    let other_selected = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: other_this,
            name: fixture.store.symbols.get(target).name,
            backquoted: false,
        }),
        position: None,
        ty: other_selected_type,
    });
    let other_call = typed_call(
        &mut fixture,
        other_selected,
        vec![argument],
        ApplyKind::Regular,
        int_type,
    );
    assert_eq!(
        fixture
            .unit()
            .lower_same_class_call_receiver(
                fixture.class,
                fixture.methods[1],
                other_call,
                &fixture.definitions,
            )
            .unwrap_err(),
        JvmCodegenError::UnsupportedJvmCallShape {
            tree: other_selected,
            kind: UnsupportedJvmCallShapeKind::NonCurrentThis,
        }
    );

    let caller = fixture.methods[1];
    let static_flags = fixture
        .store
        .symbols
        .get(caller)
        .flags
        .union(SymbolFlags::STATIC);
    fixture.store.symbols.get_mut(caller).flags = static_flags;
    assert_eq!(
        fixture
            .unit()
            .lower_same_class_call_receiver(
                fixture.class,
                caller,
                qualified_call,
                &fixture.definitions,
            )
            .unwrap_err(),
        JvmCodegenError::UnsupportedJvmSymbolFlag {
            symbol: caller,
            flag: UnsupportedJvmSymbolFlag::Static,
        }
    );
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
    let target = private
        .unit()
        .resolve_same_class_call(
            private.class,
            private.methods[1],
            call,
            &private.definitions,
        )
        .expect("private calls within the owning class are valid");
    assert_eq!(target.method_symbol(), private_method);

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
    let directory = jdk_test_directory("emission");
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

fn verify_emitted_call_class_with_jdk(bytes: &[u8], harness_source: &str, expected_output: &str) {
    let directory = jdk_test_directory("call-emission");
    let class_directory = directory.join("example");
    fs::create_dir_all(&class_directory).unwrap();
    fs::write(class_directory.join("Answer.class"), bytes).unwrap();
    let classpath = directory.to_str().unwrap();

    let javap = Command::new("javap")
        .args(["-c", "-classpath", classpath, "example.Answer"])
        .output()
        .unwrap();
    assert!(
        javap.status.success(),
        "javap rejected emitted call class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javap.stdout),
        String::from_utf8_lossy(&javap.stderr)
    );
    assert!(String::from_utf8_lossy(&javap.stdout).contains("invokevirtual"));

    let harness = directory.join("CallHarness.java");
    fs::write(&harness, harness_source).unwrap();
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .unwrap();
    assert!(
        javac.status.success(),
        "javac rejected emitted call harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args(["-Xverify:all", "-classpath", classpath, "CallHarness"])
        .output()
        .unwrap();
    assert!(
        java.status.success(),
        "java -Xverify:all rejected emitted call class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&java.stdout).trim(),
        expected_output
    );
    fs::remove_dir_all(directory).unwrap();
}

fn verify_constructor_class_with_jdk(bytes: &[u8]) {
    let directory = jdk_test_directory("constructor-emission");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("CtorTarget.class"), bytes).unwrap();
    let harness = directory.join("ConstructorHarness.java");
    fs::write(
        &harness,
        "public class ConstructorHarness { public static void main(String[] args) { System.out.println(new CtorTarget(true, 10).caller()); } }\n",
    )
    .unwrap();
    let classpath = directory.to_str().unwrap();
    let javac = Command::new("javac")
        .args(["-classpath", classpath, "-d", classpath])
        .arg(&harness)
        .output()
        .unwrap();
    assert!(
        javac.status.success(),
        "javac rejected constructor harness\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&javac.stdout),
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .args([
            "-Xverify:all",
            "-classpath",
            classpath,
            "ConstructorHarness",
        ])
        .output()
        .unwrap();
    assert!(
        java.status.success(),
        "java -Xverify:all rejected allocated class\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&java.stdout),
        String::from_utf8_lossy(&java.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&java.stdout).trim(), "0");
    fs::remove_dir_all(directory).unwrap();
}

fn jdk_test_directory(name: &str) -> std::path::PathBuf {
    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "dotty-jvm-{name}-{}-{sequence}",
        std::process::id()
    ))
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
        Err(JvmCodegenError::FieldEmissionUnsupported {
            class: fixture.class,
            field,
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
    let int_parent = match superclass.store.types.get(superclass.definitions.int) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(parent),
            ..
        } => *parent,
        other => panic!("Int definition should be a canonical type reference: {other:?}"),
    };
    assert_eq!(
        superclass.emit(),
        Err(JvmCodegenError::InvalidJvmSuperclass {
            class: superclass.class,
            parent: Some(int_parent),
            issue: crate::JvmSuperclassIssue::ParentNotPlanned,
        })
    );
}

#[test]
fn compilation_unit_metadata_rejects_a_stale_term_reference_in_field_initializer() {
    let mut fixture = EmissionFixture::new(false);
    let mut other_store = SemanticStore::new();
    let other_type = other_store.types.alloc(Type::Constant(Constant::Int(0)));

    let field_name = fixture.store.names.intern("field");
    let field = fixture.store.symbols.alloc(Symbol {
        name: Name::new(field_name, Namespace::Term),
        owner: Some(fixture.class),
        kind: SymbolKind::Field,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Private,
        info: SymbolInfo::Complete(fixture.definitions.int),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let stale_symbol = loop {
        let symbol = alloc_symbol(&mut other_store, SymbolKind::Value, None, other_type);
        if !fixture.store.symbols.contains(symbol) {
            break symbol;
        }
    };
    let initializer_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(stale_symbol),
    });
    let initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(42),
        }),
        position: None,
        ty: initializer_type,
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

    let mut unit = fixture.unit();
    unit.classes[0].fields.push(JvmFieldPlan {
        field,
        source_visibility: crate::JvmFieldSourceVisibility::PlainPrivate,
        initializer: Some(initializer),
    });
    assert_eq!(
        unit.metadata(&fixture.definitions),
        Err(JvmCodegenError::UnknownFieldInitializerSymbol {
            field,
            initializer,
            symbol: stale_symbol,
        })
    );
}

#[test]
fn planned_field_reads_reject_stale_targets_and_forged_this_prefixes() {
    let mut fixture = EmissionFixture::new(false);
    let field_name = fixture.store.names.intern("current");
    let field = fixture.store.symbols.alloc(Symbol {
        name: Name::new(field_name, Namespace::Term),
        owner: Some(fixture.class),
        kind: SymbolKind::Field,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Private,
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
    let initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(5),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let this_type = fixture.store.types.alloc(Type::ThisType {
        class: fixture.class,
    });
    let receiver = fixture.ast.alloc(Tree {
        kind: TreeKind::This(dotty_core::ast::This { qual: None }),
        position: None,
        ty: this_type,
    });

    let mut other_store = SemanticStore::new();
    let other_type = other_store.types.alloc(Type::Constant(Constant::Int(0)));
    let stale = loop {
        let symbol = alloc_symbol(&mut other_store, SymbolKind::Value, None, other_type);
        if !fixture.store.symbols.contains(symbol) {
            break symbol;
        }
    };
    let stale_reference = fixture.store.types.alloc(Type::TermRef {
        prefix: this_type,
        target: TermRefTarget::Symbol(stale),
    });
    let stale_select = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name: Name::new(field_name, Namespace::Term),
            backquoted: false,
        }),
        position: None,
        ty: stale_reference,
    });

    let mut unit = fixture.unit();
    unit.classes[0].fields.push(JvmFieldPlan {
        field,
        source_visibility: crate::JvmFieldSourceVisibility::PlainPrivate,
        initializer: Some(initializer),
    });
    unit.classes[0].methods[0].body = Some(stale_select);
    assert!(matches!(
        unit.lower_method_body(fixture.class, fixture.methods[0], &fixture.definitions),
        Err(JvmCodegenError::UnknownJvmFieldReference {
            tree,
            field,
        }) if tree == stale_select && field == stale
    ));
    drop(unit);

    let forged_prefix = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.no_prefix,
        target: TermRefTarget::Symbol(field),
    });
    let forged_select = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name: Name::new(field_name, Namespace::Term),
            backquoted: false,
        }),
        position: None,
        ty: forged_prefix,
    });
    let mut unit = fixture.unit();
    unit.classes[0].fields.push(JvmFieldPlan {
        field,
        source_visibility: crate::JvmFieldSourceVisibility::PlainPrivate,
        initializer: Some(initializer),
    });
    unit.classes[0].methods[0].body = Some(forged_select);
    assert!(matches!(
        unit.lower_method_body(fixture.class, fixture.methods[0], &fixture.definitions),
        Err(JvmCodegenError::InvalidJvmFieldReferencePrefix {
            tree,
            field: actual_field,
            class,
        }) if tree == forged_select && actual_field == field && class == fixture.class
    ));
}

#[test]
fn same_named_planned_local_keeps_its_local_load() {
    let mut fixture = EmissionFixture::new(false);
    let name_id = fixture.store.names.intern("current");
    let field = fixture.store.symbols.alloc(Symbol {
        name: Name::new(name_id, Namespace::Term),
        owner: Some(fixture.class),
        kind: SymbolKind::Field,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Private,
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
        .enter(Name::new(name_id, Namespace::Term), field);
    let field_initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(5),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let int_ty = fixture.definitions.int;
    let local = add_emission_local_symbol(
        &mut fixture,
        0,
        "current",
        int_ty,
        false,
        SymbolFlags::EMPTY,
    );
    let local_initializer = fixture.ast.alloc(Tree {
        kind: TreeKind::Literal(Literal {
            value: Constant::Int(7),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    let declaration =
        add_emission_local_valdef(&mut fixture, 0, local, int_ty, Some(local_initializer));
    let reference = typed_emission_symbol_ident(&mut fixture, local);
    let body = typed_block(&mut fixture, vec![declaration], reference, int_ty);
    fixture.bodies[0] = body;

    let mut unit = fixture.unit();
    unit.classes[0].fields.push(JvmFieldPlan {
        field,
        source_visibility: crate::JvmFieldSourceVisibility::PlainPrivate,
        initializer: Some(field_initializer),
    });
    unit.classes[0].methods[0].locals.push(JvmLocalBinding {
        declaration,
        symbol: local,
        mutable: false,
    });
    let code = unit
        .lower_method_body(fixture.class, fixture.methods[0], &fixture.definitions)
        .unwrap();
    let code = format!("{code:?}");
    assert!(code.contains("ILoad"));
    assert!(!code.contains("GetField"));
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

struct PrimitiveIntrinsicFixture {
    store: SemanticStore,
    ast: TypedAst,
    definitions: Definitions,
}

impl PrimitiveIntrinsicFixture {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        Self {
            store,
            ast: TypedAst::new(),
            definitions,
        }
    }

    fn call(
        &mut self,
        method: SymbolId,
        result: dotty_core::TypeId,
    ) -> TreeId<dotty_core::ast::Typed> {
        let receiver_type = self.store.types.alloc(Type::Constant(Constant::Int(4)));
        let receiver = self.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(4),
            }),
            position: None,
            ty: receiver_type,
        });
        let name = self.store.symbols.get(method).name;
        let function_type = self.store.types.alloc(Type::TermRef {
            prefix: receiver_type,
            target: TermRefTarget::Symbol(method),
        });
        let function = self.ast.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier: receiver,
                name,
                backquoted: false,
            }),
            position: None,
            ty: function_type,
        });
        let argument_type = self.store.types.alloc(Type::Constant(Constant::Int(6)));
        let argument = self.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(6),
            }),
            position: None,
            ty: argument_type,
        });
        self.ast.alloc(Tree {
            kind: TreeKind::Apply(Apply {
                function,
                args: vec![argument],
                kind: ApplyKind::Regular,
            }),
            position: None,
            ty: result,
        })
    }

    fn set_function_prefix(
        &mut self,
        call: TreeId<dotty_core::ast::Typed>,
        prefix: dotty_core::TypeId,
    ) -> TreeId<dotty_core::ast::Typed> {
        let function = match &self.ast.get(call).kind {
            TreeKind::Apply(apply) => apply.function,
            _ => unreachable!(),
        };
        let method = match self.store.types.get(self.ast.get(function).ty) {
            Type::TermRef {
                target: TermRefTarget::Symbol(method),
                ..
            } => *method,
            _ => unreachable!(),
        };
        let function_type = self.store.types.alloc(Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(method),
        });
        self.ast.get_mut(function).ty = function_type;
        function
    }
}

#[test]
fn resolves_each_registered_int_primitive_by_symbol_identity() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let entries = [
        (
            symbols.plus,
            JvmIntIntrinsic::IntAdd,
            fixture.definitions.int,
        ),
        (
            symbols.minus,
            JvmIntIntrinsic::IntSubtract,
            fixture.definitions.int,
        ),
        (
            symbols.multiply,
            JvmIntIntrinsic::IntMultiply,
            fixture.definitions.int,
        ),
        (
            symbols.divide,
            JvmIntIntrinsic::IntDivide,
            fixture.definitions.int,
        ),
        (
            symbols.remainder,
            JvmIntIntrinsic::IntRemainder,
            fixture.definitions.int,
        ),
        (
            symbols.less_than,
            JvmIntIntrinsic::IntLessThan,
            fixture.definitions.boolean,
        ),
        (
            symbols.less_or_equal,
            JvmIntIntrinsic::IntLessOrEqual,
            fixture.definitions.boolean,
        ),
        (
            symbols.greater_than,
            JvmIntIntrinsic::IntGreaterThan,
            fixture.definitions.boolean,
        ),
        (
            symbols.greater_or_equal,
            JvmIntIntrinsic::IntGreaterOrEqual,
            fixture.definitions.boolean,
        ),
    ];
    for (method, expected, result) in entries {
        let call = fixture.call(method, result);
        assert_eq!(
            resolve_int_intrinsic(&fixture.store, &fixture.ast, call, &fixture.definitions),
            Ok(Some(expected))
        );
    }
    assert_eq!(
        fixture
            .definitions
            .int_primitive_operator_symbols(&fixture.store),
        Some(symbols)
    );
}

#[test]
fn same_named_noncanonical_int_method_is_not_an_intrinsic() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let name = fixture.store.names.intern("+");
    let signature = fixture.store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: TermName::new(name),
            ty: fixture.definitions.int,
            erased: false,
            varargs: false,
        }],
        result: fixture.definitions.int,
        kind: MethodKind::Plain,
    }));
    let method = fixture.store.symbols.alloc(Symbol {
        name: Name::new(name, Namespace::Term),
        owner: fixture
            .store
            .types
            .get(fixture.definitions.int)
            .reference_symbol(),
        kind: SymbolKind::Method,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(signature),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let call = fixture.call(method, fixture.definitions.int);
    assert_eq!(
        resolve_int_intrinsic(&fixture.store, &fixture.ast, call, &fixture.definitions),
        Ok(None)
    );

    let name = fixture.store.names.intern("<");
    let signature = fixture.store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: TermName::new(name),
            ty: fixture.definitions.int,
            erased: false,
            varargs: false,
        }],
        result: fixture.definitions.boolean,
        kind: MethodKind::Plain,
    }));
    let comparison = fixture.store.symbols.alloc(Symbol {
        name: Name::new(name, Namespace::Term),
        owner: fixture
            .store
            .types
            .get(fixture.definitions.int)
            .reference_symbol(),
        kind: SymbolKind::Method,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(signature),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let comparison_call = fixture.call(comparison, fixture.definitions.boolean);
    assert_eq!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            comparison_call,
            &fixture.definitions,
        ),
        Ok(None)
    );
}

#[test]
fn builtin_call_with_missing_registry_entry_returns_typed_error() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let symbols = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap();
    let method = symbols.plus;
    fixture.store.symbols.set_info(method, SymbolInfo::Missing);
    let call = fixture.call(method, fixture.definitions.int);
    assert_eq!(
        resolve_int_intrinsic(&fixture.store, &fixture.ast, call, &fixture.definitions),
        Err(JvmIntIntrinsicError::MissingBuiltinRegistry { tree: call, method })
    );
}

#[test]
fn canonical_intrinsic_rejects_wrong_result_type() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let method = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .plus;
    let call = fixture.call(method, fixture.definitions.boolean);
    assert!(matches!(
        resolve_int_intrinsic(&fixture.store, &fixture.ast, call, &fixture.definitions),
        Err(JvmIntIntrinsicError::WrongResultType { tree, .. }) if tree == call
    ));
}

#[test]
fn canonical_intrinsic_rejects_wrong_operand_type_and_using_application() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let method = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .plus;
    let wrong_operand_call = fixture.call(method, fixture.definitions.int);
    let argument = match &fixture.ast.get(wrong_operand_call).kind {
        TreeKind::Apply(apply) => apply.args[0],
        _ => unreachable!(),
    };
    fixture.ast.get_mut(argument).ty = fixture.definitions.boolean;
    assert_eq!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            wrong_operand_call,
            &fixture.definitions,
        ),
        Err(JvmIntIntrinsicError::WrongOperandType {
            tree: wrong_operand_call,
            operand: argument,
        })
    );

    let regular_call = fixture.call(method, fixture.definitions.int);
    let (function, args) = match &fixture.ast.get(regular_call).kind {
        TreeKind::Apply(apply) => (apply.function, apply.args.clone()),
        _ => unreachable!(),
    };
    let using_call = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args,
            kind: ApplyKind::Using,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    assert_eq!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            using_call,
            &fixture.definitions
        ),
        Err(JvmIntIntrinsicError::UnsupportedCallShape { tree: using_call })
    );
}

#[test]
fn canonical_intrinsic_reports_wrong_arity_and_owner() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let method = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .plus;
    let valid_call = fixture.call(method, fixture.definitions.int);
    let function = match &fixture.ast.get(valid_call).kind {
        TreeKind::Apply(apply) => apply.function,
        _ => unreachable!(),
    };
    let wrong_arity = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args: Vec::new(),
            kind: ApplyKind::Regular,
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    assert_eq!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            wrong_arity,
            &fixture.definitions
        ),
        Err(JvmIntIntrinsicError::WrongArity {
            tree: wrong_arity,
            expected: 1,
            found: 0,
        })
    );

    fixture.store.symbols.get_mut(method).owner = Some(method);
    assert!(matches!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            valid_call,
            &fixture.definitions
        ),
        Err(JvmIntIntrinsicError::WrongOwner { tree, method: found, .. })
            if tree == valid_call && found == method
    ));
}

#[test]
fn canonical_intrinsic_rejects_unsupported_or_receiver_inconsistent_prefixes() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let method = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .plus;
    let unsupported_call = fixture.call(method, fixture.definitions.int);
    let unsupported = fixture.definitions.no_prefix;
    let unsupported_function = fixture.set_function_prefix(unsupported_call, unsupported);
    assert_eq!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            unsupported_call,
            &fixture.definitions
        ),
        Err(JvmIntIntrinsicError::UnsupportedFunctionPrefix {
            tree: unsupported_function,
            prefix: unsupported,
        })
    );

    let mismatched_call = fixture.call(method, fixture.definitions.int);
    let mismatched_function =
        fixture.set_function_prefix(mismatched_call, fixture.definitions.boolean);
    let receiver_id = match &fixture.ast.get(mismatched_call).kind {
        TreeKind::Apply(apply) => match &fixture.ast.get(apply.function).kind {
            TreeKind::Select(select) => select.qualifier,
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let receiver = fixture.ast.get(receiver_id).ty;
    assert_eq!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            mismatched_call,
            &fixture.definitions
        ),
        Err(JvmIntIntrinsicError::FunctionPrefixMismatch {
            tree: mismatched_function,
            prefix: fixture.definitions.boolean,
            receiver,
        })
    );
}

#[test]
fn type_applied_int_call_shape_returns_a_typed_error() {
    let mut fixture = PrimitiveIntrinsicFixture::new();
    let method = fixture
        .definitions
        .int_primitive_operator_symbols(&fixture.store)
        .unwrap()
        .plus;
    let call = fixture.call(method, fixture.definitions.int);
    let function = match &fixture.ast.get(call).kind {
        TreeKind::Apply(apply) => apply.function,
        _ => unreachable!(),
    };
    let type_apply = fixture.ast.alloc(Tree {
        kind: TreeKind::TypeApply(TypeApply {
            function,
            args: Vec::new(),
        }),
        position: None,
        ty: fixture.definitions.int,
    });
    assert_eq!(
        resolve_int_intrinsic(
            &fixture.store,
            &fixture.ast,
            type_apply,
            &fixture.definitions
        ),
        Err(JvmIntIntrinsicError::UnsupportedCallShape { tree: type_apply })
    );
}
