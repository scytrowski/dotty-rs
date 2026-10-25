use dotty_classfile::access_flags::{ACC_FINAL, ACC_PRIVATE};
use dotty_classfile::classfile::{
    ClassFile, ClassFileBuilder, ClassFileVersion, Reader, TastyLinkId,
};
use dotty_core::ast::TreeKind;
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolKind};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmFieldPlan,
    JvmFieldSourceVisibility, JvmMethodPlan, JvmPrimaryConstructorPlan,
};
use dotty_jvm_tasty_contract::{
    JvmTastyContractError, validate_classfile_tasty_link, validate_jvm_tasty_class,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_pickler::{
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan, PrimaryConstructorInputPlan,
    SourceMetadata,
};
use dotty_typer::SourceTyper;

const COUNTER: &str = "package example\n\nclass Counter(start: Int):\n  private var current: Int = start\n\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n";

#[test]
fn one_session_counter_proves_source_tasty_and_jvm_abi_agreement() {
    with_counter_pair(COUNTER, SourceId::from_index(40), |unit, tasty| {
        let proof = validate_jvm_tasty_class(unit, tasty).unwrap();
        assert_eq!(proof.internal_name(), "example/Counter");
        assert_eq!(proof.tasty_path(), "example/Counter.tasty");
        let metadata = proof.jvm_metadata();
        assert_eq!(
            metadata.constructor().unwrap().descriptor().as_str(),
            "(I)V"
        );
        assert_eq!(metadata.fields().len(), 1);
        assert_eq!(metadata.fields()[0].descriptor(), "I");
        assert!(metadata.fields()[0].access_flags().contains(ACC_PRIVATE));
        assert!(!metadata.fields()[0].access_flags().contains(ACC_FINAL));
        assert_eq!(
            metadata
                .methods()
                .iter()
                .map(|method| (method.name(), method.descriptor().as_str()))
                .collect::<Vec<_>>(),
            [("get", "()I"), ("set", "(I)V")]
        );
    });
}

#[test]
fn unsupported_tasty_method_body_is_rejected_before_pair_proof() {
    let counter_with_loop = "package example\n\nclass Counter(start: Int):\n  private var current: Int = start\n\n  def get(): Int = current\n  def set(value: Int): Unit = while current < value do current = current + 1\n";
    with_counter_pair(
        counter_with_loop,
        SourceId::from_index(46),
        |unit, tasty| {
            assert!(tasty.pickle().is_err());
            assert!(matches!(
                validate_jvm_tasty_class(unit, tasty),
                Err(JvmTastyContractError::TastyPlan(_))
            ));
        },
    );
}

#[test]
fn independent_numeric_arena_ids_do_not_prove_shared_session_provenance() {
    with_counter_pair(COUNTER, SourceId::from_index(41), |first_unit, _| {
        with_counter_pair(COUNTER, SourceId::from_index(41), |_, second_tasty| {
            let result = validate_jvm_tasty_class(first_unit, second_tasty);
            assert!(matches!(
                result,
                Err(JvmTastyContractError::DifferentSession("semantic store"))
            ));
        });
    });
}

#[test]
fn mismatched_typed_initializer_root_is_rejected_before_a_pair_proof() {
    with_counter_pair(COUNTER, SourceId::from_index(42), |unit, tasty| {
        let class_plan = &unit.classes[0];
        let field = class_plan.fields[0];
        let mut changed_classes = unit.classes.clone();
        changed_classes[0].fields[0] = JvmFieldPlan {
            initializer: class_plan.methods[0].body,
            ..field
        };
        let changed = JvmCompilationUnit {
            store: unit.store,
            typed_ast: unit.typed_ast,
            classes: changed_classes,
        };
        let result = validate_jvm_tasty_class(&changed, tasty);
        let error = result.err().expect("mismatched root must fail");
        assert!(
            matches!(error, JvmTastyContractError::JvmPlan(_))
                || matches!(error, JvmTastyContractError::TypedRootMismatch { .. })
        );
    });
}

#[test]
fn missing_jvm_method_is_rejected_by_canonical_identity() {
    with_counter_pair(COUNTER, SourceId::from_index(44), |unit, tasty| {
        let missing = unit.classes[0].methods[0].method;
        let mut classes = unit.classes.clone();
        classes[0].methods.remove(0);
        let changed = JvmCompilationUnit {
            store: unit.store,
            typed_ast: unit.typed_ast,
            classes,
        };
        let result = validate_jvm_tasty_class(&changed, tasty);
        assert!(matches!(
            result,
            Err(JvmTastyContractError::MemberMissing {
                member,
                side: "JVM method plan"
            }) if member == missing
        ));
    });
}

#[test]
fn immutable_source_field_is_not_mistaken_for_the_audited_mutable_backing_field() {
    let immutable_counter = "package example\nclass Counter(start: Int):\n  private val current: Int = start\n  def get(): Int = current\n";
    with_counter_pair(
        immutable_counter,
        SourceId::from_index(45),
        |unit, tasty| {
            let result = validate_jvm_tasty_class(unit, tasty);
            assert!(matches!(
                result,
                Err(JvmTastyContractError::FieldShapeMismatch { .. })
            ));
        },
    );
}

#[test]
fn same_simple_class_name_keeps_distinct_package_artifact_paths() {
    for (source, expected) in [
        (
            "package left\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n",
            "left/Counter.tasty",
        ),
        (
            "package right\nclass Counter(start: Int):\n  private var current: Int = start\n  def get(): Int = current\n  def set(value: Int): Unit = current = value\n",
            "right/Counter.tasty",
        ),
    ] {
        with_counter_pair(source, SourceId::from_index(43), |unit, tasty| {
            let proof = validate_jvm_tasty_class(unit, tasty).unwrap();
            assert_eq!(proof.tasty_path(), expected);
        });
    }
}

#[test]
fn stale_classfile_link_uuid_is_rejected_against_validated_tasty_header() {
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jvm_tasty_interop/scala39/example");
    let class_bytes = std::fs::read(fixture_dir.join("Counter.class")).unwrap();
    let tasty_bytes = std::fs::read(fixture_dir.join("Counter.tasty")).unwrap();
    let mut reader = Reader::new(&class_bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    validate_classfile_tasty_link(&class, &tasty_bytes).unwrap();

    let mut builder = ClassFileBuilder::new(
        "example/Counter",
        ClassFileVersion {
            major: 61,
            minor: 0,
        },
    )
    .unwrap();
    builder
        .tasty_link(TastyLinkId::from_bytes([0; TastyLinkId::LENGTH]))
        .unwrap();
    let stale_bytes = builder.encode().unwrap();
    let mut stale_reader = Reader::new(&stale_bytes);
    let stale = ClassFile::decode(&mut stale_reader).unwrap();
    assert!(matches!(
        validate_classfile_tasty_link(&stale, &tasty_bytes),
        Err(JvmTastyContractError::StaleTastyLink { .. })
    ));
}

fn with_counter_pair<R>(
    source_text: &str,
    source: SourceId,
    check: impl FnOnce(
        &JvmCompilationUnit<'_>,
        &dotty_tasty_pickler::ValidatedClassStatePlan<'_, '_>,
    ) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source_text).unwrap();
    assert!(scanner.diagnostics().is_empty());
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
        "Counter.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("namer rejected Counter: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("expected package")
    };
    let [class_tree] = package.stats.as_slice() else {
        panic!("expected one class")
    };
    let class = index.symbol_at(source, *class_tree).unwrap();
    let TreeKind::TypeDef(class_definition) = &parsed.ast.get(*class_tree).kind else {
        panic!("expected class definition")
    };
    let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
        panic!("expected template")
    };
    let constructor_tree = template.constructor;
    let constructor = index.symbol_at(source, constructor_tree).unwrap();
    let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(constructor_tree).kind else {
        panic!("expected constructor")
    };
    let constructor_parameters = constructor_definition
        .value_param_clauses
        .iter()
        .map(|clause| {
            clause
                .iter()
                .map(|tree| index.derived_symbol_at(constructor, source, *tree).unwrap())
                .collect::<Vec<_>>()
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
    for parameter in constructor_parameters.iter().flatten().copied() {
        typer.complete_symbol(parameter).unwrap();
    }
    let mut tasty_members = Vec::new();
    let mut jvm_fields = Vec::new();
    let mut jvm_methods = Vec::new();
    for &tree in &template.body {
        match &parsed.ast.get(tree).kind {
            TreeKind::ValDef(definition) => {
                let symbol = index.symbol_at(source, tree).unwrap();
                assert_eq!(typer.store().symbols.get(symbol).kind, SymbolKind::Field);
                typer.complete_symbol(symbol).unwrap();
                let declared_type = match typer.store().symbols.get(symbol).info {
                    dotty_core::SymbolInfo::Complete(ty) => ty,
                    info => panic!("field type incomplete: {info:?}"),
                };
                let rhs = definition.rhs.expect("Counter field initializer");
                let context = typer.field_initializer_context_for(symbol).unwrap();
                let typed_initializer = typer
                    .type_expression_expected(rhs, context, declared_type)
                    .unwrap();
                tasty_members.push(ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
                    symbol,
                    source_tree: tree,
                    typed_initializer,
                    declared_type,
                }));
                jvm_fields.push(JvmFieldPlan {
                    field: symbol,
                    source_visibility: JvmFieldSourceVisibility::PlainPrivate,
                    initializer: Some(typed_initializer),
                });
            }
            TreeKind::DefDef(definition) => {
                let symbol = index.symbol_at(source, tree).unwrap();
                if symbol == constructor {
                    continue;
                }
                typer.complete_symbol(symbol).unwrap();
                let params = definition
                    .value_param_clauses
                    .iter()
                    .flatten()
                    .map(|tree| index.symbol_at(source, *tree).unwrap())
                    .collect::<Vec<_>>();
                let rhs = definition.rhs.expect("method RHS");
                let context = typer.expression_context_for(symbol).unwrap();
                let body = typer.type_expression(rhs, context).unwrap();
                tasty_members.push(ClassMemberPicklePlan::Method {
                    symbol,
                    source_tree: tree,
                });
                jvm_methods.push(JvmMethodPlan {
                    method: symbol,
                    parameters: params,
                    locals: Vec::new(),
                    body: Some(body),
                });
            }
            _ => panic!("unsupported Counter member"),
        }
    }
    let jvm_constructor = JvmPrimaryConstructorPlan::from_source(
        class,
        constructor,
        constructor_parameters.iter().flatten().copied().collect(),
        &parsed.ast,
        &index,
    )
    .unwrap();
    let typed_bindings = |source, tree| typer.source_typed_index().get(source, tree);
    let tasty_input = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &index,
        typed_bindings: &typed_bindings,
        source: SourceMetadata {
            id: source,
            name: "Counter.scala".into(),
        },
        class,
        class_tree: *class_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: constructor,
            source_tree: constructor_tree,
            parameter_clauses: constructor_parameters,
        },
        members: &tasty_members,
    };
    let validated_tasty = tasty_input.validate().unwrap();
    let unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![JvmClassPlan {
            class,
            methods: jvm_methods,
            fields: jvm_fields,
            initialization: JvmClassInitializationPlan::PrimaryConstructor(jvm_constructor),
        }],
    };
    check(&unit, &validated_tasty)
}
