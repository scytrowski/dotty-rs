use super::*;

fn method_tree_and_rhs(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    index: &SourceSemanticIndex,
    source: SourceId,
    name: &str,
) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
    let method = method_symbol(parsed, store, index, source, name);
    let SourceDefinition::Canonical { tree, .. } = index.definition_of(method).unwrap() else {
        panic!("method must have canonical source identity")
    };
    let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
        panic!("method symbol must map to a DefDef")
    };
    let rhs = definition.rhs.expect("method should have a body");
    (method, tree, rhs)
}

#[test]
fn generated_apply_infers_only_the_unused_covariant_slot_in_both_positions() {
    let source_text = concat!(
        "package scala { trait Product }; ",
        "package sample { ",
        "case class Rightish[+A, +B](value: B); ",
        "case class Leftish[+A, +B](value: A); ",
        "object Probe { ",
        "def right[A](value: A): Rightish[Nothing, A] = Rightish.apply(value); ",
        "def left[B](value: B): Leftish[B, Nothing] = Leftish(value) ",
        "} }",
    );
    for (method_name, class_name, bottom_index, concrete_index) in
        [("right", "Rightish", 0, 1), ("left", "Leftish", 1, 0)]
    {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, method_tree, rhs) =
            method_tree_and_rhs(&parsed, &store, &index, source, method_name);
        let TreeKind::DefDef(definition) = &parsed.ast.get(method_tree).kind else {
            panic!("method symbol must map to a DefDef")
        };
        let outer_parameter = index
            .symbol_at(source, definition.type_params[0])
            .expect("outer method type parameter should retain source identity");
        let expected_class = class_symbol(&parsed, &store, &index, source, class_name);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let typed_rhs = typer.type_expression(rhs, context).unwrap();
        let rhs_type = typer.typed_ast().get(typed_rhs).ty;
        let Type::Applied { tycon, args } = typer.store().types.get(rhs_type) else {
            panic!("generated apply result should remain an applied case-class type")
        };
        assert_eq!(args.len(), 2);
        assert_eq!(args[bottom_index], definitions.nothing_type);
        assert!(matches!(
            typer.store().types.get(args[concrete_index]),
            Type::TypeRef { target: TypeRefTarget::Symbol(parameter), .. }
                if *parameter == outer_parameter
        ));
        assert!(matches!(
            typer.store().types.get(*tycon),
            Type::TypeRef { target: TypeRefTarget::Symbol(class), .. }
                if *class == expected_class
        ));
    }
}

#[test]
fn unrelated_factories_and_unsupported_generated_shapes_keep_missing_slots_unconstrained() {
    let cases = [
        (
            concat!(
                "package sample { object UserFactory { ",
                "def apply[A, B](value: A): A = value ",
                "}; object Probe { def user[A](value: A): Any = UserFactory.apply(value) } }",
            ),
            "user",
            Some(1),
        ),
        (
            concat!(
                "package scala { trait Product }; package sample { ",
                "case class Bounded[+A >: Null, +B](value: B); ",
                "object Probe { def bounded[B](value: B): Any = Bounded(value) } }",
            ),
            "bounded",
            None,
        ),
        (
            concat!(
                "package scala { trait Product }; package sample { ",
                "class Box[+A]; case class Nested[+A, +B](value: Box[A]); ",
                "object Probe { def nested[A](value: Box[A]): Any = Nested(value) } }",
            ),
            "nested",
            Some(1),
        ),
    ];

    for (source_text, method_name, parameter_index) in cases {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method_name);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let context = typer.expression_context_for(method).unwrap();
        let result = typer.type_expression(rhs, context);
        match parameter_index {
            Some(parameter_index) => assert!(
                matches!(
                    result,
                    Err(TyperError::UnconstrainedTypeParameter { parameter_index: found, .. })
                        if found == parameter_index
                ),
                "unsupported generated apply returned {result:?} for {method_name}"
            ),
            None => assert!(
                result.is_err(),
                "noncanonical bounds must not be minimized for {method_name}"
            ),
        }
        assert!(typer.typed_ast().iter().next().is_none());
    }
}

#[test]
fn overloaded_generated_apply_does_not_receive_the_single_candidate_exception() {
    let source_text = concat!(
        "package scala { trait Product }; package sample { ",
        "case class Competing[+A, +B](value: B); ",
        "object Competing { def apply[A, B](value: String): Competing[A, B] = ??? }; ",
        "object Probe { def use: Any = Competing(\"value\") } }",
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let context = typer.expression_context_for(method).unwrap();
    assert!(typer.type_expression(rhs, context).is_err());
    assert!(typer.typed_ast().iter().next().is_none());
}

#[test]
fn failed_unconstrained_call_rolls_back_before_a_generated_apply_retry() {
    let source_text = concat!(
        "package scala { trait Product }; package sample { ",
        "case class Rightish[+A, +B](value: B); ",
        "object Probe { ",
        "def make[A](): A = ???; ",
        "def failed: Any = make(); ",
        "def retry[A](value: A): Rightish[Nothing, A] = Rightish(value) ",
        "} }",
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let (failed_method, failed_rhs) =
        method_definition_and_rhs(&parsed, &store, &index, source, "failed");
    let (retry_method, retry_rhs) =
        method_definition_and_rhs(&parsed, &store, &index, source, "retry");
    let TreeKind::DefDef(retry_definition) = &parsed
        .ast
        .get(match index.definition_of(retry_method).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => panic!("retry method should have canonical source identity"),
        })
        .kind
    else {
        panic!("retry method should map to a DefDef")
    };
    let outer_parameter = index
        .symbol_at(source, retry_definition.type_params[0])
        .expect("retry type parameter should retain source identity");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let failed_context = typer.expression_context_for(failed_method).unwrap();
    let before = typer.store().checkpoint();
    assert!(matches!(
        typer.type_expression(failed_rhs, failed_context),
        Err(TyperError::UnconstrainedTypeParameter { .. })
    ));
    assert_eq!(typer.store().checkpoint(), before);
    assert!(typer.typed_ast().iter().next().is_none());

    let retry_context = typer.expression_context_for(retry_method).unwrap();
    let typed = typer.type_expression(retry_rhs, retry_context).unwrap();
    let Type::Applied { args, .. } = typer.store().types.get(typer.typed_ast().get(typed).ty)
    else {
        panic!("retry should produce the generated case-class application")
    };
    assert_eq!(args[0], definitions.nothing_type);
    assert!(matches!(
        typer.store().types.get(args[1]),
        Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == outer_parameter
    ));
}

#[test]
fn generated_apply_does_not_minimize_invariant_or_contravariant_missing_slots() {
    let source_text = concat!(
        "package scala { trait Product }; ",
        "package sample { ",
        "case class Invariant[A, B](value: A); ",
        "case class Contravariant[-A, +B](value: B); ",
        "object Probe { ",
        "def invariant[A](value: A): Invariant[A, Int] = Invariant(value); ",
        "def contravariant[B](value: B): Contravariant[Int, B] = Contravariant(value) ",
        "} }",
    );
    for method_name in ["invariant", "contravariant"] {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let (_method, method_tree, rhs) =
            method_tree_and_rhs(&parsed, &store, &index, source, method_name);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        assert!(matches!(
            typer.type_method_definition(method_tree),
            Err(TyperError::UnconstrainedTypeParameter { .. })
        ));
        assert!(typer.source_typed_index().get(source, rhs).is_none());
    }
}
