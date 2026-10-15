use dotty_classfile::access_flags::{ACC_PUBLIC, MethodAccessFlags};
use dotty_classfile::class_file::ClassFileVersion;
use dotty_classfile::class_file_builder::ClassFileBuilder;
use dotty_classfile::reader::Reader;
use dotty_core::ast::{TreeKind, Untyped};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, TermRefTarget, TreeId, Type, Visibility,
};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmFieldPlan, JvmMethodPlan,
    emit_class,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

fn with_source<R>(
    source_text: &str,
    f: impl FnOnce(
        &dotty_core::AstArena<Untyped>,
        SourceId,
        &dotty_namer::SourceSemanticIndex,
        &Definitions,
        &mut SourceTyper<'_>,
    ) -> R,
) -> R {
    let source_id = SourceId::from_index(1);
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());

    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "Stateful.scala",
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
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    f(&parsed.ast, source_id, &index, &definitions, &mut typer)
}

fn symbol_named(
    ast: &dotty_core::AstArena<Untyped>,
    index: &dotty_namer::SourceSemanticIndex,
    source: SourceId,
    typer: &SourceTyper<'_>,
    name: &str,
) -> SymbolId {
    ast.iter()
        .find_map(|(tree, _)| {
            let symbol = index.symbol_at(source, tree)?;
            (typer
                .store()
                .names
                .resolve(typer.store().symbols.get(symbol).name.text())
                == name)
                .then_some(symbol)
        })
        .unwrap_or_else(|| panic!("missing source symbol {name}"))
}

fn field_tree_and_rhs(
    ast: &dotty_core::AstArena<Untyped>,
    index: &dotty_namer::SourceSemanticIndex,
    source: SourceId,
    typer: &SourceTyper<'_>,
    name: &str,
) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
    ast.iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            let symbol = index.symbol_at(source, tree)?;
            let symbol_name = typer
                .store()
                .names
                .resolve(definition.name.as_name().text());
            if symbol_name != name {
                return None;
            }
            Some((symbol, tree, definition.rhs?))
        })
        .unwrap_or_else(|| panic!("missing initialized field {name}"))
}

#[test]
fn private_primitive_field_initializers_keep_canonical_symbols_and_typed_roots() {
    with_source(
        "class Plain { private val seed: Int = 42; private val enabled: Boolean = true }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Plain");
            assert_eq!(typer.store().symbols.get(class).kind, SymbolKind::Class);
            assert_eq!(
                typer.store().symbols.get(class).visibility,
                Visibility::Public
            );
            typer.complete_symbol(class).unwrap();
            assert!(matches!(
                typer.store().symbols.get(class).info,
                SymbolInfo::Complete(_)
            ));
            let fields =
                ["seed", "enabled"].map(|name| field_tree_and_rhs(ast, index, source, typer, name));
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let body_order = template
                .body
                .iter()
                .filter_map(|tree| index.symbol_at(source, *tree))
                .collect::<Vec<_>>();
            assert_eq!(
                body_order,
                fields
                    .iter()
                    .map(|(field, _, _)| *field)
                    .collect::<Vec<_>>()
            );

            for ((field, declaration, rhs), (expected_type, expected_constant)) in
                fields.into_iter().zip([
                    (definitions.int, dotty_core::Constant::Int(42)),
                    (definitions.boolean, dotty_core::Constant::Boolean(true)),
                ])
            {
                let symbol = typer.store().symbols.get(field);
                assert_eq!(symbol.kind, SymbolKind::Field);
                assert_eq!(symbol.owner, Some(class));
                assert_eq!(symbol.visibility, Visibility::Private);
                assert!(!symbol.flags.contains(SymbolFlags::MUTABLE));
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(ty) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                assert_eq!(
                    typer.store().types.get(ty).reference_symbol(),
                    typer.store().types.get(expected_type).reference_symbol()
                );

                let context = typer.field_initializer_context_for(field).unwrap();
                let typed_rhs = typer.type_expression_expected(rhs, context, ty).unwrap();
                assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
                assert_eq!(
                    typer.store().types.get(typer.typed_ast().get(typed_rhs).ty),
                    &Type::Constant(expected_constant)
                );
                assert_eq!(index.symbol_at(source, declaration), Some(field));
            }
        },
    );
}

