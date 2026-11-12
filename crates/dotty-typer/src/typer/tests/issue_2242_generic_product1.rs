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
    {
        let retry_info = typer.complete_symbol(left).expect("valid retry");
        let Type::ClassInfo(info) = typer.store.types.get(retry_info) else {
            panic!("valid sibling still completes after rollback")
        };
        assert_eq!(info.class, left);
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
