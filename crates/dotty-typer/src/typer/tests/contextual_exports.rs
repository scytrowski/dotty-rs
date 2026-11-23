mod chained;
mod clauses;
mod inherited;
mod named_using;
mod omitted;
mod poly_named;
mod within;

use super::*;

const LIBRARY: &str =
    include_str!("../../../tests/fixtures/contextual-arguments/ExportLibrary.scala");
const CLIENT: &str =
    include_str!("../../../tests/fixtures/contextual-arguments/ExportClient.scala");

fn unit(client: &str) -> NamedTestUnit {
    parse_and_name(&format!(
        "{LIBRARY}\n{}",
        client.strip_prefix("package exportsprint\n").unwrap()
    ))
}

fn qualified_name<'a>(typer: &'a SourceTyper<'_>, id: SymbolId) -> (&'a str, &'a str) {
    let entry = typer.store.symbols.get(id);
    let owner = typer.store.symbols.get(entry.owner.unwrap());
    (
        typer
            .store
            .names
            .resolve(owner.name.text())
            .trim_end_matches('$'),
        typer.store.names.resolve(entry.name.text()),
    )
}

fn reference(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    typer
        .store
        .types
        .get(typer.typed_ast().get(tree).ty)
        .reference_symbol()
        .unwrap()
}

#[test]
fn named_export_has_its_own_identity_and_delegates_to_the_original_given() {
    let (parsed, mut store, packages, defs, index, source) = unit(CLIENT);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "named");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer.type_method_definition(tree).unwrap();
    let selected = typer.typed_index.get(source, rhs).unwrap();
    let forwarder = reference(&typer, selected);
    let TreeKind::Select(sel) = &typer.typed_ast().get(selected).kind else {
        panic!()
    };
    let owner = typer
        .module_class_of_object(reference(&typer, sel.qualifier))
        .unwrap();
    assert_eq!(typer.store.symbols.owner(forwarder), Some(owner));
    assert!(
        typer
            .store
            .symbols
            .get(forwarder)
            .flags
            .contains(SymbolFlags::EXPORTED | SymbolFlags::GIVEN)
    );
    let definition = typer.type_export_forwarder_definition(forwarder).unwrap();
    assert_eq!(
        typer.type_export_forwarder_definition(forwarder).unwrap(),
        definition
    );
    let TreeKind::DefDef(def) = &typer.typed_ast().get(definition).kind else {
        panic!("export definition")
    };
    let body = def.rhs.unwrap();
    let TreeKind::Select(delegate) = &typer.typed_ast().get(body).kind else {
        panic!("delegate selection")
    };
    let original = reference(&typer, body);
    assert_eq!(
        original,
        val_symbol(&parsed, typer.store(), &index, source, "ints").0
    );
    assert_ne!(original, forwarder);
    assert_eq!(
        typer.store.symbols.owner(original),
        Some(
            typer
                .module_class_of_object(reference(&typer, delegate.qualifier))
                .unwrap()
        )
    );
    assert_eq!(
        typer.store.symbols.info(forwarder),
        &SymbolInfo::Complete(typer.typed_ast().get(body).ty)
    );
    let declared = typer.store.symbols.get(forwarder).name;
    typer.complete_symbol(owner).unwrap();
    assert_eq!(
        typer
            .store
            .scopes
            .get(index.scope_of(owner).unwrap())
            .lookup_all(&declared),
        &[forwarder]
    );
}

#[test]
fn failed_export_use_removes_its_entry_from_the_existing_module_scope() {
    let (parsed, mut store, packages, defs, index, source) =
        unit("package exportsprint\nclass Bad { def bad: Boolean = Named.ints }");
    let (method, _) = method_definition_and_rhs(&parsed, &store, &index, source, "bad");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let scope = parsed
        .ast
        .iter()
        .filter_map(|(tree, _)| index.export_site_at(source, tree))
        .find(|site| {
            store
                .names
                .resolve(store.symbols.get(site.owner).name.text())
                .trim_end_matches('$')
                == "Named"
        })
        .and_then(|site| index.scope_of(site.owner))
        .unwrap();
    let name = *dotty_core::TermName::new(store.names.intern("ints")).as_name();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let before = typer.store.checkpoint();
    for _ in 0..2 {
        assert!(matches!(
            typer.type_method_definition(tree),
            Err(TyperError::ExpectedExpressionTypeMismatch { .. })
        ));
        assert_eq!(typer.store.checkpoint(), before);
        assert!(typer.store.scopes.get(scope).lookup(&name).is_none());
    }
}

