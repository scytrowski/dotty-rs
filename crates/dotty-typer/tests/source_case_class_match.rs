use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::{TreeKind, TypedAst};
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Name, Namespace, Packages, SemanticStore, SourceId,
    SourceText, SourceTypedIndex, SymbolFlags, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin,
    TreeId, Typed, Untyped,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/source-case-class-match-3.9.0/Positive.scala");
const FLAGGED_SOURCE: &str =
    include_str!("fixtures/source-case-class-product2-3.9.0/Positive.scala");
const CASE_BINDER_ESCAPE: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/CaseBinderEscape.scala");
const NON_CASE_MISSING_EXTRACTOR: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/NonCaseMissingExtractor.scala");
const WRONG_PATTERN_TYPE: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/WrongPatternType.scala");
const WRONG_RESULT: &str = include_str!("fixtures/source-case-class-match-3.9.0/WrongResult.scala");
const WRONG_ARITY_ZERO: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/WrongArityZero.scala");
const WRONG_ARITY_TWO: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/WrongArityTwo.scala");
const WRONG_SELECTOR_TYPE: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/WrongSelectorType.scala");
const WRONG_CONSTRUCTOR_TYPE: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/WrongConstructorType.scala");
const EXPLICIT_UNAPPLY_WRONG_RESULT: &str =
    include_str!("fixtures/source-case-class-match-3.9.0/ExplicitUnapplyWrongResult.scala");
const SOURCE_TRAIT_PRODUCT1: &str =
    include_str!("fixtures/source-sealed-adt-match-3.9.0/Event.scala");
type MatchRoots = (SymbolId, Vec<(TreeId<Untyped>, TreeId<Untyped>)>);

#[test]
fn source_trait_parent_product1_preserves_identity_in_both_completion_routes() {
    let class_first =
        source_trait_product1_session(SourceId::from_index(1994), false, SOURCE_TRAIT_PRODUCT1);
    let selector_first =
        source_trait_product1_session(SourceId::from_index(2994), true, SOURCE_TRAIT_PRODUCT1);
    assert_eq!(class_first, selector_first);
    assert_eq!(class_first.len(), 2);
    assert!(class_first.iter().any(|row| row.starts_with("Created|")));
    assert!(class_first.iter().any(|row| row.starts_with("Deleted|")));
    let reordered = SOURCE_TRAIT_PRODUCT1.replace(
        "sealed trait Event\n\nfinal case class Created(id: Int) extends Event\nfinal case class Deleted(code: Int) extends Event",
        "final case class Created(id: Int) extends Event\nfinal case class Deleted(code: Int) extends Event\n\nsealed trait Event",
    );
    assert_ne!(reordered, SOURCE_TRAIT_PRODUCT1);
    assert_eq!(
        source_trait_product1_session(SourceId::from_index(6994), false, &reordered),
        class_first,
        "the source trait declaration may follow its variants"
    );
}

#[test]
fn precompleted_source_parent_graph_failures_roll_back_and_retry_in_both_routes() {
    let source_text =
        SOURCE_TRAIT_PRODUCT1.replace("sealed trait Event", "sealed trait Event\ntrait Other");
    for (selector_first, malformed_graph) in [
        (false, "unrelated parent"),
        (true, "unrelated parent"),
        (false, "duplicate edge"),
        (true, "duplicate edge"),
        (false, "invalid TypeId"),
        (true, "invalid TypeId"),
        (false, "missing parent ClassInfo"),
        (true, "missing parent ClassInfo"),
    ] {
        let source = SourceId::from_index(7994 + u32::from(selector_first));
        let mut session = Session::new(&source_text, source);
        let event = session.source_type_symbol("Event");
        let other = session.source_type_symbol("Other");
        let created = session.source_type_symbol("Created");
        let selector = session
            .parsed
            .ast
            .iter()
            .find_map(|(tree, _)| session.index.derived_symbol_at(created, source, tree))
            .expect("Created._1 symbol");
        session.with_typer(|_, _, _, _, typer| {
            typer
                .complete_symbol(event)
                .expect("complete canonical Event");
            typer
                .complete_symbol(other)
                .expect("complete canonical Other");
        });

        let (parent_info, canonical_info) = {
            let SymbolInfo::Complete(parent_info) = *session.store.symbols.info(event) else {
                panic!("Event is precompleted")
            };
            let Some(Type::ClassInfo(canonical_info)) = session.store.types.try_get(parent_info)
            else {
                panic!("Event has canonical ClassInfo")
            };
            (parent_info, canonical_info.clone())
        };
        let other_parent = session.store.types.alloc(Type::TypeRef {
            prefix: session.definitions.no_prefix,
            target: TypeRefTarget::Symbol(other),
        });
        let invalid_parent = session.store.types.reserve().id();
        let mut corrupted = canonical_info.clone();
        corrupted.parents = match malformed_graph {
            "unrelated parent" => vec![other_parent],
            "duplicate edge" => vec![
                session.definitions.object_type,
                session.definitions.object_type,
            ],
            "invalid TypeId" => vec![invalid_parent],
            "missing parent ClassInfo" => Vec::new(),
            _ => unreachable!(),
        };
        let malformed_info = session.store.types.alloc(Type::ClassInfo(corrupted));
        session
            .store
            .symbols
            .set_info(event, SymbolInfo::Complete(malformed_info));
        let before = session.store.checkpoint();
        let error = if selector_first {
            session.with_typer(|_, _, _, _, typer| typer.complete_symbol(selector))
        } else {
            session.with_typer(|_, _, _, _, typer| typer.complete_symbol(created))
        }
        .expect_err("forged or malformed parent graph is rejected");
        assert!(
            matches!(&error, TyperError::MalformedClassInfo { symbol, info } if *symbol == event && *info == malformed_info),
            "{malformed_graph}, selector_first={selector_first}: {error:?}"
        );
        assert_eq!(session.store.checkpoint(), before);
        assert_eq!(*session.store.symbols.info(created), SymbolInfo::Missing);
        assert_eq!(*session.store.symbols.info(selector), SymbolInfo::Missing);
        assert_eq!(
            *session.store.symbols.info(event),
            SymbolInfo::Complete(malformed_info),
            "rollback preserves preexisting metadata for repair"
        );
        let retry = if selector_first {
            session.with_typer(|_, _, _, _, typer| typer.complete_symbol(selector))
        } else {
            session.with_typer(|_, _, _, _, typer| typer.complete_symbol(created))
        }
        .expect_err("the malformed parent graph fails stably on retry");
        assert_eq!(format!("{retry:?}"), format!("{error:?}"));
        assert_eq!(session.store.checkpoint(), before);

        session
            .store
            .symbols
            .set_info(event, SymbolInfo::Complete(parent_info));
        if selector_first {
            session.with_typer(|_, _, _, _, typer| {
                typer
                    .complete_symbol(selector)
                    .expect("valid same-store selector-first retry")
            });
        } else {
            session.with_typer(|_, _, _, _, typer| {
                typer
                    .complete_symbol(created)
                    .expect("valid same-store class-first retry")
            });
        }
    }
}

#[test]
fn source_trait_parent_product1_rolls_back_then_retries_sibling_in_same_session() {
    let source = SourceId::from_index(3994);
    let mut session = Session::new(SOURCE_TRAIT_PRODUCT1, source);
    let class = session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if session
                    .store
                    .names
                    .resolve(definition.name.as_name().text())
                    == "Created" =>
            {
                session.index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    let selector = session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, _)| session.index.derived_symbol_at(class, source, tree))
        .unwrap();
    session.with_typer_without_resolver(|_, _, _, _, typer| {
        let before = typer.store().checkpoint();
        let error = typer
            .complete_symbol(class)
            .expect_err("Product absence fails after source-parent and selector work");
        assert!(matches!(error, TyperError::TypeNameNotFound { name, .. } if typer.store().names.resolve(name.text()) == "Product"));
        assert_eq!(typer.store().checkpoint(), before, "all semantic mutations roll back");
        assert_eq!(*typer.store().symbols.info(class), SymbolInfo::Missing);
        assert_eq!(*typer.store().symbols.info(selector), SymbolInfo::Missing);
    });

    let sibling = session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if session
                    .store
                    .names
                    .resolve(definition.name.as_name().text())
                    == "Deleted" =>
            {
                session.index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    session.with_typer(|_, _, _, _, typer| {
        typer
            .complete_symbol(sibling)
            .expect("valid sibling completes after failed transaction in same session");
    });
}

#[test]
fn source_trait_parent_product1_rejects_generic_and_class_parent_shapes() {
    let cases = [
        (
            "generic_parent",
            "package adt\ntrait Event[A]\ncase class Created(id: Int) extends Event[Int]\n",
        ),
        (
            "class_parent",
            "package adt\nclass Base\ncase class Created(id: Int) extends Base\n",
        ),
        (
            "non_int_product1_field",
            "package adt\ntrait Event\ncase class Created(id: Boolean) extends Event\n",
        ),
    ];
    for (offset, (label, text)) in cases.into_iter().enumerate() {
        let source = SourceId::from_index(4994 + offset as u32);
        let mut session = Session::new(text, source);
        let class = session
            .parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if session
                        .store
                        .names
                        .resolve(definition.name.as_name().text())
                        == "Created" =>
                {
                    session.index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap();
        let selector = session
            .parsed
            .ast
            .iter()
            .find_map(|(tree, _)| session.index.derived_symbol_at(class, source, tree))
            .unwrap();
        session.with_typer(|_, _, _, _, typer| {
            let error = typer
                .complete_symbol(class)
                .expect_err("unsupported parent shape must not enter Product1 completion");
            match label {
                "non_int_product1_field" => assert!(
                    matches!(error, TyperError::UnsupportedProduct1FieldType { .. }),
                    "{label}: {error:?}"
                ),
                _ => assert!(
                    matches!(error, TyperError::MalformedClassParent { .. }),
                    "{label}: {error:?}"
                ),
            }
            let retry = typer
                .complete_symbol(selector)
                .expect_err("selector-first route must reject the same unsupported shape");
            match label {
                "non_int_product1_field" => assert!(matches!(
                    retry,
                    TyperError::UnsupportedProduct1FieldType { .. }
                )),
                _ => assert!(matches!(retry, TyperError::MalformedClassParent { .. })),
            }
        });
    }
}

#[test]
fn source_trait_parent_product1_defers_multi_parent_selector_shape() {
    let source = SourceId::from_index(7994);
    let text =
        "package adt\ntrait Event\ntrait Other\ncase class Created(id: Int) extends Event, Other\n";
    let mut session = Session::new(text, source);
    let class = session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if session
                    .store
                    .names
                    .resolve(definition.name.as_name().text())
                    == "Created" =>
            {
                session.index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    let selector = session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, _)| session.index.derived_symbol_at(class, source, tree))
        .unwrap();
    session.with_typer(|_, _, _, _, typer| {
        let class_info = typer
            .complete_symbol(class)
            .expect("ordinary multiple trait parent graph still completes");
        let Some(Type::ClassInfo(info)) = typer.store().types.try_get(class_info) else {
            panic!("ClassInfo")
        };
        assert!(
            !info
                .parents
                .iter()
                .any(|parent| match typer.store().types.try_get(*parent) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    }) => {
                        typer
                            .store()
                            .names
                            .resolve(typer.store().symbols.get(*symbol).name.text())
                            == "Product"
                    }
                    _ => false,
                })
        );
        assert!(matches!(
            typer.complete_symbol(selector),
            Err(TyperError::DeclarationContextMissing { .. })
        ));
        assert_eq!(*typer.store().symbols.info(selector), SymbolInfo::Missing);
    });
}

#[test]
fn source_trait_parent_does_not_enable_product2_completion() {
    let source = SourceId::from_index(8995);
    let text = "package adt\ntrait Event\ncase class Created(id: Int, code: Int) extends Event\n";
    let mut session = Session::new(text, source);
    let class = session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if session
                    .store
                    .names
                    .resolve(definition.name.as_name().text())
                    == "Created" =>
            {
                session.index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    let selectors = session
        .parsed
        .ast
        .iter()
        .filter_map(|(tree, _)| session.index.derived_symbol_at(class, source, tree))
        .collect::<Vec<_>>();
    session.with_typer(|_, _, _, _, typer| {
        let class_info = typer
            .complete_symbol(class)
            .expect("ordinary two-field source class retains its parent graph");
        let Some(Type::ClassInfo(info)) = typer.store().types.try_get(class_info) else {
            panic!("ClassInfo")
        };
        assert!(
            !info
                .parents
                .iter()
                .any(|parent| match typer.store().types.try_get(*parent) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    }) => {
                        typer
                            .store()
                            .names
                            .resolve(typer.store().symbols.get(*symbol).name.text())
                            == "Product"
                    }
                    _ => false,
                })
        );
        assert!(
            selectors.is_empty(),
            "explicit-parent two-field classes do not acquire Product2 selectors"
        );
    });
}

#[test]
fn source_trait_parent_product1_records_frozen_match_first_error_movement() {
    let source = SourceId::from_index(8994);
    let mut session = Session::new(SOURCE_TRAIT_PRODUCT1, source);
    let roots = [
        ("extract", session.method_named("extract"), 25),
        ("classify", session.method_named("classify"), 43),
    ];
    let mut rows = Vec::new();
    session.with_typer(|_, parsed, _, _, typer| {
        for (name, root, expected_tree) in roots {
            let typed = typer
                .type_method_definition(root)
                .unwrap_or_else(|error| panic!("root no longer reaches its post-change outcome: {error:?}"));
            assert_eq!(typer.source_typed_index().get(source, root), Some(typed));
            let span = parsed.ast.get(root).position.unwrap().span().range();
            rows.push(format!(
                "{name}\troot=tree{}\tspan={}:{}\tbefore=ExtractorPatternConstraintDeferred@tree{expected_tree}\tafter=complete=typed#tree{}",
                root.index(),
                span.start(),
                span.end(),
                typed.index()
            ));
        }
    });
    let actual = format!("{}\n", rows.join("\n"));
    let expected = include_str!(
        "fixtures/source-sealed-adt-match-3.9.0/rust-product1-first-error-movement.tsv"
    )
    .split_once('\n')
    .expect("first-error movement receipt header")
    .1;
    assert_eq!(actual, expected);
    for row in rows {
        println!("ISSUE1996_FIRST_ERROR_MOVEMENT\t{row}");
    }
}