#[test]
fn primary_constructor_parameter_and_private_var_remain_separate_identities() {
    with_source(
        "class Counter(start: Int) { private var current: Int = start; def get(): Int = current }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Counter");
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "current");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let method = symbol_named(ast, index, source, typer, "get");
            let method_rhs = ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    (index.symbol_at(source, tree) == Some(method))
                        .then_some(definition.rhs)
                        .flatten()
                })
                .expect("get method should have a source body");
            assert_eq!(
                template
                    .body
                    .iter()
                    .filter_map(|tree| index.symbol_at(source, *tree))
                    .collect::<Vec<_>>(),
                vec![field, method]
            );
            let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
                panic!("primary constructor should be a DefDef");
            };
            assert_eq!(constructor_def.value_param_clauses.len(), 1);
            assert_eq!(constructor_def.value_param_clauses[0].len(), 1);
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            assert_eq!(
                typer.store().symbols.get(constructor).kind,
                SymbolKind::Constructor
            );
            assert_eq!(typer.store().symbols.get(constructor).owner, Some(class));
            assert_eq!(
                typer.store().symbols.get(constructor).visibility,
                Visibility::Public
            );
            let parameter_tree = constructor_def.value_param_clauses[0][0];
            let canonical_parameter = index.symbol_at(source, parameter_tree).unwrap();
            let constructor_parameter = index
                .derived_symbol_at(constructor, source, parameter_tree)
                .unwrap();

            assert_eq!(
                typer.store().symbols.get(canonical_parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer.store().symbols.get(canonical_parameter).owner,
                Some(class)
            );
            assert_eq!(
                typer.store().symbols.get(canonical_parameter).visibility,
                Visibility::Private
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).visibility,
                Visibility::Public
            );
            assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
            assert_eq!(typer.store().symbols.get(field).owner, Some(class));
            assert!(
                typer
                    .store()
                    .symbols
                    .get(field)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
            );
            assert_eq!(
                typer.store().symbols.get(field).visibility,
                Visibility::Private
            );
            assert_ne!(canonical_parameter, constructor_parameter);
            assert_eq!(
                typer.store().symbols.get(canonical_parameter).owner,
                Some(class)
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).owner,
                Some(constructor)
            );
            assert!(
                !typer
                    .store()
                    .symbols
                    .get(constructor_parameter)
                    .flags
                    .contains(SymbolFlags::MUTABLE)
            );
            assert_ne!(field, constructor_parameter);

            typer.complete_symbol(constructor).unwrap();
            typer.complete_symbol(class).unwrap();
            let SymbolInfo::Complete(constructor_type) =
                typer.store().symbols.get(constructor).info
            else {
                panic!("constructor signature should be complete");
            };
            let Type::Method(signature) = typer.store().types.get(constructor_type) else {
                panic!("ordinary primary constructor should have one method clause");
            };
            assert_eq!(signature.params.len(), 1);
            assert_eq!(signature.kind, dotty_core::MethodKind::Plain);
            assert_eq!(signature.params[0].ty, definitions.int);
            let constructor_parameter_type = signature.params[0].ty;
            assert_eq!(
                typer.store().types.get(signature.result).reference_symbol(),
                Some(class)
            );
            assert!(matches!(
                typer.store().symbols.get(class).info,
                SymbolInfo::Complete(_)
            ));

            typer.complete_symbol(field).unwrap();
            let context = typer.field_initializer_context_for(field).unwrap();
            let typed_rhs = typer
                .type_expression_expected(rhs, context, constructor_parameter_type)
                .unwrap();
            assert_eq!(typer.type_expression(rhs, context).unwrap(), typed_rhs);
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
            assert!(matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(typed_rhs).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if *symbol == constructor_parameter
            ));
            assert_eq!(
                typer.store().types.get(constructor_parameter_type),
                typer.store().types.get(definitions.int)
            );

            typer.complete_symbol(method).unwrap();
            let method_context = typer.expression_context_for(method).unwrap();
            let typed_method_rhs = typer.type_expression(method_rhs, method_context).unwrap();
            assert!(matches!(
                typer
                    .store()
                    .types
                    .get(typer.typed_ast().get(typed_method_rhs).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if *symbol == field
            ));
        },
    );
}

