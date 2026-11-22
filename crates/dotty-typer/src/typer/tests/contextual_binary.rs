use super::*;
use dotty_classloader::classloader::{
    ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, LoadingSession,
};

#[test]
fn separate_compilation_finds_the_binary_conditional_and_its_companion_leaf() {
    let text = "package consumer; import evidence.{TC, Box}; class Client { def ask(using t: TC[Box[Int]]): TC[Box[Int]] = t; def result: TC[Box[Int]] = ask }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "result");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let resolver = contextual_fixture_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    typer.type_method_definition(tree).unwrap();
    let rhs = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(ask) = &typer.typed_ast().get(rhs).kind else {
        panic!()
    };
    let TreeKind::Apply(boxes) = &typer.typed_ast().get(ask.args[0]).kind else {
        panic!()
    };
    let TreeKind::TypeApply(tapp) = &typer.typed_ast().get(boxes.function).kind else {
        panic!()
    };
    assert_eq!(typer.typed_ast().get(tapp.args[0]).ty, definitions.int);
    for (tree, name) in [(tapp.function, "boxes"), (boxes.args[0], "ints")] {
        let Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } = typer.store.types.get(typer.typed_ast().get(tree).ty)
        else {
            panic!()
        };
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(*symbol).name.text()),
            name
        );
        assert!(!typer.is_current_source_symbol(*symbol));
    }
}

fn contextual_fixture_resolver(
    definitions: Definitions,
    packages: &Packages,
) -> ClasspathSymbolResolver<CompositeClassPath> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(
                root.join("tests/fixtures/contextual-arguments/binary"),
            )),
            Box::new(DirectoryClassPath::new(
                root.join("../dotty-tasty/tests/fixtures/scala3-library"),
            )),
            Box::new(JavaObjectStubPath),
        ]),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )
}

#[test]
fn imported_binary_structural_given_infers_the_implementation_type_and_passes_evidence() {
    let text = "package consumer; import structural.{Builders, Box, TC}; class Consumer { import Builders.given; def need(using x: TC[Box[Int]]):Int=x.tag; def use(using leaf:TC[Int]):Int=need; def selected(using leaf:TC[Int]):Int=Builders.boxes[Int](using leaf).tag }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "use");
    let leaf = method_parameter_symbol(&parsed, &index, source, method, 0);
    let resolver = contextual_fixture_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let TreeKind::Apply(call) = &typer
        .typed_ast()
        .get(typer.typed_index.get(source, rhs).unwrap())
        .kind
    else {
        panic!()
    };
    let result = typer.typed_ast().get(call.args[0]);
    let TreeKind::Apply(factory) = &result.kind else {
        panic!()
    };
    let Type::Applied { tycon, args } = typer.store.types.get(result.ty) else {
        panic!()
    };
    assert_eq!(args, &[definitions.int]);
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(class),
        ..
    } = typer.store.types.get(*tycon)
    else {
        panic!()
    };
    let TreeKind::TypeApply(applied) = &typer.typed_ast().get(factory.function).kind else {
        panic!()
    };
    assert_eq!(typer.typed_ast().get(applied.args[0]).ty, definitions.int);
    let get_ref = |tree| match typer.store.types.get(typer.typed_ast().get(tree).ty) {
        Type::TermRef {
            target: TermRefTarget::Symbol(s),
            ..
        } => *s,
        other => panic!("expected reference, found {other:?}"),
    };
    assert_eq!(get_ref(factory.args[0]), leaf);
    let factory = typer.store.symbols.get(get_ref(applied.function));
    let TreeKind::Select(selection) = &typer.typed_ast().get(applied.function).kind else {
        panic!("expected Builders.boxes")
    };
    let object = get_ref(selection.qualifier);
    assert_eq!(
        typer
            .store
            .names
            .resolve(typer.store.symbols.get(object).name.text()),
        "Builders"
    );
    assert_eq!(
        factory.owner,
        Some(typer.module_class_of_object(object).unwrap())
    );
    let implementation_symbol = *class;
    let implementation = typer.store.symbols.get(*class);
    assert_eq!(implementation.kind, SymbolKind::Class);
    assert_eq!(factory.kind, SymbolKind::Method);
    assert_eq!(factory.owner, implementation.owner);
    assert_eq!(
        typer.store.names.resolve(implementation.name.text()),
        "Builders$boxes"
    );
    assert_eq!(typer.store.names.resolve(factory.name.text()), "boxes");
    assert!(matches!(factory.origin, SymbolOrigin::Tasty(_)));
    let (selected, rhs) =
        method_definition_and_rhs(&parsed, typer.store(), &index, source, "selected");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(selected) else {
        panic!()
    };
    typer.type_method_definition(tree).unwrap();
    let mut tree = typer.typed_index.get(source, rhs).unwrap();
    if let TreeKind::Apply(app) = &typer.typed_arena.get(tree).kind {
        tree = app.function;
    }
    let TreeKind::Select(tag) = &typer.typed_arena.get(tree).kind else {
        panic!("expected tag selection")
    };
    let target = typer
        .store
        .types
        .get(typer.typed_arena.get(tree).ty)
        .reference_symbol()
        .unwrap();
    assert_eq!(typer.store.names.resolve(tag.name.text()), "tag");
    assert_eq!(
        typer.store.symbols.owner(target),
        Some(implementation_symbol)
    );
}