#[test]
fn source_trait_product1_parent_is_authenticated_by_symbol_not_spelling() {
    let source = SourceId::from_index(5994);
    let text = "package adt\ntrait Event\ntrait Other\ncase class Created(id: Int) extends Other\n";
    let mut session = Session::new(text, source);
    let symbols = ["Event", "Other", "Created"].map(|expected| {
        session
            .parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if session
                        .store
                        .names
                        .resolve(definition.name.as_name().text())
                        == expected =>
                {
                    session.index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap()
    });
    session.with_typer(|_, _, index, _, typer| {
        let info = typer
            .complete_symbol(symbols[2])
            .expect("canonical source trait parent");
        let Some(Type::ClassInfo(info)) = typer.store().types.try_get(info) else {
            panic!("source class info")
        };
        let parents = info
            .parents
            .iter()
            .filter_map(|parent| match typer.store().types.try_get(*parent) {
                Some(Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                }) => Some(*symbol),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(parents.contains(&symbols[1]));
        assert!(!parents.contains(&symbols[0]));
        assert!(matches!(
            index.definition_of(symbols[1]),
            Some(dotty_core::SourceDefinition::Canonical { source: found, .. }) if found == source
        ));
    });
}

fn source_trait_product1_session(
    source: SourceId,
    selector_first: bool,
    source_text: &str,
) -> Vec<String> {
    let source_text = source_text.replace(
        "object Matcher:",
        "object Matcher:\n  def accept(event: Event): Int = 0",
    );
    let mut session = Session::new(&source_text, source);
    let symbols = ["Event", "Created", "Deleted"].map(|expected| {
        session
            .parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if session
                        .store
                        .names
                        .resolve(definition.name.as_name().text())
                        == expected =>
                {
                    session.index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing source symbol {expected}"))
    });
    let [event, created, deleted] = symbols;
    let mut summaries = Vec::new();
    session.with_typer(|_, parsed, index, definitions, typer| {
        assert_ne!(created, deleted, "variants retain distinct source identities");
        assert_eq!(typer.store().symbols.get(event).kind, SymbolKind::Trait);
        assert!(typer
            .store()
            .symbols
            .get(event)
            .flags
            .contains(dotty_core::SymbolFlags::SEALED));
        for class in [created, deleted] {
            let class_tree = match index.definition_of(class).unwrap() {
                dotty_core::SourceDefinition::Canonical { tree, .. } => tree,
                other => panic!("class lost source identity: {other:?}"),
            };
            let TreeKind::TypeDef(definition) = &parsed.ast.get(class_tree).kind else {
                panic!("class source root must remain a TypeDef")
            };
            let TreeKind::Template(template) = &parsed.ast.get(definition.rhs).kind else {
                panic!("class source body must remain a Template")
            };
            let TreeKind::DefDef(constructor) = &parsed.ast.get(template.constructor).kind else {
                panic!("case-class primary constructor must remain a DefDef")
            };
            let field_tree = constructor.value_param_clauses[0][0];
            let field = index.symbol_at(source, field_tree).unwrap();
            let selector = index.derived_symbol_at(class, source, field_tree).unwrap();
            if selector_first {
                typer
                    .complete_symbol(selector)
                    .expect("selector-first completion");
            }
            let class_info = typer
                .complete_symbol(class)
                .expect("complete source variant");
            if !selector_first {
                typer
                    .complete_symbol(selector)
                    .expect("class-first selector completion");
            }
            let Some(Type::ClassInfo(info)) = typer.store().types.try_get(class_info) else {
                panic!("variant must publish ClassInfo")
            };
            let parent_symbols = info
                .parents
                .iter()
                .filter_map(|parent| match typer.store().types.try_get(*parent) {
                    Some(Type::TypeRef {
                        target: TypeRefTarget::Symbol(symbol),
                        ..
                    }) => Some(*symbol),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                parent_symbols
                    .iter()
                    .filter(|symbol| **symbol == event)
                    .count(),
                1
            );
            let product = parent_symbols
                .iter()
                .copied()
                .filter(|symbol| {
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(*symbol).name.text())
                        == "Product"
                })
                .collect::<Vec<_>>();
            assert_eq!(product.len(), 1, "one canonical Product edge");
            assert_eq!(typer.store().symbols.get(product[0]).kind, SymbolKind::Trait);
            assert!(matches!(
                typer.store().symbols.get(product[0]).origin,
                SymbolOrigin::Tasty(_)
            ));
            let scala = typer.store().symbols.get(product[0]).owner.unwrap();
            assert_eq!(typer.store().symbols.get(scala).kind, SymbolKind::Package);
            assert_eq!(
                typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(scala).name.text()),
                "scala"
            );
            assert!(parent_symbols.contains(&definitions.object_class));
            let parent_names = parent_symbols
                .iter()
                .map(|symbol| {
                    typer
                        .store()
                        .names
                        .resolve(typer.store().symbols.get(*symbol).name.text())
                })
                .collect::<Vec<_>>()
                .join(",");
            assert_eq!(parent_names.as_str(), "Object,Event,Product");
            assert_eq!(typer.store().symbols.get(class).kind, SymbolKind::Class);
            assert_eq!(typer.store().symbols.get(class).origin, SymbolOrigin::Source(source));
            assert!(
                typer
                    .store()
                    .symbols
                    .get(class)
                    .flags
                    .contains(dotty_core::SymbolFlags::CASE)
            );
            assert_eq!(typer.store().symbols.get(field).owner, Some(class));
            assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
            assert_eq!(typer.store().symbols.get(selector).owner, Some(class));
            assert_ne!(field, selector, "source field and derived selector keep distinct IDs");
            let class_scope = index.scope_of(class).unwrap();
            let selector_name = typer.store().symbols.get(selector).name;
            assert_eq!(
                typer.store().scopes.get(class_scope).lookup_all(&selector_name),
                [selector],
                "the class exposes exactly its own _1 selector"
            );
            assert_eq!(
                index.derived_symbol_at(class, source, field_tree),
                Some(selector),
                "_1 remains derived from the original source constructor ValDef"
            );
            let selector_info = typer.complete_symbol(selector).unwrap();
            let Some(Type::Method(method)) = typer.store().types.try_get(selector_info) else {
                panic!("generated _1 must have method type")
            };
            assert!(method.params.is_empty());
            assert_eq!(method.result, definitions.int);
            assert_eq!(typer.complete_symbol(class).unwrap(), class_info);
            let unapply = index
                .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
                .unwrap();
            let apply = index
                .generated_case_class_method(class, GeneratedCaseClassMethodRole::Apply)
                .unwrap();
            assert_ne!(apply, unapply);
            let unapply_info = typer.complete_symbol(unapply).unwrap();
            let Some(Type::Method(unapply_type)) = typer.store().types.try_get(unapply_info) else {
                panic!("generated unapply must have a method type")
            };
            assert_eq!(unapply_type.params.len(), 1);
            assert!(matches!(
                typer.store().types.try_get(unapply_type.params[0].ty),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class
            ));
            let child_type = unapply_type.result;
            assert!(matches!(
                typer.store().types.try_get(child_type),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class
            ));
            let companion = typer.store().symbols.get(class).links.companion.unwrap();
            assert_eq!(typer.store().symbols.get(companion).kind, SymbolKind::Object);
            assert_eq!(typer.store().symbols.get(companion).links.companion, Some(class));
            let companion_class = typer.store().symbols.get(apply).owner.unwrap();
            assert_eq!(typer.store().symbols.get(companion_class).kind, SymbolKind::ModuleClass);
            assert_eq!(typer.store().symbols.get(unapply).owner, Some(companion_class));
            let apply_info = typer.complete_symbol(apply).unwrap();
            let Some(Type::Method(apply_type)) = typer.store().types.try_get(apply_info) else {
                panic!("generated apply must have a method type")
            };
            assert_eq!(apply_type.params.len(), 1);
            assert_eq!(apply_type.params[0].ty, definitions.int);
            assert!(matches!(
                typer.store().types.try_get(apply_type.result),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class
            ));
            let accept = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| match &node.kind {
                    TreeKind::DefDef(definition)
                        if typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text())
                            == "accept" =>
                    {
                        index.symbol_at(source, tree)
                    }
                    _ => None,
                })
                .unwrap();
            let accept_info = typer.complete_symbol(accept).unwrap();
            let Some(Type::Method(accept_type)) = typer.store().types.try_get(accept_info) else {
                panic!("accept must have a method type")
            };
            let parent_type = accept_type.params[0].ty;
            let parent_relation = typer.is_subtype(child_type, parent_type);
            assert!(parent_relation.expect("nominal parent relation"));
            assert!(!typer
                .is_subtype(parent_type, child_type)
                .expect("reverse nominal relation"));
            let class_name = typer
                .store()
                .names
                .resolve(typer.store().symbols.get(class).name.text());
            let row = format!(
                "{class_name}|Class(CASE,source)|parents={parent_names}|_1:Int(source-field-derived)|apply=(Int):{class_name}|unapply=({class_name}):{class_name}|{class_name}<:Event=true|Event<:{class_name}=false"
            );
            eprintln!("ISSUE1994_CLASS\t{row}");
            summaries.push(row);
        }
    });
    summaries.sort();
    summaries
}

#[test]
fn frozen_source_case_class_match_methods_type_as_whole_defdefs() {
    let first = positive_session(SOURCE, SourceId::from_index(1933), Some("first"));
    let second = positive_session(SOURCE, SourceId::from_index(2933), Some("second"));
    assert_eq!(first, second, "fresh sessions normalize identically");
    assert_eq!(
        first,
        [
            "unwrap|Positive.scala#tree18|64:136|matching.Box|matching.Matcher|(Box):Int|scala.Product|_1:Int@Box|Box$.unapply|Bind(value:Int)",
            "classify|Positive.scala#tree36|140:237|matching.Box|matching.Matcher|(Box):Int|scala.Product|_1:Int@Box|Box$.unapply|Literal(0:Int),Bind(value:Int)",
        ]
    );

    let (package, declarations) = SOURCE
        .strip_prefix("package matching\n\n")
        .expect("frozen package header")
        .split_once("\n\nobject Matcher:")
        .expect("class before Matcher in frozen source");
    let reordered = format!("package matching\n\nobject Matcher:{declarations}\n\n{package}");
    assert_eq!(
        positive_session(&reordered, SourceId::from_index(3933), None),
        first,
        "reordered declarations preserve canonical extractor and selector identities"
    );
}

#[test]
fn flagged_product2_whole_method_baseline() {
    let first_source = SourceId::from_index(1970);
    let second_source = SourceId::from_index(2970);
    assert_ne!(
        first_source, second_source,
        "each receipt uses an independent source session"
    );
    let first = flagged_positive_session(first_source);
    let second = flagged_positive_session(second_source);
    assert_eq!(
        first, second,
        "two independent source sessions normalize identically"
    );
    assert_eq!(
        first.len(),
        2,
        "only select and classify count as positive roots"
    );
    for (session, rows) in [("first", &first), ("second", &second)] {
        for row in rows {
            println!("ISSUE1972_POSITIVE\t{session}\t{row}");
        }
    }
}