#[test]
fn val_and_var_constructor_parameters_have_distinct_field_and_parameter_symbols() {
    for source_text in ["class C(val x: Int)", "class C(var x: Int)"] {
        with_source(source_text, |ast, source, index, _, typer| {
            let class = symbol_named(ast, index, source, typer, "C");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
                panic!("primary constructor should be a DefDef");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let parameter_tree = constructor_def.value_param_clauses[0][0];
            let field = index.symbol_at(source, parameter_tree).unwrap();
            let parameter = index
                .derived_symbol_at(constructor, source, parameter_tree)
                .unwrap();

            assert_ne!(field, parameter);
            assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
            assert_eq!(typer.store().symbols.get(field).owner, Some(class));
            assert_eq!(
                typer.store().symbols.get(parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer.store().symbols.get(parameter).owner,
                Some(constructor)
            );
            assert_eq!(
                typer
                    .store()
                    .symbols
                    .get(field)
                    .flags
                    .contains(SymbolFlags::MUTABLE),
                source_text.contains("var x")
            );
        });
    }
}

#[test]
fn wrong_typed_field_initializer_is_a_typer_error_and_rolls_back_mapping() {
    with_source(
        "class Broken { private val count: Int = true }",
        |ast, source, index, definitions, typer| {
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "count");
            typer.complete_symbol(field).unwrap();
            let context = typer.field_initializer_context_for(field).unwrap();
            let store_checkpoint = typer.store().checkpoint();
            let typed_checkpoint = typer.typed_ast().checkpoint();

            let result = typer.type_expression_expected(rhs, context, definitions.int);
            assert!(
                matches!(
                    result,
                    Err(dotty_typer::TyperError::ExpectedExpressionTypeMismatch {
                        source: actual_source,
                        tree_index,
                        expected,
                        ..
                    }) if actual_source == source && tree_index == rhs.index() && expected == definitions.int
                ),
                "unexpected typing result: {result:?}"
            );
            assert_eq!(typer.store().checkpoint(), store_checkpoint);
            assert_eq!(typer.typed_ast().checkpoint(), typed_checkpoint);
            assert_eq!(typer.source_typed_index().get(source, rhs), None);

            let unchecked = typer.type_expression(rhs, context).unwrap();
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(unchecked));
            assert_eq!(
                typer.store().types.get(typer.typed_ast().get(unchecked).ty),
                &Type::Constant(dotty_core::Constant::Boolean(true))
            );
        },
    );
}

#[test]
fn duplicate_source_fields_are_not_yet_rejected_by_frontend() {
    let source_text = "class Duplicate { val value: Int = 1; val value: Int = 2 }";
    let source_id = SourceId::from_index(1);
    let source = SourceText::new(source_text).unwrap();
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let outcome = name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "Duplicate.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap();
    let index = match outcome {
        NamingOutcome::Named { index, diagnostics } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            index
        }
        NamingOutcome::Rejected { diagnostics } => {
            panic!("this audit expects to observe current namer behavior: {diagnostics:?}")
        }
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("parser should return a package root");
    };
    let TreeKind::TypeDef(class_def) = &parsed.ast.get(package.stats[0]).kind else {
        panic!("source declaration should be a class");
    };
    let class = index.symbol_at(source_id, package.stats[0]).unwrap();
    let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
        panic!("class RHS should be a template");
    };
    let duplicate_fields = template
        .body
        .iter()
        .filter_map(|tree| {
            let symbol = index.symbol_at(source_id, *tree)?;
            (store.symbols.get(symbol).kind == SymbolKind::Field).then_some(symbol)
        })
        .collect::<Vec<_>>();
    assert_eq!(duplicate_fields.len(), 2);
    assert_ne!(duplicate_fields[0], duplicate_fields[1]);
    for field in duplicate_fields {
        assert_eq!(store.symbols.get(field).owner, Some(class));
        assert_eq!(
            store.names.resolve(store.symbols.get(field).name.text()),
            "value"
        );
    }
}

#[test]
fn unsupported_constructor_forms_are_accepted_with_canonical_typer_signatures() {
    with_source(
        "class WithSecondary(value: Int) { def this() = this(0) }",
        |ast, source, index, definitions, typer| {
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let primary = index.symbol_at(source, template.constructor).unwrap();
            let secondary = template
                .body
                .iter()
                .find_map(|tree| {
                    let symbol = index.symbol_at(source, *tree)?;
                    (symbol != primary
                        && typer.store().symbols.get(symbol).kind == SymbolKind::Constructor)
                        .then_some(symbol)
                })
                .expect("secondary constructor should have a source symbol");

            for (constructor, parameter_count) in [(primary, 1), (secondary, 0)] {
                typer.complete_symbol(constructor).unwrap();
                let SymbolInfo::Complete(ty) = typer.store().symbols.get(constructor).info else {
                    panic!("constructor signature should be complete");
                };
                let Type::Method(signature) = typer.store().types.get(ty) else {
                    panic!("constructor should have a method signature");
                };
                assert_eq!(signature.params.len(), parameter_count);
                assert_eq!(signature.kind, dotty_core::MethodKind::Plain);
                if parameter_count == 1 {
                    assert_eq!(signature.params[0].ty, definitions.int);
                }
            }
        },
    );

    with_source(
        "class WithUsing(using context: Int)",
        |ast, source, index, definitions, typer| {
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let SymbolInfo::Complete(ty) = typer.store().symbols.get(constructor).info else {
                panic!("using constructor signature should be complete");
            };
            let Type::Method(signature) = typer.store().types.get(ty) else {
                panic!("using constructor should have a method signature");
            };
            assert_eq!(signature.params.len(), 1);
            assert_eq!(signature.params[0].ty, definitions.int);
            assert_eq!(signature.kind, dotty_core::MethodKind::Contextual);
        },
    );

    with_source(
        "class WithDefault(value: Int = 1)",
        |ast, source, index, definitions, typer| {
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let SymbolInfo::Complete(ty) = typer.store().symbols.get(constructor).info else {
                panic!("defaulted constructor signature should be complete");
            };
            let Type::Method(signature) = typer.store().types.get(ty) else {
                panic!("defaulted constructor should have a method signature");
            };
            assert_eq!(signature.params.len(), 1);
            assert_eq!(signature.params[0].ty, definitions.int);
            assert_eq!(signature.kind, dotty_core::MethodKind::Plain);
        },
    );
}

