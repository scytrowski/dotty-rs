#[allow(dead_code)]
mod support;

use dotty_core::ast::*;
use dotty_core::*;
use dotty_tasty::tasty::*;
use dotty_tasty_pickler::*;
use support::Fixture;

fn emit(f: &Fixture) -> Result<PickledTasty, PickleError> {
    f.plan().validate()?.pickle()
}

fn literal(output: &PickledTasty) -> i32 {
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let method = index
        .get(output.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert!(method.clauses.is_empty());
    assert!(method.parameters.is_empty());
    assert!(method.tail.is_empty());
    let RawTree::NatAst {
        tag: IDENTTPT_TAG,
        value,
        ..
    } = method.return_type
    else {
        panic!()
    };
    assert_eq!(file.render_name(value).unwrap(), "Int");
    let Some(RawTree::Leaf(rhs)) = method.rhs else {
        panic!()
    };
    assert_eq!(rhs.tag, INTCONST_TAG);
    let TermValue::Int(value) = rhs.value else {
        panic!()
    };
    assert_eq!(file.encode_validated().unwrap(), output.bytes());
    value
}

#[test]
fn complete_file_preserves_canonical_ids_owner_chain_result_and_actual_root() {
    let f = Fixture::new();
    let before = f.snapshot();
    let output = emit(&f).unwrap();
    assert_eq!(literal(&output), 42);
    assert_eq!(output.body(f.methods[0].symbol), Some(f.body));
    assert_eq!(output.body(f.constructor.symbol), None);
    assert_eq!(output.source(), &f.plan().source);
    assert_eq!(output.intended_tasty_path(), "example/Answer.tasty");
    assert_eq!(
        output
            .declarations()
            .iter()
            .map(|d| (d.symbol, d.owner))
            .collect::<Vec<_>>(),
        [
            (f.class, f.package),
            (f.constructor.symbol, f.class),
            (f.methods[0].symbol, f.class),
        ]
    );
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    assert_eq!(
        (
            file.header().major_version,
            file.header().minor_version,
            file.header().experimental_version
        ),
        (28, 9, 0)
    );
    assert!(!file.header().tooling_version.contains("incomplete"));
    assert_ne!(file.header().uuid, [0; 16]);
    assert_eq!(file.source_file().unwrap(), Some("Answer.scala"));
    assert_eq!(file.ast_nodes_with_tag(INTCONST_TAG).unwrap().len(), 1);
    assert!(file.ast_nodes_with_tag(ELIDED_TAG).unwrap().is_empty());
    assert!(file.ast_nodes_with_tag(UNITCONST_TAG).unwrap().is_empty());
    assert_eq!(f.snapshot(), before);
}

#[test]
fn independent_sessions_and_repeated_projection_have_identical_bytes() {
    let f = Fixture::new();
    assert_eq!(emit(&f).unwrap(), emit(&f).unwrap());
    assert_eq!(
        emit(&f).unwrap().bytes(),
        emit(&Fixture::new()).unwrap().bytes()
    );
}

fn set_value(f: &mut Fixture, value: i32) {
    f.source_ast.get_mut(f.rhs).kind = TreeKind::Literal(Literal {
        value: Constant::Int(value),
    });
    let ty = f.store.types.alloc(Type::Constant(Constant::Int(value)));
    f.typed_ast.get_mut(f.body).kind = TreeKind::Literal(Literal {
        value: Constant::Int(value),
    });
    f.typed_ast.get_mut(f.body).ty = ty;
}

#[test]
fn actual_values_and_signed_wire_boundaries_change_the_rhs_and_content_identity() {
    let mut f = Fixture::new();
    let base = emit(&f).unwrap();
    let base_uuid = TastyFile::parse(base.bytes()).unwrap().header().uuid;
    for value in [i32::MIN, -65, -1, 0, 7, 63, 64, 127, 128, i32::MAX] {
        set_value(&mut f, value);
        let output = emit(&f).unwrap();
        assert_eq!(literal(&output), value);
        assert_ne!(output.bytes(), base.bytes());
        assert_ne!(
            TastyFile::parse(output.bytes()).unwrap().header().uuid,
            base_uuid
        );
    }
}

#[test]
fn exact_whole_file_budget_and_wire_depth_and_bytes_are_enforced_without_mutation() {
    let f = Fixture::new();
    let before = f.snapshot();
    let output = emit(&f).unwrap();
    let plan = f.plan();
    let checked = plan.validate().unwrap();
    let exact = PickleLimits {
        max_file_bytes: output.bytes().len(),
        ..PickleLimits::default()
    };
    assert_eq!(checked.pickle_with_limits(exact).unwrap(), output);
    for size in [0, output.bytes().len() - 1] {
        assert_eq!(
            checked
                .pickle_with_limits(PickleLimits {
                    max_file_bytes: size,
                    ..exact
                })
                .unwrap_err(),
            PickleError::LimitExceeded {
                kind: LimitKind::FileBytes,
                limit: size
            }
        );
    }
    for wire in [
        AstBuildLimits {
            max_depth: 1,
            ..AstBuildLimits::default()
        },
        AstBuildLimits {
            max_bytes: 1,
            ..AstBuildLimits::default()
        },
    ] {
        assert!(matches!(
            checked.pickle_with_limits(PickleLimits {
                projection: ProjectionLimits {
                    wire,
                    ..ProjectionLimits::default()
                },
                ..exact
            }),
            Err(PickleError::Construction { .. })
        ));
    }
    assert_eq!(checked.pickle().unwrap(), output);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn widened_int_and_disagreeing_or_nonint_constants_fail_before_output() {
    let mut f = Fixture::new();
    f.typed_ast.get_mut(f.body).ty = f.definitions.int;
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::BodyType {
            tree: f.body,
            ty: f.definitions.int
        }
    );
    let ty = f.store.types.alloc(Type::Constant(Constant::Int(7)));
    f.typed_ast.get_mut(f.body).ty = ty;
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::BodyType { tree: f.body, ty }
    );
    f.typed_ast.get_mut(f.body).kind = TreeKind::Literal(Literal {
        value: Constant::Unit,
    });
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::BodyType { tree: f.body, ty }
    );
}