#[test]
fn sealed_event_extract_and_classify_are_complete_owned_methods() {
    let first = sealed_adt_positive_session(SourceId::from_index(1998));
    let second = sealed_adt_positive_session(SourceId::from_index(2998));
    assert_eq!(first, second, "fresh source sessions normalize identically");
    assert_eq!(first.len(), 2, "only the two original methods are roots");
    assert_eq!(first[0].0, "extract");
    assert_eq!(first[1].0, "classify");
    let actual = first
        .iter()
        .map(|(name, receipt)| {
            let first_error = if name == "extract" {
                "ExtractorPatternConstraintDeferred@tree25:210:221"
            } else {
                "ExtractorPatternConstraintDeferred@tree43:327:337"
            };
            format!("{name}\t{first_error}\t{receipt}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        format!("{actual}\n"),
        include_str!("fixtures/source-sealed-adt-match-3.9.0/rust-complete-methods.tsv")
            .split_once('\n')
            .expect("two-root before/after receipt header")
            .1
    );
    for (name, receipt) in first {
        println!("ISSUE1998_COMPLETE_ROOT\t{name}\t{receipt}");
    }
}

fn sealed_adt_positive_session(source: SourceId) -> Vec<(String, String)> {
    let mut session = Session::new(SOURCE_TRAIT_PRODUCT1, source);
    let event = session.source_type_symbol("Event");
    let created = session.source_type_symbol("Created");
    let deleted = session.source_type_symbol("Deleted");
    let methods = ["extract", "classify"].map(|name| (name, session.method_named(name)));
    let selector_name = Name::new(session.store.names.intern("_1"), Namespace::Term);
    session.with_typer(|source, parsed, index, definitions, typer| {
        assert_eq!(typer.store().symbols.get(event).kind, SymbolKind::Trait);
        assert!(typer.store().symbols.get(event).flags.contains(SymbolFlags::SEALED));
        assert!(matches!(typer.store().symbols.get(event).origin, SymbolOrigin::Source(id) if id == source));
        assert_ne!(created, deleted);
        for variant in [created, deleted] {
            let class_info = typer.complete_symbol(variant).expect("complete source variant");
            let Some(Type::ClassInfo(class_info)) = typer.store().types.try_get(class_info) else {
                panic!("source variant has complete ClassInfo")
            };
            assert!(class_info.parents.iter().any(|parent| matches!(
                typer.store().types.try_get(*parent),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == event
            )), "variant retains its direct source Event parent");
            let companion = typer.store().symbols.get(variant).links.companion.unwrap();
            let unapply = index.generated_case_class_method(variant, GeneratedCaseClassMethodRole::Unapply).unwrap();
            let unapply_owner = typer.store().symbols.get(unapply).owner.unwrap();
            assert_eq!(typer.store().symbols.get(companion).kind, SymbolKind::Object);
            assert_eq!(typer.store().symbols.get(unapply_owner).kind, SymbolKind::ModuleClass);
            let unapply_type = typer.complete_symbol(unapply).expect("complete canonical unapply");
            let Some(Type::Method(unapply_type)) = typer.store().types.try_get(unapply_type) else {
                panic!("generated unapply is a method")
            };
            assert_eq!(unapply_type.params.len(), 1);
            for ty in [unapply_type.params[0].ty, unapply_type.result] {
                assert!(matches!(
                    typer.store().types.try_get(ty),
                    Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == variant
                ), "unapply input and result preserve their authentic variant identity");
            }
            let class_scope = index.scope_of(variant).unwrap();
            let selector = typer.store().scopes.get(class_scope).lookup(&selector_name).unwrap();
            assert_eq!(typer.store().symbols.get(selector).owner, Some(variant));
            let selector_type = typer.complete_symbol(selector).expect("complete source Product._1 selector");
            let Some(Type::Method(selector_type)) = typer.store().types.try_get(selector_type) else {
                panic!("variant _1 selector is a method")
            };
            assert!(selector_type.params.is_empty());
            assert_eq!(selector_type.result, definitions.int);
        }
        let mut receipts = Vec::new();
        let mut binders = Vec::new();
        let mut typed_roots = Vec::new();

        for (name, method) in methods {
            let method_symbol = index.symbol_at(source, method).unwrap();
            let matcher = typer.store().symbols.get(method_symbol).owner.unwrap();
            assert_eq!(typer.store().symbols.get(matcher).kind, SymbolKind::ModuleClass);
            assert_eq!(typer.store().names.resolve(typer.store().symbols.get(matcher).name.text()), "Matcher$");
            let typed_method = typer.type_method_definition(method).unwrap_or_else(|error| {
                panic!("frozen {name} method must type completely after the prerequisites: {error:?}")
            });
            typed_roots.push((method, typed_method));
            assert_mapping(typer.source_typed_index(), source, method);
            let source_def = match &parsed.ast.get(method).kind {
                TreeKind::DefDef(definition) => definition,
                other => panic!("source root is not a DefDef: {other:?}"),
            };
            let method_range = parsed.ast.get(method).position.unwrap().span().range();
            let (expected_tree, expected_range) = if name == "extract" {
                (36, (150, 262))
            } else {
                (60, (266, 400))
            };
            assert_eq!(method.index(), expected_tree, "frozen original DefDef identity");
            assert_eq!((method_range.start(), method_range.end()), expected_range, "frozen original source span");
            let TreeKind::DefDef(typed_def) = &typer.typed_ast().get(typed_method).kind else {
                panic!("typed root is not a DefDef")
            };
            assert_eq!(typed_def.value_param_clauses.len(), 1);
            assert_eq!(typed_def.value_param_clauses[0].len(), 1);
            let source_parameter = source_def.value_param_clauses[0][0];
            let TreeKind::ValDef(source_parameter_def) = &parsed.ast.get(source_parameter).kind else {
                panic!("source method parameter is a ValDef")
            };
            assert_mapping(typer.source_typed_index(), source, source_parameter);
            let typed_parameter = typed_def.value_param_clauses[0][0];
            assert_eq!(
                typer.source_typed_index().get(source, source_parameter),
                Some(typed_parameter),
                "the returned DefDef parameter is the mapped source parameter"
            );
            let TreeKind::ValDef(typed_parameter_def) = &typer.typed_ast().get(typed_parameter).kind else {
                panic!("typed method parameter is a ValDef")
            };
            let selector_parameter = index.symbol_at(source, source_parameter).unwrap();
            assert!(matches!(
                typer.store().types.try_get(typer.typed_ast().get(typed_parameter).ty),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == event
            ));
            assert_eq!(typer.store().symbols.get(selector_parameter).kind, SymbolKind::Parameter);
            assert_eq!(typer.store().symbols.get(selector_parameter).owner, Some(method_symbol));
            assert_eq!(
                typer.store().names.resolve(typed_parameter_def.name.as_name().text()),
                typer.store().names.resolve(source_parameter_def.name.as_name().text())
            );
            assert_mapping(typer.source_typed_index(), source, source_parameter_def.tpt);
            assert_eq!(
                typer.source_typed_index().get(source, source_parameter_def.tpt),
                Some(typed_parameter_def.tpt)
            );
            assert!(matches!(
                typer.store().types.try_get(typer.typed_ast().get(typed_parameter_def.tpt).ty),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == event
            ));
            let signature = match *typer.store().symbols.info(method_symbol) {
                SymbolInfo::Complete(signature) => signature,
                other => panic!("{name} signature is incomplete: {other:?}"),
            };
            let Some(Type::Method(method_type)) = typer.store().types.try_get(signature) else {
                panic!("{name} signature is not a method type")
            };
            assert_eq!(method_type.params.len(), 1);
            assert!(matches!(
                typer.store().types.try_get(method_type.params[0].ty),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == event
            ));
            assert_eq!(method_type.result, definitions.int);
            assert_mapping(typer.source_typed_index(), source, source_def.tpt);
            assert_eq!(typer.source_typed_index().get(source, source_def.tpt), Some(typed_def.tpt));
            assert_eq!(typer.typed_ast().get(typed_def.tpt).ty, definitions.int);
            let source_rhs = source_def.rhs.unwrap();
            let typed_rhs = typed_def.rhs.expect("complete typed method body");
            assert_mapping(typer.source_typed_index(), source, source_rhs);
            assert_eq!(typer.source_typed_index().get(source, source_rhs), Some(typed_rhs));
            assert_eq!(typer.typed_ast().get(typed_rhs).ty, definitions.int);
            let (selector, source_cases) = match &parsed.ast.get(source_rhs).kind {
                TreeKind::Match(matched) => (matched.selector, matched.cases.clone()),
                other => panic!("{name} source body is not a Match: {other:?}"),
            };
            let rhs_range = parsed.ast.get(source_rhs).position.unwrap().span().range();
            for (tree, node) in parsed.ast.iter() {
                let Some(position) = node.position else { continue };
                let range = position.span().range();
                if range.start() >= rhs_range.start() && range.end() <= rhs_range.end() {
                    assert_mapping(typer.source_typed_index(), source, tree);
                }
            }
            let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_rhs).kind else {
                panic!("{name} typed body is not a Match")
            };
            assert_eq!(source_cases.len(), if name == "extract" { 2 } else { 3 });
            assert_eq!(typed_match.cases.len(), source_cases.len());
            assert_mapping(typer.source_typed_index(), source, selector);
            let typed_selector = typed_match.selector;
            assert_eq!(
                typer.source_typed_index().get(source, selector),
                Some(typed_selector),
                "the returned Match selector is the mapped source selector"
            );
            assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_selector), selector_parameter);
            assert!(matches!(
                typer.store().types.try_get(typer.typed_ast().get(typed_selector).ty),
                Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == selector_parameter
            ));
            let SymbolInfo::Complete(selector_type) = *typer.store().symbols.info(selector_parameter) else {
                panic!("{name} selector parameter type is complete")
            };
            assert!(matches!(
                typer.store().types.try_get(selector_type),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == event
            ));

            let mut case_receipts = Vec::new();
            for (case_index, (source_case, typed_case)) in source_cases.iter().zip(&typed_match.cases).enumerate() {
                assert_mapping(typer.source_typed_index(), source, *source_case);
                assert_eq!(typer.source_typed_index().get(source, *source_case), Some(*typed_case));
                let TreeKind::CaseDef(source_case_node) = &parsed.ast.get(*source_case).kind else {
                    panic!("source branch is not a CaseDef")
                };
                let TreeKind::CaseDef(typed_case_node) = &typer.typed_ast().get(*typed_case).kind else {
                    panic!("typed branch is not a CaseDef")
                };
                assert_mapping(typer.source_typed_index(), source, source_case_node.pattern);
                assert_eq!(typer.source_typed_index().get(source, source_case_node.pattern), Some(typed_case_node.pattern));
                assert_mapping(typer.source_typed_index(), source, source_case_node.body);
                let mapped_body = typer.source_typed_index().get(source, source_case_node.body).unwrap();
                assert_eq!(
                    typed_case_node.body,
                    mapped_body,
                    "the returned CaseDef body is the mapped source body"
                );
                let typed_body = typed_case_node.body;
                let (variant, expected_binder) = match (name, case_index) {
                    ("extract", 0) | ("classify", 0 | 1) => (created, "id"),
                    ("extract", 1) => (deleted, "code"),
                    ("classify", 2) => (deleted, "_"),
                    _ => panic!("unexpected source case {name}[{case_index}]"),
                };
                let TreeKind::UnApply(unapply_node) = &typer.typed_ast().get(typed_case_node.pattern).kind else {
                    panic!("{name} case must retain an UnApply")
                };
                assert_mapping(typer.source_typed_index(), source, source_case_node.pattern);
                let TreeKind::Apply(source_extractor) = &parsed.ast.get(source_case_node.pattern).kind else {
                    panic!("frozen source extractor pattern remains an Apply")
                };
                assert_eq!(source_extractor.args.len(), 1);
                let expected_unapply = index.generated_case_class_method(variant, GeneratedCaseClassMethodRole::Unapply).unwrap();
                assert_eq!(
                    typer.source_typed_index().get(source, source_extractor.function),
                    Some(match &typer.typed_ast().get(unapply_node.function).kind {
                        TreeKind::Select(selection) => selection.qualifier,
                        other => panic!("typed extractor function is not selected: {other:?}"),
                    }),
                    "each source extractor name maps to its authentic companion qualifier"
                );
                assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), unapply_node.function), expected_unapply);
                assert_eq!(index.generated_case_class_method_identity(expected_unapply), Some(dotty_core::GeneratedCaseClassMethod { class: variant, role: GeneratedCaseClassMethodRole::Unapply }));
                let unapply_owner = typer.store().symbols.get(expected_unapply).owner.unwrap();
                assert_eq!(typer.store().symbols.get(unapply_owner).kind, SymbolKind::ModuleClass);
                assert_eq!(typer.store().names.resolve(typer.store().symbols.get(unapply_owner).name.text()), if variant == created { "Created$" } else { "Deleted$" });
                let TreeKind::Select(selected) = &typer.typed_ast().get(unapply_node.function).kind else {
                    panic!("UnApply function must be the authentic selected extractor")
                };
                let companion = typer.store().symbols.get(variant).links.companion.unwrap();
                assert!(matches!(
                    typer.store().types.try_get(typer.typed_ast().get(selected.qualifier).ty),
                    Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == companion
                ));
                assert_eq!(unapply_node.patterns.len(), 1);
                let nested = unapply_node.patterns[0];
                assert_eq!(typer.source_typed_index().get(source, source_extractor.args[0]), Some(nested));
                assert!(matches!(
                    typer.store().types.try_get(typer.typed_ast().get(typed_case_node.pattern).ty),
                    Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == variant
                ), "the pattern prototype is narrowed to its authentic source variant");
                if expected_binder == "_" {
                    let TreeKind::Ident(wildcard) = &typer.typed_ast().get(nested).kind else {
                        panic!("wildcard remains an identifier-shaped pattern")
                    };
                    assert_eq!(typer.store().names.resolve(wildcard.name.text()), "_");
                    assert_eq!(typer.typed_ast().get(nested).ty, definitions.int);
                    assert!(matches!(
                        typer.store().types.try_get(typer.typed_ast().get(typed_case_node.body).ty),
                        Some(Type::Constant(Constant::Int(0)))
                    ));
                    case_receipts.push("Deleted(_)");
                } else if name == "classify" && case_index == 0 {
                    assert!(matches!(typer.typed_ast().get(nested).kind, TreeKind::Literal(_)));
                    assert!(matches!(
                        typer.store().types.try_get(typer.typed_ast().get(nested).ty),
                        Some(Type::Constant(Constant::Int(0)))
                    ));
                    assert!(matches!(
                        typer.store().types.try_get(typer.typed_ast().get(typed_case_node.body).ty),
                        Some(Type::Constant(Constant::Int(0)))
                    ));
                    case_receipts.push("Created(0)");
                } else {
                    let TreeKind::Bind(binding) = &typer.typed_ast().get(nested).kind else {
                        panic!("nested extractor argument must be a binder")
                    };
                    let binder = typed_symbol(typer.typed_ast(), typer.store(), nested);
                    assert_eq!(typer.store().symbols.get(binder).kind, SymbolKind::Local);
                    assert_eq!(typer.store().symbols.get(binder).owner, Some(method_symbol));
                    assert_eq!(typer.store().symbols.get(binder).origin, SymbolOrigin::Source(source));
                    assert_eq!(typer.store().names.resolve(binding.name.text()), expected_binder);
                    assert_eq!(*typer.store().symbols.info(binder), SymbolInfo::Complete(definitions.int));
                    assert!(matches!(typer.typed_ast().get(binding.body).kind, TreeKind::Ident(_)));
                    assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_body), binder);
                    assert_ne!(binder, selector_parameter);
                    binders.push(binder);
                    case_receipts.push(if variant == created { "Created(id)" } else { "Deleted(code)" });
                }
            }
            assert_eq!(
                case_receipts,
                if name == "extract" { vec!["Created(id)", "Deleted(code)"] } else { vec!["Created(0)", "Created(id)", "Deleted(_)"] }
            );
            let (extractors, binders) = if name == "extract" {
                ("Created$.unapply,Deleted$.unapply", "Created:id:Int,Deleted:code:Int")
            } else {
                ("Created$.unapply,Created$.unapply,Deleted$.unapply", "Created:id:Int")
            };
            receipts.push((name.to_owned(), format!(
                "Event.scala#tree{}|{}:{}|signature=(Event):Int|owner=Matcher$|selector=Event|cases={}|extractors={extractors}|binders={binders}|typed=complete DefDef+Match|mappings=all source RHS trees",
                method.index(), method_range.start(), method_range.end(), case_receipts.join(",")
            )));
        }
        assert_eq!(binders.len(), 3);
        for (index, binder) in binders.iter().enumerate() {
            assert!(!binders[..index].contains(binder), "each case receives a distinct binder symbol");
        }
        for (source_root, typed_root) in typed_roots {
            assert_eq!(typer.source_typed_index().get(source, source_root), Some(typed_root));
            assert!(matches!(typer.typed_ast().get(typed_root).kind, TreeKind::DefDef(_)));
        }
        receipts
    })
}

