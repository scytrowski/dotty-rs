use super::super::*;

const EXPR_SOURCE: &str =
    include_str!("../../../tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala");
const NEGATIVE_CASES: &str =
    include_str!("../../../tests/fixtures/source-recursive-adt-match-3.9.0/negative-cases.tsv");
const RUST_NEGATIVE_MATRIX: &str = include_str!(
    "../../../tests/fixtures/source-recursive-adt-match-3.9.0/rust-negative-matrix.tsv"
);

struct NegativeCase {
    name: String,
    root: String,
    scala_result: String,
    mutations: Vec<(String, String)>,
}

fn negative_cases() -> Vec<NegativeCase> {
    let mut cases = Vec::<NegativeCase>::new();
    for (line_number, line) in NEGATIVE_CASES.lines().enumerate().skip(1) {
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(
            columns.len(),
            5,
            "malformed negative case row {}",
            line_number + 1
        );
        let (name, root, scala_result, old, new) =
            (columns[0], columns[1], columns[2], columns[3], columns[4]);
        assert!(matches!(
            scala_result,
            "accepted" | "accepted_with_warning" | "rejected"
        ));
        let unescape = |value: &str| value.replace("\\n", "\n");
        let mutation = (unescape(old), unescape(new));
        if let Some(case) = cases.last_mut().filter(|case| case.name == name) {
            assert_eq!(case.root, root);
            assert_eq!(case.scala_result, scala_result);
            case.mutations.push(mutation);
        } else {
            cases.push(NegativeCase {
                name: name.to_owned(),
                root: root.to_owned(),
                scala_result: scala_result.to_owned(),
                mutations: vec![mutation],
            });
        }
    }
    cases
}

fn mutated_source(case: &NegativeCase) -> String {
    let mut source = EXPR_SOURCE.to_owned();
    for (old, new) in &case.mutations {
        assert_eq!(source.matches(old).count(), 1, "{}: {old:?}", case.name);
        source = source.replacen(old, new, 1);
    }
    source
}

fn tree_span(parsed: &dotty_parser::ParseResult, index: u32) -> String {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (tree.index() == index).then(|| {
                let range = node.position.unwrap().span().range();
                format!("tree{index}@{}..{}", range.start(), range.end())
            })
        })
        .unwrap_or_else(|| format!("tree{index}@unknown"))
}

fn normalized_error(error: &TyperError, parsed: &dotty_parser::ParseResult) -> String {
    let debug = format!("{error:?}");
    let tree = debug
        .split("tree_index: ")
        .nth(1)
        .and_then(|rest| rest.split(|ch: char| !ch.is_ascii_digit()).next())
        .and_then(|value| value.parse::<u32>().ok())
        .map(|index| tree_span(parsed, index))
        .unwrap_or_else(|| "tree@unknown".to_owned());
    let category = debug.split('{').next().unwrap_or("unknown").trim();
    format!("deferred:{category}:{tree}")
}

fn exercise_negative_case(case: &NegativeCase, source_id: SourceId) -> String {
    let source_text = mutated_source(case);
    let mut receipts = Vec::new();
    for _ in 0..2 {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let (parsed, index) =
            parse_and_name_unit(&source_text, source_id, &mut store, &mut packages);
        let root = tree_named(&parsed, &store, &case.root);
        let mut typer = typer_for(
            &parsed,
            &mut store,
            &packages,
            definitions,
            &index,
            source_id,
        );
        let before = issue_1999_late_failure_projection(&typer, &parsed);
        let before_store = typer.store().checkpoint();
        let before_typed = typer.typed_ast().checkpoint();
        let first = match typer.type_method_definition(root) {
            Ok(_) => format!("accepted:{}", tree_span(&parsed, root.index())),
            Err(error) => {
                let receipt = normalized_error(&error, &parsed);
                assert_eq!(typer.store().checkpoint(), before_store, "{}", case.name);
                assert_eq!(
                    typer.typed_ast().checkpoint(),
                    before_typed,
                    "{}",
                    case.name
                );
                assert_issue_1999_projection_unchanged(
                    &before,
                    &issue_1999_late_failure_projection(&typer, &parsed),
                    &format!("{} failure restores semantic state", case.name),
                );
                let retry = typer
                    .type_method_definition(root)
                    .expect_err("repeated negative root remains rejected");
                assert_eq!(normalized_error(&retry, &parsed), receipt, "{}", case.name);
                assert_eq!(
                    typer.store().checkpoint(),
                    before_store,
                    "{} retry",
                    case.name
                );
                assert_eq!(
                    typer.typed_ast().checkpoint(),
                    before_typed,
                    "{} retry",
                    case.name
                );
                receipt
            }
        };
        receipts.push(first);
    }
    assert_eq!(
        receipts[0], receipts[1],
        "fresh-session receipt for {}",
        case.name
    );
    receipts.remove(0)
}

