use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use dotty_core::ast::*;
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::tasty::*;
use dotty_tasty_pickler::*;
use dotty_typer::SourceTyper;

#[derive(Clone, Default)]
struct Bindings(
    HashMap<(SourceId, TreeId<Untyped>), TreeId<Typed>>,
    HashMap<(SourceId, SymbolId), Vec<LocalValueBinding>>,
);

impl SourceTypedBindings for Bindings {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.0.get(&(source, tree)).copied()
    }

    fn local_value_bindings(&self, source: SourceId, method: SymbolId) -> Vec<LocalValueBinding> {
        self.1.get(&(source, method)).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Copy)]
enum TestCorruption {
    LiteralMethod(&'static str),
    ShadowedFieldTarget {
        method: &'static str,
        field_reader: &'static str,
        identifier: &'static str,
    },
}

fn with_frontend<R>(
    source_text: &str,
    f: impl FnOnce(
        &AstArena<Untyped>,
        SourceId,
        &SourceSemanticIndex,
        &Definitions,
        &mut SourceTyper<'_>,
    ) -> R,
) -> R {
    let source = SourceId::from_index(1390);
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
        "ClassStatePlan.scala",
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

fn name_of(ast: &AstArena<Untyped>, tree: TreeId<Untyped>, typer: &SourceTyper<'_>) -> String {
    let name = match &ast.get(tree).kind {
        TreeKind::TypeDef(definition) => definition.name.as_name(),
        TreeKind::ValDef(definition) => definition.name.as_name(),
        TreeKind::DefDef(definition) => definition.name.as_name(),
        _ => panic!("expected named declaration"),
    };
    typer.store().names.resolve(name.text()).to_owned()
}

fn symbol_for_tree(
    source: SourceId,
    index: &SourceSemanticIndex,
    tree: TreeId<Untyped>,
) -> SymbolId {
    index
        .symbol_at(source, tree)
        .expect("canonical source symbol")
}

fn class_tree_for(
    ast: &AstArena<Untyped>,
    source: SourceId,
    index: &SourceSemanticIndex,
    class: SymbolId,
) -> TreeId<Untyped> {
    ast.iter()
        .find_map(|(tree, node)| {
            (index.symbol_at(source, tree) == Some(class)
                && matches!(node.kind, TreeKind::TypeDef(_)))
            .then_some(tree)
        })
        .expect("class source tree")
}

fn clone_typed_ast(ast: &AstArena<Typed>) -> AstArena<Typed> {
    let mut cloned = AstArena::new();
    for (_, tree) in ast.iter() {
        cloned.alloc(tree.clone());
    }
    cloned
}

fn raw_tree_tag(tree: &RawTree<'_>) -> u8 {
    match tree {
        RawTree::Leaf(term) => term.tag,
        RawTree::Ast { tag, .. } | RawTree::NatAst { tag, .. } => *tag,
        RawTree::LengthNode(node) => node.tag,
    }
}

fn class_state_plan<'a>(
    ast: &'a AstArena<Untyped>,
    source: SourceId,
    index: &'a SourceSemanticIndex,
    _definitions: &'a Definitions,
    typer: &'a mut SourceTyper<'_>,
    class_name: &str,
    source_name: &str,
) -> (
    SymbolId,
    Vec<ClassMemberPicklePlan>,
    PrimaryConstructorInputPlan,
    Bindings,
) {
    let (class_tree, class) = ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::TypeDef(definition) = &node.kind else {
                return None;
            };
            let symbol = index.symbol_at(source, tree)?;
            (typer
                .store()
                .names
                .resolve(definition.name.as_name().text())
                == class_name)
                .then_some((tree, symbol))
        })
        .expect("class tree and symbol");
    typer.complete_symbol(class).unwrap();
    let TreeKind::TypeDef(class_definition) = &ast.get(class_tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(template) = &ast.get(class_definition.rhs).kind else {
        unreachable!()
    };
    let constructor_tree = template.constructor;
    let constructor = symbol_for_tree(source, index, constructor_tree);
    typer.complete_symbol(constructor).unwrap();
    let TreeKind::DefDef(constructor_def) = &ast.get(constructor_tree).kind else {
        unreachable!()
    };
    let parameter_clauses = constructor_def
        .value_param_clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|&tree| {
                    index
                        .derived_symbol_at(constructor, source, tree)
                        .expect("constructor-owned parameter symbol")
                })
                .collect()
        })
        .collect::<Vec<Vec<SymbolId>>>();
    let constructor = PrimaryConstructorInputPlan {
        symbol: constructor,
        source_tree: constructor_tree,
        parameter_clauses,
    };
    let mut bindings = Bindings::default();
    let mut members = Vec::with_capacity(template.body.len());
    for &tree in &template.body {
        match &ast.get(tree).kind {
            TreeKind::ValDef(definition) => {
                let symbol = symbol_for_tree(source, index, tree);
                typer.complete_symbol(symbol).unwrap();
                let SymbolInfo::Complete(ty) = typer.store().symbols.get(symbol).info else {
                    panic!("field type completes")
                };
                let rhs = definition.rhs.expect("supported field has initializer");
                let context = typer.field_initializer_context_for(symbol).unwrap();
                let typed = typer.type_expression_expected(rhs, context, ty).unwrap();
                bindings.0.insert((source, rhs), typed);
                members.push(ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
                    symbol,
                    source_tree: tree,
                    typed_initializer: typed,
                    declared_type: ty,
                }));
            }
            TreeKind::DefDef(_) => {
                let symbol = symbol_for_tree(source, index, tree);
                typer.complete_symbol(symbol).unwrap();
                members.push(ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree: tree,
                });
            }
            _ => panic!("unsupported member {}", name_of(ast, tree, typer)),
        }
    }
    let _ = source_name;
    (class, members, constructor, bindings)
}

