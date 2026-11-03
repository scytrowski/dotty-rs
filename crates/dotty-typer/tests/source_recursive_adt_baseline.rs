use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::TreeKind;
use dotty_core::types::{Constant, Type, TypeRefTarget};
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Packages, SemanticStore, SourceId, SourceText,
    SymbolFlags, SymbolInfo, SymbolKind, SymbolOrigin, TypeName,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/source-recursive-adt-match-3.9.0/Expr.scala");

struct UnwrapProofContext<'a> {
    source_ast: &'a dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    source: SourceId,
    index: &'a dotty_core::SourceSemanticIndex,
    typer: &'a SourceTyper<'a>,
    source_method: dotty_core::TreeId<dotty_core::ast::Untyped>,
    method: dotty_core::SymbolId,
    parameter: dotty_core::SymbolId,
    typed_method: dotty_core::TreeId<dotty_core::ast::Typed>,
}

#[test]
fn evaluator_recursive_adt_roots_type_after_generated_unapply_completion() {
    let (first, first_paths, first_unwrap, first_eval) = analyze_roots(SourceId::from_index(20341));
    let (second, second_paths, second_unwrap, second_eval) =
        analyze_roots(SourceId::from_index(20342));
    assert_eq!(first, second, "fresh source sessions normalize identically");
    assert_eq!(
        first_paths, second_paths,
        "deferred probes normalize identically"
    );
    assert_eq!(
        first_unwrap, second_unwrap,
        "unwrap normalizes across fresh sessions"
    );
    assert_eq!(
        first_eval, second_eval,
        "eval normalizes across fresh sessions"
    );
    for (session, roots) in [("first", &first), ("second", &second)] {
        for root in ["unwrap", "eval"] {
            let row = roots
                .iter()
                .find(|row| row.starts_with(&format!("{root}\t")))
                .expect("one original source-owned root per method");
            let path = row
                .split('\t')
                .find_map(|field| field.strip_prefix("path="))
                .expect("source path and parser tree identity");
            let span = row
                .split('\t')
                .find_map(|field| field.strip_prefix("source_bytes="))
                .expect("original source span");
            let proof = match root {
                "unwrap" => concat!(
                    "owner=adt.Evaluator$|signature=(Expr):Expr|selector=Expr|",
                    "cases=Num(_)=>expr:Expr,Succ(inner:Expr)=>inner:Expr|",
                    "extractors=Num$.unapply:Num,Succ$.unapply:Succ|",
                    "parents=Num:Expr+Product,Succ:Expr+Product|",
                    "fields=Num._1:Int,Succ._1:Expr|",
                    "mappings=DefDef,formal,Match,selector,CaseDef[0..1],UnApply[0..1],",
                    "qualifier[0..1],wildcard,binder,body[0..1]|result=Expr"
                ),
                "eval" => concat!(
                    "owner=adt.Evaluator$|signature=(Expr):Int|selector=Expr|",
                    "cases=Num(value:Int)=>value:Int,Succ(inner:Expr)=>eval(inner):Int+1:Int|",
                    "extractors=Num$.unapply:Num,Succ$.unapply:Succ|",
                    "parents=Num:Expr+Product,Succ:Expr+Product|",
                    "fields=Num._1:Int,Succ._1:Expr|",
                    "recursive=Evaluator.eval same Method SymbolId;argument=Succ.inner:Expr;result=Int|",
                    "mappings=DefDef,formal,Match,selector,CaseDef[0..1],UnApply[0..1],",
                    "qualifier[0..1],binder,body[0..1],recursive-function,argument,call,",
                    "addition,literal|result=Int"
                ),
                _ => unreachable!(),
            };
            println!("ISSUE2040_POSITIVE\t{session}\t{root}\t{path}\t{span}\t{proof}");
        }
    }
    assert_eq!(
        first.len(),
        2,
        "only unwrap and eval count as whole DefDef roots"
    );
    let actual = format!("{}\n", first.join("\n"));
    let expected = include_str!(
        "fixtures/source-recursive-adt-match-3.9.0/rust-two-root-after-generated-unapply.tsv"
    )
    .split_once('\n')
    .expect("two-root receipt header")
    .1;
    assert_eq!(actual, expected);
    for row in first {
        println!("ISSUE2034_ROOT\t{row}");
    }
    let actual = format!("{}\n", first_paths.join("\n"));
    let expected = include_str!(
        "fixtures/source-recursive-adt-match-3.9.0/rust-paths-after-generated-unapply.tsv"
    )
    .split_once('\n')
    .expect("deferred path header")
    .1;
    assert_eq!(actual, expected);
    for row in first_paths {
        println!("ISSUE2034_DEFERRED\t{row}");
    }
    let actual = format!("{first_unwrap}\n");
    let expected =
        include_str!("fixtures/source-recursive-adt-match-3.9.0/rust-unwrap-before-after.tsv");
    assert_eq!(actual, expected);
    println!("ISSUE2037_UNWRAP\t{first_unwrap}");
    let actual = format!("{first_eval}\n");
    let expected =
        include_str!("fixtures/source-recursive-adt-match-3.9.0/rust-eval-before-after.tsv")
            .lines()
            .nth(2)
            .and_then(|row| row.split_once('\t').map(|(_, receipt)| receipt))
            .expect("after row in recursive eval receipt");
    assert_eq!(actual.trim_end(), expected);
    println!("ISSUE2038_EVAL\t{first_eval}");
}

#[test]
fn evaluator_recursive_adt_classification_records_canonical_mechanisms() {
    let rows = characterize_classes(SourceId::from_index(20343), false, SOURCE);
    assert_eq!(
        rows,
        characterize_classes(SourceId::from_index(20344), true, SOURCE),
        "independent sessions and class-first/member-lookup-first completion agree"
    );
    let actual = format!("{}\n", rows.join("\n"));
    let expected = include_str!(
        "fixtures/source-recursive-adt-match-3.9.0/rust-class-evidence-after-generated-unapply.tsv"
    )
    .split_once('\n')
    .expect("class evidence header")
    .1;
    assert_eq!(actual, expected);
    for row in rows {
        println!("ISSUE2034_CLASS\t{row}");
    }
}

#[test]
fn evaluator_eval_types_after_unwrap_with_reordered_source_declarations() {
    let original = eval_after_unwrap_receipt(SourceId::from_index(20348), SOURCE);
    let reordered = SOURCE
        .replace(
            "final case class Num(value: Int) extends Expr\nfinal case class Succ(inner: Expr) extends Expr",
            "final case class Succ(inner: Expr) extends Expr\nfinal case class Num(value: Int) extends Expr",
        )
        .replace(
            "  def unwrap(expr: Expr): Expr =\n    expr match\n      case Num(_)      => expr\n      case Succ(inner) => inner\n\n  def eval(expr: Expr): Int =\n    expr match\n      case Num(value)  => value\n      case Succ(inner) => eval(inner) + 1",
            "  def eval(expr: Expr): Int =\n    expr match\n      case Num(value)  => value\n      case Succ(inner) => eval(inner) + 1\n\n  def unwrap(expr: Expr): Expr =\n    expr match\n      case Num(_)      => expr\n      case Succ(inner) => inner",
        );
    let reordered = eval_after_unwrap_receipt(SourceId::from_index(20349), &reordered);
    assert_eq!(
        normalize_eval_receipt(&original),
        normalize_eval_receipt(&reordered),
        "source class and method declaration order do not change recursive eval semantics"
    );
}

