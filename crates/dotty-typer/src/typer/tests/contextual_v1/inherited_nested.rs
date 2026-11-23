use super::*;

#[test]
#[ignore = "requires the pinned Scala 3.9 classpath"]
fn fractional_numeric_ops_uses_the_subclass_prefix() {
    let fixture =
        include_str!("../../../../tests/fixtures/contextual-v1/controls/FractionalInherited.scala");
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(fixture);
    let child = class_symbol(&parsed, &store, &index, source, "Slice");
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "inherited");
    let tree = declaration(&index, method);
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    typer.type_method_definition(tree).unwrap();
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        unreachable!()
    };
    let TreeKind::ValDef(param) = &parsed.ast.get(def.value_param_clauses[0][0]).kind else {
        unreachable!()
    };
    for tpt in [param.tpt, def.tpt] {
        let ty = typer.type_index.type_at(source, tpt).unwrap();
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(target),
        } = *typer.store.types.get(ty)
        else {
            panic!("expected a nested class type");
        };
        assert_eq!(
            *typer.store.types.get(prefix),
            Type::ThisType { class: child }
        );
        let target = typer.store.symbols.get(target);
        assert!(matches!(target.origin, SymbolOrigin::Classfile(_)));
        assert_eq!(
            typer.store.names.resolve(target.name.text()),
            "Numeric$NumericOps"
        );
        let numeric = typer.store.symbols.get(target.owner.unwrap());
        assert!(matches!(numeric.origin, SymbolOrigin::Tasty(_)));
        assert_eq!(typer.store.names.resolve(numeric.name.text()), "Numeric");
    }
}
