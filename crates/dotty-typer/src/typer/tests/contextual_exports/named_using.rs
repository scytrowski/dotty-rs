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
