use std::collections::HashMap;

use dotty_core::ast::*;
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_pickler::*;
use dotty_typer::SourceTyper;

#[derive(Clone, Default)]
struct Bindings(HashMap<(SourceId, TreeId<Untyped>), TreeId<Typed>>);

impl SourceTypedBindings for Bindings {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.0.get(&(source, tree)).copied()
    }
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
        let mut typed_ast = AstArena::new();
        for (_, tree) in typer.typed_ast().iter() {
            typed_ast.alloc(tree.clone());
        }
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
    });
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
        let stale_bindings = Bindings(stale_bindings);
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