#[test]
fn predef_context_methods_use_the_library_symbol_and_keep_local_shadowing() {
    let text = "class Use { def summoned(using n: Int): Int = summon[Int]; def supplied(using n: Int): Int = summon[Int](using n); def missing: Int = summon[Int]; def legacy(using n: Int): Int = implicitly[Int] }; class Shadow { def summon[A](using n: Int): Int = 7; def shadowed(using n: Int): Int = summon[Int] }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for name in ["shadowed", "legacy", "missing", "summoned", "supplied"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        if name == "missing" {
            assert!(matches!(
                typer.type_method_definition(tree),
                Err(TyperError::MissingContextualArgument { .. })
            ));
            continue;
        }
        typer
            .type_method_definition(tree)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let typed = typer.typed_index.get(source, rhs).unwrap();
        if name == "summoned" || name == "supplied" {
            let param = method_parameter_symbol(&parsed, &index, source, method, 0);
            assert!(
                matches!(typer.store.types.get(typer.typed_ast().get(typed).ty), Type::TermRef {target:TermRefTarget::Symbol(symbol),..} if *symbol==param)
            );
        } else {
            let TreeKind::Apply(app) = &typer.typed_ast().get(typed).kind else {
                panic!()
            };
            let TreeKind::TypeApply(tapp) = &typer.typed_ast().get(app.function).kind else {
                panic!()
            };
            let Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } = typer
                .store
                .types
                .get(typer.typed_ast().get(tapp.function).ty)
            else {
                panic!()
            };
            assert_eq!(typer.is_current_source_symbol(*symbol), name == "shadowed");
        }
    }
}

#[test]
fn summoned_value_is_the_receiver_of_selections_and_method_calls() {
    let text = "trait A { def show: Int; def at(i: Int): Int }
        class Use {
          def selected(using a: A): Int = summon[A].show
          def called(using a: A): Int = ((summon[A])).at(4)
        }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let class = class_symbol(&parsed, &store, &index, source, "A");
    let resolver = library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    for (name, member) in [("selected", "show"), ("called", "at")] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let expected = method_named_on_class(&parsed, typer.store(), &index, source, member, class);
        let evidence = method_parameter_symbol(&parsed, &index, source, method, 0);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!("source method")
        };
        typer.type_method_definition(tree).unwrap();
        let source_select = match &parsed.ast.get(rhs).kind {
            TreeKind::Apply(call) => call.function,
            _ => rhs,
        };
        let TreeKind::Select(select) = &parsed.ast.get(source_select).kind else {
            panic!("member selection")
        };
        let typed = typer
            .source_typed_index()
            .get(source, source_select)
            .unwrap();
        let node = typer.typed_ast().get(typed);
        let TreeKind::Select(member) = &node.kind else {
            panic!("typed member selection")
        };
        assert_eq!(
            typer.store.types.get(node.ty).reference_symbol(),
            Some(expected)
        );
        let mut receiver = select.qualifier;
        loop {
            assert_eq!(
                typer.source_typed_index().get(source, receiver),
                Some(member.qualifier)
            );
            let TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) =
                &parsed.ast.get(receiver).kind
            else {
                break;
            };
            receiver = parens.inner;
        }
        assert_eq!(
            typer
                .store
                .types
                .get(typer.typed_ast().get(member.qualifier).ty)
                .reference_symbol(),
            Some(evidence)
        );
    }
}

