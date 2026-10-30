use super::*;

struct Root {
    path: &'static str,
    owner: &'static str,
    source_root: &'static str,
    method: &'static str,
    source: &'static str,
    captures: &'static [&'static str],
    result_shape: &'static str,
}

const IDENTITY: &str = "class C { def identity: Int => Int = x => x }";

const ROOTS: [Root; 6] = [
    Root {
        path: "strict-class-C",
        owner: "C",
        source_root: "class C",
        method: "identity",
        source: IDENTITY,
        captures: &[],
        result_shape: "Function1[Int,Int]",
    },
    Root {
        path: "lexical-closure-contract/MethodParameters.scala",
        owner: "MethodParameters$",
        source_root: "package lexicalclosure / object MethodParameters",
        method: "addBy",
        source: include_str!(
            "../../../tests/fixtures/lexical-closure-contract/MethodParameters.scala"
        ),
        captures: &["delta"],
        result_shape: "Function1[Int,Int]",
    },
    Root {
        path: "lexical-closure-contract/LocalValues.scala",
        owner: "LocalValues$",
        source_root: "package lexicalclosure / object LocalValues",
        method: "offset",
        source: include_str!("../../../tests/fixtures/lexical-closure-contract/LocalValues.scala"),
        captures: &["bias"],
        result_shape: "Function1[Int,Int]",
    },
    Root {
        path: "lexical-closure-contract/ThisCapture.scala",
        owner: "Outer",
        source_root: "class Outer",
        method: "self",
        source: include_str!("../../../tests/fixtures/lexical-closure-contract/ThisCapture.scala"),
        captures: &["Outer.this"],
        result_shape: "Function1[Int,Outer]",
    },
    Root {
        path: "lexical-closure-contract/SameOwnerMembers.scala",
        owner: "Calculator",
        source_root: "class Calculator",
        method: "lifted",
        source: include_str!(
            "../../../tests/fixtures/lexical-closure-contract/SameOwnerMembers.scala"
        ),
        captures: &["Calculator.this"],
        result_shape: "Function1[Int,Int]",
    },
    Root {
        path: "lexical-closure-contract/CombinedCaptures.scala",
        owner: "Calculator",
        source_root: "package lexicalclosure / class Calculator",
        method: "makeAdder",
        source: include_str!(
            "../../../tests/fixtures/lexical-closure-contract/CombinedCaptures.scala"
        ),
        captures: &["Calculator.this", "delta"],
        result_shape: "Function1[Int,Int]",
    },
];

fn source_method_tree(
    parsed: &dotty_parser::ParseResult,
    index: &SourceSemanticIndex,
    source: SourceId,
    method: SymbolId,
) -> TreeId<Untyped> {
    let matches: Vec<_> = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            (index.symbol_at(source, tree) == Some(method)
                && matches!(node.kind, TreeKind::DefDef(_)))
            .then_some(tree)
        })
        .collect();
    assert_eq!(matches.len(), 1, "source method must have one DefDef root");
    matches[0]
}

fn owner_name(store: &SemanticStore, symbol: SymbolId) -> String {
    store
        .names
        .resolve(store.symbols.get(symbol).name.text())
        .to_owned()
}

