use super::super::*;

const EXPR_SOURCE: &str =
    include_str!("../../../tests/fixtures/source-recursive-adt-match-3.9.0/Expr.scala");
const NEGATIVE_CASES: &str =
    include_str!("../../../tests/fixtures/source-recursive-adt-match-3.9.0/negative-cases.tsv");
const RUST_NEGATIVE_MATRIX: &str = include_str!(
    "../../../tests/fixtures/source-recursive-adt-match-3.9.0/rust-negative-matrix.tsv"
);
const BINARY_EXPR_SOURCE: &str =
    include_str!("../../../tests/fixtures/source-binary-recursive-adt-3.9.0/Expr.scala");
const BINARY_EVAL_FAILURE_RECEIPT: &str = include_str!(
    "../../../tests/fixtures/source-binary-recursive-adt-3.9.0/rust-eval-negative-two-call.tsv"
);
const BINARY_CONTROL_ROOTS: &str = include_str!(
    "../../../tests/fixtures/source-binary-recursive-adt-3.9.0/rust-control-roots.tsv"
);
const BINARY_CONTROL_RECEIPTS: &str = include_str!(
    "../../../tests/fixtures/source-binary-recursive-adt-3.9.0/rust-control-matrix.tsv"
);
const NESTED_EXPR_SOURCE: &str =
    include_str!("../../../tests/fixtures/source-nested-recursive-adt-3.9.0/Expr.scala");
const NESTED_ROLLBACK_CONTROL_SUFFIX: &str = "\nobject RollbackControls:\n  def prior(value: Int): Int = value\n  def peelLeftFails(expr: Expr): Boolean =\n    expr match\n      case Add(Add(left, _), _) => true\n      case _ => expr\n  def sumFails(expr: Expr): Boolean =\n    expr match\n      case Num(_) => true\n      case Add(Num(value), right) => value + Evaluator.sum(right)\n      case Add(left, right) => Evaluator.sum(left) + Evaluator.sum(right)\n";
const NESTED_LATE_FAILURE_RECEIPTS: &str =
    include_str!("../../../tests/fixtures/source-nested-recursive-adt-3.9.0/rust-late-failure.tsv");
const NESTED_SCALA_PEEL_CONTROL: &str = include_str!(
    "../../../tests/fixtures/source-nested-recursive-adt-3.9.0/LatePeelLeftResult.scala"
);
const NESTED_SCALA_SUM_CONTROL: &str =
    include_str!("../../../tests/fixtures/source-nested-recursive-adt-3.9.0/LateSumResult.scala");
const NESTED_SCALA_OUTER_ARITY_CONTROL: &str = include_str!(
    "../../../tests/fixtures/source-nested-recursive-adt-3.9.0/WrongOuterPatternArity.scala"
);

struct NegativeCase {
    name: String,
    root: String,
    scala_result: String,
    mutations: Vec<(String, String)>,
}

