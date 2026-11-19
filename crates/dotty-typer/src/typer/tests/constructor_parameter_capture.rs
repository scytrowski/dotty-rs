use super::*;

fn method_tree(
    parsed: &dotty_parser::ParseResult,
    index: &SourceSemanticIndex,
    source: SourceId,
    method: SymbolId,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (index.symbol_at(source, tree) == Some(method)
                && matches!(node.kind, TreeKind::DefDef(_)))
            .then_some(tree)
        })
        .expect("source method has a definition tree")
}

fn typed_capture_receipt(source_text: &str) -> (String, Vec<String>) {
    let (parsed, mut store, mut packages, definitions, index, source) = parse_and_name(source_text);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let method = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (matches!(node.kind, TreeKind::DefDef(_))
                && index.symbol_at(source, tree).is_some_and(|symbol| {
                    store.names.resolve(store.symbols.get(symbol).name.text()) == "capture"
                }))
            .then(|| index.symbol_at(source, tree).unwrap())
        })
        .expect("capture method symbol");
    let method_tree = method_tree(&parsed, &index, source, method);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(method_tree).unwrap();
    let typed_method = typer.source_typed_index().get(source, method_tree).unwrap();
    let TreeKind::DefDef(definition) = &typer.typed_ast().get(typed_method).kind else {
        panic!("capture method remains a source method definition");
    };
    let rhs = definition.rhs.expect("method body");
    let TreeKind::Closure(closure) = &typer.typed_ast().get(rhs).kind else {
        panic!("method body is a closure");
    };
    let capture_symbols = closure
        .env
        .iter()
        .map(
            |capture| match typer.store().types.get(typer.typed_ast().get(*capture).ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("capture retains its canonical term reference: {other:?}"),
            },
        )
        .collect::<Vec<_>>();
    let source_use = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::Ident(identifier)
                if typer.store().names.resolve(identifier.name.text()) == "xs" =>
            {
                Some((tree, node.position))
            }
            _ => None,
        })
        .expect("source constructor-parameter use");
    assert_eq!(
        typer.source_typed_index().get(source, source_use.0),
        closure.env.first().copied(),
        "capture retains the first typed source use identity"
    );
    assert_eq!(
        typer.typed_ast().get(closure.env[0]).position,
        source_use.1,
        "capture retains the source position"
    );
    let captures = capture_symbols
        .iter()
        .map(|symbol| {
            typer
                .store()
                .names
                .resolve(typer.store().symbols.get(*symbol).name.text())
                .to_owned()
        })
        .collect();
    let class = typer.store().symbols.get(method).owner.unwrap();
    let class_parameter = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(parameter)
                if typer.store().names.resolve(parameter.name.as_name().text()) == "xs" =>
            {
                index.symbol_at(source, tree).filter(|symbol| {
                    typer.store().symbols.get(*symbol).kind == SymbolKind::Parameter
                        && typer.store().symbols.get(*symbol).owner == Some(class)
                })
            }
            _ => None,
        })
        .expect("canonical class constructor parameter");
    assert_eq!(capture_symbols, [class_parameter]);
    assert_eq!(
        typer.store().symbols.get(class_parameter).owner,
        Some(class)
    );
    assert_eq!(
        typer.store().symbols.get(class_parameter).visibility,
        Visibility::Private
    );
    assert_eq!(
        typer.store().symbols.get(class_parameter).kind,
        SymbolKind::Parameter
    );
    let class_scope = index.scope_of(class).unwrap();
    assert!(
        typer
            .store()
            .scopes
            .get(class_scope)
            .lookup_all(&typer.store().symbols.get(class_parameter).name)
            .is_empty()
    );
    (
        format!("owner={class:?};parameter={class_parameter:?};captures={captures:?}"),
        captures,
    )
}

#[test]
fn ordinary_function1_captures_canonical_plain_class_parameter_once_in_first_use_order() {
    let source = "class C(xs: Int) { def capture: Int => Int = value => xs }";
    let (_, captures) = typed_capture_receipt(source);
    assert_eq!(captures, ["xs"]);
    let (_, repeated) =
        typed_capture_receipt("class C(xs: Int) { def capture: Int => Int = value => xs + xs }");
    assert_eq!(
        repeated,
        ["xs"],
        "repeated first-use references deduplicate"
    );
}

