use super::*;
use dotty_core::{Namespace, SourceText};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;

const SOURCE: &str = r#"
package scala { trait Product }
package scala.util {
  sealed abstract class Either[+A, +B] extends Product
  final case class Left[+A, +B](value: A) extends Either[A, B]
  final case class Right[+A, +B](value: B) extends Either[A, B]
  final case class Renamed[+L, +R](payload: L) extends Either[L, R]
  final case class WrongField[A, B](value: Int) extends Either[A, B]
  final case class ReversedParent[A, B](value: A) extends Either[B, A]
  final case class SelectorFirstReversed[A, B](value: B) extends Either[B, A]
  final case class Fresh[A, B](value: A) extends Either[A, B]
  final case class FreshAfterSelector[A, B](value: B) extends Either[A, B]
  final case class WrongArity[A](value: A) extends Either[A, A]
}
"#;

#[test]
fn generic_either_product1_projects_the_ordered_source_field_and_substitutes_receivers() {
    let source = SourceId::from_index(22420);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).unwrap(),
        source,
        ContextualScanner::new(SOURCE).unwrap(),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Either.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();

    let find_class = |store: &SemanticStore, name: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing source class {name}"))
    };
    let left = find_class(&store, "Left");
    let right = find_class(&store, "Right");
    let renamed = find_class(&store, "Renamed");
    let either = find_class(&store, "Either");
    let text_name = Name::new(store.names.intern("_1"), Namespace::Term);
    let class_parameters = [left, right, renamed].map(|class| {
        let typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        typer.source_class_type_parameters(class).unwrap()
    });
    assert_eq!(class_parameters[0].len(), 2);
    assert_eq!(class_parameters[1].len(), 2);

    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let left_info = typer.complete_symbol(left).unwrap();
    assert!(matches!(
        *typer.store().symbols.info(either),
        SymbolInfo::Complete(_)
    ));
    let right_info = typer.complete_symbol(right).unwrap();
    let renamed_info = typer.complete_symbol(renamed).unwrap();
    for (class, info, field_parameter, parameter_set) in [
        (left, left_info, class_parameters[0][0], 0),
        (right, right_info, class_parameters[1][1], 1),
        (renamed, renamed_info, class_parameters[2][0], 2),
    ] {
        let Type::ClassInfo(class_info) = typer.store.types.get(info) else {
            panic!("case class completion publishes ClassInfo")
        };
        assert_eq!(class_info.parents.len(), 2);
        assert_eq!(
            class_info.parents.iter().filter(|parent| matches!(
                typer.store.types.get(**parent),
                Type::Applied { tycon, args }
                    if matches!(typer.store.types.get(*tycon),
                        Type::TypeRef { target: TypeRefTarget::Symbol(parent), .. }
                            if typer.store.names.resolve(typer.store.symbols.get(*parent).name.text()) == "Either")
                        && args.len() == 2
            )).count(),
            1,
            "the declared applied Either parent is preserved"
        );
        let parent_arguments = class_info
            .parents
            .iter()
            .find_map(|parent| match typer.store.types.get(*parent) {
                Type::Applied { tycon, args }
                    if matches!(typer.store.types.get(*tycon),
                        Type::TypeRef { target: TypeRefTarget::Symbol(parent), .. }
                            if typer.store.names.resolve(typer.store.symbols.get(*parent).name.text()) == "Either") =>
                {
                    Some(args)
                }
                _ => None,
            })
            .unwrap();
        let argument_symbols = parent_arguments
            .iter()
            .map(|argument| match typer.store.types.get(*argument) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(parameter),
                    ..
                } => *parameter,
                other => panic!("source Either argument retains its type identity: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            argument_symbols, class_parameters[parameter_set],
            "the declared Either arguments preserve class parameter order"
        );
        assert!(class_info.parents.iter().any(|parent| matches!(
            typer.store.types.get(*parent),
            Type::TypeRef { target: TypeRefTarget::Symbol(product), .. }
                if typer.store.names.resolve(typer.store.symbols.get(*product).name.text()) == "Product"
        )));
        let selector = typer
            .store
            .scopes
            .get(class_info.declarations)
            .lookup(&text_name)
            .expect("the class owns its generated _1 selector");
        let SymbolInfo::Complete(selector_info) = *typer.store.symbols.info(selector) else {
            panic!("generated selector is complete")
        };
        let Type::Method(method) = typer.store.types.get(selector_info) else {
            panic!("generated selector has a method type")
        };
        assert!(method.params.is_empty());
        assert!(matches!(typer.store.types.get(method.result),
            Type::TypeRef { target: TypeRefTarget::Symbol(parameter), .. }
                if *parameter == field_parameter));
        assert_eq!(typer.store.symbols.get(selector).owner, Some(class));
        let field = parsed
            .ast
            .iter()
            .find_map(|(class_tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if index.symbol_at(source, class_tree) == Some(class) =>
                {
                    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                        panic!("source case class has a template")
                    };
                    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind
                    else {
                        panic!("source case class has a constructor")
                    };
                    Some(constructor.value_param_clauses[0][0])
                }
                _ => None,
            })
            .unwrap();
        let field_symbol = index.symbol_at(source, field).unwrap();
        assert_ne!(
            selector, field_symbol,
            "selector and source field keep distinct IDs"
        );
        assert_eq!(
            typer.store.symbols.get(field_symbol).kind,
            SymbolKind::Field
        );
        assert_eq!(typer.store.symbols.get(field_symbol).owner, Some(class));
        assert_eq!(
            index.definition_of(field_symbol),
            Some(SourceDefinition::Canonical {
                source,
                tree: field
            })
        );
    }

    for (class, parameters, expected) in [
        (left, &class_parameters[0], definitions.int),
        (right, &class_parameters[1], definitions.boolean),
    ] {
        let tycon = typer
            .store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let receiver = typer.store.types.alloc(Type::Applied {
            tycon,
            args: vec![definitions.int, definitions.boolean],
        });
        let candidate = typer
            .lookup_members(receiver, text_name)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.declaring_class == class)
            .unwrap();
        let adapted = typer.member_type_on(&candidate).unwrap();
        let Type::Method(method) = typer.store.types.get(adapted) else {
            panic!("adapted selector remains a method")
        };
        assert_eq!(method.result, expected);
        assert_eq!(parameters.len(), 2);
    }

    let wrong_field = find_class(typer.store, "WrongField");
    let wrong_field_info = typer.complete_symbol(wrong_field).unwrap();
    let Type::ClassInfo(wrong_field_info) = typer.store.types.get(wrong_field_info) else {
        panic!("unrelated field shape remains an ordinary source class")
    };
    assert!(!wrong_field_info.parents.iter().any(|parent| matches!(
        typer.store.types.get(*parent),
        Type::TypeRef { target: TypeRefTarget::Symbol(product), .. }
            if typer.store.names.resolve(typer.store.symbols.get(*product).name.text()) == "Product"
    )));

    let reversed_parent = find_class(typer.store, "ReversedParent");
    let reversed_scope = index.scope_of(reversed_parent).unwrap();
    let reversed_selector = typer
        .store
        .scopes
        .get(reversed_scope)
        .lookup(&text_name)
        .unwrap();
    let reversed_field = parsed
        .ast
        .iter()
        .find_map(|(class_tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if index.symbol_at(source, class_tree) == Some(reversed_parent) =>
            {
                let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                    panic!("ReversedParent has a template")
                };
                let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind
                else {
                    panic!("ReversedParent has a constructor")
                };
                Some(constructor.value_param_clauses[0][0])
            }
            _ => None,
        })
        .unwrap();
    let reversed_field_symbol = index.symbol_at(source, reversed_field).unwrap();
    let reversed_field_tpt = match &parsed.ast.get(reversed_field).kind {
        TreeKind::ValDef(field) => field.tpt,
        _ => panic!("ReversedParent constructor parameter is a field"),
    };
    let store_before_class_failure = typer.store().checkpoint();
    let typed_before_class_failure = typer.typed_ast().checkpoint();
    let typed_mappings_before_class_failure = typer.source_typed_index().len();
    let source_types_before_class_failure = parsed
        .ast
        .iter()
        .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
        .collect::<Vec<_>>();
    let selector_info_before_class_failure = *typer.store().symbols.info(reversed_selector);
    let field_info_before_class_failure = *typer.store().symbols.info(reversed_field_symbol);
    let field_type_before_class_failure = typer
        .source_type_index()
        .type_at(source, reversed_field_tpt);
    let either_info_before_class_failure = *typer.store().symbols.info(either);
    let reversed_parameters = typer.source_class_type_parameters(reversed_parent).unwrap();
    let reversed_parameter_info_before_class_failure = reversed_parameters
        .iter()
        .map(|parameter| *typer.store().symbols.info(*parameter))
        .collect::<Vec<_>>();
    let result = typer.complete_symbol(reversed_parent);
    assert!(
        matches!(
            result,
            Err(TyperError::UnsupportedProduct1FieldType { class: rejected, .. })
                if rejected == reversed_parent
        ),
        "unexpected result for ReversedParent: {result:?}"
    );
    assert_eq!(
        *typer.store.symbols.info(reversed_parent),
        SymbolInfo::Missing
    );
    assert_eq!(typer.store().checkpoint(), store_before_class_failure);
    assert_eq!(typer.typed_ast().checkpoint(), typed_before_class_failure);
    assert_eq!(
        typer.source_typed_index().len(),
        typed_mappings_before_class_failure
    );
    assert_eq!(
        parsed
            .ast
            .iter()
            .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
            .collect::<Vec<_>>(),
        source_types_before_class_failure
    );
    assert_eq!(
        *typer.store().symbols.info(reversed_selector),
        selector_info_before_class_failure
    );
    assert_eq!(
        *typer.store().symbols.info(reversed_field_symbol),
        field_info_before_class_failure
    );
    assert_eq!(
        typer
            .source_type_index()
            .type_at(source, reversed_field_tpt),
        field_type_before_class_failure
    );
    assert_eq!(
        *typer.store().symbols.info(either),
        either_info_before_class_failure
    );
    assert_eq!(
        reversed_parameters
            .iter()
            .map(|parameter| *typer.store().symbols.info(*parameter))
            .collect::<Vec<_>>(),
        reversed_parameter_info_before_class_failure
    );
    {
        let fresh = find_class(typer.store, "Fresh");
        assert_eq!(*typer.store.symbols.info(fresh), SymbolInfo::Missing);
        let retry_info = typer
            .complete_symbol(fresh)
            .expect("valid class-first retry");
        let Type::ClassInfo(info) = typer.store.types.get(retry_info) else {
            panic!("valid sibling still completes after rollback")
        };
        assert_eq!(info.class, fresh);
    }

    let selector_first = find_class(typer.store, "SelectorFirstReversed");
    let selector_first_scope = index.scope_of(selector_first).unwrap();
    let selector_first_selector = typer
        .store
        .scopes
        .get(selector_first_scope)
        .lookup(&text_name)
        .unwrap();
    let selector_first_parameters = typer.source_class_type_parameters(selector_first).unwrap();
    let selector_first_parameter_info_before = selector_first_parameters
        .iter()
        .map(|parameter| *typer.store().symbols.info(*parameter))
        .collect::<Vec<_>>();
    let before_selector_failure = typer.store().checkpoint();
    let typed_before_selector_failure = typer.typed_ast().checkpoint();
    let typed_mappings_before_selector_failure = typer.source_typed_index().len();
    let source_types_before_selector_failure = parsed
        .ast
        .iter()
        .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
        .collect::<Vec<_>>();
    let selector_failure = typer
        .complete_symbol(selector_first_selector)
        .expect_err("selector-first completion rejects a reversed generic parent");
    assert!(matches!(
        selector_failure,
        TyperError::UnsupportedProduct1FieldType { class, .. } if class == selector_first
    ));
    assert_eq!(typer.store().checkpoint(), before_selector_failure);
    assert_eq!(
        typer.typed_ast().checkpoint(),
        typed_before_selector_failure
    );
    assert_eq!(
        typer.source_typed_index().len(),
        typed_mappings_before_selector_failure
    );
    assert_eq!(
        parsed
            .ast
            .iter()
            .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
            .collect::<Vec<_>>(),
        source_types_before_selector_failure
    );
    assert_eq!(
        *typer.store.symbols.info(selector_first),
        SymbolInfo::Missing
    );
    assert_eq!(
        *typer.store.symbols.info(selector_first_selector),
        SymbolInfo::Missing
    );
    assert_eq!(
        selector_first_parameters
            .iter()
            .map(|parameter| *typer.store().symbols.info(*parameter))
            .collect::<Vec<_>>(),
        selector_first_parameter_info_before
    );
    {
        let fresh = find_class(typer.store, "FreshAfterSelector");
        assert_eq!(*typer.store.symbols.info(fresh), SymbolInfo::Missing);
        let retry_info = typer
            .complete_symbol(fresh)
            .expect("valid selector-first sibling retry");
        let Type::ClassInfo(info) = typer.store.types.get(retry_info) else {
            panic!("valid sibling completes after selector-first rollback")
        };
        assert_eq!(info.class, fresh);
    }

    let wrong_arity = find_class(typer.store, "WrongArity");
    let wrong_arity_info = typer.complete_symbol(wrong_arity).unwrap();
    let Type::ClassInfo(wrong_arity_info) = typer.store.types.get(wrong_arity_info) else {
        panic!("unsupported arity remains an ordinary source class")
    };
    assert!(!wrong_arity_info.parents.iter().any(|parent| matches!(
        typer.store.types.get(*parent),
        Type::TypeRef { target: TypeRefTarget::Symbol(product), .. }
            if typer.store.names.resolve(typer.store.symbols.get(*product).name.text()) == "Product"
    )));
}