struct BinaryAddCaseNodes {
    binders: [TreeId<Untyped>; 2],
    calls: [TreeId<Untyped>; 2],
    arguments: [TreeId<Untyped>; 2],
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
    typer.transaction_failure_observer = Some(Rc::new(move |typer, _error| {
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
    println!(
        "ISSUE2040_ROLLBACK\tlate_result_mismatch\tobserved=Succ.inner:Expr,Evaluator.eval(inner):Int,Match,CaseDef[0..1];restored=store,resolver,typed_arena,SourceTypedIndex,scope_projection,binder_frames"
    );
    println!(
        "ISSUE2040_ROLLBACK\tsame_session_retry\trepeated_diagnostic=stable;recovered=Evaluator.unwrap(expr:Expr):Expr,Evaluator.eval(expr:Expr):Int;fresh_session=normalized_equal"
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

#[test]
fn binary_recursive_second_argument_failure_and_late_failure_restore_then_retry_eval() {
    let source_text = format!(
        "{BINARY_EXPR_SOURCE}\nobject BinaryEvalControls:\n  def invalidSecondArgument(expr: Expr): Int =\n    expr match\n      case Num(value) => value\n      case Add(left, right) => Evaluator.eval(left) + Evaluator.eval(true)\n  def failsAfterBoth(expr: Expr): Boolean =\n    expr match\n      case Num(_) => true\n      case Add(left, right) => Evaluator.eval(left) + Evaluator.eval(right)\n"
    );
    let source = SourceId::from_index(20_781);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let (parsed, index) = parse_and_name_unit(&source_text, source, &mut store, &mut packages);
    let eval = tree_named(&parsed, &store, "eval");
    let invalid = tree_named(&parsed, &store, "invalidSecondArgument");
    let late_failure = tree_named(&parsed, &store, "failsAfterBoth");
    let eval_symbol = index.symbol_at(source, eval).unwrap();
    let invalid_symbol = index.symbol_at(source, invalid).unwrap();
    let expr = class_symbol(&parsed, &store, &index, source, "Expr");
    let boolean = type_symbol(&store, definitions.boolean);
    let invalid_nodes = binary_add_case_nodes(&parsed, invalid);
    let invalid_binders = invalid_nodes.binders;
    let invalid_calls = invalid_nodes.calls;
    let invalid_arguments = invalid_nodes.arguments;
    let late_nodes = binary_add_case_nodes(&parsed, late_failure);
    let late_binders = late_nodes.binders;
    let late_calls = late_nodes.calls;
    let late_arguments = late_nodes.arguments;
    let TreeKind::Apply(invalid_second_application) = &parsed.ast.get(invalid_calls[1]).kind else {
        unreachable!()
    };
    let invalid_second_function = invalid_second_application.function;
    let invalid_bool_argument = invalid_arguments[1];
    assert!(matches!(
        parsed.ast.get(invalid_bool_argument).kind,
        TreeKind::Literal(_)
    ));

    let mut typer = typer_for(&parsed, &mut store, &packages, definitions, &index, source);

    let before_store = typer.store().checkpoint();
    let before_typed = typer.typed_ast().checkpoint();
    let before_mappings = typer.source_typed_index().len();
    let invalid_observed = Rc::new(RefCell::new(false));
    let invalid_observed_by_callback = Rc::clone(&invalid_observed);
    typer.transaction_failure_observer = Some(Rc::new(move |typer, _error| {
        let Some(first_call) = typer.source_typed_index().get(source, invalid_calls[0]) else {
            return;
        };
        let TreeKind::Apply(first_application) = &typer.typed_ast().get(first_call).kind else {
            panic!("the first recursive application is typed before the second argument fails")
        };
        assert_eq!(first_application.args.len(), 1);
        let left_binder = typer
            .pattern_binding_symbol_at(source, invalid_binders[0])
            .expect("first Add binder is live before rollback");
        assert_eq!(
            typer.source_typed_index().get(source, invalid_arguments[0]),
            Some(first_application.args[0])
        );
        assert!(
            typer
                .source_typed_index()
                .get(source, invalid_arguments[1])
                .is_some()
        );
        assert!(
            typer
                .source_typed_index()
                .get(source, invalid_calls[1])
                .is_none(),
            "the invalid second application is not published"
        );
        let typed_second_function = typer
            .source_typed_index()
            .get(source, invalid_second_function)
            .expect("the second call resolves its canonical function before argument checking");
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_second_function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == eval_symbol
        ));
        let SymbolInfo::Complete(eval_signature) = *typer.store().symbols.info(eval_symbol) else {
            panic!("the canonical eval signature is complete before argument checking")
        };
        let Type::Method(eval_signature) = typer.store().types.get(eval_signature) else {
            panic!("canonical eval has a method signature")
        };
        assert_eq!(eval_signature.params.len(), 1);
        assert_eq!(
            type_symbol(typer.store(), eval_signature.params[0].ty),
            expr
        );
        let typed_bad_argument = typer
            .source_typed_index()
            .get(source, invalid_arguments[1])
            .unwrap();
        assert!(
            matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(typed_bad_argument).ty),
                Type::Constant(dotty_core::Constant::Boolean(true))
            ),
            "the second recursive argument is Boolean where canonical eval expects Expr"
        );
        let right_binder = typer
            .pattern_binding_symbol_at(source, invalid_binders[1])
            .expect("second Add binder is live before rollback");
        let SymbolInfo::Complete(right_type) = *typer.store().symbols.info(right_binder) else {
            panic!("the second Add binder has its canonical type before rollback")
        };
        assert_eq!(type_symbol(typer.store(), right_type), expr);
        assert_eq!(
            typer.store().symbols.get(left_binder).owner,
            Some(invalid_symbol)
        );
        *invalid_observed_by_callback.borrow_mut() = true;
    }));
    let error = typer
        .type_method_definition(invalid)
        .expect_err("eval(true) is rejected as the second Expr argument");
    assert!(
        matches!(
            error,
            TyperError::ApplicationArgumentTypeMismatch {
                tree_index,
                argument_index: 0,
                actual,
                ..
            } if tree_index == invalid_calls[1].index()
                && type_symbol(typer.store(), actual) == boolean
        ),
        "the second call reports a concrete argument type mismatch: {error:?}"
    );
    assert!(
        *invalid_observed.borrow(),
        "the first call and second argument were observed before rollback"
    );
    assert_eq!(typer.store().checkpoint(), before_store);
    assert_eq!(typer.typed_ast().checkpoint(), before_typed);
    assert_eq!(typer.source_typed_index().len(), before_mappings);
    assert!(typer.source_typed_index().get(source, invalid).is_none());
    for call in invalid_calls {
        assert!(typer.source_typed_index().get(source, call).is_none());
    }
    for argument in invalid_arguments {
        assert!(typer.source_typed_index().get(source, argument).is_none());
    }
    for binder in invalid_binders {
        assert!(typer.source_typed_index().get(source, binder).is_none());
        assert!(typer.pattern_binding_symbol_at(source, binder).is_none());
    }

    let before_store = typer.store().checkpoint();
    let before_typed = typer.typed_ast().checkpoint();
    let before_mappings = typer.source_typed_index().len();
    let late_observation = Rc::new(RefCell::new(None));
    let late_observation_by_callback = Rc::clone(&late_observation);
    typer.transaction_failure_observer = Some(Rc::new(move |typer, _error| {
        let mapped_calls = late_calls.map(|call| typer.source_typed_index().get(source, call));
        let [Some(first_call), Some(second_call)] = mapped_calls else {
            return;
        };
        for (position, (call, argument)) in [first_call, second_call]
            .into_iter()
            .zip(late_arguments)
            .enumerate()
        {
            let TreeKind::Apply(application) = &typer.typed_ast().get(call).kind else {
                panic!("both recursive calls remain ordinary Apply nodes before rollback")
            };
            assert_eq!(application.args.len(), 1);
            let callee = match typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty)
            {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("recursive callee is a canonical term reference: {other:?}"),
            };
            assert_eq!(
                callee, eval_symbol,
                "both calls select canonical Evaluator.eval"
            );
            assert_eq!(
                application.args[0],
                typer.source_typed_index().get(source, argument).unwrap()
            );
            let binder = typer
                .pattern_binding_symbol_at(source, late_binders[position])
                .expect("both Add binders are live before rollback");
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(application.args[0]).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == binder
            ));
        }
        *late_observation_by_callback.borrow_mut() = Some((first_call, second_call));
    }));
    let late_error = typer
        .type_method_definition(late_failure)
        .expect_err("Int-valued Add case mismatches failsAfterBoth's Boolean result");
    assert!(
        matches!(
            late_error,
            TyperError::ExpectedExpressionTypeMismatch { .. }
        ),
        "the whole-method mismatch occurs after the binary case has typed: {late_error:?}"
    );
    let observed_calls = (*late_observation.borrow())
        .expect("failure observer saw both recursive Apply mappings before rollback");
    assert_ne!(observed_calls.0, observed_calls.1);
    assert_eq!(typer.store().checkpoint(), before_store);
    assert_eq!(typer.typed_ast().checkpoint(), before_typed);
    assert_eq!(typer.source_typed_index().len(), before_mappings);
    assert!(
        typer
            .source_typed_index()
            .get(source, late_failure)
            .is_none()
    );
    for call in late_calls {
        assert!(typer.source_typed_index().get(source, call).is_none());
    }
    for argument in late_arguments {
        assert!(typer.source_typed_index().get(source, argument).is_none());
    }
    for binder in late_binders {
        assert!(typer.source_typed_index().get(source, binder).is_none());
        assert!(typer.pattern_binding_symbol_at(source, binder).is_none());
    }

    let typed_eval = typer
        .type_method_definition(eval)
        .expect("the original binary recursive eval retries in the same SourceTyper");
    assert_eq!(
        typer.source_typed_index().get(source, eval),
        Some(typed_eval)
    );
    let receipt = concat!(
        "second_argument\tApplicationArgumentTypeMismatch(Boolean,Expr);first_call=observed;second_call=unpublished;rollback=both_calls,arguments,binders;retry=original_eval\n",
        "late_result\tExpectedExpressionTypeMismatch;observed=eval(left):canonical,eval(right):canonical;rollback=both_calls,arguments,binders;retry=original_eval"
    );
    assert_eq!(
        receipt,
        BINARY_EVAL_FAILURE_RECEIPT
            .split_once('\n')
            .expect("binary failure receipt header")
            .1
            .trim_end()
    );
    println!("ISSUE2078_BINARY_EVAL_FAILURES\t{receipt}");
}

