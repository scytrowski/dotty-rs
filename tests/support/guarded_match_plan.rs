use dotty::core::{Definitions, Packages, SemanticStore, SourceId, SourceText, ast::TreeKind};
use dotty::jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmMethodPlan,
    JvmPrimaryConstructorPlan,
};
pub(super) fn with_jvm<R>(
    text: &str,
    consume: impl FnOnce(&JvmCompilationUnit<'_>, &Definitions) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let source = SourceId::from_index(2471);
    let mut packages = Packages::new();
    let parsed = dotty::parser::parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        dotty::lexer::ContextualScanner::new(text).unwrap(),
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let index = dotty::namer::name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "Probe.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!()
    };
    let class_tree = package.stats[0];
    let class = index.symbol_at(source, class_tree).unwrap();
    let TreeKind::TypeDef(def) = &parsed.ast.get(class_tree).kind else {
        panic!()
    };
    let TreeKind::Template(template) = &parsed.ast.get(def.rhs).kind else {
        panic!()
    };
    let constructor = index.symbol_at(source, template.constructor).unwrap();
    let mut typer = dotty::typer::SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    for sym in [class, constructor] {
        if let Err(e) = typer.complete_symbol(sym) {
            panic!("TYPER {e:?}")
        }
    }
    let mut methods = vec![];
    for &tree in &template.body {
        if tree == template.constructor {
            continue;
        }
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            continue;
        };
        let method = index.symbol_at(source, tree).unwrap();
        if let Err(e) = typer.complete_symbol(method) {
            panic!("TYPER {e:?}")
        }
        let definition = typer.type_method_definition(tree).unwrap();
        let TreeKind::DefDef(typed_method) = &typer.typed_ast().get(definition).kind else {
            panic!()
        };
        let body = typed_method.rhs.unwrap();
        let parameters = def
            .value_param_clauses
            .iter()
            .flatten()
            .map(|&t| index.symbol_at(source, t).unwrap())
            .collect();
        let mut locals = vec![];
        for (case_id, node) in parsed.ast.iter() {
            let TreeKind::CaseDef(case) = &node.kind else {
                continue;
            };
            let Some(binding) = typer.typed_pattern_binding_at(source, case.pattern) else {
                continue;
            };
            if typer.store().symbols.get(binding.symbol()).owner != Some(method) {
                continue;
            }
            locals.push(dotty::jvm::JvmLocalBinding::pattern(
                binding.declaration(),
                binding.symbol(),
                binding.scope(),
                typer.source_typed_index().get(source, case_id).unwrap(),
            ));
        }
        locals.sort_unstable_by_key(|b| b.declaration.index());
        methods.push(JvmMethodPlan {
            method,
            parameters,
            locals,
            body: Some(body),
        });
    }
    let init =
        JvmPrimaryConstructorPlan::from_source(class, constructor, vec![], &parsed.ast, &index)
            .unwrap();
    let unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![JvmClassPlan {
            class,
            fields: vec![],
            methods,
            initialization: JvmClassInitializationPlan::PrimaryConstructor(init),
        }],
    };
    let expected = unit.classes[0]
        .methods
        .iter()
        .map(|m| (m.method, m.locals.clone()))
        .collect::<Vec<_>>();
    let result = consume(&unit, &definitions);
    drop(unit);
    let output = typer.into_typed_output();
    assert_eq!(
        output.pattern_bindings.len(),
        expected
            .iter()
            .map(|(_, locals)| locals.len())
            .sum::<usize>()
    );
    let planner =
        dotty::jvm_method_plan::JvmMethodPlanner::new(&store, &parsed.ast, &index, &output);
    for (method, locals) in expected {
        assert_eq!(planner.plan(method).unwrap().locals, locals);
    }
    result
}
