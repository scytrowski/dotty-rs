use super::*;
use dotty_core::ast::DefDef;

fn with_body(
    name: &str,
    check: impl FnOnce(&SourceTyper<'_>, &DefDef<Untyped>, TreeId<Typed>, SymbolId),
) {
    let text = upstream("library/src/scala/math/Fractional.scala");
    let (parsed, mut store, packages, defs, index, source) =
        parse_and_name_file(&text, "Fractional.scala");
    let ops = class_symbol(&parsed, &store, &index, source, "FractionalOps");
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, name);
    let tree = declaration(&index, method);
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        unreachable!()
    };
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let typed = typer.type_method_definition(tree).unwrap();
    assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
    let body = typer.source_typed_index().get(source, rhs).unwrap();
    check(&typer, def, body, ops);
    outcome("match");
}

fn parameter(typer: &SourceTyper<'_>, def: &DefDef<Untyped>, clause: usize) -> SymbolId {
    let [parameter] = def.value_param_clauses[clause][..] else {
        panic!("one formal parameter")
    };
    typer.index.symbol_at(typer.source, parameter).unwrap()
}

fn source_declaration(typer: &SourceTyper<'_>, owner: SymbolId, name: &str) -> SymbolId {
    let mut symbols = typer
        .arena
        .iter()
        .filter_map(|(id, _)| typer.index.symbol_at(typer.source, id))
        .filter(|&symbol| {
            let entry = typer.store.symbols.get(symbol);
            entry.owner == Some(owner) && typer.store.names.resolve(entry.name.text()) == name
        });
    let symbol = symbols.next().expect("source declaration");
    assert!(symbols.next().is_none(), "unique {name} in its owner");
    symbol
}

#[test]
#[ignore = "requires the pinned Scala 3.9 sources and classpath"]
fn ops_division() {
    with_body("/", |typer, def, body, ops| {
        let fractional = typer.store.symbols.owner(ops).unwrap();
        let TreeKind::Apply(call) = &typer.typed_ast().get(body).kind else {
            panic!("div application")
        };
        assert_eq!(
            referent(typer, call.function),
            source_declaration(typer, fractional, "div")
        );
        let [lhs, rhs] = call.args[..] else {
            panic!("two div arguments")
        };
        assert_eq!(referent(typer, lhs), source_declaration(typer, ops, "lhs"));
        assert_eq!(referent(typer, rhs), parameter(typer, def, 0));
        assert_eq!(
            typer
                .store
                .types
                .get(typer.typed_ast().get(body).ty)
                .reference_symbol(),
            Some(source_declaration(typer, fractional, "T"))
        );
    });
}

fn constructed_prefix(
    typer: &SourceTyper<'_>,
    def: &DefDef<Untyped>,
    body: TreeId<Typed>,
    ops: SymbolId,
) -> TypeId {
    let ast = typer.typed_ast();
    let TreeKind::Apply(call) = &ast.get(body).kind else {
        panic!("constructor application")
    };
    let [arg] = call.args[..] else {
        panic!("one source constructor argument")
    };
    assert_eq!(referent(typer, arg), parameter(typer, def, 0));
    let ctor = typer.store.symbols.get(referent(typer, call.function));
    assert_eq!(ctor.kind, SymbolKind::Constructor);
    assert_eq!(ctor.owner, Some(ops));
    let TreeKind::Select(init) = &ast.get(call.function).kind else {
        panic!("constructor selection")
    };
    let new = ast.get(init.qualifier);
    assert!(matches!(new.kind, TreeKind::New(_)));
    let Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(class),
    } = *typer.store.types.get(new.ty)
    else {
        panic!("qualified nested class")
    };
    assert_eq!(class, ops);
    let Type::TypeRef {
        prefix: result_prefix,
        target: TypeRefTarget::Symbol(result_class),
    } = *typer.store.types.get(ast.get(body).ty)
    else {
        panic!("qualified result of constructor call")
    };
    assert_eq!(result_class, ops);
    assert_eq!(
        typer.store.types.get(result_prefix),
        typer.store.types.get(prefix)
    );
    prefix
}

#[test]
#[ignore = "requires the pinned Scala 3.9 sources and classpath"]
fn member_factory() {
    with_body("mkNumericOps", |typer, def, body, ops| {
        let prefix = constructed_prefix(typer, def, body, ops);
        let fractional = typer.store.symbols.owner(ops).unwrap();
        assert_eq!(
            *typer.store.types.get(prefix),
            Type::ThisType { class: fractional }
        );
    });
}

fn assert_fractional_t(
    typer: &SourceTyper<'_>,
    applied: TypeId,
    ops: SymbolId,
    def: &DefDef<Untyped>,
) {
    let Type::Applied { tycon, args } = typer.store.types.get(applied) else {
        panic!("expected Fractional[T]")
    };
    let fractional = typer.store.symbols.owner(ops).unwrap();
    assert_eq!(
        typer.store.types.get(*tycon).reference_symbol(),
        Some(fractional)
    );
    let [t] = args[..] else {
        panic!("one enclosing type argument")
    };
    let formal = typer
        .index
        .symbol_at(typer.source, def.type_params[0])
        .unwrap();
    assert_eq!(typer.store.types.get(t).reference_symbol(), Some(formal));
}

#[test]
#[ignore = "requires the pinned Scala 3.9 sources and classpath"]
fn contextual_factory() {
    with_body("infixFractionalOps", |typer, def, body, ops| {
        let prefix = constructed_prefix(typer, def, body, ops);
        let Type::TermRef {
            target: TermRefTarget::Symbol(witness),
            ..
        } = *typer.store.types.get(prefix)
        else {
            panic!("stable term prefix for num.FractionalOps")
        };
        assert_eq!(witness, parameter(typer, def, 1));
        let result = typer.type_index.type_at(typer.source, def.tpt).unwrap();
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        } = *typer.store.types.get(result)
        else {
            panic!("projected Fractional[T]#FractionalOps")
        };
        assert_eq!(class, ops);
        assert_fractional_t(typer, prefix, ops, def);
    });
}

#[test]
#[ignore = "requires the pinned Scala 3.9 sources and classpath"]
fn companion_apply() {
    with_body("apply", |typer, def, body, ops| {
        assert_eq!(referent(typer, body), parameter(typer, def, 0));
        let result = typer.type_index.type_at(typer.source, def.tpt).unwrap();
        assert_fractional_t(typer, result, ops, def);
    });
}