#[test]
fn failed_summon_and_failed_member_lookup_report_their_own_sites_and_roll_back() {
    let text = "trait A { def show: Int }
        class Use {
          def missing: Int = summon[A].show
          def ambiguous(using x: A, y: A): Int = (summon[A]).show
          def unknown(using a: A): Int = summon[A].absent
        }";
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let resolver = library_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    for name in ["missing", "ambiguous", "unknown"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let TreeKind::Select(select) = &parsed.ast.get(rhs).kind else {
            panic!("source selection")
        };
        let summon = match &parsed.ast.get(select.qualifier).kind {
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => parens.inner,
            _ => select.qualifier,
        };
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!("source method")
        };
        let checkpoint = typer.store().checkpoint();
        for _ in 0..2 {
            let error = typer.type_method_definition(tree).unwrap_err();
            match error {
                TyperError::MissingContextualArgument {
                    source: at,
                    tree_index,
                    ..
                } if name == "missing" => {
                    assert_eq!((at, tree_index), (source, summon.index()));
                }
                TyperError::AmbiguousContextualArgument {
                    source: at,
                    tree_index,
                    candidates,
                    ..
                } if name == "ambiguous" => {
                    assert_eq!((at, tree_index), (source, summon.index()));
                    assert_eq!(
                        candidates,
                        [0, 1].map(|i| method_parameter_symbol(&parsed, &index, source, method, i))
                    );
                }
                TyperError::MemberNotFound {
                    source: at,
                    tree_index,
                    name: member,
                    ..
                } if name == "unknown" => {
                    assert_eq!((at, tree_index, member), (source, rhs.index(), select.name));
                }
                other => panic!("{name}: {other:?}"),
            }
            assert_eq!(typer.store().checkpoint(), checkpoint);
            assert!(typer.source_typed_index().is_empty());
        }
    }
}

pub(super) fn library_resolver(
    definitions: Definitions,
    packages: &Packages,
) -> ClasspathSymbolResolver<CompositeClassPath> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    ClasspathSymbolResolver::new(
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(
                root.join("../dotty-tasty/tests/fixtures/scala3-library"),
            )),
            Box::new(
                dotty_classloader::classloader::JdkClassPath::new(
                    std::path::PathBuf::from(
                        std::env::var_os("JAVA_HOME").expect("JDK 21 JAVA_HOME"),
                    )
                    .join("jmods"),
                )
                .unwrap(),
            ),
        ]),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )
}

#[test]
fn library_not_given_inverts_each_search_and_selects_the_real_value_method() {
    let text = "import scala.util.NotGiven; class Ev; class C { def need(using m: NotGiven[Ev]): Int = 1; def free: Int = need; def present(using a: Ev): Int = need; def ambiguous(using a: Ev,b: Ev): Int = need }; object Other { class NotGiven[A]; def ordinary(using m: NotGiven[Int]): Int = 7; def shadow(using instance: NotGiven[Int]): Int = ordinary }";
    let (parsed, mut store, packages, definitions, index, source) = parse_and_name(text);
    let resolver = library_resolver(definitions, &packages);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    for name in ["present", "ambiguous", "free", "shadow", "present"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let result = typer.type_method_definition(tree);
        if name == "present" {
            assert!(
                matches!(result, Err(TyperError::MissingContextualArgument { .. })),
                "{name}: {result:?}"
            );
            continue;
        }
        result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let typed = typer.typed_index.get(source, rhs).unwrap();
        let TreeKind::Apply(app) = &typer.typed_ast().get(typed).kind else {
            panic!()
        };
        if name == "shadow" {
            let param = method_parameter_symbol(&parsed, &index, source, method, 0);
            assert!(
                matches!(typer.store.types.get(typer.typed_ast().get(app.args[0]).ty), Type::TermRef{target:TermRefTarget::Symbol(symbol),..} if *symbol==param)
            );
            continue;
        }
        let TreeKind::Select(sel) = &typer.typed_ast().get(app.args[0]).kind else {
            panic!("expected NotGiven.value")
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(value),
            ..
        } = typer.store.types.get(typer.typed_ast().get(app.args[0]).ty)
        else {
            panic!()
        };
        let Type::TermRef {
            target: TermRefTarget::Symbol(object),
            ..
        } = typer
            .store
            .types
            .get(typer.typed_ast().get(sel.qualifier).ty)
        else {
            panic!()
        };
        assert_eq!(typer.store.names.resolve(sel.name.text()), "value");
        assert_eq!(
            typer
                .store
                .names
                .resolve(typer.store.symbols.get(*object).name.text()),
            "NotGiven"
        );
        assert_eq!(
            typer.store.symbols.get(*value).owner,
            Some(typer.module_class_of_object(*object).unwrap())
        );
    }
}

