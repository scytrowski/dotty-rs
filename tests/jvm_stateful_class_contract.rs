use dotty_classfile::access_flags::{ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC, MethodAccessFlags};
use dotty_classfile::class_file::ClassFileVersion;
use dotty_classfile::class_file_builder::ClassFileBuilder;
use dotty_classfile::reader::Reader;
use dotty_core::ast::{
    Apply, ApplyKind, AstArena, Select, Tree, TreeKind, Typed, Untyped, VisibilitySyntax,
};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolFlags, SymbolId, SymbolInfo,
    SymbolKind, TermRefTarget, TreeId, Type, Visibility,
};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit,
    JvmConstructorInitializerInstruction, JvmFieldPlan, JvmMethodPlan, JvmPrimaryConstructorPlan,
    emit_class,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn require_jdk21_or_skip() -> bool {
    let unavailable = ["java", "javac"]
        .into_iter()
        .filter_map(|tool| match jdk_tool_major_version(tool) {
            Some(major) if major >= 21 => None,
            Some(major) => Some(format!("{tool} {major} (requires JDK 21+)")),
            None => Some(format!("{tool} (missing or version unavailable)")),
        })
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        return true;
    }

    let message = format!(
        "JVM bytecode harness requires java and javac from JDK 21+ on PATH; unavailable: {}",
        unavailable.join(", ")
    );
    if std::env::var("DOTTY_CLASSFILE_REQUIRE_JDK").as_deref() == Ok("1") {
        panic!("{message}");
    }
    eprintln!("skipping optional JVM bytecode harness: {message}");
    false
}

fn jdk_tool_major_version(tool: &str) -> Option<u32> {
    let output = Command::new(tool).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    version_text
        .split(|character: char| !character.is_ascii_digit())
        .find_map(|component| component.parse::<u32>().ok())
}

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

fn method_tree_and_rhs(
    ast: &dotty_core::AstArena<Untyped>,
    index: &dotty_namer::SourceSemanticIndex,
    source: SourceId,
    typer: &SourceTyper<'_>,
    owner: SymbolId,
    name: &str,
) -> (SymbolId, TreeId<Untyped>, TreeId<Untyped>) {
    ast.iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let method = index.symbol_at(source, tree)?;
            if typer.store().symbols.get(method).owner == Some(owner)
                && typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text())
                    == name
            {
                Some((method, tree, definition.rhs?))
            } else {
                None
            }
        })
        .unwrap_or_else(|| panic!("missing method {name} on owner {owner:?}"))
}

fn complete_class_info(typer: &SourceTyper<'_>, class: SymbolId) -> dotty_core::types::ClassInfo {
    let SymbolInfo::Complete(info_id) = typer.store().symbols.get(class).info else {
        panic!("class symbol should have complete ClassInfo")
    };
    let Type::ClassInfo(info) = typer.store().types.get(info_id) else {
        panic!("class symbol info should be ClassInfo")
    };
    info.clone()
}

#[test]
fn private_primitive_field_initializers_keep_canonical_symbols_and_typed_roots() {
    with_source(
        "class Plain { private val seed: Int = 42; private val enabled: Boolean = true; private val large: Long = 9000000000L }",
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
            let fields = ["seed", "enabled", "large"]
                .map(|name| field_tree_and_rhs(ast, index, source, typer, name));
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
                &body_order[..fields.len()],
                fields
                    .iter()
                    .map(|(field, _, _)| *field)
                    .collect::<Vec<_>>()
            );

            for ((field, declaration, rhs), (expected_type, expected_constant)) in
                fields.into_iter().zip([
                    (definitions.int, dotty_core::Constant::Int(42)),
                    (definitions.boolean, dotty_core::Constant::Boolean(true)),
                    (definitions.long, dotty_core::Constant::Long(9_000_000_000)),
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
fn reference_fields_and_constructor_parameters_keep_real_source_identity() {
    let source_text = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/jvm/ReferenceFieldsAndConstructors.scala"),
    )
    .expect("read reference field and constructor fixture");
    with_source(&source_text, |ast, source, index, definitions, typer| {
        let class = |name: &str| symbol_named(ast, index, source, typer, name);
        let counter = class("Counter");
        let holder = class("Holder");
        let mutable_holder = class("MutableHolder");
        let client = class("Client");
        for owner in [counter, holder, mutable_holder, client] {
            typer.complete_symbol(owner).unwrap();
            let info = complete_class_info(typer, owner);
            let scope = index
                .scope_of(owner)
                .expect("class declaration scope is indexed");
            assert_eq!(info.class, owner);
            assert_eq!(info.declarations, scope);
            assert_eq!(typer.store().scopes.get(scope).owner, Some(owner));
            assert_eq!(info.parents, [definitions.object_type]);
        }

        let fields = [
            (holder, "ref", false, None),
            (holder, "own", false, Some("this")),
            (mutable_holder, "current", true, None),
        ];
        for (owner, name, mutable, expected_qualifier) in fields {
            let (field, declaration, rhs) = field_tree_and_rhs(ast, index, source, typer, name);
            let symbol = typer.store().symbols.get(field);
            assert_eq!(symbol.kind, SymbolKind::Field);
            assert_eq!(symbol.owner, Some(owner));
            assert_eq!(symbol.visibility, Visibility::Private);
            assert_eq!(
                symbol.flags.contains(SymbolFlags::MUTABLE),
                mutable,
                "field mutability for {name} comes from the real source declaration"
            );
            let TreeKind::ValDef(definition) = &ast.get(declaration).kind else {
                panic!("field source node should be ValDef");
            };
            match definition.metadata.visibility {
                Some(VisibilitySyntax::Private { qualifier }) => {
                    assert_eq!(
                        qualifier.map(|name| typer.store().names.resolve(name.text())),
                        expected_qualifier,
                        "retain the plain private vs private[this] source distinction"
                    );
                }
                other => panic!("expected source private visibility, got {other:?}"),
            }

            let marker = format!("{name}: Counter");
            let name_offset = source_text.find(&marker).unwrap() as u32;
            let tree_span = ast
                .get(declaration)
                .position
                .expect("field has source range");
            assert_eq!(tree_span.source(), source);
            let tree_range = tree_span.span().range();
            assert!(tree_range.start() <= name_offset && name_offset < tree_range.end());
            let symbol_span = symbol
                .position
                .expect("field symbol retains source position");
            assert_eq!(symbol_span.source(), source);
            let symbol_range = symbol_span.span().range();
            assert!(symbol_range.start() <= name_offset && name_offset < symbol_range.end());

            let scope = complete_class_info(typer, owner).declarations;
            let field_name = symbol.name;
            assert!(
                typer
                    .store()
                    .scopes
                    .get(scope)
                    .lookup_all(&field_name)
                    .contains(&field)
            );

            typer.complete_symbol(field).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("reference field type should be complete")
            };
            assert!(matches!(
                typer.store().types.get(field_type),
                Type::TypeRef {
                    target: dotty_core::types::TypeRefTarget::Symbol(target),
                    ..
                } if *target == counter
            ));
            let context = typer.field_initializer_context_for(field).unwrap();
            let typed_rhs = typer
                .type_expression_expected(rhs, context, field_type)
                .expect("constructor parameter is a valid typed reference field initializer");
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
            assert!(matches!(
                typer.typed_ast().get(typed_rhs).kind,
                TreeKind::Ident(_)
            ));
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(typed_rhs).ty),
                Type::TermRef {
                    target: dotty_core::types::TermRefTarget::Symbol(_),
                    ..
                }
            ));
        }

        let mut constructor_parameters = Vec::new();
        for (parameter_index, (owner, parameter_name)) in
            [(holder, "initial"), (mutable_holder, "initial")]
                .into_iter()
                .enumerate()
        {
            let class_tree = ast
                .iter()
                .find_map(|(tree, node)| {
                    (index.symbol_at(source, tree) == Some(owner)).then_some((tree, node))
                })
                .map(|(tree, _)| tree)
                .expect("class declaration tree");
            let TreeKind::TypeDef(class_definition) = &ast.get(class_tree).kind else {
                panic!("class declaration is a TypeDef");
            };
            let TreeKind::Template(template) = &ast.get(class_definition.rhs).kind else {
                panic!("class definition contains a Template");
            };
            let constructor = index
                .symbol_at(source, template.constructor)
                .expect("canonical primary constructor symbol");
            let TreeKind::DefDef(constructor_definition) = &ast.get(template.constructor).kind
            else {
                panic!("primary constructor source node is DefDef");
            };
            let [parameter_tree] = constructor_definition.value_param_clauses[0].as_slice() else {
                panic!("constructor has one source parameter");
            };
            let source_parameter = index
                .symbol_at(source, *parameter_tree)
                .expect("source parameter symbol");
            let constructor_parameter = index
                .derived_symbol_at(constructor, source, *parameter_tree)
                .expect("constructor-owned parameter identity");
            assert_eq!(
                typer
                    .store()
                    .names
                    .resolve(match &ast.get(*parameter_tree).kind {
                        TreeKind::ValDef(parameter) => parameter.name.as_name().text(),
                        _ => panic!("constructor parameter source is ValDef"),
                    }),
                parameter_name
            );
            assert_ne!(source_parameter, constructor_parameter);
            assert_eq!(
                typer.store().symbols.get(source_parameter).owner,
                Some(owner)
            );
            assert_eq!(
                typer.store().symbols.get(source_parameter).visibility,
                Visibility::Private
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).owner,
                Some(constructor)
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).kind,
                SymbolKind::Parameter
            );
            assert_eq!(
                typer.store().symbols.get(constructor_parameter).visibility,
                Visibility::Public
            );
            constructor_parameters.push((owner, constructor_parameter));
            let source_parameter_span = typer
                .store()
                .symbols
                .get(source_parameter)
                .position
                .expect("source parameter retains source position");
            let parameter_span = typer
                .store()
                .symbols
                .get(constructor_parameter)
                .position
                .expect("constructor parameter retains source position");
            assert_eq!(source_parameter_span.source(), source);
            assert_eq!(parameter_span.source(), source);
            assert_eq!(source_parameter_span.span(), parameter_span.span());
            let parameter_offset = source_text
                .match_indices("initial: Counter")
                .nth(parameter_index)
                .unwrap()
                .0 as u32;
            let parameter_range = parameter_span.span().range();
            assert!(
                parameter_range.start() <= parameter_offset
                    && parameter_offset < parameter_range.end(),
                "parameter source range {parameter_range:?} should cover offset {parameter_offset} for {owner:?}"
            );

            typer.complete_symbol(constructor).unwrap();
            let SymbolInfo::Complete(constructor_type) =
                typer.store().symbols.get(constructor).info
            else {
                panic!("constructor signature should be complete")
            };
            let Type::Method(signature) = typer.store().types.get(constructor_type) else {
                panic!("constructor signature is a Method type")
            };
            assert_eq!(signature.params.len(), 1);
            assert!(matches!(
                typer.store().types.get(signature.params[0].ty),
                Type::TypeRef {
                    target: dotty_core::types::TypeRefTarget::Symbol(target),
                    ..
                } if *target == counter
            ));
        }

        for (owner, field_name) in [
            (holder, "ref"),
            (holder, "own"),
            (mutable_holder, "current"),
        ] {
            let (_, _, rhs) = field_tree_and_rhs(ast, index, source, typer, field_name);
            let typed_rhs = typer
                .source_typed_index()
                .get(source, rhs)
                .expect("reference field initializer was typed above");
            let actual_parameter =
                match typer.store().types.get(typer.typed_ast().get(typed_rhs).ty) {
                    Type::TermRef {
                        target: dotty_core::types::TermRefTarget::Symbol(parameter),
                        ..
                    } => *parameter,
                    other => panic!("field initializer retains a parameter TermRef, got {other:?}"),
                };
            let expected_parameter = constructor_parameters
                .iter()
                .find_map(|(parameter_owner, parameter)| {
                    (*parameter_owner == owner).then_some(*parameter)
                })
                .unwrap();
            assert_eq!(actual_parameter, expected_parameter);
        }

        let (client_method, client_method_tree, construct_rhs) =
            method_tree_and_rhs(ast, index, source, typer, client, "construct");
        typer.complete_symbol(client_method).unwrap();
        let construct_context = typer.expression_context_for(client_method).unwrap();
        let construct = typer
            .type_expression(construct_rhs, construct_context)
            .expect("Client.construct types a real new Holder(c) call");
        let TreeKind::Apply(application) = &typer.typed_ast().get(construct).kind else {
            panic!("new Holder(c) remains an application");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("constructor function remains selected from the new type");
        };
        let constructor = match typer
            .store()
            .types
            .get(typer.typed_ast().get(application.function).ty)
        {
            Type::TermRef {
                target: dotty_core::types::TermRefTarget::Symbol(constructor),
                ..
            } => *constructor,
            other => panic!("constructor selection keeps canonical TermRef, got {other:?}"),
        };
        assert_eq!(typer.store().symbols.get(constructor).owner, Some(holder));
        assert_eq!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(construct).ty)
                .reference_symbol(),
            Some(holder)
        );
        assert_eq!(application.args.len(), 1);
        let TreeKind::DefDef(client_definition) = &ast.get(client_method_tree).kind else {
            panic!("Client.construct source node is a DefDef");
        };
        let [client_parameter_tree] = client_definition.value_param_clauses[0].as_slice() else {
            panic!("Client.construct has one source parameter");
        };
        let client_parameter = index
            .symbol_at(source, *client_parameter_tree)
            .expect("Client.construct parameter has canonical identity");
        assert!(matches!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(application.args[0]).ty),
            Type::TermRef {
                target: dotty_core::types::TermRefTarget::Symbol(parameter),
                ..
            } if *parameter == client_parameter
        ));
        assert!(matches!(
            typer.typed_ast().get(selection.qualifier).kind,
            TreeKind::New(_)
        ));

        let (counter_get, _, _) = method_tree_and_rhs(ast, index, source, typer, counter, "get");
        let (holder_read, _, holder_read_rhs) =
            method_tree_and_rhs(ast, index, source, typer, holder, "read");
        typer.complete_symbol(counter_get).unwrap();
        typer.complete_symbol(holder_read).unwrap();
        let context = typer.expression_context_for(holder_read).unwrap();
        let holder_read_typed = typer
            .type_expression(holder_read_rhs, context)
            .expect("immutable reference field selection ref.get() types");
        let TreeKind::Apply(application) = &typer.typed_ast().get(holder_read_typed).kind else {
            panic!("ref.get() remains an application")
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("ref.get() method selection retains its field receiver")
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef {
                target: dotty_core::types::TermRefTarget::Symbol(method),
                ..
            } if *method == counter_get
        ));
        assert!(matches!(
            &typer.typed_ast().get(selection.qualifier).kind,
            TreeKind::Ident(_)
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(selection.qualifier).ty),
            Type::TermRef {
                target: dotty_core::types::TermRefTarget::Symbol(field),
                ..
            } if *field == field_tree_and_rhs(ast, index, source, typer, "ref").0
        ));
        assert_eq!(
            typer
                .store()
                .types
                .get(typer.typed_ast().get(holder_read_typed).ty),
            typer.store().types.get(definitions.int)
        );

        for (owner, name) in [
            (holder, "get"),
            (mutable_holder, "get"),
            (mutable_holder, "replace"),
        ] {
            let (method, _, rhs) = method_tree_and_rhs(ast, index, source, typer, owner, name);
            typer.complete_symbol(method).unwrap();
            let context = typer.expression_context_for(method).unwrap();
            typer
                .type_expression(rhs, context)
                .unwrap_or_else(|error| panic!("{name} body should type from source: {error:?}"));
        }

        let (mutable_read, _, mutable_read_rhs) =
            method_tree_and_rhs(ast, index, source, typer, mutable_holder, "read");
        typer.complete_symbol(mutable_read).unwrap();
        let context = typer.expression_context_for(mutable_read).unwrap();
        assert!(matches!(
            typer.type_expression(mutable_read_rhs, context),
            Err(dotty_typer::TyperError::UnstableSelectionPrefix {
                source: error_source,
                ..
            }) if error_source == source
        ));
    });
}