fn tree_named(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    name: &str,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == name =>
            {
                Some(tree)
            }
            _ => None,
        })
        .last()
        .unwrap_or_else(|| panic!("missing source method {name}"))
}

fn typer_for<'a>(
    parsed: &'a dotty_parser::ParseResult,
    store: &'a mut SemanticStore,
    packages: &'a Packages,
    definitions: Definitions,
    index: &'a SourceSemanticIndex,
    source: SourceId,
) -> SourceTyper<'a> {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let java_home = std::env::var_os("JAVA_HOME").expect("pinned JDK is configured");
    let mut classpaths: Vec<Box<dyn dotty_classloader::classloader::ClassPathEntry>> = vec![
        Box::new(dotty_classloader::classloader::DirectoryClassPath::new(
            fixture,
        )),
        Box::new(
            dotty_classloader::classloader::JdkClassPath::new(
                std::path::PathBuf::from(java_home).join("jmods"),
            )
            .expect("pinned JDK modules"),
        ),
    ];
    if let Some(maven_root) = std::env::var_os("COURSIER_MAVEN_ROOT") {
        classpaths.push(Box::new(
            dotty_classloader::classloader::DirectoryClassPath::new(maven_root),
        ));
    }
    let classpath = dotty_classloader::classloader::CompositeClassPath::new(classpaths);
    let resolver = dotty_classloader::classloader::ClasspathSymbolResolver::new(
        classpath,
        definitions,
        dotty_classloader::classloader::LoadingSession::new(),
    );
    SourceTyper::new(&parsed.ast, source, index, store, definitions, packages)
        .with_resolver(Box::new(resolver))
}

