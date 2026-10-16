mod support;

use dotty_core::ast::{Ident, Literal, Modifier, Modifiers, ValDef};
use dotty_core::*;
use dotty_tasty_pickler::*;
use support::Fixture;

fn add_parameters(f: &mut Fixture) -> [SymbolId; 2] {
    let method = f.methods[0].symbol;
    let tree = f.methods[0].source_tree;
    let TreeKind::DefDef(definition) = &f.source_ast.get(tree).kind else {
        panic!()
    };
    let tpt = definition.tpt;
    let mut symbols = Vec::new();
    let mut trees = Vec::new();
    for text in ["first", "second"] {
        let name = TermName::new(f.store.names.intern(text));
        let symbol = support::source_symbol(
            &mut f.store,
            *name.as_name(),
            SymbolKind::Parameter,
            method,
            f.source,
            SymbolInfo::Complete(f.definitions.int),
        );
        let parameter = support::node(
            &mut f.source_ast,
            TreeKind::ValDef(ValDef {
                name,
                tpt,
                rhs: None,
                metadata: Modifiers::default(),
            }),
        );
        f.index.record_symbol(f.source, parameter, symbol).unwrap();
        symbols.push(symbol);
        trees.push(parameter);
    }
    let TreeKind::DefDef(definition) = &mut f.source_ast.get_mut(tree).kind else {
        panic!()
    };
    definition.value_param_clauses = vec![trees];
    f.methods[0].parameter_clauses = vec![symbols.clone()];
    [symbols[0], symbols[1]]
}

#[test]
fn minimal_plan_is_borrowed_read_only_and_uses_canonical_package() {
    let f = Fixture::new();
    let before = f.snapshot();
    let mark = f.store.checkpoint();
    let plan = f.plan();
    let checked = plan.validate().unwrap();
    assert!(std::ptr::eq(checked.plan().store, &f.store));
    assert!(std::ptr::eq(checked.plan().typed_ast, &f.typed_ast));
    assert_eq!(checked.package(), f.package);
    assert_eq!(checked.plan().source_index.scope_of(f.class), Some(f.scope));
    let TreeKind::TypeDef(class) = &f.source_ast.get(f.class_tree).kind else {
        panic!()
    };
    assert_eq!(class.rhs, f.template);
    assert_eq!(checked.plan().methods[0].body, Some(f.body));
    assert_eq!(f.store.checkpoint(), mark);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn incomplete_class_and_wrong_member_owner_have_distinct_typed_errors() {
    let mut f = Fixture::new();
    f.store.symbols.set_info(f.class, SymbolInfo::Missing);
    let before = f.snapshot();
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::IncompleteSymbol {
            symbol: f.class,
            info: SymbolInfo::Missing
        })
    );
    assert_eq!(f.snapshot(), before);
    f.store
        .symbols
        .set_info(f.class, SymbolInfo::Complete(f.class_info));
    let method = f.methods[0].symbol;
    f.store.symbols.get_mut(method).owner = Some(f.package);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::Owner {
            symbol: method,
            expected: Some(f.class),
            actual: Some(f.package)
        })
    );
}

#[test]
fn concrete_rhs_requires_its_exact_source_to_typed_binding() {
    let mut f = Fixture::new();
    f.bindings.0.clear();
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::BodyBinding {
            source: f.source,
            tree: f.rhs,
            expected: f.body,
            actual: None
        })
    );
    f.bindings.0.insert((f.source, f.rhs), f.body);
    f.methods[0].body = None;
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::MissingBody {
            method: f.methods[0].symbol
        })
    );
}

#[test]
fn source_paths_are_normalized_and_owner_cycles_are_bounded() {
    let mut f = Fixture::new();
    for name in [
        "",
        "/Answer.scala",
        "../Answer.scala",
        "a/./Answer.scala",
        "a//Answer.scala",
        "C:\\Answer.scala",
    ] {
        let mut plan = f.plan();
        plan.source.name = name.into();
        assert_eq!(
            plan.validate().err(),
            Some(PickleError::InvalidSourceName),
            "{name}"
        );
    }
    f.store.symbols.get_mut(f.package_root).owner = Some(f.package);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::OwnerCycle { symbol: f.package })
    );
}

