use super::*;

const LIB: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/InheritedExports.scala");

fn stable_path(typer: &mut SourceTyper<'_>, symbol: SymbolId) -> (SymbolId, SymbolId) {
    let tree = typer.type_export_forwarder_definition(symbol).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!("exported method")
    };
    let rhs = def.rhs.unwrap();
    let TreeKind::Select(sel) = &typer.typed_ast().get(rhs).kind else {
        panic!("stable delegation")
    };
    assert_eq!(
        typer.store.symbols.info(symbol),
        &SymbolInfo::Complete(typer.typed_ast().get(rhs).ty)
    );
    (reference(typer, sel.qualifier), reference(typer, rhs))
}

#[test]
fn named_inherited_export_preserves_receiver_and_original_declaration() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LIB);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut typer, &parsed, "named").unwrap();
    let alias = reference(&typer, call);
    assert_eq!(qualified_name(&typer, alias), ("Named", "selected"));
    let (receiver, original) = stable_path(&mut typer, alias);
    assert_eq!(qualified_name(&typer, receiver).1, "Ints");
    assert_eq!(qualified_name(&typer, original), ("Base", "key"));
    let tree = client_result(&mut typer, &parsed, "direct").unwrap();
    assert_eq!(reference(&typer, tree), original);
    let call = client_result(&mut typer, &parsed, "directSearch").unwrap();
    assert_eq!(reference(&typer, evidence(&typer, call)), original);
}

const BAD: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/InheritedErrors.scala");

fn with_client(client: &str) -> NamedTestUnit {
    parse_and_name(&format!(
        "{LIB}\n{}",
        client.strip_prefix("package inheritexport\n").unwrap()
    ))
}

#[test]
fn an_exported_given_has_the_priority_of_its_public_owner() {
    let (parsed, mut store, packages, defs, index, source) = with_client(BAD);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (method, expected) in [("all", ("Api", "key")), ("rankedControl", ("High", "high"))] {
        let call = client_result(&mut typer, &parsed, method).unwrap();
        assert_eq!(
            qualified_name(&typer, reference(&typer, evidence(&typer, call))),
            expected
        );
    }
    for (method, expected) in [
        ("clash", [("Api", "key"), ("Rival", "other")]),
        ("ranked", [("Ranked", "high"), ("Ranked", "low")]),
    ] {
        let TyperError::AmbiguousContextualArgument {
            candidate_names, ..
        } = rejected_call(&mut typer, &parsed, method)
        else {
            panic!("expected ambiguity at {method}")
        };
        let mut names: Vec<_> = candidate_names
            .iter()
            .map(|(owner, name)| {
                (
                    typer
                        .store
                        .names
                        .resolve(owner.unwrap().text())
                        .trim_end_matches('$'),
                    typer.store.names.resolve(name.text()),
                )
            })
            .collect();
        names.sort();
        assert_eq!(names, expected);
    }
}

#[test]
fn bounded_exports_project_the_same_parent_declaration_on_each_receiver() {
    for order in [["typed", "bool"], ["bool", "typed"]] {
        let (parsed, mut store, packages, defs, index, source) = with_client(BAD);
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
        let original = val_symbol(&parsed, typer.store(), &index, source, "key").0;
        for method in order {
            let (exporter, module) = if method == "typed" {
                ("Only", "Ints")
            } else {
                ("BoolOnly", "Bools")
            };
            let call = client_result(&mut typer, &parsed, method).unwrap();
            let id = reference(&typer, evidence(&typer, call));
            assert_eq!(qualified_name(&typer, id), (exporter, "key"));
            let (receiver, member) = stable_path(&mut typer, id);
            assert_eq!(qualified_name(&typer, receiver).1, module);
            assert_eq!(member, original);
        }
        for method in ["wrong", "hidden"] {
            assert!(
                matches!(
                    rejected_call(&mut typer, &parsed, method),
                    TyperError::MissingContextualArgument { .. }
                ),
                "{method}"
            );
        }
    }
}

#[test]
fn a_bound_that_only_matches_another_instantiation_publishes_no_witness() {
    let input = LIB.replace("export Ints.given TC[Int]", "export Ints.given TC[Boolean]");
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&input);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    assert!(matches!(
        rejected_call(&mut typer, &parsed, "typed"),
        TyperError::MissingContextualArgument { .. }
    ));
}
