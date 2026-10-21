use dotty_core::ast::{AstArena, TreeKind, Untyped};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, TermRefTarget, TreeId, Type, Visibility,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const PRIVATE_VALUES: &str = include_str!("fixtures/tasty_pickler/PrivateValues.scala");
const BOX: &str = include_str!("fixtures/tasty_pickler/Box.scala");
const COUNTER: &str = include_str!("fixtures/tasty_pickler/Counter.scala");

fn with_source<R>(
    source_text: &str,
    f: impl FnOnce(
        &AstArena<Untyped>,
        SourceId,
        &dotty_namer::SourceSemanticIndex,
        &Definitions,
        &mut SourceTyper<'_>,
    ) -> R,
) -> R {
    let source = SourceId::from_index(1389);
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source,
        "TastyClassState.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            index
        }
        NamingOutcome::Rejected { diagnostics } => panic!("namer rejected source: {diagnostics:?}"),
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    f(&parsed.ast, source, &index, &definitions, &mut typer)
}

fn named_symbol(
    ast: &AstArena<Untyped>,
    source: SourceId,
    index: &dotty_namer::SourceSemanticIndex,
    typer: &SourceTyper<'_>,
    name: &str,
) -> SymbolId {
    ast.iter()
        .find_map(|(tree, node)| {
            let symbol = index.symbol_at(source, tree)?;
            let node_name = match &node.kind {
                TreeKind::TypeDef(definition) => definition.name.as_name().text(),
                TreeKind::ValDef(definition) => definition.name.as_name().text(),
                TreeKind::DefDef(definition) => definition.name.as_name().text(),
                _ => return None,
            };
            (typer.store().names.resolve(node_name) == name).then_some(symbol)
        })
        .unwrap_or_else(|| panic!("missing source symbol {name}"))
}

fn class_tree(
    ast: &AstArena<Untyped>,
    source: SourceId,
    index: &dotty_namer::SourceSemanticIndex,
    class: SymbolId,
) -> (TreeId<Untyped>, TreeId<Untyped>) {
    ast.iter()
        .find_map(|(tree, node)| {
            if index.symbol_at(source, tree) != Some(class) {
                return None;
            }
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            matches!(ast.get(definition.rhs).kind, TreeKind::Template(_))
                .then_some((tree, definition.rhs))
        })
        .expect("class source tree and template")
}

fn field(
    ast: &AstArena<Untyped>,
    source: SourceId,
    index: &dotty_namer::SourceSemanticIndex,
    typer: &SourceTyper<'_>,
    name: &str,
) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
    ast.iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            let symbol = index.symbol_at(source, tree)?;
            (typer
                .store()
                .names
                .resolve(definition.name.as_name().text())
                == name)
                .then_some((symbol, tree, definition.rhs?))
        })
        .unwrap_or_else(|| panic!("missing initialized field {name}"))
}

fn name_of(ast: &AstArena<Untyped>, tree: TreeId<Untyped>, typer: &SourceTyper<'_>) -> String {
    let node = ast.get(tree);
    let name = match &node.kind {
        TreeKind::ValDef(definition) => definition.name.as_name().text(),
        TreeKind::DefDef(definition) => definition.name.as_name().text(),
        _ => panic!("expected a class member definition"),
    };
    typer.store().names.resolve(name).to_owned()
}

