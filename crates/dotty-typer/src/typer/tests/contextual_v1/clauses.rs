use super::*;

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn c1_curried_threading_default() {
    let text = upstream("tests/run/curried-using-threading.scala");
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(&text, "curried-using-threading.scala");
    let test = method_symbol(&parsed, &store, &index, source, "Test");
    let narrow = method_symbol(&parsed, &store, &index, source, "narrow");
    let given = parsed
        .ast
        .iter()
        .find_map(|(id, node)| {
            let TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) = &node.kind else {
                return None;
            };
            module
                .metadata
                .modifiers
                .contains(&dotty_core::ast::Modifier::Given)
                .then(|| index.symbol_at(source, id).unwrap())
        })
        .unwrap();
    assert_eq!(
        store.names.resolve(store.symbols.get(given).name.text()),
        "given_Context"
    );
    assert_eq!(store.symbols.owner(given), store.symbols.owner(narrow));
    let mut sites = parsed.ast.iter().filter_map(|(id, node)| {
        let TreeKind::Apply(call) = &node.kind else {
            return None;
        };
        let span = node.position?.span().range();
        (call.args.is_empty() && &text[span.start() as usize..span.end() as usize] == "narrow()")
            .then_some(id)
    });
    let site = sites.next().expect("original narrow() in Test");
    assert!(sites.next().is_none());
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let ctx = typer.expression_context_for(test).unwrap();
    let result = typer.type_expression(site, ctx);
    assert!(
        matches!(result, Err(TyperError::ApplicationArityMismatch { source: at, tree_index, expected: 1, actual: 0 })
            if at == source && tree_index == site.index()),
        "{result:?}"
    );
    reduced_ordinary_default_after_using();
    outcome("blocked");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn c2_bound_and_implicit_default() {
    let text = upstream("tests/pos/i8491.scala");
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(&text, "i8491.scala");
    let method = method_symbol(&parsed, &store, &index, source, "qux2");
    let tree = declaration(&index, method);
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!("qux2 method")
    };
    let parameter = def.value_param_clauses[0][0];
    let defaulted = index.symbol_at(source, parameter).unwrap();
    let TreeKind::TypeDef(type_parameter) = &parsed.ast.get(def.type_params[0]).kind else {
        panic!("type parameter A")
    };
    let TreeKind::PhaseSpecific(UntypedNode::ContextBounds(bounds)) =
        &parsed.ast.get(type_parameter.rhs).kind
    else {
        panic!("A: Foo")
    };
    let witness = index.symbol_at(source, bounds.context_bounds[0]).unwrap();
    let owner = store.symbols.owner(method).unwrap();
    let getter = index.derived_symbol_at(owner, source, parameter).unwrap();
    let foo = class_symbol(&parsed, &store, &index, source, "Foo");
    let bar = class_symbol(&parsed, &store, &index, source, "Bar");
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));

    // Signature completion already places the bound witness before B.
    let signature = typer.complete_symbol(method).unwrap();
    let Type::Poly(poly) = typer.store().types.get(signature) else {
        panic!("polymorphic qux2")
    };
    let Type::Method(clause) = typer.store().types.get(poly.result) else {
        panic!("implicit clause")
    };
    assert_eq!(clause.kind, MethodKind::Implicit);
    let [evidence, b] = clause.params.as_slice() else {
        panic!("expected evidence and B")
    };
    for (formal, symbol) in [(evidence, witness), (b, defaulted)] {
        assert_eq!(typer.store().symbols.owner(symbol), Some(method));
        assert_eq!(
            *formal.name.as_name(),
            typer.store().symbols.get(symbol).name
        );
    }
    let Type::Applied { tycon, .. } = typer.store().types.get(evidence.ty) else {
        panic!("Foo[A]")
    };
    assert_eq!(
        typer.store().types.get(*tycon).reference_symbol(),
        Some(foo)
    );
    assert_eq!(typer.store().types.get(b.ty).reference_symbol(), Some(bar));

    // The first failure is completing the getter signature, before typing
    // the anonymous class in B's initializer. Scala assigns it ordinal 2.
    assert_eq!(typer.store().default_getter(method, 1), None);
    for result in [
        typer.complete_symbol(getter).map(|_| ()),
        typer.type_method_definition(tree).map(|_| ()),
    ] {
        assert!(
            matches!(result,Err(TyperError::UsingApplicationDeferred { source: s, tree_index }) if s == source && tree_index == parameter.index()),
            "{result:?}"
        );
    }
    outcome("outside-v1");
}