#[test]
fn binary_contextual_defaults_replay_the_selected_receiver_and_evaluated_argument() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/BinaryDefaultsClient.scala"
    ));
    let next = method_symbol(&parsed, &store, &index, source, "next");
    let resolver = contextual_fixture_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    for name in ["fallback", "supplied", "explicit", "noEarlierClause"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!("client method")
        };
        typer
            .type_method_definition(tree)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let ast = typer.typed_ast();
        let reference = |tree| {
            typer
                .store()
                .types
                .get(ast.get(tree).ty)
                .reference_symbol()
                .expect("reference")
        };
        let original = typer.typed_index.get(source, rhs).unwrap();
        let (call, shared) = match &ast.get(original).kind {
            TreeKind::Block(block) if name == "fallback" => {
                let [temp] = block.stats.as_slice() else {
                    panic!("one evaluation of next")
                };
                let TreeKind::ValDef(value) = &ast.get(*temp).kind else {
                    panic!("shared argument")
                };
                let TreeKind::Apply(init) = &ast.get(value.rhs.unwrap()).kind else {
                    panic!("next()")
                };
                assert_eq!(reference(init.function), next);
                (
                    block.expr,
                    Some(
                        typer
                            .typed_index
                            .synthetic_definition_symbol(*temp)
                            .unwrap(),
                    ),
                )
            }
            _ => (original, None),
        };
        let TreeKind::Apply(evidence) = &ast.get(call).kind else {
            panic!("contextual call")
        };
        let [argument] = evidence.args.as_slice() else {
            panic!("one contextual parameter")
        };
        if name == "supplied" {
            let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
                panic!()
            };
            assert_eq!(
                Some(reference(*argument)),
                index.symbol_at(source, def.value_param_clauses[1][0])
            );
            assert_eq!(evidence.kind, ApplyKind::Regular);
            continue;
        }
        assert_eq!(evidence.kind, ApplyKind::Using);
        if name == "explicit" {
            assert!(matches!(&ast.get(*argument).kind, TreeKind::Literal(_)));
            continue;
        }
        let (target, getter) = if name == "fallback" {
            let TreeKind::Apply(target) = &ast.get(evidence.function).kind else {
                panic!("offset(x)")
            };
            let TreeKind::Apply(getter) = &ast.get(*argument).kind else {
                panic!("getter(x)")
            };
            assert_eq!(target.args, getter.args);
            assert_eq!(Some(reference(target.args[0])), shared);
            (target.function, getter.function)
        } else {
            (evidence.function, *argument)
        };
        let TreeKind::Select(target_selection) = &ast.get(target).kind else {
            panic!("method selection")
        };
        let TreeKind::Select(getter_selection) = &ast.get(getter).kind else {
            panic!("getter selection")
        };
        assert_eq!(target_selection.qualifier, getter_selection.qualifier);
        let position = usize::from(name == "fallback");
        assert_eq!(
            typer.store().default_getter(reference(target), position),
            Some(reference(getter))
        );
        let getter = typer.store().symbols.get(reference(getter));
        assert_eq!(getter.owner, typer.store().symbols.owner(reference(target)));
        assert!(matches!(getter.origin, SymbolOrigin::Tasty(_)));
        assert_eq!(
            typer.store().names.resolve(getter.name.text()),
            if name == "fallback" {
                "offset$default$2"
            } else {
                "read$default$1"
            }
        );
    }
}

