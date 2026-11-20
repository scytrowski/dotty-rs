use super::*;

fn typed_reference(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    typer
        .store
        .types
        .get(typer.typed_arena.get(tree).ty)
        .reference_symbol()
        .unwrap()
}

#[test]
fn contextual_extension_selects_the_inner_provider_and_applies_its_using_clause() {
    let fixture = include_str!("../../../tests/fixtures/contextual-consumers/Extensions.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let param = |clause: usize, n: usize| {
        index
            .symbol_at(source, def.value_param_clauses[clause][n])
            .unwrap()
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.type_method_definition(tree).unwrap();
    let typed = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(using_call) = &typer.typed_arena.get(typed).kind else {
        panic!()
    };
    assert_eq!(typed_reference(&typer, using_call.args[0]), param(1, 1));
    let TreeKind::Apply(ordinary) = &typer.typed_arena.get(using_call.function).kind else {
        panic!()
    };
    let TreeKind::Apply(receiver) = &typer.typed_arena.get(ordinary.function).kind else {
        panic!()
    };
    assert_eq!(typed_reference(&typer, receiver.args[0]), param(0, 0));
    let TreeKind::Select(extension) = &typer.typed_arena.get(receiver.function).kind else {
        panic!()
    };
    assert_eq!(typed_reference(&typer, extension.qualifier), param(1, 0));
    assert_eq!(typer.store.names.resolve(extension.name.text()), "render");
    let (property, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "property");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(property) else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let typed = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(evidence) = &typer.typed_arena.get(typed).kind else {
        panic!()
    };
    let TreeKind::Apply(base) = &typer.typed_arena.get(evidence.function).kind else {
        panic!()
    };
    let TreeKind::Select(selected) = &typer.typed_arena.get(base.function).kind else {
        panic!()
    };
    assert_eq!(typer.store.names.resolve(selected.name.text()), "size");
    assert_eq!(base.args.len(), 1);
    assert_eq!(evidence.args.len(), 1);
}

#[test]
fn equally_nested_contextual_extensions_report_the_conflicting_providers() {
    let fixture = include_str!("../../../tests/fixtures/contextual-consumers/Extensions.scala");
    let text = fixture.replace(
        "inner: Ops, fmt: Format",
        "first: Ops, second: Ops, fmt: Format",
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "choose");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let expected: Vec<_> = def.value_param_clauses[1][..2]
        .iter()
        .map(|p| index.symbol_at(source, *p).unwrap())
        .collect();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let checkpoint = typer.store.checkpoint();
    assert!(
        matches!(typer.type_method_definition(tree),Err(TyperError::AmbiguousContextualArgument {candidates,..}) if candidates==expected)
    );
    assert_eq!(typer.store.checkpoint(), checkpoint);
    assert!(typer.typed_index.is_empty());
}

#[test]
fn contextual_conversion_uses_canonical_scala_conversion_in_result_and_argument_positions() {
    let text = include_str!("../../../tests/fixtures/contextual-consumers/Conversions.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = super::contextual_binary::library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for name in ["result", "argument"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let formal = |clause: usize| {
            index
                .symbol_at(source, def.value_param_clauses[clause][0])
                .unwrap()
        };
        typer.type_method_definition(tree).unwrap();
        let mut adapted = typer.typed_index.get(source, rhs).unwrap();
        if name == "argument" {
            let TreeKind::Apply(accept) = &typer.typed_arena.get(adapted).kind else {
                panic!("expected accept")
            };
            adapted = accept.args[0];
        }
        let TreeKind::Apply(app) = &typer.typed_arena.get(adapted).kind else {
            panic!("expected conversion")
        };
        let TreeKind::Select(selected) = &typer.typed_arena.get(app.function).kind else {
            panic!()
        };
        assert_eq!(typed_reference(&typer, selected.qualifier), formal(1));
        assert_eq!(typed_reference(&typer, app.args[0]), formal(0));
        let apply = typer
            .store
            .symbols
            .get(typed_reference(&typer, app.function));
        assert_eq!(typer.store.names.resolve(apply.name.text()), "apply");
        assert!(matches!(apply.origin, SymbolOrigin::Tasty(_)));
    }
}