#[test]
fn private_primitive_fields_keep_source_identity_visibility_and_ordered_initializers() {
    with_source(PRIVATE_VALUES, |ast, source, index, definitions, typer| {
        let class = named_symbol(ast, source, index, typer, "PrivateValues");
        let (_, template_tree) = class_tree(ast, source, index, class);
        let TreeKind::Template(template) = &ast.get(template_tree).kind else {
            panic!("class RHS is a Template")
        };
        let members = template
            .body
            .iter()
            .map(|tree| name_of(ast, *tree, typer))
            .collect::<Vec<_>>();
        assert_eq!(members, ["seed", "enabled"]);
        assert_eq!(typer.store().symbols.get(class).kind, SymbolKind::Class);
        assert_eq!(
            index
                .scope_of(class)
                .map(|scope| typer.store().scopes.get(scope).owner),
            Some(Some(class))
        );

        for (name, expected) in [("seed", definitions.int), ("enabled", definitions.boolean)] {
            let (symbol, declaration, rhs) = field(ast, source, index, typer, name);
            let info = typer.store().symbols.get(symbol);
            assert_eq!(info.kind, SymbolKind::Field);
            assert_eq!(info.owner, Some(class));
            assert_eq!(info.visibility, Visibility::Private);
            assert!(!info.flags.contains(SymbolFlags::MUTABLE));
            typer.complete_symbol(symbol).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(symbol).info else {
                panic!("field type should be complete")
            };
            assert_eq!(
                typer.store().types.get(field_type).reference_symbol(),
                typer.store().types.get(expected).reference_symbol()
            );
            let context = typer.field_initializer_context_for(symbol).unwrap();
            let typed = typer
                .type_expression_expected(rhs, context, field_type)
                .unwrap();
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
            assert_eq!(index.symbol_at(source, declaration), Some(symbol));
        }
    });
}

