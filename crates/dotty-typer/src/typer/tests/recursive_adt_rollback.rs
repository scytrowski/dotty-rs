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

fn symbol_path(store: &SemanticStore, symbol: SymbolId) -> String {
    let mut names = Vec::new();
    let mut next = Some(symbol);
    while let Some(current) = next {
        let info = store.symbols.get(current);
        names.push(store.names.resolve(info.name.text()).to_owned());
        next = info.owner;
    }
    names.reverse();
    names.join(".")
}

fn normalized_type_shape(store: &SemanticStore, ty: TypeId) -> String {
    match store.types.get(ty) {
        Type::NoType => "NoType".to_owned(),
        Type::NoPrefix => "NoPrefix".to_owned(),
        Type::Error(_) => "Error".to_owned(),
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } => {
            format!("TermRef({})", symbol_path(store, *symbol))
        }
        Type::TermRef {
            target: TermRefTarget::Name(name),
            ..
        } => {
            format!(
                "TermRefName({})",
                store.names.resolve(name.as_name().text())
            )
        }
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => {
            format!("TypeRef({})", symbol_path(store, *symbol))
        }
        Type::TypeRef {
            target: TypeRefTarget::Name(name),
            ..
        } => {
            format!(
                "TypeRefName({})",
                store.names.resolve(name.as_name().text())
            )
        }
        Type::ThisType { class } => format!("This({})", symbol_path(store, *class)),
        Type::SuperType { .. } => "Super".to_owned(),
        Type::Constant(constant) => format!("Constant({constant:?})"),
        Type::Applied { tycon, args } => format!(
            "Applied({};{})",
            normalized_type_shape(store, *tycon),
            args.iter()
                .map(|arg| normalized_type_shape(store, *arg))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Method(method) => format!(
            "Method({})->{}",
            method
                .params
                .iter()
                .map(|param| normalized_type_shape(store, param.ty))
                .collect::<Vec<_>>()
                .join(","),
            normalized_type_shape(store, method.result)
        ),
        Type::Poly(poly) => format!("Poly({})", normalized_type_shape(store, poly.result)),
        Type::And { left, right } => format!(
            "And({},{})",
            normalized_type_shape(store, *left),
            normalized_type_shape(store, *right)
        ),
        Type::Or { left, right } => format!(
            "Or({},{})",
            normalized_type_shape(store, *left),
            normalized_type_shape(store, *right)
        ),
        Type::ByName { result } => format!("ByName({})", normalized_type_shape(store, *result)),
        Type::JavaArray { element } => format!("Array({})", normalized_type_shape(store, *element)),
        Type::Repeated { element } => {
            format!("Repeated({})", normalized_type_shape(store, *element))
        }
        Type::Bounds { .. } => "Bounds".to_owned(),
        Type::AliasingBounds { alias } => {
            format!("Alias({})", normalized_type_shape(store, *alias))
        }
        Type::Flexible { underlying } => {
            format!("Flexible({})", normalized_type_shape(store, *underlying))
        }
        Type::Refined { parent, .. } => {
            format!("Refined({})", normalized_type_shape(store, *parent))
        }
        Type::Recursive { parent } => {
            format!("Recursive({})", normalized_type_shape(store, *parent))
        }
        Type::RecThis { .. } => "RecThis".to_owned(),
        Type::TypeLambda(lambda) => format!(
            "TypeLambda({})",
            normalized_type_shape(store, lambda.result)
        ),
        Type::ParamRef { index, .. } => format!("ParamRef({index})"),
        Type::Match(_) => "MatchType".to_owned(),
        Type::MatchCase { pattern, result } => format!(
            "MatchCase({},{})",
            normalized_type_shape(store, *pattern),
            normalized_type_shape(store, *result)
        ),
        Type::Annotated { underlying, .. } => {
            format!("Annotated({})", normalized_type_shape(store, *underlying))
        }
        Type::Wildcard { bounds } => format!("Wildcard({})", normalized_type_shape(store, *bounds)),
        Type::ClassInfo(_) => "ClassInfo".to_owned(),
    }
}

