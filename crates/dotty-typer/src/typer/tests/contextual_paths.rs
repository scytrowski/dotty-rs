use super::*;

#[test]
fn path_dependent_results_and_evidence_preserve_the_exact_import_receiver() {
    let fixture = include_str!("../../../tests/fixtures/contextual-arguments/Paths.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (method_name, field) in [("left", "p"), ("right", "q"), ("viaView", "p")] {
        let (method, rhs) =
            method_definition_and_rhs(&parsed, typer.store(), &index, source, method_name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_or_else(|e| panic!("{method_name}: {e:?}"));
        let arena = typer.typed_ast();
        let typed = arena.get(typer.typed_index.get(source, rhs).unwrap());
        let TreeKind::Apply(call) = &typed.kind else {
            panic!("read")
        };
        let TreeKind::Select(receiver) = &arena.get(call.function).kind else {
            panic!("selected read")
        };
        let receiver = reference(&typer, receiver.qualifier);
        let TreeKind::Select(ev) = &arena.get(call.args[0]).kind else {
            panic!("selected evidence")
        };
        assert_eq!(reference(&typer, ev.qualifier), receiver);
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(receiver).name.text()),
            field
        );
        let Type::TypeRef { prefix, .. } = typer.store.types.get(typed.ty) else {
            panic!("Item")
        };
        assert!(
            matches!(typer.store.types.get(*prefix), Type::TermRef{target: TermRefTarget::Symbol(path),..} if *path==receiver)
        );
    }
}

#[test]
fn contextual_evidence_expands_transparent_alias_anchors_and_keeps_opaque_identity() {
    let fixture = include_str!("../../../tests/fixtures/contextual-arguments/Anchors.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for (method_name, evidence) in [
        ("viaAlias", "targetKey"),
        ("viaOpaque", "tokenKey"),
        ("inside", "raw"),
    ] {
        let (method, rhs) =
            method_definition_and_rhs(&parsed, typer.store(), &index, source, method_name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_or_else(|e| panic!("{method_name}: {e:?}"));
        let typed = typer
            .typed_ast()
            .get(typer.typed_index.get(source, rhs).unwrap());
        let TreeKind::Apply(call) = &typed.kind else {
            panic!("call")
        };
        let selected = reference(&typer, call.args[0]);
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(selected).name.text()),
            evidence
        );
    }
}

#[test]
fn different_family_paths_fail_conformance_atomically() {
    let fixture = include_str!("../../../tests/fixtures/contextual-arguments/PathErrors.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "wrong");
    let method_tree = definition_tree(&index, method);
    for _ in 0..2 {
        let error = typer.type_method_definition(method_tree).unwrap_err();
        assert!(
            matches!(error, TyperError::ExpectedExpressionTypeMismatch { .. }),
            "{error:?}"
        );
        assert!(typer.typed_index.get(source, rhs).is_none());
    }
}

#[test]
fn contextual_method_instantiates_later_parameters_and_results_at_the_supplied_path() {
    let fixture = include_str!("../../../tests/fixtures/contextual-arguments/Dependent.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["run", "direct"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let typed = typer
            .typed_ast()
            .get(typer.typed_index.get(source, rhs).unwrap());
        let TreeKind::Apply(call) = &typed.kind else {
            panic!("second clause")
        };
        let TreeKind::Apply(first) = &typer.typed_ast().get(call.function).kind else {
            panic!("first clause")
        };
        let path = reference(&typer, first.args[0]);
        let TreeKind::Select(evidence) = &typer.typed_ast().get(call.args[0]).kind else {
            panic!("path evidence")
        };
        assert_eq!(reference(&typer, evidence.qualifier), path);
        let Type::TypeRef { prefix, .. } = typer.store.types.get(typed.ty) else {
            panic!("dependent result")
        };
        assert!(
            matches!(typer.store.types.get(*prefix), Type::TermRef{target: TermRefTarget::Symbol(symbol),..} if *symbol == path)
        );
    }
}

#[test]
fn opaque_anchor_never_accepts_evidence_for_the_hidden_representation_outside_its_owner() {
    let fixture = include_str!("../../../tests/fixtures/contextual-arguments/OpaqueError.scala");
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "forged");
    let error = typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap_err();
    assert!(
        matches!(error, TyperError::MissingContextualArgument { .. }),
        "{error:?}"
    );
}