#[test]
fn generic_either_parent_metadata_is_completed_and_authenticated_transactionally() {
    use dotty_core::CompletionId;

    #[derive(Clone, Copy)]
    enum ParentState {
        Error,
        Deferred,
        MalformedClassInfo,
    }

    for (parent_state, selector_first) in [
        (ParentState::Error, false),
        (ParentState::Deferred, true),
        (ParentState::MalformedClassInfo, false),
    ] {
        let source = SourceId::from_index(22430 + selector_first as u32);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(SOURCE).unwrap(),
            source,
            ContextualScanner::new(SOURCE).unwrap(),
            &mut store.names,
        );
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Either.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let symbol_named = |store: &SemanticStore, wanted: &str| {
            parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == wanted =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing class {wanted}"))
        };
        let either = symbol_named(&store, "Either");
        let left = symbol_named(&store, "Left");
        let left_scope = index.scope_of(left).unwrap();
        let selector_name = Name::new(store.names.intern("_1"), Namespace::Term);
        let selector = store.scopes.get(left_scope).lookup(&selector_name).unwrap();
        let field = parsed
            .ast
            .iter()
            .find_map(|(class_tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if index.symbol_at(source, class_tree) == Some(left) =>
                {
                    let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                        panic!("Left has a template")
                    };
                    let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind
                    else {
                        panic!("Left has a constructor")
                    };
                    Some(constructor.value_param_clauses[0][0])
                }
                _ => None,
            })
            .unwrap();
        let field_symbol = index.symbol_at(source, field).unwrap();
        let field_tpt = match &parsed.ast.get(field).kind {
            TreeKind::ValDef(field) => field.tpt,
            _ => panic!("constructor parameter is a field declaration"),
        };
        let parent_scope = index.scope_of(either).unwrap();
        match parent_state {
            ParentState::Error => store.symbols.set_info(either, SymbolInfo::Error),
            ParentState::Deferred => {
                // CompletionId intentionally has no public constructor. This
                // test injects an opaque, valid u32-backed ID to exercise the
                // Deferred branch without exposing a production constructor.
                let deferred = unsafe { std::mem::transmute::<u32, CompletionId>(0) };
                store
                    .symbols
                    .set_info(either, SymbolInfo::Deferred(deferred));
            }
            ParentState::MalformedClassInfo => {
                let info = store.types.alloc(Type::ClassInfo(ClassInfo {
                    prefix: definitions.no_prefix,
                    class: either,
                    parents: Vec::new(),
                    declarations: parent_scope,
                    self_type: None,
                }));
                store.symbols.set_info(either, SymbolInfo::Complete(info));
            }
        }

        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let parent_info_before = *typer.store().symbols.info(either);
        let class_info_before = *typer.store().symbols.info(left);
        let selector_info_before = *typer.store().symbols.info(selector);
        let field_info_before = *typer.store().symbols.info(field_symbol);
        let parameters = typer.source_class_type_parameters(left).unwrap();
        let parameter_info_before = parameters
            .iter()
            .map(|parameter| *typer.store().symbols.info(*parameter))
            .collect::<Vec<_>>();
        let store_before = typer.store().checkpoint();
        let typed_before = typer.typed_ast().checkpoint();
        let typed_mappings_before = typer.source_typed_index().len();
        let source_types_before = parsed
            .ast
            .iter()
            .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
            .collect::<Vec<_>>();
        let result = if selector_first {
            typer.complete_symbol(selector)
        } else {
            typer.complete_symbol(left)
        };
        match parent_state {
            ParentState::Error => assert!(matches!(
                result,
                Err(TyperError::SymbolAlreadyErrored { symbol }) if symbol == either
            )),
            ParentState::Deferred => assert!(matches!(
                result,
                Err(TyperError::DeferredSymbolCompletion { symbol }) if symbol == either
            )),
            ParentState::MalformedClassInfo => assert!(matches!(
                result,
                Err(TyperError::MalformedClassInfo { symbol, .. }) if symbol == either
            )),
        }
        assert_eq!(typer.store().checkpoint(), store_before);
        assert_eq!(typer.typed_ast().checkpoint(), typed_before);
        assert_eq!(typer.source_typed_index().len(), typed_mappings_before);
        assert_eq!(
            parsed
                .ast
                .iter()
                .map(|(tree, _)| (tree, typer.source_type_index().type_at(source, tree)))
                .collect::<Vec<_>>(),
            source_types_before
        );
        assert_eq!(*typer.store().symbols.info(either), parent_info_before);
        assert_eq!(*typer.store().symbols.info(left), class_info_before);
        assert_eq!(*typer.store().symbols.info(selector), selector_info_before);
        assert_eq!(*typer.store().symbols.info(field_symbol), field_info_before);
        assert_eq!(
            parameters
                .iter()
                .map(|parameter| *typer.store().symbols.info(*parameter))
                .collect::<Vec<_>>(),
            parameter_info_before
        );
        assert_eq!(
            typer.source_type_index().type_at(source, field_tpt),
            source_types_before
                .iter()
                .find_map(|(tree, ty)| (*tree == field_tpt).then_some(*ty))
                .flatten()
        );

        // Repair only the injected parent metadata, then prove a valid sibling
        // still completes in the same session after the failed projection.
        typer.store.symbols.set_info(either, SymbolInfo::Missing);
        let right = symbol_named(typer.store(), "Right");
        assert_eq!(*typer.store().symbols.info(right), SymbolInfo::Missing);
        let right_info = typer
            .complete_symbol(right)
            .expect("valid sibling completes after parent repair");
        assert!(
            matches!(typer.store().types.get(right_info), Type::ClassInfo(info) if info.class == right)
        );
        assert!(matches!(
            *typer.store().symbols.info(either),
            SymbolInfo::Complete(_)
        ));
    }
}