fn typed_kind_shape(tree: &dotty_core::ast::TypedTree) -> &'static str {
    match &tree.kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::Literal(_) => "Literal",
        TreeKind::Apply(_) => "Apply",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::New(_) => "New",
        TreeKind::Typed(_) => "Typed",
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Assign(_) => "Assign",
        TreeKind::Block(_) => "Block",
        TreeKind::If(_) => "If",
        TreeKind::Match(_) => "Match",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::Return(_) => "Return",
        TreeKind::While(_) => "While",
        TreeKind::Try(_) => "Try",
        TreeKind::Closure(_) => "Closure",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::Template(_) => "Template",
        TreeKind::PackageDef(_) => "PackageDef",
        TreeKind::Import(_) => "Import",
        TreeKind::Export(_) => "Export",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::SingletonTypeTree(_) => "SingletonTypeTree",
        TreeKind::AppliedTypeTree(_) => "AppliedTypeTree",
        TreeKind::RefinedTypeTree(_) => "RefinedTypeTree",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::MatchTypeTree(_) => "MatchTypeTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::UnApply(_) => "UnApply",
        TreeKind::Annotated(_) => "Annotated",
        TreeKind::Quote(_) => "Quote",
        TreeKind::Splice(_) => "Splice",
        TreeKind::QuotePattern(_) => "QuotePattern",
        TreeKind::SplicePattern(_) => "SplicePattern",
        TreeKind::Inlined(_) => "Inlined",
        TreeKind::PhaseSpecific(_) => "PhaseSpecific",
    }
}