#[test]
fn real_typed_primitive_field_initializers_lower_in_constructor_source_order() {
    with_source(
        "class Counter(start: Int, flag: Boolean) { private val seed: Int = 42; private var current: Int = start; private val doubled: Int = start + 1; private val active: Boolean = !flag; private val copied: Int = current; def get(): Int = current; def increment(): Unit = current = current + 1; def set(value: Int): Unit = current = value; def enabled(): Boolean = active }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Counter");
            typer.complete_symbol(class).unwrap();
            let TreeKind::TypeDef(definition) = &ast
                .iter()
                .find_map(|(tree, node)| {
                    (index.symbol_at(source, tree) == Some(class)).then_some(node)
                })
                .unwrap()
                .kind
            else {
                panic!("Counter source declaration should be a TypeDef");
            };
            let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                panic!("Counter declaration should have a template");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
                panic!("primary constructor should be a DefDef");
            };
            let constructor_parameters = constructor_def
                .value_param_clauses
                .iter()
                .flatten()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>();
            typer.complete_symbol(constructor).unwrap();
            let constructor_plan = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                constructor_parameters.clone(),
                ast,
                index,
            )
            .unwrap();

            let mut source_fields = Vec::new();
            for tree in &template.body {
                let TreeKind::ValDef(field_def) = &ast.get(*tree).kind else {
                    continue;
                };
                let Some(field) = index.symbol_at(source, *tree) else {
                    continue;
                };
                let rhs = field_def
                    .rhs
                    .expect("supported fields have explicit initializers");
                source_fields.push((field, rhs));
            }
            assert_eq!(source_fields.len(), 5);

            let mut field_plans = Vec::new();
            let mut field_symbols = Vec::new();
            for (field, rhs) in source_fields {
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                assert_eq!(typer.source_typed_index().get(source, rhs), None);
                let typed_rhs = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                assert_eq!(
                    typer.source_typed_index().get(source, rhs),
                    Some(typed_rhs),
                    "the exact field RHS tree is retained once it is typed"
                );
                field_plans.push(JvmFieldPlan {
                    field,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(typed_rhs),
                });
                field_symbols.push(field);
            }

            let mut method_plans = Vec::new();
            for name in ["get", "increment", "set", "enabled"] {
                let (method, method_tree, rhs) = template
                    .body
                    .iter()
                    .find_map(|tree| {
                        let TreeKind::DefDef(definition) = &ast.get(*tree).kind else {
                            return None;
                        };
                        (typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text())
                            == name)
                            .then_some((index.symbol_at(source, *tree)?, *tree, definition.rhs?))
                    })
                    .unwrap_or_else(|| panic!("missing method {name}"));
                typer.complete_symbol(method).unwrap();
                let TreeKind::DefDef(definition) = &ast.get(method_tree).kind else {
                    unreachable!()
                };
                let parameters = definition
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect();
                let context = typer.expression_context_for(method).unwrap();
                let body = typer.type_expression(rhs, context).unwrap();
                method_plans.push(JvmMethodPlan {
                    method,
                    parameters,
                    locals: Vec::new(),
                    body: Some(body),
                });
            }

            let class_plan = JvmClassPlan {
                class,
                methods: method_plans,
                fields: field_plans,
                initialization: JvmClassInitializationPlan::PrimaryConstructor(constructor_plan),
            };
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![class_plan],
            };
            let metadata = unit.metadata(definitions).unwrap();
            let class_metadata = &metadata[0];
            let constructor_metadata = class_metadata.constructor().unwrap();
            let initializer_plan = unit
                .lower_constructor_field_initializers(class, definitions)
                .unwrap();
            assert_eq!(initializer_plan.class(), class);
            assert_eq!(initializer_plan.constructor().symbol(), constructor);
            assert_eq!(
                initializer_plan
                    .fields()
                    .iter()
                    .map(|field| field.field())
                    .collect::<Vec<_>>(),
                field_symbols,
                "lowering preserves source declaration order"
            );

            let [start_parameter, flag_parameter] = constructor_metadata.parameters() else {
                panic!("expected two constructor parameters");
            };
            assert_eq!(start_parameter.symbol(), constructor_parameters[0]);
            assert_eq!(start_parameter.slot(), 1);
            assert_eq!(start_parameter.ty(), dotty_jvm::JvmType::Int);
            assert_eq!(flag_parameter.symbol(), constructor_parameters[1]);
            assert_eq!(flag_parameter.slot(), 2);
            assert_eq!(flag_parameter.ty(), dotty_jvm::JvmType::Boolean);

            let seed = initializer_plan.fields()[0].instructions();
            assert!(matches!(
                seed.first(),
                Some(JvmConstructorInitializerInstruction::LoadThis)
            ));
            assert!(matches!(
                seed.get(1),
                Some(JvmConstructorInitializerInstruction::Constant(42))
            ));
            assert!(
                matches!(seed.last(), Some(JvmConstructorInitializerInstruction::PutField { field, .. }) if *field == field_symbols[0])
            );

            let current = initializer_plan.fields()[1].instructions();
            assert!(
                matches!(current.get(1), Some(JvmConstructorInitializerInstruction::LoadParameter { symbol, slot: 1, value_type: dotty_jvm::JvmType::Int }) if *symbol == constructor_parameters[0])
            );
            assert!(
                matches!(current.last(), Some(JvmConstructorInitializerInstruction::PutField { field, .. }) if *field == field_symbols[1])
            );

            let doubled = initializer_plan.fields()[2].instructions();
            assert!(doubled.iter().any(|instruction| matches!(
                instruction,
                JvmConstructorInitializerInstruction::IntAdd
            )));
            assert_eq!(doubled.iter().filter(|instruction| matches!(instruction, JvmConstructorInitializerInstruction::LoadParameter { symbol, .. } if *symbol == constructor_parameters[0])).count(), 1);

            let active = initializer_plan.fields()[3].instructions();
            assert!(active.iter().any(|instruction| matches!(
                instruction,
                JvmConstructorInitializerInstruction::BooleanNot
            )));
            assert!(active.iter().any(|instruction| matches!(instruction, JvmConstructorInitializerInstruction::LoadParameter { symbol, slot: 2, value_type: dotty_jvm::JvmType::Boolean } if *symbol == constructor_parameters[1])));

            let copied = initializer_plan.fields()[4].instructions();
            assert!(copied.iter().any(|instruction| matches!(instruction, JvmConstructorInitializerInstruction::GetField { field, .. } if *field == field_symbols[1])));

            let generated = emit_class(&unit, class, definitions).unwrap();
            let mut reader = Reader::new(generated.bytes());
            let class_file = dotty_classfile::class_file::ClassFile::decode(&mut reader).unwrap();
            class_file.validate_references().unwrap();
            assert_eq!(class_file.fields.len(), 5);
            assert_eq!(class_file.methods.len(), 5);
            assert_eq!(
                class_file
                    .constant_pool
                    .utf8(class_file.methods[0].name_index)
                    .unwrap(),
                "<init>"
            );
            assert_eq!(
                class_file
                    .constant_pool
                    .utf8(class_file.methods[0].descriptor_index)
                    .unwrap(),
                "(IZ)V"
            );
            assert_eq!(
                class_file
                    .fields
                    .iter()
                    .map(|field| class_file.constant_pool.utf8(field.name_index).unwrap())
                    .collect::<Vec<_>>(),
                ["seed", "current", "doubled", "active", "copied"]
            );
            assert!(class_file.fields[0].access_flags.is_private());
            assert!(class_file.fields[0].access_flags.is_final());
            assert!(class_file.fields[1].access_flags.is_private());
            assert!(!class_file.fields[1].access_flags.is_final());
            let method_identities = class_file.methods[1..]
                .iter()
                .map(|method| {
                    (
                        class_file.constant_pool.utf8(method.name_index).unwrap(),
                        class_file
                            .constant_pool
                            .utf8(method.descriptor_index)
                            .unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                method_identities,
                [
                    ("get", "()I"),
                    ("increment", "()V"),
                    ("set", "(I)V"),
                    ("enabled", "()Z")
                ]
            );

            if require_jdk21_or_skip() {
                let class_bytes = generated.into_bytes();
                let nonce = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let output = std::env::temp_dir().join(format!(
                    "dotty-jvm-constructor-fields-{}-{nonce}",
                    std::process::id()
                ));
                fs::create_dir_all(&output).unwrap();
                fs::write(output.join("Counter.class"), class_bytes).unwrap();
                let harness = output.join("ConstructorHarness.java");
                fs::write(
                    &harness,
                    "public final class ConstructorHarness { private static java.lang.reflect.Field field(String name) throws Exception { java.lang.reflect.Field field = Counter.class.getDeclaredField(name); field.setAccessible(true); return field; } public static void main(String[] args) throws Exception { Counter counter = new Counter(9, true); if (field(\"seed\").getInt(counter) != 42) throw new AssertionError(\"seed\"); if (field(\"current\").getInt(counter) != 9) throw new AssertionError(\"current\"); if (field(\"doubled\").getInt(counter) != 10) throw new AssertionError(\"doubled\"); if (field(\"active\").getBoolean(counter)) throw new AssertionError(\"active\"); if (field(\"copied\").getInt(counter) != 9) throw new AssertionError(\"copied\"); if (counter.get() != 9 || counter.enabled()) throw new AssertionError(\"getter\"); counter.increment(); if (counter.get() != 10) throw new AssertionError(\"increment\"); counter.set(17); if (counter.get() != 17) throw new AssertionError(\"setter\"); } }\n",
                )
                .unwrap();
                let javac = Command::new("javac")
                    .args(["-classpath", output.to_str().unwrap(), "-d"])
                    .arg(output.to_str().unwrap())
                    .arg(&harness)
                    .output()
                    .unwrap();
                assert!(
                    javac.status.success(),
                    "javac rejected constructor harness\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&javac.stdout),
                    String::from_utf8_lossy(&javac.stderr)
                );
                let java = Command::new("java")
                    .args([
                        "-Xverify:all",
                        "-classpath",
                        output.to_str().unwrap(),
                        "ConstructorHarness",
                    ])
                    .output()
                    .unwrap();
                assert!(
                    java.status.success(),
                    "java -Xverify:all rejected constructor initializer code\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&java.stdout),
                    String::from_utf8_lossy(&java.stderr)
                );
                fs::remove_dir_all(output).unwrap();
            }
        },
    );
}