#[test]
fn sealed_event_case_local_negative_matrix_rejects_whole_methods() {
    let unrelated_extractor = SOURCE_TRAIT_PRODUCT1
        .replace(
            "object Matcher:",
            "object Text:\n  def unapply(value: String): Option[String] = None\n\nobject Matcher:",
        )
        .replace(
            "      case Created(id)   => id",
            "      case Text(name)   => 0\n      case Created(id)   => id",
        );
    let unrelated_same_spelling = SOURCE_TRAIT_PRODUCT1
        .replace(
            "object Matcher:",
            "object Foreign:\n  final case class Created(id: Int)\n\nobject Matcher:",
        )
        .replace(
            "      case Created(id)   => id",
            "      case Foreign.Created(id)   => id",
        );
    let non_event_variant = SOURCE_TRAIT_PRODUCT1
        .replace("sealed trait Event", "sealed trait Event\ntrait Other")
        .replace("final case class Deleted(code: Int) extends Event", "final case class Deleted(code: Int) extends Other")
        .replace(
            "      case Deleted(_)  => 0",
            "      case Deleted(_)  => 0\n\n  def valid(event: Event): Int =\n    event match\n      case Created(_) => 1\n      case _ => 0",
        );
    let cases = [
        (
            "created_bool_literal",
            SOURCE_TRAIT_PRODUCT1.replace("case Created(0)", "case Created(true)"),
            "classify",
            "PatternTypeMismatch",
            "rejected",
        ),
        (
            "deleted_bool_literal",
            SOURCE_TRAIT_PRODUCT1.replace("case Deleted(_)  => 0", "case Deleted(true)  => 0"),
            "classify",
            "PatternTypeMismatch",
            "rejected",
        ),
        (
            "created_zero_arity",
            SOURCE_TRAIT_PRODUCT1.replace("case Created(id)   => id", "case Created()   => 0"),
            "extract",
            "ExtractorPatternArityUnsupported",
            "rejected",
        ),
        (
            "deleted_two_arity",
            SOURCE_TRAIT_PRODUCT1.replace(
                "case Deleted(code) => code",
                "case Deleted(left, right) => left",
            ),
            "extract",
            "ExtractorProductSelectorCountMismatch",
            "rejected",
        ),
        (
            "wrong_declared_result_after_cases",
            SOURCE_TRAIT_PRODUCT1.replace(
                "def extract(event: Event): Int",
                "def extract(event: Event): Boolean",
            ),
            "extract",
            "ExpectedExpressionTypeMismatch",
            "rejected",
        ),
        (
            "cross_case_binder_escape",
            SOURCE_TRAIT_PRODUCT1.replace("case Deleted(code) => code", "case Deleted(code) => id"),
            "extract",
            "TermNameNotFound",
            "rejected",
        ),
        (
            "unrelated_string_extractor",
            unrelated_extractor,
            "extract",
            "ExtractorPatternConstraintDeferred",
            "rejected",
        ),
        (
            "same_spelling_wrong_unapply_input",
            unrelated_same_spelling,
            "extract",
            "ExtractorUnapplyNotFound",
            "rejected",
        ),
        (
            "non_event_variant",
            non_event_variant,
            "extract",
            "ExtractorPatternConstraintDeferred",
            "rejected",
        ),
    ];

    let mut matrix_receipts = Vec::new();
    for (offset, (label, source_text, method_name, expected_error, scala_status)) in
        cases.into_iter().enumerate()
    {
        assert_ne!(
            source_text, SOURCE_TRAIT_PRODUCT1,
            "{label} mutation must apply"
        );
        let source = SourceId::from_index(19_990 + offset as u32);
        let mut session = Session::new(&source_text, source);
        let failed_method = session.method_named(method_name);
        let recovery_method = session.method_named(if label == "non_event_variant" {
            "valid"
        } else if method_name == "extract" {
            "classify"
        } else {
            "extract"
        });
        let rhs = match &session.parsed.ast.get(failed_method).kind {
            TreeKind::DefDef(definition) => definition.rhs.unwrap(),
            _ => panic!("failed root is not a DefDef"),
        };
        let match_tree = session
            .parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                (tree == rhs && matches!(node.kind, TreeKind::Match(_))).then_some(tree)
            })
            .expect("frozen root body is one Match");
        let root_range = session
            .parsed
            .ast
            .get(failed_method)
            .position
            .unwrap()
            .span()
            .range();
        let match_range = session
            .parsed
            .ast
            .get(match_tree)
            .position
            .unwrap()
            .span()
            .range();
        session.with_typer(|source, parsed, index, _definitions, typer| {
            let prior_typed = typer
                .type_method_definition(recovery_method)
                .expect("the other frozen method remains a valid whole-method root");
            let before = event_method_failure_projection(typer, source, parsed, index);
            let first = typer
                .type_method_definition(failed_method)
                .expect_err("the malformed whole method is rejected");
            let diagnostic = format!("{first:?}");
            assert!(diagnostic.starts_with(expected_error), "{label}: {diagnostic}");
            let diagnostic_tree = diagnostic
                .split_once("tree_index: ")
                .and_then(|(_, suffix)| suffix.split_once([',', '}']))
                .and_then(|(index, _)| index.parse::<u32>().ok())
                .expect("typer diagnostic includes its source tree index");
            let diagnostic_range = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| {
                    (tree.index() == diagnostic_tree)
                        .then(|| node.position.map(|position| position.span().range()))
                        .flatten()
                })
                .expect("first diagnostic source tree has a span");
            let after_first = event_method_failure_projection(typer, source, parsed, index);
            let changed = event_method_failure_projection_changes(&before, &after_first);
            assert!(changed.is_empty(), "{label}: changed {changed:?} after failure rollback");
            assert_eq!(typer.source_typed_index().get(source, failed_method), None);
            assert_eq!(
                typer.source_typed_index().get(source, recovery_method),
                Some(prior_typed),
                "{label}: a previously successful sibling remains published"
            );
            let retry = typer
                .type_method_definition(failed_method)
                .expect_err("retry remains rejected");
            assert_eq!(format!("{retry:?}"), diagnostic, "{label}: retry is stable");
            let after_retry = event_method_failure_projection(typer, source, parsed, index);
            let changed = event_method_failure_projection_changes(&before, &after_retry);
            assert!(changed.is_empty(), "{label}: changed {changed:?} before retry rollback");
            let recovered = typer
                .type_method_definition(recovery_method)
                .expect("same-session positive root remains typeable after failure");
            assert_eq!(recovered, prior_typed);
            let same_session_identity =
                event_recovery_identity(typer, source, parsed, index, recovery_method);

            let fresh_source = SourceId::from_index(29_990 + offset as u32);
            let mut fresh = Session::new(&source_text, fresh_source);
            let fresh_failed_method = fresh.method_named(method_name);
            let fresh_recovery_method = fresh.method_named(if label == "non_event_variant" {
                "valid"
            } else if method_name == "extract" {
                "classify"
            } else {
                "extract"
            });
            fresh.with_typer(|fresh_source, fresh_parsed, fresh_index, _, fresh_typer| {
                let fresh_error = fresh_typer
                    .type_method_definition(fresh_failed_method)
                    .expect_err("fresh session rejects the same whole method");
                let fresh_diagnostic = format!("{fresh_error:?}");
                assert!(
                    fresh_diagnostic.starts_with(expected_error),
                    "{label}: fresh-session diagnostic {fresh_diagnostic}"
                );
                let fresh_tree = fresh_diagnostic
                    .split_once("tree_index: ")
                    .and_then(|(_, suffix)| suffix.split_once([',', '}']))
                    .and_then(|(tree, _)| tree.parse::<u32>().ok())
                    .expect("fresh-session diagnostic has source tree index");
                assert_eq!(fresh_tree, diagnostic_tree, "{label}: normalized error tree");
                let fresh_range = fresh_parsed
                    .ast
                    .iter()
                    .find_map(|(tree, node)| {
                        (tree.index() == fresh_tree)
                            .then(|| node.position.map(|position| position.span().range()))
                            .flatten()
                    })
                    .expect("fresh-session diagnostic source tree has span");
                assert_eq!(fresh_range, diagnostic_range, "{label}: normalized diagnostic span");
                let fresh_recovered = fresh_typer
                    .type_method_definition(fresh_recovery_method)
                    .expect("fresh session can type the valid sibling after failure");
                assert_eq!(
                    event_recovery_identity(
                        fresh_typer,
                        fresh_source,
                        fresh_parsed,
                        fresh_index,
                        fresh_recovery_method,
                    ),
                    same_session_identity,
                    "{label}: successful semantic identity matches after recovery"
                );
                assert!(fresh_typer
                    .source_typed_index()
                    .get(fresh_source, fresh_recovery_method)
                    .is_some());
                let _ = fresh_recovered;
            });
            matrix_receipts.push(format!(
                "{label}\tEvent.scala#tree{}@{}..{}\tMatch#tree{}@{}..{}\tErrorTree#tree{diagnostic_tree}@{}..{}\t{expected_error}\tScala:{scala_status}\t{diagnostic}",
                failed_method.index(),
                root_range.start(),
                root_range.end(),
                match_tree.index(),
                match_range.start(),
                match_range.end(),
                diagnostic_range.start(),
                diagnostic_range.end(),
            ));
        });
    }
    assert_eq!(
        format!(
            "case\tmethod root\tMatch\tfirst error tree\tTyper diagnostic\tScala\tTyper detail\n{}\n",
            matrix_receipts.join("\n")
        ),
        include_str!("fixtures/source-sealed-adt-match-3.9.0/rust-failure-matrix.tsv")
    );
}

