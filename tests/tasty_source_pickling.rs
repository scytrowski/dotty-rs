use std::fs;
use std::path::PathBuf;
use std::process::Command;

use dotty_core::ast::*;
use dotty_core::resolution::{MemberRequest, ResolutionError, ResolverCheckpoint, SymbolResolver};
use dotty_core::*;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::tasty::*;
use dotty_tasty_pickler::*;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;
use dotty_typer::SourceTyper;

const ANSWER: &str = include_str!("fixtures/tasty_pickler/Answer.scala");
const PARAMETERS: &str = include_str!("fixtures/tasty_pickler/Parameters.scala");
const OPERATIONS: &str = include_str!("fixtures/tasty_pickler/Operations.scala");
const DECISIONS: &str = include_str!("fixtures/tasty_pickler/Decisions.scala");
const RUN_SCALA_ORACLE: &str = "DOTTY_TASTY_PICKLER_RUN_SCALA_ORACLE";

struct CanonicalIntResolver {
    symbol: SymbolId,
}

impl SymbolResolver for CanonicalIntResolver {
    fn checkpoint(&self) -> ResolverCheckpoint {
        ResolverCheckpoint::new(0)
    }
    fn rollback_to(&mut self, _store: &mut SemanticStore, _checkpoint: ResolverCheckpoint) {}
    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let candidate = store.symbols.get(self.symbol);
        let owner = candidate.owner;
        let exact_name = candidate.name == request.name;
        let exact_builtin = candidate.origin == SymbolOrigin::Builtin
            && owner.is_none()
            && matches!(store.types.get(request.prefix),
                Type::TermRef { target: TermRefTarget::Symbol(prefix), .. }
                    if store.symbols.contains(*prefix)
                        && store.symbols.get(*prefix).kind == SymbolKind::Package
                        && store.symbols.get(*prefix).name.namespace() == Namespace::Term
                        && store.names.try_resolve(store.symbols.get(*prefix).name.text()) == Some("scala"));
        Ok((exact_name && exact_builtin).then_some(self.symbol))
    }
    fn resolve_package(
        &mut self,
        _store: &mut SemanticStore,
        _path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }
}

struct CanonicalScalaResolver {
    symbols: Vec<SymbolId>,
}

fn assert_builtin_type(store: &SemanticStore, actual: TypeId, expected: TypeId) {
    let actual = match store.types.get(actual) {
        Type::ByName { result } => *result,
        _ => actual,
    };
    let expected_symbol = store
        .types
        .get(expected)
        .reference_symbol()
        .expect("builtin definition has a canonical type symbol");
    assert_eq!(
        store.types.get(actual).reference_symbol(),
        Some(expected_symbol)
    );
}

impl SymbolResolver for CanonicalScalaResolver {
    fn checkpoint(&self) -> ResolverCheckpoint {
        ResolverCheckpoint::new(0)
    }
    fn rollback_to(&mut self, _store: &mut SemanticStore, _checkpoint: ResolverCheckpoint) {}
    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let scala_prefix = matches!(store.types.get(request.prefix),
            Type::TermRef { target: TermRefTarget::Symbol(prefix), .. }
                if store.symbols.contains(*prefix)
                    && store.symbols.get(*prefix).kind == SymbolKind::Package
                    && store.symbols.get(*prefix).name.namespace() == Namespace::Term
                    && store.names.try_resolve(store.symbols.get(*prefix).name.text()) == Some("scala"));
        let symbol = self.symbols.iter().copied().find(|symbol| {
            store.symbols.contains(*symbol)
                && store.symbols.get(*symbol).origin == SymbolOrigin::Builtin
                && store.symbols.get(*symbol).owner.is_none()
                && store.symbols.get(*symbol).name == request.name
        });
        Ok((scala_prefix).then_some(symbol).flatten())
    }
    fn resolve_package(
        &mut self,
        _store: &mut SemanticStore,
        _path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok(None)
    }
}

struct Product {
    bytes: Vec<u8>,
    output: PickledTasty,
    source_value: i32,
}

