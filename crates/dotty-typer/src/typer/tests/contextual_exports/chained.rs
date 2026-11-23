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

#[test]
fn rejected_chain_restores_scopes_and_can_be_followed_by_a_success() {
    let (parsed, mut store, packages, defs, index, source) = errors_unit();
    let scopes: Vec<_> = parsed
        .ast
        .iter()
        .filter_map(|(t, _)| index.export_site_at(source, t))
        .map(|s| index.scope_of(s.owner).unwrap())
        .collect();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let initial = typer.store.checkpoint();
    let before: Vec<_> = scopes
        .iter()
        .map(|&s| typer.store.scopes.get(s).entered_symbols().count())
        .collect();
    for _ in 0..2 {
        assert!(matches!(
            rejected_call(&mut typer, &parsed, "clash"),
            TyperError::AmbiguousContextualArgument { .. }
        ));
        assert_eq!(typer.store.checkpoint(), initial);
        assert_eq!(typer.typed_ast().iter().count(), 0);
        for (&scope, &expected) in scopes.iter().zip(&before) {
            assert_eq!(
                typer.store.scopes.get(scope).entered_symbols().count(),
                expected
            );
        }
    }
    let call = client_result(&mut typer, &parsed, "mono").unwrap();
    let public = reference(&typer, evidence(&typer, call));
    let middle = stable_delegate(&mut typer, public);
    let control = client_result(&mut typer, &parsed, "control").unwrap();
    assert_eq!(reference(&typer, control), middle);
}

#[test]
fn empty_given_cycle_terminates_without_publishing_evidence() {
    let text = include_str!("../../../../tests/fixtures/contextual-arguments/ChainedCycle.scala");
    for qualifier in ["LeftSide", "RightSide"] {
        let (parsed, mut store, packages, defs, index, source) =
            parse_and_name(&text.replace("import LeftSide", &format!("import {qualifier}")));
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
        let checkpoint = typer.store.checkpoint();
        for _ in 0..2 {
            assert!(matches!(
                rejected_call(&mut typer, &parsed, "use"),
                TyperError::MissingContextualArgument { .. }
            ));
            assert_eq!(typer.store.checkpoint(), checkpoint);
        }
    }
}

#[test]
fn declaration_order_and_precompleted_modules_preserve_the_same_delegation() {
    let forward = concat!(
        "object Front { export Mid.given }\n",
        "object Mid { export Leaf.given }\n"
    );
    let reverse = forward.lines().rev().collect::<Vec<_>>().join("\n");
    for objects in [forward, reverse.as_str()] {
        let text = format!(
            "package test\n{objects}\nobject Leaf {{ given leaf: Int = 1 }}\nclass Client {{ def read: Int = Front.leaf }}"
        );
        for warm in [false, true] {
            let (parsed, mut store, packages, defs, index, source) = parse_and_name(&text);
            let mut typer =
                SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
            if warm {
                let mid = parsed
                    .ast
                    .iter()
                    .filter_map(|(t, _)| index.export_site_at(source, t))
                    .map(|s| s.owner)
                    .find(|&owner| {
                        typer
                            .store
                            .names
                            .resolve(typer.store.symbols.get(owner).name.text())
                            .trim_end_matches('$')
                            == "Mid"
                    })
                    .unwrap();
                typer.complete_symbol(mid).unwrap();
            }
            let root = client_result(&mut typer, &parsed, "read").unwrap();
            let front = reference(&typer, root);
            let mid = stable_delegate(&mut typer, front);
            let leaf = stable_delegate(&mut typer, mid);
            assert_eq!(
                [front, mid, leaf].map(|s| qualified_name(&typer, s)),
                [("Front", "leaf"), ("Mid", "leaf"), ("Leaf", "leaf")]
            );
        }
    }
}

#[test]
fn deeply_nested_cold_exports_report_a_limit_and_restore_the_session() {
    let mut text = String::from("package chain\n");
    for i in 0..64 {
        text.push_str(&format!(
            "object Layer{i} {{ export Layer{}.given }}\n",
            i + 1
        ));
    }
    text.push_str("object Layer64 { given seed: Int = 1 }");
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&text);
    let first = parsed
        .ast
        .iter()
        .filter_map(|(t, _)| index.export_site_at(source, t))
        .map(|s| s.owner)
        .find(|&id| {
            store
                .names
                .resolve(store.symbols.get(id).name.text())
                .trim_end_matches('$')
                == "Layer0"
        })
        .unwrap();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let checkpoint = typer.store.checkpoint();
    for _ in 0..2 {
        assert!(matches!(
            typer.complete_symbol(first),
            Err(TyperError::UnsupportedExpression {
                expression_kind: "source export qualifier completion depth limit",
                ..
            })
        ));
        assert_eq!(typer.export_qualifier_depth, 0);
        assert_eq!(typer.store.checkpoint(), checkpoint);
    }
}