fn event_recovery_identity(
    typer: &SourceTyper<'_>,
    source: SourceId,
    parsed: &dotty_parser::ParseResult,
    index: &dotty_core::SourceSemanticIndex,
    method: TreeId<Untyped>,
) -> String {
    let typed_root = typer
        .source_typed_index()
        .get(source, method)
        .expect("recovered method root is published");
    let method_symbol = index.symbol_at(source, method).expect("method symbol");
    let signature = match *typer.store().symbols.info(method_symbol) {
        SymbolInfo::Complete(signature) => signature,
        other => panic!("recovered method has complete signature: {other:?}"),
    };
    let Type::Method(signature) = typer.store().types.get(signature) else {
        panic!("recovered method has MethodType signature")
    };
    let source_definition = match &parsed.ast.get(method).kind {
        TreeKind::DefDef(definition) => definition,
        _ => panic!("recovered source root is DefDef"),
    };
    let typed_definition = match &typer.typed_ast().get(typed_root).kind {
        TreeKind::DefDef(definition) => definition,
        _ => panic!("recovered typed root is DefDef"),
    };
    let source_rhs = source_definition.rhs.expect("recovered method has RHS");
    let typed_rhs = typed_definition.rhs.expect("typed method has RHS");
    let (TreeKind::Match(source_match), TreeKind::Match(typed_match)) = (
        &parsed.ast.get(source_rhs).kind,
        &typer.typed_ast().get(typed_rhs).kind,
    ) else {
        panic!("recovered method body is Match")
    };
    let mut cases = Vec::new();
    for (source_case, typed_case) in source_match.cases.iter().zip(&typed_match.cases) {
        let TreeKind::CaseDef(source_case) = &parsed.ast.get(*source_case).kind else {
            panic!("recovered source branch is CaseDef")
        };
        let TreeKind::CaseDef(typed_case) = &typer.typed_ast().get(*typed_case).kind else {
            panic!("recovered typed branch is CaseDef")
        };
        match &typer.typed_ast().get(typed_case.pattern).kind {
            TreeKind::UnApply(unapply) => {
                let extractor = typed_symbol(typer.typed_ast(), typer.store(), unapply.function);
                let extractor_owner = typer.store().symbols.get(extractor).owner.unwrap();
                let extractor_owner_name = typer
                    .store()
                    .names
                    .resolve(typer.store().symbols.get(extractor_owner).name.text());
                let binders = unapply
                    .patterns
                    .iter()
                    .filter_map(|pattern| match &typer.typed_ast().get(*pattern).kind {
                        TreeKind::Bind(binding) => {
                            let symbol = typed_symbol(typer.typed_ast(), typer.store(), *pattern);
                            let ty = match *typer.store().symbols.info(symbol) {
                                SymbolInfo::Complete(ty) => type_display_name(typer, ty),
                                other => panic!("case binder has complete type: {other:?}"),
                            };
                            Some(format!(
                                "{}:{ty}",
                                typer.store().names.resolve(binding.name.text())
                            ))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                cases.push(format!("{extractor_owner_name}({})", binders.join(",")));
            }
            TreeKind::Ident(ident) => {
                cases.push(typer.store().names.resolve(ident.name.text()).to_owned())
            }
            other => panic!("recovered pattern is supported UnApply or Ident: {other:?}"),
        }
        assert!(
            typer
                .source_typed_index()
                .get(source, source_case.pattern)
                .is_some()
        );
    }
    let param_types = signature
        .params
        .iter()
        .map(|parameter| type_display_name(typer, parameter.ty))
        .collect::<Vec<_>>();
    format!(
        "owner={};params={param_types:?};result={};cases={cases:?};mapped={}",
        typer.store().names.resolve(
            typer
                .store()
                .symbols
                .get(typer.store().symbols.get(method_symbol).owner.unwrap())
                .name
                .text()
        ),
        type_display_name(typer, signature.result),
        source_match.cases.len(),
    )
}

fn type_display_name(typer: &SourceTyper<'_>, ty: dotty_core::TypeId) -> String {
    match typer.store().types.try_get(ty) {
        Some(Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }) => typer
            .store()
            .names
            .resolve(typer.store().symbols.get(*symbol).name.text())
            .to_owned(),
        Some(other) => format!("{:?}", std::mem::discriminant(other)),
        None => "missing".to_owned(),
    }
}

fn event_method_failure_projection(
    typer: &SourceTyper<'_>,
    source: SourceId,
    parsed: &dotty_parser::ParseResult,
    index: &dotty_core::SourceSemanticIndex,
) -> Vec<(&'static str, String)> {
    let mappings = parsed
        .ast
        .iter()
        .map(|(tree, _)| (tree.index(), typer.source_typed_index().get(source, tree)))
        .collect::<Vec<_>>();
    let binders = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            matches!(node.kind, TreeKind::Bind(_))
                .then_some((tree.index(), typer.pattern_binding_symbol_at(source, tree)))
        })
        .collect::<Vec<_>>();
    let typed_nodes = typer
        .typed_ast()
        .iter()
        .map(|(tree, node)| (tree.index(), node.clone()))
        .collect::<Vec<_>>();
    let mut scopes = parsed
        .ast
        .iter()
        .filter_map(|(tree, _)| index.symbol_at(source, tree))
        .filter_map(|symbol| index.scope_of(symbol).map(|scope| (symbol, scope)))
        .map(|(symbol, scope)| {
            let mut members = typer
                .store()
                .scopes
                .get(scope)
                .entered_symbols()
                .map(|member| member.index())
                .collect::<Vec<_>>();
            members.sort_unstable();
            format!(
                "symbol={};scope={};members={members:?}",
                symbol.index(),
                scope.index()
            )
        })
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    vec![
        ("store", format!("{:?}", typer.store().checkpoint())),
        ("symbols", format!("{:?}", typer.store().symbols)),
        ("types", format!("{:?}", typer.store().types)),
        ("source scopes", format!("{scopes:?}")),
        (
            "typed checkpoint",
            format!("{:?}", typer.typed_ast().checkpoint()),
        ),
        ("typed nodes", format!("{typed_nodes:?}")),
        (
            "source typed index length",
            format!("{}", typer.source_typed_index().len()),
        ),
        ("source typed mappings", format!("{mappings:?}")),
        ("case-local binders", format!("{binders:?}")),
    ]
}