#[test]
fn contextual_conversion_reports_ambiguity_and_ignores_an_unrelated_conversion_class() {
    for canonical in [true, false] {
        let text = if canonical {
            "class From; class To; class C { def result(x: From)(using a: Conversion[From, To], b: Conversion[From, To]): To = x }"
        } else {
            "package decoy; class From; class To; class Conversion[A,B] { def apply(x:A):B }; class C { def result(x: From)(using a: Conversion[From, To]): To = x }"
        };
        let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
        let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "result");
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let resolver = super::contextual_binary::library_resolver(definitions, &packages);
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(resolver));
        let before = typer.store.checkpoint();
        let error = typer.type_method_definition(tree).unwrap_err();
        assert!(
            match error {
                TyperError::AmbiguousContextualArgument { ref candidates, .. } =>
                    canonical && candidates.len() == 2,
                TyperError::ExpectedExpressionTypeMismatch { .. } => !canonical,
                _ => false,
            },
            "{error:?}"
        );
        assert_eq!(typer.store.checkpoint(), before);
        assert!(typer.typed_index.is_empty());
    }
}

#[test]
fn legacy_conversion_respects_nesting_and_conversion_instance_priority() {
    let text = include_str!("../../../tests/fixtures/contextual-consumers/LegacyConversions.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = super::contextual_binary::library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for (name, target) in [
        ("oldOnly", "old"),
        ("nearest", "inner"),
        ("mixed", "instance"),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        typer.type_method_definition(tree).unwrap();
        let root = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(app) = &typer.typed_arena.get(root).kind else {
            panic!("missing conversion")
        };
        let converted_by = if target == "old" {
            app.function
        } else {
            let TreeKind::Select(member) = &typer.typed_arena.get(app.function).kind else {
                panic!(
                    "{name} expected Conversion.apply, got {:?}",
                    typer.typed_arena.get(app.function)
                )
            };
            member.qualifier
        };
        let selected = typer
            .store
            .symbols
            .get(typed_reference(&typer, converted_by));
        assert_eq!(typer.store.names.resolve(selected.name.text()), target);
    }
}

#[test]
fn equally_nested_legacy_conversions_report_both_symbols_and_restore_the_store() {
    let text = "class From; class To; class C { implicit def first(x: From): To = new To; implicit def second(x: From): To = new To; def result(x: From): To = x }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "result");
    let candidates: Vec<_> = ["first", "second"]
        .map(|name| method_definition_and_rhs(&parsed, &store, &index, source, name).0)
        .into();
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let checkpoint = typer.store.checkpoint();
    let error = typer.type_method_definition(tree).unwrap_err();
    assert!(
        matches!(&error,TyperError::AmbiguousContextualArgument{candidates:actual,..} if actual==&candidates),
        "{error:?}"
    );
    assert_eq!(typer.store.checkpoint(), checkpoint);
    assert!(typer.typed_index.is_empty());
}

const MEMBER_CONVERSIONS: &str =
    include_str!("../../../tests/fixtures/contextual-consumers/MemberConversions.scala");

fn member_conversion_fixture(extra: &str) -> String {
    format!(
        "{MEMBER_CONVERSIONS}\n{}",
        extra.strip_prefix("package memberconversions").unwrap()
    )
}

