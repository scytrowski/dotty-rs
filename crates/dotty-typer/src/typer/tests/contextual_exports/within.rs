use super::*;

const CLIENT: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/WithinClause.scala");
const ERRORS: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/WithinClauseErrors.scala");

fn clients() -> NamedTestUnit {
    parse_and_name(&format!(
        "{CLIENT}\n{}",
        ERRORS.strip_prefix("package within\n").unwrap()
    ))
}

#[test]
fn explicit_arguments_select_the_second_type_on_the_first_argument_path() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let split = client_result(&mut typer, &parsed, "split").unwrap();
    let TreeKind::Apply(outer) = &typer.typed_ast().get(split).kind else {
        panic!()
    };
    assert_eq!(
        qualified_name(&typer, reference(&typer, outer.args[0])),
        ("split", "v")
    );
    let call = client_result(&mut typer, &parsed, "explicit").unwrap();
    let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
        panic!()
    };
    assert_eq!(app.kind, ApplyKind::Using);
    let p = reference(&typer, app.args[0]);
    assert_eq!(qualified_name(&typer, p), ("explicit", "p"));
    assert_eq!(
        qualified_name(&typer, reference(&typer, app.args[1])),
        ("explicit", "v")
    );
    let Type::TypeRef { prefix, .. } = typer.store.types.get(typer.typed_ast().get(call).ty) else {
        panic!("p.Out")
    };
    assert_eq!(typer.store.types.get(*prefix).reference_symbol(), Some(p));
    let error = client_result(&mut typer, &parsed, "wrong").unwrap_err();
    assert!(
        matches!(
            error,
            TyperError::ApplicationArgumentTypeMismatch {
                argument_index: 1,
                ..
            }
        ),
        "{error:?}"
    );
}

fn assert_arguments(typer: &SourceTyper<'_>, tree: TreeId<Typed>, site: &str, names: &[&str]) {
    let TreeKind::Apply(call) = &typer.typed_ast().get(tree).kind else {
        panic!("argument clause at {site}")
    };
    let actual: Vec<_> = call
        .args
        .iter()
        .map(|&tree| qualified_name(typer, reference(typer, tree)))
        .collect();
    assert_eq!(
        actual,
        names.iter().map(|&name| (site, name)).collect::<Vec<_>>()
    );
}

#[test]
fn inferred_method_and_provider_clauses_use_the_selected_family_path() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, owner, method) in [
        ("inferred", "Client", "read"),
        ("original", "Makers", "product"),
        ("oldDirect", "Legacy", "old"),
    ] {
        let root = client_result(&mut typer, &parsed, site).unwrap();
        let tree = if site == "inferred" {
            root
        } else {
            evidence(&typer, root)
        };
        assert_arguments(&typer, tree, site, &["p", "v"]);
        let TreeKind::Apply(call) = &typer.typed_ast().get(tree).kind else {
            panic!()
        };
        assert_eq!(
            qualified_name(&typer, reference(&typer, call.function)),
            (owner, method)
        );
    }
    assert!(matches!(
        rejected_call(&mut typer, &parsed, "wrongSearch"),
        TyperError::MissingContextualArgument { .. }
    ));
}

#[test]
fn partial_using_keeps_the_supplied_path_and_searches_the_trailing_argument() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let tree = client_result(&mut typer, &parsed, "partial").unwrap();
    assert_arguments(&typer, tree, "partial", &["p", "v"]);
    let TreeKind::Apply(call) = &typer.typed_ast().get(tree).kind else {
        panic!()
    };
    assert_eq!(call.kind, ApplyKind::Using);
    let Type::TypeRef { prefix, .. } = typer.store.types.get(typer.typed_ast().get(tree).ty) else {
        panic!()
    };
    assert_eq!(
        typer.store.types.get(*prefix).reference_symbol(),
        Some(reference(&typer, call.args[0]))
    );
    for site in ["partialMissing", "partialAmbiguous"] {
        let (_, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, site);
        let checkpoint = typer.store.checkpoint();
        for _ in 0..2 {
            let error = client_result(&mut typer, &parsed, site).unwrap_err();
            match error {
                TyperError::MissingContextualArgument {
                    source: s,
                    tree_index,
                    ..
                } if site == "partialMissing" => assert_eq!((s, tree_index), (source, rhs.index())),
                TyperError::AmbiguousContextualArgument {
                    source: s,
                    tree_index,
                    candidate_names,
                    ..
                } if site == "partialAmbiguous" => {
                    assert_eq!((s, tree_index), (source, rhs.index()));
                    let mut names: Vec<_> = candidate_names
                        .iter()
                        .map(|(_, n)| typer.store.names.resolve(n.text()))
                        .collect();
                    names.sort();
                    assert_eq!(names, ["a", "b"]);
                }
                _ => panic!("{site}: {error:?}"),
            }
            assert_eq!(typer.store.checkpoint(), checkpoint);
        }
    }
}

#[test]
fn omitted_default_after_explicit_using_is_chosen_over_lexical_evidence() {
    let text = "class C {
        def choose(using a: Int, b: Boolean = false): Boolean = b
        def run(using other: Boolean): Boolean = choose(using 7)
    }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let tree = client_result(&mut typer, &parsed, "run").unwrap();
    let TreeKind::Apply(call) = &typer.typed_ast().get(tree).kind else {
        panic!()
    };
    assert_eq!(call.kind, ApplyKind::Using);
    assert_eq!(
        qualified_name(&typer, reference(&typer, call.args[1])),
        ("C", "choose$default$2")
    );
}