#[test]
fn reference_fields_read_write_and_call_through_the_current_instance() {
    let source_text = "class Counter():\n  def get(): Int = 7\nclass Other()\nclass Holder(left: Counter, enabled: Boolean, right: Other):\n  private val delegate: Counter = left\n  private var current: Counter = left\n  private val active: Boolean = enabled\n  private val peer: Other = right\n  def value(): Counter = delegate\n  def valueThis(): Counter = this.delegate\n  def read(): Int = delegate.get()\n  def readThis(): Int = this.delegate.get()\n  def replace(next: Counter): Unit = { current = next }\n  def replaceThis(next: Counter): Unit = { this.current = next }\n  def valueAfter(): Counter = current\n  def branch(flag: Boolean): Counter = if flag then delegate else current\n";
    with_source(source_text, |ast, source, index, definitions, typer| {
        let mut classes = Vec::new();
        let mut class_symbols = Vec::new();
        let mut field_symbols = Vec::new();
        for name in ["Counter", "Other", "Holder"] {
            let class = symbol_named(ast, index, source, typer, name);
            class_symbols.push(class);
            typer.complete_symbol(class).unwrap();
            let class_tree = ast
                .iter()
                .find_map(|(tree, _)| {
                    (index.symbol_at(source, tree) == Some(class)).then_some(tree)
                })
                .unwrap();
            let TreeKind::TypeDef(definition) = &ast.get(class_tree).kind else {
                panic!("class source node is a TypeDef")
            };
            let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                panic!("class body is a template")
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let TreeKind::DefDef(constructor_definition) = &ast.get(template.constructor).kind
            else {
                panic!("primary constructor source node is a DefDef")
            };
            let parameters = constructor_definition
                .value_param_clauses
                .iter()
                .flatten()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>();
            typer.complete_symbol(constructor).unwrap();
            let proof = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                parameters,
                ast,
                index,
            )
            .unwrap();
            let mut fields = Vec::new();
            for tree in &template.body {
                let TreeKind::ValDef(definition) = &ast.get(*tree).kind else {
                    continue;
                };
                let field = index.symbol_at(source, *tree).unwrap();
                let rhs = definition.rhs.unwrap();
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field signature is complete")
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let typed_rhs = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                fields.push(JvmFieldPlan {
                    field,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(typed_rhs),
                });
                field_symbols.push(field);
            }
            classes.push(JvmClassPlan {
                class,
                methods: Vec::new(),
                fields,
                initialization: JvmClassInitializationPlan::PrimaryConstructor(proof),
            });
        }

        for name in [
            "get",
            "value",
            "valueThis",
            "read",
            "readThis",
            "replace",
            "replaceThis",
            "valueAfter",
            "branch",
        ] {
            let (method, body, parameters) = ast
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
                    let expected_owner = if name == "get" {
                        class_symbols[0]
                    } else {
                        class_symbols[2]
                    };
                    if typer.store().symbols.get(method).owner != Some(expected_owner) {
                        return None;
                    }
                    let parameters = definition
                        .value_param_clauses
                        .iter()
                        .flatten()
                        .map(|parameter| index.symbol_at(source, *parameter))
                        .collect::<Option<Vec<_>>>()?;
                    Some((method, definition.rhs?, parameters))
                })
                .unwrap_or_else(|| panic!("missing method {name}"));
            typer.complete_symbol(method).unwrap();
            let context = typer.expression_context_for(method).unwrap();
            let typed_body = typer.type_expression(body, context).unwrap();
            let owner = if name == "get" {
                class_symbols[0]
            } else {
                class_symbols[2]
            };
            let owner_index = class_symbols
                .iter()
                .position(|class| *class == owner)
                .unwrap();
            classes[owner_index].methods.push(JvmMethodPlan {
                method,
                parameters,
                locals: Vec::new(),
                body: Some(typed_body),
            });
        }

        let unit = JvmCompilationUnit {
            store: typer.store(),
            typed_ast: typer.typed_ast(),
            classes,
        };
        let metadata = unit.metadata(definitions).unwrap();
        let holder_index = metadata
            .iter()
            .position(|class| class.identity().symbol() == class_symbols[2])
            .unwrap();
        let holder_metadata = &metadata[holder_index];
        let constructor_parameters = holder_metadata.constructor().unwrap().parameters();
        assert_eq!(constructor_parameters.len(), 3);
        assert_eq!(constructor_parameters[0].slot(), 1);
        assert_eq!(
            constructor_parameters[0].ty(),
            dotty_jvm::JvmType::Reference(class_symbols[0])
        );
        assert_eq!(constructor_parameters[1].slot(), 2);
        assert_eq!(constructor_parameters[1].ty(), dotty_jvm::JvmType::Boolean);
        assert_eq!(constructor_parameters[2].slot(), 3);
        assert_eq!(
            constructor_parameters[2].ty(),
            dotty_jvm::JvmType::Reference(class_symbols[1])
        );

        let initializer = unit
            .lower_constructor_field_initializers(class_symbols[2], definitions)
            .unwrap();
        let holder_methods = &unit.classes[2].methods;
        for name in ["value", "valueThis", "valueAfter", "branch"] {
            let method = holder_methods
                .iter()
                .find(|method| {
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(method.method).name.text())
                        == name
                })
                .unwrap();
            let code = unit
                .lower_method_body(class_symbols[2], method.method, definitions)
                .unwrap();
            let debug = format!("{code:?}");
            assert!(debug.contains("ALoad"), "{name} loads this: {debug}");
            assert!(
                debug.contains("operation: Get"),
                "{name} reads field: {debug}"
            );
            assert!(
                debug.contains("AReturn"),
                "{name} returns reference: {debug}"
            );
        }
        let read_method = holder_methods
            .iter()
            .find(|method| {
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(method.method).name.text())
                    == "read"
            })
            .unwrap();
        let read_code = unit
            .lower_method_body(class_symbols[2], read_method.method, definitions)
            .unwrap();
        let read_debug = format!("{read_code:?}");
        assert!(read_debug.contains("operation: Get"), "read must getfield");
        assert!(
            read_debug.contains("operation: Virtual"),
            "read must invokevirtual: {read_debug}"
        );
        let read_this_method = holder_methods
            .iter()
            .find(|method| {
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(method.method).name.text())
                    == "readThis"
            })
            .unwrap();
        let read_this = unit
            .lower_method_body(class_symbols[2], read_this_method.method, definitions)
            .unwrap();
        let read_this_debug = format!("{read_this:?}");
        assert!(read_this_debug.contains("operation: Get"));
        assert!(read_this_debug.contains("operation: Virtual"));
        let replace_method = holder_methods
            .iter()
            .find(|method| {
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(method.method).name.text())
                    == "replace"
            })
            .unwrap();
        let replace_code = unit
            .lower_method_body(class_symbols[2], replace_method.method, definitions)
            .unwrap();
        let replace_debug = format!("{replace_code:?}");
        assert!(replace_debug.contains("AStore"), "replace stores reference");
        assert!(replace_debug.contains("ALoad"), "replace reloads reference");
        assert!(
            replace_debug.contains("operation: Put"),
            "replace must putfield"
        );
        let replace_this_method = holder_methods
            .iter()
            .find(|method| {
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(method.method).name.text())
                    == "replaceThis"
            })
            .unwrap();
        let replace_this = unit
            .lower_method_body(class_symbols[2], replace_this_method.method, definitions)
            .unwrap();
        assert!(format!("{replace_this:?}").contains("operation: Put"));
        assert_eq!(
            initializer
                .fields()
                .iter()
                .map(|field| field.field())
                .collect::<Vec<_>>(),
            field_symbols
        );
        for (field_index, (parameter_index, expected_type)) in [
            (0, dotty_jvm::JvmType::Reference(class_symbols[0])),
            (0, dotty_jvm::JvmType::Reference(class_symbols[0])),
            (1, dotty_jvm::JvmType::Boolean),
            (2, dotty_jvm::JvmType::Reference(class_symbols[1])),
        ]
        .into_iter()
        .enumerate()
        {
            let instructions = initializer.fields()[field_index].instructions();
            assert!(matches!(
                instructions.first(),
                Some(JvmConstructorInitializerInstruction::LoadThis)
            ));
            assert!(matches!(
                instructions.get(1),
                Some(JvmConstructorInitializerInstruction::LoadParameter { symbol, slot, value_type })
                    if *symbol == constructor_parameters[parameter_index].symbol()
                        && *slot == constructor_parameters[parameter_index].slot()
                        && *value_type == expected_type
            ));
            assert!(
                matches!(instructions.last(), Some(JvmConstructorInitializerInstruction::PutField { field, .. }) if *field == field_symbols[field_index])
            );
        }

        let generated = unit
            .classes
            .iter()
            .map(|plan| emit_class(&unit, plan.class, definitions).unwrap())
            .collect::<Vec<_>>();
        let holder_bytes = generated
            .iter()
            .find(|class| class.internal_name() == "Holder")
            .unwrap()
            .bytes();
        let mut reader = Reader::new(holder_bytes);
        let class_file = dotty_classfile::class_file::ClassFile::decode(&mut reader).unwrap();
        class_file.validate_references().unwrap();
        let constructor = &class_file.methods[0];
        assert_eq!(
            class_file
                .constant_pool
                .utf8(constructor.descriptor_index)
                .unwrap(),
            "(LCounter;ZLOther;)V"
        );
        assert_eq!(class_file.fields.len(), 4);
        assert_eq!(
            class_file
                .fields
                .iter()
                .map(|field| class_file
                    .constant_pool
                    .utf8(field.descriptor_index)
                    .unwrap())
                .collect::<Vec<_>>(),
            ["LCounter;", "LCounter;", "Z", "LOther;"]
        );
        assert!(
            class_file.fields[0]
                .access_flags
                .contains(ACC_PRIVATE | ACC_FINAL)
        );
        assert!(class_file.fields[1].access_flags.contains(ACC_PRIVATE));
        assert!(!class_file.fields[1].access_flags.contains(ACC_FINAL));
        let code = constructor
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                dotty_classfile::attribute::Attribute::Code(code) => Some(code.code),
                _ => None,
            })
            .unwrap();
        assert_eq!(code.len(), 25);
        for (offset, opcode) in [
            (0, 0x2a),  // aload_0
            (1, 0xb7),  // invokespecial Object.<init>
            (4, 0x2a),  // aload_0
            (5, 0x2b),  // aload_1
            (6, 0xb5),  // putfield delegate
            (9, 0x2a),  // aload_0
            (10, 0x2b), // aload_1
            (11, 0xb5), // putfield current
            (14, 0x2a), // aload_0
            (15, 0x1c), // iload_2
            (16, 0xb5), // putfield active
            (19, 0x2a), // aload_0
            (20, 0x2d), // aload_3
            (21, 0xb5), // putfield peer
            (24, 0xb1), // return
        ] {
            assert_eq!(
                code[offset], opcode,
                "wrong opcode at bytecode offset {offset}"
            );
        }

        if require_jdk21_or_skip() {
            let output = std::env::temp_dir().join(format!(
                "dotty-jvm-reference-constructor-initializer-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&output).unwrap();
            for generated in &generated {
                fs::write(
                    output.join(format!("{}.class", generated.internal_name())),
                    generated.bytes(),
                )
                .unwrap();
            }
            let harness = output.join("ReferenceConstructorHarness.java");
            fs::write(
                &harness,
                "public final class ReferenceConstructorHarness { private static java.lang.reflect.Field field(String name) throws Exception { java.lang.reflect.Field field = Holder.class.getDeclaredField(name); field.setAccessible(true); return field; } public static void main(String[] args) throws Exception { Counter counter = new Counter(); Counter next = new Counter(); Other other = new Other(); Holder holder = new Holder(counter, true, other); if (field(\"delegate\").get(holder) != counter) throw new AssertionError(\"reference parameter\"); if (field(\"current\").get(holder) != counter) throw new AssertionError(\"mutable reference parameter\"); if (!field(\"active\").getBoolean(holder)) throw new AssertionError(\"boolean parameter\"); if (field(\"peer\").get(holder) != other) throw new AssertionError(\"second reference parameter\"); if (holder.value() != counter || holder.valueThis() != counter || holder.valueAfter() != counter) throw new AssertionError(\"getfield identity\"); if (holder.read() != 7 || holder.readThis() != 7) throw new AssertionError(\"field call receiver\"); if (holder.branch(true) != counter || holder.branch(false) != counter) throw new AssertionError(\"reference branch join\"); holder.replace(next); if (holder.valueAfter() != next) throw new AssertionError(\"putfield identity\"); holder.replaceThis(counter); if (holder.valueAfter() != counter) throw new AssertionError(\"explicit this putfield identity\"); } }\n",
            )
            .unwrap();
            let javac = Command::new("javac")
                .args(["-classpath", output.to_str().unwrap(), "-d"])
                .arg(output.to_str().unwrap())
                .arg(&harness)
                .output()
                .unwrap();
            assert!(
                javac.status.success(),
                "javac rejected reference constructor initializer harness\\nstdout:\\n{}\\nstderr:\\n{}",
                String::from_utf8_lossy(&javac.stdout),
                String::from_utf8_lossy(&javac.stderr)
            );
            let java = Command::new("java")
                .args([
                    "-Xverify:all",
                    "-classpath",
                    output.to_str().unwrap(),
                    "ReferenceConstructorHarness",
                ])
                .output()
                .unwrap();
            assert!(
                java.status.success(),
                "java -Xverify:all rejected reference constructor initializer code\\nstdout:\\n{}\\nstderr:\\n{}",
                String::from_utf8_lossy(&java.stdout),
                String::from_utf8_lossy(&java.stderr)
            );
            fs::remove_dir_all(output).unwrap();
        }
    });
}