#[test]
fn binary_contextual_default_preserves_ambiguity_and_rolls_back_binary_loading() {
    assert_binary_ambiguity(
        include_str!("../../../tests/fixtures/contextual-arguments/BinaryDefaultsAmbiguous.scala"),
        "ambiguous",
    );
}

#[test]
fn ambiguous_evidence_on_an_applied_reader_prevents_the_default() {
    assert_binary_ambiguity(
        include_str!("../../../tests/fixtures/contextual-arguments/AppliedPairAmbiguous.scala"),
        "ambiguous",
    );
}

fn assert_binary_ambiguity(text: &str, name: &str) {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, name);
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        panic!()
    };
    let site = match &parsed.ast.get(rhs).kind {
        // In choose(), evidence is requested by the selected function before ().
        TreeKind::Apply(call) if call.args.is_empty() => call.function,
        _ => rhs,
    };
    let candidates = def
        .value_param_clauses
        .last()
        .unwrap()
        .iter()
        .map(|p| index.symbol_at(source, *p).unwrap())
        .collect::<Vec<_>>();
    let resolver = contextual_fixture_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    let before = typer.store().checkpoint();
    let trees = typer.typed_arena.checkpoint();
    for _ in 0..2 {
        let error = typer.type_method_definition(tree).unwrap_err();
        assert!(
            matches!(&error, TyperError::AmbiguousContextualArgument { tree_index, candidates: found, .. }
                if *tree_index == site.index() && *found == candidates),
            "{error:?}"
        );
        assert_eq!(typer.store().checkpoint(), before);
        assert_eq!(typer.typed_arena.checkpoint(), trees);
        assert!(typer.typed_index.is_empty());
    }
}

#[test]
fn binary_contextual_default_rejects_a_mismatched_getter_signature_before_insertion() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(include_str!(
        "../../../tests/fixtures/contextual-arguments/BinaryDefaultsClient.scala"
    ));
    let (explicit, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "explicit");
    let fallback = method_symbol(&parsed, &store, &index, source, "fallback");
    let Some(SourceDefinition::Canonical {
        tree: explicit_tree,
        ..
    }) = index.definition_of(explicit)
    else {
        panic!()
    };
    let Some(SourceDefinition::Canonical {
        tree: fallback_tree,
        ..
    }) = index.definition_of(fallback)
    else {
        panic!()
    };
    let resolver = contextual_fixture_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    typer.type_method_definition(explicit_tree).unwrap();
    let root = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(call) = &typer.typed_ast().get(root).kind else {
        panic!()
    };
    let TreeKind::Apply(prefix) = &typer.typed_ast().get(call.function).kind else {
        panic!()
    };
    let target = typer
        .store
        .types
        .get(typer.typed_ast().get(prefix.function).ty)
        .reference_symbol()
        .unwrap();
    let getter = typer
        .store
        .default_getter(target, 1)
        .expect("offset default for n");
    let original = typer
        .completed_expression_symbol_info(getter, &mut Vec::new())
        .unwrap();
    let Type::Method(correct) = typer.store.types.get(original).clone() else {
        panic!("expected a getter taking x")
    };
    for wrong_result in [false, true] {
        let mut wrong = correct.clone();
        if wrong_result {
            wrong.result = defs.boolean;
        } else {
            wrong.params[0].ty = defs.boolean;
        }
        let info = typer.store.types.alloc(Type::Method(wrong));
        typer
            .store
            .symbols
            .set_info(getter, SymbolInfo::Complete(info));
        let before = typer.store.checkpoint();
        let err = typer.type_method_definition(fallback_tree).unwrap_err();
        if wrong_result {
            assert!(
                matches!(err, TyperError::ExpectedExpressionTypeMismatch { actual, .. } if actual == defs.boolean),
                "{err:?}"
            );
        } else {
            assert!(
                matches!(err, TyperError::UsingApplicationDeferred { .. }),
                "{err:?}"
            );
        }
        assert_eq!(typer.store.checkpoint(), before);
        assert!(typer.typed_index.get(source, fallback_tree).is_none());
    }
    typer
        .store
        .symbols
        .set_info(getter, SymbolInfo::Complete(original));
    typer.type_method_definition(fallback_tree).unwrap();
}

