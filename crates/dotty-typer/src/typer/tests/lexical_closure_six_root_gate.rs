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

#[derive(Debug, PartialEq, Eq)]
struct SessionReceipt {
    owner: String,
    method: String,
    result_shape: String,
    captures: Vec<String>,
    mapping_ok: bool,
    canonical_graph: String,
}

impl SessionReceipt {
    fn normalized(&self) -> String {
        format!(
            "owner={};method={};result={};captures=[{}];mapping={};{}",
            self.owner,
            self.method,
            self.result_shape,
            self.captures.join(","),
            self.mapping_ok,
            self.canonical_graph
        )
    }
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

fn symbol_path(store: &SemanticStore, symbol: SymbolId) -> String {
    let info = store.symbols.get(symbol);
    let name = normalized_symbol_name(store.names.resolve(info.name.text()));
    let kind = format!("{:?}", info.kind);
    match info.owner {
        Some(owner) => format!("{}/{}:{}", symbol_path(store, owner), kind, name),
        None => format!("{}:{}", kind, name),
    }
}

fn normalized_symbol_name(name: &str) -> String {
    if name.starts_with("$sourceLambda$") {
        "$sourceLambda".to_owned()
    } else if name.starts_with("$lambda_wildcard_") {
        "$lambda_wildcard".to_owned()
    } else {
        name.to_owned()
    }
}

fn simple_type_name(store: &SemanticStore, ty: TypeId) -> String {
    match store.types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => owner_name(store, *symbol),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            simple_type_name(store, *tycon),
            args.iter()
                .map(|arg| simple_type_name(store, *arg))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => panic!("unexpected result type in frozen closure gate: {other:?}"),
    }
}

fn type_shape(store: &SemanticStore, ty: TypeId) -> String {
    match store.types.get(ty) {
        Type::NoType => "NoType".to_owned(),
        Type::NoPrefix => "NoPrefix".to_owned(),
        Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => format!(
            "TypeRef({};{})",
            type_shape(store, *prefix),
            symbol_path(store, *symbol)
        ),
        Type::TypeRef {
            prefix,
            target: TypeRefTarget::Name(name),
            ..
        } => format!(
            "TypeRef({};name:{})",
            type_shape(store, *prefix),
            store.names.resolve(name.as_name().text())
        ),
        Type::TermRef {
            prefix,
            target: TermRefTarget::Symbol(symbol),
            ..
        } => format!(
            "TermRef({};{})",
            type_shape(store, *prefix),
            symbol_path(store, *symbol)
        ),
        Type::TermRef {
            prefix,
            target: TermRefTarget::Name(name),
            ..
        } => format!(
            "TermRef({};name:{})",
            type_shape(store, *prefix),
            store.names.resolve(name.as_name().text())
        ),
        Type::ThisType { class } => format!("ThisType({})", symbol_path(store, *class)),
        Type::Constant(value) => format!("Constant({value:?})"),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            type_shape(store, *tycon),
            args.iter()
                .map(|arg| type_shape(store, *arg))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::Method(method) => format!(
            "Method({})->{}",
            method
                .params
                .iter()
                .map(|parameter| type_shape(store, parameter.ty))
                .collect::<Vec<_>>()
                .join(","),
            type_shape(store, method.result)
        ),
        other => panic!("unexpected type in frozen closure gate: {other:?}"),
    }
}