#[test]
fn constructor_field_initializer_rejects_qualified_parameter_references() {
    with_source(
        "class Qualified(start: Int) { private val value: Int = start }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Qualified");
            typer.complete_symbol(class).unwrap();
            let class_tree = ast
                .iter()
                .find_map(|(tree, _node)| {
                    (index.symbol_at(source, tree) == Some(class)).then_some(tree)
                })
                .unwrap();
            let TreeKind::TypeDef(definition) = &ast.get(class_tree).kind else {
                panic!("Qualified source declaration should be a TypeDef");
            };
            let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                panic!("Qualified declaration should have a template");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let TreeKind::DefDef(constructor_def) = &ast.get(template.constructor).kind else {
                panic!("primary constructor should be a DefDef");
            };
            let parameters = constructor_def.value_param_clauses[0]
                .iter()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>();
            typer.complete_symbol(constructor).unwrap();
            let constructor_plan = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                parameters.clone(),
                ast,
                index,
            )
            .unwrap();
            let (field, rhs) = template
                .body
                .iter()
                .find_map(|tree| {
                    let TreeKind::ValDef(definition) = &ast.get(*tree).kind else {
                        return None;
                    };
                    Some((index.symbol_at(source, *tree)?, definition.rhs?))
                })
                .unwrap();
            typer.complete_symbol(field).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("field type should be complete");
            };
            let context = typer.field_initializer_context_for(field).unwrap();
            let typed_rhs = typer
                .type_expression_expected(rhs, context, field_type)
                .unwrap();
            let typed_rhs_tree = typer.typed_ast().get(typed_rhs).clone();
            assert!(matches!(typed_rhs_tree.kind, TreeKind::Ident(_)));

            for missing_qualifier in [false, true] {
                let mut typed_ast = AstArena::<Typed>::new();
                for (_, tree) in typer.typed_ast().iter() {
                    typed_ast.alloc(tree.clone());
                }

                let qualifier = if missing_qualifier {
                    let checkpoint = typed_ast.checkpoint();
                    typed_ast.alloc(typed_rhs_tree.clone());
                    let missing = typed_ast.alloc(typed_rhs_tree.clone());
                    typed_ast.rollback_to(checkpoint);
                    missing
                } else {
                    typed_ast.alloc(Tree {
                        kind: TreeKind::Apply(Apply {
                            function: typed_rhs,
                            args: Vec::new(),
                            kind: ApplyKind::Regular,
                        }),
                        position: None,
                        ty: typed_rhs_tree.ty,
                    })
                };
                let initializer = typed_ast.alloc(Tree {
                    kind: TreeKind::Select(Select {
                        qualifier,
                        name: typer.store().symbols.get(parameters[0]).name,
                        backquoted: false,
                    }),
                    position: None,
                    ty: typed_rhs_tree.ty,
                });
                if missing_qualifier {
                    assert!(typed_ast.try_get(qualifier).is_none());
                }

                let unit = JvmCompilationUnit {
                    store: typer.store(),
                    typed_ast: &typed_ast,
                    classes: vec![JvmClassPlan {
                        class,
                        methods: Vec::new(),
                        fields: vec![JvmFieldPlan {
                            field,
                            source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                            initializer: Some(initializer),
                        }],
                        initialization: JvmClassInitializationPlan::PrimaryConstructor(
                            constructor_plan.clone(),
                        ),
                    }],
                };
                assert!(matches!(
                    unit.lower_constructor_field_initializers(class, definitions),
                    Err(dotty_jvm::JvmCodegenError::UnsupportedConstructorFieldInitializer {
                        field: found_field,
                        tree,
                        node: "qualified constructor parameter reference",
                    }) if found_field == field && tree == initializer
                ));
            }
        },
    );
}

#[test]
fn constructor_field_initializer_lowering_rejects_forward_field_reads() {
    with_source(
        "class Forward() { private val first: Int = second; private val second: Int = 2 }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Forward");
            typer.complete_symbol(class).unwrap();
            let class_tree = ast
                .iter()
                .find_map(|(tree, node)| {
                    (index.symbol_at(source, tree) == Some(class)).then_some((tree, node))
                })
                .unwrap()
                .0;
            let TreeKind::TypeDef(definition) = &ast.get(class_tree).kind else {
                panic!("Forward source declaration should be a TypeDef");
            };
            let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                panic!("Forward declaration should have a template");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let constructor_plan = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                Vec::new(),
                ast,
                index,
            )
            .unwrap();

            let mut fields = Vec::new();
            for tree in &template.body {
                let TreeKind::ValDef(field_def) = &ast.get(*tree).kind else {
                    continue;
                };
                let Some(field) = index.symbol_at(source, *tree) else {
                    continue;
                };
                let rhs = field_def.rhs.unwrap();
                typer.complete_symbol(field).unwrap();
                let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                    panic!("field type should be complete");
                };
                let context = typer.field_initializer_context_for(field).unwrap();
                let typed_rhs = typer
                    .type_expression_expected(rhs, context, field_type)
                    .unwrap();
                assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));
                fields.push(JvmFieldPlan {
                    field,
                    source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(typed_rhs),
                });
            }
            let first = fields[0].field;
            let second = fields[1].field;
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields,
                    initialization: JvmClassInitializationPlan::PrimaryConstructor(
                        constructor_plan,
                    ),
                }],
            };
            assert_eq!(
                unit.lower_constructor_field_initializers(class, definitions),
                Err(
                    dotty_jvm::JvmCodegenError::FieldInitializerReadsUninitializedField {
                        field: first,
                        tree: unit.classes[0].fields[0].initializer.unwrap(),
                        referenced_field: second,
                    }
                )
            );
        },
    );
}