#[test]
#[ignore = "requires pinned Scala 3.9 sources"]
fn c3_explicit_context_function() {
    let text = upstream("tests/pos/i5295.scala");
    let (parsed, mut store, mut packages, defs, index, source) =
        parse_and_name_file(&text, "i5295.scala");
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "foo");
    let bar = method_symbol(&parsed, &store, &index, source, "bar");
    let wrapper = store.symbols.owner(method).unwrap();
    assert_eq!(store.symbols.owner(bar), Some(wrapper));
    assert_eq!(store.symbols.get(wrapper).kind, SymbolKind::ModuleClass);
    assert_eq!(store.symbols.get(wrapper).origin, SymbolOrigin::Synthetic);
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let method_tree = declaration(&index, method);
    let typed = typer.type_method_definition(method_tree).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(typed).kind else {
        panic!("typed foo")
    };
    let declared_result = typer.typed_ast().get(def.tpt).ty;
    let typed_rhs = def.rhs.unwrap();
    assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
    let string = typer
        .store
        .types
        .get(declared_result)
        .reference_symbol()
        .unwrap();
    assert_eq!(
        typer
            .store
            .names
            .resolve(typer.store.symbols.get(string).name.text()),
        "String"
    );
    let context = typer.expression_context_for(method).unwrap();
    assert_eq!(typer.type_expression(rhs, context).unwrap(), typed_rhs);
    assert_explicit_application(&mut typer, rhs, typed_rhs, bar, declared_result);
    reduced_explicit_context_function_in_member();
    outcome("match");
}

fn reduced_ordinary_default_after_using() {
    let text =
        include_str!("../../../../tests/fixtures/contextual-v1/controls/CurriedDefault.scala");
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "test");
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let ctx = typer.expression_context_for(method).unwrap();
    let result = typer.type_expression_expected(rhs, ctx, defs.int);
    assert!(
        matches!(result,Err(TyperError::ApplicationArityMismatch {source: s, tree_index, expected: 1, actual: 0}) if s == source && tree_index == rhs.index()),
        "{result:?}"
    );
}

fn reduced_explicit_context_function_in_member() {
    let text = include_str!(
        "../../../../tests/fixtures/contextual-v1/controls/ExplicitContextFunction.scala"
    );
    let (parsed, mut store, mut packages, defs, index, source) = parse_and_name(text);
    packages.enter(&mut store, SymbolOrigin::Synthetic, &["scala"]);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "foo");
    let bar = method_symbol(&parsed, &store, &index, source, "bar");
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer
        .type_method_definition(declaration(&index, method))
        .unwrap();
    let typed_rhs = typer.source_typed_index().get(source, rhs).unwrap();
    assert_explicit_application(&mut typer, rhs, typed_rhs, bar, defs.int);
}

fn assert_explicit_application(
    typer: &mut SourceTyper<'_>,
    rhs: TreeId<Untyped>,
    typed_rhs: TreeId<Typed>,
    bar: SymbolId,
    result: TypeId,
) {
    let family = typer
        .source_function_class(SourceFunctionKind::Contextual, 1, rhs.index())
        .unwrap();
    let ast = typer.typed_ast();
    let store = typer.store();
    let node = ast.get(typed_rhs);
    let TreeKind::Apply(call) = &node.kind else {
        panic!("contextual application of bar")
    };
    let application_result = node.ty;
    assert_eq!(call.kind, ApplyKind::Using);
    let [literal] = call.args.as_slice() else {
        panic!("one explicit argument")
    };
    assert!(
        matches!(&ast.get(*literal).kind, TreeKind::Literal(l) if l.value == dotty_core::Constant::Int(4))
    );
    let selection = ast.get(call.function);
    let TreeKind::Select(member) = &selection.kind else {
        panic!("selection of ContextFunction1.apply")
    };
    assert_eq!(
        store
            .types
            .get(ast.get(member.qualifier).ty)
            .reference_symbol(),
        Some(bar)
    );
    let Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(apply),
    } = store.types.get(selection.ty)
    else {
        panic!("resolved apply")
    };
    let Type::Applied { tycon, args } = store.types.get(*prefix) else {
        panic!("instantiated context function")
    };
    let [parameter, function_result] = args[..] else {
        panic!("context function takes Int and returns the declared result")
    };
    assert_eq!(parameter, typer.definitions.int);
    assert_eq!(store.types.get(*tycon).reference_symbol(), Some(family));
    assert_eq!(store.symbols.owner(*apply), Some(family));
    let SymbolInfo::Complete(info) = *store.symbols.info(family) else {
        panic!("complete ContextFunction1")
    };
    let Type::ClassInfo(class) = store.types.get(info) else {
        panic!("context function class info")
    };
    assert_eq!(
        store
            .scopes
            .get(class.declarations)
            .lookup_all(&member.name),
        &[*apply]
    );
    assert_eq!(
        store.names.resolve(store.symbols.get(*apply).name.text()),
        "apply"
    );
    for actual in [application_result, function_result] {
        assert!(typer.is_subtype(actual, result).unwrap());
        assert!(typer.is_subtype(result, actual).unwrap());
    }
}