#[test]
fn member_conversion_keeps_direct_members_and_reports_the_original_missing_name() {
    let text = member_conversion_fixture(include_str!(
        "../../../tests/fixtures/contextual-consumers/MemberConversionMissing.scala"
    ));
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&text);
    let own = class_symbol(&parsed, &store, &index, source, "Own");
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "direct");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let typed = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Select(select) = &typer.typed_arena.get(typed).kind else {
        panic!("direct selection")
    };
    assert_eq!(
        typer.store.symbols.owner(typed_reference(&typer, typed)),
        Some(own)
    );
    assert_eq!(
        typed_reference(&typer, select.qualifier),
        method_parameter_symbol(&parsed, &index, source, method, 0)
    );

    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "absent");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let before = typer.store.checkpoint();
    assert!(
        matches!(typer.type_method_definition(tree), Err(TyperError::MemberNotFound {tree_index,name,..})
        if tree_index==rhs.index() && typer.store.names.resolve(name.text())=="show")
    );
    assert_eq!(typer.store.checkpoint(), before);
    assert!(typer.typed_index.get(source, rhs).is_none());
}

#[test]
fn member_conversion_selects_apply_and_the_destination_member() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(MEMBER_CONVERSIONS);
    let dest = class_symbol(&parsed, &store, &index, source, "To");
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "converted");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let typed = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Select(show) = &typer.typed_arena.get(typed).kind else {
        panic!("converted selection")
    };
    assert_eq!(typer.store.names.resolve(show.name.text()), "show");
    assert_eq!(
        typer.store.symbols.owner(typed_reference(&typer, typed)),
        Some(dest)
    );
    let TreeKind::Apply(convert) = &typer.typed_arena.get(show.qualifier).kind else {
        panic!("conversion application")
    };
    let [arg] = convert.args[..] else {
        panic!("single receiver")
    };
    assert_eq!(
        typed_reference(&typer, arg),
        method_parameter_symbol(&parsed, &index, source, method, 0)
    );
    let TreeKind::Select(apply) = &typer.typed_arena.get(convert.function).kind else {
        panic!("Conversion.apply")
    };
    assert_eq!(typer.store.names.resolve(apply.name.text()), "apply");
    assert_eq!(
        typed_reference(&typer, apply.qualifier),
        index
            .symbol_at(source, def.value_param_clauses[1][0])
            .unwrap()
    );
}

#[test]
fn member_conversion_ambiguity_names_both_conversions_and_rolls_back() {
    let text = member_conversion_fixture(include_str!(
        "../../../tests/fixtures/contextual-consumers/MemberConversionAmbiguous.scala"
    ));
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&text);
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "ambiguous");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let expected: Vec<_> = def.value_param_clauses[1]
        .iter()
        .map(|&p| index.symbol_at(source, p).unwrap())
        .collect();
    let before = typer.store.checkpoint();
    assert!(
        matches!(typer.type_method_definition(tree),Err(TyperError::AmbiguousMemberConversion{source:at,tree_index,name,candidates,..})
        if at==source && tree_index==rhs.index() && typer.store.names.resolve(name.text())=="show" && candidates==expected)
    );
    assert_eq!(typer.store.checkpoint(), before);
    assert!(typer.typed_index.is_empty());
}

#[test]
fn member_conversion_uses_the_structural_given_in_the_receiver_companion() {
    let text =
        include_str!("../../../tests/fixtures/contextual-consumers/MemberConversionScope.scala");
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let dest = class_symbol(&parsed, &store, &index, source, "To");
    let from = class_symbol(&parsed, &store, &index, source, "From");
    let companion = store.symbols.get(from).links.companion.unwrap();
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "fromCompanion");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let selected = typer.typed_index.get(source, rhs).unwrap();
    assert_eq!(
        typer.store.symbols.owner(typed_reference(&typer, selected)),
        Some(dest)
    );
    let TreeKind::Select(show) = &typer.typed_arena.get(selected).kind else {
        panic!("show")
    };
    let TreeKind::Apply(call) = &typer.typed_arena.get(show.qualifier).kind else {
        panic!("converted receiver")
    };
    let TreeKind::Select(apply) = &typer.typed_arena.get(call.function).kind else {
        panic!("apply")
    };
    let TreeKind::Select(render) = &typer.typed_arena.get(apply.qualifier).kind else {
        panic!("companion given")
    };
    assert_eq!(typer.store.names.resolve(render.name.text()), "render");
    assert_eq!(typed_reference(&typer, render.qualifier), companion);

    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "fromScope");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let root = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Select(show) = &typer.typed_arena.get(root).kind else {
        panic!("show")
    };
    let TreeKind::Apply(call) = &typer.typed_arena.get(show.qualifier).kind else {
        panic!("converted receiver")
    };
    let TreeKind::Select(apply) = &typer.typed_arena.get(call.function).kind else {
        panic!("apply")
    };
    assert_eq!(
        typed_reference(&typer, apply.qualifier),
        index
            .symbol_at(source, def.value_param_clauses[1][1])
            .unwrap()
    );
}

