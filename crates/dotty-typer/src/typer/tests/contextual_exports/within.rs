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