fn client_result(
    typer: &mut SourceTyper<'_>,
    parsed: &dotty_parser::ParseResult,
    name: &str,
) -> Result<TreeId<Typed>, TyperError> {
    let (method, rhs) =
        method_definition_and_rhs(parsed, typer.store(), typer.index, typer.source, name);
    let Some(SourceDefinition::Canonical { tree, .. }) = typer.index.definition_of(method) else {
        panic!("client method")
    };
    typer.type_method_definition(tree)?;
    let typed = typer.typed_index.get(typer.source, rhs).unwrap();
    Ok(match &typer.typed_ast().get(typed).kind {
        TreeKind::Block(block) => block.expr,
        _ => typed,
    })
}

fn rejected_call(
    typer: &mut SourceTyper<'_>,
    parsed: &dotty_parser::ParseResult,
    name: &str,
) -> TyperError {
    let (_, rhs) =
        method_definition_and_rhs(parsed, typer.store(), typer.index, typer.source, name);
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("client block")
    };
    let error = client_result(typer, parsed, name).unwrap_err();
    let (source, tree_index) = match &error {
        TyperError::MissingContextualArgument {
            source, tree_index, ..
        }
        | TyperError::AmbiguousContextualArgument {
            source, tree_index, ..
        } => (*source, *tree_index),
        _ => panic!("{name}: {error:?}"),
    };
    assert_eq!(
        (source, tree_index),
        (typer.source, block.expr.index()),
        "{name}"
    );
    error
}

fn evidence(typer: &SourceTyper<'_>, root: TreeId<Typed>) -> TreeId<Typed> {
    let TreeKind::Apply(call) = &typer.typed_ast().get(root).kind else {
        panic!("contextual call")
    };
    let [arg] = call.args.as_slice() else {
        panic!("one witness")
    };
    *arg
}

#[test]
fn renamed_export_is_the_same_member_for_direct_selection_and_given_search() {
    let (parsed, mut store, packages, defs, index, source) = unit(CLIENT);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let call = client_result(&mut typer, &parsed, "renamed").unwrap();
    let actual = reference(&typer, evidence(&typer, call));
    let direct = client_result(&mut typer, &parsed, "aliasSelection").unwrap();
    assert_eq!(reference(&typer, direct), actual);
    assert_eq!(
        typer
            .store
            .names
            .resolve(typer.store.symbols.get(actual).name.text()),
        "selected"
    );
    let target = typer.store.export_target(actual).unwrap();
    assert_eq!(
        typer.store.types.get(target).reference_symbol(),
        Some(val_symbol(&parsed, typer.store(), &index, source, "ints").0)
    );
    let owner = typer.store.symbols.owner(actual).unwrap();
    let scope = index.scope_of(owner).unwrap();
    let original = *dotty_core::TermName::new(typer.store.names.intern("ints")).as_name();
    assert!(typer.store.scopes.get(scope).lookup(&original).is_none());
}

const ERRORS: &str =
    include_str!("../../../tests/fixtures/contextual-arguments/ExportErrors.scala");

#[test]
fn given_wildcard_preserves_visibility_and_reports_both_competing_members() {
    let (parsed, mut store, packages, defs, index, source) = unit(CLIENT);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let direct = client_result(&mut typer, &parsed, "direct").unwrap();
    assert_eq!(
        reference(&typer, evidence(&typer, direct)),
        val_symbol(&parsed, typer.store(), &index, source, "ints").0
    );
    let call = client_result(&mut typer, &parsed, "wildcard").unwrap();
    let selected = reference(&typer, evidence(&typer, call));
    assert_eq!(
        typer
            .store
            .types
            .get(typer.store.export_target(selected).unwrap())
            .reference_symbol(),
        Some(val_symbol(&parsed, typer.store(), &index, source, "flags").0)
    );
    let (parsed, mut store, packages, defs, index, source) = unit(ERRORS);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let before = typer.store.checkpoint();
    assert!(matches!(
        rejected_call(&mut typer, &parsed, "hidden"),
        TyperError::MissingContextualArgument { .. }
    ));
    for _ in 0..2 {
        let error = rejected_call(&mut typer, &parsed, "ambiguous");
        let TyperError::AmbiguousContextualArgument {
            candidate_names, ..
        } = error
        else {
            panic!("{error:?}")
        };
        let mut names: Vec<_> = candidate_names
            .iter()
            .map(|(owner, name)| {
                (
                    typer
                        .store
                        .names
                        .resolve(owner.unwrap().text())
                        .trim_end_matches('$'),
                    typer.store.names.resolve(name.text()),
                )
            })
            .collect();
        names.sort();
        assert_eq!(names, [("All", "ints"), ("Other", "another")]);
        assert_eq!(typer.store.checkpoint(), before);
    }
}