const GENERIC_LEGACY: &str =
    include_str!("../../../tests/fixtures/contextual-consumers/GenericLegacyConversions.scala");

#[test]
fn generic_legacy_conversion_keeps_inferred_arguments_and_receiver_identity() {
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(GENERIC_LEGACY);
    let resolver = super::contextual_binary::library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for (name, conversion, type_args) in [
        ("result", "wrap", vec![definitions.int]),
        ("argument", "wrap", vec![definitions.int]),
        ("viaCompanion", "fromParcel", vec![definitions.int]),
        ("inherited", "wrap", vec![definitions.int]),
        (
            "fromExpected",
            "make",
            vec![definitions.int, definitions.boolean],
        ),
        ("split", "pair", vec![definitions.int, definitions.boolean]),
        ("specialized", "special", vec![]),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let target = method_symbol(&parsed, typer.store(), &index, source, conversion);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let receiver = index
            .symbol_at(source, def.value_param_clauses[0][0])
            .unwrap();
        typer
            .type_method_definition(tree)
            .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        let expression = if name == "argument" {
            let TreeKind::Apply(call) = &parsed.ast.get(rhs).kind else {
                panic!()
            };
            call.args[0]
        } else {
            rhs
        };
        let adapted = typer.typed_index.get(source, expression).unwrap();
        let TreeKind::Apply(call) = &typer.typed_arena.get(adapted).kind else {
            panic!()
        };
        assert_eq!(typed_reference(&typer, call.args[0]), receiver);
        let (class, args): (_, &[TypeId]) = match name {
            "fromExpected" => ("Wrapped", &[definitions.boolean]),
            "split" => ("Impl", &[definitions.int, definitions.boolean]),
            _ => ("Wrapped", &[definitions.int]),
        };
        let Type::Applied {
            tycon,
            args: actual,
        } = typer.store.types.get(typer.typed_arena.get(adapted).ty)
        else {
            panic!("expected instantiated result")
        };
        let symbol = typer.store.types.get(*tycon).reference_symbol().unwrap();
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(symbol).name.text()),
            class
        );
        assert_eq!(actual, args);
        let callee = if type_args.is_empty() {
            call.function
        } else {
            let TreeKind::TypeApply(poly) = &typer.typed_arena.get(call.function).kind else {
                panic!("{name}: expected TypeApply")
            };
            let inferred: Vec<_> = poly
                .args
                .iter()
                .map(|arg| typer.typed_arena.get(*arg).ty)
                .collect();
            assert_eq!(inferred, type_args);
            poly.function
        };
        assert_eq!(typed_reference(&typer, callee), target);
    }
}

#[test]
fn conversion_search_keeps_a_user_import_named_implicit_conversions() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(GENERIC_LEGACY);
    let (field, _) = val_symbol(&parsed, &store, &index, source, "implicitConversions");
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "viaNamesake");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    typer.type_method_definition(tree).unwrap();
    let adapted = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(call) = &typer.typed_arena.get(adapted).kind else {
        panic!("expected conversion")
    };
    let TreeKind::Select(apply) = &typer.typed_arena.get(call.function).kind else {
        panic!("expected Conversion.apply")
    };
    assert_eq!(typer.store.names.resolve(apply.name.text()), "apply");
    assert_eq!(typed_reference(&typer, apply.qualifier), field);
}

