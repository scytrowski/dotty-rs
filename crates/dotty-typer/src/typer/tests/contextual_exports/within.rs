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

fn dependent_delegate(typer: &mut SourceTyper<'_>, method: SymbolId) -> SymbolId {
    let tree = typer.type_export_forwarder_definition(method).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(tree).kind else {
        panic!()
    };
    let [clause] = def.value_param_clauses.as_slice() else {
        panic!("one parameter clause")
    };
    let params: Vec<_> = clause
        .iter()
        .map(|&tree| {
            let sym = typer.typed_index.synthetic_definition_symbol(tree).unwrap();
            assert_eq!(typer.store.symbols.owner(sym), Some(method));
            sym
        })
        .collect();
    let SymbolInfo::Complete(ty) = *typer.store.symbols.info(params[1]) else {
        panic!()
    };
    let Type::TypeRef { prefix, .. } = typer.store.types.get(ty) else {
        panic!("forwarder f.Out")
    };
    assert_eq!(
        typer.store.types.get(*prefix).reference_symbol(),
        Some(params[0])
    );
    let TreeKind::Apply(body) = &typer.typed_ast().get(def.rhs.unwrap()).kind else {
        panic!()
    };
    assert_eq!(
        body.args
            .iter()
            .map(|&arg| reference(typer, arg))
            .collect::<Vec<_>>(),
        params
    );
    let callee = match &typer.typed_ast().get(body.function).kind {
        TreeKind::TypeApply(app) => {
            assert_eq!(app.args.len(), def.type_params.len());
            for (&arg, &definition) in app.args.iter().zip(&def.type_params) {
                assert_eq!(
                    Some(reference(typer, arg)),
                    typer.typed_index.synthetic_definition_symbol(definition)
                );
            }
            app.function
        }
        _ => {
            assert!(def.type_params.is_empty());
            body.function
        }
    };
    reference(typer, callee)
}

#[test]
fn exported_dependent_parameters_forward_their_own_value_and_type_symbols() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, owner, name) in [
        ("viaApi", "Api", "product"),
        ("named", "Api", "product"),
        ("generic", "Generic", "lifted"),
    ] {
        let tree = client_result(&mut typer, &parsed, site).unwrap();
        let call = if site == "viaApi" {
            evidence(&typer, tree)
        } else {
            tree
        };
        let args: &[&str] = if site == "generic" {
            &["p", "v", "tag"]
        } else {
            &["p", "v"]
        };
        assert_arguments(&typer, call, site, args);
        let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
            panic!()
        };
        let target = if site == "generic" {
            let TreeKind::TypeApply(ta) = &typer.typed_ast().get(app.function).kind else {
                panic!()
            };
            assert_eq!(typer.typed_ast().get(ta.args[0]).ty, defs.int);
            ta.function
        } else {
            app.function
        };
        let method = reference(&typer, target);
        assert_eq!(qualified_name(&typer, method), (owner, name));
        let leaf = dependent_delegate(&mut typer, method);
        assert_eq!(qualified_name(&typer, leaf), ("Makers", name));
    }
}

#[test]
fn reexported_and_legacy_witnesses_keep_their_immediate_delegate() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, owner, name, flags) in [
        ("viaAlias", "Again", "derived", SymbolFlags::GIVEN),
        ("oldExport", "Old", "old", SymbolFlags::IMPLICIT),
    ] {
        let root = client_result(&mut typer, &parsed, site).unwrap();
        let call = evidence(&typer, root);
        assert_arguments(&typer, call, site, &["p", "v"]);
        let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
            panic!()
        };
        let method = reference(&typer, app.function);
        assert_eq!(qualified_name(&typer, method), (owner, name));
        assert_eq!(
            typer.store.symbols.get(method).flags & (SymbolFlags::GIVEN | SymbolFlags::IMPLICIT),
            flags
        );
        let target = dependent_delegate(&mut typer, method);
        if site == "viaAlias" {
            assert_eq!(qualified_name(&typer, target), ("Api", "product"));
            let leaf = dependent_delegate(&mut typer, target);
            assert_eq!(qualified_name(&typer, leaf), ("Makers", "product"));
        } else {
            assert_eq!(qualified_name(&typer, target), ("Legacy", "old"));
        }
    }
}

#[test]
fn unsuccessful_dependent_export_search_can_be_retried_before_and_after_completion() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let scopes: Vec<_> = parsed
        .ast
        .iter()
        .filter_map(|(tree, _)| index.export_site_at(source, tree))
        .map(|site| index.scope_of(site.owner).unwrap())
        .collect();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for warm in [false, true] {
        if warm {
            let root = client_result(&mut typer, &parsed, "viaAlias").unwrap();
            assert_arguments(&typer, evidence(&typer, root), "viaAlias", &["p", "v"]);
        }
        let before = typer.store.checkpoint();
        let count = typer.typed_ast().iter().count();
        let members = |t: &SourceTyper<'_>| {
            scopes
                .iter()
                .map(|&scope| {
                    t.store
                        .scopes
                        .get(scope)
                        .entered_symbols()
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let scopes_before = members(&typer);
        for site in ["noFirst", "noSecond", "ambiguous"] {
            for _ in 0..2 {
                match rejected_call(&mut typer, &parsed, site) {
                    TyperError::MissingContextualArgument { .. } if site != "ambiguous" => {}
                    TyperError::AmbiguousContextualArgument {
                        candidate_names, ..
                    } if site == "ambiguous" => {
                        let mut names: Vec<_> = candidate_names
                            .iter()
                            .map(|(owner, n)| {
                                (
                                    typer.store.names.resolve(owner.unwrap().text()),
                                    typer.store.names.resolve(n.text()),
                                )
                            })
                            .collect();
                        names.sort();
                        assert_eq!(names, [(site, "a"), (site, "b")]);
                    }
                    error => panic!("{site}: {error:?}"),
                }
                assert_eq!(typer.store.checkpoint(), before);
                assert_eq!(members(&typer), scopes_before);
                assert_eq!(typer.typed_ast().iter().count(), count);
            }
        }
    }
}

#[test]
fn a_required_path_cannot_be_invented_for_an_unstable_argument() {
    let text = "trait Family { type Out }
class C {
  def get(f: Family): Family = f
  def read(using f: Family, v: f.Out): f.Out = v
  def bad(p: Family)(v: p.Out): p.Out = read(using get(p), v)
}";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let (_, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "bad");
    let before = typer.store.checkpoint();
    for _ in 0..2 {
        let error = client_result(&mut typer, &parsed, "bad").unwrap_err();
        assert!(
            matches!(error,TyperError::DependentMethodApplicationDeferred{tree_index,..} if tree_index==rhs.index()),
            "{error:?}"
        );
        assert_eq!(typer.store.checkpoint(), before);
    }
}