fn normalize_eval_receipt(receipt: &str) -> String {
    receipt
        .split('\t')
        .enumerate()
        .filter_map(|(index, field)| (!matches!(index, 1..=4)).then_some(field))
        .collect::<Vec<_>>()
        .join("\t")
}

fn eval_after_unwrap_receipt(source: SourceId, source_text: &str) -> String {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).expect("valid reordered source"),
        source,
        ContextualScanner::new(source_text).expect("scan reordered source"),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Expr.scala",
        &mut store,
        &mut packages,
    )
    .expect("name reordered recursive source");
    let mut methods = std::collections::BTreeMap::new();
    for (tree, node) in parsed.ast.iter() {
        if let TreeKind::DefDef(definition) = &node.kind {
            let name = store.names.resolve(definition.name.as_name().text());
            if matches!(name, "eval" | "unwrap") {
                methods.insert(
                    name.to_owned(),
                    (tree, index.symbol_at(source, tree).unwrap()),
                );
            }
        }
    }
    let (unwrap_tree, _) = methods["unwrap"];
    let (eval_tree, eval_symbol) = methods["eval"];
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    let resolver = ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new());
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    let typed_unwrap = typer
        .type_method_definition(unwrap_tree)
        .expect("unwrap completes before eval in this session");
    assert!(matches!(
        typer.typed_ast().get(typed_unwrap).kind,
        TreeKind::DefDef(_)
    ));
    let typed_eval = typer
        .type_method_definition(eval_tree)
        .expect("the explicitly annotated recursive eval completes after unwrap");
    let parameter = match &parsed.ast.get(eval_tree).kind {
        TreeKind::DefDef(definition) => index
            .symbol_at(source, definition.value_param_clauses[0][0])
            .expect("canonical source eval formal"),
        _ => unreachable!(),
    };
    assert_complete_eval(
        &parsed.ast,
        source,
        &index,
        &typer,
        eval_tree,
        eval_symbol,
        parameter,
        typed_eval,
        definitions.int,
    )
}

#[test]
fn recursive_eval_late_failure_rolls_back_and_allows_same_session_retry() {
    let source_text = format!(
        "{SOURCE}\nobject BrokenEvaluator:\n  def evalBroken(expr: Expr): Int =\n    expr match\n      case Num(value)  => value\n      case Succ(inner) => evalBroken(inner) + true\n"
    );
    let source = SourceId::from_index(20347);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(&source_text).expect("valid late-failure source"),
        source,
        ContextualScanner::new(&source_text).expect("scan late-failure source"),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Expr.scala",
        &mut store,
        &mut packages,
    )
    .expect("name recursive eval retry source");
    let mut methods = std::collections::BTreeMap::new();
    for (tree, node) in parsed.ast.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let name = store.names.resolve(definition.name.as_name().text());
        if matches!(name, "eval" | "unwrap" | "evalBroken") {
            methods.insert(
                name.to_owned(),
                (tree, index.symbol_at(source, tree).unwrap()),
            );
        }
    }
    let (broken_tree, broken_symbol) = methods["evalBroken"];
    let (eval_tree, _) = methods["eval"];
    let (unwrap_tree, _) = methods["unwrap"];
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    let resolver = ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new());
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));

    let store_before = typer.store().checkpoint();
    let typed_before = typer.typed_ast().checkpoint();
    let broken_info_before = *typer.store().symbols.info(broken_symbol);
    let first_error = typer
        .type_method_definition(broken_tree)
        .expect_err("the trailing Boolean operand must fail after the recursive call");
    let first_diagnostic = format!("{first_error:?}");
    assert_eq!(typer.store().checkpoint(), store_before);
    assert_eq!(typer.typed_ast().checkpoint(), typed_before);
    assert_eq!(
        *typer.store().symbols.info(broken_symbol),
        broken_info_before
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, broken_tree)
            .is_none()
    );
    let TreeKind::DefDef(broken_definition) = &parsed.ast.get(broken_tree).kind else {
        unreachable!()
    };
    let broken_rhs = broken_definition.rhs.unwrap();
    let TreeKind::Match(broken_match) = &parsed.ast.get(broken_rhs).kind else {
        unreachable!()
    };
    let TreeKind::CaseDef(broken_case) = &parsed.ast.get(broken_match.cases[1]).kind else {
        unreachable!()
    };
    let TreeKind::Block(broken_body) = &parsed.ast.get(broken_case.body).kind else {
        unreachable!()
    };
    let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(broken_infix)) =
        &parsed.ast.get(broken_body.expr).kind
    else {
        unreachable!()
    };
    assert!(
        matches!(
            &first_error,
            TyperError::OverloadApplicationNoApplicable {
                source: error_source,
                tree_index,
                ..
            } if *error_source == source && *tree_index == broken_body.expr.index()
        ),
        "{first_diagnostic}"
    );
    assert!(
        first_diagnostic.contains("ArgumentNonConformance"),
        "{first_diagnostic}"
    );
    let recursive_call = broken_infix.left;
    assert!(
        typer
            .source_typed_index()
            .get(source, recursive_call)
            .is_none()
    );

    let second_error = typer
        .type_method_definition(broken_tree)
        .expect_err("repeating the malformed method remains an error");
    assert_eq!(format!("{second_error:?}"), first_diagnostic);
    assert_eq!(typer.store().checkpoint(), store_before);
    assert_eq!(typer.typed_ast().checkpoint(), typed_before);
    assert_eq!(
        *typer.store().symbols.info(broken_symbol),
        broken_info_before
    );
    assert!(
        typer
            .source_typed_index()
            .get(source, broken_tree)
            .is_none()
    );

    let typed_eval = typer
        .type_method_definition(eval_tree)
        .expect("valid original eval still types in the same session");
    let typed_unwrap = typer
        .type_method_definition(unwrap_tree)
        .expect("valid unwrap sibling still types in the same session");
    assert!(matches!(
        typer.typed_ast().get(typed_eval).kind,
        TreeKind::DefDef(_)
    ));
    assert!(matches!(
        typer.typed_ast().get(typed_unwrap).kind,
        TreeKind::DefDef(_)
    ));
    assert_eq!(
        typer.source_typed_index().get(source, eval_tree),
        Some(typed_eval)
    );
    assert_eq!(
        typer.source_typed_index().get(source, unwrap_tree),
        Some(typed_unwrap)
    );
}

#[test]
fn recursive_adt_generated_unapply_is_independent_of_declaration_order() {
    let reordered = SOURCE
        .replace(
            "final case class Num(value: Int) extends Expr\nfinal case class Succ(inner: Expr) extends Expr",
            "final case class Succ(inner: Expr) extends Expr\nfinal case class Num(value: Int) extends Expr",
        );
    let original = characterize_classes(SourceId::from_index(20345), false, SOURCE);
    let reordered = characterize_classes(SourceId::from_index(20346), false, &reordered);
    assert_eq!(
        normalized_signatures(&original),
        normalized_signatures(&reordered),
        "class declaration order cannot change canonical unapply input/results"
    );
}

fn normalized_signatures(rows: &[String]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            let name = row.split('\t').next()?;
            if !matches!(name, "Num" | "Succ") {
                return None;
            }
            let class_symbol = row.split("symbol=").nth(1)?.split('\t').next()?;
            let signature = row.split("\ttype=").nth(1)?.split('\t').next()?;
            assert_eq!(signature, format!("({class_symbol}):{class_symbol}"));
            Some(format!("{name}:(class):class"))
        })
        .collect()
}

