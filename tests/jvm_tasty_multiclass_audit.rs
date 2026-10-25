use dotty::jvm_tasty_plan::{PairedPlanError, with_paired_jvm_tasty_class_plan};
use dotty_classfile::access_flags::{ACC_FINAL, ACC_PRIVATE, ACC_PUBLIC};
use dotty_classfile::attribute::Attribute as ClassAttribute;
use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SymbolInfo, SymbolKind,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::tasty::{
    Attribute as TastyAttribute, DefinitionBody, RawName, StandardSection, TastyFile,
};
use dotty_typer::SourceTyper;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_multiclass/CounterGauge.scala");
const COUNTER_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_multiclass/scala39/example/Counter.class");
const COUNTER_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_multiclass/scala39/example/Counter.tasty");
const GAUGE_CLASS: &[u8] =
    include_bytes!("fixtures/jvm_tasty_multiclass/scala39/example/Gauge.class");
const GAUGE_TASTY: &[u8] =
    include_bytes!("fixtures/jvm_tasty_multiclass/scala39/example/Gauge.tasty");

#[test]
fn scala_39_two_class_oracle_has_distinct_classfiles_tasty_sidecars_and_uuids() {
    let cases = [
        (
            "Counter",
            "current",
            COUNTER_CLASS,
            COUNTER_TASTY,
            [
                0x00, 0x75, 0x15, 0x4b, 0xea, 0xa4, 0xba, 0xe8, 0x00, 0x4b, 0x8b, 0x70, 0x90, 0xf4,
                0x34, 0x1b,
            ],
        ),
        (
            "Gauge",
            "reading",
            GAUGE_CLASS,
            GAUGE_TASTY,
            [
                0x00, 0x5b, 0x42, 0xac, 0x86, 0x0b, 0xfb, 0xe8, 0x00, 0x5a, 0x27, 0xb4, 0xf8, 0xed,
                0xbd, 0x1b,
            ],
        ),
    ];

    for (class_name, field_name, class_bytes, tasty_bytes, expected_uuid) in cases {
        let expected_class_length = if class_name == "Counter" { 648 } else { 644 };
        let expected_tasty_length = if class_name == "Counter" { 576 } else { 573 };
        assert_eq!(class_bytes.len(), expected_class_length);
        assert_eq!(tasty_bytes.len(), expected_tasty_length);
        let mut reader = Reader::new(class_bytes);
        let class = ClassFile::decode(&mut reader).unwrap();
        assert!(reader.is_at_end());
        assert_eq!((class.version.major, class.version.minor), (61, 0));
        assert_eq!(
            class.constant_pool.class_name(class.this_class).unwrap(),
            format!("example/{class_name}")
        );
        let [field] = class.fields.as_slice() else {
            panic!("{class_name} has exactly one backing field");
        };
        assert_eq!(class.constant_pool.utf8(field.name_index), Ok(field_name));
        assert_eq!(class.constant_pool.utf8(field.descriptor_index), Ok("I"));
        assert!(field.access_flags.contains(ACC_PRIVATE));
        assert!(!field.access_flags.contains(ACC_FINAL));
        let methods = class
            .methods
            .iter()
            .map(|method| {
                (
                    class.constant_pool.utf8(method.name_index).unwrap(),
                    class.constant_pool.utf8(method.descriptor_index).unwrap(),
                    method.access_flags,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            methods
                .iter()
                .map(|(name, descriptor, _)| (*name, *descriptor))
                .collect::<Vec<_>>(),
            [("<init>", "(I)V"), ("get", "()I"), ("set", "(I)V")]
        );
        assert!(
            methods
                .iter()
                .all(|(_, _, flags)| flags.contains(ACC_PUBLIC))
        );
        let class_source_file = class
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                ClassAttribute::SourceFile(index) => {
                    Some(class.constant_pool.utf8(*index).unwrap())
                }
                _ => None,
            })
            .expect("classfile records its source basename");
        assert_eq!(class_source_file, "CounterGauge.scala");
        assert_eq!(
            *class.tasty_link_id().unwrap().unwrap().as_bytes(),
            expected_uuid
        );

        let tasty = TastyFile::parse_and_validate_scala_3_9(tasty_bytes).unwrap();
        assert_eq!(
            (
                tasty.header().major_version,
                tasty.header().minor_version,
                tasty.header().experimental_version,
            ),
            (28, 9, 0)
        );
        assert_eq!(tasty.header().uuid, expected_uuid);
        let tasty_attributes = tasty.attributes().unwrap().unwrap();
        assert_eq!(tasty_attributes.len(), 1);
        let TastyAttribute::SourceFile(source_file_name) = tasty_attributes[0] else {
            panic!("TASTy Attributes section records the source file");
        };
        assert_eq!(
            tasty.names().render(source_file_name).unwrap(),
            "tests/fixtures/jvm_tasty_multiclass/CounterGauge.scala"
        );
        assert!(tasty.section(StandardSection::Asts).is_some());
        let asts = tasty.asts().unwrap();
        assert_eq!(asts.len(), 1, "each sidecar has one package root");
        let package = asts.get(0).unwrap().decode_package().unwrap();
        let package_path = package.path_name().expect("direct package path");
        assert_eq!(tasty.names().render(package_path).unwrap(), "example");
        assert_eq!(package.stats.len(), 1, "each package root owns one class");
        let declaration = package.stats.get(0).unwrap();
        let DefinitionBody::TypeDef { name, .. } = declaration.decode_definition_body().unwrap()
        else {
            panic!("package root contains a class TypeDef");
        };
        assert_eq!(tasty.names().render(name).unwrap(), class_name);
        assert!(matches!(tasty.names().get(name), Some(RawName::Utf8(_))));
    }
    assert_ne!(
        TastyFile::parse(COUNTER_TASTY).unwrap().header().uuid,
        TastyFile::parse(GAUGE_TASTY).unwrap().header().uuid,
        "these independent class sidecars have distinct measured UUIDs"
    );
}

