use dotty::jvm_tasty_constructor_alias::{
    ConstructorReferenceAliasError, with_constructor_reference_alias_proof,
};
use dotty_core::SourceId;
use dotty_core::ast::TreeKind;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_constructor_injection/CounterOwner.scala");

#[test]
fn one_session_receipt_preserves_canonical_constructor_field_and_read_edges() {
    with_constructor_reference_alias_proof(
        SOURCE,
        "CounterOwner.scala",
        SourceId::from_index(1752),
        |proof| {
            assert_eq!(proof.source_id(), SourceId::from_index(1752));
            assert_ne!(proof.template_parameter(), proof.constructor_parameter());
            assert_eq!(proof.constructor_descriptor(), "(Lexample/Counter;)V");
            assert_eq!(proof.field_descriptor(), "Lexample/Counter;");
            assert_eq!(proof.read_target_descriptor(), "()I");
            let session = proof.session();
            let TreeKind::PackageDef(package) = &session.source_ast.get(session.source_root).kind
            else {
                panic!("receipt retains the original package root");
            };
            assert_eq!(package.stats.len(), 2);
            assert_eq!(
                session.store.symbols.get(proof.template_parameter()).owner,
                Some(proof.owner_class())
            );
            assert_eq!(
                session
                    .store
                    .symbols
                    .get(proof.constructor_parameter())
                    .owner,
                Some(proof.constructor())
            );
            let Some(dotty_core::SourceDefinition::Canonical { source, tree }) = session
                .source_index
                .definition_of(proof.template_parameter())
            else {
                panic!("template parameter has canonical source provenance");
            };
            assert_eq!(source, proof.source_id());
            assert_eq!(
                session
                    .source_index
                    .definition_of(proof.constructor_parameter()),
                Some(dotty_core::SourceDefinition::Derived { source, tree })
            );
            assert_eq!(
                session
                    .source_typed_index
                    .get(proof.source_id(), proof.source_initializer()),
                Some(proof.typed_initializer())
            );
            assert!(proof.source_initializer().index() > 0);
            assert!(proof.typed_initializer().index() > 0);
            assert!(proof.source_read().index() > 0);
            assert!(proof.typed_read().index() > 0);
            (
                proof.counter_class(),
                proof.owner_class(),
                proof.constructor(),
                proof.field(),
                proof.read_method(),
                proof.counter_get(),
            )
        },
    )
    .unwrap();
}