fn characterize_classes(
    source: SourceId,
    complete_class_first: bool,
    source_text: &str,
) -> Vec<String> {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(source_text).expect("valid source"),
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
        "Expr.scala",
        &mut store,
        &mut packages,
    )
    .expect("name frozen Expr source");
    let mut declarations = Vec::new();
    let mut expr = None;
    for (tree, node) in parsed.ast.iter() {
        let TreeKind::TypeDef(definition) = &node.kind else {
            continue;
        };
        let name = store.names.resolve(definition.name.as_name().text());
        let symbol = index.symbol_at(source, tree).expect("source type symbol");
        if name == "Expr" {
            expr = Some((symbol, tree));
        }
        if matches!(name, "Num" | "Succ") {
            declarations.push((name.to_owned(), symbol, tree, definition.rhs));
        }
    }
    declarations.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(declarations.len(), 2);
    let (expr_symbol, expr_tree) = expr.expect("source Expr trait");
    assert_eq!(store.symbols.get(expr_symbol).kind, SymbolKind::Trait);
    assert!(
        matches!(store.symbols.get(expr_symbol).origin, SymbolOrigin::Source(id) if id == source)
    );

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    let product_fixture = fixture.join("scala/Product.tasty");
    assert!(product_fixture.is_file());
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    let resolver = ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new());
    let expr_name = TypeName::new(store.names.intern("Expr"));
    let companion_names = declarations
        .iter()
        .map(|(name, ..)| {
            (
                name.clone(),
                TypeName::new(store.names.intern(&format!("{name}$"))),
            )
        })
        .collect::<Vec<_>>();
    let unapply_name = dotty_core::TermName::new(store.names.intern("unapply"));
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    let package = typer.store().symbols.get(expr_symbol).owner.unwrap();
    let package_scope = index.scope_of(package).expect("source package scope");
    assert_eq!(
        typer
            .store()
            .scopes
            .get(package_scope)
            .lookup(expr_name.as_name()),
        Some(expr_symbol),
        "source parent resolves to the canonical Expr trait"
    );
    let mut rows = Vec::new();
    let mut generated_formals = Vec::new();
    rows.push(format!(
        "Expr\ttree={}\tsymbol={expr_symbol:?}\tkind=Trait\torigin=Source\tsealed={}",
        expr_tree.index(),
        typer
            .store()
            .symbols
            .get(expr_symbol)
            .flags
            .contains(dotty_core::SymbolFlags::SEALED)
    ));
    for (name, class, tree, rhs) in declarations {
        assert_eq!(typer.store().symbols.get(class).kind, SymbolKind::Class);
        assert!(
            typer
                .store()
                .symbols
                .get(class)
                .flags
                .contains(dotty_core::SymbolFlags::CASE)
        );
        assert!(
            matches!(typer.store().symbols.get(class).origin, SymbolOrigin::Source(id) if id == source)
        );
        let TreeKind::Template(template) = &parsed.ast.get(rhs).kind else {
            panic!("{name} body is not a source template");
        };
        assert_eq!(template.parents.len(), 1);
        let TreeKind::Ident(parent_ident) = &parsed.ast.get(template.parents[0]).kind else {
            panic!("{name} parent is not a simple identifier");
        };
        assert_eq!(parent_ident.name, *expr_name.as_name());
        let source_parent = typer
            .store()
            .scopes
            .get(package_scope)
            .lookup(&parent_ident.name)
            .expect("canonical source parent identity");
        assert_eq!(source_parent, expr_symbol);

        let companion = typer
            .store()
            .symbols
            .get(class)
            .links
            .companion
            .expect("case companion");
        let module_name = companion_names
            .iter()
            .find(|(candidate, _)| candidate == &name)
            .unwrap()
            .1;
        let module_class = typer
            .store()
            .scopes
            .get(package_scope)
            .lookup(module_name.as_name())
            .expect("generated companion module class");
        let unapply = index
            .generated_case_class_method(class, GeneratedCaseClassMethodRole::Unapply)
            .expect("canonical generated unapply identity");
        assert_ne!(unapply, class);
        assert_eq!(typer.store().symbols.get(unapply).owner, Some(module_class));
        if complete_class_first {
            typer
                .complete_symbol(class)
                .expect("class-first source completion succeeds");
        }
        assert_eq!(
            typer
                .store()
                .scopes
                .get(
                    index
                        .scope_of(module_class)
                        .expect("companion member scope")
                )
                .lookup(unapply_name.as_name()),
            Some(unapply),
            "ordinary companion member lookup returns the canonical generated method"
        );
        let formal = index
            .derived_symbol_at(unapply, source, tree)
            .expect("canonical generated unapply formal derived from case-class tree");
        let formal_symbol = typer.store().symbols.get(formal);
        assert_eq!(formal_symbol.kind, SymbolKind::Parameter);
        assert!(formal_symbol.flags.contains(SymbolFlags::SYNTHETIC));
        assert_eq!(formal_symbol.owner, Some(unapply));
        assert_eq!(formal_symbol.origin, SymbolOrigin::Source(source));
        assert_ne!(formal, unapply);
        assert_ne!(formal, class);
        assert!(!generated_formals.contains(&formal));
        generated_formals.push(formal);
        let unapply_scope = index
            .scope_of(unapply)
            .expect("generated unapply parameter scope");
        assert_eq!(
            typer
                .store()
                .scopes
                .get(unapply_scope)
                .lookup(&formal_symbol.name),
            Some(formal),
            "the generated unapply formal resolves to its canonical derived identity"
        );
        let unapply_type = typer
            .complete_symbol(unapply)
            .expect("ordinary generated unapply completion");
        assert_eq!(
            typer.complete_symbol(unapply).unwrap(),
            unapply_type,
            "repeated generated method completion is idempotent"
        );
        let unapply_shape = match typer.store().types.get(unapply_type) {
            Type::Method(method) => {
                assert_eq!(method.params.len(), 1);
                let arg = type_ref_symbol(&typer, method.params[0].ty);
                let result = type_ref_symbol(&typer, method.result);
                assert_eq!(
                    arg, class,
                    "generated formal input targets canonical case class"
                );
                assert_eq!(
                    result, class,
                    "generated unapply result targets canonical case class"
                );
                format!("({arg:?}):{result:?}")
            }
            other => format!("unexpected={other:?}"),
        };

        let constructor = &parsed.ast.get(template.constructor).kind;
        let TreeKind::DefDef(constructor) = constructor else {
            panic!("{name} constructor is not a DefDef");
        };
        assert_eq!(constructor.value_param_clauses.len(), 1);
        let field_tree = constructor.value_param_clauses[0][0];
        let field = index
            .symbol_at(source, field_tree)
            .expect("source constructor field");
        assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(typer.store().symbols.get(field).owner, Some(class));
        assert_eq!(field, index.symbol_at(source, field_tree).unwrap());
        let field_name = typer
            .store()
            .names
            .resolve(typer.store().symbols.get(field).name.text())
            .to_owned();
        let selector = index
            .derived_symbol_at(class, source, field_tree)
            .expect("canonical Product1 selector derived from source field");
        assert_eq!(typer.store().symbols.get(selector).owner, Some(class));
        assert_ne!(
            selector, field,
            "source field and synthetic _1 stay distinct"
        );
        assert_ne!(selector, formal, "generated unapply formal stays distinct");
        assert_ne!(
            formal, field,
            "generated unapply formal is not the source field"
        );
        let selector_shape = match typer.complete_symbol(selector) {
            Ok(ty) => match typer.store().types.get(ty) {
                Type::Method(method) => {
                    let result = type_ref_symbol(&typer, method.result);
                    assert!(method.params.is_empty(), "case selector is parameterless");
                    if name == "Succ" {
                        assert_eq!(result, expr_symbol);
                    }
                    if name == "Num" {
                        let int = type_ref_symbol(&typer, definitions.int);
                        assert_eq!(result, int, "Num._1 targets canonical scala.Int");
                    }
                    format!("_1:{result:?}")
                }
                other => format!("unexpected={other:?}"),
            },
            Err(error) => format!("error={error:?}"),
        };
        let class_info = match typer.complete_symbol(class) {
            Ok(ty) => match typer.store().types.get(ty) {
                Type::ClassInfo(info) => {
                    let parent_symbols = info
                        .parents
                        .iter()
                        .map(|parent| type_ref_symbol(&typer, *parent))
                        .collect::<Vec<_>>();
                    assert_eq!(parent_symbols.len(), 3);
                    assert_eq!(parent_symbols[0], definitions.object_class);
                    assert_eq!(parent_symbols[1], expr_symbol);
                    let product = parent_symbols[2];
                    let product_symbol = typer.store().symbols.get(product);
                    assert_eq!(
                        typer.store().names.resolve(product_symbol.name.text()),
                        "Product"
                    );
                    assert_eq!(product_symbol.kind, SymbolKind::Trait);
                    assert!(matches!(product_symbol.origin, SymbolOrigin::Tasty(_)));
                    assert_eq!(
                        parent_symbols
                            .iter()
                            .filter(|parent| **parent == product)
                            .count(),
                        1,
                        "exactly one canonical Product edge is retained"
                    );
                    format!("parents={parent_symbols:?}")
                }
                other => format!("unexpected={other:?}"),
            },
            Err(error) => format!("error={error:?}"),
        };
        rows.push(format!(
            "{name}\ttree={}\tsymbol={class:?}\tkind=CASE-Class\torigin=Source\tparent=Expr@tree{}=>{source_parent:?}\tcompanion={companion:?}\tmodule_class={module_class:?}\tunapply={unapply:?},owner={name}$,formal=x$1:{formal:?},formal_identity=Derived({unapply:?},source,{})\tformal_kind=Parameter,formal_synthetic=true,formal_origin=Source\ttype={unapply_shape}\tfield={field_name}:{field:?}\tselector={selector:?},{selector_shape}\tclass_info={class_info}",
            tree.index(), template.parents[0].index(), tree.index()
        ));
        if name == "Succ" {
            let field_type_tree = match &parsed.ast.get(field_tree).kind {
                TreeKind::ValDef(field) => field.tpt,
                _ => unreachable!(),
            };
            let TreeKind::Ident(field_type) = &parsed.ast.get(field_type_tree).kind else {
                panic!("Succ.inner type is not a simple source type reference");
            };
            assert_eq!(field_type.name, *expr_name.as_name());
            let field_type_target = typer
                .store()
                .scopes
                .get(package_scope)
                .lookup(&field_type.name)
                .expect("canonical Expr target of source Succ.inner type");
            assert_eq!(field_type_target, expr_symbol);
            assert_eq!(typer.store().symbols.get(field).owner, Some(class));
            rows.push(format!(
                "Succ.inner_type\ttree{}=>{field_type_target:?}\tfield={field:?}\tfield_owner={class:?}\ttype_completion=canonical_source_TypeRef",
                field_type_tree.index()
            ));
        }
    }
    rows
}