fn normalized_success_root(
    parsed: &dotty_parser::ParseResult,
    typer: &SourceTyper<'_>,
    source: SourceId,
    root: TreeId<Untyped>,
    method_name: &str,
) -> String {
    let store = typer.store();
    let TreeKind::DefDef(definition) = &parsed.ast.get(root).kind else {
        unreachable!()
    };
    let rhs = definition.rhs.expect("accepted root has a body");
    let root_span = parsed.ast.get(root).position.unwrap().span().range();
    let mut shape = parsed
        .ast
        .iter()
        .filter_map(|(source_tree, source_node)| {
            let span = source_node.position?.span().range();
            if span.start() < root_span.start() || span.end() > root_span.end() {
                return None;
            }
            let typed_id = typer.source_typed_index().get(source, source_tree)?;
            let typed = typer.typed_ast().get(typed_id);
            Some(format!(
                "{}..{}:{}:{}",
                span.start(),
                span.end(),
                typed_kind_shape(typed),
                normalized_type_shape(store, typed.ty)
            ))
        })
        .collect::<Vec<_>>();
    shape.sort();

    let TreeKind::Match(matched) = &parsed.ast.get(rhs).kind else {
        panic!("accepted method keeps its match root")
    };
    let TreeKind::CaseDef(succ_case) = &parsed.ast.get(matched.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(extractor) = &parsed.ast.get(succ_case.pattern).kind else {
        panic!("Succ pattern remains an extractor")
    };
    let binder_tree = extractor.args[0];
    let binder = typer
        .pattern_binding_symbol_at(source, binder_tree)
        .expect("Succ.inner binder is published");
    let method_symbol = typer
        .index
        .symbol_at(source, root)
        .expect("canonical source method symbol");
    assert_eq!(store.symbols.get(binder).owner, Some(method_symbol));
    let SymbolInfo::Complete(binder_type) = *store.symbols.info(binder) else {
        panic!("binder type is complete")
    };
    let expr_symbol = class_symbol(parsed, store, typer.index, source, "Expr");
    assert_eq!(type_symbol(store, binder_type), expr_symbol);

    let body = match &parsed.ast.get(succ_case.body).kind {
        TreeKind::Block(block) => block.expr,
        _ => succ_case.body,
    };
    let (reference, callee) = if method_name == "eval" {
        let TreeKind::PhaseSpecific(UntypedNode::InfixOp(addition)) = &parsed.ast.get(body).kind
        else {
            panic!("eval case keeps its addition")
        };
        let call = addition.left;
        let TreeKind::Apply(application) = &parsed.ast.get(call).kind else {
            panic!("recursive eval call remains an Apply")
        };
        (application.args[0], Some(application.function))
    } else {
        (body, None)
    };
    let typed_reference = typer
        .source_typed_index()
        .get(source, reference)
        .expect("binder reference has a typed mapping");
    let Type::TermRef {
        target: TermRefTarget::Symbol(reference_symbol),
        ..
    } = store.types.get(typer.typed_ast().get(typed_reference).ty)
    else {
        panic!("body reference retains canonical TermRef")
    };
    assert_eq!(
        *reference_symbol, binder,
        "body reference points to its own case binder"
    );
    let callee_shape = if let Some(callee_tree) = callee {
        let canonical = typer
            .index
            .symbol_at(source, root)
            .expect("canonical eval method");
        let typed_callee = typer
            .source_typed_index()
            .get(source, callee_tree)
            .expect("recursive callee has a typed mapping");
        let Type::TermRef {
            target: TermRefTarget::Symbol(selected),
            ..
        } = store.types.get(typer.typed_ast().get(typed_callee).ty)
        else {
            panic!("recursive callee is a canonical TermRef")
        };
        assert_eq!(*selected, canonical);
        symbol_path(store, *selected)
    } else {
        "none".to_owned()
    };
    let span = parsed.ast.get(root).position.unwrap().span().range();
    format!(
        "{}@{}..{};root-type={};binder={};binder-type={};body-reference={};callee={};shape=[{}]",
        method_name,
        span.start(),
        span.end(),
        normalized_type_shape(
            store,
            typer
                .typed_ast()
                .get(
                    typer
                        .source_typed_index()
                        .get(source, root)
                        .expect("root typed mapping")
                )
                .ty
        ),
        symbol_path(store, binder),
        normalized_type_shape(store, binder_type),
        symbol_path(store, *reference_symbol),
        callee_shape,
        shape.join(";")
    )
}

fn late_failure_span_receipt(parsed: &dotty_parser::ParseResult, root: TreeId<Untyped>) -> String {
    let TreeKind::DefDef(definition) = &parsed.ast.get(root).kind else {
        unreachable!()
    };
    let root_span = parsed.ast.get(root).position.unwrap().span().range();
    let rhs = definition.rhs.expect("failing method has a body");
    let TreeKind::Match(matched) = &parsed.ast.get(rhs).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(succ_case) = &parsed.ast.get(matched.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(extractor) = &parsed.ast.get(succ_case.pattern).kind else {
        unreachable!()
    };
    let binder = parsed
        .ast
        .get(extractor.args[0])
        .position
        .unwrap()
        .span()
        .range();
    let body = match &parsed.ast.get(succ_case.body).kind {
        TreeKind::Block(block) => block.expr,
        _ => succ_case.body,
    };
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(addition)) = &parsed.ast.get(body).kind else {
        unreachable!()
    };
    let recursive = parsed
        .ast
        .get(addition.left)
        .position
        .unwrap()
        .span()
        .range();
    format!(
        "root={}..{};binder={}..{};recursive_apply={}..{}",
        root_span.start(),
        root_span.end(),
        binder.start(),
        binder.end(),
        recursive.start(),
        recursive.end()
    )
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
    let first_span_receipt = late_failure_span_receipt(&parsed, broken);
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
    let fresh_unwrap = tree_named(&fresh_parsed, &fresh_store, "unwrap");
    let fresh_eval = tree_named(&fresh_parsed, &fresh_store, "eval");
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
    assert_eq!(
        normalized_error(&fresh_error, &fresh_parsed),
        normalized_error(&first, &parsed),
        "diagnostic category and independently parsed tree span match"
    );
    assert_eq!(
        late_failure_span_receipt(&fresh_parsed, fresh_broken),
        first_span_receipt,
        "root, binder and recursive-Apply spans are independently read from both parses"
    );
    let fresh_retry = fresh_typer
        .type_method_definition(fresh_broken)
        .expect_err("the independent session retry preserves the same failure");
    assert_eq!(
        normalized_error(&fresh_retry, &fresh_parsed),
        normalized_error(&fresh_error, &fresh_parsed)
    );
    fresh_typer
        .type_method_definition(fresh_unwrap)
        .expect("untouched unwrap types in the independent fresh session");
    fresh_typer
        .type_method_definition(fresh_eval)
        .expect("untouched eval types in the independent fresh session");
    let recovered_receipt = format!(
        "{}\n{}",
        normalized_success_root(&parsed, &typer, source, unwrap, "unwrap"),
        normalized_success_root(&parsed, &typer, source, eval, "eval")
    );
    let fresh_receipt = format!(
        "{}\n{}",
        normalized_success_root(
            &fresh_parsed,
            &fresh_typer,
            fresh_source,
            fresh_unwrap,
            "unwrap"
        ),
        normalized_success_root(
            &fresh_parsed,
            &fresh_typer,
            fresh_source,
            fresh_eval,
            "eval"
        )
    );
    assert_eq!(
        recovered_receipt, fresh_receipt,
        "recovered success roots preserve normalized type shapes, binder/reference relationships, and selected recursive callee"
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