fn event_method_failure_projection_changes(
    before: &[(&'static str, String)],
    after: &[(&'static str, String)],
) -> Vec<&'static str> {
    before
        .iter()
        .zip(after)
        .filter_map(|((name, before), (_, after))| (before != after).then_some(*name))
        .collect()
}

#[test]
fn sealed_event_rejects_variant_extractor_with_a_foreign_owner() {
    let mut session = Session::new(SOURCE_TRAIT_PRODUCT1, SourceId::from_index(3998));
    let created = session.source_type_symbol("Created");
    let deleted = session.source_type_symbol("Deleted");
    let method = session.method_named("extract");
    let created_unapply = session
        .index
        .generated_case_class_method(created, GeneratedCaseClassMethodRole::Unapply)
        .unwrap();
    let deleted_unapply = session
        .index
        .generated_case_class_method(deleted, GeneratedCaseClassMethodRole::Unapply)
        .unwrap();
    let foreign_owner = session.store.symbols.get(deleted_unapply).owner.unwrap();
    session.store.symbols.get_mut(created_unapply).owner = Some(foreign_owner);
    let source = session.source;
    session.with_typer(|_, _, _, _, typer| {
        let error = typer
            .type_method_definition(method)
            .expect_err("a Created extractor cannot be authenticated by Deleted$ ownership");
        assert!(matches!(
            error,
            TyperError::SymbolSourceKindMismatch {
                tree_index: 11,
                symbol,
                kind: SymbolKind::Method,
                ..
            } if symbol == created_unapply
        ));
        assert!(typer.source_typed_index().get(source, method).is_none());
        assert!(
            typer.typed_ast().iter().next().is_none(),
            "failed source method publishes no typed trees"
        );
    });
}

#[test]
fn sealed_event_rejects_forged_companion_and_selector_ownership() {
    for (offset, corruption) in ["forged companion link", "foreign _1 owner"]
        .into_iter()
        .enumerate()
    {
        let mut session = Session::new(
            SOURCE_TRAIT_PRODUCT1,
            SourceId::from_index(4_000 + offset as u32),
        );
        let source = session.source;
        let created = session.source_type_symbol("Created");
        let deleted = session.source_type_symbol("Deleted");
        let method = session.method_named("extract");
        let created_unapply = session
            .index
            .generated_case_class_method(created, GeneratedCaseClassMethodRole::Unapply)
            .unwrap();
        let deleted_companion = session.store.symbols.get(deleted).links.companion.unwrap();
        let created_links = session.store.symbols.get(created).links;
        let class_scope = session.index.scope_of(created).unwrap();
        let selector_name = Name::new(session.store.names.intern("_1"), Namespace::Term);
        let selector = session
            .store
            .scopes
            .get(class_scope)
            .lookup(&selector_name)
            .unwrap();
        let selector_owner = session.store.symbols.get(selector).owner;
        match corruption {
            "forged companion link" => {
                session.store.symbols.get_mut(created).links.companion = Some(deleted_companion);
            }
            "foreign _1 owner" => {
                session.store.symbols.get_mut(selector).owner = Some(deleted);
            }
            _ => unreachable!(),
        }
        session.with_typer(|_, parsed, index, _, typer| {
            let before = event_method_failure_projection(typer, source, parsed, index);
            let error = typer
                .type_method_definition(method)
                .expect_err("forged source companion or selector ownership is rejected");
            assert!(
                matches!(
                    &error,
                    TyperError::SymbolSourceKindMismatch { .. }
                        | TyperError::ExtractorQualifierNotFound { .. }
                        | TyperError::ExtractorQualifierNotValueLike { .. }
                        | TyperError::ExtractorUnapplyNotFound { .. }
                        | TyperError::ExtractorResultMemberNotFound { .. }
                        | TyperError::DeclarationContextMissing { .. }
                ),
                "{corruption}: {error:?}"
            );
            assert_eq!(
                event_method_failure_projection(typer, source, parsed, index),
                before,
                "{corruption}: failure leaves the deliberate corruption and all other state intact"
            );
            assert!(typer.source_typed_index().get(source, method).is_none());
            let retry = typer
                .type_method_definition(method)
                .expect_err("the structural corruption fails stably on retry");
            assert_eq!(format!("{retry:?}"), format!("{error:?}"));
            assert_eq!(
                event_method_failure_projection(typer, source, parsed, index),
                before,
                "{corruption}: stable failure still rolls back to the same state"
            );
            println!("ISSUE1999_STRUCTURAL\t{corruption}\t{error:?}");
        });
        match corruption {
            "forged companion link" => {
                session.store.symbols.get_mut(created).links = created_links;
            }
            "foreign _1 owner" => {
                session.store.symbols.get_mut(selector).owner = selector_owner;
            }
            _ => unreachable!(),
        }
        session.with_typer(|_, _, _, _, typer| {
            typer
                .type_method_definition(method)
                .expect("restored canonical identities recover in the same session");
        });
        assert_ne!(created_unapply, deleted_companion);
    }
}

#[test]
fn flagged_product2_negative_matrix_rejects_without_publishing_roots() {
    let cases = [
        (
            "wrong_nested_literal_type",
            FLAGGED_SOURCE.replace("Flagged(0, true)", "Flagged(true, 0)"),
            "classify",
        ),
        (
            "wrong_arity_one",
            FLAGGED_SOURCE.replace("Flagged(value, enabled)", "Flagged(value)"),
            "select",
        ),
        (
            "wrong_arity_three",
            FLAGGED_SOURCE.replace("Flagged(value, enabled)", "Flagged(value, enabled, 0)"),
            "select",
        ),
        (
            "wrong_declared_result",
            FLAGGED_SOURCE.replace(
                "def classify(item: Flagged): Int",
                "def classify(item: Flagged): Boolean",
            ),
            "classify",
        ),
        (
            "cross_case_binder_escape",
            "package matching\ncase class Flagged(value: Int, enabled: Boolean)\nobject Matcher:\n  def classify(item: Flagged): Int =\n    item match\n      case Flagged(value, _) => value\n      case _ => value".to_owned(),
            "classify",
        ),
        (
            "missing_case_class_extractor_identity",
            "package matching\nclass Flagged(val value: Int, val enabled: Boolean)\nobject Matcher:\n  def select(item: Flagged): Int =\n    item match\n      case Flagged(value, enabled) => value".to_owned(),
            "select",
        ),
    ];
    for (offset, (label, text, method_name)) in cases.into_iter().enumerate() {
        let source = SourceId::from_index(1980 + offset as u32);
        let mut session = Session::new(&text, source);
        let method = session.method_named(method_name);
        session.with_typer(|_, _, _, _, typer| {
            let before_store = typer.store().checkpoint();
            let before_typed = typer.typed_ast().checkpoint();
            let before_mappings = typer.source_typed_index().len();
            let error = typer
                .type_method_definition(method)
                .expect_err("the bounded malformed root is rejected");
            let expected = match label {
                "wrong_nested_literal_type" => {
                    matches!(error, TyperError::PatternTypeMismatch { .. })
                }
                "wrong_arity_one" | "wrong_arity_three" => matches!(
                    error,
                    TyperError::ExtractorPatternArityUnsupported { .. }
                        | TyperError::ExtractorProductSelectorCountMismatch { .. }
                ),
                "wrong_declared_result" => {
                    matches!(error, TyperError::ExpectedExpressionTypeMismatch { .. })
                }
                "cross_case_binder_escape" => matches!(
                    error,
                    TyperError::TermNameNotFound { .. }
                        | TyperError::UnsupportedTermReference { .. }
                ),
                "missing_case_class_extractor_identity" => {
                    matches!(error, TyperError::ExtractorQualifierNotFound { .. })
                }
                _ => unreachable!(),
            };
            assert!(expected, "{label}: unexpected diagnostic {error:?}");
            assert_eq!(
                typer.store().checkpoint(),
                before_store,
                "{label}: semantic rollback"
            );
            assert_eq!(
                typer.typed_ast().checkpoint(),
                before_typed,
                "{label}: typed arena rollback"
            );
            assert_eq!(
                typer.source_typed_index().len(),
                before_mappings,
                "{label}: source index rollback"
            );
            assert!(
                typer.source_typed_index().get(source, method).is_none(),
                "{label}: no typed root mapping"
            );
        });
    }
}

#[test]
fn flagged_product2_requires_canonical_product_identity() {
    let source = SourceId::from_index(1990);
    let mut session = Session::new(FLAGGED_SOURCE, source);
    let method = session.method_named("select");
    session.with_typer_without_resolver(|_, _, _, _, typer| {
        let error = typer
            .type_method_definition(method)
            .expect_err("Product cannot be guessed without its canonical classpath identity");
        assert!(matches!(error, TyperError::TypeNameNotFound { name, .. } if typer.store().names.resolve(name.text()) == "Product"), "{error:?}");
    });
}

fn flagged_positive_session(source: SourceId) -> Vec<String> {
    let mut session = Session::new(FLAGGED_SOURCE, source);
    let class = flagged_class_symbol(&session, source);
    let class_tree = session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, _)| {
            (session.index.symbol_at(source, tree) == Some(class)).then_some(tree)
        })
        .unwrap();
    assert_eq!(class_tree.index(), 8, "frozen Flagged class source tree");
    let companion = session.store.symbols.get(class).links.companion.unwrap();
    assert!(
        matches!(session.store.symbols.get(class).origin, SymbolOrigin::Source(id) if id == source)
    );
    assert_eq!(
        session.store.symbols.get(companion).links.companion,
        Some(class)
    );
    let unapply = session
        .index
        .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
        .expect("original generated Flagged.unapply");
    let class_scope = session.index.scope_of(class).expect("Flagged class scope");
    let third_selector = Name::new(session.store.names.intern("_3"), Namespace::Term);
    assert!(
        session
            .store
            .scopes
            .get(class_scope)
            .lookup_all(&third_selector)
            .is_empty(),
        "the two-field source class has no _3 selector"
    );
    let mut selectors = Vec::new();
    for (spelling, expected_tree, expected_span) in [("_1", 2, (37, 47)), ("_2", 4, (49, 65))] {
        let name = Name::new(session.store.names.intern(spelling), Namespace::Term);
        let [selector] = session.store.scopes.get(class_scope).lookup_all(&name) else {
            panic!("Flagged has exactly one {spelling} selector");
        };
        assert_eq!(session.store.symbols.get(*selector).owner, Some(class));
        assert_eq!(
            session.store.symbols.get(*selector).kind,
            SymbolKind::Method
        );
        let source_fields = session
            .parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| {
                (session.index.derived_symbol_at(class, source, tree) == Some(*selector))
                    .then_some((tree, node.position.unwrap().span().range()))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            source_fields.len(),
            1,
            "unique source field provenance for {spelling}"
        );
        assert_eq!(source_fields[0].0.index(), expected_tree);
        assert_eq!(
            (source_fields[0].1.start(), source_fields[0].1.end()),
            expected_span
        );
        selectors.push(*selector);
    }

    let methods = ["select", "classify"]
        .into_iter()
        .map(|name| {
            let method = session.method_named(name);
            let span = session
                .parsed
                .ast
                .get(method)
                .position
                .unwrap()
                .span()
                .range();
            (name, method, span)
        })
        .collect::<Vec<_>>();
    session.with_typer(|_, parsed, index, definitions, typer| {
        let class_info = typer.complete_symbol(class).expect("complete source Flagged");
        let Some(Type::ClassInfo(class_info)) = typer.store().types.try_get(class_info) else {
            panic!("Flagged has complete ClassInfo");
        };
        let products = class_info
            .parents
            .iter()
            .filter_map(|parent| match typer.store().types.try_get(*parent) {
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. })
                    if typer.store().names.resolve(typer.store().symbols.get(*symbol).name.text()) == "Product"
                        && matches!(typer.store().symbols.get(*symbol).origin, SymbolOrigin::Tasty(_)) => Some(*symbol),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(products.len(), 1, "Flagged has exactly one canonical Product parent");
        let product = products[0];
        assert_eq!(typer.store().symbols.get(product).kind, SymbolKind::Trait);
        assert_eq!(typer.store().names.resolve(typer.store().symbols.get(product).name.text()), "Product");
        let unapply_type = typer.complete_symbol(unapply).expect("generated unapply signature");
        let Some(Type::Method(unapply_type)) = typer.store().types.try_get(unapply_type) else {
            panic!("generated unapply has a complete MethodType");
        };
        assert_eq!(unapply_type.params.len(), 1);
        assert!(matches!(typer.store().types.try_get(unapply_type.params[0].ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class));
        assert!(matches!(typer.store().types.try_get(unapply_type.result), Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class));
        let mut summaries = Vec::new();
        let mut method_binders = Vec::new();
        for (name, method, span) in methods {
            let expected = if name == "select" { (24, 86, 208, 1) } else { (44, 212, 332, 2) };
            assert_eq!((method.index(), span.start(), span.end()), (expected.0, expected.1, expected.2));
            let method_symbol = index.symbol_at(source, method).unwrap();
            let owner = typer.store().symbols.get(method_symbol).owner.unwrap();
            assert_eq!(typer.store().symbols.get(owner).kind, SymbolKind::ModuleClass);
            assert_eq!(typer.store().names.resolve(typer.store().symbols.get(owner).name.text()), "Matcher$");

            let typed_method = typer.type_method_definition(method).unwrap_or_else(|error| panic!("{name}: {error:?}"));
            println!("ISSUE1970_TYPED_ROOT\t{source:?}\t{name}\tPositive.scala#tree{}\t{}:{}\ttyped#tree{}", method.index(), span.start(), span.end(), typed_method.index());
            assert_mapping(typer.source_typed_index(), source, method);
            assert_eq!(typer.source_typed_index().get(source, method), Some(typed_method));
            let signature = match *typer.store().symbols.info(method_symbol) {
                SymbolInfo::Complete(signature) => signature,
                other => panic!("{name} signature is incomplete: {other:?}"),
            };
            let Some(Type::Method(signature)) = typer.store().types.try_get(signature) else {
                panic!("{name} signature is not a MethodType");
            };
            assert_eq!(signature.params.len(), 1);
            assert!(matches!(typer.store().types.try_get(signature.params[0].ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class));
            assert_eq!(signature.result, definitions.int);

            let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
                panic!("{name} is a complete typed DefDef");
            };
            let source_definition = match &parsed.ast.get(method).kind {
                TreeKind::DefDef(definition) => definition,
                _ => unreachable!(),
            };
            assert_eq!(typed_definition.value_param_clauses[0].len(), 1);
            assert_mapping(typer.source_typed_index(), source, source_definition.value_param_clauses[0][0]);
            assert_mapping(typer.source_typed_index(), source, source_definition.tpt);
            assert_eq!(typer.source_typed_index().get(source, source_definition.tpt), Some(typed_definition.tpt));
            assert_eq!(typer.typed_ast().get(typed_definition.tpt).ty, definitions.int);
            let source_rhs = source_definition.rhs.unwrap();
            let typed_rhs = typed_definition.rhs.unwrap();
            assert_mapping(typer.source_typed_index(), source, source_rhs);
            assert_eq!(typer.source_typed_index().get(source, source_rhs), Some(typed_rhs));
            let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_rhs).kind else {
                panic!("{name} RHS is Match");
            };
            let TreeKind::Match(source_match) = &parsed.ast.get(source_rhs).kind else {
                panic!("frozen RHS remains Match");
            };
            assert_eq!(typed_match.cases.len(), expected.3);
            assert_eq!(typer.typed_ast().get(typed_rhs).ty, definitions.int);
            assert_mapping(typer.source_typed_index(), source, source_match.selector);
            let typed_selector = typer.source_typed_index().get(source, source_match.selector).unwrap();
            let parameter_symbol = index.symbol_at(source, source_definition.value_param_clauses[0][0]).unwrap();
            assert!(matches!(typer.store().types.try_get(typer.typed_ast().get(typed_selector).ty), Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == parameter_symbol));
            let mut case_shapes = Vec::new();
            for (case_index, (source_case, typed_case)) in source_match.cases.iter().zip(&typed_match.cases).enumerate() {
                assert_mapping(typer.source_typed_index(), source, *source_case);
                assert_eq!(typer.source_typed_index().get(source, *source_case), Some(*typed_case));
                let TreeKind::CaseDef(source_case_node) = &parsed.ast.get(*source_case).kind else { panic!("source CaseDef") };
                let TreeKind::CaseDef(typed_case_node) = &typer.typed_ast().get(*typed_case).kind else { panic!("typed CaseDef") };
                assert_mapping(typer.source_typed_index(), source, source_case_node.pattern);
                assert_eq!(typer.source_typed_index().get(source, source_case_node.pattern), Some(typed_case_node.pattern));
                assert_mapping(typer.source_typed_index(), source, source_case_node.body);
                let typed_body = typer.source_typed_index().get(source, source_case_node.body).unwrap();
                let TreeKind::UnApply(typed_unapply) = &typer.typed_ast().get(typed_case_node.pattern).kind else { panic!("typed UnApply") };
                assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_unapply.function), unapply);
                assert_eq!(index.generated_case_class_method_identity(unapply), Some(dotty_core::GeneratedCaseClassMethod { class, role: GeneratedCaseClassMethodRole::Unapply }));
                let unapply_owner = typer.store().symbols.get(unapply).owner.unwrap();
                assert_eq!(typer.store().symbols.get(unapply_owner).kind, SymbolKind::ModuleClass);
                assert_eq!(typer.store().names.resolve(typer.store().symbols.get(unapply_owner).name.text()), "Flagged$");
                assert_eq!(typed_unapply.patterns.len(), 2);
                let TreeKind::Apply(source_apply) = &parsed.ast.get(source_case_node.pattern).kind else { panic!("source extractor application") };
                let source_qualifier = source_apply.function;
                assert_mapping(typer.source_typed_index(), source, source_qualifier);
                assert!(matches!(parsed.ast.get(source_qualifier).kind, TreeKind::Ident(_) | TreeKind::Select(_)));
                let TreeKind::Select(typed_selection) = &typer.typed_ast().get(typed_unapply.function).kind else { panic!("typed companion extractor") };
                assert_eq!(typer.source_typed_index().get(source, source_qualifier), Some(typed_selection.qualifier));
                for (source_pattern, typed_pattern) in source_apply.args.iter().zip(&typed_unapply.patterns) {
                    assert_mapping(typer.source_typed_index(), source, *source_pattern);
                    assert_eq!(typer.source_typed_index().get(source, *source_pattern), Some(*typed_pattern));
                }
                let mut case_binders = Vec::new();
                match (name, case_index) {
                    ("select", 0) => {
                        for (position, (wanted_name, wanted_type)) in [("value", definitions.int), ("enabled", definitions.boolean)].into_iter().enumerate() {
                            let source_pattern = source_apply.args[position];
                            let typed_pattern = typed_unapply.patterns[position];
                            let TreeKind::Bind(binding) = &typer.typed_ast().get(typed_pattern).kind else { panic!("case-local {wanted_name} binder") };
                            let binder = typed_symbol(typer.typed_ast(), typer.store(), typed_pattern);
                            assert_eq!(typer.store().names.resolve(binding.name.text()), wanted_name);
                            assert_eq!(*typer.store().symbols.info(binder), SymbolInfo::Complete(wanted_type));
                            assert_eq!(typer.store().symbols.get(binder).owner, Some(method_symbol));
                            assert!(matches!(typer.store().symbols.get(binder).origin, SymbolOrigin::Source(id) if id == source));
                            assert!(matches!(typer.typed_ast().get(binding.body).kind, TreeKind::Ident(_)));
                            let span = parsed.ast.get(source_pattern).position.unwrap().span().range();
                            case_shapes.push(format!("Bind({wanted_name}:{})@{}:{}", if position == 0 { "Int" } else { "Boolean" }, span.start(), span.end()));
                            method_binders.push(binder);
                            case_binders.push(binder);
                        }
                        let TreeKind::Block(source_block) = &parsed.ast.get(source_case_node.body).kind else { panic!("source select body block") };
                        let TreeKind::Block(typed_block) = &typer.typed_ast().get(typed_body).kind else { panic!("typed select body block") };
                        assert_mapping(typer.source_typed_index(), source, source_block.expr);
                        assert_eq!(typer.source_typed_index().get(source, source_block.expr), Some(typed_block.expr));
                        let TreeKind::If(source_if) = &parsed.ast.get(source_block.expr).kind else { panic!("source select If") };
                        let TreeKind::If(typed_if) = &typer.typed_ast().get(typed_block.expr).kind else { panic!("typed select If") };
                        assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_if.cond), case_binders[1]);
                        assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_if.then_branch), case_binders[0]);
                        assert_eq!(*typer.store().symbols.info(case_binders[1]), SymbolInfo::Complete(definitions.boolean));
                        assert_eq!(*typer.store().symbols.info(case_binders[0]), SymbolInfo::Complete(definitions.int));
                        assert!(matches!(typer.store().types.try_get(typer.typed_ast().get(typed_if.else_branch).ty), Some(Type::Constant(Constant::Int(0)))));
                        for source_tree in [source_if.cond, source_if.then_branch, source_if.else_branch] {
                            assert_mapping(typer.source_typed_index(), source, source_tree);
                        }
                        assert_eq!(typer.source_typed_index().get(source, source_if.cond), Some(typed_if.cond));
                        assert_eq!(typer.source_typed_index().get(source, source_if.then_branch), Some(typed_if.then_branch));
                        assert_eq!(typer.source_typed_index().get(source, source_if.else_branch), Some(typed_if.else_branch));
                    }
                    ("classify", 0) => {
                        for (position, wanted_literal) in [Constant::Int(0), Constant::Boolean(true)].into_iter().enumerate() {
                            let typed_pattern = typed_unapply.patterns[position];
                            assert!(matches!(typer.typed_ast().get(typed_pattern).kind, TreeKind::Literal(_)));
                            assert_eq!(typer.store().types.try_get(typer.typed_ast().get(typed_pattern).ty), Some(&Type::Constant(wanted_literal)));
                        }
                        case_shapes.push("Literal(0:Int),Literal(true:Boolean)".to_owned());
                    }
                    ("classify", 1) => {
                        let TreeKind::Bind(binding) = &typer.typed_ast().get(typed_unapply.patterns[0]).kind else { panic!("classify value binder") };
                        let binder = typed_symbol(typer.typed_ast(), typer.store(), typed_unapply.patterns[0]);
                        assert_eq!(typer.store().names.resolve(binding.name.text()), "value");
                        assert_eq!(*typer.store().symbols.info(binder), SymbolInfo::Complete(definitions.int));
                        assert_eq!(typer.store().symbols.get(binder).owner, Some(method_symbol));
                        assert!(matches!(typer.store().symbols.get(binder).origin, SymbolOrigin::Source(id) if id == source));
                        assert!(matches!(typer.typed_ast().get(typed_unapply.patterns[1]).kind, TreeKind::Ident(_)));
                        assert_eq!(typer.typed_ast().get(typed_unapply.patterns[1]).ty, definitions.boolean);
                        assert_eq!(typer.store().names.resolve(match &typer.typed_ast().get(typed_unapply.patterns[1]).kind { TreeKind::Ident(ident) => ident.name.text(), _ => unreachable!() }), "_");
                        assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_body), binder);
                        method_binders.push(binder);
                        case_binders.push(binder);
                        case_shapes.push("Bind(value:Int),Wildcard(Boolean)".to_owned());
                    }
                    _ => unreachable!("only the two frozen methods are expected"),
                }
            }
            let normalized = format!("{name}\tPositive.scala#tree{}\t{}:{}\tmatching.Flagged\tmatching.Matcher\t(Flagged):Int\tscala.Product\t_1:Int@Flagged,_2:Boolean@Flagged\tFlagged$.unapply\t{}\tcomplete\tInt\ttrue\t4b144913df1d2021359c44633dc2a23f21bf6288bf2528c86cc6f53d79b8ee31", expected.0, expected.1, expected.2, case_shapes.join(";"));
            summaries.push(normalized);
        }
        assert_eq!(selectors.len(), 2);
        assert_ne!(selectors[0], selectors[1], "Product2 selectors have distinct identities");
        for (selector, expected_type) in selectors.into_iter().zip([definitions.int, definitions.boolean]) {
            let selector_info = typer.complete_symbol(selector).expect("complete canonical source selector");
            let Some(Type::Method(selector_type)) = typer.store().types.try_get(selector_info) else { panic!("selector is parameterless method") };
            assert!(selector_type.params.is_empty());
            assert_eq!(selector_type.result, expected_type);
        }
        assert_eq!(method_binders.len(), 3);
        assert_eq!(method_binders.iter().copied().collect::<std::collections::HashSet<_>>().len(), 3, "each case binder has independent identity");
        summaries
    })
}