#[test]
fn non_private_visibility_forms_are_canonicalized_and_self_forward_reads_type() {
    with_source(
        "class Visibility { val publicField: Int = 1; protected val protectedField: Int = 2; private[this] val thisOnly: Int = 3 }",
        |ast, source, index, _definitions, typer| {
            for (name, visibility) in [
                ("publicField", Visibility::Public),
                ("protectedField", Visibility::Protected),
                ("thisOnly", Visibility::Private),
            ] {
                let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, name);
                assert_eq!(typer.store().symbols.get(field).visibility, visibility);
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(expected) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let typed = typer
                    .type_expression_expected(rhs, context, expected)
                    .unwrap();
                assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed));
            }
        },
    );

    with_source(
        "class Reads { private val self: Int = this.self; private val earlier: Int = 1; private val forward: Int = later; private val later: Int = 2 }",
        |ast, source, index, _, typer| {
            for (name, target_name) in [("self", "self"), ("forward", "later")] {
                let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, name);
                let target = field_tree_and_rhs(ast, index, source, typer, target_name).0;
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(expected) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let typed = typer
                    .type_expression_expected(rhs, context, expected)
                    .unwrap();
                assert!(matches!(
                    typer.store().types.get(typer.typed_ast().get(typed).ty),
                    Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                        if *symbol == target
                ));
            }
        },
    );
}

#[test]
fn real_source_private_primitive_fields_project_in_frontend_order() {
    let snapshot = || {
        with_source(
            "class Fields { private val seed: Int = 42; private var enabled: Boolean = true }",
            |ast, source, index, definitions, typer| {
                let class = symbol_named(ast, index, source, typer, "Fields");
                typer.complete_symbol(class).unwrap();

                let mut fields = Vec::new();
                for name in ["seed", "enabled"] {
                    let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, name);
                    typer.complete_symbol(field).unwrap();
                    let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info
                    else {
                        panic!("field type should be complete");
                    };
                    let context = typer.field_initializer_context_for(field).unwrap();
                    let initializer = typer
                        .type_expression_expected(rhs, context, field_type)
                        .unwrap();
                    fields.push(JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(initializer),
                    });
                }

                let unit = JvmCompilationUnit {
                    store: typer.store(),
                    typed_ast: typer.typed_ast(),
                    classes: vec![JvmClassPlan {
                        class,
                        methods: Vec::new(),
                        fields,
                        initialization: JvmClassInitializationPlan::Unsupported,
                    }],
                };
                let metadata = unit.metadata(definitions).unwrap();
                let projected = metadata[0].fields();
                assert_eq!(projected.len(), 2);
                assert_eq!(projected[0].name(), "seed");
                assert_eq!(projected[0].descriptor(), "I");
                assert!(projected[0].access_flags().is_private());
                assert!(projected[0].access_flags().is_final());
                assert_eq!(projected[1].name(), "enabled");
                assert_eq!(projected[1].descriptor(), "Z");
                assert!(projected[1].access_flags().is_private());
                assert!(!projected[1].access_flags().is_final());

                assert_eq!(unit.metadata(definitions).unwrap(), metadata);
                let snapshot = projected
                    .iter()
                    .map(|field| {
                        (
                            field.name().to_owned(),
                            field.descriptor().to_owned(),
                            field.access_flags().0,
                        )
                    })
                    .collect::<Vec<_>>();

                let mut emission_plan = unit.classes[0].clone();
                emission_plan.initialization = JvmClassInitializationPlan::ImplicitNoArg;
                let emission_unit = JvmCompilationUnit {
                    store: unit.store,
                    typed_ast: unit.typed_ast,
                    classes: vec![emission_plan],
                };
                assert!(matches!(
                    emit_class(&emission_unit, class, definitions),
                    Err(dotty_jvm::JvmCodegenError::FieldEmissionUnsupported {
                        class: actual_class,
                        field: actual_field,
                    }) if actual_class == class && actual_field == projected[0].symbol()
                ));
                snapshot
            },
        )
    };
    assert_eq!(snapshot(), snapshot());

    with_source(
        "class Counter(start: Int) { private var current: Int = start }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Counter");
            typer.complete_symbol(class).unwrap();
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "current");
            typer.complete_symbol(field).unwrap();
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
                panic!("primary constructor should be a DefDef");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let source_parameter = constructor_def.value_param_clauses[0][0];
            let constructor_parameter = index
                .derived_symbol_at(constructor, source, source_parameter)
                .unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("field type should be complete");
            };
            let context = typer.field_initializer_context_for(field).unwrap();
            let initializer = typer
                .type_expression_expected(rhs, context, field_type)
                .unwrap();
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(initializer).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if *symbol == constructor_parameter
            ));

            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields: vec![JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(initializer),
                    }],
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            let metadata = unit.metadata(definitions).unwrap();
            let field_metadata = &metadata[0].fields()[0];
            assert_eq!(field_metadata.symbol(), field);
            assert_eq!(field_metadata.descriptor(), "I");
            assert!(field_metadata.access_flags().is_private());
            assert!(!field_metadata.access_flags().is_final());
        },
    );
}