fn compile_source() -> Product {
    let source = SourceId::from_index(11);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(ANSWER).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let parsed = parse_compilation_unit(
        SourceText::new(ANSWER).unwrap(),
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
        "Answer.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("namer rejected canonical fixture: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!()
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!("expected one source class")
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_def) = &parsed.ast.get(*class_tree).kind else {
        panic!()
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
        panic!()
    };
    let constructor_tree = template.constructor;
    let constructor = index.symbol_at(source, constructor_tree).unwrap();
    let TreeKind::DefDef(ctor) = &parsed.ast.get(constructor_tree).kind else {
        panic!()
    };
    let clauses = |definition: &DefDef<Untyped>| {
        definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect()
            })
            .collect::<Vec<Vec<SymbolId>>>()
    };
    let ctor_clauses = clauses(ctor);
    let methods = template
        .body
        .iter()
        .filter_map(|tree| {
            let TreeKind::DefDef(definition) = &parsed.ast.get(*tree).kind else {
                return None;
            };
            Some((*tree, definition))
        })
        .collect::<Vec<_>>();
    assert_eq!(methods.len(), 1);
    let (method_tree, method_def) = methods[0];
    let method = index.symbol_at(source, method_tree).unwrap();
    let source_rhs = method_def.rhs.expect("concrete fixture RHS");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    typer.complete_symbol(constructor).unwrap();
    typer.complete_symbol(method).unwrap();
    let context = typer.expression_context_for(method).unwrap();
    let typed_body = typer.type_expression(source_rhs, context).unwrap();
    assert_eq!(
        typer.source_typed_index().get(source, source_rhs),
        Some(typed_body)
    );
    let TreeKind::Literal(Literal {
        value: Constant::Int(source_value),
    }) = typer.typed_ast().get(typed_body).kind
    else {
        panic!("expected actual typed Int literal")
    };
    let bindings = |source, tree| typer.source_typed_index().get(source, tree);
    let plan = PicklePlan {
        store: typer.store(),
        definitions: &definitions,
        typed_ast: typer.typed_ast(),
        source_ast: &parsed.ast,
        source_index: &index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source,
            name: "Answer.scala".into(),
        },
        source_root: parsed.root,
        class,
        class_tree: *class_tree,
        constructor: DefaultConstructorPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: ctor_clauses,
        },
        methods: &[MethodPicklePlan {
            symbol: method,
            source_tree: method_tree,
            parameter_clauses: clauses(method_def),
            body: Some(typed_body),
        }],
        local_values: &[],
    };
    let checked = plan.validate().unwrap();
    let output = checked.pickle().unwrap();
    assert_eq!(output.body(method), Some(typed_body));
    Product {
        bytes: output.bytes().to_vec(),
        output,
        source_value,
    }
}

struct OperationsProduct {
    bytes: Vec<u8>,
    output: PickledTasty,
    methods: Vec<(String, SymbolId)>,
}