fn flagged_class_symbol(session: &Session, source: SourceId) -> SymbolId {
    session
        .parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if session
                    .store
                    .names
                    .resolve(definition.name.as_name().text())
                    == "Flagged" =>
            {
                session.index.symbol_at(source, tree)
            }
            _ => None,
        })
        .expect("source Flagged symbol")
}

#[test]
fn late_body_error_rolls_back_extractor_and_case_binder_state_before_retry() {
    let text = SOURCE.replacen("case Box(value) => value", "case Box(value) => true", 1);
    let mut session = Session::new(&text, SourceId::from_index(1934));
    let (class, roots) = session.roots();
    let method = roots[0].0;
    let unapply = session
        .index
        .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
        .unwrap();
    assert_eq!(session.store.symbols.get(class).kind, SymbolKind::Class);
    session.with_typer(|_source, _parsed, index, _definitions, typer| {
        let before_store = typer.store().checkpoint();
        let before_typed = typer.typed_ast().checkpoint();
        let before_mappings = typer.source_typed_index().len();
        let error = typer
            .type_method_definition(method)
            .expect_err("Boolean body does not conform to the declared Int result");
        assert!(
            matches!(error, TyperError::ExpectedExpressionTypeMismatch { .. }),
            "{error:?}"
        );
        assert_eq!(typer.store().checkpoint(), before_store);
        assert_eq!(typer.typed_ast().checkpoint(), before_typed);
        assert_eq!(typer.source_typed_index().len(), before_mappings);
        assert_eq!(*typer.store().symbols.info(unapply), SymbolInfo::Missing);
        assert!(
            index
                .generated_case_class_method_identity(unapply)
                .is_some()
        );
        let retry = typer
            .type_method_definition(method)
            .expect_err("same source must still fail after retry");
        assert_eq!(format!("{retry:?}"), format!("{error:?}"));
        assert_eq!(typer.store().checkpoint(), before_store);
        assert_eq!(typer.typed_ast().checkpoint(), before_typed);
        assert_eq!(typer.source_typed_index().len(), before_mappings);
        let classify = roots[1].0;
        let typed_classify = typer
            .type_method_definition(classify)
            .expect("valid sibling method remains typeable after rollback");
        assert!(matches!(
            typer.typed_ast().get(typed_classify).kind,
            TreeKind::DefDef(_)
        ));
    });
}

#[test]
fn wrong_component_type_and_wrong_extractor_arities_are_rejected() {
    for (text, expected) in [
        (
            "package matching; case class Box(value: Int); object Matcher { def f(box: Box): Int = box match { case Box(true) => 0 } }",
            "component type",
        ),
        (
            "package matching; case class Box(value: Int); object Matcher { def f(box: Box): Int = box match { case Box() => 0 } }",
            "zero arity",
        ),
        (
            "package matching; case class Box(value: Int); object Matcher { def f(box: Box): Int = box match { case Box(a, b) => a } }",
            "two arity",
        ),
    ] {
        let mut session = Session::new(text, SourceId::from_index(1935));
        let (_, roots) = session.roots();
        let method = roots[0].0;
        session.with_typer(|_source, _parsed, _index, _definitions, typer| {
            let error = typer.type_method_definition(method).unwrap_err();
            assert!(
                matches!(
                    error,
                    TyperError::PatternTypeMismatch { .. }
                        | TyperError::ExtractorPatternArityUnsupported { .. }
                        | TyperError::ExtractorProductSelectorCountMismatch { .. }
                ),
                "{expected} must be rejected through extractor typing, got {error:?}"
            );
        });
    }
}

#[test]
fn ordinary_classes_wrong_method_results_and_case_binder_escape_are_rejected() {
    let cases = [
        (
            "package matching\nclass Box(val value: Int)\nobject Matcher:\n  def f(box: Box): Int =\n    box match\n      case Box(value) => value",
            "extractor qualifier",
        ),
        (
            "package matching\ncase class Box(value: Int)\nobject Matcher:\n  def f(box: Box): Boolean =\n    box match\n      case Box(value) => value",
            "declared result",
        ),
        (
            "package matching\ncase class Box(value: Int)\nobject Matcher:\n  def f(box: Box): Int =\n    box match\n      case Box(value) => 0\n      case _ => value",
            "case-local binder escape",
        ),
        (
            "package matching\ncase class Box(value: Int)\nobject Matcher:\n  def f(box: Box): Int =\n    true match\n      case Box(_) => 0",
            "wrong selector type",
        ),
    ];
    for (text, label) in cases {
        let mut session = Session::new(text, SourceId::from_index(1936));
        let (_, roots) = session.roots();
        let method = roots[0].0;
        session.with_typer(|_source, _parsed, _index, _definitions, typer| {
            let error = typer.type_method_definition(method).unwrap_err();
            assert!(
                matches!(
                    error,
                    TyperError::ExtractorQualifierNotFound { .. }
                        | TyperError::TermNameNotFound { .. }
                        | TyperError::ExpectedExpressionTypeMismatch { .. }
                        | TyperError::MatchSelectorTypeCannotBeAdapted { .. }
                        | TyperError::ExtractorPatternConstraintDeferred { .. }
                        | TyperError::UnsupportedTermReference { .. }
                ),
                "{label} must be rejected explicitly, got {error:?}"
            );
        });
    }
}

#[test]
fn frozen_extractor_negative_matrix_rejects_or_defers_without_typed_roots() {
    let cases = [
        (
            "wrong_declared_result",
            WRONG_RESULT,
            "mismatch",
            "scala_invalid",
        ),
        (
            "wrong_arity_zero",
            WRONG_ARITY_ZERO,
            "mismatch",
            "scala_invalid",
        ),
        (
            "wrong_arity_two",
            WRONG_ARITY_TWO,
            "mismatch",
            "scala_invalid",
        ),
        (
            "wrong_constructor_type",
            WRONG_CONSTRUCTOR_TYPE,
            "mismatch",
            "scala_invalid",
        ),
        (
            "wrong_pattern_type",
            WRONG_PATTERN_TYPE,
            "extract",
            "scala_invalid",
        ),
        (
            "non_case_missing_extractor",
            NON_CASE_MISSING_EXTRACTOR,
            "extract",
            "scala_invalid",
        ),
        (
            "case_binder_escape",
            CASE_BINDER_ESCAPE,
            "extract",
            "scala_invalid",
        ),
        (
            "wrong_selector_type",
            WRONG_SELECTOR_TYPE,
            "mismatch",
            "scala_valid_rust_deferred",
        ),
        (
            "explicit_unapply_wrong_result",
            EXPLICIT_UNAPPLY_WRONG_RESULT,
            "extract",
            "scala_invalid",
        ),
    ];

    for (case_name, text, method_name, scala_classification) in cases {
        let mut session = Session::new(text, SourceId::from_index(1937));
        let method = session.method_named(method_name);
        session.with_typer(|source, parsed, _index, _definitions, typer| {
            let before_store = typer.store().checkpoint();
            let before_typed = typer.typed_ast().checkpoint();
            let before_mappings = typer.source_typed_index().len();
            let error = typer
                .type_method_definition(method)
                .expect_err("negative/deferred matrix entries cannot publish a typed root");
            let category = error_category(&error);
            assert_ne!(category, "unexpected", "{case_name}: {error:?}");
            assert_eq!(typer.store().checkpoint(), before_store, "{case_name}: store rollback");
            assert_eq!(typer.typed_ast().checkpoint(), before_typed, "{case_name}: typed rollback");
            assert_eq!(
                typer.source_typed_index().len(),
                before_mappings,
                "{case_name}: source mapping rollback"
            );
            assert!(
                typer.source_typed_index().get(source, method).is_none(),
                "{case_name}: failed method has no typed-root mapping"
            );
            let source_path = match case_name {
                "wrong_declared_result" => "WrongResult.scala",
                "wrong_arity_zero" => "WrongArityZero.scala",
                "wrong_arity_two" => "WrongArityTwo.scala",
                "wrong_constructor_type" => "WrongConstructorType.scala",
                "wrong_pattern_type" => "WrongPatternType.scala",
                "non_case_missing_extractor" => "NonCaseMissingExtractor.scala",
                "case_binder_escape" => "CaseBinderEscape.scala",
                "wrong_selector_type" => "WrongSelectorType.scala",
                "explicit_unapply_wrong_result" => "ExplicitUnapplyWrongResult.scala",
                _ => unreachable!("fixed matrix entry"),
            };
            let method_span = parsed.ast.get(method).position.unwrap().span().range();
            println!(
                "ISSUE1938_NEGATIVE\t{case_name}\t{source_path}#tree{}\t{}:{}\trejected:{category}\t{scala_classification}\t{category}",
                method.index(),
                method_span.start(),
                method_span.end()
            );
        });
    }
}

fn error_category(error: &TyperError) -> &'static str {
    match error {
        TyperError::ExpectedExpressionTypeMismatch { .. } => "result_type_mismatch",
        TyperError::ExtractorPatternArityUnsupported { .. } => "extractor_arity_unsupported",
        TyperError::ExtractorProductSelectorCountMismatch { .. } => {
            "product_selector_count_mismatch"
        }
        TyperError::ExtractorPatternConstraintDeferred { .. } => "extractor_constraint_deferred",
        TyperError::ExtractorQualifierNotFound { .. } => "extractor_qualifier_not_found",
        TyperError::PatternTypeMismatch { .. } => "pattern_type_mismatch",
        TyperError::TypedPatternTypeMismatch { .. } => "typed_pattern_type_mismatch",
        TyperError::TermNameNotFound { .. } => "term_name_not_found",
        TyperError::MatchSelectorTypeCannotBeAdapted { .. } => "selector_type_cannot_be_adapted",
        TyperError::UnsupportedTermReference { .. } => "unsupported_term_reference",
        TyperError::ExtractorResultMemberNotFound { .. } => "extractor_result_member_not_found",
        TyperError::ExtractorResultMemberUnsupported { .. } => {
            "extractor_result_member_unsupported"
        }
        _ => "unexpected",
    }
}