#[test]
fn generic_legacy_conversion_rejects_mismatch_bounds_and_ambiguity_without_partial_trees() {
    let rejected =
        include_str!("../../../tests/fixtures/contextual-consumers/GenericLegacyRejected.scala");
    let text = format!(
        "{GENERIC_LEGACY}\n{}",
        rejected.strip_prefix("package genericconversions").unwrap()
    );
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(&text);
    let candidates =
        ["first", "second"].map(|name| method_symbol(&parsed, &store, &index, source, name));
    let resolver = super::contextual_binary::library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for name in [
        "wrongCompanionResult",
        "wrongResult",
        "outsideBound",
        "ambiguous",
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let checkpoint = typer.store.checkpoint();
        let err = typer.type_method_definition(tree).unwrap_err();
        assert!(
            match &err {
                TyperError::AmbiguousContextualArgument {
                    candidates: actual,
                    tree_index,
                    ..
                } => name == "ambiguous" && actual == &candidates && *tree_index == rhs.index(),
                TyperError::ExpectedExpressionTypeMismatch { tree_index, .. } =>
                    name != "ambiguous" && *tree_index == rhs.index(),
                _ => false,
            },
            "{name}: {err:?}"
        );
        assert_eq!(typer.store.checkpoint(), checkpoint);
        assert!(typer.typed_index.is_empty());
    }
}

const LEGACY_MEMBERS: &str =
    include_str!("../../../tests/fixtures/contextual-consumers/LegacyMemberConversions.scala");

#[test]
fn legacy_member_conversion_infers_from_the_receiver_and_selects_the_instantiated_member() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LEGACY_MEMBERS);
    let wrapped = class_symbol(&parsed, &store, &index, source, "Wrapped");
    let box_class = class_symbol(&parsed, &store, &index, source, "Box");
    let companion = store.symbols.get(box_class).links.companion.unwrap();
    // A legacy method needs no scala.Conversion class on the classpath.
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (name, conversion) in [
        ("converted", "wrap"),
        ("fromScope", "local"),
        ("specialized", "mono"),
        ("keep", ""),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let arg = method_parameter_symbol(&parsed, &index, source, method, 0);
        typer
            .type_method_definition(tree)
            .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        let root = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Select(member) = &typer.typed_arena.get(root).kind else {
            panic!("selection")
        };
        if name == "keep" {
            assert_eq!(typed_reference(&typer, member.qualifier), arg);
            assert_eq!(
                typer.store.symbols.owner(typed_reference(&typer, root)),
                Some(box_class)
            );
            continue;
        }
        assert_eq!(typer.store.names.resolve(member.name.text()), "show");
        assert_eq!(
            typer.store.symbols.owner(typed_reference(&typer, root)),
            Some(wrapped)
        );
        let adapted = typer.typed_arena.get(member.qualifier);
        assert!(
            matches!(typer.store.types.get(adapted.ty), Type::Applied { tycon, args }
            if typer.store.types.get(*tycon).reference_symbol() == Some(wrapped) && args == &[defs.int])
        );
        let TreeKind::Apply(call) = &adapted.kind else {
            panic!("conversion application")
        };
        assert_eq!(call.args.len(), 1);
        assert_eq!(typed_reference(&typer, call.args[0]), arg);
        let method = method_symbol(&parsed, typer.store(), &index, source, conversion);
        if name == "specialized" {
            assert_eq!(typed_reference(&typer, call.function), method);
            continue;
        }
        let TreeKind::TypeApply(poly) = &typer.typed_arena.get(call.function).kind else {
            panic!("inferred application")
        };
        assert_eq!(poly.args.len(), 1);
        assert_eq!(typer.typed_arena.get(poly.args[0]).ty, defs.int);
        assert_eq!(typed_reference(&typer, poly.function), method);
        if name == "converted" {
            let TreeKind::Select(sel) = &typer.typed_arena.get(poly.function).kind else {
                panic!("companion method")
            };
            assert_eq!(typed_reference(&typer, sel.qualifier), companion);
        }
    }
}