#[test]
fn budgets_and_unsupported_metadata_do_not_mutate_input() {
    let mut f = Fixture::new();
    let limits = ValidationLimits {
        max_items: 0,
        ..ValidationLimits::default()
    };
    assert_eq!(
        f.plan().validate_with_limits(limits).err(),
        Some(PickleError::LimitExceeded {
            kind: LimitKind::Items,
            limit: 0
        })
    );
    f.store.symbols.get_mut(f.class).flags = SymbolFlags::FINAL;
    let before = f.snapshot();
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::Unsupported {
            feature: UnsupportedFeature::SymbolMetadata,
            symbol: Some(f.class)
        })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn constructor_semantic_result_must_be_class_instance_instead_of_wire_unit() {
    let mut f = Fixture::new();
    let SymbolInfo::Complete(info) = f.store.symbols.get(f.constructor.symbol).info else {
        panic!()
    };
    let Type::Method(signature) = f.store.types.get_mut(info) else {
        panic!()
    };
    signature.result = f.definitions.unit;
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::MalformedType {
            ty: f.definitions.unit,
            role: TypeRole::ConstructorResult
        })
    );
}

#[test]
fn stale_symbol_type_and_source_tree_ids_are_reported_without_panics() {
    let mut f = Fixture::new();
    let mark = f.store.checkpoint();
    let stale_symbol = f.store.symbols.alloc(f.store.symbols.get(f.class).clone());
    let stale_type = f.store.types.alloc(Type::NoPrefix);
    f.store.rollback_to(mark);
    let mut plan = f.plan();
    plan.class = stale_symbol;
    assert_eq!(
        plan.validate().err(),
        Some(PickleError::InvalidSymbol {
            symbol: stale_symbol
        })
    );
    f.store
        .symbols
        .set_info(f.methods[0].symbol, SymbolInfo::Complete(stale_type));
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::InvalidType { ty: stale_type })
    );
    f.store
        .symbols
        .set_info(f.methods[0].symbol, SymbolInfo::Complete(f.definitions.int));
    let checkpoint = f.source_ast.checkpoint();
    let stale_tree = support::node(
        &mut f.source_ast,
        TreeKind::TypeTree(dotty_core::ast::TypeTree),
    );
    f.source_ast.rollback_to(checkpoint);
    let TreeKind::TypeDef(definition) = &mut f.source_ast.get_mut(f.class_tree).kind else {
        panic!()
    };
    definition.rhs = stale_tree;
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::InvalidSourceTree { tree: stale_tree })
    );
}

#[test]
fn unfilled_and_error_types_remain_incomplete_instead_of_becoming_guessed_ints() {
    let mut f = Fixture::new();
    let unfilled = f.store.types.reserve().id();
    f.store
        .symbols
        .set_info(f.methods[0].symbol, SymbolInfo::Complete(unfilled));
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::IncompleteType { ty: unfilled })
    );
    f.store
        .symbols
        .set_info(f.methods[0].symbol, SymbolInfo::Error);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::IncompleteSymbol {
            symbol: f.methods[0].symbol,
            info: SymbolInfo::Error
        })
    );
}

#[test]
fn stale_typed_root_and_a_different_live_equal_literal_are_distinct_failures() {
    let mut f = Fixture::new();
    let checkpoint = f.typed_ast.checkpoint();
    let duplicate = f.typed_ast.alloc(f.typed_ast.get(f.body).clone());
    f.methods[0].body = Some(duplicate);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::BodyBinding {
            source: f.source,
            tree: f.rhs,
            expected: duplicate,
            actual: Some(f.body)
        })
    );
    f.typed_ast.rollback_to(checkpoint);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::InvalidTypedTree { tree: duplicate })
    );
}

#[test]
fn equal_spelling_cannot_replace_the_canonical_source_method_identity() {
    let mut f = Fixture::new();
    let canonical = f.methods[0].symbol;
    let replacement = f
        .store
        .symbols
        .alloc(f.store.symbols.get(canonical).clone());
    let name = f.store.symbols.get(canonical).name;
    f.store.scopes.get_mut(f.scope).remove(canonical);
    f.store.scopes.get_mut(f.scope).enter(name, replacement);
    f.methods[0].symbol = replacement;
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::SourceBinding {
            source: f.source,
            tree: f.methods[0].source_tree,
            expected: replacement,
            actual: Some(canonical)
        })
    );
}