fn with_mutable_frontend<R>(
    source_text: &str,
    class_name: &str,
    source_name: &str,
    f: impl FnOnce(
        &mut SemanticStore,
        &AstArena<Untyped>,
        SourceId,
        &SourceSemanticIndex,
        &Definitions,
        &AstArena<Typed>,
        SymbolId,
        Vec<ClassMemberPicklePlan>,
        PrimaryConstructorInputPlan,
        Bindings,
    ) -> R,
) -> R {
    let source = SourceId::from_index(1390);
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
        source_name,
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
    let (class, members, constructor, bindings, typed_ast) = {
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let (class, members, constructor, bindings) = class_state_plan(
            &parsed.ast,
            source,
            &index,
            &definitions,
            &mut typer,
            class_name,
            source_name,
        );
        let typed_ast = clone_typed_ast(typer.typed_ast());
        (class, members, constructor, bindings, typed_ast)
    };
    f(
        &mut store,
        &parsed.ast,
        source,
        &index,
        &definitions,
        &typed_ast,
        class,
        members,
        constructor,
        bindings,
    )
}

#[test]
fn source_typer_counter_and_private_values_satisfy_the_read_only_plan() {
    let counter = "package example\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n  def increment(): Int =\n    current = current + 1\n    current\n";
    with_frontend(counter, |ast, source, index, definitions, typer| {
        let (class, members, constructor, bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            "Counter",
            "Counter.scala",
        );
        let plan = ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: typer.typed_ast(),
            source_index: index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source,
                name: "Counter.scala".into(),
            },
            class,
            class_tree: ast
                .iter()
                .find_map(|(tree, _)| {
                    (index.symbol_at(source, tree) == Some(class)
                        && matches!(ast.get(tree).kind, TreeKind::TypeDef(_)))
                    .then_some(tree)
                })
                .unwrap(),
            constructor,
            members: &members,
        };
        assert!(matches!(
            plan.validate_with_limits(ValidationLimits {
                max_items: 0,
                ..ValidationLimits::default()
            }),
            Err(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                ..
            })
        ));
        let validated = plan.validate().unwrap();
        assert_eq!(validated.plan().class, class);
        assert!(matches!(members[0], ClassMemberPicklePlan::Field(_)));
    });

    let private_values = "package example\nclass PrivateValues:\n  private val seed: Int = 42\n  private val enabled: Boolean = true\n";
    with_frontend(private_values, |ast, source, index, definitions, typer| {
        let (class, members, constructor, bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            "PrivateValues",
            "PrivateValues.scala",
        );
        let class_tree = ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(class)
                    && matches!(node.kind, TreeKind::TypeDef(_)))
                .then_some(tree)
            })
            .unwrap();
        ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: typer.typed_ast(),
            source_index: index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source,
                name: "PrivateValues.scala".into(),
            },
            class,
            class_tree,
            constructor,
            members: &members,
        }
        .validate()
        .unwrap();
    });

    let box_source = "package example\nclass Box(start: Int)\n";
    with_frontend(box_source, |ast, source, index, definitions, typer| {
        let (class, members, constructor, bindings) =
            class_state_plan(ast, source, index, definitions, typer, "Box", "Box.scala");
        let class_tree = ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(class)
                    && matches!(node.kind, TreeKind::TypeDef(_)))
                .then_some(tree)
            })
            .unwrap();
        assert!(members.is_empty());
        ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: typer.typed_ast(),
            source_index: index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source,
                name: "Box.scala".into(),
            },
            class,
            class_tree,
            constructor,
            members: &members,
        }
        .validate()
        .unwrap();
    })
}

fn private_fields_output() -> (PickledTasty, SymbolId, Vec<SymbolId>, TreeId<Typed>) {
    let source_text = "package example\nclass PrivateFields:\n  private val seed: Int = 42\n  private val enabled: Boolean = true\n  private var counter: Int = 0\n  def constant(): Int = 1\n";
    with_frontend(source_text, |ast, source, index, definitions, typer| {
        let (class, members, constructor, mut bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            "PrivateFields",
            "PrivateFields.scala",
        );
        let ClassMemberPicklePlan::Method {
            symbol: method,
            source_tree: method_tree,
        } = members[3]
        else {
            unreachable!()
        };
        let TreeKind::DefDef(method_definition) = &ast.get(method_tree).kind else {
            unreachable!()
        };
        let method_rhs = method_definition.rhs.unwrap();
        let method_context = typer.expression_context_for(method).unwrap();
        let typed_method_rhs = typer
            .type_expression_expected(method_rhs, method_context, definitions.int)
            .unwrap();
        bindings.0.insert((source, method_rhs), typed_method_rhs);
        let class_tree = class_tree_for(ast, source, index, class);
        let output = ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: typer.typed_ast(),
            source_index: index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source,
                name: "PrivateFields.scala".into(),
            },
            class,
            class_tree,
            constructor,
            members: &members,
        }
        .validate()
        .unwrap()
        .pickle()
        .unwrap();

        let method_source_tree = match members[3] {
            ClassMemberPicklePlan::Method { source_tree, .. } => source_tree,
            _ => unreachable!(),
        };
        let method_body = match &ast.get(method_source_tree).kind {
            TreeKind::DefDef(definition) => definition.rhs.unwrap(),
            _ => unreachable!(),
        };
        let method_body = bindings.typed_at(source, method_body).unwrap();
        (
            output,
            class,
            members
                .iter()
                .map(|member| match member {
                    ClassMemberPicklePlan::Field(field) => field.symbol,
                    ClassMemberPicklePlan::Method { symbol, .. } => *symbol,
                })
                .collect(),
            method_body,
        )
    })
}

