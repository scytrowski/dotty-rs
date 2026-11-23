use super::*;

const LIB: &str = include_str!("../../../../tests/fixtures/contextual-arguments/PolyNamed.scala");
const ERRORS: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/PolyNamedErrors.scala");

fn clients() -> NamedTestUnit {
    parse_and_name(&format!(
        "{LIB}\n{}",
        ERRORS.strip_prefix("package inference\n").unwrap()
    ))
}

fn call<'a>(t: &'a SourceTyper<'_>, tree: TreeId<Typed>) -> &'a dotty_core::ast::Apply<Typed> {
    let TreeKind::Apply(call) = &t.typed_ast().get(tree).kind else {
        panic!("expected using call")
    };
    assert_eq!(call.kind, ApplyKind::Using);
    call
}

fn result_arguments<'a>(t: &'a SourceTyper<'_>, tree: TreeId<Typed>) -> &'a [TypeId] {
    let Type::Applied { args, .. } = t.store.types.get(t.typed_ast().get(tree).ty) else {
        panic!("expected parameterized result")
    };
    args
}

#[test]
fn full_named_using_infers_from_values_and_reports_binding_errors() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for site in ["pos", "explicit", "ordered"] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        assert_eq!(result_arguments(&t, tree), [defs.int]);
        let names: Vec<_> = call(&t, tree)
            .args
            .iter()
            .map(|&a| qualified_name(&t, reference(&t, a)))
            .collect();
        assert_eq!(names, [(site, "b"), (site, "f")]);
    }
    for (site, bad) in [("unknown", 0), ("duplicate", 1)] {
        let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, site);
        let TreeKind::Apply(a) = &parsed.ast.get(rhs).kind else {
            panic!()
        };
        let err = client_result(&mut t, &parsed, site).unwrap_err();
        assert!(
            matches!(err,TyperError::UnknownNamedArgument{tree_index,..} if site=="unknown" && tree_index==a.args[bad].index())
                || matches!(err,TyperError::DuplicateNamedArgument{tree_index,..} if site=="duplicate" && tree_index==a.args[bad].index()),
            "{err:?}"
        );
    }
}

#[test]
fn reordered_named_values_constrain_their_own_formals_and_execute_in_written_order() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, params, types) in [
        ("reversed", ["b", "f"], vec![defs.int]),
        ("two", ["b", "c"], vec![defs.int, defs.boolean]),
    ] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        assert_eq!(result_arguments(&t, tree), types);
        let names: Vec<_> = call(&t, tree)
            .args
            .iter()
            .map(|&arg| qualified_name(&t, reference(&t, arg)).1)
            .collect();
        assert_eq!(names, params);
    }
    client_result(&mut t, &parsed, "effects").unwrap();
    let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, "effects");
    let TreeKind::Block(block) = &t
        .typed_ast()
        .get(t.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!("lifted effects")
    };
    let [boolean, integer] = block.stats[..] else {
        panic!("two strict initializers")
    };
    for (stat, name) in [(boolean, "boolean"), (integer, "integer")] {
        let TreeKind::ValDef(v) = &t.typed_ast().get(stat).kind else {
            panic!()
        };
        let TreeKind::Apply(a) = &t.typed_ast().get(v.rhs.unwrap()).kind else {
            panic!()
        };
        assert_eq!(qualified_name(&t, reference(&t, a.function)).1, name);
    }
    for (&arg, stat) in call(&t, block.expr).args.iter().zip([integer, boolean]) {
        assert_eq!(
            Some(reference(&t, arg)),
            t.typed_index.synthetic_definition_symbol(stat)
        );
    }
}

#[test]
fn inferred_type_drives_search_for_leading_middle_and_trailing_holes() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, value, fmt) in [
        ("tail", 0, 1),
        ("leading", 1, 0),
        ("gap", 0, 1),
        ("wide", 0, 1),
    ] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        assert_eq!(result_arguments(&t, tree), [defs.int]);
        let args = &call(&t, tree).args;
        assert_eq!(qualified_name(&t, reference(&t, args[value])), (site, "b"));
        assert_eq!(qualified_name(&t, reference(&t, args[fmt])), (site, "f"));
        if site == "gap" {
            assert!(matches!(
                t.typed_ast().get(args[2]).kind,
                TreeKind::Literal(_)
            ))
        }
    }
    for site in ["missing", "ambiguous"] {
        let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, site);
        let err = client_result(&mut t, &parsed, site).unwrap_err();
        match err {
            TyperError::MissingContextualArgument { tree_index, .. } if site == "missing" => {
                assert_eq!(tree_index, rhs.index())
            }
            TyperError::AmbiguousContextualArgument {
                tree_index,
                candidate_names,
                ..
            } if site == "ambiguous" => {
                assert_eq!(tree_index, rhs.index());
                let mut names: Vec<_> = candidate_names
                    .iter()
                    .map(|(_, n)| t.store.names.resolve(n.text()))
                    .collect();
                names.sort();
                assert_eq!(names, ["l", "r"]);
            }
            _ => panic!("{err:?}"),
        }
    }
}

