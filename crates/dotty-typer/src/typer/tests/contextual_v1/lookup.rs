use super::*;

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn lookup_legacy_member() {
    let text = upstream("tests/pos/test-implicits1.scala");
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(&text, "test-implicits1.scala");
    // X and Xdecorator also have a method called foo; select the declaration
    // belonging to the same object as the initialized y field and its evidence.
    let (y, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            if let TreeKind::ValDef(v) = &node.kind
                && store.names.resolve(v.name.as_name().text()) == "y"
            {
                Some((index.symbol_at(source, tree)?, v.rhs?))
            } else {
                None
            }
        })
        .unwrap();
    let owner = store.symbols.owner(y).unwrap();
    assert_eq!(
        store.names.resolve(store.symbols.get(owner).name.text()),
        "Implicits$"
    );
    let scope = store.scopes.get(index.scope_of(owner).unwrap());
    let member = |name| {
        let name = Name::new(store.names.get(name).unwrap(), Namespace::Term);
        let [symbol] = scope.lookup_all(&name) else {
            panic!("expected one member")
        };
        *symbol
    };
    let (foo, evidence) = (member("foo"), member("impl"));
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let context = ExpressionContext {
        owner,
        lexical: index.declaration_context_of(y).unwrap(),
        local_scopes: None,
    };
    let call = typer
        .type_expression_expected(rhs, context, defs.int)
        .unwrap();
    let TreeKind::Apply(contextual) = &typer.typed_ast().get(call).kind else {
        panic!("expected inferred implicit")
    };
    let [arg] = contextual.args.as_slice() else {
        panic!("one implicit argument")
    };
    assert_eq!(referent(&typer, *arg), evidence);
    let TreeKind::Apply(regular) = &typer.typed_ast().get(contextual.function).kind else {
        panic!("expected foo(1)")
    };
    assert_eq!(referent(&typer, regular.function), foo);
    outcome("match");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn lookup_legacy_block() {
    let text = upstream("tests/pos/test-implicits.scala");
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(&text, "test-implicits.scala");
    let (foo, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "foo");
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("expected foo block")
    };
    let [local] = block.stats.as_slice() else {
        panic!("expected implicit val x")
    };
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    typer
        .type_method_definition(declaration(&index, foo))
        .unwrap();
    let local = typer.local_symbol_at(source, *local).unwrap();
    assert_eq!(typer.store().symbols.owner(local), Some(foo));
    let typed = typer.source_typed_index().get(source, block.expr).unwrap();
    let TreeKind::Apply(app) = &typer.typed_ast().get(typed).kind else {
        panic!("expected implicitly[String](x)")
    };
    let [arg] = app.args.as_slice() else {
        panic!("one implicit argument")
    };
    assert_eq!(referent(&typer, *arg), local);
    let TreeKind::TypeApply(poly) = &typer.typed_ast().get(app.function).kind else {
        panic!("expected type application")
    };
    let method = typer.store().symbols.get(referent(&typer, poly.function));
    assert!(matches!(method.origin, SymbolOrigin::Tasty(_)));
    assert_eq!(
        typer.store().names.resolve(method.name.text()),
        "implicitly"
    );
    let owner = typer.store().symbols.get(method.owner.unwrap());
    assert_eq!(typer.store().names.resolve(owner.name.text()), "Predef$");
    outcome("match");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn lookup_nested_priority() {
    let text = upstream("tests/run/implicit-disambiguation.scala");
    let reduced = text
        .replace("def f(using B, C): String", "def f(using B, C): A")
        .replace("summon[A].show", "summon[A]");
    for (input, complete) in [(&text, true), (&reduced, false)] {
        let (parsed, mut store, packages, defs, index, source) =
            parse_and_name_file(input, "implicit-disambiguation.scala");
        let (f, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "f");
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!("expected f block")
        };
        let resolver = audit_resolver(defs, &packages);
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
            .with_resolver(Box::new(resolver));
        let result = typer.type_method_definition(declaration(&index, f));
        if complete {
            assert!(
                matches!(&parsed.ast.get(block.expr).kind,TreeKind::Select(sel) if typer.store().names.resolve(sel.name.text()) == "show")
            );
            assert!(
                matches!(result,Err(TyperError::UnstableSelectionPrefix{source:s,tree_index,..}) if s==source && tree_index==block.expr.index()),
                "{result:?}"
            );
        } else {
            // A separate control retains both method parameters and the local given.
            // Only the non-contextual member selection and its result type change.
            result.unwrap();
            let [given] = block.stats.as_slice() else {
                panic!("expected local given a")
            };
            let evidence = typer.local_symbol_at(source, *given).unwrap();
            let session = typer.source_typed_index();
            let typed = session.get(source, block.expr).unwrap();
            assert_eq!(referent(&typer, typed), evidence);
            assert_eq!(typer.store().symbols.owner(evidence), Some(f));
            let TreeKind::ValDef(local) = &parsed.ast.get(*given).kind else {
                panic!("expected given definition")
            };
            let initializer = session.get(source, local.rhs.unwrap()).unwrap();
            assert_eq!(
                referent(&typer, initializer),
                method_parameter_symbol(&parsed, &index, source, f, 0)
            );
        }
    }
    outcome("blocked");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn lookup_inherited_priority() {
    let text = upstream("tests/pos/given-priority.scala");
    let default_language = text.replace("import language.`3.6`", "");
    for input in [&text, &default_language] {
        let (parsed, mut store, packages, defs, index, source) =
            parse_and_name_file(input, "given-priority.scala");
        let (test1, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "test1");
        let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
            panic!("expected test1 block")
        };
        let [import, first_val, ..] = block.stats.as_slice() else {
            panic!("import and first val")
        };
        assert!(matches!(parsed.ast.get(*import).kind, TreeKind::Import(_)));
        let TreeKind::ValDef(x) = &parsed.ast.get(*first_val).kind else {
            panic!("expected val x")
        };
        assert_eq!(store.names.resolve(x.name.as_name().text()), "x");
        let first_summon = x.rhs.unwrap();
        let given = |spelling: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(id, node)| match &node.kind {
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(m))
                        if store.names.resolve(m.name.as_name().text()) == spelling =>
                    {
                        index.symbol_at(source, id)
                    }
                    _ => None,
                })
                .unwrap()
        };
        let (g1, g2) = (given("g1"), given("g2"));
        for (symbol, expected_owner) in [(g1, "LowPriorityImplicits"), (g2, "NormalImplicits$")] {
            let owner = store.symbols.get(store.symbols.owner(symbol).unwrap());
            assert_eq!(store.names.resolve(owner.name.text()), expected_owner);
        }
        let resolver = audit_resolver(defs, &packages);
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
            .with_resolver(Box::new(resolver));
        // Type through the audited initializer with its original import.
        // Later wildcard val declarations are outside the selected site.
        let context = typer.expression_context_for(test1).unwrap();
        typer
            .type_block_expression(
                rhs,
                dotty_core::ast::Block {
                    stats: vec![*import],
                    expr: first_summon,
                },
                parsed.ast.get(rhs).position,
                context,
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();
        let selected = typer
            .source_typed_index()
            .get(source, first_summon)
            .unwrap();
        assert_eq!(referent(&typer, selected), g2);
    }
    outcome("match");
}