fn emit_class_state_source(
    source_text: &str,
    class_name: &str,
    source_name: &str,
    corruption: Option<TestCorruption>,
) -> Result<PickledTasty, PickleError> {
    with_frontend(source_text, |ast, source, index, definitions, typer| {
        let (class, members, constructor, mut bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            class_name,
            source_name,
        );
        for member in &members {
            let ClassMemberPicklePlan::Method {
                symbol,
                source_tree,
            } = member
            else {
                continue;
            };
            let TreeKind::DefDef(definition) = &ast.get(*source_tree).kind else {
                unreachable!()
            };
            let rhs = definition.rhs.expect("test methods have bodies");
            let context = typer.expression_context_for(*symbol).unwrap();
            let SymbolInfo::Complete(signature) = typer.store().symbols.get(*symbol).info else {
                panic!("test method has a complete signature")
            };
            let result_type = match typer.store().types.try_get(signature) {
                Some(Type::Method(signature)) => signature.result,
                _ => signature,
            };
            let typed = typer
                .type_expression_expected(rhs, context, result_type)
                .unwrap();
            bindings.0.insert((source, rhs), typed);
            let locals =
                ast.iter()
                    .filter_map(|(source_tree, tree)| {
                        matches!(tree.kind, TreeKind::ValDef(_))
                            .then(|| {
                                typer
                                    .typed_local_binding_at(source, source_tree)
                                    .map(|binding| (source_tree, binding))
                            })
                            .flatten()
                    })
                    .filter_map(|(source_tree, binding)| {
                        (typer.store().symbols.get(binding.symbol()).owner == Some(*symbol))
                            .then_some(LocalValueBinding {
                                source_tree,
                                symbol: binding.symbol(),
                                typed_declaration: binding.declaration(),
                                scope: binding.scope(),
                            })
                    })
                    .collect();
            bindings.1.insert((source, *symbol), locals);
        }
        for (source_tree, _) in ast.iter() {
            if let Some(typed) = typer.source_typed_index().get(source, source_tree) {
                bindings.0.insert((source, source_tree), typed);
            }
        }

        let mut typed_ast = clone_typed_ast(typer.typed_ast());
        if let Some(corruption) = corruption {
            let (name, field_type) = match corruption {
                TestCorruption::LiteralMethod(name) => (name, None),
                TestCorruption::ShadowedFieldTarget {
                    method,
                    field_reader,
                    identifier: _,
                } => {
                    let reader = members
                        .iter()
                        .find_map(|member| {
                            let ClassMemberPicklePlan::Method {
                                symbol,
                                source_tree,
                            } = member
                            else {
                                return None;
                            };
                            (name_of(ast, *source_tree, typer) == field_reader)
                                .then_some((*symbol, *source_tree))
                        })
                        .expect("field reader method exists");
                    let TreeKind::DefDef(reader_def) = &ast.get(reader.1).kind else {
                        unreachable!()
                    };
                    let reader_rhs = reader_def.rhs.expect("reader has a body");
                    let reader_typed = bindings.typed_at(source, reader_rhs).unwrap();
                    (method, Some(typer.typed_ast().get(reader_typed).ty))
                }
            };
            let method = members.iter().find_map(|member| {
                let ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree,
                } = member
                else {
                    return None;
                };
                (name_of(ast, *source_tree, typer) == name).then_some(*symbol)
            });
            let method = method.expect("corruption target exists");
            let ClassMemberPicklePlan::Method { source_tree, .. } = members
                .iter()
                .find(|member| matches!(member, ClassMemberPicklePlan::Method { symbol, .. } if *symbol == method))
                .copied()
                .unwrap()
            else {
                unreachable!()
            };
            let TreeKind::DefDef(definition) = &ast.get(source_tree).kind else {
                unreachable!()
            };
            let rhs = definition.rhs.unwrap();
            let typed_root = bindings.typed_at(source, rhs).unwrap();
            match corruption {
                TestCorruption::LiteralMethod(_) => {
                    let forged = typed_ast.get_mut(typed_root);
                    let TreeKind::Literal(literal) = &mut forged.kind else {
                        panic!("test corruption expects a literal body")
                    };
                    literal.value = Constant::Int(42);
                }
                TestCorruption::ShadowedFieldTarget { identifier, .. } => {
                    let source_rhs = definition.rhs.unwrap();
                    let local_or_parameter = match &ast.get(source_rhs).kind {
                        TreeKind::Block(block) => {
                            let TreeKind::Ident(ident) = ast.get(block.expr).kind else {
                                panic!("shadowed local body ends in an identifier")
                            };
                            assert_eq!(typer.store().names.resolve(ident.name.text()), identifier);
                            bindings.typed_at(source, block.expr).unwrap()
                        }
                        TreeKind::Ident(ident) => {
                            assert_eq!(typer.store().names.resolve(ident.name.text()), identifier);
                            typed_root
                        }
                        _ => panic!("shadowed method body has an identifier"),
                    };
                    typed_ast.get_mut(local_or_parameter).ty = field_type.unwrap();
                }
            }
        }

        let class_tree = class_tree_for(ast, source, index, class);
        ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: &typed_ast,
            source_index: index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source,
                name: source_name.into(),
            },
            class,
            class_tree,
            constructor,
            members: &members,
        }
        .validate()?
        .pickle()
    })
}