#[test]
fn constructor_field_initializer_lowering_rejects_self_reads() {
    with_source(
        "class SelfRead() { private val value: Int = value }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "SelfRead");
            typer.complete_symbol(class).unwrap();
            let class_tree = ast
                .iter()
                .find_map(|(tree, node)| {
                    (index.symbol_at(source, tree) == Some(class)).then_some((tree, node))
                })
                .unwrap()
                .0;
            let TreeKind::TypeDef(definition) = &ast.get(class_tree).kind else {
                panic!("SelfRead source declaration should be a TypeDef");
            };
            let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                panic!("SelfRead declaration should have a template");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let constructor_plan = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                Vec::new(),
                ast,
                index,
            )
            .unwrap();
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "value");
            typer.complete_symbol(field).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("field type should be complete");
            };
            let context = typer.field_initializer_context_for(field).unwrap();
            let typed_rhs = typer
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
                        initializer: Some(typed_rhs),
                    }],
                    initialization: JvmClassInitializationPlan::PrimaryConstructor(
                        constructor_plan,
                    ),
                }],
            };
            assert!(matches!(
                unit.lower_constructor_field_initializers(class, definitions),
                Err(dotty_jvm::JvmCodegenError::FieldInitializerReadsUninitializedField {
                    field: found_field,
                    referenced_field,
                    ..
                }) if found_field == field && referenced_field == field
            ));
        },
    );
}

#[test]
fn constructor_field_initializer_lowering_rejects_user_method_calls() {
    with_source(
        "class Calling() { private def value(): Int = 7; private val result: Int = value() }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Calling");
            typer.complete_symbol(class).unwrap();
            let class_tree = ast
                .iter()
                .find_map(|(tree, node)| {
                    (index.symbol_at(source, tree) == Some(class)).then_some((tree, node))
                })
                .unwrap()
                .0;
            let TreeKind::TypeDef(definition) = &ast.get(class_tree).kind else {
                panic!("Calling source declaration should be a TypeDef");
            };
            let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                panic!("Calling declaration should have a template");
            };
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let constructor_plan = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                Vec::new(),
                ast,
                index,
            )
            .unwrap();
            let (field, rhs) = template
                .body
                .iter()
                .find_map(|tree| {
                    let TreeKind::ValDef(definition) = &ast.get(*tree).kind else {
                        return None;
                    };
                    Some((index.symbol_at(source, *tree)?, definition.rhs?))
                })
                .unwrap();
            typer.complete_symbol(field).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("field type should be complete");
            };
            let context = typer.field_initializer_context_for(field).unwrap();
            let typed_rhs = typer
                .type_expression_expected(rhs, context, field_type)
                .unwrap();
            assert_eq!(typer.source_typed_index().get(source, rhs), Some(typed_rhs));

            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields: vec![JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(typed_rhs),
                    }],
                    initialization: JvmClassInitializationPlan::PrimaryConstructor(
                        constructor_plan,
                    ),
                }],
            };
            assert!(matches!(
                unit.lower_constructor_field_initializers(class, definitions),
                Err(dotty_jvm::JvmCodegenError::UnsupportedConstructorFieldInitializer {
                    field: found_field,
                    tree,
                    node: "non-approved constructor application",
                }) if found_field == field && tree == typed_rhs
            ));
        },
    );
}