#[test]
fn binary_late_failure_restores_complete_state_retries_then_types_both_original_roots() {
    let source_text = format!(
        "{BINARY_EXPR_SOURCE}\nobject BinaryRollbackControls:\n  def prior(value: Int): Int = value\n  def failsAfterBoth(expr: Expr): Boolean =\n    expr match\n      case Num(_) => true\n      case Add(left, right) => Evaluator.eval(left) + Evaluator.eval(right)\n"
    );
    let source = SourceId::from_index(20_791);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let (parsed, index) = parse_and_name_unit(&source_text, source, &mut store, &mut packages);
    let prior = tree_named(&parsed, &store, "prior");
    let late_failure = tree_named(&parsed, &store, "failsAfterBoth");
    let select_left = tree_named(&parsed, &store, "selectLeft");
    let eval = tree_named(&parsed, &store, "eval");
    let eval_symbol = index.symbol_at(source, eval).unwrap();
    let expr = class_symbol(&parsed, &store, &index, source, "Expr");
    let nodes = binary_add_case_nodes(&parsed, late_failure);
    let TreeKind::DefDef(failing_definition) = &parsed.ast.get(late_failure).kind else {
        unreachable!()
    };
    let late_match_tree = failing_definition.rhs.unwrap();
    let mut typer = typer_for(&parsed, &mut store, &packages, definitions, &index, source);
    let prior_typed = typer
        .type_method_definition(prior)
        .expect("an unrelated successful root remains published before the late failure");
    let before = binary_complete_state_projection(&typer, &parsed);
    let nodes_seen = Rc::new(RefCell::new(false));
    let nodes_seen_by_observer = Rc::clone(&nodes_seen);
    typer.transaction_failure_observer = Some(Rc::new(move |typer, _error| {
        let typed_calls = nodes
            .calls
            .map(|call| typer.source_typed_index().get(source, call));
        let [Some(first_call), Some(second_call)] = typed_calls else {
            return;
        };
        let mut binder_symbols = Vec::new();
        for (position, (typed_call, source_argument)) in [first_call, second_call]
            .into_iter()
            .zip(nodes.arguments)
            .enumerate()
        {
            let TreeKind::Apply(application) = &typer.typed_ast().get(typed_call).kind else {
                panic!("both recursive applications are typed before rollback")
            };
            assert_eq!(application.args.len(), 1);
            let selected = match typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty)
            {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("recursive callee is canonical: {other:?}"),
            };
            assert_eq!(selected, eval_symbol);
            assert_eq!(
                typer.source_typed_index().get(source, source_argument),
                Some(application.args[0]),
                "each call argument maps from its original source reference"
            );
            let binder = typer
                .pattern_binding_symbol_at(source, nodes.binders[position])
                .expect("each original Add binder is live before rollback");
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(application.args[0]).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == binder
            ));
            let SymbolInfo::Complete(binder_type) = *typer.store().symbols.info(binder) else {
                panic!("the live Add binder has a complete type")
            };
            assert_eq!(type_symbol(typer.store(), binder_type), expr);
            binder_symbols.push(binder);
        }
        assert_ne!(binder_symbols[0], binder_symbols[1]);
        let typed_match = typer
            .source_typed_index()
            .get(source, late_match_tree)
            .expect("the failing root's Match was typed before result checking");
        assert!(matches!(
            typer.typed_ast().get(typed_match).kind,
            TreeKind::Match(_)
        ));
        assert!(
            typer
                .source_typed_index()
                .get(source, late_failure)
                .is_none()
        );
        *nodes_seen_by_observer.borrow_mut() = true;
    }));

    let first = typer
        .type_method_definition(late_failure)
        .expect_err("the Boolean result fails after both recursive Add calls type");
    assert!(
        matches!(first, TyperError::ExpectedExpressionTypeMismatch { .. }),
        "{first:?}"
    );
    assert!(
        *nodes_seen.borrow(),
        "both calls and binders existed before unwind"
    );
    let first_debug = format!("{first:?}");
    assert_eq!(
        binary_complete_state_projection(&typer, &parsed),
        before,
        "late failure restores all prior state contents, indexes, and scope buckets"
    );
    assert_eq!(
        typer.source_typed_index().get(source, prior),
        Some(prior_typed)
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, late_failure)
            .is_none()
    );
    for tree in nodes
        .calls
        .into_iter()
        .chain(nodes.arguments)
        .chain(nodes.binders)
    {
        assert!(typer.source_typed_index().get(source, tree).is_none());
    }
    for binder in nodes.binders {
        assert!(typer.pattern_binding_symbol_at(source, binder).is_none());
    }

    let retry = typer
        .type_method_definition(late_failure)
        .expect_err("the same failing root can be retried deterministically");
    assert_eq!(format!("{retry:?}"), first_debug);
    assert_eq!(
        binary_complete_state_projection(&typer, &parsed),
        before,
        "repeated failure preserves the same complete state"
    );

    let typed_select_left = typer
        .type_method_definition(select_left)
        .expect("the original selectLeft first succeeds in this live typer after rollback");
    let typed_eval = typer
        .type_method_definition(eval)
        .expect("the original eval first succeeds in this live typer after rollback");
    assert_eq!(
        typer.source_typed_index().get(source, select_left),
        Some(typed_select_left)
    );
    assert_eq!(
        typer.source_typed_index().get(source, eval),
        Some(typed_eval)
    );
    let recovered = format!(
        "{}\n{}",
        normalized_success_root(&parsed, &typer, source, select_left, "selectLeft"),
        normalized_binary_eval_recovery(&parsed, &typer, source, eval)
    );

    let fresh_source = SourceId::from_index(20_792);
    let mut fresh_store = SemanticStore::new();
    let fresh_definitions = Definitions::bootstrap(&mut fresh_store);
    let mut fresh_packages = Packages::new();
    let (fresh_parsed, fresh_index) = parse_and_name_unit(
        &source_text,
        fresh_source,
        &mut fresh_store,
        &mut fresh_packages,
    );
    let fresh_prior = tree_named(&fresh_parsed, &fresh_store, "prior");
    let fresh_late_failure = tree_named(&fresh_parsed, &fresh_store, "failsAfterBoth");
    let fresh_select_left = tree_named(&fresh_parsed, &fresh_store, "selectLeft");
    let fresh_eval = tree_named(&fresh_parsed, &fresh_store, "eval");
    let mut fresh_typer = typer_for(
        &fresh_parsed,
        &mut fresh_store,
        &fresh_packages,
        fresh_definitions,
        &fresh_index,
        fresh_source,
    );
    fresh_typer
        .type_method_definition(fresh_prior)
        .expect("the independent session retains the same prior successful root");
    let fresh_error = fresh_typer
        .type_method_definition(fresh_late_failure)
        .expect_err("the independent session rejects the same late-failing root");
    assert_eq!(
        normalized_error(&fresh_error, &fresh_parsed),
        normalized_error(&first, &parsed)
    );
    let fresh_retry = fresh_typer
        .type_method_definition(fresh_late_failure)
        .expect_err("the independent failure also retries deterministically");
    assert_eq!(format!("{fresh_retry:?}"), format!("{fresh_error:?}"));
    fresh_typer
        .type_method_definition(fresh_select_left)
        .expect("selectLeft remains recoverable in a fresh session");
    fresh_typer
        .type_method_definition(fresh_eval)
        .expect("eval remains recoverable in a fresh session");
    let fresh_recovered = format!(
        "{}\n{}",
        normalized_success_root(
            &fresh_parsed,
            &fresh_typer,
            fresh_source,
            fresh_select_left,
            "selectLeft"
        ),
        normalized_binary_eval_recovery(&fresh_parsed, &fresh_typer, fresh_source, fresh_eval)
    );
    assert_eq!(
        recovered, fresh_recovered,
        "recovery receipts normalize across sessions"
    );
    println!(
        "ISSUE2080_ROLLBACK\tlate_after_two_calls\tbefore=both_Add_Expr_binders_and_both_eval_Applies_same_canonical_method_distinct_argument_binders;restored=complete_state_projection+resolver+typed_ast+types+SourceTypedIndex+scope_buckets;retry=stable_failure_then_selectLeft_and_eval_recovered_in_session;fresh_session=normalized_equal"
    );
}