#[test]
fn validated_private_fields_emit_source_ordered_valdefs_and_preserve_methods() {
    let (output, class, members, method_body) = private_fields_output();
    let (other_session, _, _, _) = private_fields_output();
    assert_eq!(output.bytes(), other_session.bytes());
    assert_eq!(output.intended_tasty_path(), "example/PrivateFields.tasty");
    assert_eq!(output.declarations().len(), 6);
    assert_eq!(
        output.body(output.declarations()[5].symbol),
        Some(method_body)
    );
    assert_eq!(
        output.declarations()[2..]
            .iter()
            .map(|declaration| declaration.symbol)
            .collect::<Vec<_>>(),
        members
    );

    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let fields = output.declarations()[2..5]
        .iter()
        .map(|declaration| {
            assert_eq!(declaration.owner, class);
            index
                .get(declaration.address)
                .unwrap()
                .decode_definition_body()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let expected = [
        (
            "seed",
            IDENTTPT_TAG,
            INTCONST_TAG,
            vec![PRIVATE_TAG, LOCAL_TAG],
        ),
        (
            "enabled",
            IDENTTPT_TAG,
            TRUECONST_TAG,
            vec![PRIVATE_TAG, LOCAL_TAG],
        ),
        (
            "counter",
            IDENTTPT_TAG,
            INTCONST_TAG,
            vec![PRIVATE_TAG, LOCAL_TAG, MUTABLE_TAG],
        ),
    ];
    for (field, (name, type_tag, rhs_tag, modifiers)) in fields.iter().zip(expected) {
        let DefinitionBody::ValDef {
            name: name_ref,
            type_tree,
            rhs,
            tail,
        } = field
        else {
            panic!("class field is a VALDEF")
        };
        assert_eq!(file.render_name(*name_ref).unwrap(), name);
        assert_eq!(raw_tree_tag(type_tree), type_tag);
        assert_eq!(raw_tree_tag(rhs.as_ref().unwrap()), rhs_tag);
        assert_eq!(
            tail.iter()
                .map(|modifier| match modifier {
                    DefinitionTail::Modifier(tag) => *tag,
                    _ => panic!("field has only plain modifiers"),
                })
                .collect::<Vec<_>>(),
            modifiers
        );
    }
    let method = output.declarations()[5];
    let method = index
        .get(method.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert_eq!(file.render_name(method.name).unwrap(), "constant");
    assert_eq!(method.clauses, [EMPTYCLAUSE_TAG]);
    assert_eq!(raw_tree_tag(&method.rhs.unwrap()), INTCONST_TAG);
}

#[test]
fn class_state_reuses_canonical_parameter_and_forward_method_validation() {
    let parameterized = "package example\nclass Parameterized:\n  private val seed: Int = 1\n  def identity(value: Int): Int = value\n";
    let output =
        emit_class_state_source(parameterized, "Parameterized", "Parameterized.scala", None)
            .unwrap();
    assert_eq!(output.intended_tasty_path(), "example/Parameterized.tasty");
    assert_eq!(output.declarations().len(), 5);

    let references = "package example\nclass References:\n  private val seed: Int = 1\n  def first: Int = second\n  def second: Int = 2\n";
    let output =
        emit_class_state_source(references, "References", "References.scala", None).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let methods = output.declarations()[3..]
        .iter()
        .map(|declaration| {
            file.ast_address_index()
                .unwrap()
                .get(declaration.address)
                .unwrap()
                .decode_defdef_body()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(methods.len(), 2);
    assert_eq!(raw_tree_tag(methods[0].rhs.as_ref().unwrap()), SELECT_TAG);
}

#[test]
fn class_state_lowers_implicit_and_explicit_private_primitive_field_reads() {
    let source = "package example\nclass PrivateFields:\n  private val seed: Int = 42\n  private val enabled: Boolean = true\n  private var counter: Int = 0\n  def seedValue(): Int = seed\n  def seedViaThis(): Int = this.seed\n  def enabledValue(): Boolean = enabled\n";
    let source = source.replace(
        "  def enabledValue(): Boolean = enabled\n",
        "  def enabledValue(): Boolean = enabled\n  def shadowedSeed(): Int = { val seed: Int = 7; seed }\n  def constant(): Int = 1\n  def enabledBranch(): Int = if enabled then 1 else 0\n  def enabledViaThis(): Boolean = this.enabled\n  def disabled(): Boolean = !enabled\n  def seedBranch(flag: Boolean): Int = if flag then seed else 0\n",
    );
    let output = emit_class_state_source(&source, "PrivateFields", "PrivateFields.scala", None)
        .expect("canonical private field reads and local shadowing should serialize");
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let methods = &output.declarations()[5..8];
    let bodies = methods
        .iter()
        .map(|declaration| {
            index
                .get(declaration.address)
                .unwrap()
                .decode_defdef_body()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3);
    assert!(bodies.iter().all(|body| {
        let rhs = body.rhs.as_ref().unwrap();
        raw_tree_tag(rhs) == SELECT_TAG
    }));
    assert_eq!(file.render_name(bodies[0].name).unwrap(), "seedValue");
    assert_eq!(file.render_name(bodies[1].name).unwrap(), "seedViaThis");
    assert_eq!(file.render_name(bodies[2].name).unwrap(), "enabledValue");
    assert!(output.body(output.declarations()[8].symbol).is_some());

    if std::env::var_os("RUN_SCALA_ORACLE").is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = root.join("target/issue-1392/generated/example/PrivateFields.tasty");
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, output.bytes()).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg(&candidate)
            .arg("--runtime")
            .env("TMPDIR", root.join("target/issue-1392/tmp"))
            .status()
            .expect("start the pinned Scala 3.9 -from-tasty gate");
        assert!(result.success(), "PrivateFields -from-tasty gate failed");
    }
}

#[test]
fn class_state_registers_local_values_and_rejects_forged_method_bodies() {
    let source = "package example\nclass LocalState:\n  private val seed: Int = 1\n  def run(): Int = { val saved: Int = 1; saved + 1 }\n  def sumTo(limit: Int): Int = { var total: Int = 0; var index: Int = 0; while index < limit do { total = total + index; index = index + 1 }; total }\n";
    let output = emit_class_state_source(source, "LocalState", "LocalState.scala", None).unwrap();
    assert_eq!(output.declarations().len(), 9);

    let corrupt = "package example\nclass CorruptBody:\n  private val seed: Int = 1\n  def constant(): Int = 1\n";
    assert!(
        emit_class_state_source(
            corrupt,
            "CorruptBody",
            "CorruptBody.scala",
            Some(TestCorruption::LiteralMethod("constant")),
        )
        .is_err()
    );
}

#[test]
fn class_state_rejects_shadowed_names_retargeted_to_fields() {
    let source = "package example\nclass ShadowedFields:\n  private val seed: Int = 42\n  def seedValue(): Int = seed\n  def take(seed: Int): Int = seed\n  def local(): Int = { val seed: Int = 7; seed }\n";
    for method in ["take", "local"] {
        let result = emit_class_state_source(
            source,
            "ShadowedFields",
            "ShadowedFields.scala",
            Some(TestCorruption::ShadowedFieldTarget {
                method,
                field_reader: "seedValue",
                identifier: "seed",
            }),
        );
        assert!(result.is_err(), "forged field target accepted in {method}");
    }
}

#[test]
fn class_state_artifact_path_keeps_nested_package_owner_components() {
    let source = "package example.nested\nclass NestedFields:\n  private val seed: Int = 1\n  def constant: Int = 1\n";
    let output =
        emit_class_state_source(source, "NestedFields", "NestedFields.scala", None).unwrap();
    assert_eq!(
        output.intended_tasty_path(),
        "example/nested/NestedFields.tasty"
    );
}

#[test]
fn unsupported_visibility_order_and_missing_bindings_are_rejected() {
    let source = "package example\nclass PrivateValues:\n  private val seed: Int = 42\n  private val enabled: Boolean = true\n";
    with_frontend(source, |ast, source, index, definitions, typer| {
        let (class, mut members, constructor, bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            "PrivateValues",
            "PrivateValues.scala",
        );
        let class_tree = ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(class)
                    && matches!(node.kind, TreeKind::TypeDef(_)))
                .then_some(tree)
            })
            .unwrap();
        macro_rules! make_plan {
            ($members:expr, $bindings:expr) => {
                ClassStatePicklePlan {
                    store: typer.store(),
                    definitions,
                    source_ast: ast,
                    typed_ast: typer.typed_ast(),
                    source_index: index,
                    typed_bindings: $bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "PrivateValues.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor: constructor.clone(),
                    members: $members,
                }
            };
        }
        members.swap(0, 1);
        assert!(matches!(
            make_plan!(&members, &bindings).validate(),
            Err(PickleError::SourceOrder { .. })
        ));
        members.swap(0, 1);
        let missing = Bindings::default();
        assert!(matches!(
            make_plan!(&members, &missing).validate(),
            Err(PickleError::FieldInitializerBinding { .. })
        ));

        let mut wrong_type = members.clone();
        let ClassMemberPicklePlan::Field(field) = &mut wrong_type[0] else {
            unreachable!()
        };
        field.declared_type = definitions.boolean;
        assert!(matches!(
            make_plan!(&wrong_type, &bindings).validate(),
            Err(PickleError::FieldType { .. })
        ));

        let mut stale_bindings = bindings.0.clone();
        let ClassMemberPicklePlan::Field(field) = members[0] else {
            unreachable!()
        };
        let TreeKind::ValDef(definition) = &ast.get(field.source_tree).kind else {
            unreachable!()
        };
        let source_rhs = definition.rhs.unwrap();
        let mut foreign_arena = AstArena::<Typed>::new();
        let stale_tree = foreign_arena.alloc(Tree {
            kind: TreeKind::Literal(Literal {
                value: Constant::Int(0),
            }),
            position: ast.get(source_rhs).position,
            ty: definitions.int,
        });
        stale_bindings.insert((source, source_rhs), stale_tree);
        let stale_bindings = Bindings(stale_bindings, HashMap::new());
        let mut stale_members = members.clone();
        let ClassMemberPicklePlan::Field(field) = &mut stale_members[0] else {
            unreachable!()
        };
        field.typed_initializer = stale_tree;
        let empty_typed_arena = AstArena::<Typed>::new();
        let invalid_plan = ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: &empty_typed_arena,
            source_index: index,
            typed_bindings: &stale_bindings,
            source: SourceMetadata {
                id: source,
                name: "PrivateValues.scala".into(),
            },
            class,
            class_tree,
            constructor: constructor.clone(),
            members: &stale_members,
        };
        assert!(matches!(
            invalid_plan.validate(),
            Err(PickleError::InvalidTypedTree { tree }) if tree == stale_tree
        ));
    });

    let private_this = "package example\nclass PrivateThis:\n  private[this] val seed: Int = 42\n";
    with_frontend(private_this, |ast, source, index, definitions, typer| {
        let (class, members, constructor, bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            "PrivateThis",
            "PrivateThis.scala",
        );
        let class_tree = ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(class)
                    && matches!(node.kind, TreeKind::TypeDef(_)))
                .then_some(tree)
            })
            .unwrap();
        assert!(matches!(
            ClassStatePicklePlan {
                store: typer.store(),
                definitions,
                source_ast: ast,
                typed_ast: typer.typed_ast(),
                source_index: index,
                typed_bindings: &bindings,
                source: SourceMetadata {
                    id: source,
                    name: "PrivateThis.scala".into()
                },
                class,
                class_tree,
                constructor,
                members: &members,
            }
            .validate(),
            Err(PickleError::Unsupported {
                feature: UnsupportedFeature::ClassField,
                ..
            })
        ));
    });

    let unsafe_reads = "package example\nclass UnsafeReads:\n  private val first: Int = second\n  private val second: Int = 1\n";
    with_frontend(unsafe_reads, |ast, source, index, definitions, typer| {
        let (class, members, constructor, bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            "UnsafeReads",
            "UnsafeReads.scala",
        );
        let class_tree = ast
            .iter()
            .find_map(|(tree, node)| {
                (index.symbol_at(source, tree) == Some(class)
                    && matches!(node.kind, TreeKind::TypeDef(_)))
                .then_some(tree)
            })
            .unwrap();
        assert!(matches!(
            ClassStatePicklePlan {
                store: typer.store(),
                definitions,
                source_ast: ast,
                typed_ast: typer.typed_ast(),
                source_index: index,
                typed_bindings: &bindings,
                source: SourceMetadata {
                    id: source,
                    name: "UnsafeReads.scala".into()
                },
                class,
                class_tree,
                constructor,
                members: &members,
            }
            .validate(),
            Err(PickleError::Unsupported {
                feature: UnsupportedFeature::UnsafeFieldInitializer,
                ..
            })
        ));
    });
}

