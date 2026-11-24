use super::*;

#[test]
#[ignore = "requires the pinned Scala 3.9 classpath"]
fn inherited_constructor_checks_the_argument_against_the_outer_base_view() {
    let text = r#"
        trait Slice[A] extends scala.math.Numeric[A] {
          def divide(x: A, y: A): A
          class Ops(lhs: A) extends NumericOps(lhs) {
            def quotient(rhs: A): A = divide(lhs, rhs)
          }
        }
        trait IntSlice extends scala.math.Numeric[Int] {
          class Concrete(lhs: Int) extends NumericOps(lhs)
          class WrongArity(lhs: Int) extends NumericOps(lhs, lhs)
          class WrongType(lhs: Boolean) extends NumericOps(lhs)
        }
    "#;
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (method, body) = method_definition_and_rhs(&parsed, &store, &index, source, "quotient");
    let divide = method_symbol(&parsed, &store, &index, source, "divide");
    let slice = class_symbol(&parsed, &store, &index, source, "Slice");
    let ops = store.symbols.owner(method).unwrap();
    let template = |class| {
        let TreeKind::TypeDef(def) = &parsed.ast.get(declaration(&index, class)).kind else {
            panic!("class declaration")
        };
        let TreeKind::Template(template) = &parsed.ast.get(def.rhs).kind else {
            panic!("class template")
        };
        template
    };
    let parent_tree = template(ops).parents[0];
    let TreeKind::DefDef(child_ctor) = &parsed.ast.get(template(ops).constructor).kind else {
        panic!("primary constructor")
    };
    let ctor = index.symbol_at(source, template(ops).constructor).unwrap();
    let lhs = index
        .derived_symbol_at(ctor, source, child_ctor.value_param_clauses[0][0])
        .unwrap();
    let concrete = class_symbol(&parsed, &store, &index, source, "Concrete");
    let wrong_arity = class_symbol(&parsed, &store, &index, source, "WrongArity");
    let wrong_type = class_symbol(&parsed, &store, &index, source, "WrongType");
    let resolver = audit_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    typer
        .type_method_definition(declaration(&index, method))
        .unwrap();
    let body = typer.source_typed_index().get(source, body).unwrap();
    let TreeKind::Apply(application) = &typer.typed_ast().get(body).kind else {
        panic!("expected the enclosing divide method");
    };
    assert_eq!(referent(&typer, application.function), divide);
    let get = |ty| typer.store.types.get(ty);
    let parent = typer.source_typed_index().get(source, parent_tree).unwrap();
    let TreeKind::Apply(call) = &typer.typed_ast().get(parent).kind else {
        panic!("application of the parent constructor")
    };
    assert_eq!(call.args.len(), 1);
    assert_eq!(referent(&typer, call.args[0]), lhs);
    let Type::TermRef {
        prefix: instance,
        target: TermRefTarget::Symbol(ctor),
    } = *get(typer.typed_ast().get(call.function).ty)
    else {
        panic!("constructor selection")
    };
    let Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(numeric_ops),
    } = *get(instance)
    else {
        panic!("inherited nested class")
    };
    assert_eq!(get(prefix), &Type::ThisType { class: slice });
    assert_eq!(typer.store.symbols.owner(ctor), Some(numeric_ops));
    assert_eq!(
        typer
            .store
            .names
            .resolve(typer.store.symbols.get(numeric_ops).name.text()),
        "Numeric$NumericOps"
    );
    typer.complete_symbol(concrete).unwrap();
    let checkpoint = typer.store.checkpoint();
    let error = typer.complete_symbol(wrong_arity).unwrap_err();
    assert!(
        matches!(error, TyperError::ApplicationArityMismatch { tree_index, expected: 1, actual: 2, .. }
            if tree_index == template(wrong_arity).parents[0].index()),
        "{error:?}"
    );
    assert_eq!(typer.store.checkpoint(), checkpoint);
    let call = template(wrong_type).parents[0];
    let error = typer.complete_symbol(wrong_type).unwrap_err();
    assert!(
        matches!(&error, TyperError::ApplicationArgumentTypeMismatch {
            source: site, tree_index, argument_index: 0, actual, expected,
            actual_description, expected_description
        } if (*site, *tree_index, *actual, *expected) == (source, call.index(), defs.boolean, defs.int)
            && actual_description.as_ref() == "Boolean" && expected_description.as_ref() == "Int"),
        "{error:?}"
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, template(wrong_type).parents[0])
            .is_none()
    );
    assert_eq!(typer.store.checkpoint(), checkpoint);
}