#[test]
fn unsupported_recursive_expressions_are_rejected_without_traversing_or_simplifying() {
    let mut f = Fixture::new();
    // Even a cyclic malformed block is rejected as a block, never followed.
    f.typed_ast.get_mut(f.body).kind = TreeKind::Block(Block {
        stats: vec![],
        expr: f.body,
    });
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::Unsupported {
            feature: UnsupportedFeature::BodyExpression,
            symbol: Some(f.methods[0].symbol),
        }
    );
}

#[test]
fn absent_stale_or_substituted_roots_do_not_produce_a_file() {
    let mut f = Fixture::new();
    f.methods[0].body = None;
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::MissingBody {
            method: f.methods[0].symbol
        }
    );
    let mark = f.typed_ast.checkpoint();
    let other = f.typed_ast.alloc(f.typed_ast.get(f.body).clone());
    f.methods[0].body = Some(other);
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::BodyBinding {
            source: f.source,
            tree: f.rhs,
            expected: other,
            actual: Some(f.body),
        }
    );
    f.typed_ast.rollback_to(mark);
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::InvalidTypedTree { tree: other }
    );
}

#[test]
fn bad_method_owner_and_result_type_are_specific_errors() {
    let mut f = Fixture::new();
    let method = f.methods[0].symbol;
    f.store.symbols.get_mut(method).owner = Some(f.package);
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::Owner {
            symbol: method,
            expected: Some(f.class),
            actual: Some(f.package),
        }
    );
    f.store.symbols.get_mut(method).owner = Some(f.class);
    f.store.symbols.get_mut(method).info = SymbolInfo::Complete(f.definitions.unit);
    assert_eq!(
        emit(&f).unwrap_err(),
        PickleError::UnsupportedTypeReference {
            ty: f.definitions.unit
        }
    );
}

#[test]
fn full_rhs_and_declared_result_match_the_unchanged_scala_oracle() {
    let f = Fixture::new();
    let output = emit(&f).unwrap();
    let generated = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let oracle = TastyFile::parse_and_validate_scala_3_9(include_bytes!(
        "../../../tests/fixtures/tasty_pickler/tasty/example/Answer.tasty"
    ))
    .unwrap();
    let generated_index = generated.ast_address_index().unwrap();
    let oracle_index = oracle.ast_address_index().unwrap();
    let actual = generated_index
        .get(output.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let expected = oracle_index
        .iter()
        .find_map(|tree| {
            let body = tree.decode_defdef_body().ok()?;
            (oracle.render_name(body.name).ok()?.as_str() == "value").then_some(body)
        })
        .unwrap();
    let (Some(RawTree::Leaf(actual_rhs)), Some(RawTree::Leaf(expected_rhs))) =
        (actual.rhs, expected.rhs)
    else {
        panic!()
    };
    assert_eq!(
        (actual_rhs.tag, actual_rhs.value),
        (expected_rhs.tag, expected_rhs.value)
    );
    let (
        RawTree::NatAst {
            tag: a_tag,
            value: a_name,
            ..
        },
        RawTree::NatAst {
            tag: e_tag,
            value: e_name,
            ..
        },
    ) = (actual.return_type, expected.return_type)
    else {
        panic!()
    };
    assert_eq!(a_tag, e_tag);
    assert_eq!(
        generated.render_name(a_name).unwrap(),
        oracle.render_name(e_name).unwrap()
    );
}

#[test]
fn stale_canonical_symbol_is_rejected_before_assembly() {
    let mut f = Fixture::new();
    let mark = f.store.checkpoint();
    let stale = f.store.symbols.alloc(f.store.symbols.get(f.class).clone());
    f.store.rollback_to(mark);
    let mut plan = f.plan();
    plan.class = stale;
    assert_eq!(
        plan.validate()
            .and_then(|checked| checked.pickle())
            .unwrap_err(),
        PickleError::InvalidSymbol { symbol: stale }
    );
}