#[test]
fn real_typed_current_instance_field_reads_lower_to_aload_getfield() {
    with_source(
        "class Current { private var current: Int = 5; private val flag: Boolean = true; def get(): Int = current; def getThis(): Int = this.current; def isEnabled(): Boolean = this.flag }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Current");
            typer.complete_symbol(class).unwrap();

            let mut fields = Vec::new();
            for name in ["current", "flag"] {
                let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, name);
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let initializer = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                fields.push(JvmFieldPlan {
                    field,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(initializer),
                });
            }

            let mut methods = Vec::new();
            for name in ["get", "getThis", "isEnabled"] {
                let (method, rhs) = ast
                    .iter()
                    .find_map(|(tree, node)| {
                        let TreeKind::DefDef(definition) = &node.kind else {
                            return None;
                        };
                        if typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text())
                            == name
                        {
                            Some((index.symbol_at(source, tree)?, definition.rhs?))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| panic!("missing method {name}"));
                typer.complete_symbol(method).unwrap();
                let context = typer.expression_context_for(method).unwrap();
                let body = typer.type_expression(rhs, context).unwrap();
                methods.push(JvmMethodPlan {
                    method,
                    parameters: Vec::new(),
                    locals: Vec::new(),
                    body: Some(body),
                });
            }
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods,
                    fields,
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };

            let lower_and_encode = |name: &str, descriptor: &str| {
                let method = unit.classes[0]
                    .methods
                    .iter()
                    .find(|method| {
                        typer
                            .store()
                            .names
                            .resolve(typer.store().symbols.get(method.method).name.text())
                            == name
                    })
                    .unwrap()
                    .method;
                let code = unit.lower_method_body(class, method, definitions).unwrap();
                let mut builder = ClassFileBuilder::new(
                    "Current",
                    ClassFileVersion {
                        major: 65,
                        minor: 0,
                    },
                )
                .unwrap();
                builder
                    .add_prebuilt_code_method(name, descriptor, MethodAccessFlags(ACC_PUBLIC), code)
                    .unwrap();
                let bytes = builder.encode().unwrap();
                let mut reader = Reader::new(&bytes);
                let class_file =
                    dotty_classfile::class_file::ClassFile::decode(&mut reader).unwrap();
                let code = class_file.methods[0]
                    .attributes
                    .iter()
                    .find_map(|attribute| match attribute {
                        dotty_classfile::attribute::Attribute::Code(code) => {
                            Some(code.code.to_vec())
                        }
                        _ => None,
                    })
                    .unwrap();
                let index = u16::from_be_bytes([code[2], code[3]]);
                let dotty_classfile::constant_pool::ConstantPoolEntry::Fieldref {
                    class_index,
                    name_and_type_index,
                } = class_file
                    .constant_pool
                    .get(dotty_classfile::constant_pool::ConstantPoolIndex(index))
                    .unwrap()
                else {
                    panic!("getfield must reference Fieldref")
                };
                let owner = class_file
                    .constant_pool
                    .class_name(*class_index)
                    .unwrap()
                    .to_owned();
                let (field, field_descriptor) = class_file
                    .constant_pool
                    .name_and_type(*name_and_type_index)
                    .unwrap();
                (code, owner, field.to_owned(), field_descriptor.to_owned())
            };

            let integer = lower_and_encode("get", "()I");
            let explicit = lower_and_encode("getThis", "()I");
            let boolean = lower_and_encode("isEnabled", "()Z");
            for ((code, owner, field, descriptor), method, expected_field, expected_descriptor) in [
                (&integer, "get", "current", "I"),
                (&explicit, "getThis", "current", "I"),
                (&boolean, "isEnabled", "flag", "Z"),
            ] {
                assert_eq!(&code[0..1], &[0x2a], "aload_0 for {method}");
                assert_eq!(code[1], 0xb4, "getfield for {method}");
                assert_eq!(code[4], 0xac, "ireturn for {method}");
                assert_eq!(owner, "Current");
                assert_eq!(field, expected_field);
                assert_eq!(descriptor, expected_descriptor);
            }
        },
    );
}