fn positive_session(
    source_text: &str,
    source: SourceId,
    receipt_session: Option<&str>,
) -> Vec<String> {
    let mut session = Session::new(source_text, source);
    let (class, roots) = session.roots();
    session.with_typer(|source, parsed, index, definitions, typer| {
        let companion = typer.store().symbols.get(class).links.companion.unwrap();
        assert!(matches!(typer.store().symbols.get(class).origin, SymbolOrigin::Source(id) if id == source));
        assert_eq!(typer.store().symbols.get(companion).links.companion, Some(class));
        let unapply = index
            .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
            .expect("original generated Box.unapply identity");
        let apply = index
            .generated_case_class_method(class, GeneratedCaseClassMethodRole::Apply)
            .expect("original generated Box.apply identity");
        assert_ne!(apply, unapply, "apply and unapply have independent identities");
        let box_info = typer.complete_symbol(class).expect("complete source Box");
        let product = match typer.store().types.try_get(box_info) {
            Some(Type::ClassInfo(info)) => info
                .parents
                .iter()
                .find_map(|parent| match typer.store().types.try_get(*parent) {
                    Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. })
                        if typer.store().names.resolve(typer.store().symbols.get(*symbol).name.text()) == "Product"
                            && matches!(typer.store().symbols.get(*symbol).origin, SymbolOrigin::Tasty(_)) => Some(*symbol),
                    _ => None,
                })
                .expect("Box parent is the authenticated scala.Product TASTy symbol"),
            other => panic!("Box class info missing after typing: {other:?}"),
        };
        assert_eq!(typer.store().symbols.get(product).kind, SymbolKind::Trait);
        let scala_package = typer.store().symbols.get(product).owner.unwrap();
        assert_eq!(typer.store().symbols.get(scala_package).kind, SymbolKind::Package);
        assert_eq!(typer.store().names.resolve(typer.store().symbols.get(scala_package).name.text()), "scala");
        let class_scope = index.scope_of(class).expect("source Box declaration scope");
        assert_eq!(typer.store().scopes.get(class_scope).owner, Some(class));
        let product_selectors = typer
            .store()
            .scopes
            .get(class_scope)
            .entered_symbols()
            .filter(|symbol| {
                typer.store().names.resolve(typer.store().symbols.get(*symbol).name.text()) == "_1"
            })
            .collect::<Vec<_>>();
        assert_eq!(product_selectors.len(), 1, "Box has exactly one canonical _1 selector");
        let product_selector = product_selectors[0];
        assert_eq!(typer.store().symbols.get(product_selector).owner, Some(class));
        assert_eq!(typer.store().symbols.get(product_selector).kind, SymbolKind::Method);
        assert_eq!(
            parsed
                .ast
                .iter()
                .filter(|(tree, _)| index.derived_symbol_at(class, source, *tree) == Some(product_selector))
                .count(),
            1,
            "_1 retains its unique source constructor-parameter provenance"
        );
        let selector_info = typer
            .complete_symbol(product_selector)
            .expect("complete source Box._1 projection");
        let Some(Type::Method(selector_type)) = typer.store().types.try_get(selector_info) else {
            panic!("Box._1 must complete to a parameterless method");
        };
        assert!(selector_type.params.is_empty());
        assert_eq!(selector_type.result, definitions.int, "Box._1 has exact type Int");
        let mut summaries = Vec::new();
        let mut method_binders = Vec::new();

        for (name, method, parameter, expected_cases) in [
            ("unwrap", roots[0].0, roots[0].1, 1),
            ("classify", roots[1].0, roots[1].1, 2),
        ] {
            let method_symbol = index.symbol_at(source, method).unwrap();
            let method_owner = typer.store().symbols.get(method_symbol).owner.unwrap();
            assert_eq!(typer.store().symbols.get(method_owner).kind, SymbolKind::ModuleClass);
            assert_eq!(
                typer.store().names.resolve(typer.store().symbols.get(method_owner).name.text()),
                "Matcher$"
            );
            let typed_method = typer
                .type_method_definition(method)
                .unwrap_or_else(|error| panic!("{name} failed: {error:?}"));
            assert_mapping(typer.source_typed_index(), source, method);
            assert_eq!(
                typer.source_typed_index().get(source, method),
                Some(typed_method)
            );
            let signature = match *typer.store().symbols.info(method_symbol) {
                SymbolInfo::Complete(signature) => signature,
                other => panic!("{name} method signature is incomplete: {other:?}"),
            };
            let Some(Type::Method(method_type)) = typer.store().types.try_get(signature) else {
                panic!("{name} signature must be a complete MethodType");
            };
            assert_eq!(method_type.params.len(), 1);
            assert!(matches!(
                typer.store().types.try_get(method_type.params[0].ty),
                Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == class
            ));
            assert_eq!(method_type.result, definitions.int);

            let TreeKind::DefDef(typed_definition) = &typer.typed_ast().get(typed_method).kind else {
                panic!("{name} must be a complete typed DefDef");
            };
            assert_eq!(typed_definition.value_param_clauses[0].len(), 1);
            assert_mapping(typer.source_typed_index(), source, parameter);
            let typed_rhs = typed_definition.rhs.expect("method body");
            let source_definition = match &parsed.ast.get(method).kind {
                TreeKind::DefDef(definition) => definition,
                _ => unreachable!(),
            };
            let method_span = parsed.ast.get(method).position.unwrap().span().range();
            let (frozen_root, frozen_span) = if name == "unwrap" {
                (18, (64, 136))
            } else {
                (36, (140, 237))
            };
            if source_text == SOURCE {
                assert_eq!(method.index(), frozen_root, "frozen original method root");
                assert_eq!(
                    (method_span.start(), method_span.end()),
                    frozen_span,
                    "frozen original method source span"
                );
            }
            assert_mapping(typer.source_typed_index(), source, source_definition.tpt);
            assert_eq!(
                typer.source_typed_index().get(source, source_definition.tpt),
                Some(typed_definition.tpt)
            );
            assert_eq!(
                typer.typed_ast().get(typed_definition.tpt).ty,
                definitions.int,
                "typed result tree carries a real result type"
            );
            let source_rhs = source_definition.rhs.unwrap();
            assert_mapping(typer.source_typed_index(), source, source_rhs);
            assert_eq!(
                typer.source_typed_index().get(source, source_rhs),
                Some(typed_rhs)
            );
            let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_rhs).kind else {
                panic!("{name} RHS must be a typed Match");
            };
            assert_eq!(typed_match.cases.len(), expected_cases);
            assert_eq!(typer.typed_ast().get(typed_rhs).ty, definitions.int);
            let TreeKind::Match(source_match) = &parsed.ast.get(source_rhs).kind else {
                panic!("frozen source RHS remains a Match");
            };
            assert_mapping(typer.source_typed_index(), source, source_match.selector);
            let selector = typer
                .source_typed_index()
                .get(source, source_match.selector)
                .unwrap();
            let parameter_symbol = index.symbol_at(source, parameter).unwrap();
            assert!(matches!(
                typer.store().types.try_get(typer.typed_ast().get(selector).ty),
                Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == parameter_symbol
            ));
            let mut case_binders = Vec::new();
            let mut case_shapes = Vec::new();
            for (case_index, (source_case, typed_case)) in
                source_match.cases.iter().zip(&typed_match.cases).enumerate()
            {
                assert_mapping(typer.source_typed_index(), source, *source_case);
                assert_eq!(
                    typer.source_typed_index().get(source, *source_case),
                    Some(*typed_case)
                );
                let TreeKind::CaseDef(source_case_node) = &parsed.ast.get(*source_case).kind else {
                    panic!("source case remains CaseDef");
                };
                let TreeKind::CaseDef(typed_case_node) = &typer.typed_ast().get(*typed_case).kind else {
                    panic!("typed case remains CaseDef");
                };
                assert_mapping(typer.source_typed_index(), source, source_case_node.pattern);
                assert_eq!(
                    typer
                        .source_typed_index()
                        .get(source, source_case_node.pattern),
                    Some(typed_case_node.pattern)
                );
                assert_mapping(typer.source_typed_index(), source, source_case_node.body);
                let typed_body = typer
                    .source_typed_index()
                    .get(source, source_case_node.body)
                    .unwrap();
                let TreeKind::UnApply(typed_unapply) =
                    &typer.typed_ast().get(typed_case_node.pattern).kind
                else {
                    panic!("case pattern must be a real typed UnApply");
                };
                let TreeKind::Select(selected) = &typer.typed_ast().get(typed_unapply.function).kind else {
                    panic!("UnApply retains its selected function");
                };
                assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_unapply.function), unapply);
                assert_eq!(
                    index.generated_case_class_method_identity(unapply),
                    Some(dotty_core::GeneratedCaseClassMethod {
                        class,
                        role: GeneratedCaseClassMethodRole::Unapply,
                    })
                );
                let unapply_owner = typer.store().symbols.get(unapply).owner.unwrap();
                assert_eq!(typer.store().symbols.get(unapply_owner).kind, SymbolKind::ModuleClass);
                assert_eq!(
                    typer.store().names.resolve(typer.store().symbols.get(unapply_owner).name.text()),
                    "Box$"
                );
                let unapply_scope = index.scope_of(unapply_owner).expect("Box companion scope");
                let unapply_name = typer.store().symbols.get(unapply).name;
                assert_eq!(
                    typer.store().scopes.get(unapply_scope).lookup_all(&unapply_name),
                    [unapply],
                    "the companion owns exactly one canonical generated unapply"
                );
                let qualifier_type = typer.typed_ast().get(selected.qualifier).ty;
                assert!(matches!(
                    typer.store().types.try_get(qualifier_type),
                    Some(Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }) if *symbol == companion
                ));
                assert_eq!(typed_unapply.patterns.len(), 1);
                let source_nested = match &parsed.ast.get(source_case_node.pattern).kind {
                    TreeKind::Apply(application) => application.args[0],
                    _ => panic!("original source extractor pattern remains Apply"),
                };
                let source_extractor = match &parsed.ast.get(source_case_node.pattern).kind {
                    TreeKind::Apply(application) => application.function,
                    _ => unreachable!(),
                };
                assert_mapping(typer.source_typed_index(), source, source_extractor);
                assert_eq!(
                    typer.source_typed_index().get(source, source_extractor),
                    Some(selected.qualifier),
                    "source extractor qualifier maps to the typed companion reference"
                );
                assert_mapping(typer.source_typed_index(), source, source_nested);
                assert_eq!(
                    typer.source_typed_index().get(source, source_nested),
                    Some(typed_unapply.patterns[0])
                );
                if name == "classify" && case_index == 0 {
                    assert!(matches!(
                        typer.typed_ast().get(typed_unapply.patterns[0]).kind,
                        TreeKind::Literal(_)
                    ));
                    assert!(matches!(
                        typer.store().types.try_get(typer.typed_ast().get(typed_unapply.patterns[0]).ty),
                        Some(Type::Constant(Constant::Int(0)))
                    ));
                    case_shapes.push("Literal(0:Int)".to_owned());
                } else {
                    let binding = typed_unapply.patterns[0];
                    let TreeKind::Bind(bind) = &typer.typed_ast().get(binding).kind else {
                        panic!("variable pattern must be a case-local Bind")
                    };
                    let binder = typed_symbol(typer.typed_ast(), typer.store(), binding);
                    assert_eq!(typer.store().symbols.get(binder).kind, SymbolKind::Local);
                    assert_eq!(typer.store().symbols.get(binder).owner, Some(method_symbol));
                    assert_eq!(
                        *typer.store().symbols.info(binder),
                        SymbolInfo::Complete(definitions.int)
                    );
                    assert_eq!(typer.store().names.resolve(bind.name.text()), "value");
                    assert_ne!(binder, parameter_symbol, "case binder cannot alias the method parameter");
                    assert_ne!(binder, method_symbol, "case binder cannot alias its method");
                    assert!(matches!(typer.store().symbols.get(binder).origin, SymbolOrigin::Source(id) if id == source));
                    assert!(matches!(typer.typed_ast().get(bind.body).kind, TreeKind::Ident(_)));
                    assert_eq!(typed_symbol(typer.typed_ast(), typer.store(), typed_body), binder);
                    case_shapes.push("Bind(value:Int)".to_owned());
                    case_binders.push(binder);
                    method_binders.push(binder);
                }
            }
            if name == "classify" {
                assert_eq!(case_binders.len(), 1);
            }
            let root = format!("Positive.scala#tree{}", frozen_root);
            let span = format!("{}:{}", frozen_span.0, frozen_span.1);
            let normalized = format!(
                "{name}|{root}|{span}|matching.Box|matching.Matcher|(Box):Int|scala.Product|_1:Int@Box|Box$.unapply|{}",
                case_shapes.join(",")
            );
            if let Some(session) = receipt_session {
                println!(
                    "ISSUE1938_POSITIVE\t{session}\t{name}\t{root}\t{span}\tmatching.Box\tmatching.Matcher\t(Box):Int\tscala.Product\t_1:Int@Box\tBox$.unapply\t{}\tInt\tcomplete\ttrue",
                    case_shapes.join(",")
                );
            }
            summaries.push(normalized);
        }
        assert_eq!(method_binders.len(), 2);
        assert_ne!(method_binders[0], method_binders[1]);
        summaries
    })
}

fn assert_mapping(index: &SourceTypedIndex, source: SourceId, tree: TreeId<Untyped>) {
    assert!(
        index.get(source, tree).is_some(),
        "source tree {} lacks typed mapping",
        tree.index()
    );
}

fn typed_symbol(
    arena: &TypedAst,
    store: &SemanticStore,
    tree: TreeId<Typed>,
) -> dotty_core::SymbolId {
    match store.types.try_get(arena.get(tree).ty) {
        Some(Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }) => *symbol,
        other => panic!("typed tree must retain its canonical symbol, got {other:?}"),
    }
}

struct Session {
    source: SourceId,
    parsed: dotty_parser::ParseResult,
    store: SemanticStore,
    definitions: Definitions,
    index: dotty_core::SourceSemanticIndex,
    packages: Packages,
}

impl Session {
    fn new(source_text: &str, source: SourceId) -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let parsed = dotty_parser::parse_compilation_unit(
            SourceText::new(source_text).expect("valid source text"),
            source,
            ContextualScanner::new(source_text).expect("scan source"),
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "Positive.scala",
            &mut store,
            &mut packages,
        )
        .expect("name source");
        Self {
            source,
            parsed,
            store,
            definitions,
            index,
            packages,
        }
    }

    fn roots(&self) -> MatchRoots {
        let class = self
            .parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if self.store.names.resolve(definition.name.as_name().text()) == "Box" =>
                {
                    self.index.symbol_at(self.source, tree)
                }
                _ => None,
            })
            .expect("source Box symbol");
        let methods = self
            .parsed
            .ast
            .iter()
            .filter_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if matches!(
                        self.store.names.resolve(definition.name.as_name().text()),
                        "unwrap" | "classify" | "f"
                    ) =>
                {
                    Some((tree, definition.value_param_clauses[0][0]))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        (class, methods)
    }

    fn method_named(&self, method_name: &str) -> TreeId<Untyped> {
        self.parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if self.store.names.resolve(definition.name.as_name().text())
                        == method_name =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source method {method_name} is missing"))
    }

    fn source_type_symbol(&self, expected: &str) -> SymbolId {
        self.parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if self.store.names.resolve(definition.name.as_name().text()) == expected =>
                {
                    self.index.symbol_at(self.source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source type {expected} is missing"))
    }

    fn with_typer<R>(
        &mut self,
        action: impl FnOnce(
            SourceId,
            &dotty_parser::ParseResult,
            &dotty_core::SourceSemanticIndex,
            Definitions,
            &mut SourceTyper<'_>,
        ) -> R,
    ) -> R {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../dotty-tasty/tests/fixtures/scala3-library");
        let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
        let classpath = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
            Box::new(
                JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules"),
            ),
        ]);
        let resolver =
            ClasspathSymbolResolver::new(classpath, self.definitions, LoadingSession::new());
        let mut typer = SourceTyper::new(
            &self.parsed.ast,
            self.source,
            &self.index,
            &mut self.store,
            self.definitions,
            &self.packages,
        )
        .with_resolver(Box::new(resolver));
        action(
            self.source,
            &self.parsed,
            &self.index,
            self.definitions,
            &mut typer,
        )
    }

    fn with_typer_without_resolver<R>(
        &mut self,
        action: impl FnOnce(
            SourceId,
            &dotty_parser::ParseResult,
            &dotty_core::SourceSemanticIndex,
            Definitions,
            &mut SourceTyper<'_>,
        ) -> R,
    ) -> R {
        let mut typer = SourceTyper::new(
            &self.parsed.ast,
            self.source,
            &self.index,
            &mut self.store,
            self.definitions,
            &self.packages,
        );
        action(
            self.source,
            &self.parsed,
            &self.index,
            self.definitions,
            &mut typer,
        )
    }
}