#[test]
fn binary_pick_default_reuses_the_receiver_and_both_arguments_in_source_order() {
    assert_pair_default(
        include_str!("../../../tests/fixtures/contextual-arguments/BinaryPairClient.scala"),
        "run",
        "pick",
    );
}

#[test]
fn binary_pick_default_shares_the_applied_reader_before_its_arguments() {
    assert_pair_default(APPLIED_PAIR_CLIENT, "fallback", "pick");
}

fn assert_pair_default(text: &str, method_name: &str, target_name: &str) {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (run, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, method_name);
    let source_methods = ["reader", "left", "right"]
        .map(|name| method_symbol(&parsed, &store, &index, source, name));
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(run) else {
        panic!("expected run definition")
    };
    let TreeKind::Apply(original) = &parsed.ast.get(rhs).kind else {
        panic!("expected pick(left(), right())")
    };
    let TreeKind::Select(selected) = &parsed.ast.get(original.function).kind else {
        panic!("expected reader.pick")
    };
    let [left, right] = original.args.as_slice() else {
        panic!("expected two source arguments")
    };
    let source_operands = [selected.qualifier, *left, *right];
    let resolver = contextual_fixture_resolver(defs, &packages);
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(resolver));
    typer.type_method_definition(tree).unwrap();
    let ast = typer.typed_ast();
    let store = typer.store();
    let bindings = typer.source_typed_index();
    let reference = |tree| {
        store
            .types
            .get(ast.get(tree).ty)
            .reference_symbol()
            .unwrap()
    };
    let TreeKind::Block(block) = &ast.get(bindings.get(source, rhs).unwrap()).kind else {
        panic!("expected shared operands")
    };
    let [receiver, left, right] = block.stats.as_slice() else {
        panic!("expected exactly three operand definitions")
    };
    let [receiver_temp, left_temp, right_temp] =
        [*receiver, *left, *right].map(|stat| bindings.synthetic_definition_symbol(stat).unwrap());
    for ((stat, operand), method) in block.stats.iter().zip(source_operands).zip(source_methods) {
        let TreeKind::ValDef(shared) = &ast.get(*stat).kind else {
            panic!("expected temporary")
        };
        let init = shared.rhs.unwrap();
        assert_eq!(bindings.get(source, operand), Some(init));
        let function = if !matches!(parsed.ast.get(operand).kind, TreeKind::Apply(_)) {
            assert_eq!(stat, receiver);
            assert!(matches!(
                ast.get(init).kind,
                TreeKind::Ident(_) | TreeKind::Select(_)
            ));
            init
        } else {
            let TreeKind::Apply(call) = &ast.get(init).kind else {
                panic!("expected argument method call")
            };
            assert!(call.args.is_empty());
            call.function
        };
        assert_eq!(reference(function), method);
    }
    assert_ne!(left_temp, right_temp);
    let TreeKind::Apply(contextual) = &ast.get(block.expr).kind else {
        panic!("expected contextual application")
    };
    assert_eq!(contextual.kind, ApplyKind::Using);
    let [getter_call] = contextual.args.as_slice() else {
        panic!("expected the default getter")
    };
    let [target, getter] = [contextual.function, *getter_call].map(|root| {
        let TreeKind::Apply(call) = &ast.get(root).kind else {
            panic!("expected the ordinary parameter clause")
        };
        assert_eq!(call.kind, ApplyKind::Regular);
        let [x, y] = call.args.as_slice() else {
            panic!("expected both earlier arguments")
        };
        let TreeKind::Select(member) = &ast.get(call.function).kind else {
            panic!("expected a member of the shared receiver")
        };
        for (operand, temporary) in [
            (member.qualifier, receiver_temp),
            (*x, left_temp),
            (*y, right_temp),
        ] {
            assert!(matches!(ast.get(operand).kind, TreeKind::Ident(_)));
            assert_eq!(reference(operand), temporary);
        }
        reference(call.function)
    });
    assert_eq!(store.default_getter(target, 2), Some(getter));
    assert_eq!(store.default_getter_origin(getter), Some((target, 2)));
    let symbol = store.symbols.get(getter);
    assert!(matches!(symbol.origin, SymbolOrigin::Tasty(_)));
    assert_eq!(store.symbols.get(target).origin, symbol.origin);
    assert_eq!(
        store.names.resolve(store.symbols.get(target).name.text()),
        target_name
    );
    assert_eq!(
        store.names.resolve(symbol.name.text()),
        format!("{target_name}$default$3")
    );
    let SymbolInfo::Complete(qualified) = store.symbols.get(receiver_temp).info else {
        panic!("expected typed receiver")
    };
    let owner = store.types.get(qualified).reference_symbol().unwrap();
    assert_eq!(store.symbols.owner(target), Some(owner));
    assert_eq!(symbol.owner, Some(owner));
}