fn typed_tree_shape(arena: &AstArena<Typed>, store: &SemanticStore, tree: TreeId<Typed>) -> String {
    let node = arena.get(tree);
    let ty = type_shape(store, node.ty);
    match &node.kind {
        TreeKind::Ident(identifier) => format!(
            "Ident({};{} )",
            normalized_symbol_name(store.names.resolve(identifier.name.text())),
            ty
        ),
        TreeKind::This(_) => format!("This({ty})"),
        TreeKind::Literal(literal) => format!("Literal({:?};{ty})", literal.value),
        TreeKind::Select(selection) => format!(
            "Select({},{};{})",
            typed_tree_shape(arena, store, selection.qualifier),
            normalized_symbol_name(store.names.resolve(selection.name.text())),
            ty
        ),
        TreeKind::Apply(application) => format!(
            "Apply({};[{}];{})",
            typed_tree_shape(arena, store, application.function),
            application
                .args
                .iter()
                .map(|arg| typed_tree_shape(arena, store, *arg))
                .collect::<Vec<_>>()
                .join(","),
            ty
        ),
        TreeKind::Block(block) => format!(
            "Block(stats=[{}];expr={};{})",
            block
                .stats
                .iter()
                .map(|stat| typed_tree_shape(arena, store, *stat))
                .collect::<Vec<_>>()
                .join(","),
            typed_tree_shape(arena, store, block.expr),
            ty
        ),
        TreeKind::Closure(closure) => format!(
            "Closure(env=[{}];method={};tpt={};{})",
            closure
                .env
                .iter()
                .map(|capture| typed_tree_shape(arena, store, *capture))
                .collect::<Vec<_>>()
                .join(","),
            typed_tree_shape(arena, store, closure.method),
            closure
                .tpt
                .map(|tpt| typed_tree_shape(arena, store, tpt))
                .unwrap_or_else(|| "none".to_owned()),
            ty
        ),
        TreeKind::ValDef(value) => format!(
            "ValDef({};rhs={};{})",
            normalized_symbol_name(store.names.resolve(value.name.as_name().text())),
            value
                .rhs
                .map(|rhs| typed_tree_shape(arena, store, rhs))
                .unwrap_or_else(|| "none".to_owned()),
            ty
        ),
        TreeKind::DefDef(definition) => format!(
            "DefDef({};params=[{}];rhs={};{})",
            normalized_symbol_name(store.names.resolve(definition.name.as_name().text())),
            definition
                .value_param_clauses
                .iter()
                .map(|clause| clause
                    .iter()
                    .map(|parameter| typed_tree_shape(arena, store, *parameter))
                    .collect::<Vec<_>>()
                    .join(","))
                .collect::<Vec<_>>()
                .join(";"),
            definition
                .rhs
                .map(|rhs| typed_tree_shape(arena, store, rhs))
                .unwrap_or_else(|| "none".to_owned()),
            ty
        ),
        other => panic!("unexpected tree in frozen closure gate: {other:?}"),
    }
}