#[test]
fn rejects_wrong_declaration_order_foreign_counter_and_non_alias_rhs() {
    let wrong_order = SOURCE
        .replace("class Counter(start: Int):", "class Tmp(start: Int):")
        .replace(
            "class CounterOwner(counter: Counter):",
            "class CounterOwner(counter: Tmp):",
        );
    assert!(matches!(
        with_constructor_reference_alias_proof(
            &wrong_order,
            "Wrong.scala",
            SourceId::from_index(1753),
            |_| ()
        ),
        Err(ConstructorReferenceAliasError::SourceShape(_))
            | Err(ConstructorReferenceAliasError::TypeIdentity(_))
    ));

    let new_rhs = SOURCE.replace(
        "private val inner: Counter = counter",
        "private val inner: Counter = new Counter(0)",
    );
    assert!(matches!(
        with_constructor_reference_alias_proof(
            &new_rhs,
            "New.scala",
            SourceId::from_index(1754),
            |_| ()
        ),
        Err(ConstructorReferenceAliasError::TypedBinding(_))
    ));

    let mutable = SOURCE.replace("private val inner: Counter", "private var inner: Counter");
    assert!(matches!(
        with_constructor_reference_alias_proof(
            &mutable,
            "Mutable.scala",
            SourceId::from_index(1755),
            |_| ()
        ),
        Err(ConstructorReferenceAliasError::SourceShape(_))
            | Err(ConstructorReferenceAliasError::TypedBinding(_))
    ));

    let wrong_field_type = SOURCE.replace("private val inner: Counter", "private val inner: Int");
    assert!(
        with_constructor_reference_alias_proof(
            &wrong_field_type,
            "WrongField.scala",
            SourceId::from_index(1756),
            |_| ()
        )
        .is_err()
    );

    let default_parameter =
        SOURCE.replace("counter: Counter)", "counter: Counter = new Counter(0))");
    assert!(
        with_constructor_reference_alias_proof(
            &default_parameter,
            "Default.scala",
            SourceId::from_index(1757),
            |_| ()
        )
        .is_err()
    );

    let accessor_parameter = SOURCE.replace("counter: Counter)", "val counter: Counter)");
    assert!(
        with_constructor_reference_alias_proof(
            &accessor_parameter,
            "Accessor.scala",
            SourceId::from_index(1758),
            |_| ()
        )
        .is_err()
    );

    let contextual_parameter = SOURCE.replace("counter: Counter)", "using counter: Counter)");
    assert!(
        with_constructor_reference_alias_proof(
            &contextual_parameter,
            "Using.scala",
            SourceId::from_index(1759),
            |_| ()
        )
        .is_err()
    );

    let secondary_constructor = SOURCE.replace(
        "  def read(): Int = inner.get()",
        "  def this(value: Int) = this(new Counter(value))\n  def read(): Int = inner.get()",
    );
    assert!(
        with_constructor_reference_alias_proof(
            &secondary_constructor,
            "Secondary.scala",
            SourceId::from_index(1760),
            |_| ()
        )
        .is_err()
    );

    let wrong_get = SOURCE.replace("inner.get()", "inner.set(0)");
    assert!(
        with_constructor_reference_alias_proof(
            &wrong_get,
            "WrongGet.scala",
            SourceId::from_index(1761),
            |_| ()
        )
        .is_err()
    );

    let private_counter = SOURCE.replace(
        "class Counter(start: Int):",
        "private class Counter(start: Int):",
    );
    assert!(
        with_constructor_reference_alias_proof(
            &private_counter,
            "PrivateCounter.scala",
            SourceId::from_index(1762),
            |_| ()
        )
        .is_err()
    );

    let duplicate_inner = SOURCE.replace(
        "  private val inner: Counter = counter",
        "  private val inner: Counter = counter\n  private val inner: Counter = counter",
    );
    assert!(
        with_constructor_reference_alias_proof(
            &duplicate_inner,
            "DuplicateInner.scala",
            SourceId::from_index(1763),
            |_| ()
        )
        .is_err()
    );

    let extra_class = SOURCE.replace(
        "class CounterOwner(counter: Counter):",
        "class Other\n\nclass CounterOwner(counter: Counter):",
    );
    assert!(
        with_constructor_reference_alias_proof(
            &extra_class,
            "ExtraClass.scala",
            SourceId::from_index(1764),
            |_| ()
        )
        .is_err()
    );

    let foreign_package = SOURCE.replace(
        "class CounterOwner(counter: Counter):",
        "package foreign\nclass CounterOwner(counter: Counter):",
    );
    assert!(
        with_constructor_reference_alias_proof(
            &foreign_package,
            "Foreign.scala",
            SourceId::from_index(1765),
            |_| ()
        )
        .is_err()
    );
}

#[test]
fn rejects_nested_package_and_lazy_field_as_outside_the_proven_abi_shape() {
    let nested_package = SOURCE.replace("package example", "package outer.example");
    assert!(matches!(
        with_constructor_reference_alias_proof(
            &nested_package,
            "NestedPackage.scala",
            SourceId::from_index(1766),
            |_| ()
        ),
        Err(ConstructorReferenceAliasError::AbiShape(
            "canonical package/class names"
        ))
    ));

    let lazy_field = SOURCE.replace(
        "private val inner: Counter",
        "private lazy val inner: Counter",
    );
    assert!(matches!(
        with_constructor_reference_alias_proof(
            &lazy_field,
            "Lazy.scala",
            SourceId::from_index(1767),
            |_| ()
        ),
        Err(ConstructorReferenceAliasError::SourceShape(_))
    ));
}
