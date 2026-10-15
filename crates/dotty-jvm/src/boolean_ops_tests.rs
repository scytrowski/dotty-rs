use dotty_core::ast::{
    Apply, ApplyKind, Literal, Select, Tree, TreeKind, TypeApply, Typed, TypedAst,
};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{
    Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{Constant, MethodKind, MethodParam, MethodType, TermRefTarget, Type};

use super::{
    JvmBooleanIntrinsic, JvmBooleanIntrinsicError, JvmBooleanRhsEvaluation,
    resolve_boolean_intrinsic,
};

struct BooleanIntrinsicFixture {
    store: SemanticStore,
    ast: TypedAst,
    definitions: Definitions,
}

impl BooleanIntrinsicFixture {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        Self {
            store,
            ast: TypedAst::new(),
            definitions,
        }
    }

    fn boolean_operand(&mut self, value: bool) -> TreeId<Typed> {
        let ty = self
            .store
            .types
            .alloc(Type::Constant(Constant::Boolean(value)));
        self.ast.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Boolean(value),
            }),
            position: None,
            ty,
        })
    }

    fn boolean_singleton_operand(&mut self, name: &str) -> TreeId<Typed> {
        let symbol = self.store.symbols.alloc(Symbol {
            name: Name::new(self.store.names.intern(name), Namespace::Term),
            owner: None,
            kind: SymbolKind::Parameter,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(self.definitions.boolean),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let ty = self.store.types.alloc(Type::TermRef {
            prefix: self.definitions.no_prefix,
            target: TermRefTarget::Symbol(symbol),
        });
        self.ast.alloc(Tree {
            kind: TreeKind::Ident(dotty_core::ast::Ident {
                name: self.store.symbols.get(symbol).name,
                backquoted: false,
            }),
            position: None,
            ty,
        })
    }

    fn select(&mut self, method: SymbolId) -> TreeId<Typed> {
        let receiver = self.boolean_operand(true);
        let receiver_type = self.ast.get(receiver).ty;
        let name = self.store.symbols.get(method).name;
        let selection_type = self.store.types.alloc(Type::TermRef {
            prefix: receiver_type,
            target: TermRefTarget::Symbol(method),
        });
        self.ast.alloc(Tree {
            kind: TreeKind::Select(Select {
                qualifier: receiver,
                name,
                backquoted: false,
            }),
            position: None,
            ty: selection_type,
        })
    }

    fn expression(&mut self, method: SymbolId, result_type: dotty_core::TypeId) -> TreeId<Typed> {
        let function = self.select(method);
        let argument = self.boolean_operand(false);
        self.ast.alloc(Tree {
            kind: TreeKind::Apply(Apply {
                function,
                args: vec![argument],
                kind: ApplyKind::Regular,
            }),
            position: None,
            ty: result_type,
        })
    }

    fn symbols(&self) -> dotty_core::definitions::BooleanPrimitiveOperatorSymbols {
        self.definitions
            .boolean_primitive_operator_symbols(&self.store)
            .unwrap()
    }
}

#[test]
fn resolves_all_eight_canonical_boolean_operations_and_rhs_modes() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let symbols = fixture.symbols();
    let cases = [
        (
            symbols.unary_not,
            JvmBooleanIntrinsic::Not,
            JvmBooleanRhsEvaluation::None,
        ),
        (
            symbols.and_then,
            JvmBooleanIntrinsic::ShortCircuitAnd,
            JvmBooleanRhsEvaluation::ShortCircuit,
        ),
        (
            symbols.or_else,
            JvmBooleanIntrinsic::ShortCircuitOr,
            JvmBooleanRhsEvaluation::ShortCircuit,
        ),
        (
            symbols.eager_and,
            JvmBooleanIntrinsic::And,
            JvmBooleanRhsEvaluation::Strict,
        ),
        (
            symbols.eager_or,
            JvmBooleanIntrinsic::Or,
            JvmBooleanRhsEvaluation::Strict,
        ),
        (
            symbols.xor,
            JvmBooleanIntrinsic::Xor,
            JvmBooleanRhsEvaluation::Strict,
        ),
        (
            symbols.equal,
            JvmBooleanIntrinsic::Equal,
            JvmBooleanRhsEvaluation::Strict,
        ),
        (
            symbols.not_equal,
            JvmBooleanIntrinsic::NotEqual,
            JvmBooleanRhsEvaluation::Strict,
        ),
    ];

    for (method, intrinsic, evaluation) in cases {
        let expression = if intrinsic == JvmBooleanIntrinsic::Not {
            fixture.select(method)
        } else {
            fixture.expression(method, fixture.definitions.boolean)
        };
        let resolved = resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        );
        assert_eq!(resolved, Ok(Some(intrinsic)));
        assert_eq!(intrinsic.rhs_evaluation(), evaluation);
        assert_eq!(
            resolve_boolean_intrinsic(
                &fixture.store,
                &fixture.ast,
                expression,
                &fixture.definitions,
            ),
            Ok(Some(intrinsic))
        );
    }
}

