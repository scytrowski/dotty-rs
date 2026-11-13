use super::*;

fn generated_apply_index_with_role(
    parsed: &dotty_parser::ParseResult,
    store: &SemanticStore,
    source: SourceId,
    source_index: &SourceSemanticIndex,
    class: SymbolId,
    apply: SymbolId,
    role: Option<dotty_core::GeneratedCaseClassMethodRole>,
) -> SourceSemanticIndex {
    let SourceDefinition::Canonical {
        source: class_source,
        tree: class_tree,
    } = source_index.definition_of(class).unwrap()
    else {
        panic!("case class has canonical source identity")
    };
    assert_eq!(class_source, source);
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
        panic!("canonical case class tree is a TypeDef")
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!("canonical case class tree has a template")
    };
    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
        panic!("case class template has a constructor")
    };

    let mut index = SourceSemanticIndex::new();
    index.record_symbol(source, class_tree, class).unwrap();
    for parameter_tree in &constructor.type_params {
        let parameter = source_index.symbol_at(source, *parameter_tree).unwrap();
        index
            .record_symbol(source, *parameter_tree, parameter)
            .unwrap();
    }

    let object_owner = store.symbols.get(class).owner.unwrap();
    let module_class = source_index
        .derived_symbol_at(object_owner, source, class_tree)
        .expect("case class has its derived companion module class");
    index
        .record_scope(
            object_owner,
            source_index
                .scope_of(object_owner)
                .expect("owner has a scope"),
        )
        .unwrap();
    index
        .record_scope(
            module_class,
            source_index
                .scope_of(module_class)
                .expect("companion module class has a scope"),
        )
        .unwrap();
    index
        .record_derived_symbol(object_owner, source, class_tree, module_class)
        .unwrap();
    index
        .record_derived_symbol(module_class, source, class_tree, apply)
        .unwrap();
    for parameter_tree in &constructor.type_params {
        let parameter = source_index
            .derived_symbol_at(apply, source, *parameter_tree)
            .expect("generated apply retains its derived type parameter");
        index
            .record_derived_symbol(apply, source, *parameter_tree, parameter)
            .unwrap();
    }
    if let Some(role) = role {
        index
            .record_generated_case_class_method(class, role, source, class_tree, apply)
            .unwrap();
    }
    index
}

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
fn generated_apply_signature_keeps_field_cache_class_owned_in_both_completion_orders() {
    let source_text = concat!(
        "package scala { trait Product }; package sample { ",
        "case class Rightish[+A, +B](value: B); ",
        "case class Leftish[+A, +B](value: A); ",
        "case class Renamed[+ClassSide, +ValueSide](value: ClassSide) }",
    );
    for (class_name, field_parameter_index) in
        [("Rightish", 1usize), ("Leftish", 0), ("Renamed", 0)]
    {
        for class_info_first in [true, false] {
            let (parsed, mut store, packages, definitions, index, source) =
                parse_and_name(source_text);
            let class = class_symbol(&parsed, &store, &index, source, class_name);
            let apply = index
                .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Apply)
                .expect("generated Apply has canonical role identity");
            let SourceDefinition::Canonical {
                tree: class_tree, ..
            } = index.definition_of(class).unwrap()
            else {
                panic!("case class has canonical source tree")
            };
            let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
                panic!("canonical class tree is a TypeDef")
            };
            let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
                panic!("class has a template")
            };
            let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
                panic!("case class has a constructor")
            };
            let class_parameters = constructor
                .type_params
                .iter()
                .map(|tree| index.symbol_at(source, *tree).unwrap())
                .collect::<Vec<_>>();
            let apply_parameters = constructor
                .type_params
                .iter()
                .map(|tree| index.derived_symbol_at(apply, source, *tree).unwrap())
                .collect::<Vec<_>>();
            let field = constructor.value_param_clauses[0][0];
            let TreeKind::ValDef(field_definition) = &parsed.ast.get(field).kind else {
                panic!("case-class constructor parameter is a value definition")
            };
            let field_type_tree = field_definition.tpt;
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            if class_info_first {
                typer.complete_symbol(class).unwrap();
                typer.complete_symbol(apply).unwrap();
            } else {
                typer.complete_symbol(apply).unwrap();
                typer.complete_symbol(class).unwrap();
            }

            assert_eq!(class_parameters.len(), 2);
            assert_eq!(apply_parameters.len(), 2);
            let class_parameter = class_parameters[field_parameter_index];
            let apply_parameter = apply_parameters[field_parameter_index];
            assert_ne!(class_parameter, apply_parameter);
            assert_eq!(typer.store.symbols.get(class_parameter).owner, Some(class));
            assert_eq!(typer.store.symbols.get(apply_parameter).owner, Some(apply));
            assert_eq!(
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(class_parameter).name.text()),
                typer
                    .store
                    .names
                    .resolve(typer.store.symbols.get(apply_parameter).name.text()),
                "source-derived Apply parameters can share spelling but retain distinct owners"
            );
            let cached_field_type = typer
                .source_type_index()
                .type_at(source, field_type_tree)
                .expect("constructor field type is cached after either completion order");
            assert!(matches!(
                typer.store.types.get(cached_field_type),
                Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                    if *symbol == class_parameter
            ));

            let apply_signature = match *typer.store.symbols.info(apply) {
                SymbolInfo::Complete(signature) => signature,
                other => panic!("generated Apply signature is complete: {other:?}"),
            };
            let Type::Poly(poly) = typer.store.types.get(apply_signature) else {
                panic!("generated Apply retains its polymorphic signature")
            };
            assert_eq!(poly.params.len(), 2);
            let Type::Method(method) = typer.store.types.get(poly.result) else {
                panic!("generated Apply signature has a method clause")
            };
            assert_eq!(method.params.len(), 1);
            let Type::ParamRef {
                binder: method_binder,
                index: method_parameter_index,
            } = typer.store.types.get(method.params[0].ty)
            else {
                panic!("generated Apply field parameter references its method type binder")
            };
            assert_eq!(*method_parameter_index as usize, field_parameter_index);
            assert!(matches!(
                typer.store.types.get(method.params[0].ty),
                Type::ParamRef { binder, index }
                    if binder == method_binder && *index as usize == field_parameter_index
            ));
            let Type::Applied { args, .. } = typer.store.types.get(method.result) else {
                panic!("generated Apply result is the applied source class")
            };
            assert!(matches!(
                typer.store.types.get(args[field_parameter_index]),
                Type::ParamRef { binder, index }
                    if binder == method_binder && *index as usize == field_parameter_index
            ));
        }
    }
}