fn type_ref_symbol(typer: &SourceTyper<'_>, ty: dotty_core::TypeId) -> dotty_core::SymbolId {
    match typer.store().types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("expected canonical symbol TypeRef, got {other:?}"),
    }
}

fn analyze_roots(source: SourceId) -> (Vec<String>, Vec<String>, String, String) {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(SOURCE).expect("valid frozen source"),
        source,
        ContextualScanner::new(SOURCE).expect("scan frozen source"),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Expr.scala",
        &mut store,
        &mut packages,
    )
    .expect("name frozen Expr source");
    let mut roots = Vec::new();
    for (tree, node) in parsed.ast.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let name = store.names.resolve(definition.name.as_name().text());
        if !matches!(name, "unwrap" | "eval") {
            continue;
        }
        let symbol = index.symbol_at(source, tree).expect("source method symbol");
        let parameter_tree = definition.value_param_clauses[0][0];
        let parameter = index
            .symbol_at(source, parameter_tree)
            .expect("canonical source parameter symbol");
        let owner = store.symbols.get(symbol).owner.expect("method owner");
        assert_eq!(store.symbols.get(parameter).owner, Some(symbol));
        assert_eq!(
            store.names.resolve(store.symbols.get(owner).name.text()),
            "Evaluator$"
        );
        assert!(
            matches!(store.symbols.get(symbol).origin, SymbolOrigin::Source(id) if id == source)
        );
        assert!(
            matches!(store.symbols.get(parameter).origin, SymbolOrigin::Source(id) if id == source)
        );
        roots.push((
            name.to_owned(),
            tree,
            node.position.unwrap(),
            symbol,
            parameter,
            owner,
        ));
    }
    roots.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(roots.len(), 2, "generated unapply methods are excluded");
    let eval_symbol = roots
        .iter()
        .find(|(name, ..)| name == "eval")
        .map(|(_, _, _, symbol, _, _)| *symbol)
        .expect("original Evaluator.eval definition");
    let eval_owner = store.symbols.get(eval_symbol).owner.expect("eval owner");
    let eval_scope = index.scope_of(eval_owner).expect("Evaluator$ source scope");
    let eval_name = store.symbols.get(eval_symbol).name;
    assert_eq!(
        store.scopes.get(eval_scope).lookup(&eval_name),
        Some(eval_symbol),
        "the unshadowed recursive eval reference resolves to its canonical Evaluator method"
    );
    let eval_owner_name = store
        .names
        .resolve(store.symbols.get(eval_owner).name.text())
        .to_owned();

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-tasty/tests/fixtures/scala3-library");
    assert!(fixture.join("scala/Product.tasty").is_file());
    let java_home = std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required");
    let classpath = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(fixture)) as Box<dyn ClassPathEntry>,
        Box::new(JdkClassPath::new(PathBuf::from(java_home).join("jmods")).expect("JDK modules")),
    ]);
    let resolver = ClasspathSymbolResolver::new(classpath, definitions, LoadingSession::new());
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));
    let mut rows = Vec::new();
    let mut unwrap_receipt = None;
    let mut eval_receipt = None;
    let mut root_trees = Vec::new();
    for (name, tree, position, symbol, parameter, owner) in roots {
        let outcome = match typer.type_method_definition(tree) {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::DefDef(_)
                ));
                if name == "unwrap" {
                    unwrap_receipt = Some(assert_complete_unwrap(UnwrapProofContext {
                        source_ast: &parsed.ast,
                        source,
                        index: &index,
                        typer: &typer,
                        source_method: tree,
                        method: symbol,
                        parameter,
                        typed_method: typed,
                    }));
                } else {
                    eval_receipt = Some(assert_complete_eval(
                        &parsed.ast,
                        source,
                        &index,
                        &typer,
                        tree,
                        symbol,
                        parameter,
                        typed,
                        definitions.int,
                    ));
                    assert_eq!(
                        typer.type_method_definition(tree).unwrap(),
                        typed,
                        "retyping the same source method reuses its canonical typed root"
                    );
                }
                format!("complete=typed#tree{}", typed.index())
            }
            Err(error) => {
                assert!(typer.source_typed_index().get(source, tree).is_none());
                let error_tree = match &error {
                    TyperError::UnsupportedProduct1FieldType { class, tree_index } => {
                        assert_eq!(
                            typer
                                .store()
                                .names
                                .resolve(typer.store().symbols.get(*class).name.text()),
                            "Succ",
                            "the authentic first blocker concerns the source Succ class"
                        );
                        let (tree, node) = parsed
                            .ast
                            .iter()
                            .find(|(candidate, _)| candidate.index() == *tree_index)
                            .expect("first blocker source tree");
                        let range = node.position.unwrap().span().range();
                        format!(
                            "Expr.scala#tree{}@{}:{}",
                            tree.index(),
                            range.start(),
                            range.end()
                        )
                    }
                    TyperError::MalformedMethodClause {
                        method,
                        method_tree_index,
                        clause_index: 0,
                    } => {
                        let generated_unapply = parsed
                            .ast
                            .iter()
                            .find_map(|(class_tree, node)| match &node.kind {
                                TreeKind::TypeDef(definition)
                                    if typer
                                        .store()
                                        .names
                                        .resolve(definition.name.as_name().text())
                                        == "Succ" =>
                                {
                                    index.symbol_at(source, class_tree).and_then(|class| {
                                        index.generated_case_class_method(
                                            class,
                                            GeneratedCaseClassMethodRole::Unapply,
                                        )
                                    })
                                }
                                _ => None,
                            })
                            .expect("canonical generated Succ.unapply");
                        assert_eq!(*method, generated_unapply);
                        let (class_tree, node) = parsed
                            .ast
                            .iter()
                            .find(|(candidate, _)| candidate.index() == *method_tree_index)
                            .expect("generated unapply blocker source tree");
                        let range = node.position.unwrap().span().range();
                        format!(
                            "Expr.scala#tree{}@{}:{}",
                            class_tree.index(),
                            range.start(),
                            range.end()
                        )
                    }
                    other => panic!("unexpected baseline first blocker for {name}: {other:?}"),
                };
                format!("first_error={error:?}(source_tree={error_tree});typed_index=absent")
            }
        };
        let range = position.span().range();
        rows.push(format!(
            "{name}\tpath=Expr.scala#tree{}\tmethod={symbol:?}\tparameter={parameter:?}\tsource_bytes={}:{}\towner=Evaluator$:{owner:?}\t{outcome}",
            tree.index(), range.start(), range.end(),
        ));
        root_trees.push((name, tree));
    }
    let mut deferred = Vec::new();
    for (name, root) in root_trees {
        let root_range = parsed.ast.get(root).position.unwrap().span().range();
        let TreeKind::DefDef(definition) = &parsed.ast.get(root).kind else {
            unreachable!()
        };
        let return_tree = definition.tpt;
        let rhs = definition.rhs.expect("original method body");
        let TreeKind::Match(matched) = &parsed.ast.get(rhs).kind else {
            panic!("{name} body is not the original match");
        };
        assert_eq!(matched.cases.len(), 2);
        let final_case = matched.cases[1];
        let TreeKind::CaseDef(case) = &parsed.ast.get(final_case).kind else {
            unreachable!()
        };
        let TreeKind::Apply(pattern) = &parsed.ast.get(case.pattern).kind else {
            panic!("{name} second pattern is not an extractor application");
        };
        let TreeKind::Ident(extractor) = &parsed.ast.get(pattern.function).kind else {
            panic!("{name} extractor is not a simple identifier");
        };
        assert_eq!(typer.store().names.resolve(extractor.name.text()), "Succ");
        let binder = pattern.args[0];
        let TreeKind::Ident(binder_name) = &parsed.ast.get(binder).kind else {
            panic!("{name} Succ argument is not the source binder identifier");
        };
        assert_eq!(
            typer.store().names.resolve(binder_name.name.text()),
            "inner"
        );
        let body = case.body;
        let TreeKind::Block(block) = &parsed.ast.get(body).kind else {
            panic!("{name} second case body is not a block");
        };
        let body_expr = block.expr;
        let body_range = parsed.ast.get(body_expr).position.unwrap().span().range();
        let typed_status = |tree| {
            typer
                .source_typed_index()
                .get(source, tree)
                .map(|typed| format!("typed#tree{}", typed.index()))
                .unwrap_or_else(|| "deferred".to_owned())
        };
        let return_range = parsed.ast.get(return_tree).position.unwrap().span().range();
        let binder_range = parsed.ast.get(binder).position.unwrap().span().range();
        if name == "unwrap" {
            deferred.push(format!(
                "unwrap\treturn=tree{}@{}:{}:Expr,{}\tbinder=tree{}@{}:{}:inner,{}\tbody=tree{}@{}:{}:inner,{}",
                return_tree.index(), return_range.start(), return_range.end(), typed_status(return_tree), binder.index(), binder_range.start(), binder_range.end(), typed_status(binder), body_expr.index(), body_range.start(), body_range.end(), typed_status(body_expr)
            ));
        } else {
            let recursive = parsed
                .ast
                .iter()
                .find_map(|(tree, node)| {
                    let range = node.position?.span().range();
                    if range.start() < root_range.start() || range.end() > root_range.end() {
                        return None;
                    }
                    let TreeKind::Apply(application) = &node.kind else {
                        return None;
                    };
                    let TreeKind::Ident(function) = &parsed.ast.get(application.function).kind
                    else {
                        return None;
                    };
                    (typer.store().names.resolve(function.name.text()) == "eval").then_some((
                        tree,
                        application.function,
                        application.args[0],
                    ))
                })
                .expect("original recursive eval(inner) call");
            let (call, function, argument) = recursive;
            let call_range = parsed.ast.get(call).position.unwrap().span().range();
            let function_range = parsed.ast.get(function).position.unwrap().span().range();
            let argument_range = parsed.ast.get(argument).position.unwrap().span().range();
            deferred.push(format!(
                "eval\treturn=tree{}@{}:{}:Int,{}\tbinder=tree{}@{}:{}:inner,{}\tcall=tree{}@{}:{}:eval(inner),namer_owner={eval_owner_name}:{eval_symbol:?},typed_result={}\tfunction=tree{}@{}:{}:eval,{}\targument=tree{}@{}:{}:inner,{}",
                return_tree.index(), return_range.start(), return_range.end(), typed_status(return_tree), binder.index(), binder_range.start(), binder_range.end(), typed_status(binder), call.index(), call_range.start(), call_range.end(), typed_status(call), function.index(), function_range.start(), function_range.end(), typed_status(function), argument.index(), argument_range.start(), argument_range.end(), typed_status(argument)
            ));
        }
    }
    deferred.sort();
    (
        rows,
        deferred,
        unwrap_receipt.expect("the original Evaluator.unwrap root was typed"),
        eval_receipt.expect("the original Evaluator.eval root was typed"),
    )
}

