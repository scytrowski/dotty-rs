mod support;

use dotty_core::*;
use dotty_tasty_pickler::*;
use support::Fixture;

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
