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