#[allow(clippy::too_many_arguments)]
fn assert_complete_eval(
    source_ast: &dotty_core::ast::AstArena<dotty_core::ast::Untyped>,
    source: SourceId,
    index: &dotty_core::SourceSemanticIndex,
    typer: &SourceTyper<'_>,
    source_method: dotty_core::TreeId<dotty_core::ast::Untyped>,
    method: dotty_core::SymbolId,
    parameter: dotty_core::SymbolId,
    typed_method: dotty_core::TreeId<dotty_core::ast::Typed>,
    int_type: dotty_core::TypeId,
) -> String {
    let store = typer.store();
    let owner = store.symbols.get(method).owner.expect("eval owner");
    assert_eq!(store.symbols.get(owner).kind, SymbolKind::ModuleClass);
    assert_eq!(
        store.names.resolve(store.symbols.get(owner).name.text()),
        "Evaluator$"
    );
    assert!(matches!(store.symbols.get(method).origin, SymbolOrigin::Source(id) if id == source));
    assert_eq!(store.symbols.get(method).kind, SymbolKind::Method);
    assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
    assert_eq!(store.symbols.get(parameter).owner, Some(method));

    let SymbolInfo::Complete(method_type) = *store.symbols.info(method) else {
        panic!("eval signature is completed before the body is accepted")
    };
    let Type::Method(signature) = store.types.get(method_type) else {
        panic!("eval has a method signature")
    };
    assert_eq!(signature.params.len(), 1);
    let SymbolInfo::Complete(parameter_type) = *store.symbols.info(parameter) else {
        panic!("eval formal has a completed type")
    };
    assert_eq!(
        denoted_symbol(typer, parameter_type),
        denoted_symbol(typer, signature.params[0].ty)
    );
    let int_symbol = denoted_symbol(typer, int_type).expect("canonical Int symbol");
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(int_symbol).name.text()),
        "Int"
    );
    assert_eq!(denoted_symbol(typer, signature.result), Some(int_symbol));

    let TreeKind::DefDef(source_def) = &source_ast.get(source_method).kind else {
        panic!("source root is a DefDef")
    };
    let expr_symbol = source_ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "Expr" =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .expect("canonical source Expr trait");
    assert_eq!(
        denoted_symbol(typer, signature.params[0].ty),
        Some(expr_symbol)
    );
    let source_rhs = source_def.rhs.expect("eval has a body");
    let TreeKind::Match(source_match) = &source_ast.get(source_rhs).kind else {
        panic!("eval body is the original Match")
    };
    assert_eq!(source_match.cases.len(), 2);
    let mapped = |tree| {
        let typed = typer
            .source_typed_index()
            .get(source, tree)
            .unwrap_or_else(|| panic!("missing typed mapping for {tree:?}"));
        assert_eq!(
            source_ast.get(tree).position,
            typer.typed_ast().get(typed).position,
            "source span survives typing for {tree:?}"
        );
        typed
    };
    assert_eq!(mapped(source_method), typed_method);
    let typed_def = match &typer.typed_ast().get(typed_method).kind {
        TreeKind::DefDef(definition) => definition,
        other => panic!("typed root remains a DefDef: {other:?}"),
    };
    assert_eq!(typed_def.rhs, Some(mapped(source_rhs)));
    let source_parameter = source_def.value_param_clauses[0][0];
    assert_eq!(
        typed_def.value_param_clauses[0][0],
        mapped(source_parameter),
        "the explicit formal has a source-to-typed mapping"
    );
    let typed_result = mapped(source_def.tpt);
    assert_eq!(
        denoted_symbol(typer, typer.typed_ast().get(typed_result).ty),
        Some(int_symbol)
    );
    let typed_match_id = mapped(source_rhs);
    let typed_match = match &typer.typed_ast().get(typed_match_id).kind {
        TreeKind::Match(matched) => matched,
        other => panic!("typed RHS remains a Match: {other:?}"),
    };
    assert_eq!(typed_match.cases.len(), 2);
    assert_eq!(mapped(source_match.selector), typed_match.selector);
    assert_eq!(typed_symbol(typer, typed_match.selector), parameter);
    assert_eq!(
        denoted_symbol(typer, typer.typed_ast().get(typed_match_id).ty),
        Some(int_symbol)
    );

    let mut case_receipts = Vec::new();
    let mut selected = Vec::new();
    let mut case_binders = Vec::new();
    let mut recursive_receipt = None;
    for (case_index, (source_case_id, typed_case_id)) in source_match
        .cases
        .iter()
        .zip(&typed_match.cases)
        .enumerate()
    {
        assert_eq!(mapped(*source_case_id), *typed_case_id);
        let TreeKind::CaseDef(source_case) = &source_ast.get(*source_case_id).kind else {
            panic!("source case is a CaseDef")
        };
        let TreeKind::CaseDef(typed_case) = &typer.typed_ast().get(*typed_case_id).kind else {
            panic!("typed case is a CaseDef")
        };
        assert_eq!(typed_case.guard, None);
        assert_eq!(mapped(source_case.pattern), typed_case.pattern);
        assert_eq!(mapped(source_case.body), typed_case.body);
        let TreeKind::Apply(source_extractor) = &source_ast.get(source_case.pattern).kind else {
            panic!("source case uses an extractor application")
        };
        let TreeKind::UnApply(typed_extractor) = &typer.typed_ast().get(typed_case.pattern).kind
        else {
            panic!("typed pattern is an UnApply")
        };
        assert_eq!(source_extractor.args.len(), 1);
        assert_eq!(typed_extractor.patterns.len(), 1);
        let TreeKind::Select(typed_extractor_selection) =
            &typer.typed_ast().get(typed_extractor.function).kind
        else {
            panic!("typed extractor is selected from its authentic companion")
        };
        assert_eq!(
            mapped(source_extractor.function),
            typed_extractor_selection.qualifier,
            "the source extractor reference is retained through typing"
        );
        let variant_name = if case_index == 0 { "Num" } else { "Succ" };
        let variant = source_ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == variant_name =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .expect("canonical variant symbol");
        assert_eq!(
            denoted_symbol(typer, typer.typed_ast().get(typed_case.pattern).ty),
            Some(variant)
        );
        let unapply = index
            .generated_case_class_method(variant, GeneratedCaseClassMethodRole::Unapply)
            .expect("authentic source-generated unapply");
        let selected_symbol = typed_symbol(typer, typed_extractor.function);
        assert_eq!(selected_symbol, unapply);
        selected.push(format!("{variant_name}$.unapply@{variant_name}$"));

        let source_component = source_extractor.args[0];
        let typed_component = typed_extractor.patterns[0];
        assert_eq!(mapped(source_component), typed_component);
        let TreeKind::Bind(binding) = &typer.typed_ast().get(typed_component).kind else {
            panic!("each one-field extractor binds its source component")
        };
        let binder = typed_symbol(typer, typed_component);
        case_binders.push(binder);
        let binder_name = store.names.resolve(binding.name.text()).to_owned();
        let SymbolInfo::Complete(binder_type) = *store.symbols.info(binder) else {
            panic!("pattern binder has a complete type")
        };
        let expected_binder_type = if case_index == 0 {
            int_symbol
        } else {
            expr_symbol
        };
        assert_eq!(
            denoted_symbol(typer, binder_type),
            Some(expected_binder_type)
        );
        assert_eq!(store.symbols.get(binder).owner, Some(method));
        assert_eq!(
            store.symbols.get(binder).origin,
            SymbolOrigin::Source(source)
        );
        let body_expr = if let TreeKind::Block(block) = &source_ast.get(source_case.body).kind {
            block.expr
        } else {
            source_case.body
        };
        let typed_body_expr = mapped(body_expr);
        let typed_case_body_expr =
            if let TreeKind::Block(block) = &typer.typed_ast().get(typed_case.body).kind {
                block.expr
            } else {
                typed_case.body
            };
        assert_eq!(typed_body_expr, typed_case_body_expr);
        assert_eq!(
            denoted_symbol(typer, typer.typed_ast().get(typed_body_expr).ty),
            Some(int_symbol)
        );
        if case_index == 0 {
            assert_eq!(binder_name, "value");
            assert_eq!(typed_symbol(typer, typed_body_expr), binder);
            case_receipts.push("Num(value:Int)=>value:Int");
        } else {
            assert_eq!(binder_name, "inner");
            let TreeKind::PhaseSpecific(dotty_core::ast::UntypedNode::InfixOp(infix)) =
                &source_ast.get(body_expr).kind
            else {
                panic!("Succ body is the original recursive call plus literal")
            };
            assert_eq!(store.names.resolve(infix.op.text()), "+");
            let recursive_id = infix.left;
            let TreeKind::Apply(recursive_source) = &source_ast.get(recursive_id).kind else {
                panic!("left operand is the recursive method application")
            };
            assert_eq!(recursive_source.args.len(), 1);
            let source_function = recursive_source.function;
            let source_argument = recursive_source.args[0];
            let TreeKind::Ident(function_ident) = &source_ast.get(source_function).kind else {
                panic!("recursive reference is a simple method identifier")
            };
            assert_eq!(store.names.resolve(function_ident.name.text()), "eval");
            let typed_call = mapped(recursive_id);
            let TreeKind::Apply(typed_application) = &typer.typed_ast().get(typed_call).kind else {
                panic!("recursive call remains one ordinary Apply")
            };
            assert_eq!(typed_application.args.len(), 1);
            let typed_function = mapped(source_function);
            assert_eq!(typed_application.function, typed_function);
            assert_eq!(typed_symbol(typer, typed_function), method);
            let typed_argument = mapped(source_argument);
            assert_eq!(typed_application.args[0], typed_argument);
            assert_eq!(typed_symbol(typer, typed_argument), binder);
            assert_eq!(
                denoted_symbol(typer, typer.typed_ast().get(typed_argument).ty),
                Some(expected_binder_type)
            );
            assert_eq!(
                denoted_symbol(typer, typer.typed_ast().get(typed_call).ty),
                Some(int_symbol)
            );

            let addition = mapped(body_expr);
            let TreeKind::Apply(typed_addition) = &typer.typed_ast().get(addition).kind else {
                panic!("typed addition is an ordinary method application")
            };
            assert_eq!(typed_addition.args.len(), 1);
            let TreeKind::Select(addition_function) =
                &typer.typed_ast().get(typed_addition.function).kind
            else {
                panic!("Int addition resolves through a selected + method")
            };
            assert_eq!(addition_function.qualifier, typed_call);
            let plus_symbol = typed_symbol(typer, typed_addition.function);
            assert_eq!(
                store
                    .names
                    .resolve(store.symbols.get(plus_symbol).name.text()),
                "+"
            );
            let literal = mapped(infix.right);
            assert!(matches!(
                typer.typed_ast().get(literal).kind,
                TreeKind::Literal(_)
            ));
            assert!(matches!(
                store.types.get(typer.typed_ast().get(literal).ty),
                Type::Constant(Constant::Int(1))
            ));
            assert_eq!(
                denoted_symbol(typer, typer.typed_ast().get(addition).ty),
                Some(int_symbol)
            );
            recursive_receipt = Some(
                "call=eval(inner):same_symbol,arg=Succ.inner:Expr,result=Int,addition=Int,literal=1:Int"
                    .to_owned(),
            );
            case_receipts.push("Succ(inner:Expr)=>eval(inner):Int + 1:Int");
        }
    }
    assert_eq!(
        case_receipts,
        [
            "Num(value:Int)=>value:Int",
            "Succ(inner:Expr)=>eval(inner):Int + 1:Int"
        ]
    );
    assert_eq!(case_binders.len(), 2);
    assert_ne!(case_binders[0], case_binders[1]);
    assert!(!case_binders.contains(&parameter));
    let method_position = source_ast
        .get(source_method)
        .position
        .expect("method span")
        .span()
        .range();
    let call_receipt = recursive_receipt.expect("recursive call receipt");
    format!(
        "Evaluator.eval\tmethod={method:?}\tparameter={parameter:?}\towner=Evaluator$:{owner:?}\tsource_bytes={}:{}\tresult=Int\tcases=[{}]\tselected=[{}]\t{call_receipt}\tspans=preserved\tindependent_sessions=equal",
        method_position.start(),
        method_position.end(),
        case_receipts.join(","),
        selected.join(",")
    )
}