#[test]
fn legacy_member_conversion_prefers_a_conversion_value_in_the_same_scope() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(LEGACY_MEMBERS);
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "mixed");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let root = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Select(show) = &typer.typed_arena.get(root).kind else {
        panic!()
    };
    let TreeKind::Apply(call) = &typer.typed_arena.get(show.qualifier).kind else {
        panic!()
    };
    let TreeKind::Select(apply) = &typer.typed_arena.get(call.function).kind else {
        panic!()
    };
    assert_eq!(typer.store.names.resolve(apply.name.text()), "apply");
    let given = typer
        .store
        .symbols
        .get(typed_reference(&typer, apply.qualifier));
    assert_eq!(typer.store.names.resolve(given.name.text()), "modern");
}

#[test]
fn legacy_member_conversion_preserves_missing_names_ambiguity_and_rollback() {
    let rejected =
        include_str!("../../../tests/fixtures/contextual-consumers/LegacyMemberRejected.scala");
    let text = format!(
        "{LEGACY_MEMBERS}\n{}",
        rejected.strip_prefix("package legacymembers").unwrap()
    );
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&text);
    let expected =
        ["first", "second"].map(|name| method_symbol(&parsed, &store, &index, source, name));
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (name, target) in [
        ("privateMember", "hidden"),
        ("absent", "absent"),
        ("ambiguous", "show"),
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let before = typer.store.checkpoint();
        let err = typer.type_method_definition(tree).unwrap_err();
        let (site, missing) = match err {
            TyperError::AmbiguousMemberConversion {
                tree_index,
                name,
                candidates,
                ..
            } if target == "show" => {
                assert_eq!(candidates, expected);
                (tree_index, name)
            }
            TyperError::MemberNotFound {
                tree_index, name, ..
            } if target != "show" => (tree_index, name),
            err => panic!("{target}: {err:?}"),
        };
        assert_eq!(site, rhs.index());
        assert_eq!(typer.store.names.resolve(missing.text()), target);
        assert_eq!(typer.store.checkpoint(), before);
        assert!(typer.typed_index.is_empty());
    }
}

const CONDITIONAL_MEMBER: &str =
    include_str!("../../../tests/fixtures/contextual-consumers/ConditionalMemberConversion.scala");

#[test]
fn conditional_member_conversion_applies_the_factory_with_call_site_evidence() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(CONDITIONAL_MEMBER);
    let dest = class_symbol(&parsed, &store, &index, source, "Wrapped");
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    for (name, provider) in [
        ("converted", "wrap"),
        ("mixed", "modern"),
        ("fromDerived", "inheritedAlternative"),
    ] {
        let wrap = method_symbol(&parsed, typer.store(), &index, source, provider);
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let symbol = |clause: usize| {
            index
                .symbol_at(source, def.value_param_clauses[clause][0])
                .unwrap()
        };
        typer.type_method_definition(tree).unwrap();
        let root = typer.typed_index.get(source, rhs).unwrap();
        let ast = typer.typed_ast();
        let TreeKind::Select(show) = &ast.get(root).kind else {
            panic!("show")
        };
        assert_eq!(
            typer.store.symbols.owner(typed_reference(&typer, root)),
            Some(dest)
        );
        assert_eq!(typer.store.names.resolve(show.name.text()), "show");
        let TreeKind::Apply(converted) = &ast.get(show.qualifier).kind else {
            panic!("converted receiver")
        };
        assert_eq!(typed_reference(&typer, converted.args[0]), symbol(0));
        if name == "fromDerived" {
            assert_eq!(typed_reference(&typer, converted.function), wrap);
            continue;
        }
        let TreeKind::Select(apply) = &ast.get(converted.function).kind else {
            panic!("Conversion.apply")
        };
        assert_eq!(typer.store.names.resolve(apply.name.text()), "apply");
        let TreeKind::Apply(factory) = &ast.get(apply.qualifier).kind else {
            panic!("contextual application")
        };
        assert_eq!(typed_reference(&typer, factory.args[0]), symbol(1));
        let TreeKind::TypeApply(poly) = &ast.get(factory.function).kind else {
            panic!("type application")
        };
        assert_eq!(ast.get(poly.args[0]).ty, defs.int);
        assert_eq!(typed_reference(&typer, poly.function), wrap);
    }
}