#[test]
fn constructor_parameter_environment_keeps_first_use_order_and_deduplicates() {
    let source = "class C(first: Int, second: Int) { def capture: Int => Int = value => second + first + second }";
    let (parsed, mut store, mut packages, definitions, index, source_id) = parse_and_name(source);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let method = parsed
        .ast
        .iter()
        .find_map(|(tree, _)| {
            let symbol = index.symbol_at(source_id, tree)?;
            (store.names.resolve(store.symbols.get(symbol).name.text()) == "capture")
                .then_some(symbol)
        })
        .unwrap();
    let method_tree = method_tree(&parsed, &index, source_id, method);
    let class = store.symbols.get(method).owner.unwrap();
    let expected = ["second", "first"].map(|name| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(parameter)
                    if store.names.resolve(parameter.name.as_name().text()) == name =>
                {
                    index.symbol_at(source_id, tree).filter(|symbol| {
                        store.symbols.get(*symbol).owner == Some(class)
                            && store.symbols.get(*symbol).kind == SymbolKind::Parameter
                    })
                }
                _ => None,
            })
            .unwrap()
    });
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(method_tree).unwrap();
    let typed_method = typer
        .source_typed_index()
        .get(source_id, method_tree)
        .unwrap();
    let source_rhs = match &parsed.ast.get(method_tree).kind {
        TreeKind::DefDef(definition) => definition.rhs.unwrap(),
        _ => unreachable!(),
    };
    let TreeKind::DefDef(definition) = &typer.typed_ast().get(typed_method).kind else {
        panic!("capture method remains a method");
    };
    assert!(definition.rhs.is_some());
    let typed_rhs = typer
        .source_typed_index()
        .get(source_id, source_rhs)
        .unwrap();
    let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("method body is a closure");
    };
    let actual = closure
        .env
        .iter()
        .map(
            |tree| match typer.store().types.get(typer.typed_ast().get(*tree).ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("closure capture retains a canonical symbol: {other:?}"),
            },
        )
        .collect::<Vec<_>>();
    assert_eq!(
        actual, expected,
        "first use wins and repeated use deduplicates"
    );
}

#[test]
#[ignore = "run with SCALA39_CLASSPATH and Java for the pinned Scala 3.9.0 oracle"]
fn pinned_scala_3_9_0_accepts_direct_owner_method_constructor_capture() {
    let classpath = std::env::var_os("SCALA39_CLASSPATH").expect("SCALA39_CLASSPATH");
    let scratch = std::env::var_os("TMPDIR").expect("managed TMPDIR");
    let scratch = std::path::PathBuf::from(scratch);
    let output = scratch.join("issue-2380-scala-oracle-classes");
    std::fs::create_dir_all(&output).unwrap();
    let version = std::process::Command::new("java")
        .arg("-cp")
        .arg(&classpath)
        .arg("dotty.tools.dotc.Main")
        .arg("-version")
        .output()
        .expect("launch pinned Scala version check");
    assert!(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout).contains("Scala compiler version 3.9.0"),
        "oracle classpath must resolve to Scala 3.9.0: {}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr),
    );
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/constructor-parameter-function1-3.9.0/DirectCapture.scala");
    let result = std::process::Command::new("java")
        .arg("-XX:-UsePerfData")
        .arg("-Xshare:off")
        .arg(format!("-Djava.io.tmpdir={}", scratch.display()))
        .arg("-cp")
        .arg(&classpath)
        .arg("dotty.tools.dotc.Main")
        .arg("-classpath")
        .arg(&classpath)
        .arg("-d")
        .arg(&output)
        .arg("-color:never")
        .arg(source)
        .output()
        .expect("launch pinned Scala 3.9.0 compiler");
    assert!(
        result.status.success(),
        "pinned Scala oracle rejected the direct owner-method capture:\n{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    std::fs::remove_dir_all(output).unwrap();
    println!("SCALA39_DIRECT_CONSTRUCTOR_CAPTURE accepted=true");
}