#[test]
fn field_reads_follow_canonical_bindings_and_fail_closed_outside_the_supported_subset() {
    with_source(
        "class Current { private val current: Int = 5; private val hidden: Int = 6; def get(): Int = current; def shadow(current: Int): Int = current; def branch(flag: Boolean): Int = if (flag) current else 0; def select(other: Current): Int = other.current; def readHidden(): Int = hidden }\nclass Other { private val foreign: Int = 1; def read(): Int = this.foreign }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Current");
            let other_class = symbol_named(ast, index, source, typer, "Other");
            typer.complete_symbol(class).unwrap();
            typer.complete_symbol(other_class).unwrap();

            let mut fields = Vec::new();
            for name in ["current", "hidden", "foreign"] {
                let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, name);
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let initializer = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                fields.push(JvmFieldPlan {
                    field,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(initializer),
                });
            }

            let mut methods = Vec::new();
            for (owner, name) in [
                (class, "get"),
                (class, "shadow"),
                (class, "branch"),
                (class, "select"),
                (class, "readHidden"),
                (other_class, "read"),
            ] {
                let (method, rhs, parameters) = ast
                    .iter()
                    .find_map(|(tree, node)| {
                        let TreeKind::DefDef(definition) = &node.kind else {
                            return None;
                        };
                        if typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text())
                            != name
                        {
                            return None;
                        }
                        let method = index.symbol_at(source, tree)?;
                        if typer.store().symbols.get(method).owner != Some(owner) {
                            return None;
                        }
                        let rhs = definition.rhs?;
                        let parameters = definition
                            .value_param_clauses
                            .iter()
                            .flatten()
                            .map(|parameter| index.symbol_at(source, *parameter))
                            .collect::<Option<Vec<_>>>()?;
                        Some((method, rhs, parameters))
                    })
                    .unwrap_or_else(|| panic!("missing source method {name}"));
                typer.complete_symbol(method).unwrap();
                let context = typer.expression_context_for(method).unwrap();
                let body = typer.type_expression(rhs, context).unwrap();
                methods.push((name, method, body, parameters));
            }

            let method = |name: &str| {
                methods
                    .iter()
                    .find(|(candidate, _, _, _)| *candidate == name)
                    .unwrap()
            };
            let plan_for = |method: SymbolId, body, parameters: Vec<SymbolId>, planned_fields| {
                JvmCompilationUnit {
                    store: typer.store(),
                    typed_ast: typer.typed_ast(),
                    classes: vec![JvmClassPlan {
                        class,
                        methods: vec![JvmMethodPlan {
                            method,
                            parameters,
                            locals: Vec::new(),
                            body: Some(body),
                        }],
                        fields: planned_fields,
                        initialization: JvmClassInitializationPlan::Unsupported,
                    }],
                }
            };
            let current_field = fields
                .iter()
                .find(|field| {
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(field.field).name.text())
                        == "current"
                })
                .unwrap();
            let current_plan = vec![*current_field];
            let current_id = current_field.field;

            let (_, shadow_method, shadow_body, shadow_parameters) = method("shadow");
            let shadow = plan_for(
                *shadow_method,
                *shadow_body,
                shadow_parameters.clone(),
                current_plan.clone(),
            );
            let shadow_code = shadow
                .lower_method_body(class, *shadow_method, definitions)
                .unwrap();
            assert!(format!("{shadow_code:?}").contains("ILoad"));
            assert!(!format!("{shadow_code:?}").contains("GetField"));

            let (_, branch_method, branch_body, branch_parameters) = method("branch");
            let branch = plan_for(
                *branch_method,
                *branch_body,
                branch_parameters.clone(),
                current_plan.clone(),
            );
            assert!(matches!(
                branch.lower_method_body(class, *branch_method, definitions),
                Err(dotty_jvm::JvmCodegenError::JvmFieldReadInUnsupportedControlFlow {
                    method,
                    field,
                    ..
                }) if method == *branch_method && field == current_id
            ));

            let (_, get_method, _, _) = method("get");
            let (_, _, select_body, _) = method("select");
            let other_receiver =
                plan_for(*get_method, *select_body, Vec::new(), current_plan.clone());
            assert!(matches!(
                other_receiver.lower_method_body(class, *get_method, definitions),
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmFieldReceiver {
                    field,
                    ..
                }) if field == current_id
            ));
            let (_, _, hidden_body, _) = method("readHidden");
            let hidden = plan_for(*get_method, *hidden_body, Vec::new(), current_plan.clone());
            let hidden_field = fields
                .iter()
                .find(|field| {
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(field.field).name.text())
                        == "hidden"
                })
                .unwrap()
                .field;
            assert!(matches!(
                hidden.lower_method_body(class, *get_method, definitions),
                Err(dotty_jvm::JvmCodegenError::FieldNotInJvmPlan {
                    class: actual_class,
                    field,
                    ..
                }) if actual_class == class && field == hidden_field
            ));
            let (_, _, foreign_body, _) = method("read");
            let wrong_owner = plan_for(*get_method, *foreign_body, Vec::new(), current_plan);
            let foreign_field = fields
                .iter()
                .find(|field| {
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(field.field).name.text())
                        == "foreign"
                })
                .unwrap()
                .field;
            assert!(matches!(
                wrong_owner.lower_method_body(class, *get_method, definitions),
                Err(dotty_jvm::JvmCodegenError::WrongOwner {
                    symbol,
                    expected,
                    actual: Some(actual),
                }) if symbol == foreign_field && expected == class && actual == other_class
            ));
        },
    );
}