#[test]
fn kind_origin_and_span_errors_preserve_exact_identity_context() {
    let mut f = Fixture::new();
    f.store.symbols.get_mut(f.class).kind = SymbolKind::Trait;
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::Kind {
            symbol: f.class,
            expected: SymbolKind::Class,
            actual: SymbolKind::Trait
        })
    );
    f.store.symbols.get_mut(f.class).kind = SymbolKind::Class;
    let foreign_source = SourceId::from_index(999);
    f.store.symbols.get_mut(f.class).origin = SymbolOrigin::Source(foreign_source);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::SourceOrigin {
            symbol: f.class,
            expected: f.source,
            actual: SymbolOrigin::Source(foreign_source)
        })
    );
    f.store.symbols.get_mut(f.class).origin = SymbolOrigin::Source(f.source);
    f.typed_ast.get_mut(f.body).position = Some(SourceSpan::new(
        foreign_source,
        Span::without_point(TextRange::new(0, 2).unwrap()),
    ));
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::SourceSpan {
            expected: f.source,
            actual: foreign_source
        })
    );
}

#[test]
fn invalid_names_and_forged_builtin_tables_do_not_fall_back_to_spelling() {
    let mut f = Fixture::new();
    let mut names = NameInterner::new();
    let name = (0..1000)
        .map(|n| names.intern(&format!("foreign{n}")))
        .last()
        .unwrap();
    f.store.symbols.get_mut(f.class).name = Name::new(name, Namespace::Type);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::InvalidName { name })
    );
    let f = Fixture::new();
    let mut wrong = f.definitions;
    wrong.int = wrong.unit;
    let mut plan = f.plan();
    plan.definitions = &wrong;
    assert_eq!(
        plan.validate().err(),
        Some(PickleError::BuiltinIdentity {
            builtin: Builtin::Int,
            ty: wrong.unit
        })
    );
}

#[test]
fn duplicate_and_omitted_members_cannot_be_hidden_by_the_explicit_plan() {
    let f = Fixture::new();
    let duplicates = [f.methods[0].clone(), f.methods[0].clone()];
    let mut plan = f.plan();
    plan.methods = &duplicates;
    assert_eq!(
        plan.validate().err(),
        Some(PickleError::DuplicateDeclaration {
            symbol: f.methods[0].symbol
        })
    );
    plan.methods = &[];
    assert_eq!(
        plan.validate().err(),
        Some(PickleError::SourceOrder { class: f.class })
    );
}

#[test]
fn scope_membership_failures_do_not_depend_on_hashmap_iteration() {
    for duplicate in [false, true] {
        let mut f = Fixture::new();
        let method = f.methods[0].symbol;
        let name = f.store.symbols.get(method).name;
        if duplicate {
            f.store.scopes.get_mut(f.scope).enter(name, method);
        } else {
            f.store.scopes.get_mut(f.scope).remove(method);
        }
        assert_eq!(
            f.plan().validate().err(),
            Some(PickleError::ScopeMembers { owner: f.class })
        );
    }
}

#[test]
fn source_modifiers_missing_from_semantic_flags_are_still_observable() {
    let mut f = Fixture::new();
    let TreeKind::TypeDef(definition) = &mut f.source_ast.get_mut(f.class_tree).kind else {
        panic!()
    };
    definition.metadata.modifiers.push(Modifier::Open);
    assert_eq!(f.store.symbols.get(f.class).flags, SymbolFlags::EMPTY);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::Unsupported {
            feature: UnsupportedFeature::SourceMetadata,
            symbol: Some(f.class)
        })
    );
}