#[test]
fn typed_given_selector_and_star_preserve_the_exported_evidence_kind() {
    let (parsed, mut store, packages, defs, index, source) = unit(CLIENT);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for (method, decl, flag) in [
        ("bounded", "ints", SymbolFlags::GIVEN),
        ("legacy", "old", SymbolFlags::IMPLICIT),
    ] {
        let call = client_result(&mut typer, &parsed, method).unwrap();
        let selected = reference(&typer, evidence(&typer, call));
        assert_eq!(
            typer.store.symbols.get(selected).flags & (SymbolFlags::IMPLICIT | SymbolFlags::GIVEN),
            flag
        );
        assert_eq!(
            typer
                .store
                .types
                .get(typer.store.export_target(selected).unwrap())
                .reference_symbol(),
            Some(val_symbol(&parsed, typer.store(), &index, source, decl).0)
        );
    }
    let (parsed, mut store, packages, defs, index, source) = unit(ERRORS);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    for method in ["excluded", "star"] {
        assert!(
            matches!(
                rejected_call(&mut typer, &parsed, method),
                TyperError::MissingContextualArgument { .. }
            ),
            "{method}"
        );
    }
}

#[test]
fn polymorphic_export_passes_its_own_type_and_witness_to_the_original_method() {
    let (parsed, mut store, packages, defs, index, source) = unit(CLIENT);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let root = client_result(&mut typer, &parsed, "conditional").unwrap();
    let call = evidence(&typer, root);
    let TreeKind::Apply(app) = &typer.typed_ast().get(call).kind else {
        panic!("provider application")
    };
    let leaf = reference(&typer, app.args[0]);
    let TreeKind::TypeApply(tapp) = &typer.typed_ast().get(app.function).kind else {
        panic!("inferred type")
    };
    assert_eq!(
        typer.typed_ast().get(tapp.args[0]).ty,
        typer.definitions.int
    );
    let provider = reference(&typer, tapp.function);
    assert_eq!(
        typer.store.symbols.owner(provider),
        typer.store.symbols.owner(leaf)
    );
    assert_eq!(
        typer
            .store
            .types
            .get(typer.store.export_target(leaf).unwrap())
            .reference_symbol(),
        Some(val_symbol(&parsed, typer.store(), &index, source, "ints").0)
    );
    let original = method_definition_and_rhs(&parsed, typer.store(), &index, source, "boxed").0;
    assert_ne!(
        typer.store.symbols.info(provider),
        typer.store.symbols.info(original)
    );
    assert_eq!(provider_delegate(&mut typer, provider), original);
}

fn provider_delegate(typer: &mut SourceTyper<'_>, provider: SymbolId) -> SymbolId {
    let def = typer.type_export_forwarder_definition(provider).unwrap();
    let TreeKind::DefDef(def) = &typer.typed_ast().get(def).kind else {
        panic!()
    };
    let type_param = typer
        .typed_index
        .synthetic_definition_symbol(def.type_params[0])
        .unwrap();
    let parameter = typer
        .typed_index
        .synthetic_definition_symbol(def.value_param_clauses[0][0])
        .unwrap();
    assert_eq!(typer.store.symbols.owner(type_param), Some(provider));
    assert_eq!(typer.store.symbols.owner(parameter), Some(provider));
    let TreeKind::Apply(delegate) = &typer.typed_ast().get(def.rhs.unwrap()).kind else {
        panic!()
    };
    assert_eq!(reference(typer, delegate.args[0]), parameter);
    let TreeKind::TypeApply(ta) = &typer.typed_ast().get(delegate.function).kind else {
        panic!()
    };
    assert_eq!(reference(typer, ta.args[0]), type_param);
    reference(typer, ta.function)
}

#[test]
fn conditional_export_requires_an_unambiguous_dependency_at_the_call_site() {
    let (parsed, mut store, packages, defs, index, source) = unit(ERRORS);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let checkpoint = typer.store.checkpoint();
    assert!(matches!(
        rejected_call(&mut typer, &parsed, "missingDependency"),
        TyperError::MissingContextualArgument { .. }
    ));
    let error = rejected_call(&mut typer, &parsed, "ambiguousDependency");
    let TyperError::AmbiguousContextualArgument { candidates, .. } = error else {
        panic!("{error:?}")
    };
    let mut names: Vec<_> = candidates
        .iter()
        .map(|&id| {
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(id).name.text())
        })
        .collect();
    names.sort();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(typer.store.checkpoint(), checkpoint);
}