#[test]
fn corrupted_initializer_types_and_constructor_records_are_rejected() {
    let source_text = "package example\nclass Counter(start: Int):\n  private val seed: Int = 42\n";
    with_mutable_frontend(
        source_text,
        "Counter",
        "Counter.scala",
        |store,
         ast,
         source,
         index,
         definitions,
         typed_ast,
         class,
         members,
         constructor,
         bindings| {
            let class_tree = class_tree_for(ast, source, index, class);
            let ClassMemberPicklePlan::Field(field) = members[0] else {
                unreachable!()
            };
            let TreeKind::ValDef(definition) = &ast.get(field.source_tree).kind else {
                unreachable!()
            };
            let source_rhs = definition.rhs.unwrap();
            let typed_position = typed_ast.get(field.typed_initializer).position;
            let changed_type = store.types.alloc(Type::Constant(Constant::Int(7)));
            let mut changed_typed_ast = AstArena::<Typed>::new();
            let changed_initializer = changed_typed_ast.alloc(Tree {
                kind: TreeKind::Literal(Literal {
                    value: Constant::Int(7),
                }),
                position: typed_position,
                ty: changed_type,
            });
            let mut changed_members = members.clone();
            let ClassMemberPicklePlan::Field(field) = &mut changed_members[0] else {
                unreachable!()
            };
            field.typed_initializer = changed_initializer;
            let mut changed_bindings = bindings.clone();
            changed_bindings
                .0
                .insert((source, source_rhs), changed_initializer);
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast: &changed_typed_ast,
                    source_index: index,
                    typed_bindings: &changed_bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor: constructor.clone(),
                    members: &changed_members,
                }
                .validate(),
                Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::UnsafeFieldInitializer,
                    ..
                })
            ));

            let int_symbol = store.types.get(definitions.int).reference_symbol().unwrap();
            let term_ref = store.types.alloc(Type::TermRef {
                prefix: definitions.no_prefix,
                target: TermRefTarget::Symbol(int_symbol),
            });
            let mut malformed_members = members.clone();
            let ClassMemberPicklePlan::Field(field) = &mut malformed_members[0] else {
                unreachable!()
            };
            field.declared_type = term_ref;
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast,
                    source_index: index,
                    typed_bindings: &bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor: constructor.clone(),
                    members: &malformed_members,
                }
                .validate(),
                Err(PickleError::UnsupportedTypeReference { ty }) if ty == term_ref
            ));

            let invalid_prefix = store.types.alloc(Type::TypeRef {
                prefix: definitions.object_type,
                target: TypeRefTarget::Symbol(int_symbol),
            });
            let mut malformed_members = members.clone();
            let ClassMemberPicklePlan::Field(field) = &mut malformed_members[0] else {
                unreachable!()
            };
            field.declared_type = invalid_prefix;
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast,
                    source_index: index,
                    typed_bindings: &bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor: constructor.clone(),
                    members: &malformed_members,
                }
                .validate(),
                Err(PickleError::UnsupportedTypeReference { ty }) if ty == invalid_prefix
            ));

            store.symbols.get_mut(constructor.symbol).flags = SymbolFlags::MUTABLE;
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast,
                    source_index: index,
                    typed_bindings: &bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor: constructor.clone(),
                    members: &members,
                }
                .validate(),
                Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::PrimaryConstructorInput,
                    symbol: Some(symbol),
                }) if symbol == constructor.symbol
            ));
            store.symbols.get_mut(constructor.symbol).flags = SymbolFlags::EMPTY;

            let parameter = constructor.parameter_clauses[0][0];
            let ClassMemberPicklePlan::Field(field) = members[0] else {
                unreachable!()
            };
            let seed = match &ast.get(field.source_tree).kind {
                TreeKind::ValDef(definition) => *definition.name.as_name(),
                _ => unreachable!(),
            };
            store.symbols.get_mut(parameter).name = seed;
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast,
                    source_index: index,
                    typed_bindings: &bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor,
                    members: &members,
                }
                .validate(),
                Err(PickleError::SourceDefinition { symbol }) if symbol == parameter
            ));
        },
    );
}

