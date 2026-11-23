use super::*;

const CLIENT: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/NamedUsing.scala");
const ERRORS: &str =
    include_str!("../../../../tests/fixtures/contextual-arguments/NamedUsingErrors.scala");

fn clients() -> NamedTestUnit {
    parse_and_name(&format!(
        "{CLIENT}\n{}",
        ERRORS.strip_prefix("package namedctx\n").unwrap()
    ))
}

fn args<'a>(t: &'a SourceTyper<'_>, tree: TreeId<Typed>) -> &'a [TreeId<Typed>] {
    let TreeKind::Apply(app) = &t.typed_ast().get(tree).kind else {
        panic!("expected application")
    };
    assert_eq!(app.kind, ApplyKind::Using);
    &app.args
}

#[test]
fn binds_written_names_and_rejects_invalid_names_at_the_argument() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for site in ["pos", "ordered"] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        let names: Vec<_> = args(&t, tree)
            .iter()
            .map(|&a| qualified_name(&t, reference(&t, a)))
            .collect();
        assert_eq!(names, [(site, "i"), (site, "b"), (site, "t")]);
    }
    client_result(&mut t, &parsed, "prefix").unwrap();
    for (site, index) in [("unknown", 0), ("duplicate", 1)] {
        let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, site);
        let TreeKind::Apply(call) = &parsed.ast.get(rhs).kind else {
            panic!()
        };
        let error = client_result(&mut t, &parsed, site).unwrap_err();
        match error {
            TyperError::UnknownNamedArgument { tree_index, .. } if site == "unknown" => {
                assert_eq!(tree_index, call.args[index].index())
            }
            TyperError::DuplicateNamedArgument { tree_index, .. } if site == "duplicate" => {
                assert_eq!(tree_index, call.args[index].index())
            }
            _ => panic!("{error:?}"),
        }
    }
}

#[test]
fn reordered_strict_arguments_run_once_in_written_order() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let tree = client_result(&mut t, &parsed, "reverse").unwrap();
    let names: Vec<_> = args(&t, tree)
        .iter()
        .map(|&a| qualified_name(&t, reference(&t, a)).1)
        .collect();
    assert_eq!(names, ["i", "b", "t"]);
    client_result(&mut t, &parsed, "effects").unwrap();
    let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, "effects");
    let tree = t.typed_index.get(source, rhs).unwrap();
    let TreeKind::Block(block) = &t.typed_ast().get(tree).kind else {
        panic!("missing lifted arguments")
    };
    let [flag, n] = block.stats[..] else {
        panic!("two strict operands")
    };
    for (stat, name) in [(flag, "tickBool"), (n, "tickInt")] {
        let TreeKind::ValDef(v) = &t.typed_ast().get(stat).kind else {
            panic!()
        };
        let TreeKind::Apply(a) = &t.typed_ast().get(v.rhs.unwrap()).kind else {
            panic!()
        };
        assert_eq!(qualified_name(&t, reference(&t, a.function)).1, name);
    }
    for (&arg, stat) in args(&t, block.expr).iter().zip([n, flag]) {
        assert_eq!(
            Some(reference(&t, arg)),
            t.typed_index.synthetic_definition_symbol(stat)
        );
    }
}

#[test]
fn reordering_keeps_prior_strict_clauses_first_and_by_name_operands_lazy() {
    let code = "class C {
        def next(): Int = 1
        def consume(first: Int)(using n: Int, delayed: => Int): Int = n
        def run(): Int = consume(next())(using delayed = next(), n = next())
    }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(code);
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    client_result(&mut t, &parsed, "run").unwrap();
    let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, "run");
    let TreeKind::Block(b) = &t
        .typed_ast()
        .get(t.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!()
    };
    let [first, n] = b.stats[..] else {
        panic!("only strict arguments are bound")
    };
    let TreeKind::Apply(call) = &t.typed_ast().get(b.expr).kind else {
        panic!()
    };
    let TreeKind::Apply(prior) = &t.typed_ast().get(call.function).kind else {
        panic!()
    };
    assert_eq!(
        Some(reference(&t, prior.args[0])),
        t.typed_index.synthetic_definition_symbol(first)
    );
    assert_eq!(
        Some(reference(&t, call.args[0])),
        t.typed_index.synthetic_definition_symbol(n)
    );
    assert!(matches!(
        &t.typed_ast().get(call.args[1]).kind,
        TreeKind::Apply(_)
    ));
}