fn one_fresh_session(root: &Root) -> SessionReceipt {
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
    let Type::TermRef {
        target: TermRefTarget::Symbol(typed_method_symbol),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_method).ty)
    else {
        panic!("typed DefDef type must reference its canonical source method");
    };
    assert_eq!(*typed_method_symbol, method);
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
    assert_eq!(
        typed_definition.rhs,
        Some(typed_rhs),
        "returned typed DefDef must own its mapped RHS"
    );
    assert_eq!(typer.typed_ast().get(typed_rhs).position, Some(rhs_span));
    let typed_lambda = typer.source_typed_index().get(source, lambda_tree).unwrap();
    assert_eq!(
        typer.typed_ast().get(typed_lambda).position,
        Some(lambda_span)
    );
    match &typer.typed_ast().get(typed_rhs).kind {
        TreeKind::Closure(_) => assert_eq!(typed_rhs, typed_lambda),
        TreeKind::Block(block) => assert_eq!(
            block.expr, typed_lambda,
            "typed method block must contain its mapped closure as the result"
        ),
        other => panic!("typed method RHS must be its closure or a block ending in it: {other:?}"),
    }

    let SymbolInfo::Complete(method_type) = typer.store().symbols.info(method) else {
        panic!("source method must have a completed signature");
    };
    assert_eq!(typer.store().symbols.get(method).kind, SymbolKind::Method);
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typed_definition.name.as_name().text()),
        owner_name(typer.store(), method).as_str(),
        "typed DefDef name must designate the canonical source method"
    );
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
            format!(
                "{}[{},{}]",
                function_name,
                simple_type_name(typer.store(), args[0]),
                simple_type_name(typer.store(), args[1])
            )
        }
        other => panic!("expected Function1 result shape, got {other:?}"),
    };
    assert_eq!(result_shape, root.result_shape, "declared result shape");
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
        let source_definition = match &parsed.ast.get(method_tree).kind {
            TreeKind::DefDef(definition) => definition,
            _ => unreachable!(),
        };
        let source_parameter_tree = source_definition.value_param_clauses[0][0];
        assert_eq!(
            index.symbol_at(source, source_parameter_tree),
            Some(parameter)
        );
        let typed_parameter_tree = typer
            .source_typed_index()
            .get(source, source_parameter_tree)
            .expect("source method parameter mapping");
        assert_eq!(
            typed_definition.value_param_clauses[0][0], typed_parameter_tree,
            "typed method must contain its mapped canonical parameter"
        );
        assert_eq!(
            typer.typed_ast().get(typed_parameter_tree).ty,
            definitions.int,
            "typed method parameter type"
        );
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
    let closure_method_reference = typer.typed_ast().get(closure.method);
    let Type::TermRef {
        target: TermRefTarget::Symbol(closure_method_symbol),
        ..
    } = typer.store().types.get(closure_method_reference.ty)
    else {
        panic!("closure method reference must carry its canonical method symbol");
    };
    assert_eq!(*closure_method_symbol, lambda_method);
    assert_eq!(
        typer.store().symbols.get(lambda_method).owner,
        Some(method),
        "synthetic closure method must be owned by the source method"
    );
    let TreeKind::DefDef(lambda_definition) = &typer.typed_ast().get(lambda_definition_tree).kind
    else {
        panic!("synthetic lambda method must be a complete typed DefDef");
    };
    let Type::TermRef {
        target: TermRefTarget::Symbol(typed_lambda_method),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(lambda_definition_tree).ty)
    else {
        panic!("typed synthetic DefDef must reference its canonical method");
    };
    assert_eq!(*typed_lambda_method, lambda_method);
    let typed_lambda_body = typer
        .source_typed_index()
        .get(source, lambda_body_tree)
        .unwrap();
    assert_eq!(lambda_definition.rhs, Some(typed_lambda_body));
    assert_eq!(
        typer
            .store()
            .names
            .resolve(lambda_definition.name.as_name().text()),
        owner_name(typer.store(), lambda_method).as_str(),
        "closure method reference must resolve to its registered typed definition"
    );
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
                    *class, method_owner,
                    "captured this must designate the exact source method owner"
                );
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
                    .then_some((source_tree, typed))
                });
                assert_eq!(
                    source_use.map(|(_, typed)| typed),
                    Some(*capture),
                    "capture maps from its canonical source use"
                );
                if let Some(parameter) = method_parameter {
                    assert_eq!(
                        *symbol, parameter,
                        "captured method argument must be its canonical source parameter"
                    );
                } else {
                    let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
                        panic!("captured local must be declared in the method's source block");
                    };
                    let local_binding = typer
                        .typed_local_binding_at(source, source_block.stats[0])
                        .expect("canonical source local binding");
                    assert_eq!(
                        *symbol,
                        local_binding.symbol(),
                        "captured local reference must target its canonical declaration"
                    );
                }
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
        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("typed method RHS must retain its local initialization block");
        };
        assert_eq!(typed_block.stats, vec![binding.declaration]);
        assert_eq!(typed_block.expr, typed_lambda);
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
        assert_eq!(
            typer.store().symbols.get(binding.symbol).owner,
            Some(method),
            "captured local declaration must remain owned by its source method"
        );
    }

    let method_signature = type_shape(typer.store(), *method_type);
    let typed_graph = format!(
        "{};synthetic={}",
        typed_tree_shape(typer.typed_ast(), typer.store(), typed_method),
        typed_tree_shape(typer.typed_ast(), typer.store(), lambda_definition_tree)
    );
    SessionReceipt {
        owner: symbol_path(typer.store(), method_owner),
        method: symbol_path(typer.store(), method),
        result_shape,
        captures: capture_names,
        mapping_ok: mapped_semantic_nodes > 0,
        canonical_graph: format!("signature={method_signature};graph={typed_graph}"),
    }
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
        report.push_str(&format!(
            "{}\t{}\t{}\t{}\taccepted\t{}\t[{}]\t{}\t{}\t{}\n",
            root.path,
            root.owner,
            root.method,
            root.source_root,
            first.result_shape,
            first.captures.join(","),
            first.mapping_ok,
            first.normalized(),
            second.normalized(),
        ));
    }
    assert_eq!(report.lines().count(), 7, "all six rows must be present");
    assert_eq!(
        report,
        include_str!("../../../tests/fixtures/lexical-closure-contract/six-root-gate.tsv"),
        "deterministic gate report changed"
    );
}