fn assert_complete_unwrap(context: UnwrapProofContext<'_>) -> String {
    let UnwrapProofContext {
        source_ast,
        source,
        index,
        typer,
        source_method,
        method,
        parameter,
        typed_method,
    } = context;
    let store = typer.store();
    let source_type_symbol = |target: &str| {
        source_ast.iter().find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == target =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
    };
    let expr = source_type_symbol("Expr").expect("the original source Expr trait");
    assert_eq!(store.symbols.get(expr).kind, SymbolKind::Trait);
    assert!(store.symbols.get(expr).flags.contains(SymbolFlags::SEALED));
    assert!(matches!(store.symbols.get(expr).origin, SymbolOrigin::Source(id) if id == source));
    assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
    assert_eq!(store.symbols.get(parameter).owner, Some(method));
    assert!(
        matches!(store.symbols.get(parameter).origin, SymbolOrigin::Source(id) if id == source)
    );

    let SymbolInfo::Complete(method_type) = *store.symbols.info(method) else {
        panic!("unwrap signature is complete")
    };
    let Type::Method(signature) = store.types.get(method_type) else {
        panic!("unwrap has a method signature")
    };
    assert_eq!(signature.params.len(), 1);
    assert_eq!(type_ref_symbol(typer, signature.params[0].ty), expr);
    assert_eq!(type_ref_symbol(typer, signature.result), expr);

    let TreeKind::DefDef(source_definition) = &source_ast.get(source_method).kind else {
        panic!("source root is a DefDef")
    };
    let source_rhs = source_definition.rhs.expect("unwrap has a source body");
    let TreeKind::Match(source_match) = &source_ast.get(source_rhs).kind else {
        panic!("unwrap body is the original Match")
    };
    assert_eq!(source_match.cases.len(), 2);
    let mapped = |source_tree| {
        let typed_tree = typer
            .source_typed_index()
            .get(source, source_tree)
            .unwrap_or_else(|| panic!("source tree {source_tree:?} has a typed mapping"));
        assert_eq!(
            source_ast.get(source_tree).position,
            typer.typed_ast().get(typed_tree).position,
            "source span survives typing for {source_tree:?}"
        );
        typed_tree
    };
    assert_eq!(mapped(source_method), typed_method);
    let typed_definition = &typer.typed_ast().get(typed_method).kind;
    let TreeKind::DefDef(typed_definition) = typed_definition else {
        panic!("typed root remains a DefDef")
    };
    let typed_rhs = mapped(source_rhs);
    let typed_result_type = mapped(source_definition.tpt);
    assert_eq!(
        denoted_symbol(typer, typer.typed_ast().get(typed_result_type).ty),
        Some(expr)
    );
    assert_eq!(typed_definition.rhs, Some(typed_rhs));
    let TreeKind::Match(typed_match) = &typer.typed_ast().get(typed_rhs).kind else {
        panic!("typed RHS remains a Match")
    };
    assert_eq!(typed_match.cases.len(), 2);
    assert_eq!(typed_match.cases.len(), source_match.cases.len());

    let source_selector = source_match.selector;
    let typed_selector = mapped(source_selector);
    assert_eq!(typed_match.selector, typed_selector);
    assert_eq!(typed_symbol(typer, typed_selector), parameter);
    assert_eq!(type_ref_symbol(typer, signature.params[0].ty), expr);
    assert_eq!(
        denoted_symbol(typer, typer.typed_ast().get(typed_selector).ty),
        Some(expr)
    );
    assert_eq!(
        denoted_symbol(typer, typer.typed_ast().get(typed_rhs).ty),
        Some(expr)
    );

    let mut receipt = Vec::new();
    let mut selected_methods = Vec::new();
    for (case_index, (source_case, typed_case)) in source_match
        .cases
        .iter()
        .zip(typed_match.cases.iter())
        .enumerate()
    {
        assert_eq!(mapped(*source_case), *typed_case);
        let TreeKind::CaseDef(source_case) = &source_ast.get(*source_case).kind else {
            panic!("source case {case_index} is a CaseDef")
        };
        let TreeKind::CaseDef(typed_case) = &typer.typed_ast().get(*typed_case).kind else {
            panic!("typed case {case_index} is a CaseDef")
        };
        assert_eq!(typed_case.guard, None);
        assert_eq!(mapped(source_case.pattern), typed_case.pattern);
        assert_eq!(mapped(source_case.body), typed_case.body);
        let TreeKind::Apply(source_extractor) = &source_ast.get(source_case.pattern).kind else {
            panic!("source case pattern {case_index} is an extractor application")
        };
        let TreeKind::UnApply(typed_extractor) = &typer.typed_ast().get(typed_case.pattern).kind
        else {
            panic!("typed case pattern {case_index} is an UnApply")
        };
        assert_eq!(source_extractor.args.len(), 1);
        assert_eq!(typed_extractor.patterns.len(), 1);
        assert_eq!(
            mapped(source_extractor.function),
            match &typer.typed_ast().get(typed_extractor.function).kind {
                TreeKind::Select(selected) => selected.qualifier,
                other => panic!("extractor function is a selected companion member: {other:?}"),
            }
        );
        let selected = typed_symbol(typer, typed_extractor.function);
        let variant_name = if case_index == 0 { "Num" } else { "Succ" };
        let variant = source_type_symbol(variant_name).expect("canonical source case class");
        assert_eq!(
            denoted_symbol(typer, typer.typed_ast().get(typed_case.pattern).ty),
            Some(variant)
        );
        let unapply = index
            .generated_case_class_method(variant, GeneratedCaseClassMethodRole::Unapply)
            .expect("authentic derived unapply role");
        assert_eq!(selected, unapply);
        let companion = store
            .symbols
            .get(variant)
            .links
            .companion
            .expect("source companion");
        let unapply_owner = store.symbols.get(unapply).owner.expect("generated owner");
        assert_eq!(
            store.symbols.get(unapply_owner).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(
            store
                .names
                .resolve(store.symbols.get(unapply_owner).name.text()),
            format!("{variant_name}$")
        );
        assert_ne!(unapply_owner, companion);
        selected_methods.push(format!("{variant_name}$.unapply@{variant_name}$"));

        let source_nested = source_extractor.args[0];
        let typed_nested = typed_extractor.patterns[0];
        assert_eq!(mapped(source_nested), typed_nested);
        let source_body_expr =
            if let TreeKind::Block(block) = &source_ast.get(source_case.body).kind {
                block.expr
            } else {
                source_case.body
            };
        let mapped_body_expr = mapped(source_body_expr);
        let body = if let TreeKind::Block(block) = &typer.typed_ast().get(typed_case.body).kind {
            block.expr
        } else {
            typed_case.body
        };
        assert_eq!(mapped_body_expr, body);
        assert_eq!(
            denoted_symbol(typer, typer.typed_ast().get(body).ty),
            Some(expr)
        );
        match case_index {
            0 => {
                let TreeKind::Ident(wildcard) = &typer.typed_ast().get(typed_nested).kind else {
                    panic!("Num wildcard does not introduce a binder")
                };
                assert_eq!(store.names.resolve(wildcard.name.text()), "_");
                assert_eq!(store.symbols.get(parameter).kind, SymbolKind::Parameter);
                assert_eq!(typed_symbol(typer, body), parameter);
                receipt.push("Num(_):Expr=>expr");
            }
            1 => {
                let TreeKind::Bind(binding) = &typer.typed_ast().get(typed_nested).kind else {
                    panic!("Succ(inner) introduces one case-local binder")
                };
                let binder = typed_symbol(typer, typed_nested);
                assert_eq!(store.names.resolve(binding.name.text()), "inner");
                assert_eq!(store.symbols.get(binder).kind, SymbolKind::Local);
                assert_eq!(store.symbols.get(binder).owner, Some(method));
                assert_eq!(
                    store.symbols.get(binder).origin,
                    SymbolOrigin::Source(source)
                );
                let SymbolInfo::Complete(binder_type) = *store.symbols.info(binder) else {
                    panic!("the case-local binder has a completed type")
                };
                assert_eq!(type_ref_symbol(typer, binder_type), expr);
                assert_eq!(type_ref_symbol(typer, signature.params[0].ty), expr);
                assert_eq!(typed_symbol(typer, body), binder);
                receipt.push("Succ(inner:Expr):Expr=>case-local inner");
            }
            _ => unreachable!(),
        }
    }
    assert_eq!(
        receipt,
        [
            "Num(_):Expr=>expr",
            "Succ(inner:Expr):Expr=>case-local inner"
        ]
    );
    format!(
        "Expr.scala#tree{}|before=UnsupportedProduct1FieldType(Succ.inner)|after=complete Evaluator.unwrap(expr:Expr):Expr|match=Expr,cases=[{}]|selected=[{}]|source_mappings=DefDef,Match,selector,CaseDef[0..1],UnApply[0..1],qualifier[0..1],wildcard,binder,body[0..1]|spans=preserved",
        source_method.index(),
        receipt.join(","),
        selected_methods.join(",")
    )
}

fn typed_symbol(
    typer: &SourceTyper<'_>,
    tree: dotty_core::TreeId<dotty_core::ast::Typed>,
) -> dotty_core::SymbolId {
    match typer.store().types.get(typer.typed_ast().get(tree).ty) {
        Type::TermRef {
            target: dotty_core::types::TermRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("expected canonical term reference for {tree:?}, got {other:?}"),
    }
}

fn denoted_symbol(typer: &SourceTyper<'_>, ty: dotty_core::TypeId) -> Option<dotty_core::SymbolId> {
    match typer.store().types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        Type::TermRef {
            target: dotty_core::types::TermRefTarget::Symbol(symbol),
            ..
        } => match *typer.store().symbols.info(*symbol) {
            SymbolInfo::Complete(ty) => denoted_symbol(typer, ty),
            _ => None,
        },
        _ => None,
    }
}