#[test]
fn user_defined_operator_with_same_signature_is_not_an_intrinsic() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let name_id = fixture.store.names.intern("&&");
    let name = Name::new(name_id, Namespace::Term);
    let custom_class = fixture.store.symbols.alloc(Symbol {
        name: Name::new(fixture.store.names.intern("CustomBoolean"), Namespace::Type),
        owner: None,
        kind: SymbolKind::Class,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Missing,
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let signature = fixture.store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: dotty_core::names::TermName::new(fixture.store.names.intern("rhs")),
            ty: fixture.definitions.boolean,
            erased: false,
            varargs: false,
        }],
        result: fixture.definitions.boolean,
        kind: MethodKind::Plain,
    }));
    let method = fixture.store.symbols.alloc(Symbol {
        name,
        owner: Some(custom_class),
        kind: SymbolKind::Method,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(signature),
        origin: SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let receiver_type = fixture
        .store
        .types
        .alloc(Type::type_ref(fixture.definitions.no_prefix, custom_class));
    let receiver = fixture.ast.alloc(Tree {
        kind: TreeKind::Ident(dotty_core::ast::Ident {
            name: fixture.store.symbols.get(custom_class).name,
            backquoted: false,
        }),
        position: None,
        ty: receiver_type,
    });
    let selection_type = fixture.store.types.alloc(Type::TermRef {
        prefix: receiver_type,
        target: TermRefTarget::Symbol(method),
    });
    let function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name,
            backquoted: false,
        }),
        position: None,
        ty: selection_type,
    });
    let argument = fixture.boolean_singleton_operand("rhs");
    let expression = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args: vec![argument],
            kind: ApplyKind::Regular,
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });

    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Ok(None)
    );
}

#[test]
fn universal_any_equality_is_not_a_boolean_intrinsic() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let name = Name::new(fixture.store.names.intern("=="), Namespace::Term);
    let signature = fixture.store.types.alloc(Type::Method(MethodType {
        params: vec![MethodParam {
            name: dotty_core::names::TermName::new(fixture.store.names.intern("that")),
            ty: fixture.definitions.any_type,
            erased: false,
            varargs: false,
        }],
        result: fixture.definitions.boolean,
        kind: MethodKind::Plain,
    }));
    let method = fixture.store.symbols.alloc(Symbol {
        name,
        owner: Some(fixture.definitions.any_class),
        kind: SymbolKind::Method,
        flags: SymbolFlags::EMPTY,
        visibility: Visibility::Public,
        info: SymbolInfo::Complete(signature),
        origin: SymbolOrigin::Builtin,
        annotations: Vec::new(),
        position: None,
        links: SymbolLinks::default(),
    });
    let expression = fixture.expression(method, fixture.definitions.boolean);

    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Ok(None)
    );
}