#[test]
fn primary_constructor_parameter_and_private_var_remain_separate_identities() {
    with_source(
        "class Counter(start: Long) { private var current: Long = start; def get(): Long = current }",
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
            assert_eq!(signature.params[0].ty, definitions.long);
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
                typer.store().types.get(definitions.long)
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

            let constructor_plan = JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                vec![constructor_parameter],
                ast,
                index,
            )
            .unwrap();
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods: vec![JvmMethodPlan {
                        method,
                        parameters: Vec::new(),
                        locals: Vec::new(),
                        body: Some(typed_method_rhs),
                    }],
                    fields: vec![JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(typed_rhs),
                    }],
                    initialization: JvmClassInitializationPlan::PrimaryConstructor(
                        constructor_plan,
                    ),
                }],
            };
            let metadata = unit.metadata(definitions).unwrap();
            assert_eq!(metadata[0].fields()[0].descriptor(), "J");
            assert_eq!(metadata[0].constructor().unwrap().parameters()[0].slot(), 1);
            assert_eq!(
                metadata[0].constructor().unwrap().parameters()[0].width(),
                2
            );
            let initializer = unit
                .lower_constructor_field_initializers(class, definitions)
                .unwrap();
            assert!(matches!(
                initializer.fields()[0].instructions(),
                [
                    JvmConstructorInitializerInstruction::LoadThis,
                    JvmConstructorInitializerInstruction::LoadParameter {
                        symbol,
                        slot: 1,
                        value_type: dotty_jvm::JvmType::Long,
                    },
                    JvmConstructorInitializerInstruction::PutField {
                        field: actual_field,
                        descriptor,
                        ..
                    },
                ] if *symbol == constructor_parameter
                    && *actual_field == field
                    && descriptor == "J"
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
            "class Fields { private val seed: Int = 42; private var enabled: Boolean = true; private val large: Long = 9000000000L }",
            |ast, source, index, definitions, typer| {
                let class = symbol_named(ast, index, source, typer, "Fields");
                typer.complete_symbol(class).unwrap();

                let mut fields = Vec::new();
                for name in ["seed", "enabled", "large"] {
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
                assert_eq!(projected.len(), 3);
                assert_eq!(projected[0].name(), "seed");
                assert_eq!(projected[0].descriptor(), "I");
                assert!(projected[0].access_flags().is_private());
                assert!(projected[0].access_flags().is_final());
                assert_eq!(projected[1].name(), "enabled");
                assert_eq!(projected[1].descriptor(), "Z");
                assert!(projected[1].access_flags().is_private());
                assert!(!projected[1].access_flags().is_final());
                assert_eq!(projected[2].name(), "large");
                assert_eq!(projected[2].descriptor(), "J");
                assert!(projected[2].access_flags().is_private());
                assert!(projected[2].access_flags().is_final());

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
        "class Counter(start: Long) { private var current: Long = start }",
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
            assert_eq!(field_metadata.descriptor(), "J");
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
fn real_typed_mutable_field_assignments_lower_to_putfield_in_evaluation_order() {
    with_source(
        "class State { private var current: Int = 0; private var active: Boolean = false; private val fixed: Int = 1; def set(current: Int): Unit = { this.current = current }; def setUnqualified(value: Int): Unit = { current = value }; def setActive(value: Boolean): Unit = { this.active = value }; def increment(): Unit = { current = current + 1 }; def stop(): Unit = { current = { return } }; def localShadow(): Unit = { var current = 1; current = 2 }; def branch(flag: Boolean): Unit = if (flag) { current = 3 } else (); def loop(flag: Boolean): Unit = while (flag) { current = 4 }; def external(other: State): Unit = { other.current = 5 } }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "State");
            typer.complete_symbol(class).unwrap();

            let mut fields = Vec::new();
            for name in ["current", "active", "fixed"] {
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

            let method_names = [
                "set",
                "setUnqualified",
                "setActive",
                "increment",
                "stop",
                "localShadow",
                "branch",
                "loop",
            ];
            let mut methods = Vec::new();
            let mut typed_bodies = Vec::new();
            for name in method_names {
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
                        if typer.store().symbols.get(method).owner != Some(class) {
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
                let locals = if name == "localShadow" {
                    let TreeKind::Block(block) = &ast.get(rhs).kind else {
                        panic!("localShadow should have a block body");
                    };
                    let declaration = block
                        .stats
                        .iter()
                        .copied()
                        .find(|tree| matches!(ast.get(*tree).kind, TreeKind::ValDef(_)))
                        .unwrap();
                    let binding = typer
                        .typed_local_binding_at(source, declaration)
                        .expect("local shadow should have a canonical typed binding");
                    vec![dotty_jvm::JvmLocalBinding {
                        declaration: binding.declaration(),
                        symbol: binding.symbol(),
                        mutable: typer
                            .store()
                            .symbols
                            .get(binding.symbol())
                            .flags
                            .contains(SymbolFlags::MUTABLE),
                    }]
                } else {
                    Vec::new()
                };
                methods.push(JvmMethodPlan {
                    method,
                    parameters,
                    locals,
                    body: Some(body),
                });
                typed_bodies.push((name, method, body));
            }

            let (external_method, external_rhs) = ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    (typer
                        .store()
                        .names
                        .resolve(definition.name.as_name().text())
                        == "external")
                        .then_some((index.symbol_at(source, tree)?, definition.rhs?))
                })
                .expect("missing external receiver method");
            typer.complete_symbol(external_method).unwrap();
            let external_context = typer.expression_context_for(external_method).unwrap();
            let external_body = typer
                .type_expression(external_rhs, external_context)
                .unwrap();

            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![JvmClassPlan {
                    class,
                    methods,
                    fields: fields.clone(),
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            let typed_body = |name: &str| {
                typed_bodies
                    .iter()
                    .find(|(candidate, _, _)| *candidate == name)
                    .copied()
                    .unwrap()
            };
            let encode_body = |name: &str, descriptor: &str| {
                let (_, method, _) = typed_body(name);
                let code = unit.lower_method_body(class, method, definitions).unwrap();
                let mut builder = ClassFileBuilder::new(
                    "State",
                    ClassFileVersion {
                        major: 65,
                        minor: 0,
                    },
                )
                .unwrap();
                builder
                    .add_field(
                        "current",
                        "I",
                        dotty_classfile::access_flags::FieldAccessFlags(
                            dotty_classfile::access_flags::ACC_PRIVATE,
                        ),
                    )
                    .unwrap();
                builder
                    .add_field(
                        "active",
                        "Z",
                        dotty_classfile::access_flags::FieldAccessFlags(
                            dotty_classfile::access_flags::ACC_PRIVATE,
                        ),
                    )
                    .unwrap();
                builder
                    .add_field(
                        "fixed",
                        "I",
                        dotty_classfile::access_flags::FieldAccessFlags(
                            dotty_classfile::access_flags::ACC_PRIVATE
                                | dotty_classfile::access_flags::ACC_FINAL,
                        ),
                    )
                    .unwrap();
                builder
                    .add_prebuilt_code_method(name, descriptor, MethodAccessFlags(ACC_PUBLIC), code)
                    .unwrap();
                let mut constructor = dotty_classfile::bytecode::CodeBuilder::new();
                constructor
                    .aload(0)
                    .and_then(|code| code.invokespecial("java/lang/Object", "<init>", "()V"))
                    .and_then(|code| code.return_void())
                    .unwrap();
                builder
                    .add_prebuilt_code_method(
                        "<init>",
                        "()V",
                        MethodAccessFlags(ACC_PUBLIC),
                        constructor,
                    )
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
                let field_ref = code
                    .iter()
                    .position(|opcode| *opcode == 0xb5)
                    .map(|putfield| {
                        let index = u16::from_be_bytes([code[putfield + 1], code[putfield + 2]]);
                        let dotty_classfile::constant_pool::ConstantPoolEntry::Fieldref {
                            class_index,
                            name_and_type_index,
                        } = class_file
                            .constant_pool
                            .get(dotty_classfile::constant_pool::ConstantPoolIndex(index))
                            .unwrap()
                        else {
                            panic!("putfield must reference Fieldref");
                        };
                        let (field, descriptor) = class_file
                            .constant_pool
                            .name_and_type(*name_and_type_index)
                            .unwrap();
                        (
                            class_file
                                .constant_pool
                                .class_name(*class_index)
                                .unwrap()
                                .to_owned(),
                            field.to_owned(),
                            descriptor.to_owned(),
                        )
                    });
                (code, field_ref, bytes)
            };

            for (name, descriptor, field, field_descriptor) in [
                ("set", "(I)V", "current", "I"),
                ("setUnqualified", "(I)V", "current", "I"),
                ("setActive", "(Z)V", "active", "Z"),
            ] {
                let (code, field_ref, _) = encode_body(name, descriptor);
                assert_eq!(
                    code[0], 0x2a,
                    "aload_0 evaluates the receiver first for {name}"
                );
                let rhs_load = code
                    .iter()
                    .position(|opcode| *opcode == 0x1b)
                    .expect("parameter is loaded for the RHS");
                let receiver_reload = code
                    .iter()
                    .rposition(|opcode| *opcode == 0x2c)
                    .expect("saved receiver is reloaded after the RHS");
                assert!(
                    rhs_load < receiver_reload,
                    "RHS follows receiver for {name}"
                );
                let putfield = code.iter().position(|opcode| *opcode == 0xb5).unwrap();
                assert_eq!(code.iter().filter(|opcode| **opcode == 0xb5).count(), 1);
                assert!(!code.contains(&0xb4), "LHS is not evaluated as a read");
                assert_eq!(code[putfield + 3], 0xb1, "Unit return for {name}");
                assert_eq!(
                    field_ref,
                    Some((
                        "State".to_owned(),
                        field.to_owned(),
                        field_descriptor.to_owned()
                    ))
                );
            }

            let (increment_code, increment_field, _) = encode_body("increment", "()V");
            assert_eq!(increment_code[0], 0x2a, "field receiver is loaded first");
            assert!(
                increment_code.contains(&0xb4),
                "RHS reads the current field"
            );
            assert!(
                increment_code.contains(&0x60),
                "RHS adds one before storing"
            );
            assert!(
                increment_code.contains(&0xb5),
                "composite assignment stores once"
            );
            assert_eq!(
                increment_code
                    .iter()
                    .filter(|opcode| **opcode == 0xb5)
                    .count(),
                1
            );
            assert_eq!(
                increment_field,
                Some(("State".to_owned(), "current".to_owned(), "I".to_owned()))
            );

            let (local_code, local_field, _) = encode_body("localShadow", "()V");
            assert!(!local_code.contains(&0xb5), "shadowing local uses istore");
            assert!(
                !local_code.contains(&0xb4),
                "shadowing local does not read field"
            );
            assert!(local_code.contains(&0x3c), "local var stores to its slot");
            assert!(local_field.is_none());

            let (stop_code, stop_field, _) = encode_body("stop", "()V");
            assert!(stop_code.contains(&0xb1), "local return is encoded");
            assert!(!stop_code.contains(&0xb5), "diverging RHS does not store");
            assert!(stop_field.is_none());

            if require_jdk21_or_skip() {
                for (name, descriptor, harness_body) in [
                    (
                        "set",
                        "(I)V",
                        "state.set(41); java.lang.reflect.Field field = State.class.getDeclaredField(\"current\"); field.setAccessible(true); if (field.getInt(state) != 41) throw new AssertionError(\"Int putfield\");",
                    ),
                    (
                        "setActive",
                        "(Z)V",
                        "state.setActive(true); java.lang.reflect.Field field = State.class.getDeclaredField(\"active\"); field.setAccessible(true); if (!field.getBoolean(state)) throw new AssertionError(\"Boolean putfield\");",
                    ),
                    (
                        "increment",
                        "()V",
                        "state.increment(); java.lang.reflect.Field field = State.class.getDeclaredField(\"current\"); field.setAccessible(true); if (field.getInt(state) != 1) throw new AssertionError(\"compound putfield\");",
                    ),
                ] {
                    let (_, _, class_bytes) = encode_body(name, descriptor);
                    let nonce = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos();
                    let output = std::env::temp_dir().join(format!(
                        "dotty-jvm-field-write-{}-{nonce}",
                        std::process::id()
                    ));
                    fs::create_dir_all(&output).unwrap();
                    fs::write(output.join("State.class"), class_bytes).unwrap();
                    let harness = output.join("WriteHarness.java");
                    fs::write(
                        &harness,
                        format!(
                            "public final class WriteHarness {{ public static void main(String[] args) throws Exception {{ State state = new State(); {harness_body} }} }}\n"
                        ),
                    )
                    .unwrap();
                    let javac = Command::new("javac")
                        .args(["-classpath", output.to_str().unwrap(), "-d"])
                        .arg(output.to_str().unwrap())
                        .arg(&harness)
                        .output()
                        .unwrap();
                    assert!(
                        javac.status.success(),
                        "javac rejected {name} harness\nstdout:\n{}\nstderr:\n{}",
                        String::from_utf8_lossy(&javac.stdout),
                        String::from_utf8_lossy(&javac.stderr)
                    );
                    let java = Command::new("java")
                        .args([
                            "-Xverify:all",
                            "-classpath",
                            output.to_str().unwrap(),
                            "WriteHarness",
                        ])
                        .output()
                        .unwrap();
                    assert!(
                        java.status.success(),
                        "java -Xverify:all rejected {name}\nstdout:\n{}\nstderr:\n{}",
                        String::from_utf8_lossy(&java.stdout),
                        String::from_utf8_lossy(&java.stderr)
                    );
                    fs::remove_dir_all(output).unwrap();
                }
            }

            let (_, branch_method, _) = typed_body("branch");
            let branch_code = unit
                .lower_method_body(class, branch_method, definitions)
                .expect("current-instance field writes are supported in branches");
            assert!(format!("{branch_code:?}").contains("operation: Put"));

            let (_, loop_method, _) = typed_body("loop");
            let loop_code = unit
                .lower_method_body(class, loop_method, definitions)
                .expect("current-instance field writes are supported in loops");
            assert!(format!("{loop_code:?}").contains("operation: Put"));

            let (_, setter_method, _) = typed_body("setUnqualified");
            let mut no_fields = unit.classes[0].clone();
            no_fields.fields.clear();
            let unplanned = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![no_fields],
            };
            assert!(matches!(
                unplanned.lower_method_body(class, setter_method, definitions),
                Err(dotty_jvm::JvmCodegenError::FieldNotInJvmPlan { field, .. })
                    if field == fields[0].field
            ));

            let mut external_method_plan = unit.classes[0].methods[1].clone();
            external_method_plan.body = Some(external_body);
            let mut external_class_plan = unit.classes[0].clone();
            external_class_plan.methods = vec![external_method_plan];
            let external = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![external_class_plan],
            };
            assert!(matches!(
                external.lower_method_body(class, setter_method, definitions),
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmFieldReceiver { field, .. })
                    if field == fields[0].field
            ));
        },
    );
}

#[test]
fn field_reads_follow_canonical_bindings_in_control_flow_and_fail_closed_elsewhere() {
    with_source(
        "class Current { private val current: Int = 5; private val hidden: Int = 6; private val flag: Boolean = true; def get(): Int = current; def shadow(current: Int): Int = current; def branch(flag: Boolean): Int = if (flag) current else 0; def shortCircuit(): Boolean = this.flag && true; def compared(): Boolean = this.current < 7; def select(other: Current): Int = other.current; def readHidden(): Int = hidden }\nclass Other { private val foreign: Int = 1; def read(): Int = this.foreign }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "Current");
            let other_class = symbol_named(ast, index, source, typer, "Other");
            typer.complete_symbol(class).unwrap();
            typer.complete_symbol(other_class).unwrap();

            let mut fields = Vec::new();
            for name in ["current", "hidden", "flag", "foreign"] {
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
                (class, "shortCircuit"),
                (class, "compared"),
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
            let flag_field = fields
                .iter()
                .find(|field| {
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(field.field).name.text())
                        == "flag"
                })
                .unwrap();

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
            let branch_code = branch
                .lower_method_body(class, *branch_method, definitions)
                .expect("planned current-instance field reads are supported in branches");
            assert!(format!("{branch_code:?}").contains("operation: Get"));

            let (_, short_circuit_method, short_circuit_body, _) = method("shortCircuit");
            let short_circuit = plan_for(
                *short_circuit_method,
                *short_circuit_body,
                Vec::new(),
                vec![*current_field, *flag_field],
            );
            let short_circuit_code = short_circuit
                .lower_method_body(class, *short_circuit_method, definitions)
                .expect("planned Boolean field reads are supported in short-circuit flow");
            assert!(format!("{short_circuit_code:?}").contains("operation: Get"));

            let (_, compared_method, compared_body, _) = method("compared");
            let compared = plan_for(
                *compared_method,
                *compared_body,
                Vec::new(),
                current_plan.clone(),
            );
            let compared_result = compared.lower_method_body(class, *compared_method, definitions);
            let compared_code = compared_result
                .expect("planned field reads are supported in relational conditions");
            assert!(format!("{compared_code:?}").contains("operation: Get"));

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
fn field_metadata_projects_private_reference_fields_from_the_full_unit() {
    with_source(
        "class Counter {}\nclass Holder(initial: Counter) { private val original: Counter = initial; private var current: Counter = initial }",
        |ast, source, index, definitions, typer| {
            let counter = symbol_named(ast, index, source, typer, "Counter");
            let holder = symbol_named(ast, index, source, typer, "Holder");
            typer.complete_symbol(counter).unwrap();
            typer.complete_symbol(holder).unwrap();

            let mut fields = Vec::new();
            for name in ["original", "current"] {
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
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![
                    JvmClassPlan {
                        class: counter,
                        methods: Vec::new(),
                        fields: Vec::new(),
                        initialization: JvmClassInitializationPlan::Unsupported,
                    },
                    JvmClassPlan {
                        class: holder,
                        methods: Vec::new(),
                        fields,
                        initialization: JvmClassInitializationPlan::Unsupported,
                    },
                ],
            };
            let metadata = unit.metadata(definitions).unwrap();
            let fields = metadata[1].fields();
            let counter_descriptor = format!("L{};", metadata[0].identity().internal_name());
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].name(), "original");
            assert_eq!(fields[0].descriptor(), counter_descriptor);
            assert_eq!(
                fields[0].value_type(),
                dotty_jvm::JvmType::Reference(counter)
            );
            assert!(fields[0].access_flags().is_private());
            assert!(fields[0].access_flags().is_final());
            assert_eq!(fields[1].name(), "current");
            assert_eq!(fields[1].descriptor(), counter_descriptor);
            assert_eq!(
                fields[1].value_type(),
                dotty_jvm::JvmType::Reference(counter)
            );
            assert!(fields[1].access_flags().is_private());
            assert!(!fields[1].access_flags().is_final());

            let standalone =
                unit.classes[1].metadata_with_typed_ast(unit.store, unit.typed_ast, definitions);
            assert!(matches!(
                standalone,
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmType {
                    kind: dotty_jvm::UnsupportedJvmTypeKind::ReferenceClass,
                    ..
                })
            ));
        },
    );
}

#[test]
fn field_metadata_rejects_a_reference_identifier_without_a_parameter_term_ref() {
    with_source(
        "class Counter {}\nclass Holder(initial: Counter) { private val item: Counter = initial }",
        |ast, source, index, definitions, typer| {
            let counter = symbol_named(ast, index, source, typer, "Counter");
            let holder = symbol_named(ast, index, source, typer, "Holder");
            typer.complete_symbol(counter).unwrap();
            typer.complete_symbol(holder).unwrap();
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "item");
            typer.complete_symbol(field).unwrap();
            let SymbolInfo::Complete(field_type) = typer.store().symbols.get(field).info else {
                panic!("field type should be complete");
            };
            let context = typer.field_initializer_context_for(field).unwrap();
            let initializer = typer
                .type_expression_expected(rhs, context, field_type)
                .unwrap();

            let mut malformed_ast = dotty_core::ast::TypedAst::new();
            let mut malformed_initializer_tree = typer.typed_ast().get(initializer).clone();
            malformed_initializer_tree.ty = field_type;
            let malformed_initializer = malformed_ast.alloc(malformed_initializer_tree);
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: &malformed_ast,
                classes: vec![
                    JvmClassPlan {
                        class: counter,
                        methods: Vec::new(),
                        fields: Vec::new(),
                        initialization: JvmClassInitializationPlan::Unsupported,
                    },
                    JvmClassPlan {
                        class: holder,
                        methods: Vec::new(),
                        fields: vec![JvmFieldPlan {
                            field,
                            source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                            initializer: Some(malformed_initializer),
                        }],
                        initialization: JvmClassInitializationPlan::Unsupported,
                    },
                ],
            };
            assert_eq!(
                unit.metadata(definitions),
                Err(
                    dotty_jvm::JvmCodegenError::UnsupportedJvmReferenceFieldInitializer {
                        field,
                        initializer: malformed_initializer,
                        symbol: None,
                    }
                )
            );
        },
    );
}