#[test]
fn generated_apply_parameter_first_completion_reuses_canonical_projection() {
    let source_text = concat!(
        "package scala { trait Product }; package sample { ",
        "case class Generic[+A, +B](value: B); ",
        "case class Concrete[+A](value: Int) }",
    );
    for (class_name, field_type_parameter) in [("Generic", Some(1usize)), ("Concrete", None)] {
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
        let class = class_symbol(&parsed, &store, &index, source, class_name);
        let apply = index
            .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Apply)
            .expect("generated Apply has canonical role identity");
        let SourceDefinition::Canonical {
            tree: class_tree, ..
        } = index.definition_of(class).unwrap()
        else {
            panic!("case class has canonical source tree")
        };
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
            panic!("canonical class tree is a TypeDef")
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("class has a template")
        };
        let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
            panic!("case class has a constructor")
        };
        let field_tree = constructor.value_param_clauses[0][0];
        let field_parameter = index
            .derived_symbol_at(apply, source, field_tree)
            .expect("generated Apply parameter has derived source identity");
        let class_parameters = constructor
            .type_params
            .iter()
            .map(|tree| index.symbol_at(source, *tree).unwrap())
            .collect::<Vec<_>>();
        let field_tpt = match &parsed.ast.get(field_tree).kind {
            TreeKind::ValDef(definition) => definition.tpt,
            _ => panic!("constructor parameter tree is a ValDef"),
        };

        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer
            .complete_symbol(field_parameter)
            .expect("generated Apply parameter can complete before its method");
        typer
            .complete_symbol(apply)
            .expect("method completion reuses its completed generated parameter");

        let cached_field_type = typer
            .source_type_index()
            .type_at(source, field_tpt)
            .expect("canonical constructor field type is cached");
        match field_type_parameter {
            Some(index) => assert!(matches!(
                typer.store.types.get(cached_field_type),
                Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }
                    if *symbol == class_parameters[index]
            )),
            None => assert_eq!(cached_field_type, definitions.int),
        }
        assert!(matches!(
            *typer.store.symbols.info(field_parameter),
            SymbolInfo::Complete(_)
        ));
        assert!(matches!(
            *typer.store.symbols.info(apply),
            SymbolInfo::Complete(_)
        ));
    }
}