fn binary_complete_state_projection(
    typer: &SourceTyper<'_>,
    parsed: &dotty_parser::ParseResult,
) -> Vec<(&'static str, String)> {
    let mut state = issue_1999_late_failure_projection(typer, parsed);
    state.extend([
        ("source type index", format!("{:?}", typer.type_index)),
        (
            "synthetic package entries",
            format!("{:?}", typer.synthetic_package_entries),
        ),
        (
            "synthetic generated member entries",
            format!("{:?}", typer.synthetic_class_member_entries),
        ),
        (
            "import qualifier cache",
            format!("{:?}", typer.import_qualifier_cache),
        ),
        (
            "import member cache",
            format!("{:?}", typer.import_member_cache),
        ),
        (
            "semantic transaction depth",
            format!("{}", typer.semantic_transaction_depth),
        ),
        (
            "source type projection depth",
            format!("{}", typer.source_type_projection_depth),
        ),
        (
            "signature parameter in progress",
            format!("{:?}", typer.signature_parameter_in_progress),
        ),
        (
            "uninitialized parent receiver",
            format!("{:?}", typer.uninitialized_parent_receiver),
        ),
        (
            "pattern definition attempts",
            format!("{:?}", typer.patdef_typing_attempts),
        ),
        (
            "local method symbol index",
            format!("{:?}", typer.local_methods.symbols_by_tree),
        ),
        (
            "local method definition index",
            format!("{:?}", typer.local_methods.definitions),
        ),
        (
            "local method scope index",
            format!("{:?}", typer.local_methods.scopes),
        ),
        (
            "local method declaration contexts",
            format!("{:?}", typer.local_methods.declaration_contexts),
        ),
        (
            "local method parameter indexes",
            format!(
                "{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
                typer.local_methods.parameter_symbols_by_tree,
                typer.local_methods.parameter_definitions,
                typer.local_methods.parameter_contexts,
                typer.local_methods.type_parameter_symbols_by_tree,
                typer.local_methods.type_parameter_definitions,
                typer.local_methods.type_parameter_contexts
            ),
        ),
    ]);
    state
}