#[test]
fn field_metadata_rejects_reference_fields_outside_the_unit_and_self_initialization() {
    with_source(
        "class Outside {}\nclass Holder(initial: Outside) { private val item: Outside = initial }",
        |ast, source, index, definitions, typer| {
            let outside = symbol_named(ast, index, source, typer, "Outside");
            let holder = symbol_named(ast, index, source, typer, "Holder");
            typer.complete_symbol(outside).unwrap();
            typer.complete_symbol(holder).unwrap();
            let (field, _, rhs) = field_tree_and_rhs(ast, index, source, typer, "item");
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
                    class: holder,
                    methods: Vec::new(),
                    fields: vec![JvmFieldPlan {
                        field,
                        source_visibility: dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                        initializer: Some(initializer),
                    }],
                    initialization: JvmClassInitializationPlan::Unsupported,
                }],
            };
            assert_eq!(
                unit.metadata(definitions),
                Err(dotty_jvm::JvmCodegenError::ReferenceClassNotInJvmPlan { class: outside })
            );
        },
    );

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
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmReferenceType {
                    kind: dotty_jvm::UnsupportedJvmReferenceTypeKind::NotCanonicalTypeRef,
                    ..
                })
            ));
        },
    );
}

#[test]
fn field_metadata_rejects_a_reference_initializer_with_another_class_identity() {
    with_source(
        "class Counter {}\nclass Other {}\nclass Holder(counter: Counter, other: Other) { private val first: Counter = counter; private val second: Other = other }",
        |ast, source, index, definitions, typer| {
            let counter = symbol_named(ast, index, source, typer, "Counter");
            let other = symbol_named(ast, index, source, typer, "Other");
            let holder = symbol_named(ast, index, source, typer, "Holder");
            for class in [counter, other, holder] {
                typer.complete_symbol(class).unwrap();
            }
            let (counter_field, _, _) = field_tree_and_rhs(ast, index, source, typer, "first");
            let (other_field, _, other_rhs) =
                field_tree_and_rhs(ast, index, source, typer, "second");
            for field in [counter_field, other_field] {
                typer.complete_symbol(field).unwrap();
            }
            let SymbolInfo::Complete(other_type) = typer.store().symbols.get(other_field).info
            else {
                panic!("Other field type should be complete");
            };
            let other_context = typer.field_initializer_context_for(other_field).unwrap();
            let other_initializer = typer
                .type_expression_expected(other_rhs, other_context, other_type)
                .unwrap();
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: [counter, other, holder]
                    .into_iter()
                    .map(|class| JvmClassPlan {
                        class,
                        methods: Vec::new(),
                        fields: if class == holder {
                            vec![JvmFieldPlan {
                                field: counter_field,
                                source_visibility:
                                    dotty_jvm::JvmFieldSourceVisibility::PlainPrivate,
                                initializer: Some(other_initializer),
                            }]
                        } else {
                            Vec::new()
                        },
                        initialization: JvmClassInitializationPlan::Unsupported,
                    })
                    .collect(),
            };
            assert_eq!(
                unit.metadata(definitions),
                Err(dotty_jvm::JvmCodegenError::FieldInitializerTypeMismatch {
                    field: counter_field,
                    initializer: other_initializer,
                    expected: dotty_jvm::JvmType::Reference(counter),
                    found: Some(dotty_jvm::JvmType::Reference(other)),
                })
            );
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

#[test]
fn primary_constructor_metadata_projects_exact_unit_references_and_mixed_slots() {
    with_source(
        "package sample\nclass Counter {}\nclass Holder(initial: Counter) {}\nclass Duo(value: Int, counter: Counter, enabled: Boolean) {}\nclass Client { def make(counter: Counter): Unit = { new Holder(counter); () } }",
        |ast, source, index, definitions, typer| {
            let counter = symbol_named(ast, index, source, typer, "Counter");
            let holder = symbol_named(ast, index, source, typer, "Holder");
            let duo = symbol_named(ast, index, source, typer, "Duo");
            let client = symbol_named(ast, index, source, typer, "Client");
            for class in [counter, holder, duo, client] {
                typer.complete_symbol(class).unwrap();
            }

            let source_constructor = |class| {
                let (constructor, parameter_trees) = ast
                    .iter()
                    .find_map(|(tree, node)| {
                        if index.symbol_at(source, tree) != Some(class) {
                            return None;
                        }
                        let TreeKind::TypeDef(definition) = &node.kind else {
                            return None;
                        };
                        let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
                            return None;
                        };
                        let TreeKind::DefDef(constructor) = &ast.get(template.constructor).kind
                        else {
                            return None;
                        };
                        Some((
                            index.symbol_at(source, template.constructor)?,
                            constructor
                                .value_param_clauses
                                .iter()
                                .flatten()
                                .copied()
                                .collect::<Vec<_>>(),
                        ))
                    })
                    .expect("class has source primary constructor");
                let parameters = parameter_trees
                    .iter()
                    .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                    .collect::<Vec<_>>();
                let proof = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                    class,
                    constructor,
                    parameters,
                    ast,
                    index,
                )
                .unwrap();
                (constructor, parameter_trees, proof)
            };
            let (holder_constructor, holder_parameter_trees, holder_proof) =
                source_constructor(holder);
            let (duo_constructor, duo_parameter_trees, duo_proof) = source_constructor(duo);
            typer.complete_symbol(holder_constructor).unwrap();
            typer.complete_symbol(duo_constructor).unwrap();

            let (client_method, client_parameters, client_rhs) = ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::DefDef(definition) = &node.kind else {
                        return None;
                    };
                    let method = index.symbol_at(source, tree)?;
                    if typer.store().symbols.get(method).owner != Some(client) {
                        return None;
                    }
                    let parameter_trees = definition
                        .value_param_clauses
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<Vec<_>>();
                    let parameters = parameter_trees
                        .iter()
                        .map(|tree| index.symbol_at(source, *tree).unwrap())
                        .collect::<Vec<_>>();
                    Some((method, parameters, definition.rhs?))
                })
                .expect("Client.make source method");
            typer.complete_symbol(client_method).unwrap();
            let context = typer.expression_context_for(client_method).unwrap();
            let client_body = typer.type_expression(client_rhs, context).unwrap();
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![
                    JvmClassPlan {
                        class: counter,
                        methods: Vec::new(),
                        fields: Vec::new(),
                        initialization: JvmClassInitializationPlan::Unsupported,
                    },
                    JvmClassPlan {
                        class: holder,
                        methods: Vec::new(),
                        fields: Vec::new(),
                        initialization: JvmClassInitializationPlan::PrimaryConstructor(
                            holder_proof,
                        ),
                    },
                    JvmClassPlan {
                        class: duo,
                        methods: Vec::new(),
                        fields: Vec::new(),
                        initialization: JvmClassInitializationPlan::PrimaryConstructor(duo_proof),
                    },
                    JvmClassPlan {
                        class: client,
                        methods: vec![JvmMethodPlan {
                            method: client_method,
                            parameters: client_parameters.clone(),
                            locals: Vec::new(),
                            body: Some(client_body),
                        }],
                        fields: Vec::new(),
                        initialization: JvmClassInitializationPlan::Unsupported,
                    },
                ],
            };

            let metadata = unit.metadata(definitions).unwrap();
            let holder_metadata = metadata
                .iter()
                .find(|metadata| metadata.identity().symbol() == holder)
                .unwrap()
                .constructor()
                .unwrap();
            let holder_parameter = &holder_metadata.parameters()[0];
            assert_eq!(holder_metadata.descriptor().as_str(), "(Lsample/Counter;)V");
            assert_eq!(holder_metadata.owner_internal_name(), "sample/Holder");
            assert_eq!(holder_metadata.symbol(), holder_constructor);
            assert_eq!(
                holder_parameter.symbol(),
                index
                    .derived_symbol_at(holder_constructor, source, holder_parameter_trees[0])
                    .unwrap()
            );
            assert_eq!(
                holder_parameter.ty(),
                dotty_jvm::JvmType::Reference(counter)
            );
            assert_eq!(holder_parameter.slot(), 1);
            assert_eq!(holder_parameter.width(), 1);

            let duo_metadata = metadata
                .iter()
                .find(|metadata| metadata.identity().symbol() == duo)
                .unwrap()
                .constructor()
                .unwrap();
            assert_eq!(duo_metadata.descriptor().as_str(), "(ILsample/Counter;Z)V");
            assert_eq!(duo_metadata.symbol(), duo_constructor);
            assert_eq!(
                duo_metadata
                    .parameters()
                    .iter()
                    .map(|parameter| (parameter.ty(), parameter.slot(), parameter.width()))
                    .collect::<Vec<_>>(),
                [
                    (dotty_jvm::JvmType::Int, 1, 1),
                    (dotty_jvm::JvmType::Reference(counter), 2, 1),
                    (dotty_jvm::JvmType::Boolean, 3, 1),
                ]
            );
            for (parameter, tree) in duo_metadata.parameters().iter().zip(duo_parameter_trees) {
                assert_eq!(
                    parameter.symbol(),
                    index
                        .derived_symbol_at(duo_constructor, source, tree)
                        .unwrap()
                );
                assert_eq!(
                    typer.store().symbols.get(parameter.symbol()).owner,
                    Some(duo_constructor)
                );
            }

            assert!(matches!(
                unit.classes[1].metadata_with_typed_ast(unit.store, unit.typed_ast, definitions),
                Err(dotty_jvm::JvmCodegenError::UnsupportedJvmType {
                    kind: dotty_jvm::UnsupportedJvmTypeKind::ReferenceClass,
                    ..
                })
            ));

            let unplanned_unit = JvmCompilationUnit {
                store: unit.store,
                typed_ast: unit.typed_ast,
                classes: unit.classes.iter().skip(1).cloned().collect(),
            };
            assert_eq!(
                unplanned_unit.metadata(definitions),
                Err(dotty_jvm::JvmCodegenError::ReferenceClassNotInJvmPlan { class: counter })
            );

            let JvmClassInitializationPlan::PrimaryConstructor(holder_proof) =
                &unit.classes[1].initialization
            else {
                unreachable!()
            };
            let wrong_owner_plan = JvmClassPlan {
                class: counter,
                methods: Vec::new(),
                fields: Vec::new(),
                initialization: JvmClassInitializationPlan::PrimaryConstructor(
                    holder_proof.clone(),
                ),
            };
            assert_eq!(
                wrong_owner_plan.metadata(unit.store, definitions),
                Err(dotty_jvm::JvmCodegenError::InvalidPrimaryConstructor {
                    constructor: holder_constructor,
                    issue: dotty_jvm::JvmConstructorIssue::SourceIdentity,
                })
            );

            let application = unit
                .typed_ast
                .iter()
                .find_map(|(tree, node)| {
                    let TreeKind::Apply(application) = &node.kind else {
                        return None;
                    };
                    let function = unit.typed_ast.try_get(application.function)?;
                    let TreeKind::Select(select) = &function.kind else {
                        return None;
                    };
                    let qualifier = unit.typed_ast.try_get(select.qualifier)?;
                    (unit.store.names.resolve(select.name.text()) == "<init>"
                        && matches!(qualifier.kind, TreeKind::New(_)))
                    .then_some(tree)
                })
                .expect("typed new Holder(counter) application");
            let target = unit
                .resolve_constructor_call(client, client_method, application, definitions)
                .unwrap();
            assert_eq!(target.descriptor().as_str(), "(Lsample/Counter;)V");
            assert_eq!(
                target.argument_types(),
                &[dotty_jvm::JvmType::Reference(counter)]
            );
            assert_eq!(target.owner_class(), holder);
            unit.lower_method_body(client, client_method, definitions)
                .expect("planned reference parameter is a supported constructor argument");
        },
    );
}