#[test]
fn searches_only_unfilled_slots_and_reports_the_failed_goal() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (site, expected) in [
        ("hole", [Some("i"), None, Some("t")]),
        ("mixed", [None, Some("b"), Some("t")]),
    ] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        for (&arg, name) in args(&t, tree).iter().zip(expected) {
            if let Some(name) = name {
                assert_eq!(qualified_name(&t, reference(&t, arg)), (site, name))
            } else {
                assert!(matches!(t.typed_ast().get(arg).kind, TreeKind::Literal(_)))
            }
        }
    }
    for site in ["missing", "ambiguous", "misplaced"] {
        let checkpoint = t.store.checkpoint();
        let err = client_result(&mut t, &parsed, site).unwrap_err();
        match err {
            TyperError::MissingContextualArgument { .. } if site == "missing" => {}
            TyperError::AmbiguousContextualArgument {
                candidate_names, ..
            } if site == "ambiguous" => assert_eq!(candidate_names.len(), 2),
            TyperError::PositionalAfterNamedArgument { .. } if site == "misplaced" => {}
            _ => panic!("{site}: {err:?}"),
        }
        assert_eq!(t.store.checkpoint(), checkpoint);
    }
}

#[test]
fn default_getter_warns_only_when_it_shadows_unique_evidence() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for site in ["useDefault", "preferGiven", "overrideDefault"] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        let actual = qualified_name(&t, reference(&t, args(&t, tree)[0]));
        if site == "overrideDefault" {
            assert_eq!(actual, (site, "i"))
        } else {
            assert_eq!(actual, ("Client", "fallback$default$1"))
        }
        let warnings = t.take_diagnostics();
        if site == "preferGiven" {
            let [warning] = &warnings[..] else {
                panic!("one E220")
            };
            let TyperWarning::DefaultShadowsGiven { name } = warning.issue() else {
                panic!()
            };
            assert_eq!(t.store.names.resolve(name.as_name().text()), "n");
            let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, site);
            assert_eq!(
                warning.span(),
                parsed.ast.get(rhs).position.unwrap().span().range()
            );
        } else {
            assert!(warnings.is_empty())
        }
    }
}

#[test]
fn dependent_named_arguments_use_the_chosen_path_and_report_the_wrong_rhs() {
    let (parsed, mut store, packages, defs, index, source) = clients();
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for site in ["dep", "omitFamily"] {
        let tree = client_result(&mut t, &parsed, site).unwrap();
        let [family, witness] = args(&t, tree) else {
            panic!()
        };
        let family = reference(&t, *family);
        assert_eq!(qualified_name(&t, family), (site, "p"));
        assert_eq!(qualified_name(&t, reference(&t, *witness)), (site, "w"));
        let Type::TypeRef { prefix, .. } = t.store.types.get(t.typed_ast().get(tree).ty) else {
            panic!()
        };
        assert_eq!(t.store.types.get(*prefix).reference_symbol(), Some(family));
    }
    let (_, rhs) = method_definition_and_rhs(&parsed, t.store(), t.index, source, "wrongPath");
    let TreeKind::Apply(a) = &parsed.ast.get(rhs).kind else {
        panic!()
    };
    let TreeKind::NamedArg(value) = &parsed.ast.get(a.args[0]).kind else {
        panic!()
    };
    let checkpoint = t.store.checkpoint();
    for _ in 0..2 {
        let error = client_result(&mut t, &parsed, "wrongPath").unwrap_err();
        assert!(
            matches!(error,TyperError::ApplicationArgumentTypeMismatch{tree_index,argument_index:1,..} if tree_index==value.arg.index()),
            "{error:?}"
        );
        assert_eq!(t.store.checkpoint(), checkpoint);
    }
}

#[test]
fn lifting_a_field_preserves_the_original_singleton_path() {
    let code = "trait Family { type Out }
        class C(val p: Family) {
          def read(using f: Family, v: f.Out): f.Out = v
          def run(w: p.Out): p.Out = read(using v = w, f = p)
        }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(code);
    let mut t = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut t, &parsed, "run").unwrap();
    let result = t.typed_ast().get(call).ty;
    let Type::TypeRef { prefix, .. } = t.store.types.get(result) else {
        panic!()
    };
    let p = t.store.types.get(*prefix).reference_symbol().unwrap();
    assert_eq!(qualified_name(&t, p), ("C", "p"));
    let temp = reference(&t, args(&t, call)[0]);
    assert_eq!(t.store.symbols.info(temp), &SymbolInfo::Complete(*prefix));
}