#[test]
fn field_metadata_fails_closed_for_invalid_bindings_types_and_modifiers() {
    with_source(
        "class Fields { private val seed: Int = 42; private var enabled: Boolean = true; val visible: Int = 1; private lazy val delayed: Int = 2 }\nclass Other { private val foreign: Int = 3 }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Fields");
            let other = symbol_named(ast, index, source, typer, "Other");
            typer.complete_symbol(class).unwrap();
            typer.complete_symbol(other).unwrap();

            let typed_field = |typer: &mut SourceTyper<'_>, name: &str| {
                let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, name);
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let initializer = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                (field, initializer)
            };
            let (seed, seed_rhs) = typed_field(typer, "seed");
            let (_enabled, enabled_rhs) = typed_field(typer, "enabled");
            let (visible, visible_rhs) = typed_field(typer, "visible");
            let (delayed, delayed_rhs) = typed_field(typer, "delayed");
            let (foreign, foreign_rhs) = typed_field(typer, "foreign");
            let plan = |fields| JvmClassPlan {
                class,
                methods: Vec::new(),
                fields,
                initialization: JvmClassInitializationPlan::Unsupported,
            };
            let metadata = |plan: &JvmClassPlan| {
                plan.metadata_with_typed_ast(typer.store(), typer.typed_ast(), definitions)
            };

            assert!(matches!(
                metadata(&plan(vec![JvmFieldPlan {
                    field: foreign,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(foreign_rhs),
                }])),
                Err(dotty_jvm::JvmCodegenError::WrongOwner {
                    symbol,
                    expected,
                    actual: Some(actual),
                }) if symbol == foreign && expected == class && actual == other
            ));
            assert_eq!(
                metadata(&plan(vec![
                    JvmFieldPlan {
                        field: seed,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(seed_rhs),
                    };
                    2
                ])),
                Err(dotty_jvm::JvmCodegenError::DuplicateFieldPlan { field: seed })
            );
            assert_eq!(
                metadata(&plan(vec![JvmFieldPlan {
                    field: seed,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: None,
                }])),
                Err(dotty_jvm::JvmCodegenError::MissingFieldInitializer { field: seed })
            );
            assert_eq!(
                metadata(&plan(vec![JvmFieldPlan {
                    field: seed,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(enabled_rhs),
                }])),
                Err(dotty_jvm::JvmCodegenError::FieldInitializerTypeMismatch {
                    field: seed,
                    initializer: enabled_rhs,
                    expected: dotty_jvm::JvmType::Int,
                    found: Some(dotty_jvm::JvmType::Boolean),
                })
            );
            assert_eq!(
                metadata(&plan(vec![JvmFieldPlan {
                    field: visible,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(visible_rhs),
                }])),
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmVisibility {
                    symbol: visible,
                    visibility: Visibility::Public,
                })
            );
            assert_eq!(
                metadata(&plan(vec![JvmFieldPlan {
                    field: delayed,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(delayed_rhs),
                }])),
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmFieldFlag {
                    field: delayed,
                    flag: dotty_jvm::UnsupportedJvmFieldFlag::Lazy,
                })
            );
        },
    );

    with_source(
        "class Missing { private val value: Int = 1 }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Missing");
            let field = field_tree_and_rhs(ast, index, source, typer, "value").0;
            typer.complete_symbol(class).unwrap();
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields: vec![JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: None,
                    }],
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            assert_eq!(
                unit.metadata(definitions),
                Err(dotty_jvm::JvmCodegenError::MissingSemanticInfo { symbol: field })
            );
        },
    );

    with_source(
        "class Duplicate { private val value: Int = 1; private val value: Boolean = true }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Duplicate");
            typer.complete_symbol(class).unwrap();
            let mut fields = Vec::new();
            for tree in ast
                .iter()
                .filter_map(|(tree, node)| matches!(node.kind, TreeKind::ValDef(_)).then_some(tree))
            {
                let Some(field) = index.symbol_at(source, tree) else {
                    continue;
                };
                if typer.store().symbols.get(field).owner != Some(class) {
                    continue;
                }
                let TreeKind::ValDef(definition) = &ast.get(tree).kind else {
                    unreachable!();
                };
                let rhs = definition.rhs.unwrap();
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let initializer = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                fields.push(JvmFieldPlan {
                    field,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(initializer),
                });
            }
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields,
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            assert!(matches!(
                unit.metadata(definitions),
                Err(dotty_jvm::JvmCodegenError::DuplicateJvmFieldName {
                    class: actual_class,
                    name,
                    ..
                }) if actual_class == class && name == "value"
            ));
        },
    );
}