#[test]
fn ordinary_primary_constructor_metadata_uses_derived_parameters_and_void_result() {
    for (text, descriptor) in [
        ("class C()", "()V"),
        ("class C", "()V"),
        ("class C(x: Int)", "(I)V"),
        ("class C(flag: Boolean, x: Int)", "(ZI)V"),
        ("class C private (x: Int)", "(I)V"),
    ] {
        with_source(text, |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "C");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(template) => Some(template),
                    _ => None,
                })
                .unwrap();
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let TreeKind::DefDef(def) = &ast.get(template.constructor).kind else {
                panic!()
            };
            let parameters = def
                .value_param_clauses
                .iter()
                .flatten()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>();
            let proof = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                parameters.clone(),
                ast,
                index,
            )
            .unwrap_or_else(|e| {
                panic!(
                    "{text}: {e:?}, {def:?}, params: {:?}",
                    def.value_param_clauses
                        .iter()
                        .flatten()
                        .map(|id| ast.get(*id))
                        .collect::<Vec<_>>()
                )
            });
            typer.complete_symbol(class).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let plan = JvmClassPlan {
                class,
                methods: Vec::new(),
                fields: Vec::new(),
                initialization: JvmClassInitializationPlan::PrimaryConstructor(proof),
            };
            let metadata = plan.metadata(typer.store(), definitions).unwrap();
            let unit = JvmCompilationUnit {
                store: typer.store(),
                typed_ast: typer.typed_ast(),
                classes: vec![plan],
            };
            let generated = emit_class(&unit, class, definitions).unwrap();
            let mut reader = Reader::new(generated.bytes());
            let class_file = dotty_classfile::class_file::ClassFile::decode(&mut reader).unwrap();
            assert_eq!(class_file.methods.len(), 1);
            let emitted_constructor = &class_file.methods[0];
            assert_eq!(
                class_file
                    .constant_pool
                    .utf8(emitted_constructor.descriptor_index)
                    .unwrap(),
                descriptor
            );
            assert_eq!(
                class_file
                    .constant_pool
                    .utf8(emitted_constructor.name_index)
                    .unwrap(),
                "<init>"
            );
            let constructor_metadata = metadata.constructor().unwrap();
            assert_eq!(constructor_metadata.descriptor().as_str(), descriptor);
            assert_eq!(constructor_metadata.owner_internal_name(), "C");
            assert_eq!(constructor_metadata.symbol(), constructor);
            assert_eq!(
                constructor_metadata.access_flags().is_private(),
                text.contains("private")
            );
            assert_eq!(
                constructor_metadata.access_flags().is_public(),
                !text.contains("private")
            );
            for (i, (actual, expected)) in constructor_metadata
                .parameters()
                .iter()
                .zip(parameters)
                .enumerate()
            {
                assert_eq!(actual.symbol(), expected);
                assert_eq!(actual.slot(), (i + 1) as u16);
                assert_eq!(typer.store().symbols.get(expected).owner, Some(constructor));
            }
        });
    }
}

#[test]
fn primary_constructor_source_proof_rejects_wrong_identities_and_unsupported_forms() {
    use dotty_jvm::{JvmCodegenError, JvmConstructorIssue, JvmPrimaryConstructorPlan};
    for text in [
        "class C(val x: Int)",
        "class C(var x: Int)",
        "class C(x: Int = 1)",
        "class C(using x: Int)",
        "class C[A](x: Int)",
        "class C(x: Int)(y: Boolean)",
    ] {
        with_source(text, |ast, source, index, _, typer| {
            let class = symbol_named(ast, index, source, typer, "C");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(t) => Some(t),
                    _ => None,
                })
                .unwrap();
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let TreeKind::DefDef(def) = &ast.get(template.constructor).kind else {
                panic!()
            };
            let parameters = def
                .value_param_clauses
                .iter()
                .flatten()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>();
            assert!(
                matches!(
                    JvmPrimaryConstructorPlan::from_source(
                        class,
                        constructor,
                        parameters,
                        ast,
                        index
                    ),
                    Err(JvmCodegenError::InvalidPrimaryConstructor {
                        issue: JvmConstructorIssue::SourceShape,
                        ..
                    })
                ),
                "{text}"
            );
        });
    }
    with_source(
        "class C(a: Int, b: Int) { def this() = this(0, 0) }",
        |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "C");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(t) => Some(t),
                    _ => None,
                })
                .unwrap();
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let TreeKind::DefDef(def) = &ast.get(template.constructor).kind else {
                panic!()
            };
            let parameters = def.value_param_clauses[0]
                .iter()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>();
            for wrong in [
                Vec::new(),
                vec![parameters[0], parameters[0]],
                vec![parameters[1], parameters[0]],
                vec![
                    index
                        .symbol_at(source, def.value_param_clauses[0][0])
                        .unwrap(),
                    parameters[1],
                ],
            ] {
                assert!(matches!(
                    JvmPrimaryConstructorPlan::from_source(class, constructor, wrong, ast, index),
                    Err(JvmCodegenError::InvalidPrimaryConstructor {
                        issue: JvmConstructorIssue::ParameterIdentity,
                        ..
                    })
                ));
            }
            assert!(matches!(
                JvmPrimaryConstructorPlan::from_source(
                    class,
                    class,
                    parameters.clone(),
                    ast,
                    index
                ),
                Err(JvmCodegenError::InvalidPrimaryConstructor {
                    issue: JvmConstructorIssue::SourceIdentity,
                    ..
                })
            ));
            let proof =
                JvmPrimaryConstructorPlan::from_source(class, constructor, parameters, ast, index)
                    .unwrap();
            typer.complete_symbol(class).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let plan = JvmClassPlan {
                class,
                methods: Vec::new(),
                fields: Vec::new(),
                initialization: JvmClassInitializationPlan::PrimaryConstructor(proof),
            };
            assert!(matches!(
                plan.metadata(typer.store(), definitions),
                Err(JvmCodegenError::InvalidPrimaryConstructor {
                    issue: JvmConstructorIssue::SecondaryConstructor,
                    ..
                })
            ));
        },
    );
}

#[test]
fn primary_constructor_signature_rejects_reference_and_special_parameter_types() {
    for text in [
        "class C(x: C)",
        "class C(x: => Int)",
        "class C(xs: Int*)",
        "class C(x: 1)",
    ] {
        with_source(text, |ast, source, index, definitions, typer| {
            let class = symbol_named(ast, index, source, typer, "C");
            let template = ast
                .iter()
                .find_map(|(_, node)| match &node.kind {
                    TreeKind::Template(t) => Some(t),
                    _ => None,
                })
                .unwrap();
            let constructor = index.symbol_at(source, template.constructor).unwrap();
            let TreeKind::DefDef(def) = &ast.get(template.constructor).kind else {
                panic!()
            };
            let parameters = def.value_param_clauses[0]
                .iter()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect();
            let proof = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                class,
                constructor,
                parameters,
                ast,
                index,
            )
            .unwrap();
            typer.complete_symbol(class).unwrap();
            typer.complete_symbol(constructor).unwrap();
            let plan = JvmClassPlan {
                class,
                methods: Vec::new(),
                fields: Vec::new(),
                initialization: JvmClassInitializationPlan::PrimaryConstructor(proof),
            };
            let error = plan.metadata(typer.store(), definitions).unwrap_err();
            match text {
                "class C(x: C)" => assert!(matches!(
                    error,
                    dotty_jvm::JvmCodegenError::UnsupportedJvmType {
                        kind: dotty_jvm::UnsupportedJvmTypeKind::ReferenceClass,
                        ..
                    }
                )),
                "class C(x: => Int)" => assert!(matches!(
                    error,
                    dotty_jvm::JvmCodegenError::UnsupportedJvmType {
                        kind: dotty_jvm::UnsupportedJvmTypeKind::ByName,
                        ..
                    }
                )),
                "class C(xs: Int*)" => assert!(matches!(
                    error,
                    dotty_jvm::JvmCodegenError::UnsupportedMethodParameter {
                        kind: dotty_jvm::UnsupportedMethodParameterKind::Varargs,
                        ..
                    }
                )),
                _ => assert!(matches!(
                    error,
                    dotty_jvm::JvmCodegenError::InvalidPrimaryConstructor {
                        issue: dotty_jvm::JvmConstructorIssue::ParameterSignature,
                        ..
                    }
                )),
            }
        });
    }
}

#[test]
fn primary_constructor_enforces_the_instance_parameter_slot_limit() {
    for count in [254, 255] {
        let params = (0..count)
            .map(|i| format!("p{i}: Int"))
            .collect::<Vec<_>>()
            .join(", ");
        with_source(
            &format!("class C({params})"),
            |ast, source, index, definitions, typer| {
                let class = symbol_named(ast, index, source, typer, "C");
                let template = ast
                    .iter()
                    .find_map(|(_, node)| match &node.kind {
                        TreeKind::Template(t) => Some(t),
                        _ => None,
                    })
                    .unwrap();
                let constructor = index.symbol_at(source, template.constructor).unwrap();
                let TreeKind::DefDef(def) = &ast.get(template.constructor).kind else {
                    panic!()
                };
                let parameters = def.value_param_clauses[0]
                    .iter()
                    .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                    .collect();
                let proof = dotty_jvm::JvmPrimaryConstructorPlan::from_source(
                    class,
                    constructor,
                    parameters,
                    ast,
                    index,
                )
                .unwrap();
                typer.complete_symbol(class).unwrap();
                typer.complete_symbol(constructor).unwrap();
                let plan = JvmClassPlan {
                    class,
                    methods: Vec::new(),
                    fields: Vec::new(),
                    initialization: JvmClassInitializationPlan::PrimaryConstructor(proof),
                };
                let result = plan.metadata(typer.store(), definitions);
                if count == 254 {
                    assert_eq!(
                        result
                            .unwrap()
                            .constructor()
                            .unwrap()
                            .parameters()
                            .last()
                            .unwrap()
                            .slot(),
                        254
                    );
                } else {
                    assert!(matches!(
                        result,
                        Err(dotty_jvm::JvmCodegenError::TooManyParameterSlots { slots: 256, .. })
                    ));
                }
            },
        );
    }
}
