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