#[test]
fn nested_late_failures_restore_and_retry_before_original_roots_recover() {
    assert!(
        NESTED_SCALA_PEEL_CONTROL.starts_with(NESTED_EXPR_SOURCE),
        "Scala peel control preserves the immutable source prefix"
    );
    assert!(
        NESTED_SCALA_SUM_CONTROL.starts_with(NESTED_EXPR_SOURCE),
        "Scala sum control preserves the immutable source prefix"
    );
    assert!(
        NESTED_SCALA_OUTER_ARITY_CONTROL.starts_with(NESTED_EXPR_SOURCE)
            && NESTED_SCALA_OUTER_ARITY_CONTROL.contains("case Add(Add(left, _), _, _) => left"),
        "Scala outer-arity control preserves the immutable source and malformed pattern"
    );
    assert!(
        NESTED_SCALA_PEEL_CONTROL.contains("case Add(Add(left, _), _) => true")
            && NESTED_SCALA_SUM_CONTROL
                .contains("case Add(Num(value), right) => value + Evaluator.sum(right)")
            && NESTED_SCALA_SUM_CONTROL
                .contains("case Add(left, right) => Evaluator.sum(left) + Evaluator.sum(right)"),
        "Scala classifications cover the nested source shapes exercised by Rust"
    );
    let source_text = format!("{NESTED_EXPR_SOURCE}{NESTED_ROLLBACK_CONTROL_SUFFIX}");
    let source = SourceId::from_index(21_361);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let (parsed, index) = parse_and_name_unit(&source_text, source, &mut store, &mut packages);
    let prior = tree_named(&parsed, &store, "prior");
    let peel_failure = tree_named(&parsed, &store, "peelLeftFails");
    let sum_failure = tree_named(&parsed, &store, "sumFails");
    let peel_failure_symbol = index
        .symbol_at(source, peel_failure)
        .expect("rollback source root has a symbol");
    let rollback_owner = store
        .symbols
        .get(peel_failure_symbol)
        .owner
        .expect("rollback root has an enclosing module");
    let rollback_owner_name = store
        .names
        .resolve(store.symbols.get(rollback_owner).name.text())
        .to_owned();
    assert_eq!(rollback_owner_name, "RollbackControls$");
    let sum_failure_symbol = index
        .symbol_at(source, sum_failure)
        .expect("sum rollback source root has a symbol");
    assert_eq!(
        store.symbols.get(sum_failure_symbol).owner,
        Some(rollback_owner),
        "both rollback roots belong to the emitted RollbackControls$ source owner"
    );
    let peel_adds = nested_extractors(&parsed, &store, peel_failure, "Add");
    let sum_adds = nested_extractors(&parsed, &store, sum_failure, "Add");
    let sum_num_extractors = nested_extractors(&parsed, &store, sum_failure, "Num");
    assert_eq!(
        peel_adds.len(),
        2,
        "peelLeft control has both nested Add sites"
    );
    assert_eq!(sum_adds.len(), 2, "sum control has both nested Add sites");
    assert_eq!(sum_num_extractors.len(), 2, "sum control has two Num sites");
    let TreeKind::Apply(peel_outer) = &parsed.ast.get(peel_adds[0]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(peel_inner) = &parsed.ast.get(peel_adds[1]).kind else {
        unreachable!()
    };
    assert_eq!(peel_outer.args[0], peel_adds[1]);
    let peel_left = peel_inner.args[0];
    let TreeKind::Apply(sum_first_add) = &parsed.ast.get(sum_adds[0]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(sum_second_add) = &parsed.ast.get(sum_adds[1]).kind else {
        unreachable!()
    };
    let sum_right_binders = [sum_first_add.args[1], sum_second_add.args[1]];
    let sum_left_binder = sum_second_add.args[0];
    let TreeKind::Apply(sum_num) = &parsed.ast.get(*sum_num_extractors.last().unwrap()).kind else {
        unreachable!()
    };
    let sum_value_binder = sum_num.args[0];
    let sum_calls = nested_named_calls(&parsed, &store, sum_failure, "sum");
    assert_eq!(
        sum_calls.len(),
        3,
        "all three original recursive sum Apply sites are retained"
    );

    let original_peel = tree_named(&parsed, &store, "peelLeft");
    let original_sum = tree_named_before(&parsed, &store, "sum", NESTED_EXPR_SOURCE.len() as u32);
    let original_sum_symbol = index.symbol_at(source, original_sum).unwrap();
    let expr = class_symbol(&parsed, &store, &index, source, "Expr");
    let mut typer = typer_for(&parsed, &mut store, &packages, definitions, &index, source);
    let prior_typed = typer
        .type_method_definition(prior)
        .expect("the unrelated valid root is established before failures");
    let before = binary_complete_state_projection(&typer, &parsed);

    let peel_observed = Rc::new(RefCell::new(false));
    let peel_observed_by_callback = Rc::clone(&peel_observed);
    let peel_adds_by_callback = peel_adds.clone();
    typer.transaction_failure_observer = Some(Rc::new(move |typer, _error| {
        let mapped = peel_adds_by_callback
            .iter()
            .map(|tree| typer.source_typed_index().get(source, *tree))
            .collect::<Vec<_>>();
        if mapped.iter().any(Option::is_none) {
            return;
        }
        let outer = mapped[0].unwrap();
        let inner = mapped[1].unwrap();
        for typed in [outer, inner] {
            assert!(matches!(
                typer.typed_ast().get(typed).kind,
                TreeKind::UnApply(_)
            ));
        }
        let binder = typer
            .pattern_binding_symbol_at(source, peel_left)
            .expect("the inner left binder exists before rollback");
        let SymbolInfo::Complete(binding_type) = *typer.store().symbols.info(binder) else {
            panic!("the inner left binder has a complete type before rollback")
        };
        assert_eq!(type_symbol(typer.store(), binding_type), expr);
        assert!(
            typer
                .source_typed_index()
                .get(source, peel_failure)
                .is_none()
        );
        *peel_observed_by_callback.borrow_mut() = true;
    }));
    let peel_error = typer
        .type_method_definition(peel_failure)
        .expect_err("Expr-valued nested peelLeft body mismatches Boolean result");
    assert!(matches!(
        peel_error,
        TyperError::ExpectedExpressionTypeMismatch { .. }
    ));
    assert!(
        *peel_observed.borrow(),
        "nested Add sites and left binder were observed before unwind"
    );
    let peel_debug = format!("{peel_error:?}");
    assert_eq!(binary_complete_state_projection(&typer, &parsed), before);
    assert_eq!(
        typer.source_typed_index().get(source, prior),
        Some(prior_typed)
    );
    assert!(typer.pattern_binding_symbol_at(source, peel_left).is_none());
    for tree in peel_adds.into_iter().chain([peel_left]) {
        assert!(typer.source_typed_index().get(source, tree).is_none());
    }
    let peel_retry = typer
        .type_method_definition(peel_failure)
        .expect_err("the same nested peelLeft failure retries");
    assert_eq!(format!("{peel_retry:?}"), peel_debug);
    assert_eq!(binary_complete_state_projection(&typer, &parsed), before);

    let sum_observed = Rc::new(RefCell::new(false));
    let sum_observed_by_callback = Rc::clone(&sum_observed);
    let sum_calls_by_callback = sum_calls.clone();
    let sum_adds_by_callback = sum_adds.clone();
    let sum_num_extractors_by_callback = sum_num_extractors.clone();
    typer.transaction_failure_observer = Some(Rc::new(move |typer, _error| {
        for extractor in sum_adds_by_callback
            .iter()
            .chain(sum_num_extractors_by_callback.iter())
        {
            let typed = typer
                .source_typed_index()
                .get(source, *extractor)
                .expect("nested Add and Num extractor trees exist before rollback");
            assert!(matches!(
                typer.typed_ast().get(typed).kind,
                TreeKind::UnApply(_)
            ));
        }
        let typed_calls = sum_calls_by_callback
            .iter()
            .map(|tree| typer.source_typed_index().get(source, *tree))
            .collect::<Vec<_>>();
        if typed_calls.iter().any(Option::is_none) {
            return;
        }
        let [first, second, third] = [
            typed_calls[0].unwrap(),
            typed_calls[1].unwrap(),
            typed_calls[2].unwrap(),
        ];
        let mut rights = Vec::new();
        for (position, (tree, typed)) in sum_calls_by_callback
            .iter()
            .copied()
            .zip([first, second, third])
            .enumerate()
        {
            let TreeKind::Apply(application) = &typer.typed_ast().get(typed).kind else {
                panic!("recursive call remains an Apply before rollback")
            };
            let callee = match typer
                .store()
                .types
                .get(typer.typed_ast().get(application.function).ty)
            {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("recursive call targets a canonical method: {other:?}"),
            };
            assert_eq!(callee, original_sum_symbol);
            let expected_binder = match position {
                0 => sum_right_binders[0],
                1 => sum_left_binder,
                _ => sum_right_binders[1],
            };
            let argument = application.args[0];
            let argument_symbol = match typer.store().types.get(typer.typed_ast().get(argument).ty)
            {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("recursive argument is a binder reference: {other:?}"),
            };
            let expected_symbol = typer
                .pattern_binding_symbol_at(source, expected_binder)
                .expect("recursive call's case binder exists before rollback");
            assert_eq!(argument_symbol, expected_symbol);
            assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
            if position != 1 {
                rights.push(expected_symbol);
            }
        }
        assert_ne!(
            rights[0], rights[1],
            "sibling right binders stay distinct before rollback"
        );
        let value = typer
            .pattern_binding_symbol_at(source, sum_value_binder)
            .expect("nested Num value binder exists before rollback");
        let SymbolInfo::Complete(value_type) = *typer.store().symbols.info(value) else {
            panic!("nested Num value binder has a completed type")
        };
        assert_eq!(
            type_symbol(typer.store(), value_type),
            type_symbol(typer.store(), definitions.int)
        );
        *sum_observed_by_callback.borrow_mut() = true;
    }));
    let sum_error = typer
        .type_method_definition(sum_failure)
        .expect_err("Int-valued recursive sum body mismatches Boolean result");
    assert!(
        matches!(sum_error, TyperError::ExpectedExpressionTypeMismatch { .. }),
        "{sum_error:?}"
    );
    assert!(
        *sum_observed.borrow(),
        "all three recursive Apply sites were observed before unwind"
    );
    let sum_debug = format!("{sum_error:?}");
    assert_eq!(binary_complete_state_projection(&typer, &parsed), before);
    assert_eq!(
        typer.source_typed_index().get(source, prior),
        Some(prior_typed)
    );
    for tree in sum_adds
        .into_iter()
        .chain(sum_calls)
        .chain(sum_num_extractors)
        .chain(sum_right_binders)
        .chain([sum_left_binder, sum_value_binder])
    {
        assert!(typer.source_typed_index().get(source, tree).is_none());
        assert!(typer.pattern_binding_symbol_at(source, tree).is_none());
    }
    let sum_retry = typer
        .type_method_definition(sum_failure)
        .expect_err("the same nested sum failure retries");
    assert_eq!(format!("{sum_retry:?}"), sum_debug);
    assert_eq!(binary_complete_state_projection(&typer, &parsed), before);

    let typed_peel = typer
        .type_method_definition(original_peel)
        .expect("the untouched original peelLeft recovers in the same live typer");
    let typed_sum = typer
        .type_method_definition(original_sum)
        .expect("the untouched original sum recovers in the same live typer");
    assert_eq!(
        typer.source_typed_index().get(source, original_peel),
        Some(typed_peel)
    );
    assert_eq!(
        typer.source_typed_index().get(source, original_sum),
        Some(typed_sum)
    );
    let recovered = format!(
        "{}\n{}",
        normalized_nested_root(&parsed, &typer, source, original_peel),
        normalized_nested_root(&parsed, &typer, source, original_sum)
    );

    let fresh_source = SourceId::from_index(21_362);
    let mut fresh_store = SemanticStore::new();
    let fresh_definitions = Definitions::bootstrap(&mut fresh_store);
    let mut fresh_packages = Packages::new();
    let (fresh_parsed, fresh_index) = parse_and_name_unit(
        &source_text,
        fresh_source,
        &mut fresh_store,
        &mut fresh_packages,
    );
    let fresh_prior = tree_named(&fresh_parsed, &fresh_store, "prior");
    let fresh_peel_failure = tree_named(&fresh_parsed, &fresh_store, "peelLeftFails");
    let fresh_sum_failure = tree_named(&fresh_parsed, &fresh_store, "sumFails");
    let fresh_original_peel = tree_named(&fresh_parsed, &fresh_store, "peelLeft");
    let fresh_original_sum = tree_named_before(
        &fresh_parsed,
        &fresh_store,
        "sum",
        NESTED_EXPR_SOURCE.len() as u32,
    );
    let mut fresh_typer = typer_for(
        &fresh_parsed,
        &mut fresh_store,
        &fresh_packages,
        fresh_definitions,
        &fresh_index,
        fresh_source,
    );
    fresh_typer
        .type_method_definition(fresh_prior)
        .expect("fresh session types prior root");
    for (root, expected) in [
        (fresh_peel_failure, normalized_error(&peel_error, &parsed)),
        (fresh_sum_failure, normalized_error(&sum_error, &parsed)),
    ] {
        let first = fresh_typer
            .type_method_definition(root)
            .expect_err("fresh session rejects late control");
        assert_eq!(normalized_error(&first, &fresh_parsed), expected);
        let retry = fresh_typer
            .type_method_definition(root)
            .expect_err("fresh late control retry fails");
        assert_eq!(format!("{retry:?}"), format!("{first:?}"));
    }
    fresh_typer
        .type_method_definition(fresh_original_peel)
        .expect("fresh peelLeft recovery");
    fresh_typer
        .type_method_definition(fresh_original_sum)
        .expect("fresh sum recovery");
    let fresh_recovered = format!(
        "{}\n{}",
        normalized_nested_root(
            &fresh_parsed,
            &fresh_typer,
            fresh_source,
            fresh_original_peel
        ),
        normalized_nested_root(
            &fresh_parsed,
            &fresh_typer,
            fresh_source,
            fresh_original_sum
        )
    );
    assert_eq!(
        recovered, fresh_recovered,
        "same-session recovery matches fresh parse/resolution"
    );
    let receipt = format!(
        "control\tsource_id\tfresh_source_id\tsource_bytes\troot_span\tdiagnostic\trecovery\npeelLeftFails\t{}\t{}\t{}\t{}\t{}\tfull_rollback_retry_same_session_and_fresh_parity\nsumFails\t{}\t{}\t{}\t{}\t{}\tfull_rollback_retry_same_session_and_fresh_parity",
        source.index(),
        fresh_source.index(),
        source_text.len(),
        tree_span(&parsed, peel_failure.index()),
        normalized_error(&peel_error, &parsed),
        source.index(),
        fresh_source.index(),
        source_text.len(),
        tree_span(&parsed, sum_failure.index()),
        normalized_error(&sum_error, &parsed),
    );
    assert_eq!(
        receipt,
        NESTED_LATE_FAILURE_RECEIPTS.trim_end(),
        "normalized late-failure evidence changed"
    );
    let tested_sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git is available for the execution-bound nested rollback receipt")
        .stdout;
    let tested_sha = String::from_utf8(tested_sha).expect("git SHA is UTF-8");
    let tested_sha = tested_sha.trim();
    assert!(
        tested_sha.len() == 40 && tested_sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "nested rollback evidence binds to the exact tested checkout"
    );
    println!("ISSUE2137_ROLLBACK_TESTED_SHA\t{tested_sha}");
    let source_suffix_hex = NESTED_ROLLBACK_CONTROL_SUFFIX
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    println!(
        "ISSUE2136_ROLLBACK_SOURCE\t{}\t{}\t{}\t{}\t{}",
        source.index(),
        fresh_source.index(),
        source_text.len(),
        rollback_owner_name,
        source_suffix_hex,
    );
    println!("ISSUE2136_NESTED_ROLLBACK\t{receipt}");
}

fn nested_extractors(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    root: TreeId<Untyped>,
    name: &str,
) -> Vec<TreeId<Untyped>> {
    let root_span = parsed.ast.get(root).position.unwrap().span().range();
    let mut trees = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let span = node.position?.span().range();
            if span.start() < root_span.start() || span.end() > root_span.end() {
                return None;
            }
            let TreeKind::Apply(application) = &node.kind else {
                return None;
            };
            let function_name = match &parsed.ast.get(application.function).kind {
                TreeKind::Ident(function) => store.names.resolve(function.name.text()),
                TreeKind::Select(function) => store.names.resolve(function.name.text()),
                _ => return None,
            };
            (function_name == name).then_some(tree)
        })
        .collect::<Vec<_>>();
    trees.sort_by_key(|tree| {
        parsed
            .ast
            .get(*tree)
            .position
            .unwrap()
            .span()
            .range()
            .start()
    });
    trees
}