fn one_fresh_session(root: &Root) -> String {
    // Every call assembles a new lexer/parser/namer session and allocates new
    // semantic and typed arenas. The caller intentionally invokes this twice.
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(root.source);
    enter_scala_function_classes(&mut store, &mut packages, [1]);

    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, root.method);
    let method_owner = store.symbols.get(method).owner.expect("method owner");
    assert_eq!(
        owner_name(&store, method_owner),
        root.owner,
        "{} owner",
        root.path
    );
    let method_tree = source_method_tree(&parsed, &index, source, method);
    let (lambda_tree, lambda_parameter_tree, lambda_body_tree) = match &parsed.ast.get(rhs).kind {
        TreeKind::PhaseSpecific(UntypedNode::Function(function)) => {
            (rhs, function.params[0], function.body)
        }
        TreeKind::Block(block) => match &parsed.ast.get(block.expr).kind {
            TreeKind::PhaseSpecific(UntypedNode::Function(function)) => {
                (block.expr, function.params[0], function.body)
            }
            other => panic!(
                "{} should end in a function literal, got {other:?}",
                root.path
            ),
        },
        other => panic!(
            "{} should contain a function literal, got {other:?}",
            root.path
        ),
    };
    let method_parameter = match &parsed.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) if !definition.value_param_clauses.is_empty() => definition
            .value_param_clauses[0]
            .first()
            .and_then(|tree| index.symbol_at(source, *tree)),
        TreeKind::DefDef(_) => None,
        _ => unreachable!(),
    };

    let method_span = parsed.ast.get(method_tree).position.expect("method span");
    let rhs_span = parsed.ast.get(rhs).position.expect("method RHS span");
    let lambda_span = parsed.ast.get(lambda_tree).position.expect("lambda span");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let typed_method = typer
        .type_method_definition(method_tree)
        .unwrap_or_else(|error| panic!("{}#{} failed: {error:?}", root.path, root.method));
    let typed_definition = match &typer.typed_ast().get(typed_method).kind {
        TreeKind::DefDef(definition) => definition,
        other => panic!("typed method root must be DefDef, got {other:?}"),
    };
    assert!(typed_definition.rhs.is_some(), "complete method RHS");
    assert_eq!(
        typer.source_typed_index().get(source, method_tree),
        Some(typed_method)
    );
    assert_eq!(
        typer.typed_ast().get(typed_method).position,
        Some(method_span)
    );
    assert!(typer.source_typed_index().get(source, rhs).is_some());
    let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
    assert_eq!(typer.typed_ast().get(typed_rhs).position, Some(rhs_span));
    let typed_lambda = typer.source_typed_index().get(source, lambda_tree).unwrap();
    assert_eq!(
        typer.typed_ast().get(typed_lambda).position,
        Some(lambda_span)
    );

    let SymbolInfo::Complete(method_type) = typer.store().symbols.info(method) else {
        panic!("source method must have a completed signature");
    };
    let (method_parameters, method_result) = match typer.store().types.get(*method_type) {
        Type::Method(signature) => (signature.params.as_slice(), signature.result),
        // Nullary source methods are represented by their completed result
        // type directly in the shared semantic model.
        _ if method_parameter.is_none() => (&[][..], *method_type),
        other => panic!("parameterized source method signature must be Method, got {other:?}"),
    };
    let result_shape = match typer.store().types.get(method_result) {
        Type::Applied { tycon, args } => {
            let function_name = match typer.store().types.get(*tycon) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } => owner_name(typer.store(), *symbol),
                other => panic!("Function1 result must have a canonical type symbol: {other:?}"),
            };
            assert_eq!(function_name, "Function1");
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], definitions.int);
            if root.result_shape == "Function1[Int,Outer]" {
                let Type::TypeRef {
                    target: TypeRefTarget::Symbol(result_class),
                    ..
                } = typer.store().types.get(args[1])
                else {
                    panic!("Outer result should retain its canonical class symbol");
                };
                assert_eq!(owner_name(typer.store(), *result_class), "Outer");
            } else {
                assert_eq!(args[1], definitions.int);
            }
            root.result_shape
        }
        other => panic!("expected Function1 result shape, got {other:?}"),
    };
    assert_eq!(
        method_parameters.len(),
        usize::from(method_parameter.is_some())
    );
    if let Some(parameter) = method_parameter {
        assert_eq!(typer.store().symbols.get(parameter).owner, Some(method));
        assert_eq!(
            typer.store().symbols.get(parameter).kind,
            SymbolKind::Parameter
        );
        assert_eq!(method_parameters[0].ty, definitions.int);
    }
    let typed_result = typer.typed_ast().get(typed_rhs).ty;
    let (
        Type::Applied {
            tycon: signature_tycon,
            args: signature_args,
        },
        Type::Applied {
            tycon: rhs_tycon,
            args: rhs_args,
        },
    ) = (
        typer.store().types.get(method_result),
        typer.store().types.get(typed_result),
    )
    else {
        panic!("declared and typed results must both be Function1 applications");
    };
    assert_eq!(signature_args, rhs_args);
    let constructor_symbol = |tycon: &TypeId| match typer.store().types.get(*tycon) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("Function1 constructor must keep its canonical symbol: {other:?}"),
    };
    assert_eq!(
        constructor_symbol(signature_tycon),
        constructor_symbol(rhs_tycon)
    );

    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_lambda).kind else {
        panic!("typed lambda must be Closure");
    };
    assert_eq!(closure.env.len(), root.captures.len(), "capture count");
    let lambda_method = typer
        .function_literal_method_at(source, lambda_tree)
        .unwrap();
    assert_eq!(typer.store().symbols.get(lambda_method).owner, Some(method));
    let lambda_definition_tree = typer
        .function_literal_definition_at(source, lambda_tree)
        .unwrap();
    let TreeKind::DefDef(lambda_definition) = &typer.typed_ast().get(lambda_definition_tree).kind
    else {
        panic!("synthetic lambda method must be a complete typed DefDef");
    };
    let typed_lambda_body = typer
        .source_typed_index()
        .get(source, lambda_body_tree)
        .unwrap();
    assert_eq!(lambda_definition.rhs, Some(typed_lambda_body));
    assert_eq!(
        typer.typed_ast().get(typed_lambda_body).position,
        parsed.ast.get(lambda_body_tree).position
    );
    let rhs_range = rhs_span.span().range();
    let mut mapped_semantic_nodes = 0;
    for (source_tree, source_node) in parsed.ast.iter() {
        let Some(span) = source_node.position else {
            continue;
        };
        let range = span.span().range();
        if span.source() != source
            || range.start() < rhs_range.start()
            || range.end() > rhs_range.end()
            || !matches!(
                source_node.kind,
                TreeKind::Ident(_) | TreeKind::Apply(_) | TreeKind::Select(_) | TreeKind::This(_)
            )
        {
            continue;
        }
        let typed = typer
            .source_typed_index()
            .get(source, source_tree)
            .unwrap_or_else(|| {
                panic!(
                    "unmapped source semantic node in {}#{}",
                    root.path, root.method
                )
            });
        assert_eq!(typer.typed_ast().get(typed).position, Some(span));
        mapped_semantic_nodes += 1;
    }
    assert!(
        mapped_semantic_nodes > 0,
        "method body should map identifiers/calls"
    );
    let lambda_parameter = typer
        .function_literal_parameter_symbol_at(source, lambda_parameter_tree)
        .expect("source lambda parameter identity");
    assert_eq!(
        typer.store().symbols.get(lambda_parameter).owner,
        Some(lambda_method)
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, lambda_parameter_tree)
            .is_some()
    );
    let SymbolInfo::Complete(lambda_type) = typer.store().symbols.info(lambda_method) else {
        panic!("synthetic lambda signature must be complete");
    };
    let Type::Method(lambda_signature) = typer.store().types.get(*lambda_type) else {
        panic!("synthetic lambda signature must be Method");
    };
    assert_eq!(lambda_signature.params.len(), 1);
    assert_eq!(lambda_signature.params[0].ty, definitions.int);
    if root.result_shape == "Function1[Int,Outer]" {
        let Type::TypeRef {
            target: TypeRefTarget::Symbol(result_class),
            ..
        } = typer.store().types.get(lambda_signature.result)
        else {
            panic!("Outer lambda result should keep its canonical class symbol")
        };
        assert_eq!(owner_name(typer.store(), *result_class), "Outer");
    } else {
        assert_eq!(lambda_signature.result, definitions.int);
    }

    let lambda_x_uses: Vec<_> = parsed
        .ast
        .iter()
        .filter_map(|(source_tree, source_node)| {
            let TreeKind::Ident(identifier) = &source_node.kind else {
                return None;
            };
            (typer.store().names.resolve(identifier.name.text()) == "x")
                .then(|| typer.source_typed_index().get(source, source_tree))
                .flatten()
        })
        .collect();
    assert_eq!(!lambda_x_uses.is_empty(), root.method != "self");
    for typed_use in &lambda_x_uses {
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(*typed_use).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == lambda_parameter
        ));
    }

    let member_method_name = match root.method {
        "lifted" => Some("adjust"),
        "makeAdder" => Some("inc"),
        _ => None,
    };
    if let Some(member_name) = member_method_name {
        let (member, _) =
            method_definition_and_rhs(&parsed, typer.store(), &index, source, member_name);
        assert_eq!(typer.store().symbols.get(member).owner, Some(method_owner));
        assert_eq!(typer.store().symbols.get(member).kind, SymbolKind::Method);
        assert!(matches!(
            typer.store().symbols.info(member),
            SymbolInfo::Complete(_)
        ));
        assert!(
            parsed.ast.iter().any(|(source_tree, source_node)| {
                let TreeKind::Ident(identifier) = &source_node.kind else {
                    return false;
                };
                if typer.store().names.resolve(identifier.name.text()) != member_name {
                    return false;
                }
                let Some(typed) = typer.source_typed_index().get(source, source_tree) else {
                    return false;
                };
                matches!(
                    typer.store().types.get(typer.typed_ast().get(typed).ty),
                    Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == member
                )
            }),
            "{} call must retain its canonical {} method identity",
            root.method,
            member_name
        );
    }

    let mut capture_names = Vec::new();
    let mut capture_targets = Vec::new();
    for (capture, expected) in closure.env.iter().zip(root.captures) {
        let tree = typer.typed_ast().get(*capture);
        match typer.store().types.get(tree.ty) {
            Type::ThisType { class } => {
                assert!(matches!(tree.kind, TreeKind::This(_)));
                assert_eq!(
                    owner_name(typer.store(), *class),
                    expected.trim_end_matches(".this")
                );
                capture_names.push(format!("{}.this", owner_name(typer.store(), *class)));
                capture_targets.push(None);
            }
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => {
                assert!(matches!(tree.kind, TreeKind::Ident(_)));
                assert_eq!(owner_name(typer.store(), *symbol), *expected);
                let expected_kind = if *expected == "bias" {
                    SymbolKind::Local
                } else {
                    SymbolKind::Parameter
                };
                assert_eq!(typer.store().symbols.get(*symbol).kind, expected_kind);
                assert_eq!(typer.store().symbols.get(*symbol).owner, Some(method));
                assert_eq!(
                    typer.store().symbols.info(*symbol),
                    &SymbolInfo::Complete(definitions.int),
                    "captured source identity retains its declared Int type"
                );
                let source_use = parsed.ast.iter().find_map(|(source_tree, source_node)| {
                    let TreeKind::Ident(identifier) = &source_node.kind else {
                        return None;
                    };
                    if typer.store().names.resolve(identifier.name.text()) != *expected {
                        return None;
                    }
                    let typed = typer.source_typed_index().get(source, source_tree)?;
                    matches!(
                        typer.store().types.get(typer.typed_ast().get(typed).ty),
                        Type::TermRef { target: TermRefTarget::Symbol(target), .. }
                            if *target == *symbol
                    )
                    .then_some(typed)
                });
                assert_eq!(
                    source_use,
                    Some(*capture),
                    "capture maps from its canonical source use"
                );
                capture_names.push(owner_name(typer.store(), *symbol));
                capture_targets.push(Some(*symbol));
            }
            other => panic!("capture must retain canonical identity and type: {other:?}"),
        }
    }
    assert_eq!(capture_names, root.captures);
    assert!(
        !capture_targets.contains(&Some(lambda_parameter)),
        "lambda x is never captured"
    );
    assert_eq!(
        capture_targets
            .iter()
            .filter(|target| method_parameter.is_some() && **target == method_parameter)
            .count(),
        root.captures
            .iter()
            .filter(|name| **name == "delta")
            .count()
    );

    if root.method == "offset" {
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!("offset must preserve its local initialization block");
        };
        assert_eq!(block.stats.len(), 1);
        let binding = typer
            .typed_local_binding_at(source, block.stats[0])
            .unwrap();
        assert_eq!(
            typer.source_typed_index().get(source, block.stats[0]),
            Some(binding.declaration)
        );
        let TreeKind::ValDef(source_value) = &parsed.ast.get(block.stats[0]).kind else {
            unreachable!()
        };
        let TreeKind::ValDef(initialized) = &typer.typed_ast().get(binding.declaration).kind else {
            panic!("captured local must retain its typed ValDef initialization");
        };
        assert_eq!(
            initialized.rhs,
            source_value.rhs.and_then(|source_initializer| typer
                .source_typed_index()
                .get(source, source_initializer))
        );
        assert!(initialized.rhs.is_some());
    }

    let mapping_ok = true;
    format!(
        "owner={};method={};result={result_shape};captures={};mapping={mapping_ok};span=preserved",
        root.owner,
        root.method,
        capture_names.join(",")
    )
}

#[test]
fn six_frozen_roots_type_as_complete_methods_in_two_independent_sessions() {
    let mut report = String::from(
        "source_path\towner\tmethod\tsource_root\tstatus\tresult_shape\tcapture_identity_list\tmapping_ok\tsession_1\tsession_2\n",
    );
    assert_eq!(
        ROOTS.len(),
        6,
        "the acceptance denominator is frozen at six"
    );
    for root in ROOTS.iter() {
        let first = one_fresh_session(root);
        let second = one_fresh_session(root);
        assert_eq!(
            first, second,
            "{} semantic identity differs across fresh sessions",
            root.path
        );
        let expected_captures = root.captures.join(",");
        report.push_str(&format!(
            "{}\t{}\t{}\t{}\taccepted\t{}\t[{}]\ttrue\t{}\t{}\n",
            root.path,
            root.owner,
            root.method,
            root.source_root,
            root.result_shape,
            expected_captures,
            first,
            second,
        ));
    }
    assert_eq!(report.lines().count(), 7, "all six rows must be present");
    assert_eq!(
        report,
        include_str!("../../../tests/fixtures/lexical-closure-contract/six-root-gate.tsv"),
        "deterministic gate report changed"
    );
}