#[test]
fn omitted_evidence_fixes_the_selected_method_type_argument() {
    let source = format!(
        "{LIB}\n  def fromSearch(using f: Fmt[Int], b: Box[Int]): Any = middle(using flag = true)"
    );
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&source);
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let tree = client_result(&mut t, &parsed, "fromSearch").unwrap();
    let app = call(&t, tree);
    assert_eq!(result_arguments(&t, tree), [defs.int]);
    assert_eq!(
        qualified_name(&t, reference(&t, app.args[0])),
        ("fromSearch", "b")
    );
    assert_eq!(
        qualified_name(&t, reference(&t, app.args[1])),
        ("fromSearch", "f")
    );
}

fn type_application<'a>(
    t: &'a SourceTyper<'_>,
    tree: TreeId<Typed>,
) -> &'a dotty_core::ast::TypeApply<Typed> {
    let TreeKind::TypeApply(a) = &t.typed_ast().get(tree).kind else {
        panic!("expected inferred type application")
    };
    a
}

#[test]
fn generic_default_replays_inferred_type_argument_and_only_warns_for_shadowed_evidence() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for site in ["defaulted", "shadow", "overrideDefault"] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        let a = call(&t, tree);
        let target = type_application(&t, a.function);
        assert_eq!(target.args.len(), 1);
        assert_eq!(t.typed_ast().get(target.args[0]).ty, defs.int);
        if site == "overrideDefault" {
            assert_eq!(qualified_name(&t, reference(&t, a.args[1])), (site, "f"));
        } else {
            let getter = type_application(&t, a.args[1]);
            assert_eq!(
                qualified_name(&t, reference(&t, getter.function)),
                ("Client", "fallback$default$2")
            );
            assert_eq!(getter.args, target.args);
        }
        let warnings = t.take_diagnostics();
        if site == "shadow" {
            let [warning] = &warnings[..] else {
                panic!("expected one warning")
            };
            let TyperWarning::DefaultShadowsGiven { name } = warning.issue() else {
                panic!()
            };
            assert_eq!(t.store.names.resolve(name.as_name().text()), "fmt");
        } else {
            assert!(warnings.is_empty());
        }
    }
}

#[test]
fn bounded_and_conflicting_constraints_restore_the_session_and_locate_the_conflict() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for site in ["clash", "badBound"] {
        let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, site);
        let checkpoint = t.store.checkpoint();
        for _ in 0..2 {
            let err = client_result(&mut t, &parsed, site).unwrap_err();
            match err {
                TyperError::ConflictingInferenceConstraints {
                    tree_index,
                    parameter_index: 0,
                    first,
                    second,
                    ..
                } if site == "clash" => {
                    let TreeKind::Apply(a) = &parsed.ast.get(rhs).kind else {
                        panic!()
                    };
                    let TreeKind::NamedArg(value) = &parsed.ast.get(a.args[1]).kind else {
                        panic!()
                    };
                    assert_eq!(tree_index, value.arg.index());
                    assert_eq!((first, second), (defs.int, defs.boolean));
                }
                TyperError::InferredTypeArgumentBoundViolation {
                    argument,
                    parameter_index: 0,
                    side: TypeArgumentBoundSide::Upper,
                    ..
                } if site == "badBound" => assert_eq!(argument, defs.int),
                _ => panic!("{site}: {err:?}"),
            }
            assert_eq!(t.store.checkpoint(), checkpoint);
            assert!(t.typed_index.get(source, rhs).is_none());
        }
    }
    let tree = client_result(&mut t, &parsed, "goodBound").unwrap();
    let arg = result_arguments(&t, tree)[0];
    let dog = t.store.types.get(arg).reference_symbol().unwrap();
    assert_eq!(
        t.store.names.resolve(t.store.symbols.get(dog).name.text()),
        "Dog"
    );
}

#[test]
fn exported_and_legacy_calls_infer_on_the_selected_declaration() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, owner, name, result) in [
        ("exported", "Api", "selected", "Tag"),
        ("legacy", "Client", "old", "Box"),
    ] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        let a = call(&t, tree);
        let applied = type_application(&t, a.function);
        assert_eq!(
            qualified_name(&t, reference(&t, applied.function)),
            (owner, name)
        );
        assert_eq!(applied.args.len(), 1);
        assert_eq!(t.typed_ast().get(applied.args[0]).ty, defs.int);
        let Type::Applied { tycon, args } = t.store.types.get(t.typed_ast().get(tree).ty) else {
            panic!()
        };
        let class = t.store.types.get(*tycon).reference_symbol().unwrap();
        assert_eq!(
            t.store
                .names
                .resolve(t.store.symbols.get(class).name.text()),
            result
        );
        assert_eq!(args, &[defs.int]);
        for (&arg, expected) in a.args.iter().zip(["b", "f"]) {
            assert_eq!(qualified_name(&t, reference(&t, arg)), (site, expected));
        }
    }
}