fn tree_named_before(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    name: &str,
    source_offset: u32,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == name
                    && node.position.unwrap().span().range().end() <= source_offset =>
            {
                Some(tree)
            }
            _ => None,
        })
        .last()
        .unwrap_or_else(|| panic!("missing method {name} before source offset {source_offset}"))
}

fn nested_named_calls(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    root: TreeId<Untyped>,
    name: &str,
) -> Vec<TreeId<Untyped>> {
    let root_span = parsed.ast.get(root).position.unwrap().span().range();
    let mut trees = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let span = node.position?.span().range();
            if span.start() < root_span.start() || span.end() > root_span.end() {
                return None;
            }
            let TreeKind::Apply(application) = &node.kind else {
                return None;
            };
            let function_name = match &parsed.ast.get(application.function).kind {
                TreeKind::Ident(function) => store.names.resolve(function.name.text()),
                TreeKind::Select(function) => store.names.resolve(function.name.text()),
                _ => return None,
            };
            (function_name == name).then_some(tree)
        })
        .collect::<Vec<_>>();
    trees.sort_by_key(|tree| {
        parsed
            .ast
            .get(*tree)
            .position
            .unwrap()
            .span()
            .range()
            .start()
    });
    trees
}

fn normalized_nested_root(
    parsed: &dotty_parser::ParseResult,
    typer: &SourceTyper<'_>,
    source: SourceId,
    root: TreeId<Untyped>,
) -> String {
    let root_span = parsed.ast.get(root).position.unwrap().span().range();
    let mut rows = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let span = node.position?.span().range();
            if span.start() < root_span.start() || span.end() > root_span.end() {
                return None;
            }
            let typed = typer.source_typed_index().get(source, tree)?;
            let typed = typer.typed_ast().get(typed);
            Some(format!(
                "{}..{}:{}:{}",
                span.start(),
                span.end(),
                typed_kind_shape(typed),
                normalized_type_shape(typer.store(), typed.ty)
            ))
        })
        .collect::<Vec<_>>();
    rows.sort();
    format!(
        "{}@{}..{}\n{}",
        tree_span(parsed, root.index()),
        root_span.start(),
        root_span.end(),
        rows.join("\n")
    )
}

