use super::*;

const CHAIN: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/ChainedExports.scala");

fn qualified_name<'a>(typer: &'a SourceTyper<'_>, id: SymbolId) -> (&'a str, &'a str) {
    let entry = typer.store.symbols.get(id);
    let owner = typer.store.symbols.get(entry.owner.unwrap());
    (
        typer
            .store
            .names
            .resolve(owner.name.text())
            .trim_end_matches('$'),
        typer.store.names.resolve(entry.name.text()),
    )
}

fn stable_delegate(typer: &mut SourceTyper<'_>, method: SymbolId) -> SymbolId {
    let tree = typer.type_export_forwarder_definition(method).unwrap();
    assert_eq!(
        tree,
        typer.type_export_forwarder_definition(method).unwrap()
    );
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!("forwarding method")
    };
    assert!(def.type_params.is_empty() && def.value_param_clauses.is_empty());
    let rhs = def.rhs.unwrap();
    let TreeKind::Select(sel) = &typer.typed_ast().get(rhs).kind else {
        panic!("qualified delegation")
    };
    let target = reference(typer, rhs);
    assert_eq!(
        typer
            .module_class_of_object(reference(typer, sel.qualifier))
            .unwrap(),
        typer.store.symbols.owner(target).unwrap()
    );
    assert_eq!(
        typer.store.export_target(method),
        Some(typer.typed_ast().get(rhs).ty)
    );
    assert_eq!(
        typer.store.symbols.info(method),
        &SymbolInfo::Complete(typer.typed_ast().get(rhs).ty)
    );
    target
}

#[test]
fn renamed_reexport_delegates_through_the_intermediate_stable_method() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(CHAIN);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let tree = client_result(&mut typer, &parsed, "named").unwrap();
    let outer = reference(&typer, tree);
    assert_eq!(qualified_name(&typer, outer), ("Renamed", "selected"));
    let middle = stable_delegate(&mut typer, outer);
    let control = client_result(&mut typer, &parsed, "control").unwrap();
    assert_eq!(reference(&typer, control), middle);
    assert_eq!(qualified_name(&typer, middle), ("Middle", "first"));
    let original = stable_delegate(&mut typer, middle);
    assert_eq!(
        original,
        val_symbol(&parsed, typer.store(), &index, source, "ints").0
    );
}

const BAD: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/ChainedErrors.scala");

fn errors_unit() -> NamedTestUnit {
    parse_and_name(&format!(
        "{CHAIN}\n{}",
        BAD.strip_prefix("package chain\n").unwrap()
    ))
}

#[test]
fn given_search_uses_the_public_reexport_on_the_first_lookup() {
    for (method, name) in [("inside", "first"), ("mono", "first"), ("all", "flags")] {
        let (parsed, mut store, packages, defs, index, source) = parse_and_name(CHAIN);
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
        let call = client_result(&mut typer, &parsed, method).unwrap();
        let selected = reference(&typer, evidence(&typer, call));
        assert_eq!(qualified_name(&typer, selected), ("Public", name));
        let target = stable_delegate(&mut typer, selected);
        assert_eq!(qualified_name(&typer, target), ("Middle", name));
    }
}

#[test]
fn competing_import_reports_the_public_reexport_at_the_call() {
    let (parsed, mut store, packages, defs, index, source) = errors_unit();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let error = rejected_call(&mut typer, &parsed, "clash");
    let TyperError::AmbiguousContextualArgument {
        candidate_names, ..
    } = error
    else {
        panic!("{error:?}")
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
    assert_eq!(names, [("Public", "first"), ("Rival", "other")]);
}

#[test]
fn second_selector_filters_the_visible_name_type_and_contextual_flag() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(CHAIN);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (method, outer, middle, name, flag) in [
        ("bound", "Filtered", "Middle", "first", SymbolFlags::GIVEN),
        (
            "legacy",
            "OldPublic",
            "OldMiddle",
            "old",
            SymbolFlags::IMPLICIT,
        ),
        (
            "retained",
            "WithoutFirst",
            "Middle",
            "flags",
            SymbolFlags::GIVEN,
        ),
    ] {
        let call = client_result(&mut typer, &parsed, method).unwrap();
        let id = reference(&typer, evidence(&typer, call));
        assert_eq!(qualified_name(&typer, id), (outer, name));
        let to = stable_delegate(&mut typer, id);
        assert_eq!(qualified_name(&typer, to), (middle, name));
        for symbol in [id, to] {
            assert_eq!(
                typer.store.symbols.get(symbol).flags
                    & (SymbolFlags::GIVEN | SymbolFlags::IMPLICIT),
                flag
            );
        }
    }
}

#[test]
fn hidden_omitted_and_nonmatching_evidence_is_missing_at_the_original_call() {
    let (parsed, mut store, packages, defs, index, source) = errors_unit();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for method in ["excluded", "modernStar", "hidden", "omitted"] {
        assert!(
            matches!(
                rejected_call(&mut typer, &parsed, method),
                TyperError::MissingContextualArgument { .. }
            ),
            "{method}"
        );
    }
}

#[test]
fn conditional_reexport_applies_each_delegate_with_its_own_binders() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(CHAIN);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut typer, &parsed, "poly").unwrap();
    let TreeKind::Apply(arg) = &typer.typed_ast().get(evidence(&typer, call)).kind else {
        panic!("conditional witness")
    };
    assert_eq!(
        qualified_name(&typer, reference(&typer, arg.args[0])),
        ("Public", "first")
    );
    let TreeKind::TypeApply(tapp) = &typer.typed_ast().get(arg.function).kind else {
        panic!("type application")
    };
    assert_eq!(typer.typed_ast().get(tapp.args[0]).ty, defs.int);
    let public = reference(&typer, tapp.function);
    let middle = provider_delegate(&mut typer, public);
    let leaf = provider_delegate(&mut typer, middle);
    for (method, owner) in [(public, "Public"), (middle, "Middle"), (leaf, "Leaf")] {
        assert_eq!(qualified_name(&typer, method), (owner, "boxed"));
        let SymbolInfo::Complete(binder) = *typer.store.symbols.info(method) else {
            panic!()
        };
        let Type::Poly(poly) = typer.store.types.get(binder) else {
            panic!()
        };
        let Type::Method(clause) = typer.store.types.get(poly.result) else {
            panic!()
        };
        let Type::Applied { args, .. } = typer.store.types.get(clause.params[0].ty) else {
            panic!()
        };
        assert!(
            matches!(typer.store.types.get(args[0]),Type::ParamRef{binder:actual,index:0} if *actual==binder)
        );
    }
    assert_ne!(
        typer.store.symbols.info(public),
        typer.store.symbols.info(middle)
    );
    assert_ne!(
        typer.store.symbols.info(middle),
        typer.store.symbols.info(leaf)
    );
}

#[test]
fn conditional_reexport_requires_one_unambiguous_witness_in_the_client() {
    let (parsed, mut store, packages, defs, index, source) = errors_unit();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    assert!(matches!(
        rejected_call(&mut typer, &parsed, "missingDep"),
        TyperError::MissingContextualArgument { .. }
    ));
    let error = rejected_call(&mut typer, &parsed, "ambiguousDep");
    let TyperError::AmbiguousContextualArgument { candidates, .. } = error else {
        panic!("{error:?}")
    };
    let mut names: Vec<_> = candidates
        .iter()
        .map(|&sym| qualified_name(&typer, sym))
        .collect();
    names.sort();
    assert_eq!(names, [("ambiguousDep", "left"), ("ambiguousDep", "right")]);
}