#[test]
fn conditional_member_conversion_reports_the_failed_dependency_at_the_selection() {
    let rejected = include_str!(
        "../../../tests/fixtures/contextual-consumers/ConditionalMemberRejected.scala"
    );
    let text = format!(
        "{CONDITIONAL_MEMBER}\n{}",
        rejected.strip_prefix("package conditionalviews").unwrap()
    );
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&text);
    let wrap = method_symbol(&parsed, &store, &index, source, "wrap");
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    for name in ["missing", "ambiguous"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let before = typer.store.checkpoint();
        let err = typer.type_method_definition(tree).unwrap_err();
        let TyperError::MemberConversionDependency {
            source: at,
            tree_index,
            name: member,
            provider,
            cause,
        } = err
        else {
            panic!("{err:?}")
        };
        assert_eq!((at, tree_index, provider), (source, rhs.index(), wrap));
        assert_eq!(typer.store.names.resolve(member.text()), "show");
        match *cause {
            TyperError::MissingContextualArgument { .. } if name == "missing" => {}
            TyperError::AmbiguousContextualArgument { candidates, .. } if name == "ambiguous" => {
                let expected: Vec<_> = def.value_param_clauses[1]
                    .iter()
                    .map(|p| index.symbol_at(source, *p).unwrap())
                    .collect();
                assert_eq!(candidates, expected);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(before, typer.store.checkpoint());
        assert!(typer.typed_index.is_empty());
    }
}

#[test]
fn conditional_member_conversion_tries_the_companion_after_a_dependency_fails() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(CONDITIONAL_MEMBER);
    let alternate = class_symbol(&parsed, &store, &index, source, "Alternate");
    let companion = store.symbols.get(alternate).links.companion.unwrap();
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    for name in ["withoutEvidence", "conflictingEvidence"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        typer.type_method_definition(tree).unwrap();
        let ast = typer.typed_ast();
        let root = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Select(show) = &ast.get(root).kind else {
            panic!("show")
        };
        let TreeKind::Apply(converted) = &ast.get(show.qualifier).kind else {
            panic!("converted receiver")
        };
        let TreeKind::Select(apply) = &ast.get(converted.function).kind else {
            panic!("Conversion.apply")
        };
        let TreeKind::Select(fallback) = &ast.get(apply.qualifier).kind else {
            panic!("companion conversion")
        };
        assert_eq!(typer.store.names.resolve(fallback.name.text()), "fallback");
        assert_eq!(typed_reference(&typer, fallback.qualifier), companion);
    }
}

const TRAILING_IMPLICIT: &str =
    include_str!("../../../tests/fixtures/contextual-consumers/TrailingImplicitConversions.scala");

