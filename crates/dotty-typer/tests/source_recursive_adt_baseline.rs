use std::path::PathBuf;

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, DirectoryClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::TreeKind;
use dotty_core::types::{Type, TypeRefTarget};
use dotty_core::{
    Definitions, GeneratedCaseClassMethodRole, Packages, SemanticStore, SourceId, SourceText,
    SymbolKind, SymbolOrigin, TypeName,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_typer::{SourceTyper, TyperError};

const SOURCE: &str = include_str!("fixtures/source-recursive-adt-match-3.9.0/Expr.scala");

#[test]
fn evaluator_recursive_adt_two_roots_record_authentic_first_blockers() {
    let (first, first_paths) = analyze_roots(SourceId::from_index(20341));
    let (second, second_paths) = analyze_roots(SourceId::from_index(20342));
    assert_eq!(first, second, "fresh source sessions normalize identically");
    assert_eq!(
        first_paths, second_paths,
        "deferred probes normalize identically"
    );
    assert_eq!(
        first.len(),
        2,
        "only unwrap and eval count as whole DefDef roots"
    );
    let actual = format!("{}\n", first.join("\n"));
    let expected =
        include_str!("fixtures/source-recursive-adt-match-3.9.0/rust-two-root-baseline.tsv")
            .split_once('\n')
            .expect("two-root receipt header")
            .1;
    assert_eq!(actual, expected);
    for row in first {
        println!("ISSUE2034_ROOT\t{row}");
    }
    let actual = format!("{}\n", first_paths.join("\n"));
    let expected =
        include_str!("fixtures/source-recursive-adt-match-3.9.0/rust-deferred-paths.tsv")
            .split_once('\n')
            .expect("deferred path header")
            .1;
    assert_eq!(actual, expected);
    for row in first_paths {
        println!("ISSUE2034_DEFERRED\t{row}");
    }
}

#[test]
fn evaluator_recursive_adt_classification_records_canonical_mechanisms() {
    let rows = characterize_classes(SourceId::from_index(20343));
    let actual = format!("{}\n", rows.join("\n"));
    let expected =
        include_str!("fixtures/source-recursive-adt-match-3.9.0/rust-class-evidence.tsv")
            .split_once('\n')
            .expect("class evidence header")
            .1;
    assert_eq!(actual, expected);
    for row in rows {
        println!("ISSUE2034_CLASS\t{row}");
    }
}

fn characterize_classes(source: SourceId) -> Vec<String> {
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
        let unapply_shape = match typer.complete_symbol(unapply) {
            Ok(ty) => match typer.store().types.get(ty) {
                Type::Method(method) => {
                    let arg = type_symbol_name(&typer, method.params[0].ty);
                    let result = type_symbol_name(&typer, method.result);
                    format!("({arg}):{result}")
                }
                other => format!("unexpected={other:?}"),
            },
            Err(error) => format!("error={error:?}"),
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
        let selector_shape = match typer.complete_symbol(selector) {
            Ok(ty) => match typer.store().types.get(ty) {
                Type::Method(method) => format!("_1:{}", type_symbol_name(&typer, method.result)),
                other => format!("unexpected={other:?}"),
            },
            Err(error) => format!("error={error:?}"),
        };
        let class_info = match typer.complete_symbol(class) {
            Ok(ty) => match typer.store().types.get(ty) {
                Type::ClassInfo(info) => {
                    let parent_names = info
                        .parents
                        .iter()
                        .map(|parent| type_symbol_name(&typer, *parent))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("parents={parent_names}")
                }
                other => format!("unexpected={other:?}"),
            },
            Err(error) => format!("error={error:?}"),
        };
        rows.push(format!(
            "{name}\ttree={}\tsymbol={class:?}\tkind=CASE-Class\torigin=Source\tparent=Expr@tree{}=>{source_parent:?}\tcompanion={companion:?}\tmodule_class={module_class:?}\tunapply={unapply:?},owner={name}$,type={unapply_shape}\tfield={field_name}:{field:?}\tselector={selector:?},{selector_shape}\tclass_info={class_info}",
            tree.index(), template.parents[0].index()
        ));
    }
    rows
}

fn type_symbol_name(typer: &SourceTyper<'_>, ty: dotty_core::TypeId) -> String {
    match typer.store().types.get(ty) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => typer
            .store()
            .names
            .resolve(typer.store().symbols.get(*symbol).name.text())
            .to_owned(),
        other => format!("{other:?}"),
    }
}

fn analyze_roots(source: SourceId) -> (Vec<String>, Vec<String>) {
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
    let mut root_trees = Vec::new();
    for (name, tree, position, symbol, parameter, owner) in roots {
        let outcome = match typer.type_method_definition(tree) {
            Ok(typed) => {
                assert_eq!(typer.source_typed_index().get(source, tree), Some(typed));
                assert!(matches!(
                    typer.typed_ast().get(typed).kind,
                    TreeKind::DefDef(_)
                ));
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
        for target in [return_tree, binder, body_expr] {
            assert!(
                typer.source_typed_index().get(source, target).is_none(),
                "failed whole-method typing must not retain a fragment mapping"
            );
        }
        let return_range = parsed.ast.get(return_tree).position.unwrap().span().range();
        let binder_range = parsed.ast.get(binder).position.unwrap().span().range();
        if name == "unwrap" {
            deferred.push(format!(
                "unwrap\treturn=tree{}@{}:{}:Expr,deferred\tbinder=tree{}@{}:{}:inner,deferred\tbody=tree{}@{}:{}:inner,deferred",
                return_tree.index(), return_range.start(), return_range.end(), binder.index(), binder_range.start(), binder_range.end(), body_expr.index(), body_range.start(), body_range.end()
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
            for target in [call, function, argument] {
                assert!(
                    typer.source_typed_index().get(source, target).is_none(),
                    "recursive call fragment must remain unmapped after root rollback"
                );
            }
            deferred.push(format!(
                "eval\treturn=tree{}@{}:{}:Int,deferred\tbinder=tree{}@{}:{}:inner,deferred\tcall=tree{}@{}:{}:eval(inner),namer_owner={eval_owner_name}:{eval_symbol:?},typed_result=deferred\tfunction=tree{}@{}:{}:eval\targument=tree{}@{}:{}:inner",
                return_tree.index(), return_range.start(), return_range.end(), binder.index(), binder_range.start(), binder_range.end(), call.index(), call_range.start(), call_range.end(), function.index(), function_range.start(), function_range.end(), argument.index(), argument_range.start(), argument_range.end()
            ));
        }
    }
    deferred.sort();
    (rows, deferred)
}