#[test]
fn field_metadata_never_resolves_a_shadowing_local_by_name() {
    with_source(
        "class Shadow { private val seed: Int = 42; private def read(): Int = { val seed = 7; seed } }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Shadow");
            let field = field_tree_and_rhs(ast, index, source, typer, "seed").0;
            let method = symbol_named(ast, index, source, typer, "read");
            typer.complete_symbol(class).unwrap();
            let (field, field_rhs) = {
                let (_, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "seed");
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let typed = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                (field, typed)
            };
            typer.complete_symbol(method).unwrap();
            let method_rhs = ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    (index.symbol_at(source, tree) == Some(method))
                        .then_some(definition.rhs)
                        .flatten()
                })
                .unwrap();
            let context = typer.expression_context_for(method).unwrap();
            typer
                .type_expression_expected(method_rhs, context, definitions.int)
                .unwrap();
            let local_tree = ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::ValDef(definition) = &node.kind else {
                        return None;
                    };
                    (typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == "seed"
                        && index.symbol_at(source, tree) != Some(field))
                    .then_some(tree)
                })
                .unwrap();
            let local = typer
                .typed_local_binding_at(source, local_tree)
                .expect("the shadowing local should have a typed binding");

            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields: vec![JvmFieldPlan {
                        field: local.symbol(),
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(field_rhs),
                    }],
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            assert_eq!(
                unit.metadata(definitions),
                Err(dotty_jvm::JvmCodegenError::UnsupportedSymbolKind {
                    symbol: local.symbol(),
                    expected: dotty_jvm::SymbolRole::Field,
                    found: SymbolKind::Local,
                })
            );
        },
    );
}

#[test]
fn field_metadata_rejects_reference_valued_fields() {
    with_source(
        "class References { private val self: References = this }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "References");
            typer.complete_symbol(class).unwrap();
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "self");
            typer.complete_symbol(field).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("field type should be complete");
            };
            let context = typer.field_initializer_context_for(field).unwrap();
            let initializer = typer
                .type_expression_expected(rhs, context, field_type)
                .unwrap();
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields: vec![JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(initializer),
                    }],
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            assert!(matches!(
                unit.metadata(definitions),
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmType {
                    symbol,
                    kind: dotty_jvm::UnsupportedJvmTypeKind::ReferenceClass,
                    ..
                }) if symbol == field
            ));
        },
    );
}

#[test]
fn field_metadata_distinguishes_private_this_from_plain_private() {
    with_source(
        "class ObjectPrivate { private[this] val value: Int = 1 }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "ObjectPrivate");
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "value");
            typer.complete_symbol(class).unwrap();
            typer.complete_symbol(field).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("field type should be complete");
            };
            let context = typer.field_initializer_context_for(field).unwrap();
            let initializer = typer
                .type_expression_expected(rhs, context, field_type)
                .unwrap();
            assert_eq!(
                typer.store().symbols.get(field).visibility,
                Visibility::Private
            );

            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields: vec![JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PrivateThis,
                        initializer: Some(initializer),
                    }],
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            assert_eq!(
                unit.metadata(definitions),
                Err(
                    dotty_jvm::JvmCodegenError::UnsupportedJvmFieldSourceVisibility {
                        field,
                        visibility: dotty_jvm::JvmFieldSourceVisibility::PrivateThis,
                    }
                )
            );
        },
    );
}
