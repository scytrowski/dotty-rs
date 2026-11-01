use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::{TreeKind, TypedAst};
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Packages, SemanticStore, SourceId, SourceText,
    SourceTypedIndex, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TreeId, Typed, Untyped,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/source-case-class-match-3.9.0/Positive.scala");
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
type MatchRoots = (SymbolId, Vec<(TreeId<Untyped>, TreeId<Untyped>)>);

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
}