#[test]
fn generated_apply_requires_matching_forward_and_reverse_role_metadata() {
    let source_text = concat!(
        "package scala { trait Product }; package sample { ",
        "case class Rightish[+A, +B](value: B) }",
    );
    let (parsed, mut store, packages, definitions, source_index, source) =
        parse_and_name(source_text);
    let class = class_symbol(&parsed, &store, &source_index, source, "Rightish");
    let apply = source_index
        .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Apply)
        .expect("Namer publishes the generated apply role");
    let SourceDefinition::Canonical {
        tree: class_tree, ..
    } = source_index.definition_of(class).unwrap()
    else {
        panic!("case class has canonical source identity")
    };
    {
        let typer = SourceTyper::new(
            &parsed.ast,
            source,
            &source_index,
            &mut store,
            definitions,
            &packages,
        );
        assert_eq!(
            typer.generated_case_class_for_apply(apply, source, class_tree),
            Some(class),
            "the canonical forward and reverse Apply association is accepted"
        );
    }

    for role in [
        None,
        Some(dotty_core::GeneratedCaseClassMethodRole::Unapply),
    ] {
        let forged_index = generated_apply_index_with_role(
            &parsed,
            &store,
            source,
            &source_index,
            class,
            apply,
            role,
        );
        let typer = SourceTyper::new(
            &parsed.ast,
            source,
            &forged_index,
            &mut store,
            definitions,
            &packages,
        );
        assert_eq!(
            typer.generated_case_class_for_apply(apply, source, class_tree),
            None,
            "missing or wrong-role metadata cannot authenticate generated Apply: {role:?}"
        );
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
fn late_failure_after_generated_apply_rolls_back_then_retries_in_same_session() {
    let source_text = concat!(
        "package scala { trait Product }; package sample { ",
        "case class Rightish[+A, +B](value: B); ",
        "object Probe { ",
        "def failed[A](value: A): Any = Rightish(value).missing; ",
        "def retry[A](value: A): Rightish[Nothing, A] = Rightish(value) ",
        "} }",
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(source_text);
    let (failed_method, failed_tree, failed_rhs) =
        method_tree_and_rhs(&parsed, &store, &index, source, "failed");
    let (retry_method, retry_rhs) =
        method_definition_and_rhs(&parsed, &store, &index, source, "retry");
    let TreeKind::DefDef(failed_definition) = &parsed.ast.get(failed_tree).kind else {
        panic!("failed method has a canonical DefDef")
    };
    let failed_outer_parameter = index
        .symbol_at(source, failed_definition.type_params[0])
        .expect("failed method type parameter retains source identity");
    let TreeKind::Select(selection) = &parsed.ast.get(failed_rhs).kind else {
        panic!("the generated call is followed by a member-selection failure")
    };
    let generated_call = selection.qualifier;
    assert!(matches!(
        parsed.ast.get(generated_call).kind,
        TreeKind::Apply(_)
    ));
    let class = class_symbol(&parsed, &store, &index, source, "Rightish");
    let apply = index
        .generated_case_class_method(class, dotty_core::GeneratedCaseClassMethodRole::Apply)
        .unwrap();
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
    let before_store = typer.store().checkpoint();
    let before_resolver = typer.resolver.checkpoint();
    let before_typed = typer.typed_ast().checkpoint();
    let before_type_cache = format!("{:?}", typer.type_index.checkpoint());
    let before_state = format!(
        "store={:?};typed_arena={:?};typed_index={:?};resolver={:?};type_cache={};local_scopes={:?};import_members={:?};import_qualifiers={:?}",
        typer.store(),
        typer.typed_ast(),
        typer.typed_index.len(),
        typer.resolver.checkpoint(),
        before_type_cache,
        typer.local_scopes,
        typer.import_member_cache,
        typer.import_qualifier_cache,
    );
    let observed_generated_apply = Rc::new(RefCell::new(false));
    let observed = Rc::clone(&observed_generated_apply);
    typer.transaction_failure_error_observer = Some(Rc::new(move |typer, _error| {
        let typed_call = typer
            .source_typed_index()
            .get(source, generated_call)
            .expect("the generated call subtree completed before the later failure");
        let TreeKind::Apply(application) = &typer.typed_ast().get(typed_call).kind else {
            panic!("the successful source subtree remains an Apply before rollback")
        };
        let function = typer.typed_ast().get(application.function);
        assert!(matches!(
            typer.store().types.get(function.ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == apply
        ));
        let Type::Applied { tycon, args } = typer
            .store()
            .types
            .get(typer.typed_ast().get(typed_call).ty)
        else {
            panic!("the generated call has an applied class result before rollback")
        };
        assert!(matches!(
            typer.store().types.get(*tycon),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == class
        ));
        assert_eq!(args[0], definitions.nothing_type);
        assert!(matches!(
            typer.store().types.get(args[1]),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == failed_outer_parameter
        ));
        *observed.borrow_mut() = true;
    }));
    let failure = typer.type_expression(failed_rhs, failed_context);
    assert!(
        matches!(failure, Err(TyperError::UnstableSelectionPrefix { .. })),
        "generated call must be followed by the intended selection error: {failure:?}"
    );
    assert!(*observed_generated_apply.borrow());
    assert_eq!(typer.store().checkpoint(), before_store);
    assert_eq!(typer.resolver.checkpoint(), before_resolver);
    assert_eq!(typer.typed_ast().checkpoint(), before_typed);
    assert_eq!(
        format!("{:?}", typer.type_index.checkpoint()),
        before_type_cache
    );
    assert_eq!(
        format!(
            "store={:?};typed_arena={:?};typed_index={:?};resolver={:?};type_cache={};local_scopes={:?};import_members={:?};import_qualifiers={:?}",
            typer.store(),
            typer.typed_ast(),
            typer.typed_index.len(),
            typer.resolver.checkpoint(),
            before_type_cache,
            typer.local_scopes,
            typer.import_member_cache,
            typer.import_qualifier_cache,
        ),
        before_state
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, failed_tree)
            .is_none()
    );
    assert!(typer.source_typed_index().get(source, failed_rhs).is_none());
    assert!(
        typer
            .source_typed_index()
            .get(source, generated_call)
            .is_none()
    );

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