#[test]
fn constant_int_type_must_match_typed_literal_value() {
    let source_text = "package example\nclass PrivateValues:\n  private val seed: Int = 42\n";
    with_mutable_frontend(
        source_text,
        "PrivateValues",
        "PrivateValues.scala",
        |store,
         ast,
         source,
         index,
         definitions,
         typed_ast,
         class,
         members,
         constructor,
         bindings| {
            let class_tree = class_tree_for(ast, source, index, class);
            let ClassMemberPicklePlan::Field(field) = members[0] else {
                unreachable!()
            };
            let TreeKind::ValDef(definition) = &ast.get(field.source_tree).kind else {
                unreachable!()
            };
            let source_rhs = definition.rhs.unwrap();
            let typed_position = typed_ast.get(field.typed_initializer).position;
            let changed_type = store.types.alloc(Type::Constant(Constant::Int(7)));
            let mut changed_typed_ast = AstArena::<Typed>::new();
            let changed_initializer = changed_typed_ast.alloc(Tree {
                kind: TreeKind::Literal(Literal {
                    value: Constant::Int(42),
                }),
                position: typed_position,
                ty: changed_type,
            });
            let mut changed_members = members.clone();
            let ClassMemberPicklePlan::Field(field) = &mut changed_members[0] else {
                unreachable!()
            };
            field.typed_initializer = changed_initializer;
            let mut changed_bindings = bindings.clone();
            changed_bindings
                .0
                .insert((source, source_rhs), changed_initializer);

            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast: &changed_typed_ast,
                    source_index: index,
                    typed_bindings: &changed_bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "PrivateValues.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor,
                    members: &changed_members,
                }
                .validate(),
                Err(PickleError::FieldType { actual, .. }) if actual == changed_type
            ));
        },
    );
}