#[test]
fn nested_stable_receiver_remains_in_its_member_evidence_tree() {
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/Paths.scala"
    ));
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "deep");
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let typed = typer
        .typed_ast()
        .get(typer.typed_index.get(source, rhs).unwrap());
    let TreeKind::Apply(call) = &typed.kind else {
        panic!("call")
    };
    let TreeKind::Select(ev) = &typer.typed_ast().get(call.args[0]).kind else {
        panic!("evidence")
    };
    let TreeKind::Select(family) = &typer.typed_ast().get(ev.qualifier).kind else {
        panic!("family path")
    };
    let root = reference(&typer, family.qualifier);
    assert_eq!(
        typer
            .store
            .names
            .resolve(typer.store.symbols.get(root).name.text()),
        "a"
    );
}

#[test]
fn unstable_argument_cannot_substitute_a_dependent_contextual_result() {
    let text = "class Family { class Item }; class Use { def wanted(using p: Family)(using i: p.Item): p.Item = i; def fresh: Family = new Family; def unstable: Any = wanted(using fresh) }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "unstable");
    let error = typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap_err();
    assert!(
        matches!(error, TyperError::DependentMethodApplicationDeferred { .. }),
        "{error:?}"
    );
    assert!(typer.typed_index.get(source, rhs).is_none());
}

#[test]
fn mutable_receiver_cannot_form_a_path_dependent_type() {
    let text =
        "class Family { class Item }; class Use(var p: Family) { def unstable: p.Item = ??? }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "unstable");
    let error = typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap_err();
    assert!(
        matches!(error, TyperError::UnstableSelectionPrefix { .. }),
        "{error:?}"
    );
}

#[test]
fn conditional_provider_instantiates_its_second_evidence_clause_from_the_first() {
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/Dependent.scala"
    ));
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (method, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "conditional");
    typer
        .type_method_definition(definition_tree(&index, method))
        .unwrap();
    let typed = typer
        .typed_ast()
        .get(typer.typed_index.get(source, rhs).unwrap());
    let TreeKind::Apply(call) = &typed.kind else {
        panic!("needs")
    };
    let TreeKind::Apply(second) = &typer.typed_ast().get(call.args[0]).kind else {
        panic!("second provider clause")
    };
    let TreeKind::Apply(first) = &typer.typed_ast().get(second.function).kind else {
        panic!("first provider clause")
    };
    let TreeKind::Select(member) = &typer.typed_ast().get(second.args[0]).kind else {
        panic!("path evidence")
    };
    assert_eq!(
        reference(&typer, member.qualifier),
        reference(&typer, first.args[0])
    );
}

#[test]
fn inherited_nested_class_types_use_the_enclosing_receiver() {
    let fixture = "
        trait Parent[A] { class Ops; private class Hidden }
        class Ops
        trait Child[A] extends Parent[A] {
          def inherited(v: Ops): Ops = v
          def shadow[Ops](v: Ops): Ops = v
          def hidden(v: Hidden): Hidden = v
        }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let child = class_symbol(&parsed, &store, &index, source, "Child");
    let nested = class_symbol(&parsed, &store, &index, source, "Ops");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for name in ["inherited", "shadow"] {
        let (method, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap();
        let tree = definition_tree(&index, method);
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            unreachable!()
        };
        let ty = typer.type_index.type_at(source, def.tpt).unwrap();
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(target),
        } = *typer.store.types.get(ty)
        else {
            panic!("expected a type reference")
        };
        if name == "inherited" {
            assert_eq!(target, nested);
            assert_eq!(
                *typer.store.types.get(prefix),
                Type::ThisType { class: child }
            );
        } else {
            assert_eq!(
                typer.store.symbols.get(target).kind,
                SymbolKind::TypeParameter
            );
            assert_eq!(typer.store.symbols.get(target).owner, Some(method));
        }
    }
    let (hidden, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, "hidden");
    assert!(matches!(
        typer.type_method_definition(definition_tree(&index, hidden)),
        Err(TyperError::TypeNameNotFound { .. })
    ));
}

#[test]
fn unrelated_parent_names_preserve_lexical_type_lookup() {
    let fixture = "
        trait Pending[A] extends NotAvailable[A] {
          def identity(v: Int): Int = v
          def missing(v: NoSuchType): NoSuchType = v
        }
        trait PlainBase { class Nested }
        trait PlainChild extends PlainBase {
          def outsideSubset(v: Nested): Nested = v
        }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(fixture);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let (identity, _) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "identity");
    typer
        .type_method_definition(definition_tree(&index, identity))
        .unwrap();
    for (method, expected_name) in [("missing", "NoSuchType"), ("outsideSubset", "Nested")] {
        let (method, _) = method_definition_and_rhs(&parsed, typer.store(), &index, source, method);
        let err = typer
            .type_method_definition(definition_tree(&index, method))
            .unwrap_err();
        let TyperError::TypeNameNotFound { name, .. } = err else {
            panic!("{err:?}")
        };
        assert_eq!(typer.store.names.resolve(name.text()), expected_name);
    }
}