#[test]
fn plain_constructor_parameter_and_counter_initializer_keep_distinct_owner_ids() {
    with_source(BOX, |ast, source, index, definitions, typer| {
        let class = named_symbol(ast, source, index, typer, "Box");
        let (_, template_tree) = class_tree(ast, source, index, class);
        let TreeKind::Template(template) = &ast.get(template_tree).kind else {
            panic!("class RHS is a Template")
        };
        let constructor = index
            .symbol_at(source, template.constructor)
            .expect("primary constructor has a canonical symbol");
        let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
            panic!("primary constructor tree is a DefDef")
        };
        let [parameter_tree] = constructor_def.value_param_clauses[0].as_slice() else {
            panic!("Box has one plain constructor parameter")
        };
        let source_parameter = index.symbol_at(source, *parameter_tree).unwrap();
        let constructor_parameter = index
            .derived_symbol_at(constructor, source, *parameter_tree)
            .expect("constructor has a separate derived parameter identity");
        assert_ne!(source_parameter, constructor_parameter);
        assert_eq!(
            typer.store().symbols.get(source_parameter).owner,
            Some(class)
        );
        assert_eq!(
            typer.store().symbols.get(constructor_parameter).owner,
            Some(constructor)
        );
        assert_eq!(
            typer.store().symbols.get(source_parameter).kind,
            SymbolKind::Parameter
        );
        assert_eq!(
            typer.store().symbols.get(constructor_parameter).kind,
            SymbolKind::Parameter
        );
        assert!(
            template
                .body
                .iter()
                .all(|tree| name_of(ast, *tree, typer) != "start")
        );
        assert!(template.body.iter().all(|tree| {
            !matches!(&ast.get(*tree).kind, TreeKind::ValDef(def) if typer.store().names.resolve(def.name.as_name().text()) == "start")
        }));

        typer.complete_symbol(constructor).unwrap();
        let SymbolInfo::Complete(constructor_type) = typer.store().symbols.get(constructor).info
        else {
            panic!("constructor signature is complete")
        };
        let Type::Method(signature) = typer.store().types.get(constructor_type) else {
            panic!("constructor signature is a Method type")
        };
        assert_eq!(signature.params.len(), 1);
        assert_eq!(
            typer
                .store()
                .types
                .get(signature.params[0].ty)
                .reference_symbol(),
            typer.store().types.get(definitions.int).reference_symbol()
        );
        assert!(matches!(
            typer.store().types.get(signature.result),
            Type::TypeRef { target: dotty_core::types::TypeRefTarget::Symbol(owner), .. }
                if *owner == class
        ));
    });

    with_source(COUNTER, |ast, source, index, definitions, typer| {
        let class = named_symbol(ast, source, index, typer, "Counter");
        typer.complete_symbol(class).unwrap();
        let (_, template_tree) = class_tree(ast, source, index, class);
        let TreeKind::Template(template) = &ast.get(template_tree).kind else {
            panic!("class RHS is a Template")
        };
        let body_names = template
            .body
            .iter()
            .map(|tree| name_of(ast, *tree, typer))
            .collect::<Vec<_>>();
        assert_eq!(body_names, ["current", "get", "set", "increment"]);

        let (field_symbol, declaration, initializer) = field(ast, source, index, typer, "current");
        let field_info = typer.store().symbols.get(field_symbol);
        assert_eq!(field_info.kind, SymbolKind::Field);
        assert_eq!(field_info.owner, Some(class));
        assert_eq!(field_info.visibility, Visibility::Private);
        assert!(field_info.flags.contains(SymbolFlags::MUTABLE));
        assert_eq!(index.symbol_at(source, declaration), Some(field_symbol));
        typer.complete_symbol(field_symbol).unwrap();
        let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field_symbol).info else {
            panic!("current has a completed field type")
        };
        assert_eq!(
            typer.store().types.get(field_type).reference_symbol(),
            typer.store().types.get(definitions.int).reference_symbol()
        );

        let constructor = index
            .symbol_at(source, template.constructor)
            .expect("primary constructor has a canonical symbol");
        let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
            panic!("primary constructor tree is a DefDef")
        };
        let [parameter_tree] = constructor_def.value_param_clauses[0].as_slice() else {
            panic!("Counter has one plain constructor parameter")
        };
        let source_parameter = index.symbol_at(source, *parameter_tree).unwrap();
        let constructor_parameter = index
            .derived_symbol_at(constructor, source, *parameter_tree)
            .expect("constructor-owned parameter identity");
        assert_ne!(source_parameter, constructor_parameter);
        assert_eq!(
            typer.store().symbols.get(source_parameter).owner,
            Some(class)
        );
        assert_eq!(
            typer.store().symbols.get(constructor_parameter).owner,
            Some(constructor)
        );
        assert_eq!(
            typer.store().symbols.get(constructor_parameter).kind,
            SymbolKind::Parameter
        );

        let context = typer.field_initializer_context_for(field_symbol).unwrap();
        let typed_initializer = typer
            .type_expression_expected(initializer, context, field_type)
            .expect("plain constructor parameter is a valid field initializer");
        assert_eq!(
            typer.source_typed_index().get(source, initializer),
            Some(typed_initializer)
        );
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(typed_initializer).ty),
            Type::TermRef { target: TermRefTarget::Symbol(target), .. }
                if *target == constructor_parameter
        ));

        for method_name in ["get", "set", "increment"] {
            let method = named_symbol(ast, source, index, typer, method_name);
            assert_eq!(typer.store().symbols.get(method).owner, Some(class));
            typer.complete_symbol(method).unwrap();
            let method_tree = ast
                .iter()
                .find_map(|(tree, node)| {
                    (index.symbol_at(source, tree) == Some(method)
                        && matches!(node.kind, TreeKind::DefDef(_)))
                    .then_some(tree)
                })
                .unwrap();
            let TreeKind::DefDef(definition) = &ast.get(method_tree).kind else {
                unreachable!()
            };
            let rhs = definition.rhs.expect("source method has a body");
            let context = typer.expression_context_for(method).unwrap();
            let typed_rhs = typer.type_expression(rhs, context).unwrap();
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
        }

        let field_references = typer
            .typed_ast()
            .iter()
            .filter(|(_, node)| {
                matches!(
                    typer.store().types.get(node.ty),
                    Type::TermRef { target: TermRefTarget::Symbol(target), .. }
                        if *target == field_symbol
                )
            })
            .count();
        assert!(
            field_references >= 5,
            "getter and assignments keep the field ID"
        );
    });
}