#[test]
fn constant_initializer_types_must_match_boolean_literal_values() {
    let source_text =
        "package example\nclass PrivateValues:\n  private val enabled: Boolean = true\n";
    with_mutable_frontend(
        source_text,
        "PrivateValues",
        "PrivateValues.scala",
        |store,
         ast,
         source,
         index,
         definitions,
         typed_ast,
         class,
         members,
         constructor,
         bindings| {
            let class_tree = class_tree_for(ast, source, index, class);
            let ClassMemberPicklePlan::Field(field) = members[0] else {
                unreachable!()
            };
            let TreeKind::ValDef(definition) = &ast.get(field.source_tree).kind else {
                unreachable!()
            };
            let source_rhs = definition.rhs.unwrap();
            let typed_position = typed_ast.get(field.typed_initializer).position;
            let changed_type = store.types.alloc(Type::Constant(Constant::Boolean(false)));
            let mut changed_typed_ast = AstArena::<Typed>::new();
            let changed_initializer = changed_typed_ast.alloc(Tree {
                kind: TreeKind::Literal(Literal {
                    value: Constant::Boolean(true),
                }),
                position: typed_position,
                ty: changed_type,
            });
            let mut changed_members = members.clone();
            let ClassMemberPicklePlan::Field(field) = &mut changed_members[0] else {
                unreachable!()
            };
            field.typed_initializer = changed_initializer;
            let mut changed_bindings = bindings.clone();
            changed_bindings
                .0
                .insert((source, source_rhs), changed_initializer);

            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast: &changed_typed_ast,
                    source_index: index,
                    typed_bindings: &changed_bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "PrivateValues.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor,
                    members: &changed_members,
                }
                .validate(),
                Err(PickleError::FieldType { actual, .. }) if actual == changed_type
            ));
        },
    );
}