#[test]
fn constructor_parameter_capture_is_shadowed_and_does_not_cross_instance_owners() {
    let source = r#"
class C(xs: Int) {
  def capture: Int => Int = value => xs
  def localShadow: Int => Int = { val xs: Int = 2; value => xs }
  def lambdaShadow: Int => Int = (xs: Int) => xs
  def other(that: C): Int => Int = value => that.xs
}
class D(ys: Int) { def unrelated: Int => Int = value => xs }
"#;
    let (parsed, mut store, mut packages, definitions, index, source_id) = parse_and_name(source);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let methods = [
        "capture",
        "localShadow",
        "lambdaShadow",
        "other",
        "unrelated",
    ]
    .into_iter()
    .map(|name| {
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| {
                let symbol = index.symbol_at(source_id, tree)?;
                (store.names.resolve(store.symbols.get(symbol).name.text()) == name)
                    .then_some(symbol)
            })
            .unwrap();
        (name, method_tree(&parsed, &index, source_id, method))
    })
    .collect::<Vec<_>>();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (name, tree) in methods {
        let result = typer.type_method_definition(tree);
        if name == "unrelated" {
            assert!(matches!(result, Err(TyperError::TermNameNotFound { .. })));
            continue;
        }
        if name == "other" {
            assert!(
                result.is_err(),
                "other-instance selection remains unsupported"
            );
            continue;
        }
        result.unwrap_or_else(|error| panic!("{name} should type: {error:?}"));
        let typed = typer.source_typed_index().get(source_id, tree).unwrap();
        let TreeKind::DefDef(definition) = &typer.typed_ast().get(typed).kind else {
            panic!("{name} remains a method");
        };
        let rhs = definition.rhs.unwrap();
        let lambda = match &typer.typed_ast().get(rhs).kind {
            TreeKind::Block(block) => block.expr,
            _ => rhs,
        };
        let TreeKind::Closure(closure) = &typer.typed_ast().get(lambda).kind else {
            panic!("{name} ends in a closure");
        };
        if name == "capture" {
            assert_eq!(closure.env.len(), 1);
            continue;
        }
        if name == "localShadow" {
            assert_eq!(closure.env.len(), 1);
            let Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } = typer
                .store()
                .types
                .get(typer.typed_ast().get(closure.env[0]).ty)
            else {
                panic!("local shadow capture retains its identity");
            };
            assert_eq!(typer.store().symbols.get(*symbol).kind, SymbolKind::Local);
            continue;
        }
        assert!(closure.env.is_empty(), "{name} must not capture class xs");
    }
}

#[test]
fn constructor_parameter_capture_rejects_unsupported_lexical_forms() {
    let cases = [
        (
            "class C(xs: Int) { def capture: Int => Int = x => y => xs }",
            "nested",
        ),
        (
            "class C(xs: Int) { def capture: (Int, Int) => Int = (x, y) => xs }",
            "arity",
        ),
        (
            "class C(xs: Int) { def capture: Int ?=> Int = x ?=> xs }",
            "contextual",
        ),
        (
            "class C(xs: => Int) { def capture: Int => Int = x => xs }",
            "by-name",
        ),
        (
            "class C(xs: Int*) { def capture: Int => Int = x => xs.size }",
            "repeated",
        ),
        (
            "class C(var xs: Int) { def capture: Int => Int = x => xs }",
            "mutable",
        ),
    ];
    for (source, label) in cases {
        let (parsed, mut store, mut packages, definitions, index, source_id) =
            parse_and_name(source);
        enter_scala_function_classes(&mut store, &mut packages, [1, 2]);
        let method = parsed.ast.iter().find_map(|(tree, node)| {
            (matches!(node.kind, TreeKind::DefDef(_))
                && index.symbol_at(source_id, tree).is_some_and(|symbol| {
                    store.names.resolve(store.symbols.get(symbol).name.text()) == "capture"
                }))
            .then(|| index.symbol_at(source_id, tree).unwrap())
        });
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source_id,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        if let Some(method) = method {
            let tree = method_tree(&parsed, &index, source_id, method);
            assert!(
                typer.type_method_definition(tree).is_err(),
                "{label} must remain unsupported"
            );
        } else {
            assert!(
                !parsed.diagnostics.is_empty(),
                "{label} source must remain a parse diagnostic"
            );
        }
    }
}

#[test]
fn implicit_context_abstraction_rejects_plain_constructor_parameter_capture() {
    let (parsed, mut store, mut packages, definitions, index, source) =
        parse_and_name("class C(xs: Int) { def capture: Int ?=> Int = xs }");
    enter_capture_checked_function_classes(&mut store, &mut packages, [1]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "capture");
    let tree = method_tree(&parsed, &index, source, method);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let error = typer.type_method_definition(tree).unwrap_err();
    let TyperError::FunctionLiteralCaptureUnsupported {
        source: at,
        tree_index,
        symbol: Some(symbol),
    } = error
    else {
        panic!("expected unsupported constructor parameter capture, got {error:?}");
    };
    assert_eq!((at, tree_index), (source, rhs.index()));
    let store = typer.store();
    let parameter = store.symbols.get(symbol);
    assert_eq!(parameter.kind, SymbolKind::Parameter);
    assert_eq!(parameter.owner, store.symbols.owner(method));
    assert_eq!(store.names.resolve(parameter.name.text()), "xs");
}