#[test]
fn initializer_reads_keep_field_ids_without_claiming_safe_initialization_order() {
    let source_text = "class InitializationReads:\n  private val first: Int = 1\n  private val later: Int = first\n  private val selfRead: Int = selfRead\n  private val forward: Int = last\n  private val last: Int = 2\n";
    with_source(source_text, |ast, source, index, _definitions, typer| {
        for (field_name, target_name) in [
            ("later", "first"),
            ("selfRead", "selfRead"),
            ("forward", "last"),
        ] {
            let (field_symbol, _, rhs) = field(ast, source, index, typer, field_name);
            let target = named_symbol(ast, source, index, typer, target_name);
            typer.complete_symbol(field_symbol).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field_symbol).info
            else {
                panic!("field type should be complete")
            };
            let context = typer.field_initializer_context_for(field_symbol).unwrap();
            let typed_rhs = typer
                .type_expression_expected(rhs, context, field_type)
                .expect("typed name resolution is independent of initialization safety");
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(typed_rhs).ty),
                Type::TermRef { target: TermRefTarget::Symbol(actual), .. }
                    if *actual == target
            ));
        }
    });
}

#[test]
fn supported_frontend_diagnostics_and_visibility_variants_are_pinned() {
    let variants = "class VisibilityVariants:\n  val publicValue: Int = 1\n  protected val protectedValue: Int = 2\n  private[this] val ownValue: Int = 3\n  lazy val deferredValue: Int = 4\n";
    with_source(variants, |ast, source, index, _, typer| {
        for (name, visibility, flag) in [
            ("publicValue", Visibility::Public, None),
            ("protectedValue", Visibility::Protected, None),
            ("ownValue", Visibility::Private, None),
            ("deferredValue", Visibility::Public, Some(SymbolFlags::LAZY)),
        ] {
            let symbol = named_symbol(ast, source, index, typer, name);
            let info = typer.store().symbols.get(symbol);
            assert_eq!(info.kind, SymbolKind::Field);
            assert_eq!(info.visibility, visibility);
            if let Some(flag) = flag {
                assert!(info.flags.contains(flag));
            }
        }
        let own = ast
            .iter()
            .find_map(|(_tree, node)| {
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                (typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text())
                    == "ownValue")
                    .then_some(definition.metadata.visibility)
            })
            .unwrap();
        assert!(matches!(
            own,
            Some(dotty_core::ast::VisibilitySyntax::Private { qualifier: Some(_) })
        ));
    });

    with_source(
        "class WrongInitializer:\n  private val wrong: Int = true\n",
        |ast, source, index, _, typer| {
            let (wrong, _, rhs) = field(ast, source, index, typer, "wrong");
            typer.complete_symbol(wrong).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(wrong).info else {
                panic!("declared field type should complete despite the bad RHS")
            };
            let context = typer.field_initializer_context_for(wrong).unwrap();
            assert!(matches!(
                typer.type_expression_expected(rhs, context, field_type),
                Err(TyperError::ExpectedExpressionTypeMismatch { source: error_source, .. })
                    if error_source == source
            ));
            assert!(typer.source_typed_index().get(source, rhs).is_none());
        },
    );

    with_source(
        "class MissingInitializer:\n  private val missing: Int\n",
        |ast, source, index, _, typer| {
            let tree = ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::ValDef(definition) = &node.kind else {
                        return None;
                    };
                    (typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == "missing")
                        .then_some((tree, definition.rhs))
                })
                .unwrap();
            assert_eq!(
                index.symbol_at(source, tree.0).map(|symbol| typer
                    .store()
                    .symbols
                    .get(symbol)
                    .kind),
                Some(SymbolKind::Field)
            );
            assert!(
                tree.1.is_none(),
                "naming preserves the missing initializer as absent"
            );
        },
    );
}
