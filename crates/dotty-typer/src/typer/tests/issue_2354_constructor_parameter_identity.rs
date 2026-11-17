use super::*;

fn ident_in_method(
    parsed: &dotty_parser::ParseResult,
    method_tree: TreeId<Untyped>,
    name: &str,
    store: &SemanticStore,
) -> TreeId<Untyped> {
    let TreeKind::DefDef(method) = &parsed.ast.get(method_tree).kind else {
        unreachable!("method tree is a DefDef")
    };
    let rhs = method.rhs.expect("test method has an rhs");
    let range = parsed.ast.get(rhs).position.unwrap().span().range();
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            if !node.position.is_some_and(|position| {
                let span = position.span().range();
                range.start() <= span.start() && span.end() <= range.end()
            }) {
                return None;
            }
            match &node.kind {
                TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == name => {
                    Some(tree)
                }
                _ => None,
            }
        })
        .expect("method contains the requested identifier")
}

fn typed_ident_target(
    typer: &SourceTyper<'_>,
    source: SourceId,
    ident: TreeId<Untyped>,
) -> SymbolId {
    let typed = typer
        .source_typed_index()
        .get(source, ident)
        .expect("identifier has a typed-tree mapping");
    let Type::TermRef {
        target: TermRefTarget::Symbol(symbol),
        ..
    } = typer.store().types.get(typer.typed_ast().get(typed).ty)
    else {
        panic!("identifier type retains its canonical term target");
    };
    *symbol
}

fn method_tree(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    name: &str,
) -> TreeId<Untyped> {
    parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == name =>
            {
                Some(tree)
            }
            _ => None,
        })
        .expect("named source method")
}

#[test]
fn plain_constructor_parameter_is_visible_to_own_methods_without_accessor() {
    let text = "class C(xs: Int) { def read: Int = xs; def shadow(xs: Int): Int = xs; def local: Int = { val xs: Int = 2; xs } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let class = class_symbol(&parsed, &store, &index, source, "C");
    let parameter_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "xs"
                    && index.symbol_at(source, tree).is_some_and(|symbol| {
                        store.symbols.get(symbol).owner == Some(class)
                            && store.symbols.get(symbol).kind == SymbolKind::Parameter
                    }) =>
            {
                Some(tree)
            }
            _ => None,
        })
        .expect("plain constructor parameter source tree");
    let parameter = index.symbol_at(source, parameter_tree).unwrap();
    let read_tree = method_tree(&parsed, &store, "read");
    let shadow_tree = method_tree(&parsed, &store, "shadow");
    let local_tree = method_tree(&parsed, &store, "local");
    let read_ident = ident_in_method(&parsed, read_tree, "xs", &store);
    let shadow_ident = ident_in_method(&parsed, shadow_tree, "xs", &store);
    let local_ident = ident_in_method(&parsed, local_tree, "xs", &store);
    let shadow_parameter = match &parsed.ast.get(shadow_tree).kind {
        TreeKind::DefDef(method) => index
            .symbol_at(source, method.value_param_clauses[0][0])
            .unwrap(),
        _ => unreachable!(),
    };
    let class_scope = index.scope_of(class).unwrap();
    assert!(
        store
            .scopes
            .get(class_scope)
            .lookup_all(&store.symbols.get(parameter).name)
            .is_empty()
    );

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(read_tree).unwrap();
    assert_eq!(typed_ident_target(&typer, source, read_ident), parameter);
    assert_eq!(
        typer.store().symbols.get(parameter).owner,
        Some(class),
        "method typing preserves the source class owner"
    );
    assert_eq!(
        typer.store().symbols.get(parameter).visibility,
        Visibility::Private
    );
    assert!(matches!(
        *typer.store().symbols.info(parameter),
        SymbolInfo::Complete(_)
    ));
    typer.type_method_definition(shadow_tree).unwrap();
    assert_eq!(
        typed_ident_target(&typer, source, shadow_ident),
        shadow_parameter,
        "method parameters keep their normal lexical precedence"
    );
    typer.type_method_definition(local_tree).unwrap();
    let local_target = typed_ident_target(&typer, source, local_ident);
    assert_eq!(
        typer.store().symbols.get(local_target).owner,
        index.symbol_at(source, local_tree),
        "local declarations keep their normal lexical precedence"
    );
    assert_ne!(local_target, parameter);
}

#[test]
fn val_constructor_parameter_keeps_field_identity_and_generic_plain_parameter_uses_class_type_parameter()
 {
    let text =
        "class V(val xs: Int) { def read: Int = xs }; class G[T](xs: T) { def read: T = xs }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let val_class = class_symbol(&parsed, &store, &index, source, "V");
    let generic_class = class_symbol(&parsed, &store, &index, source, "G");
    let val_tree = method_tree(&parsed, &store, "read");
    let val_ident = ident_in_method(&parsed, val_tree, "xs", &store);
    let val_symbol = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "xs"
                    && index.symbol_at(source, tree).is_some_and(|symbol| {
                        store.symbols.get(symbol).owner == Some(val_class)
                            && store.symbols.get(symbol).kind == SymbolKind::Field
                    }) =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .expect("val constructor parameter is a class field");
    let generic_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            (matches!(node.kind, TreeKind::DefDef(_))
                && index
                    .symbol_at(source, tree)
                    .is_some_and(|symbol| store.symbols.get(symbol).owner == Some(generic_class)))
            .then_some(tree)
        })
        .expect("generic class method");
    let generic_ident = ident_in_method(&parsed, generic_tree, "xs", &store);
    let generic_parameter = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "xs"
                    && index.symbol_at(source, tree).is_some_and(|symbol| {
                        store.symbols.get(symbol).owner == Some(generic_class)
                            && store.symbols.get(symbol).kind == SymbolKind::Parameter
                    }) =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    let class_type_parameter = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "T"
                    && index.symbol_at(source, tree).is_some_and(|symbol| {
                        store.symbols.get(symbol).owner == Some(generic_class)
                    }) =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(val_tree).unwrap();
    assert_eq!(typed_ident_target(&typer, source, val_ident), val_symbol);
    typer.type_method_definition(generic_tree).unwrap();
    assert_eq!(
        typed_ident_target(&typer, source, generic_ident),
        generic_parameter
    );
    let parameter_type = match *typer.store().symbols.info(generic_parameter) {
        SymbolInfo::Complete(ty) => ty,
        other => panic!("plain parameter type was not completed: {other:?}"),
    };
    assert!(matches!(
        typer.store().types.get(parameter_type),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
            if *symbol == class_type_parameter
    ));
}