#[test]
fn trailing_implicit_conversion_applies_the_receiver_then_the_witness() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(TRAILING_IMPLICIT);
    let wrap = method_symbol(&parsed, &store, &index, source, "wrap");
    let resolver = super::contextual_binary::library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let fallback = method_symbol(&parsed, typer.store(), &index, source, "fallback");
    for name in ["argument", "member", "fallbackArgument", "fallbackMember"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        typer.type_method_definition(tree).unwrap();
        let root = typer.typed_index.get(source, rhs).unwrap();
        let ast = typer.typed_ast();
        let adapted = match &ast.get(root).kind {
            TreeKind::Select(show) if matches!(name, "member" | "fallbackMember") => show.qualifier,
            TreeKind::Apply(accept) if matches!(name, "argument" | "fallbackArgument") => {
                accept.args[0]
            }
            other => panic!("{other:?}"),
        };
        if name.starts_with("fallback") {
            let TreeKind::Apply(call) = &ast.get(adapted).kind else {
                panic!("companion conversion")
            };
            assert_eq!(typed_reference(&typer, call.function), fallback);
            let [arg] = call.args[..] else {
                panic!("single receiver")
            };
            assert_eq!(
                Some(typed_reference(&typer, arg)),
                index.symbol_at(source, def.value_param_clauses[0][0])
            );
            continue;
        }
        let TreeKind::Apply(evidence) = &ast.get(adapted).kind else {
            panic!("evidence")
        };
        let TreeKind::Apply(receiver) = &ast.get(evidence.function).kind else {
            panic!("receiver")
        };
        let TreeKind::TypeApply(poly) = &ast.get(receiver.function).kind else {
            panic!("type application")
        };
        assert_eq!(typed_reference(&typer, poly.function), wrap);
        assert_eq!(ast.get(poly.args[0]).ty, defs.int);
        for (call, clause) in [(receiver, 0), (evidence, 1)] {
            let [arg] = call.args[..] else {
                panic!("one argument")
            };
            assert_eq!(
                Some(typed_reference(&typer, arg)),
                index.symbol_at(source, def.value_param_clauses[clause][0])
            );
        }
    }
}

#[test]
fn trailing_implicit_conversion_preserves_the_failure_site_and_conflicting_witnesses() {
    let rejected =
        include_str!("../../../tests/fixtures/contextual-consumers/TrailingImplicitRejected.scala");
    let text = format!(
        "{TRAILING_IMPLICIT}\n{}",
        rejected.strip_prefix("package trailingviews").unwrap()
    );
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(&text);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for name in [
        "missingMember",
        "missingArgument",
        "ambiguousMember",
        "ambiguousArgument",
    ] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let operand = match &parsed.ast.get(rhs).kind {
            TreeKind::Select(show) => show.qualifier,
            TreeKind::Apply(call) => call.args[0],
            other => panic!("{other:?}"),
        };
        let before = typer.store.checkpoint();
        let err = typer.type_method_definition(tree).unwrap_err();
        match (name, err) {
            (
                "missingMember",
                TyperError::MemberNotFound {
                    tree_index, name, ..
                },
            ) => {
                assert_eq!(tree_index, rhs.index());
                assert_eq!(typer.store.names.resolve(name.text()), "show");
            }
            (
                "missingArgument",
                TyperError::ApplicationArgumentTypeMismatch {
                    tree_index,
                    argument_index: 0,
                    ..
                },
            ) => assert_eq!(tree_index, rhs.index()),
            (_, err) if name.starts_with("ambiguous") => {
                let err = if name == "ambiguousMember" {
                    let TyperError::MemberConversionDependency { cause, .. } = err else {
                        panic!("{err:?}")
                    };
                    *cause
                } else {
                    err
                };
                let TyperError::AmbiguousContextualArgument {
                    tree_index,
                    candidates,
                    ..
                } = err
                else {
                    panic!("{err:?}")
                };
                assert_eq!(tree_index, operand.index());
                let expected: Vec<_> = def.value_param_clauses[1]
                    .iter()
                    .map(|&p| index.symbol_at(source, p).unwrap())
                    .collect();
                assert_eq!(candidates, expected);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(before, typer.store.checkpoint());
        assert!(typer.typed_index.is_empty());
    }
}