#[test]
fn binary_classified_source_controls_have_stable_retry_and_session_receipts() {
    let fixture_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/source-binary-recursive-adt-3.9.0");
    let scala_classifications = std::fs::read_to_string(fixture_dir.join("scala-matrix.tsv"))
        .expect("pinned Scala classification manifest");
    let classifications = scala_classifications
        .lines()
        .skip(1)
        .map(|line| {
            let (name, classification) = line.split_once('\t').expect("Scala matrix columns");
            (name.to_owned(), classification.to_owned())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let cases = BINARY_CONTROL_ROOTS
        .lines()
        .enumerate()
        .skip(1)
        .map(|(line, row)| {
            let columns = row.split('\t').collect::<Vec<_>>();
            assert_eq!(columns.len(), 2, "binary control roots row {}", line + 1);
            let (name, root) = (columns[0], columns[1]);
            BinarySourceControl {
                name: name.to_owned(),
                root: root.to_owned(),
                scala_result: classifications
                    .get(name)
                    .unwrap_or_else(|| panic!("Scala classification exists for {name}"))
                    .clone(),
                source: std::fs::read_to_string(fixture_dir.join(format!("{name}.scala")))
                    .unwrap_or_else(|error| panic!("{name} source fixture: {error}")),
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(cases.len(), classifications.len());
    let mut receipts = Vec::new();
    for (case_index, case) in cases.iter().enumerate() {
        let source = SourceId::from_index(20_800 + case_index as u32);
        let receipt = exercise_binary_source_control(case, source);
        println!("ISSUE2079_BINARY_CONTROL\t{receipt}");
        receipts.push(receipt);
    }
    assert_eq!(
        receipts.len(),
        22,
        "the pinned Scala control inventory is stable"
    );
    let header = BINARY_CONTROL_RECEIPTS
        .lines()
        .next()
        .expect("binary control receipt header");
    assert_eq!(
        format!("{header}\n{}\n", receipts.join("\n")),
        BINARY_CONTROL_RECEIPTS
    );
}

struct BinarySourceControl {
    name: String,
    root: String,
    scala_result: String,
    source: String,
}

fn exercise_binary_source_control(case: &BinarySourceControl, source: SourceId) -> String {
    let exercise = |source| {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let (parsed, index) = parse_and_name_unit(&case.source, source, &mut store, &mut packages);
        let root = tree_named(&parsed, &store, &case.root);
        let mut typer = typer_for(&parsed, &mut store, &packages, definitions, &index, source);
        let before = binary_complete_state_projection(&typer, &parsed);
        let (outcome, first, retry) = match typer.type_method_definition(root) {
            Ok(typed) => {
                let shape = normalized_control_success(&parsed, &typer, source, root, &case.root);
                assert_eq!(
                    typer.source_typed_index().get(source, root),
                    Some(typed),
                    "accepted root publishes its source mapping for {}",
                    case.name
                );
                let retry = typer
                    .type_method_definition(root)
                    .expect("accepted source control root remains retryable");
                assert_eq!(retry, typed, "{} retry reuses its typed root", case.name);
                ("accepted", shape, "accepted_same_root".to_owned())
            }
            Err(error) => {
                let first = normalized_error(&error, &parsed);
                assert_eq!(
                    binary_complete_state_projection(&typer, &parsed),
                    before,
                    "{} first failure restores its complete state",
                    case.name
                );
                let retry = typer
                    .type_method_definition(root)
                    .expect_err("repeated source control failure remains rejected");
                assert_eq!(
                    normalized_error(&retry, &parsed),
                    first,
                    "{} retry",
                    case.name
                );
                assert_eq!(
                    binary_complete_state_projection(&typer, &parsed),
                    before,
                    "{} retry restores its complete state",
                    case.name
                );
                ("deferred", first, "rejected_same_error".to_owned())
            }
        };
        let root_span = tree_span(&parsed, root.index());
        let first = if outcome == "accepted" {
            format!("accepted:{root_span}:{first}")
        } else {
            first
        };
        let rendered = format!("{outcome}\t{root_span}\t{first}\t{retry}");
        (rendered, outcome)
    };

    let (primary, outcome) = exercise(source);
    let fresh_source = SourceId::from_index(source.index() + 1_000);
    let (fresh, fresh_outcome) = exercise(fresh_source);
    assert_eq!(
        outcome, fresh_outcome,
        "fresh Rust outcome for {}",
        case.name
    );
    let primary_semantics = primary.split('\t').nth(2).unwrap();
    let fresh_semantics = fresh.split('\t').nth(2).unwrap();
    assert_eq!(
        primary_semantics, fresh_semantics,
        "fresh normalized receipt for {}",
        case.name
    );
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        case.name, case.root, case.scala_result, outcome, primary, fresh
    )
}

fn normalized_control_success(
    parsed: &dotty_parser::ParseResult,
    typer: &SourceTyper<'_>,
    source: SourceId,
    root: TreeId<Untyped>,
    method_name: &str,
) -> String {
    let TreeKind::DefDef(definition) = &parsed.ast.get(root).kind else {
        unreachable!()
    };
    let rhs = definition.rhs.expect("accepted control has a body");
    let TreeKind::Match(matched) = &parsed.ast.get(rhs).kind else {
        panic!("accepted source control keeps its Match")
    };
    let span = parsed.ast.get(root).position.unwrap().span().range();
    typer
        .source_typed_index()
        .get(source, root)
        .expect("accepted root has a source mapping");
    let store = typer.store();
    let method_symbol = typer
        .index
        .symbol_at(source, root)
        .expect("accepted method has its source symbol");
    let SymbolInfo::Complete(method_type) = *store.symbols.info(method_symbol) else {
        panic!("accepted method signature is complete")
    };
    let mut binders = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let node_span = node.position?.span().range();
            if node_span.start() < span.start() || node_span.end() > span.end() {
                return None;
            }
            let symbol = typer.pattern_binding_symbol_at(source, tree)?;
            let name = match &node.kind {
                TreeKind::Ident(ident) => store.names.resolve(ident.name.text()),
                TreeKind::Bind(binding) => store.names.resolve(binding.name.text()),
                _ => return None,
            };
            let SymbolInfo::Complete(ty) = *store.symbols.info(symbol) else {
                panic!("accepted pattern binder has a complete type")
            };
            Some(format!(
                "{name}:{}:{}",
                symbol_path(store, symbol),
                normalized_type_shape(store, ty)
            ))
        })
        .collect::<Vec<_>>();
    binders.sort();
    let mut calls = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let node_span = node.position?.span().range();
            if node_span.start() < span.start() || node_span.end() > span.end() {
                return None;
            }
            let TreeKind::Apply(application) = &node.kind else {
                return None;
            };
            let typed_call = typer.source_typed_index().get(source, tree)?;
            let TreeKind::Apply(typed_application) = &typer.typed_ast().get(typed_call).kind else {
                return None;
            };
            let callee = match store
                .types
                .get(typer.typed_ast().get(typed_application.function).ty)
            {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => symbol_path(store, *symbol),
                other => format!("unresolved:{other:?}"),
            };
            let argument = application
                .args
                .first()
                .and_then(|argument| {
                    let typed_argument = typer.source_typed_index().get(source, *argument)?;
                    match store.types.get(typer.typed_ast().get(typed_argument).ty) {
                        Type::TermRef {
                            target: TermRefTarget::Symbol(symbol),
                            ..
                        } => Some(symbol_path(store, *symbol)),
                        _ => Some(normalized_type_shape(
                            store,
                            typer.typed_ast().get(typed_argument).ty,
                        )),
                    }
                })
                .unwrap_or_else(|| "no-argument".to_owned());
            Some(format!(
                "{}..{}:{callee}({argument})",
                node_span.start(),
                node_span.end()
            ))
        })
        .collect::<Vec<_>>();
    calls.sort();
    format!(
        "{}@{}..{};owner={};signature={};cases={}/{};binders=[{}];calls=[{}]",
        method_name,
        span.start(),
        span.end(),
        symbol_path(store, method_symbol),
        normalized_type_shape(store, method_type),
        matched.cases.len(),
        matched
            .cases
            .iter()
            .filter(|case| typer.source_typed_index().get(source, **case).is_some())
            .count(),
        binders.join(","),
        calls.join(",")
    )
}

