use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::{Literal, TreeKind, TypedAst};
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Name, Namespace, Packages, SemanticStore, SourceId,
    SourceText, SourceTypedIndex, SymbolId, SymbolInfo, SymbolKind, SymbolOrigin, TreeId, Typed,
    Untyped,
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
fn flagged_product2_late_case_failure_rolls_back_before_valid_sibling() {
    let text = FLAGGED_SOURCE.replace(
        "if enabled then value else 0",
        "if enabled then true else 0",
    );
    let source = SourceId::from_index(1971);
    let mut session = Session::new(&text, source);
    let select = session.method_named("select");
    let classify = session.method_named("classify");
    let select_symbol = session.index.symbol_at(source, select).unwrap();
    let then_branch = {
        let TreeKind::DefDef(definition) = &session.parsed.ast.get(select).kind else {
            panic!("select source method")
        };
        let TreeKind::Match(match_node) = &session.parsed.ast.get(definition.rhs.unwrap()).kind
        else {
            panic!("select source Match")
        };
        let TreeKind::CaseDef(case) = &session.parsed.ast.get(match_node.cases[0]).kind else {
            panic!("select source CaseDef")
        };
        let TreeKind::Block(block) = &session.parsed.ast.get(case.body).kind else {
            panic!("select source body block")
        };
        let TreeKind::If(if_node) = &session.parsed.ast.get(block.expr).kind else {
            panic!("select source If")
        };
        if_node.then_branch
    };
    let class = flagged_class_symbol(&session, source);
    let unapply = session
        .index
        .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
        .unwrap();
    session.with_typer(|_, _, index, _, typer| {
        let before_store = typer.store().checkpoint();
        let before_typed = typer.typed_ast().checkpoint();
        let before_mappings = typer.source_typed_index().len();
        let error = typer
            .type_method_definition(select)
            .expect_err("the Boolean then-branch violates the declared Int result");
        assert!(matches!(
            error,
            TyperError::ExpectedExpressionTypeMismatch { .. }
        ));
        assert_eq!(typer.store().checkpoint(), before_store);
        assert_eq!(typer.typed_ast().checkpoint(), before_typed);
        assert_eq!(typer.source_typed_index().len(), before_mappings);
        assert!(typer.source_typed_index().get(source, select).is_none());
        assert_eq!(
            *typer.store().symbols.info(select_symbol),
            SymbolInfo::Missing
        );
        assert_eq!(*typer.store().symbols.info(unapply), SymbolInfo::Missing);
        assert_eq!(
            index.generated_case_class_method_identity(unapply),
            Some(dotty_core::GeneratedCaseClassMethod {
                class,
                role: GeneratedCaseClassMethodRole::Unapply,
            })
        );
    });
    let broken_branch = session.parsed.ast.get_mut(then_branch);
    broken_branch.kind = TreeKind::Literal(Literal {
        value: Constant::Int(1),
    });
    session.with_typer(|_, _, _, _, typer| {
        let retried = typer
            .type_method_definition(select)
            .expect("same semantic session retries after repairing the failing source node");
        assert!(matches!(
            typer.typed_ast().get(retried).kind,
            TreeKind::DefDef(_)
        ));
        assert!(typer.source_typed_index().get(source, select).is_some());
        assert!(matches!(
            *typer.store().symbols.info(select_symbol),
            SymbolInfo::Complete(_)
        ));
        let sibling = typer
            .type_method_definition(classify)
            .expect("valid sibling remains typeable after the successful retry");
        assert!(matches!(
            typer.typed_ast().get(sibling).kind,
            TreeKind::DefDef(_)
        ));
    });
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