const APPLIED_PAIR_CLIENT: &str =
    include_str!("../../../tests/fixtures/contextual-arguments/AppliedPairClient.scala");

#[test]
fn explicit_binary_call_selects_pick_on_the_applied_reader() {
    assert_applied_pair_call(
        APPLIED_PAIR_CLIENT,
        "direct",
        "pick",
        ApplyKind::Using,
        None,
    );
}

#[test]
fn binary_pick_uses_the_inferred_or_written_witness_on_an_applied_reader() {
    assert_applied_pair_call(
        APPLIED_PAIR_CLIENT,
        "supplied",
        "pick",
        ApplyKind::Regular,
        Some(0),
    );
    assert_applied_pair_call(
        APPLIED_PAIR_CLIENT,
        "explicit",
        "pick",
        ApplyKind::Using,
        Some(1),
    );
}

fn assert_applied_pair_call(
    text: &str,
    name: &str,
    target: &str,
    kind: ApplyKind,
    witness: Option<usize>,
) {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let reader = method_symbol(&parsed, &store, &index, source, "reader");
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, name);
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(contextual_fixture_resolver(defs, &packages)));
    typer.type_method_definition(tree).unwrap();
    let ast = typer.typed_ast();
    let root = typer.typed_index.get(source, rhs).unwrap();
    let TreeKind::Apply(using) = &ast.get(root).kind else {
        panic!("explicit using")
    };
    assert_eq!(using.kind, kind);
    assert_eq!(
        typer.store.types.get(ast.get(root).ty).reference_symbol(),
        typer.store.types.get(defs.int).reference_symbol()
    );
    let TreeKind::Apply(ordinary) = &ast.get(using.function).kind else {
        panic!("ordinary clause")
    };
    let TreeKind::Select(pick) = &ast.get(ordinary.function).kind else {
        panic!("pick selection")
    };
    let reference = |tree| {
        typer
            .store
            .types
            .get(ast.get(tree).ty)
            .reference_symbol()
            .unwrap()
    };
    let [evidence] = using.args.as_slice() else {
        panic!("one contextual argument")
    };
    if let Some(ordinal) = witness {
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let symbol = index
            .symbol_at(source, def.value_param_clauses.last().unwrap()[ordinal])
            .unwrap();
        assert_eq!(reference(*evidence), symbol);
    } else {
        assert!(matches!(ast.get(*evidence).kind, TreeKind::Literal(_)));
    }
    let member = typer.store.symbols.get(reference(ordinary.function));
    assert_eq!(typer.store.names.resolve(member.name.text()), target);
    assert!(matches!(member.origin, SymbolOrigin::Tasty(_)));
    let TreeKind::Apply(receiver) = &ast.get(pick.qualifier).kind else {
        panic!("reader()")
    };
    assert!(receiver.args.is_empty());
    assert_eq!(reference(receiver.function), reader);
}