#[test]
fn constructor_parameter_capture_remains_unsupported_in_initializers() {
    let source = "class C(xs: Int) { val capture: Int => Int = x => xs }";
    let (parsed, mut store, mut packages, definitions, index, source_id) = parse_and_name(source);
    enter_scala_function_classes(&mut store, &mut packages, [1]);
    let (field, _, rhs) = val_definition_and_rhs(&parsed, &store, &index, source_id, "capture");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let context = typer.field_initializer_context_for(field).unwrap();
    assert!(
        typer.type_expression(rhs, context).is_err(),
        "a constructor parameter closure in an initializer remains unsupported"
    );
}

#[test]
fn class_parameter_capture_failure_rolls_back_and_retry_is_deterministic() {
    fn session(source: &str) -> (String, String) {
        let (parsed, mut store, mut packages, definitions, index, source_id) =
            parse_and_name(source);
        enter_scala_function_classes(&mut store, &mut packages, [1]);
        let method = parsed
            .ast
            .iter()
            .find_map(|(tree, _)| {
                let symbol = index.symbol_at(source_id, tree)?;
                (store.names.resolve(store.symbols.get(symbol).name.text()) == "capture")
                    .then_some(symbol)
            })
            .unwrap();
        let tree = method_tree(&parsed, &index, source_id, method);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source_id,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let bad = match &parsed.ast.get(tree).kind {
            TreeKind::DefDef(definition) => definition.rhs.unwrap(),
            _ => unreachable!(),
        };
        let before_store = typer.store().checkpoint();
        let before_typed = typer.typed_ast().checkpoint();
        let observed = Rc::new(RefCell::new(None));
        let observed_capture = observed.clone();
        typer.transaction_failure_observer = Some(Rc::new(move |typer| {
            let lambda = typer
                .function_literal_method_at(source_id, bad)
                .expect("the direct Function1 capture is live before rollback");
            let captures = typer
                .function_literals
                .captures(lambda)
                .iter()
                .map(
                    |capture| match typer.store().types.get(typer.typed_ast().get(*capture).ty) {
                        Type::TermRef {
                            target: TermRefTarget::Symbol(symbol),
                            ..
                        } => typer.store().symbols.get(*symbol).name,
                        other => {
                            panic!("captured constructor parameter retains identity: {other:?}")
                        }
                    },
                )
                .collect::<Vec<_>>();
            *observed_capture.borrow_mut() = Some(captures);
        }));
        typer.inject_failure_after_outermost_success = true;
        assert!(
            typer.type_method_definition(tree).is_err(),
            "one-shot late failure is injected after successful capture"
        );
        assert_eq!(observed.borrow_mut().take().unwrap().len(), 1);
        assert_eq!(typer.store().checkpoint(), before_store);
        assert_eq!(typer.typed_ast().checkpoint(), before_typed);
        assert_eq!(typer.source_typed_index().get(source_id, tree), None);
        assert!(typer.function_literal_method_at(source_id, bad).is_none());
        typer.transaction_failure_observer = None;
        let typed = typer
            .type_method_definition(tree)
            .expect("corrected same-session retry succeeds after rollback");
        assert_eq!(typer.source_typed_index().get(source_id, tree), Some(typed));
        let typed_rhs = typer.source_typed_index().get(source_id, bad).unwrap();
        let TreeKind::Closure(closure) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("retry produces a closure");
        };
        assert_eq!(closure.env.len(), 1);
        let capture_name = typer
            .store()
            .names
            .resolve(
                typer
                    .store()
                    .symbols
                    .get(
                        match typer
                            .store()
                            .types
                            .get(typer.typed_ast().get(closure.env[0]).ty)
                        {
                            Type::TermRef {
                                target: TermRefTarget::Symbol(symbol),
                                ..
                            } => *symbol,
                            other => panic!("canonical class parameter reference: {other:?}"),
                        },
                    )
                    .name
                    .text(),
            )
            .to_owned();
        (
            format!("class=C;parameter={capture_name};closure.env=[{capture_name}];result=Int"),
            capture_name,
        )
    }

    let source = "class C(xs: Int) { def capture: Int => Int = value => xs }";
    let first = session(source);
    let second = session(source);
    assert_eq!(first, second);
}