#[test]
fn recursive_eval_late_result_failure_observes_binder_call_and_rolls_back_for_retry() {
    let source_text = EXPR_SOURCE.replace(
        "object Evaluator:\n",
        concat!(
            "object Evaluator:\n",
            "  def prior(value: Int): Int = value\n\n",
            "  def broken(expr: Expr): Boolean =\n",
            "    expr match\n",
            "      case Num(_)      => 0\n",
            "      case Succ(inner) => eval(inner) + 1\n"
        ),
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&source_text);
    let prior = tree_named(&parsed, &store, "prior");
    let broken = tree_named(&parsed, &store, "broken");
    let unwrap = tree_named(&parsed, &store, "unwrap");
    let eval = tree_named(&parsed, &store, "eval");
    let eval_symbol = index
        .symbol_at(source, eval)
        .expect("canonical eval symbol");
    let (rhs, cases, binder_tree, recursive_call, recursive_argument) = {
        let TreeKind::DefDef(definition) = &parsed.ast.get(broken).kind else {
            unreachable!()
        };
        let rhs = definition.rhs.expect("broken body");
        let TreeKind::Match(matched) = &parsed.ast.get(rhs).kind else {
            panic!("broken body remains the fixture Match")
        };
        let TreeKind::CaseDef(second_case) = &parsed.ast.get(matched.cases[1]).kind else {
            unreachable!()
        };
        let TreeKind::Apply(extractor) = &parsed.ast.get(second_case.pattern).kind else {
            panic!("Succ is the canonical one-field extractor")
        };
        let binder_tree = extractor.args[0];
        let body = match &parsed.ast.get(second_case.body).kind {
            TreeKind::Block(block) => block.expr,
            _ => second_case.body,
        };
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(addition)) = &parsed.ast.get(body).kind
        else {
            panic!("late failure occurs in the final arithmetic operand")
        };
        let recursive_call = addition.left;
        let TreeKind::Apply(call) = &parsed.ast.get(recursive_call).kind else {
            panic!("left operand is the explicit recursive Apply")
        };
        (
            rhs,
            matched.cases.clone(),
            binder_tree,
            recursive_call,
            call.args[0],
        )
    };
    let method_span = parsed.ast.get(broken).position.unwrap().span().range();
    let binder_span = parsed.ast.get(binder_tree).position.unwrap().span().range();
    let recursive_span = parsed
        .ast
        .get(recursive_call)
        .position
        .unwrap()
        .span()
        .range();
    let mut typer = typer_for(&parsed, &mut store, &packages, definitions, &index, source);
    let prior_typed = typer
        .type_method_definition(prior)
        .expect("unrelated successful root precedes the failure");
    let before_projection = issue_1999_late_failure_projection(&typer, &parsed);
    let before_resolver = typer.resolver.checkpoint();
    let before_store = typer.store().checkpoint();
    let before_typed = typer.typed_ast().checkpoint();
    let before_root_mappings = typer.source_typed_index().len();
    let observation = Rc::new(RefCell::new(None));
    let observed = Rc::clone(&observation);
    let observed_cases = cases.clone();
    let parsed_ref = &parsed;
    let index_ref = &index;
    typer.transaction_failure_observer = Some(Rc::new(move |typer| {
        let Some(typed_call) = typer.source_typed_index().get(source, recursive_call) else {
            return;
        };
        let Some(typed_match) = typer.source_typed_index().get(source, rhs) else {
            return;
        };
        if observed_cases
            .iter()
            .any(|case| typer.source_typed_index().get(source, *case).is_none())
        {
            return;
        }
        let binder = typer
            .pattern_binding_symbol_at(source, binder_tree)
            .expect("authentic Succ.inner binder exists before rollback");
        assert_eq!(typer.store().symbols.get(binder).kind, SymbolKind::Local);
        let broken_symbol = index_ref.symbol_at(source, broken).unwrap();
        assert_eq!(typer.store().symbols.get(binder).owner, Some(broken_symbol));
        let SymbolInfo::Complete(binder_type) = *typer.store().symbols.info(binder) else {
            panic!("Succ.inner type is complete before rollback")
        };
        let expr = class_symbol(parsed_ref, typer.store(), index_ref, source, "Expr");
        assert_eq!(type_symbol(typer.store(), binder_type), expr);
        let TreeKind::Apply(typed_application) = &typer.typed_ast().get(typed_call).kind else {
            panic!("the canonical recursive call remains an ordinary Apply")
        };
        let typed_function = typed_application.function;
        let function_symbol = match typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("recursive function has a canonical symbol reference: {other:?}"),
        };
        assert_eq!(function_symbol, eval_symbol);
        assert_eq!(typed_application.args.len(), 1);
        assert_eq!(
            typer.source_typed_index().get(source, recursive_argument),
            Some(typed_application.args[0])
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_application.args[0]).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == binder
        ));
        let TreeKind::Match(typed_match_node) = &typer.typed_ast().get(typed_match).kind else {
            panic!("typed root retains its Match before rollback")
        };
        assert_eq!(typed_match_node.cases.len(), 2);
        assert_eq!(
            observed_cases
                .iter()
                .filter(|case| typer.source_typed_index().get(source, **case).is_some())
                .count(),
            2
        );
        *observed.borrow_mut() = Some((binder, function_symbol, typed_call, typed_match));
    }));

    let first = typer
        .type_method_definition(broken)
        .expect_err("declared Boolean result fails after both Int-valued cases type");
    assert!(
        matches!(first, TyperError::ExpectedExpressionTypeMismatch { .. }),
        "{first:?}"
    );
    let first_debug = format!("{first:?}");
    let observed_values = (*observation.borrow())
        .expect("failure observer saw the authentic case binder, recursive Apply, Match and cases");
    assert_eq!(observed_values.1, eval_symbol);
    assert_eq!(typer.store().checkpoint(), before_store);
    assert_eq!(typer.resolver.checkpoint(), before_resolver);
    assert_eq!(typer.typed_ast().checkpoint(), before_typed);
    assert_eq!(typer.source_typed_index().len(), before_root_mappings);
    assert_issue_1999_projection_unchanged(
        &before_projection,
        &issue_1999_late_failure_projection(&typer, &parsed),
        "failure restores the exact semantic and typer-owned projection",
    );
    assert_eq!(
        typer.source_typed_index().get(source, prior),
        Some(prior_typed)
    );
    assert!(typer.source_typed_index().get(source, broken).is_none());
    assert!(typer.source_typed_index().get(source, rhs).is_none());
    assert!(
        typer
            .source_typed_index()
            .get(source, recursive_call)
            .is_none()
    );
    for case in &cases {
        assert!(typer.source_typed_index().get(source, *case).is_none());
    }
    for (tree, _) in parsed.ast.iter() {
        assert!(typer.pattern_binding_symbol_at(source, tree).is_none());
    }

    let retry = typer
        .type_method_definition(broken)
        .expect_err("the failed whole root remains deterministically invalid");
    assert_eq!(format!("{retry:?}"), first_debug);
    assert_eq!(typer.store().checkpoint(), before_store);
    assert_eq!(typer.resolver.checkpoint(), before_resolver);
    assert_eq!(typer.typed_ast().checkpoint(), before_typed);
    assert_issue_1999_projection_unchanged(
        &before_projection,
        &issue_1999_late_failure_projection(&typer, &parsed),
        "retry preserves the pre-failure projection",
    );

    let typed_unwrap = typer
        .type_method_definition(unwrap)
        .expect("the original unwrap types first after failure in this session");
    let typed_eval = typer
        .type_method_definition(eval)
        .expect("the original eval types after unwrap in the same failed session");
    assert_eq!(
        typer.source_typed_index().get(source, unwrap),
        Some(typed_unwrap)
    );
    assert_eq!(
        typer.source_typed_index().get(source, eval),
        Some(typed_eval)
    );

    let (
        fresh_parsed,
        mut fresh_store,
        fresh_packages,
        fresh_definitions,
        fresh_index,
        fresh_source,
    ) = parse_and_name(&source_text);
    let fresh_broken = tree_named(&fresh_parsed, &fresh_store, "broken");
    let mut fresh_typer = typer_for(
        &fresh_parsed,
        &mut fresh_store,
        &fresh_packages,
        fresh_definitions,
        &fresh_index,
        fresh_source,
    );
    let fresh_error = fresh_typer
        .type_method_definition(fresh_broken)
        .expect_err("independent source session rejects the same root");
    assert_eq!(format!("{fresh_error:?}"), first_debug);
    assert_eq!(
        format!(
            "broken={}..{}; binder={}..{}; recursive_apply={}..{}; error={first_debug}",
            method_span.start(),
            method_span.end(),
            binder_span.start(),
            binder_span.end(),
            recursive_span.start(),
            recursive_span.end()
        ),
        format!(
            "broken={}..{}; binder={}..{}; recursive_apply={}..{}; error={fresh_error:?}",
            method_span.start(),
            method_span.end(),
            binder_span.start(),
            binder_span.end(),
            recursive_span.start(),
            recursive_span.end()
        ),
        "fresh-session diagnostic and source spans normalize identically",
    );
}

#[test]
fn recursive_adt_negative_matrix_is_stable_across_fresh_sessions() {
    let cases = negative_cases();
    let expected = RUST_NEGATIVE_MATRIX
        .lines()
        .skip(1)
        .map(|line| {
            let columns = line.split('\t').collect::<Vec<_>>();
            assert_eq!(columns.len(), 4, "malformed Rust receipt row");
            (columns[0].to_owned(), (columns[1], columns[2], columns[3]))
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(expected.len(), cases.len());
    for (index, case) in cases.iter().enumerate() {
        let receipt = exercise_negative_case(case, SourceId::from_index(20_390 + index as u32));
        let (root, scala_result, rust_result) = expected
            .get(&case.name)
            .expect("every named case has a checked Rust receipt");
        assert_eq!(*root, case.root);
        assert_eq!(*scala_result, case.scala_result);
        assert_eq!(
            *rust_result, receipt,
            "Rust receipt changed for {}",
            case.name
        );
        println!(
            "ISSUE2039_NEGATIVE\t{}\t{}\t{}\t{receipt}",
            case.name, case.root, case.scala_result
        );
    }
    assert_eq!(
        cases.len(),
        15,
        "the named negative matrix inventory changed"
    );
}