#[test]
fn parents_and_parameterless_signatures_are_not_silently_normalized() {
    let mut f = Fixture::new();
    let Type::ClassInfo(info) = f.store.types.get_mut(f.class_info) else {
        panic!()
    };
    info.parents = vec![f.definitions.int];
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::Unsupported {
            feature: UnsupportedFeature::Parents,
            symbol: Some(f.class)
        })
    );
    let Type::ClassInfo(info) = f.store.types.get_mut(f.class_info) else {
        panic!()
    };
    info.parents = vec![f.definitions.object_type];
    let wrapped = f.store.types.alloc(Type::Method(MethodType {
        params: vec![],
        result: f.definitions.int,
        kind: MethodKind::Plain,
    }));
    f.store
        .symbols
        .set_info(f.methods[0].symbol, SymbolInfo::Complete(wrapped));
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::MalformedType {
            ty: wrapped,
            role: TypeRole::MethodResult
        })
    );
}

#[test]
fn parameter_id_order_duplicates_and_ownership_are_validated_before_deferral() {
    let mut f = Fixture::new();
    let [first, second] = add_parameters(&mut f);
    let method = f.methods[0].symbol;
    f.store.symbols.get_mut(first).owner = Some(f.class);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::Owner {
            symbol: first,
            expected: Some(method),
            actual: Some(f.class)
        })
    );
    f.store.symbols.get_mut(first).owner = Some(method);
    f.methods[0].parameter_clauses[0] = vec![first, first];
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::DuplicateDeclaration { symbol: first })
    );
    f.methods[0].parameter_clauses[0] = vec![second, first];
    let TreeKind::DefDef(definition) = &f.source_ast.get(f.methods[0].source_tree).kind else {
        panic!()
    };
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::SourceBinding {
            source: f.source,
            tree: definition.value_param_clauses[0][0],
            expected: second,
            actual: Some(first)
        })
    );
}

#[test]
fn literal_value_and_constant_type_must_agree_and_other_nodes_are_unsupported() {
    let mut f = Fixture::new();
    let ty = f.typed_ast.get(f.body).ty;
    f.typed_ast.get_mut(f.body).kind = TreeKind::Literal(Literal {
        value: Constant::Int(43),
    });
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::BodyType { tree: f.body, ty })
    );
    let name = f.store.symbols.get(f.methods[0].symbol).name;
    f.typed_ast.get_mut(f.body).kind = TreeKind::Ident(Ident {
        name,
        backquoted: false,
    });
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            symbol: Some(f.methods[0].symbol)
        })
    );
}

#[test]
fn a_stale_declaration_scope_is_distinct_from_malformed_class_info() {
    let mut f = Fixture::new();
    let mark = f.store.checkpoint();
    let stale = f.store.scopes.alloc(Scope::new(Some(f.class)));
    f.store.rollback_to(mark);
    let package_scope = f.index.scope_of(f.package).unwrap();
    let mut index = SourceSemanticIndex::new();
    index
        .record_package_symbol(f.source, f.root, f.package)
        .unwrap();
    index.record_scope(f.package, package_scope).unwrap();
    index.record_scope(f.class, stale).unwrap();
    for (tree, symbol) in [
        (f.class_tree, f.class),
        (f.constructor.source_tree, f.constructor.symbol),
        (f.methods[0].source_tree, f.methods[0].symbol),
    ] {
        index.record_symbol(f.source, tree, symbol).unwrap();
    }
    f.index = index;
    let Type::ClassInfo(info) = f.store.types.get_mut(f.class_info) else {
        panic!()
    };
    info.declarations = stale;
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::InvalidScope { scope: stale })
    );
    let Type::ClassInfo(info) = f.store.types.get_mut(f.class_info) else {
        panic!()
    };
    info.class = f.package;
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::MalformedType {
            ty: f.class_info,
            role: TypeRole::ClassInfo
        })
    );
}