#[test]
fn late_method_failure_rolls_back_parameter_completion_and_retry_succeeds() {
    let text = "class C(xs: Int) { def bad: Boolean = xs; def good: Int = xs }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let class = class_symbol(&parsed, &store, &index, source, "C");
    let parameter = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "xs"
                    && index.symbol_at(source, tree).is_some_and(|symbol| {
                        store.symbols.get(symbol).owner == Some(class)
                            && store.symbols.get(symbol).kind == SymbolKind::Parameter
                    }) =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    let bad_tree = method_tree(&parsed, &store, "bad");
    let good_tree = method_tree(&parsed, &store, "good");
    let bad_rhs = match &parsed.ast.get(bad_tree).kind {
        TreeKind::DefDef(method) => method.rhs.unwrap(),
        _ => unreachable!(),
    };
    let bad_ident = ident_in_method(&parsed, bad_tree, "xs", &store);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    assert!(typer.type_method_definition(bad_tree).is_err());
    assert_eq!(typer.store().symbols.info(parameter), &SymbolInfo::Missing);
    assert_eq!(typer.source_typed_index().get(source, bad_tree), None);
    assert_eq!(typer.source_typed_index().get(source, bad_rhs), None);
    assert_eq!(typer.source_typed_index().get(source, bad_ident), None);
    assert!(
        typer.expression_scopes.is_empty(),
        "a failed method does not leave a live expression scope"
    );
    typer.type_method_definition(good_tree).unwrap();
    let good_ident = ident_in_method(&parsed, good_tree, "xs", typer.store());
    assert_eq!(typed_ident_target(&typer, source, good_ident), parameter);
}

#[test]
fn constructor_parameter_is_not_visible_in_an_unrelated_class() {
    let text = "class A(xs: Int); class B { def read: Int = xs }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let method = method_tree(&parsed, &store, "read");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    assert!(matches!(
        typer.type_method_definition(method),
        Err(TyperError::TermNameNotFound { .. })
    ));
    assert_eq!(typer.source_typed_index().get(source, method), None);
}

#[test]
fn nested_class_can_access_lexically_enclosing_constructor_parameter() {
    let text = "class Outer(xs: Int) { class Inner { def read: Int = xs } }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let outer = class_symbol(&parsed, &store, &index, source, "Outer");
    let parameter = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "xs"
                    && index.symbol_at(source, tree).is_some_and(|symbol| {
                        store.symbols.get(symbol).owner == Some(outer)
                            && store.symbols.get(symbol).kind == SymbolKind::Parameter
                    }) =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    let method = method_tree(&parsed, &store, "read");
    let ident = ident_in_method(&parsed, method, "xs", &store);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer
        .type_method_definition(method)
        .expect("a nested class retains legal access to its enclosing class parameter");
    assert_eq!(typed_ident_target(&typer, source, ident), parameter);
}

#[test]
fn duplicate_plain_constructor_parameter_names_remain_ambiguous() {
    let text = "class C(xs: Int, xs: Int) { def read: Int = xs }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let method = method_tree(&parsed, &store, "read");
    let ident = ident_in_method(&parsed, method, "xs", &store);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    assert!(matches!(
        typer.type_method_definition(method),
        Err(TyperError::AmbiguousTermReference { tree_index, .. })
            if tree_index == ident.index()
    ));
    assert_eq!(typer.source_typed_index().get(source, method), None);
    assert_eq!(typer.source_typed_index().get(source, ident), None);
}

#[test]
fn local_imports_remain_ambiguous_with_plain_and_val_constructor_parameters() {
    for (parameter, import) in [
        ("xs: Int", "import Lib.xs"),
        ("xs: Int", "import Lib.*"),
        ("val xs: Int", "import Lib.xs"),
        ("val xs: Int", "import Lib.*"),
    ] {
        let text = format!(
            "object Lib {{ val xs: Int = 2 }}; class C({parameter}) {{ def read: Int = {{ {import}; xs }} }}"
        );
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
        let method = method_tree(&parsed, &store, "read");
        let ident = ident_in_method(&parsed, method, "xs", &store);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        assert!(
            matches!(
                typer.type_method_definition(method),
                Err(TyperError::AmbiguousTermReference { tree_index, .. })
                    if tree_index == ident.index()
            ),
            "{parameter} with {import} must preserve local-import ambiguity"
        );
        assert_eq!(typer.source_typed_index().get(source, method), None);
        assert_eq!(typer.source_typed_index().get(source, ident), None);
    }
}