#[test]
fn constant_types_are_limited_to_initializer_expressions() {
    let source_text = "package example\nclass Counter(start: Int):\n  private val seed: Int = 42\n";
    with_mutable_frontend(
        source_text,
        "Counter",
        "Counter.scala",
        |store,
         ast,
         source,
         index,
         definitions,
         typed_ast,
         class,
         members,
         constructor,
         bindings| {
            let class_tree = class_tree_for(ast, source, index, class);
            let ClassMemberPicklePlan::Field(field) = members[0] else {
                unreachable!()
            };
            let constant = store.types.alloc(Type::Constant(Constant::Int(7)));

            let original_field_info = store.symbols.get(field.symbol).info;
            store.symbols.get_mut(field.symbol).info = SymbolInfo::Complete(constant);
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast,
                    source_index: index,
                    typed_bindings: &bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor: constructor.clone(),
                    members: &members,
                }
                .validate(),
                Err(PickleError::FieldType { symbol, .. }) if symbol == field.symbol
            ));
            store.symbols.get_mut(field.symbol).info = original_field_info;

            let original_constructor_info = store.symbols.get(constructor.symbol).info;
            let SymbolInfo::Complete(signature_id) = original_constructor_info else {
                panic!("constructor signature is complete")
            };
            let Some(Type::Method(mut signature)) = store.types.try_get(signature_id).cloned()
            else {
                panic!("constructor has a method signature")
            };
            signature.params[0].ty = constant;
            let malformed_signature = store.types.alloc(Type::Method(signature));
            store.symbols.get_mut(constructor.symbol).info =
                SymbolInfo::Complete(malformed_signature);
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast,
                    source_index: index,
                    typed_bindings: &bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor: constructor.clone(),
                    members: &members,
                }
                .validate(),
                Err(PickleError::UnsupportedTypeReference { .. })
            ));
            store.symbols.get_mut(constructor.symbol).info = original_constructor_info;

            let parameter = constructor.parameter_clauses[0][0];
            let original_parameter_info = store.symbols.get(parameter).info;
            store.symbols.get_mut(parameter).info = SymbolInfo::Complete(constant);
            assert!(matches!(
                ClassStatePicklePlan {
                    store,
                    definitions,
                    source_ast: ast,
                    typed_ast,
                    source_index: index,
                    typed_bindings: &bindings,
                    source: SourceMetadata {
                        id: source,
                        name: "Counter.scala".into(),
                    },
                    class,
                    class_tree,
                    constructor,
                    members: &members,
                }
                .validate(),
                Err(PickleError::UnsupportedTypeReference { .. })
            ));
            store.symbols.get_mut(parameter).info = original_parameter_info;
        },
    );
}

#[test]
fn constructor_parameter_initializer_references_match_source_name_target_and_prefix() {
    let source_text = "package example\nclass Counter(start: Int, other: Int):\n  private val seed: Int = start\n";
    with_mutable_frontend(
        source_text,
        "Counter",
        "Counter.scala",
        |store,
         ast,
         source,
         index,
         definitions,
         typed_ast,
         class,
         members,
         constructor,
         bindings| {
            let class_tree = class_tree_for(ast, source, index, class);
            let ClassMemberPicklePlan::Field(field) = members[0] else {
                unreachable!()
            };
            let TreeKind::ValDef(definition) = &ast.get(field.source_tree).kind else {
                unreachable!()
            };
            let source_rhs = definition.rhs.unwrap();
            let parameter = constructor.parameter_clauses[0][0];
            let other = constructor.parameter_clauses[0][1];
            let mut changed_bindings = bindings.clone();

            for (prefix, target) in [
                (definitions.no_prefix, other),
                (definitions.object_type, parameter),
            ] {
                let bad_type = store.types.alloc(Type::TermRef {
                    prefix,
                    target: TermRefTarget::Symbol(target),
                });
                let mut changed_typed_ast = clone_typed_ast(typed_ast);
                let mut changed_initializer =
                    changed_typed_ast.get(field.typed_initializer).clone();
                changed_initializer.ty = bad_type;
                let changed_initializer = changed_typed_ast.alloc(changed_initializer);
                let mut changed_members = members.clone();
                let ClassMemberPicklePlan::Field(field) = &mut changed_members[0] else {
                    unreachable!()
                };
                field.typed_initializer = changed_initializer;
                changed_bindings
                    .0
                    .insert((source, source_rhs), changed_initializer);
                assert!(matches!(
                    ClassStatePicklePlan {
                        store,
                        definitions,
                        source_ast: ast,
                        typed_ast: &changed_typed_ast,
                        source_index: index,
                        typed_bindings: &changed_bindings,
                        source: SourceMetadata {
                            id: source,
                            name: "Counter.scala".into(),
                        },
                        class,
                        class_tree,
                        constructor: constructor.clone(),
                        members: &changed_members,
                    }
                    .validate(),
                    Err(PickleError::Unsupported {
                        feature: UnsupportedFeature::UnsafeFieldInitializer,
                        ..
                    })
                ));
            }
        },
    );
}

#[test]
fn signed_and_full_width_int_literals_keep_exact_source_values() {
    let source_text = "package example\nclass LiteralEdges:\n  private val minusOne: Int = -1\n  private val minInt: Int = -2147483648\n  private val hexOnes: Int = 0xffffffff\n  private val hexMin: Int = 0x80000000\n";
    with_frontend(source_text, |ast, source, index, definitions, typer| {
        let (class, members, constructor, bindings) = class_state_plan(
            ast,
            source,
            index,
            definitions,
            typer,
            "LiteralEdges",
            "LiteralEdges.scala",
        );
        let class_tree = class_tree_for(ast, source, index, class);
        assert_eq!(members.len(), 4);
        ClassStatePicklePlan {
            store: typer.store(),
            definitions,
            source_ast: ast,
            typed_ast: typer.typed_ast(),
            source_index: index,
            typed_bindings: &bindings,
            source: SourceMetadata {
                id: source,
                name: "LiteralEdges.scala".into(),
            },
            class,
            class_tree,
            constructor,
            members: &members,
        }
        .validate()
        .unwrap();
    });
}