#[test]
fn rejects_malformed_canonical_owner_kind_signature_and_registry() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let method = fixture.symbols().and_then;
    let expression = fixture.expression(method, fixture.definitions.boolean);
    let boolean_class = fixture
        .store
        .types
        .get(fixture.definitions.boolean)
        .reference_symbol()
        .unwrap();

    fixture.store.symbols.get_mut(method).owner = None;
    assert!(matches!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::WrongOwner { method: found, .. }) if found == method
    ));
    fixture.store.symbols.get_mut(method).owner = Some(boolean_class);

    fixture.store.symbols.get_mut(method).kind = SymbolKind::Field;
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::WrongMethodKind {
            tree: expression,
            method,
        })
    );
    fixture.store.symbols.get_mut(method).kind = SymbolKind::Method;

    fixture
        .store
        .symbols
        .set_info(method, SymbolInfo::Complete(fixture.definitions.int));
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::WrongSignature {
            tree: expression,
            method,
        })
    );

    fixture.store.symbols.set_info(
        method,
        SymbolInfo::Complete(fixture.store.types.alloc(Type::Method(MethodType {
            params: vec![MethodParam {
                name: dotty_core::names::TermName::new(fixture.store.names.intern("rhs")),
                ty: fixture.definitions.boolean,
                erased: false,
                varargs: false,
            }],
            result: fixture.definitions.boolean,
            kind: MethodKind::Plain,
        }))),
    );
    fixture.store.symbols.get_mut(method).flags = SymbolFlags::STATIC;
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::IncompatibleMethodFlags {
            tree: expression,
            method,
            flags: SymbolFlags::STATIC,
        })
    );
}

#[test]
fn rejects_incompatible_flags_on_parameterless_unary_member() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let method = fixture.symbols().unary_not;
    let expression = fixture.select(method);
    fixture.store.symbols.get_mut(method).flags = SymbolFlags::EXTENSION;

    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::IncompatibleMethodFlags {
            tree: expression,
            method,
            flags: SymbolFlags::EXTENSION,
        })
    );
}

#[test]
fn reports_a_missing_live_registry_entry_for_a_cached_canonical_identity() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let method = fixture.symbols().and_then;
    let expression = fixture.expression(method, fixture.definitions.boolean);
    let boolean_class = fixture
        .store
        .types
        .get(fixture.definitions.boolean)
        .reference_symbol()
        .unwrap();
    let SymbolInfo::Complete(class_info) = fixture.store.symbols.get(boolean_class).info else {
        unreachable!();
    };
    let Type::ClassInfo(class_info) = fixture.store.types.get(class_info) else {
        unreachable!();
    };
    fixture
        .store
        .scopes
        .get_mut(class_info.declarations)
        .remove(method);

    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::MissingBuiltinRegistry {
            tree: expression,
            method,
        })
    );
}

#[test]
fn rejects_a_replacement_live_registry_entry_for_a_cached_canonical_identity() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let method = fixture.symbols().and_then;
    let expression = fixture.expression(method, fixture.definitions.boolean);
    let boolean_class = fixture
        .store
        .types
        .get(fixture.definitions.boolean)
        .reference_symbol()
        .unwrap();
    let SymbolInfo::Complete(class_info) = fixture.store.symbols.get(boolean_class).info else {
        unreachable!();
    };
    let Type::ClassInfo(class_info) = fixture.store.types.get(class_info) else {
        unreachable!();
    };
    let declarations = class_info.declarations;
    let replacement_name = fixture.store.symbols.get(method).name;
    let replacement_symbol = fixture.store.symbols.get(method).clone();
    fixture.store.scopes.get_mut(declarations).remove(method);
    let replacement = fixture.store.symbols.alloc(replacement_symbol);
    fixture
        .store
        .scopes
        .get_mut(declarations)
        .enter(replacement_name, replacement);

    let live_registry = fixture
        .definitions
        .boolean_primitive_operator_symbols(&fixture.store)
        .unwrap();
    assert_eq!(live_registry.and_then, replacement);
    assert_ne!(replacement, method);
    assert_eq!(
        fixture
            .definitions
            .boolean_primitive_operator_ids()
            .unwrap()
            .and_then,
        method
    );

    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::MissingBuiltinRegistry {
            tree: expression,
            method,
        })
    );
}

#[test]
fn accepts_boolean_singleton_receiver_and_argument_types() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let method = fixture.symbols().xor;
    let receiver = fixture.boolean_singleton_operand("lhs");
    let receiver_type = fixture.ast.get(receiver).ty;
    let function_type = fixture.store.types.alloc(Type::TermRef {
        prefix: receiver_type,
        target: TermRefTarget::Symbol(method),
    });
    let function = fixture.ast.alloc(Tree {
        kind: TreeKind::Select(Select {
            qualifier: receiver,
            name: fixture.store.symbols.get(method).name,
            backquoted: false,
        }),
        position: None,
        ty: function_type,
    });
    let argument = fixture.boolean_singleton_operand("rhs");
    let expression = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args: vec![argument],
            kind: ApplyKind::Regular,
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });

    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Ok(Some(JvmBooleanIntrinsic::Xor))
    );
}