#[test]
fn counter_and_gauge_keep_distinct_canonical_symbols_in_one_typer_session() {
    let source_id = SourceId::from_index(420);
    let source = SourceText::new(SOURCE).unwrap();
    let scanner = ContextualScanner::new(SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "CounterGauge.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        other => panic!("the two-class audit source names cleanly: {other:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("one compilation unit has a package root");
    };
    assert_eq!(package.stats.len(), 2);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    let mut observed = Vec::new();

    for class_tree in &package.stats {
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(*class_tree).kind else {
            panic!("each source declaration is a class");
        };
        let class = index.symbol_at(source_id, *class_tree).unwrap();
        assert_eq!(typer.store().symbols.get(class).kind, SymbolKind::Class);
        let class_name = typer
            .store()
            .names
            .resolve(class_definition.name.as_name().text())
            .to_owned();
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("class has a template");
        };
        let constructor = index.symbol_at(source_id, template.constructor).unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(template.constructor).kind
        else {
            panic!("template has source constructor");
        };
        typer.complete_symbol(class).unwrap();
        typer.complete_symbol(constructor).unwrap();
        let [parameter_tree] = constructor_definition.value_param_clauses[0].as_slice() else {
            panic!("constructor has one start parameter");
        };
        let parameter = index
            .derived_symbol_at(constructor, source_id, *parameter_tree)
            .unwrap();
        typer.complete_symbol(parameter).unwrap();
        assert_eq!(
            typer.store().symbols.get(parameter).owner,
            Some(constructor)
        );

        let mut field_id = None;
        let mut method_ids = Vec::new();
        let mut typed_roots = Vec::new();
        for member_tree in &template.body {
            match &parsed.ast.get(*member_tree).kind {
                TreeKind::ValDef(definition) => {
                    let field = index.symbol_at(source_id, *member_tree).unwrap();
                    assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
                    assert_eq!(typer.store().symbols.get(field).owner, Some(class));
                    assert_eq!(
                        typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text()),
                        if class_name == "Counter" {
                            "current"
                        } else {
                            "reading"
                        }
                    );
                    typer.complete_symbol(field).unwrap();
                    let rhs = definition.rhs.unwrap();
                    let field_type = match typer.store().symbols.get(field).info {
                        SymbolInfo::Complete(field_type) => field_type,
                        _ => panic!("field type completes"),
                    };
                    let context = typer.field_initializer_context_for(field).unwrap();
                    let typed = typer
                        .type_expression_expected(rhs, context, field_type)
                        .unwrap();
                    assert_eq!(typer.source_typed_index().get(source_id, rhs), Some(typed));
                    assert!(typer.typed_ast().get(typed).position.is_some());
                    typed_roots.push(typed);
                    field_id = Some(field);
                }
                TreeKind::DefDef(definition) if *member_tree != template.constructor => {
                    let method = index.symbol_at(source_id, *member_tree).unwrap();
                    assert_eq!(typer.store().symbols.get(method).kind, SymbolKind::Method);
                    assert_eq!(typer.store().symbols.get(method).owner, Some(class));
                    typer.complete_symbol(method).unwrap();
                    for parameter_tree in definition.value_param_clauses.iter().flatten() {
                        let parameter = index.symbol_at(source_id, *parameter_tree).unwrap();
                        typer.complete_symbol(parameter).unwrap();
                    }
                    let rhs = definition.rhs.unwrap();
                    let context = typer.expression_context_for(method).unwrap();
                    let typed = typer.type_expression(rhs, context).unwrap();
                    assert_eq!(typer.source_typed_index().get(source_id, rhs), Some(typed));
                    assert!(typer.typed_ast().get(typed).position.is_some());
                    typed_roots.push(typed);
                    method_ids.push((
                        typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text())
                            .to_owned(),
                        method,
                    ));
                }
                other => panic!("unexpected member in bounded source fixture: {other:?}"),
            }
        }
        assert_eq!(method_ids.len(), 2);
        assert_eq!(method_ids[0].0, "get");
        assert_eq!(method_ids[1].0, "set");
        assert_eq!(typed_roots.len(), 3);
        observed.push((
            class_name,
            class,
            constructor,
            parameter,
            field_id.unwrap(),
            method_ids,
        ));
    }

    assert_eq!(
        observed
            .iter()
            .map(|(name, ..)| name.as_str())
            .collect::<Vec<_>>(),
        ["Counter", "Gauge"]
    );
    let (_, counter, counter_ctor, counter_param, counter_field, counter_methods) = &observed[0];
    let (_, gauge, gauge_ctor, gauge_param, gauge_field, gauge_methods) = &observed[1];
    assert_ne!(counter, gauge);
    assert_ne!(counter_ctor, gauge_ctor);
    assert_ne!(counter_param, gauge_param);
    assert_ne!(counter_field, gauge_field);
    assert_ne!(counter_methods[0].1, gauge_methods[0].1);
    assert_ne!(counter_methods[1].1, gauge_methods[1].1);

    assert!(matches!(
        with_paired_jvm_tasty_class_plan(
            SOURCE,
            "CounterGauge.scala",
            SourceId::from_index(421),
            |_| ()
        ),
        Err(PairedPlanError::UnsupportedSourceShape(
            "exactly one top-level class; multi-class source units are not supported"
        ))
    ));
}