#[test]
fn binary_ordinary_default_after_using_replays_the_seed_and_receiver() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(APPLIED_PAIR_CLIENT);
    let reader = method_symbol(&parsed, &store, &index, source, "reader");
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(contextual_fixture_resolver(defs, &packages)));
    for name in ["stableChoice", "appliedChoice", "writtenChoice"] {
        let (method, rhs) = method_definition_and_rhs(&parsed, typer.store(), &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!()
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            panic!()
        };
        let seed = index
            .symbol_at(source, def.value_param_clauses.last().unwrap()[0])
            .unwrap();
        typer
            .type_method_definition(tree)
            .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        let ast = typer.typed_ast();
        let reference = |tree| {
            typer
                .store
                .types
                .get(ast.get(tree).ty)
                .reference_symbol()
                .unwrap()
        };
        let root = typer.typed_index.get(source, rhs).unwrap();
        let (call, receiver) = if name == "appliedChoice" {
            let TreeKind::Block(block) = &ast.get(root).kind else {
                panic!("receiver temporary")
            };
            let [temp] = block.stats.as_slice() else {
                panic!("only receiver needs sharing")
            };
            let TreeKind::ValDef(value) = &ast.get(*temp).kind else {
                panic!()
            };
            let TreeKind::Apply(init) = &ast.get(value.rhs.unwrap()).kind else {
                panic!()
            };
            assert_eq!(reference(init.function), reader);
            (
                block.expr,
                typer.typed_index.synthetic_definition_symbol(*temp),
            )
        } else if name == "stableChoice" {
            (root, index.symbol_at(source, def.value_param_clauses[0][0]))
        } else {
            (root, None)
        };
        let TreeKind::Apply(ordinary) = &ast.get(call).kind else {
            panic!("choose(seed)(value)")
        };
        let [value] = ordinary.args.as_slice() else {
            panic!()
        };
        let TreeKind::Apply(target) = &ast.get(ordinary.function).kind else {
            panic!()
        };
        assert_eq!(reference(target.args[0]), seed);
        let TreeKind::Select(member) = &ast.get(target.function).kind else {
            panic!()
        };
        assert_eq!(typer.store.names.resolve(member.name.text()), "choose");
        if name == "writtenChoice" {
            assert!(matches!(ast.get(*value).kind, TreeKind::Literal(_)));
            assert_eq!(target.kind, ApplyKind::Using);
            let TreeKind::Apply(init) = &ast.get(member.qualifier).kind else {
                panic!()
            };
            assert_eq!(reference(init.function), reader);
            continue;
        }
        let TreeKind::Apply(getter) = &ast.get(*value).kind else {
            panic!("choose default getter")
        };
        let TreeKind::Select(selected) = &ast.get(getter.function).kind else {
            panic!()
        };
        assert_eq!(getter.args, target.args);
        assert_eq!(selected.qualifier, member.qualifier);
        assert_eq!(Some(reference(selected.qualifier)), receiver);
        let getter_id = reference(getter.function);
        assert_eq!(
            typer.store.default_getter(reference(target.function), 1),
            Some(getter_id)
        );
        assert!(matches!(
            typer.store.symbols.get(getter_id).origin,
            SymbolOrigin::Tasty(_)
        ));
    }
}

#[test]
fn binary_ordinary_default_needs_the_earlier_contextual_seed() {
    let text =
        include_str!("../../../tests/fixtures/contextual-arguments/AppliedPairAmbiguous.scala");
    assert_binary_ambiguity(text, "ambiguousSeed");
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(text);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "missingSeed");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!()
    };
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages)
        .with_resolver(Box::new(contextual_fixture_resolver(defs, &packages)));
    let before = typer.store.checkpoint();
    let TreeKind::Apply(call) = &parsed.ast.get(rhs).kind else {
        panic!()
    };
    let err = typer.type_method_definition(tree).unwrap_err();
    assert!(
        matches!(err, TyperError::MissingContextualArgument { tree_index, .. } if tree_index == call.function.index()),
        "{err:?}"
    );
    assert_eq!(typer.store.checkpoint(), before);
    assert!(typer.typed_index.is_empty());
}