#[test]
fn malformed_operands_result_arity_and_type_application_are_typed_errors() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let method = fixture.symbols().and_then;
    let expression = fixture.expression(method, fixture.definitions.int);
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::WrongResultType {
            tree: expression,
            expected: fixture.definitions.boolean,
            found: fixture.definitions.int,
        })
    );

    let expression = fixture.expression(method, fixture.definitions.boolean);
    let function = match &fixture.ast.get(expression).kind {
        TreeKind::Apply(apply) => apply.function,
        _ => unreachable!(),
    };
    let wrong_prefix = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.int,
        target: TermRefTarget::Symbol(method),
    });
    fixture.ast.get_mut(function).ty = wrong_prefix;
    assert!(matches!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::FunctionPrefixMismatch {
            tree,
            prefix,
            ..
        }) if tree == function && prefix == fixture.definitions.int
    ));

    let expression = fixture.expression(method, fixture.definitions.boolean);
    let argument = match &fixture.ast.get(expression).kind {
        TreeKind::Apply(apply) => apply.args[0],
        _ => unreachable!(),
    };
    fixture.ast.get_mut(argument).ty = fixture.definitions.int;
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            expression,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::WrongOperandType {
            tree: expression,
            operand: argument,
        })
    );

    let expression = fixture.expression(method, fixture.definitions.boolean);
    let (function, args) = match &fixture.ast.get(expression).kind {
        TreeKind::Apply(apply) => (apply.function, apply.args.clone()),
        _ => unreachable!(),
    };
    let using_application = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args,
            kind: ApplyKind::Using,
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            using_application,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::UnsupportedTreeShape {
            tree: using_application,
        })
    );

    let expression = fixture.expression(method, fixture.definitions.boolean);
    let function = match &fixture.ast.get(expression).kind {
        TreeKind::Apply(apply) => apply.function,
        _ => unreachable!(),
    };
    let type_application = fixture.ast.alloc(Tree {
        kind: TreeKind::TypeApply(TypeApply {
            function,
            args: Vec::new(),
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            type_application,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::UnsupportedTreeShape {
            tree: type_application,
        })
    );

    let checkpoint = fixture.ast.checkpoint();
    let stale_id = fixture.ast.alloc(fixture.ast.get(expression).clone());
    fixture.ast.rollback_to(checkpoint);
    assert!(matches!(
        resolve_boolean_intrinsic(&fixture.store, &fixture.ast, stale_id, &fixture.definitions,),
        Err(JvmBooleanIntrinsicError::MissingTree { .. })
    ));
}

#[test]
fn wrong_arity_and_unresolved_targets_do_not_guess_from_operator_text() {
    let mut fixture = BooleanIntrinsicFixture::new();
    let symbols = fixture.symbols();
    let unary = fixture.select(symbols.unary_not);
    let function = fixture.select(symbols.and_then);
    let wrong_arity = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function,
            args: Vec::new(),
            kind: ApplyKind::Regular,
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            wrong_arity,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::WrongArity {
            tree: wrong_arity,
            expected: 1,
            found: 0,
        })
    );

    let wrong_unary_shape = fixture.ast.alloc(Tree {
        kind: TreeKind::Apply(Apply {
            function: unary,
            args: vec![],
            kind: ApplyKind::Regular,
        }),
        position: None,
        ty: fixture.definitions.boolean,
    });
    assert!(matches!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            wrong_unary_shape,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::WrongArity { expected: 0, .. })
    ));

    let unresolved = fixture.expression(symbols.and_then, fixture.definitions.boolean);
    let function = match &fixture.ast.get(unresolved).kind {
        TreeKind::Apply(apply) => apply.function,
        _ => unreachable!(),
    };
    let unresolved_type = fixture.store.types.alloc(Type::TermRef {
        prefix: fixture.definitions.boolean,
        target: TermRefTarget::Name(dotty_core::names::TermName::new(
            fixture.store.names.intern("&&"),
        )),
    });
    fixture.ast.get_mut(function).ty = unresolved_type;
    assert_eq!(
        resolve_boolean_intrinsic(
            &fixture.store,
            &fixture.ast,
            unresolved,
            &fixture.definitions,
        ),
        Err(JvmBooleanIntrinsicError::UnresolvedFunctionTarget { tree: function })
    );
}