#[test]
fn source_member_order_is_checked_before_supporting_more_than_one_method() {
    let mut f = Fixture::new();
    let first = f.methods[0].clone();
    let name = TermName::new(f.store.names.intern("other"));
    let symbol = support::source_symbol(
        &mut f.store,
        *name.as_name(),
        SymbolKind::Method,
        f.class,
        f.source,
        SymbolInfo::Complete(f.definitions.int),
    );
    let mut node = f.source_ast.get(first.source_tree).clone();
    let TreeKind::DefDef(definition) = &mut node.kind else {
        panic!()
    };
    definition.name = name;
    let tree = f.source_ast.alloc(node);
    f.index.record_symbol(f.source, tree, symbol).unwrap();
    f.store
        .scopes
        .get_mut(f.scope)
        .enter(*name.as_name(), symbol);
    let TreeKind::Template(template) = &mut f.source_ast.get_mut(f.template).kind else {
        panic!()
    };
    template.body.push(tree);
    f.methods.insert(
        0,
        MethodPicklePlan {
            symbol,
            source_tree: tree,
            parameter_clauses: vec![],
            body: Some(f.body),
        },
    );
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::SourceOrder { class: f.class })
    );
}

#[test]
fn owner_depth_and_name_size_have_explicit_limits() {
    let f = Fixture::new();
    assert_eq!(
        f.plan()
            .validate_with_limits(ValidationLimits {
                max_owner_depth: 1,
                ..ValidationLimits::default()
            })
            .err(),
        Some(PickleError::LimitExceeded {
            kind: LimitKind::OwnerDepth,
            limit: 1
        })
    );
    assert_eq!(
        f.plan()
            .validate_with_limits(ValidationLimits {
                max_name_bytes: 1,
                ..ValidationLimits::default()
            })
            .err(),
        Some(PickleError::LimitExceeded {
            kind: LimitKind::NameBytes,
            limit: 1
        })
    );
}

#[test]
fn a_consistent_typed_literal_cannot_change_the_original_source_literal_value() {
    let mut f = Fixture::new();
    let ty = f.typed_ast.get(f.body).ty;
    *f.store.types.get_mut(ty) = Type::Constant(Constant::Int(43));
    f.typed_ast.get_mut(f.body).kind = TreeKind::Literal(Literal {
        value: Constant::Int(43),
    });
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::BodySourceValue {
            source_tree: f.rhs,
            typed_tree: f.body,
            expected: 42,
            actual: 43
        })
    );
}

#[test]
fn the_package_must_bind_the_root_class_name_unambiguously() {
    let mut f = Fixture::new();
    let duplicate = f.store.symbols.alloc(f.store.symbols.get(f.class).clone());
    let name = f.store.symbols.get(f.class).name;
    let scope = f.index.scope_of(f.package).unwrap();
    f.store.scopes.get_mut(scope).enter(name, duplicate);
    assert_eq!(
        f.plan().validate().err(),
        Some(PickleError::ScopeMembers { owner: f.package })
    );
}

#[test]
fn source_constructor_clause_collections_consume_the_validation_budget() {
    let mut f = Fixture::new();
    let TreeKind::DefDef(constructor) = &mut f.source_ast.get_mut(f.constructor.source_tree).kind
    else {
        panic!()
    };
    constructor.value_param_clauses = vec![vec![]; 32];
    let limits = ValidationLimits {
        max_items: 20,
        ..ValidationLimits::default()
    };
    assert_eq!(
        f.plan().validate_with_limits(limits).err(),
        Some(PickleError::LimitExceeded {
            kind: LimitKind::Items,
            limit: 20
        })
    );
}

#[test]
fn scope_budget_errors_are_independent_of_the_first_invalid_entry() {
    for bad_first in [true, false] {
        let mut f = Fixture::new();
        let method = f.methods[0].symbol;
        let constructor = f.constructor.symbol;
        let name = f.store.symbols.get(method).name;
        let scope = f.store.scopes.get_mut(f.scope);
        scope.remove(method);
        scope.remove(constructor);
        // One bucket fixes entry order independently of HashMap randomization.
        // Both states are malformed and exceed the same remaining item budget.
        let entries = if bad_first {
            [f.package, constructor, method]
        } else {
            [constructor, method, f.package]
        };
        for symbol in entries {
            scope.enter(name, symbol);
        }
        let limits = ValidationLimits {
            max_items: 8,
            ..ValidationLimits::default()
        };
        assert_eq!(
            f.plan().validate_with_limits(limits).err(),
            Some(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: 8
            }),
            "bad_first={bad_first}"
        );
    }
}