fn compile_operations() -> OperationsProduct {
    let source = SourceId::from_index(13);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(OPERATIONS).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let parsed = parse_compilation_unit(
        SourceText::new(OPERATIONS).unwrap(),
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
        "Operations.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("namer rejected Operations fixture: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!()
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!("expected one source class")
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_def) = &parsed.ast.get(*class_tree).kind else {
        panic!()
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
        panic!()
    };
    let constructor_tree = template.constructor;
    let constructor = index.symbol_at(source, constructor_tree).unwrap();
    let TreeKind::DefDef(constructor_def) = &parsed.ast.get(constructor_tree).kind else {
        panic!()
    };
    let clauses = |definition: &DefDef<Untyped>| {
        definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let source_methods = template
        .body
        .iter()
        .filter_map(|tree| match &parsed.ast.get(*tree).kind {
            TreeKind::DefDef(definition) => Some((*tree, definition)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    typer.complete_symbol(constructor).unwrap();
    let mut methods = Vec::with_capacity(source_methods.len());
    let mut method_names = Vec::with_capacity(source_methods.len());
    for (tree, definition) in source_methods {
        let method = index.symbol_at(source, tree).unwrap();
        let method_name = typer
            .store()
            .names
            .resolve(definition.name.as_name().text())
            .to_owned();
        typer.complete_symbol(method).unwrap();
        let context = typer.expression_context_for(method).unwrap();
        let rhs = definition.rhs.expect("Operations methods have bodies");
        let body = typer
            .type_expression_expected(rhs, context, definitions.int)
            .unwrap();
        assert_eq!(typer.source_typed_index().get(source, rhs), Some(body));
        methods.push(MethodPicklePlan {
            symbol: method,
            source_tree: tree,
            parameter_clauses: clauses(definition),
            body: Some(body),
        });
        method_names.push((method_name, method));
    }
    assert_eq!(
        method_names
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["use", "identity", "first", "second", "zero"]
    );
    let bindings = |source, tree| typer.source_typed_index().get(source, tree);
    let plan = PicklePlan {
        store: typer.store(),
        definitions: &definitions,
        typed_ast: typer.typed_ast(),
        source_ast: &parsed.ast,
        source_index: &index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source,
            name: "Operations.scala".into(),
        },
        source_root: parsed.root,
        class,
        class_tree: *class_tree,
        constructor: DefaultConstructorPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: clauses(constructor_def),
        },
        methods: &methods,
        local_values: &[],
    };
    let output = plan.validate().unwrap().pickle().unwrap();
    for method in &methods {
        assert_eq!(output.body(method.symbol), method.body);
    }
    OperationsProduct {
        bytes: output.bytes().to_vec(),
        output,
        methods: method_names,
    }
}

struct DecisionsProduct {
    bytes: Vec<u8>,
    output: PickledTasty,
    methods: Vec<(String, SymbolId)>,
    stored_method: SymbolId,
}

fn compile_decisions() -> DecisionsProduct {
    let source = SourceId::from_index(15);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(DECISIONS).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let parsed = parse_compilation_unit(
        SourceText::new(DECISIONS).unwrap(),
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
        "Decisions.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("namer rejected canonical Decisions fixture: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package root")
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!("expected one Decisions class")
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_def) = &parsed.ast.get(*class_tree).kind else {
        panic!("expected class definition")
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
        panic!("expected class template")
    };
    let constructor_tree = template.constructor;
    let constructor = index.symbol_at(source, constructor_tree).unwrap();
    let TreeKind::DefDef(constructor_def) = &parsed.ast.get(constructor_tree).kind else {
        panic!("expected constructor definition")
    };
    let clauses = |definition: &DefDef<Untyped>| {
        definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let source_methods = template
        .body
        .iter()
        .filter_map(|tree| match &parsed.ast.get(*tree).kind {
            TreeKind::DefDef(definition) => Some((*tree, definition)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected_methods = ["unit", "enabled", "disabled", "choose", "nested", "stored"];
    assert_eq!(source_methods.len(), expected_methods.len());
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    typer.complete_symbol(constructor).unwrap();
    let mut methods = Vec::with_capacity(source_methods.len());
    let mut method_names = Vec::with_capacity(source_methods.len());
    let mut stored_method = None;
    let mut local_values = Vec::new();
    for ((tree, definition), expected_name) in source_methods.iter().zip(expected_methods) {
        let name = typer
            .store()
            .names
            .resolve(definition.name.as_name().text())
            .to_owned();
        assert_eq!(name, expected_name);
        let method = index.symbol_at(source, *tree).unwrap();
        let signature = typer.complete_symbol(method).unwrap();
        assert_eq!(typer.store().symbols.get(method).owner, Some(class));
        let expected_result = match name.as_str() {
            "unit" => definitions.unit,
            "enabled" | "disabled" => definitions.boolean,
            _ => definitions.int,
        };
        if expected_name == "unit" || expected_name == "enabled" || expected_name == "disabled" {
            assert_eq!(signature, expected_result);
        } else {
            let Type::Method(method_type) = typer.store().types.get(signature) else {
                panic!("{name} must have a method signature")
            };
            assert_eq!(method_type.result, expected_result);
        }
        let rhs = definition
            .rhs
            .expect("canonical Decisions methods have bodies");
        let context = typer.expression_context_for(method).unwrap();
        let body = typer
            .type_expression_expected(rhs, context, expected_result)
            .unwrap();
        assert_eq!(typer.source_typed_index().get(source, rhs), Some(body));
        if name == "stored" {
            stored_method = Some(method);
            let TreeKind::Block(source_block) = &parsed.ast.get(rhs).kind else {
                panic!("stored source body must be a block")
            };
            let [local_tree] = source_block.stats.as_slice() else {
                panic!("stored source block must have one local declaration")
            };
            let binding = typer
                .typed_local_binding_at(source, *local_tree)
                .expect("SourceTyper supplies canonical local binding evidence");
            let TreeKind::Block(typed_block) = &typer.typed_ast().get(body).kind else {
                panic!("stored typed body must be a block")
            };
            assert_eq!(typed_block.stats.len(), 1);
            assert_eq!(
                typer.source_typed_index().get(source, *local_tree),
                Some(binding.declaration())
            );
            assert_eq!(
                typer.store().symbols.get(binding.symbol()).owner,
                Some(method)
            );
            let TreeKind::If(typed_if) = &typer.typed_ast().get(typed_block.expr).kind else {
                panic!("stored result must be an If")
            };
            assert!(matches!(
                typer.store().types.get(typer.typed_ast().get(typed_if.then_branch).ty),
                Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                    if *symbol == binding.symbol()
            ));
            local_values.push(LocalValueBinding {
                source_tree: *local_tree,
                symbol: binding.symbol(),
                typed_declaration: binding.declaration(),
                scope: binding.scope(),
            });
        }
        methods.push(MethodPicklePlan {
            symbol: method,
            source_tree: *tree,
            parameter_clauses: clauses(definition),
            body: Some(body),
        });
        method_names.push((name, method));
    }
    let stored_method = stored_method.expect("stored method was typed");
    let bindings = |source, tree| typer.source_typed_index().get(source, tree);
    let plan = PicklePlan {
        store: typer.store(),
        definitions: &definitions,
        typed_ast: typer.typed_ast(),
        source_ast: &parsed.ast,
        source_index: &index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source,
            name: "Decisions.scala".into(),
        },
        source_root: parsed.root,
        class,
        class_tree: *class_tree,
        constructor: DefaultConstructorPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: clauses(constructor_def),
        },
        methods: &methods,
        local_values: &local_values,
    };
    let output = plan.validate().unwrap().pickle().unwrap();
    for method in &methods {
        assert_eq!(output.body(method.symbol), method.body);
    }
    DecisionsProduct {
        bytes: output.bytes().to_vec(),
        output,
        methods: method_names,
        stored_method,
    }
}

fn semantic_and_wire_projection(product: &Product) {
    let file = TastyFile::parse_and_validate_scala_3_9(&product.bytes).unwrap();
    assert_eq!(
        (
            file.header().major_version,
            file.header().minor_version,
            file.header().experimental_version
        ),
        (28, 9, 0)
    );
    assert_eq!(file.source_file().unwrap(), Some("Answer.scala"));
    let addresses = file.ast_address_index().unwrap();
    let declarations = product.output.declarations();
    assert_eq!(declarations.len(), 3);
    assert_eq!(declarations[1].owner, declarations[0].symbol);
    assert_eq!(declarations[2].owner, declarations[0].symbol);
    let class_node = addresses
        .get(declarations[0].address)
        .unwrap()
        .decode_definition()
        .unwrap();
    let method_node = addresses
        .get(declarations[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert_eq!(file.render_name(class_node.name()).unwrap(), "Answer");
    assert_eq!(file.render_name(method_node.name).unwrap(), "value");
    let Some(RawTree::Leaf(rhs)) = method_node.rhs else {
        panic!("missing actual method RHS")
    };
    assert_eq!(
        (rhs.tag, rhs.value),
        (INTCONST_TAG, TermValue::Int(product.source_value))
    );
    let shape = addresses
        .iter_nodes()
        .map(|node| node.tag)
        .collect::<Vec<_>>();
    for tag in [
        PACKAGE_TAG,
        TYPEDEF_TAG,
        TEMPLATE_TAG,
        DEFDEF_TAG,
        INTCONST_TAG,
    ] {
        assert!(shape.contains(&tag), "wire tree lacks tag {tag}");
    }
    assert_eq!(file.encode_validated().unwrap(), product.bytes);

    // The semantic inverse supports entering declaration identity and decoding
    // type trees/literal type nodes for this subset, but it does not rebuild a
    // typed AST body. Compare owner-qualified symbols and those semantic types.
    let mut semantic_store = SemanticStore::new();
    let semantic_definitions = Definitions::bootstrap(&mut semantic_store);
    let int_symbol = semantic_store
        .types
        .get(semantic_definitions.int)
        .reference_symbol()
        .unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut semantic_store, semantic_definitions)
        .with_resolver(Box::new(CanonicalIntResolver { symbol: int_symbol }));
    let unpickled_index = unpickler.enter_symbols().unwrap().clone();
    let package_address = addresses
        .iter_nodes()
        .find(|node| node.tag == PACKAGE_TAG)
        .expect("package root")
        .offset as u32;
    let package_id = unpickled_index
        .symbol_at(package_address)
        .expect("semantic package symbol");
    let class_id = unpickled_index.symbol_at(declarations[0].address).unwrap();
    let method_id = unpickled_index.symbol_at(declarations[2].address).unwrap();
    assert_eq!(
        unpickler
            .symbol_state_at(declarations[0].address)
            .unwrap()
            .0,
        SymbolKind::Class
    );
    assert_ne!(method_id, class_id);
    assert_eq!(
        unpickler
            .symbol_state_at(declarations[2].address)
            .unwrap()
            .0,
        SymbolKind::Method
    );
    let method_address = declarations[2].address;
    let method_children = addresses
        .iter_tree_edges()
        .filter(|edge| edge.parent.offset == method_address as usize)
        .map(|edge| edge.child)
        .collect::<Vec<_>>();
    let result_address = method_children
        .iter()
        .find(|child| child.tag == IDENTTPT_TAG)
        .expect("method has an Int IDENTtpt")
        .offset as u32;
    let rhs_address = method_children
        .iter()
        .find(|child| child.tag == INTCONST_TAG)
        .expect("method has an Int constant RHS")
        .offset as u32;
    let return_ty = unpickler.unpickle_type_tree_type(result_address).unwrap();
    let value_ty = unpickler.unpickle_type(rhs_address).unwrap();
    drop(unpickler);
    assert_eq!(semantic_store.symbols.get(class_id).owner, Some(package_id));
    assert_eq!(semantic_store.symbols.get(method_id).owner, Some(class_id));
    assert_eq!(
        semantic_store
            .names
            .resolve(semantic_store.symbols.get(method_id).name.text()),
        "value"
    );
    let canonical_int = semantic_store
        .types
        .get(semantic_definitions.int)
        .reference_symbol()
        .unwrap();
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(return_symbol),
        prefix,
    } = semantic_store.types.get(return_ty)
    else {
        panic!("expected semantic Int type reference")
    };
    assert_eq!(*return_symbol, canonical_int);
    assert!(matches!(semantic_store.types.get(*prefix),
        Type::TermRef { target: TermRefTarget::Symbol(package), .. }
            if semantic_store.symbols.get(*package).kind == SymbolKind::Package
                && semantic_store.names.try_resolve(semantic_store.symbols.get(*package).name.text()) == Some("scala")));
    assert_eq!(
        semantic_store.types.get(value_ty),
        &Type::Constant(Constant::Int(product.source_value))
    );
}

#[test]
fn real_scala_source_produces_deterministic_semantic_tasty_and_roundtrips() {
    let first = compile_source();
    let second = compile_source();
    assert_eq!(first.bytes, second.bytes);
    semantic_and_wire_projection(&first);
    semantic_and_wire_projection(&second);

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let candidate = root.join("target/issue-1195/generated/example/Answer.tasty");
        fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        fs::write(&candidate, &first.bytes).unwrap();
        let temporary = root.join("target/issue-1195/tmp");
        fs::create_dir_all(&temporary).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg("--runtime")
            .arg(&candidate)
            .env("TMPDIR", temporary)
            .status()
            .expect("start explicitly requested Scala 3.9 oracle");
        assert!(
            result.success(),
            "requested Scala interop/runtime gate failed"
        );
    }
}

#[test]
fn operations_source_preserves_canonical_references_and_runs_from_fresh_scala_class() {
    let first = compile_operations();
    let second = compile_operations();
    assert_eq!(first.bytes, second.bytes);

    let file = TastyFile::parse_and_validate_scala_3_9(&first.bytes).unwrap();
    assert_eq!(
        (
            file.header().major_version,
            file.header().minor_version,
            file.header().experimental_version
        ),
        (28, 9, 0)
    );
    assert_eq!(file.source_file().unwrap(), Some("Operations.scala"));
    let addresses = file.ast_address_index().unwrap();
    let class = first.output.declarations()[0].symbol;
    let method_declarations = first
        .output
        .declarations()
        .iter()
        .filter(|declaration| declaration.owner == class)
        .filter_map(|declaration| {
            let body = addresses
                .get(declaration.address)?
                .decode_defdef_body()
                .ok()?;
            let name = file.render_name(body.name).ok()?;
            (name != "<init>").then_some((name, *declaration, body))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        method_declarations
            .iter()
            .map(|(name, _, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["use", "identity", "first", "second", "zero"]
    );
    for ((name, symbol), (_, declaration, _)) in first.methods.iter().zip(&method_declarations) {
        assert_eq!(
            *symbol, declaration.symbol,
            "canonical declaration for {name}"
        );
    }

    let identity = method_declarations[1].1;
    let identity_parameters = first
        .output
        .declarations()
        .iter()
        .filter(|declaration| declaration.owner == identity.symbol)
        .collect::<Vec<_>>();
    assert_eq!(identity_parameters.len(), 1);
    let RawTree::Leaf(parameter_ref) = method_declarations[1].2.rhs.as_ref().unwrap() else {
        panic!("identity RHS must be a direct reference to its formal")
    };
    assert_eq!(parameter_ref.tag, TERMREFDIRECT_TAG);
    assert_eq!(
        parameter_ref.ast_ref().unwrap().address,
        identity_parameters[0].address
    );

    let RawTree::LengthNode(call_node) = method_declarations[0].2.rhs.as_ref().unwrap() else {
        panic!("use RHS must be APPLY")
    };
    assert_eq!(call_node.tag, APPLY_TAG);
    let call = call_node.decode_apply().unwrap();
    let RawTree::LengthNode(select_node) = call.function else {
        panic!("identity call function must be SELECTin")
    };
    let selected = select_node.decode_select_in().unwrap();
    let call_receiver = selected.qualifier.decode_qual_this().unwrap();
    let call_receiver_ident = call_receiver.child.decode_ident().unwrap();
    let call_receiver_class = call_receiver_ident.type_tree.decode_reference().unwrap();
    assert_eq!(
        call_receiver_class.reference,
        first.output.declarations()[0].address
    );
    assert!(matches!(
        file.render_signed_name(selected.name).unwrap(),
        Some(RenderedSignedName::Signed { original, signature })
            if original == "identity"
                && signature.result == "scala.Int"
                && signature.parameters == [RenderedParamSig::TermParameter("scala.Int".into())]
    ));
    assert_eq!(call.arguments.len(), 1);
    assert!(matches!(
        call.arguments[0],
        RawTree::Leaf(SimpleTerm {
            tag: INTCONST_TAG,
            value: TermValue::Int(42),
            ..
        })
    ));

    let first_select = method_declarations[2]
        .2
        .rhs
        .as_ref()
        .unwrap()
        .decode_select()
        .unwrap();
    assert_eq!(file.render_name(first_select.name).unwrap(), "second");
    let receiver = first_select.qualifier.decode_qual_this().unwrap();
    let receiver_ident = receiver.child.decode_ident().unwrap();
    let receiver_class = receiver_ident.type_tree.decode_reference().unwrap();
    assert_eq!(
        receiver_class.reference,
        first.output.declarations()[0].address
    );
    assert_eq!(method_declarations[4].2.clauses, [EMPTYCLAUSE_TAG]);
    assert!(matches!(
        method_declarations[4].2.rhs,
        Some(RawTree::Leaf(SimpleTerm {
            tag: INTCONST_TAG,
            value: TermValue::Int(0),
            ..
        }))
    ));
    assert_eq!(file.encode_validated().unwrap(), first.bytes);

    let mut semantic_store = SemanticStore::new();
    let semantic_definitions = Definitions::bootstrap(&mut semantic_store);
    let int_symbol = semantic_store
        .types
        .get(semantic_definitions.int)
        .reference_symbol()
        .unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut semantic_store, semantic_definitions)
        .with_resolver(Box::new(CanonicalIntResolver { symbol: int_symbol }));
    let semantic_index = unpickler.enter_symbols().unwrap().clone();
    let semantic_class = semantic_index
        .symbol_at(first.output.declarations()[0].address)
        .unwrap();
    let semantic_identity = semantic_index.symbol_at(identity.address).unwrap();
    let semantic_parameter = semantic_index
        .symbol_at(identity_parameters[0].address)
        .unwrap();
    assert_eq!(
        unpickler.symbol_state_at(identity.address).unwrap().0,
        SymbolKind::Method
    );
    assert_eq!(
        unpickler
            .symbol_state_at(identity_parameters[0].address)
            .unwrap()
            .0,
        SymbolKind::Parameter
    );
    let _ = unpickler.into_parts();
    assert_eq!(
        semantic_store.symbols.get(semantic_identity).owner,
        Some(semantic_class)
    );
    assert_eq!(
        semantic_store.symbols.get(semantic_parameter).owner,
        Some(semantic_identity)
    );
    assert_eq!(
        semantic_store
            .names
            .resolve(semantic_store.symbols.get(semantic_parameter).name.text()),
        "x"
    );

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let candidate = target.join("issue-1244/generated/example/Operations.tasty");
        fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        fs::write(&candidate, &first.bytes).unwrap();
        let temporary = target.join("issue-1244/tmp");
        fs::create_dir_all(&temporary).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg("--runtime")
            .arg(&candidate)
            .env("TMPDIR", temporary)
            .status()
            .expect("start explicitly requested Scala 3.9 Operations gate");
        assert!(
            result.success(),
            "Operations Scala interoperability gate failed"
        );
    }
}

#[test]
fn decisions_source_serializes_control_flow_and_runs_from_fresh_scala_class() {
    let first = compile_decisions();
    let second = compile_decisions();
    assert_eq!(
        first.bytes, second.bytes,
        "fresh source sessions must be deterministic"
    );

    let file = TastyFile::parse_and_validate_scala_3_9(&first.bytes).unwrap();
    assert_eq!(
        (
            file.header().major_version,
            file.header().minor_version,
            file.header().experimental_version
        ),
        (28, 9, 0)
    );
    assert_eq!(file.source_file().unwrap(), Some("Decisions.scala"));
    assert_eq!(
        first.output.intended_tasty_path(),
        "example/Decisions.tasty"
    );
    let addresses = file.ast_address_index().unwrap();
    let class = first.output.declarations()[0].symbol;
    let methods = first
        .output
        .declarations()
        .iter()
        .filter(|declaration| declaration.owner == class)
        .filter_map(|declaration| {
            let body = addresses
                .get(declaration.address)?
                .decode_defdef_body()
                .ok()?;
            let name = file.render_name(body.name).ok()?;
            (name != "<init>").then_some((name, *declaration, body))
        })
        .collect::<Vec<_>>();
    let names = methods
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["unit", "enabled", "disabled", "choose", "nested", "stored"]
    );
    assert_eq!(first.output.declarations().len(), 15);
    for ((expected_name, symbol), (name, declaration, _)) in first.methods.iter().zip(&methods) {
        assert_eq!(expected_name, name);
        assert_eq!(
            *symbol, declaration.symbol,
            "canonical method identity: {name}"
        );
        assert!(first.output.body(*symbol).is_some());
        let expected_parameters = match name.as_str() {
            "choose" => 3,
            "nested" | "stored" => 2,
            _ => 0,
        };
        assert_eq!(
            first
                .output
                .declarations()
                .iter()
                .filter(|entry| entry.owner == *symbol)
                .count(),
            expected_parameters,
            "parameter ownership/count for {name}"
        );
    }

    let stored = methods
        .iter()
        .find(|(name, _, _)| name == "stored")
        .unwrap();
    assert_eq!(stored.1.symbol, first.stored_method);
    let local_references = addresses
        .iter()
        .flat_map(|node| {
            node.ast_refs()
                .unwrap()
                .into_iter()
                .map(move |reference| (node.offset as u32, reference))
        })
        .filter(|(_, reference)| {
            reference.kind == AstRefKind::TermRefDirect
                && addresses
                    .get(reference.address)
                    .is_some_and(|node| node.tag == VALDEF_TAG)
        })
        .collect::<Vec<_>>();
    assert_eq!(local_references.len(), 1);
    let (reference_owner, local_reference) = local_references[0];
    let local_address = local_reference.address;
    let local_node = addresses.get(local_address).unwrap();
    assert_eq!(local_node.tag, VALDEF_TAG);
    let local_body = local_node.decode_definition().unwrap();
    assert_eq!(file.render_name(local_body.name()).unwrap(), "saved");
    assert_eq!(addresses.parent_of(local_address).unwrap().tag, BLOCK_TAG);
    let mut enclosing = reference_owner;
    loop {
        let Some(parent) = addresses.parent_of(enclosing) else {
            panic!("local reference is not nested in a method")
        };
        if parent.tag == DEFDEF_TAG {
            assert_eq!(parent.offset as u32, stored.1.address);
            break;
        }
        enclosing = parent.offset as u32;
    }

    let nodes = addresses.iter_nodes().collect::<Vec<_>>();
    for tag in [
        TRUECONST_TAG,
        FALSECONST_TAG,
        UNITCONST_TAG,
        IF_TAG,
        BLOCK_TAG,
        VALDEF_TAG,
        TERMREFDIRECT_TAG,
    ] {
        assert!(
            nodes.iter().any(|node| node.tag == tag),
            "missing TASTy tag {tag}"
        );
    }
    assert_eq!(nodes.iter().filter(|node| node.tag == IF_TAG).count(), 4);
    assert_eq!(
        nodes.iter().filter(|node| node.tag == VALDEF_TAG).count(),
        1
    );
    assert_eq!(file.encode_validated().unwrap(), first.bytes);

    let mut semantic_store = SemanticStore::new();
    let semantic_definitions = Definitions::bootstrap(&mut semantic_store);
    let builtin_symbols = [
        semantic_definitions.int,
        semantic_definitions.boolean,
        semantic_definitions.unit,
    ]
    .into_iter()
    .map(|ty| semantic_store.types.get(ty).reference_symbol().unwrap())
    .collect();
    let mut unpickler = TastyUnpickler::new(&file, &mut semantic_store, semantic_definitions)
        .with_resolver(Box::new(CanonicalScalaResolver {
            symbols: builtin_symbols,
        }));
    let semantic_index = unpickler.enter_symbols().unwrap().clone();
    let semantic_class = semantic_index
        .symbol_at(first.output.declarations()[0].address)
        .unwrap();
    let semantic_stored = semantic_index.symbol_at(stored.1.address).unwrap();

    let completed = first
        .output
        .declarations()
        .iter()
        .filter(|declaration| declaration.owner == class)
        .filter_map(|declaration| {
            let body = addresses
                .get(declaration.address)?
                .decode_defdef_body()
                .ok()?;
            let name = file.render_name(body.name).ok()?;
            (name != "<init>").then_some((name, *declaration))
        })
        .map(|(name, declaration)| {
            let method_info = unpickler.complete_symbol(declaration.address).unwrap();
            let parameter_infos = first
                .output
                .declarations()
                .iter()
                .filter(|parameter| parameter.owner == declaration.symbol)
                .map(|parameter| unpickler.complete_symbol(parameter.address).unwrap())
                .collect::<Vec<_>>();
            (name, method_info, parameter_infos)
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 6);
    drop(unpickler);
    assert_eq!(
        semantic_store.symbols.get(semantic_stored).owner,
        Some(semantic_class)
    );
    assert_eq!(
        semantic_store
            .names
            .resolve(semantic_store.symbols.get(semantic_stored).name.text()),
        "stored"
    );

    for (name, info, parameter_infos) in completed {
        let expected_result = match name.as_str() {
            "unit" => semantic_definitions.unit,
            "enabled" | "disabled" => semantic_definitions.boolean,
            _ => semantic_definitions.int,
        };
        let expected_parameters: &[TypeId] = match name.as_str() {
            "choose" => &[
                semantic_definitions.boolean,
                semantic_definitions.int,
                semantic_definitions.int,
            ],
            "nested" => &[semantic_definitions.boolean, semantic_definitions.boolean],
            "stored" => &[semantic_definitions.boolean, semantic_definitions.int],
            _ => &[],
        };
        assert_eq!(parameter_infos.len(), expected_parameters.len(), "{name}");
        match (name.as_str(), semantic_store.types.get(info)) {
            ("unit" | "enabled" | "disabled", Type::ByName { result }) => {
                assert_builtin_type(&semantic_store, *result, expected_result);
            }
            (_, Type::Method(method_type)) => {
                assert_eq!(method_type.kind, MethodKind::Plain, "{name}");
                assert_eq!(
                    method_type.params.len(),
                    expected_parameters.len(),
                    "{name}"
                );
                assert_builtin_type(&semantic_store, method_type.result, expected_result);
                for (parameter, expected) in method_type.params.iter().zip(expected_parameters) {
                    assert_builtin_type(&semantic_store, parameter.ty, *expected);
                }
            }
            (_, ty) => panic!("unexpected completed method type for {name}: {ty:?}"),
        }
        for (parameter, expected) in parameter_infos.iter().zip(expected_parameters) {
            assert_builtin_type(&semantic_store, *parameter, *expected);
        }
    }

    if std::env::var_os(RUN_SCALA_ORACLE).is_some() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let candidate = target.join("issue-1315/generated/example/Decisions.tasty");
        fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        fs::write(&candidate, &first.bytes).unwrap();
        let temporary = target.join("issue-1315/tmp");
        fs::create_dir_all(&temporary).unwrap();
        let result = Command::new("python3")
            .arg(root.join("tools/tasty-pickler-oracle/run.py"))
            .arg("verify")
            .arg("--runtime")
            .arg(&candidate)
            .env("TMPDIR", temporary)
            .status()
            .expect("start explicitly requested Scala 3.9 Decisions gate");
        assert!(
            result.success(),
            "Decisions Scala interoperability gate failed"
        );
    }
}

#[test]
fn generated_parameter_reference_unpickles_to_the_exact_owned_formal() {
    let source = SourceId::from_index(12);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(PARAMETERS).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(PARAMETERS).unwrap(),
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
        "Parameters.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("namer rejected Parameters fixture: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!()
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!("expected one class")
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_def) = &parsed.ast.get(*class_tree).kind else {
        panic!()
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
        panic!()
    };
    let constructor_tree = template.constructor;
    let constructor = index.symbol_at(source, constructor_tree).unwrap();
    let TreeKind::DefDef(constructor_def) = &parsed.ast.get(constructor_tree).kind else {
        panic!()
    };
    let clauses = |definition: &DefDef<Untyped>| {
        definition
            .value_param_clauses
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer.complete_symbol(class).unwrap();
    typer.complete_symbol(constructor).unwrap();
    let mut methods = Vec::new();
    for &tree in &template.body {
        let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
            continue;
        };
        let method = index.symbol_at(source, tree).unwrap();
        typer.complete_symbol(method).unwrap();
        let context = typer.expression_context_for(method).unwrap();
        let rhs = definition.rhs.expect("Parameters methods have bodies");
        let body = typer
            .type_expression_expected(rhs, context, definitions.int)
            .unwrap();
        methods.push(MethodPicklePlan {
            symbol: method,
            source_tree: tree,
            parameter_clauses: clauses(definition),
            body: Some(body),
        });
    }
    let bindings = |source, tree| typer.source_typed_index().get(source, tree);
    let plan = PicklePlan {
        store: typer.store(),
        definitions: &definitions,
        typed_ast: typer.typed_ast(),
        source_ast: &parsed.ast,
        source_index: &index,
        typed_bindings: &bindings,
        source: SourceMetadata {
            id: source,
            name: "Parameters.scala".into(),
        },
        source_root: parsed.root,
        class,
        class_tree: *class_tree,
        constructor: DefaultConstructorPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: clauses(constructor_def),
        },
        methods: &methods,
        local_values: &[],
    };
    let output = plan.validate().unwrap().pickle().unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.bytes()).unwrap();
    let addresses = file.ast_address_index().unwrap();
    let declarations = output.declarations();
    let pick = declarations
        .iter()
        .find(|declaration| {
            addresses
                .get(declaration.address)
                .and_then(|node| node.decode_defdef_body().ok())
                .is_some_and(|body| file.render_name(body.name).ok().as_deref() == Some("pick"))
        })
        .unwrap();
    let pick_body = addresses
        .get(pick.address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    let Some(RawTree::Leaf(reference)) = pick_body.rhs.as_ref() else {
        panic!("expected a direct formal reference")
    };
    let target_address = reference.ast_ref().unwrap().address;
    let parameters = declarations
        .iter()
        .filter(|declaration| declaration.owner == pick.symbol)
        .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert_eq!(target_address, parameters[1].address);

    let mut semantic_store = SemanticStore::new();
    let semantic_definitions = Definitions::bootstrap(&mut semantic_store);
    let int_symbol = semantic_store
        .types
        .get(semantic_definitions.int)
        .reference_symbol()
        .unwrap();
    let mut unpickler = TastyUnpickler::new(&file, &mut semantic_store, semantic_definitions)
        .with_resolver(Box::new(CanonicalIntResolver { symbol: int_symbol }));
    let semantic_index = unpickler.enter_symbols().unwrap().clone();
    let method_symbol = semantic_index.symbol_at(pick.address).unwrap();
    let target_symbol = semantic_index.symbol_at(target_address).unwrap();
    assert_eq!(
        unpickler.symbol_state_at(target_address).unwrap().0,
        SymbolKind::Parameter
    );
    let body_address = addresses
        .iter_tree_edges()
        .find(|edge| {
            edge.parent.offset == pick.address as usize && edge.child.tag == TERMREFDIRECT_TAG
        })
        .expect("method has a TERMREFdirect body edge")
        .child
        .offset as u32;
    let reference_type = unpickler.unpickle_type(body_address).unwrap();
    let _ = unpickler.into_parts();
    assert!(matches!(semantic_store.types.get(reference_type),
        Type::TermRef { target: TermRefTarget::Symbol(symbol), prefix }
            if *symbol == target_symbol
                && matches!(semantic_store.types.get(*prefix), Type::NoPrefix)));
    assert_eq!(
        semantic_store.symbols.get(target_symbol).owner,
        Some(method_symbol)
    );
    assert_eq!(
        semantic_store
            .names
            .resolve(semantic_store.symbols.get(target_symbol).name.text()),
        "second"
    );
}