fn binary_add_case_nodes(
    parsed: &dotty_parser::ParseResult,
    root: TreeId<Untyped>,
) -> BinaryAddCaseNodes {
    let TreeKind::DefDef(definition) = &parsed.ast.get(root).kind else {
        unreachable!()
    };
    let TreeKind::Match(matched) = &parsed.ast.get(definition.rhs.unwrap()).kind else {
        panic!("binary failure control retains its source Match")
    };
    let TreeKind::CaseDef(add_case) = &parsed.ast.get(matched.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::Apply(extractor) = &parsed.ast.get(add_case.pattern).kind else {
        panic!("Add case has two source binders")
    };
    assert_eq!(extractor.args.len(), 2);
    let body = match &parsed.ast.get(add_case.body).kind {
        TreeKind::Block(block) => block.expr,
        _ => add_case.body,
    };
    let TreeKind::PhaseSpecific(UntypedNode::InfixOp(addition)) = &parsed.ast.get(body).kind else {
        panic!("Add body contains its two recursive calls")
    };
    let calls = [addition.left, addition.right];
    let arguments = calls.map(|call| {
        let TreeKind::Apply(application) = &parsed.ast.get(call).kind else {
            panic!("Add operand is a recursive Apply")
        };
        assert_eq!(application.args.len(), 1);
        application.args[0]
    });
    BinaryAddCaseNodes {
        binders: [extractor.args[0], extractor.args[1]],
        calls,
        arguments,
    }
}

fn normalized_binary_eval_recovery(
    parsed: &dotty_parser::ParseResult,
    typer: &SourceTyper<'_>,
    source: SourceId,
    root: TreeId<Untyped>,
) -> String {
    let store = typer.store();
    let nodes = binary_add_case_nodes(parsed, root);
    let eval_symbol = typer
        .index
        .symbol_at(source, root)
        .expect("recovered eval has its canonical source symbol");
    let expr_symbol = class_symbol(parsed, store, typer.index, source, "Expr");
    let mut binder_symbols = Vec::new();
    let mut binder_receipts = Vec::new();
    for binder_tree in nodes.binders {
        let binder_symbol = typer
            .pattern_binding_symbol_at(source, binder_tree)
            .expect("recovered binary Add binder is published");
        let SymbolInfo::Complete(binder_type) = *store.symbols.info(binder_symbol) else {
            panic!("recovered Add binder has a complete type")
        };
        assert_eq!(type_symbol(store, binder_type), expr_symbol);
        let name = match &parsed.ast.get(binder_tree).kind {
            TreeKind::Ident(ident) => store.names.resolve(ident.name.text()),
            TreeKind::Bind(binding) => store.names.resolve(binding.name.text()),
            other => panic!("recovered Add binder is a named pattern: {other:?}"),
        };
        binder_symbols.push(binder_symbol);
        binder_receipts.push(format!(
            "{name}:{}:{}",
            symbol_path(store, binder_symbol),
            normalized_type_shape(store, binder_type)
        ));
    }
    assert_ne!(binder_symbols[0], binder_symbols[1]);

    let typed_calls = nodes.calls.map(|call| {
        typer
            .source_typed_index()
            .get(source, call)
            .expect("both recovered recursive calls have source mappings")
    });
    assert_ne!(typed_calls[0], typed_calls[1]);
    let mut call_receipts = Vec::new();
    let mut typed_arguments = Vec::new();
    for (position, ((source_call, source_argument), typed_call)) in nodes
        .calls
        .into_iter()
        .zip(nodes.arguments)
        .zip(typed_calls)
        .enumerate()
    {
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed_call).kind else {
            panic!("recovered recursive call is a typed Apply")
        };
        assert_eq!(application.args.len(), 1);
        let selected_callee = match store
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("recovered recursive callee is canonical: {other:?}"),
        };
        assert_eq!(selected_callee, eval_symbol);
        let typed_argument = application.args[0];
        assert_eq!(
            typer.source_typed_index().get(source, source_call),
            Some(typed_call)
        );
        assert_eq!(
            typer.source_typed_index().get(source, source_argument),
            Some(typed_argument),
            "each recovered call maps its own original source argument"
        );
        assert_ne!(typed_argument, application.function);
        let Type::TermRef {
            target: TermRefTarget::Symbol(argument_symbol),
            ..
        } = store.types.get(typer.typed_ast().get(typed_argument).ty)
        else {
            panic!("each recovered recursive argument is its original Add binder")
        };
        assert_eq!(*argument_symbol, binder_symbols[position]);
        typed_arguments.push(typed_argument);
        let call_span = tree_span(parsed, source_call.index());
        call_receipts.push(format!(
            "{call_span}:{}({})",
            symbol_path(store, selected_callee),
            symbol_path(store, *argument_symbol)
        ));
    }
    assert_ne!(typed_arguments[0], typed_arguments[1]);
    format!(
        "eval-binary;binders=[{}];calls=[{}]",
        binder_receipts.join(","),
        call_receipts.join(",")
    )
}
