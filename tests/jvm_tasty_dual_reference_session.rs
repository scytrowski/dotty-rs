use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_classfile::{attribute::Attribute, class_file::ClassFile, reader::Reader};
use dotty_core::ast::TreeKind;
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceId, SourceText, SourceTypedIndex, SymbolId,
    SymbolInfo, SymbolKind, TermRefTarget, TreeId, Type, TypeRefTarget, Typed, Untyped, Visibility,
};
use dotty_jvm::{
    JvmClassInitializationPlan, JvmClassPlan, JvmCompilationUnit, JvmFieldPlan,
    JvmFieldSourceVisibility, JvmMethodPlan, JvmPrimaryConstructorPlan,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_pickler::{
    ClassFieldPicklePlan, ClassMemberPicklePlan, ClassStatePicklePlan, PrimaryConstructorInputPlan,
    SourceMetadata, SourceTypedBindings,
};
use dotty_typer::SourceTyper;

const SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_constructors/Pair.scala");

struct SessionBindings<'a> {
    index: &'a SourceTypedIndex,
    classes: &'a [SymbolId],
    methods: &'a [SymbolId],
}

impl SourceTypedBindings for SessionBindings<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.index.get(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.classes
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        self.methods
    }
}

#[test]
fn rust_frontend_session_keeps_two_parameter_and_field_alias_identities() {
    let source_id = SourceId::from_index(1874);
    let source_text = SourceText::new(SOURCE).unwrap();
    let scanner = ContextualScanner::new(SOURCE).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty());
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "Pair.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("the audit source names without diagnostics: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("the source has one package root");
    };
    assert_eq!(package.stats.len(), 2);

    let mut pair_data = None;
    let mut counter_class = None;
    let mut counter_get = None;
    let mut counter_constructor = None;
    let mut counter_constructor_params = None;
    let mut counter_current = None;
    let mut counter_set = None;
    let mut counter_get_body = None;
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    let mut roots = Vec::new();
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_definition) = &parsed.ast.get(class_tree).kind else {
            panic!("both declarations are ordinary classes");
        };
        let TreeKind::Template(template) = &parsed.ast.get(class_definition.rhs).kind else {
            panic!("each class has a template");
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let constructor_tree = template.constructor;
        let constructor = source_index.symbol_at(source_id, constructor_tree).unwrap();
        let TreeKind::DefDef(constructor_definition) = &parsed.ast.get(constructor_tree).kind
        else {
            panic!("each template has a primary constructor");
        };
        assert_eq!(constructor_definition.value_param_clauses.len(), 1);
        let constructor_params = constructor_definition.value_param_clauses[0]
            .iter()
            .map(|&parameter_tree| {
                source_index
                    .derived_symbol_at(constructor, source_id, parameter_tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        typer.complete_symbol(class).unwrap();
        typer.complete_symbol(constructor).unwrap();
        for &parameter in &constructor_params {
            typer.complete_symbol(parameter).unwrap();
        }
        let name = typer
            .store()
            .names
            .resolve(class_definition.name.as_name().text());
        if name == "Pair" {
            let [first_tree, second_tree] =
                constructor_definition.value_param_clauses[0].as_slice()
            else {
                panic!("Pair has two ordered constructor formals");
            };
            let template_params = [*first_tree, *second_tree]
                .map(|tree| source_index.symbol_at(source_id, tree).unwrap());
            for (source_parameter_tree, template_parameter) in
                [*first_tree, *second_tree].into_iter().zip(template_params)
            {
                typer.complete_symbol(template_parameter).unwrap();
                assert_eq!(
                    typer.store().symbols.get(template_parameter).kind,
                    SymbolKind::Parameter
                );
                assert_eq!(
                    typer.store().symbols.get(template_parameter).owner,
                    Some(class)
                );
                assert!(matches!(
                    source_index.definition_of(template_parameter),
                    Some(dotty_core::SourceDefinition::Canonical { source, tree })
                        if source == source_id && tree == source_parameter_tree
                ));
                let SymbolInfo::Complete(template_type) =
                    typer.store().symbols.get(template_parameter).info
                else {
                    panic!("template-owned parameter type completes in the same session");
                };
                assert!(matches!(
                    typer.store().types.get(template_type),
                    Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == source_index.symbol_at(source_id, package.stats[0]).unwrap()
                ));
            }
            let [first_constructor_param, second_constructor_param] = constructor_params.as_slice()
            else {
                panic!("Pair has two constructor-owned formal identities");
            };
            assert_ne!(template_params[0], template_params[1]);
            assert_ne!(*first_constructor_param, *second_constructor_param);
            assert_ne!(template_params[0], *first_constructor_param);
            assert_ne!(template_params[1], *second_constructor_param);
            for (template_parameter, constructor_parameter) in template_params
                .into_iter()
                .zip(constructor_params.iter().copied())
            {
                assert_eq!(
                    typer.store().symbols.get(template_parameter).kind,
                    SymbolKind::Parameter
                );
                assert_eq!(
                    typer.store().symbols.get(template_parameter).owner,
                    Some(class)
                );
                assert!(matches!(
                    source_index.definition_of(constructor_parameter),
                    Some(dotty_core::SourceDefinition::Derived { source, .. }) if source == source_id
                ));
                let SymbolInfo::Complete(parameter_type) =
                    typer.store().symbols.get(constructor_parameter).info
                else {
                    panic!("constructor formal type is completed in this session");
                };
                assert!(matches!(
                    typer.store().types.get(parameter_type),
                    Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == source_index.symbol_at(source_id, package.stats[0]).unwrap()
                ));
            }
            let mut fields = Vec::new();
            let mut methods = Vec::new();
            for &member_tree in &template.body {
                match &parsed.ast.get(member_tree).kind {
                    TreeKind::ValDef(definition) => {
                        let field = source_index.symbol_at(source_id, member_tree).unwrap();
                        typer.complete_symbol(field).unwrap();
                        let SymbolInfo::Complete(field_type) =
                            typer.store().symbols.get(field).info
                        else {
                            panic!("field signature is complete");
                        };
                        let rhs = definition.rhs.unwrap();
                        let context = typer.field_initializer_context_for(field).unwrap();
                        let typed_rhs = typer
                            .type_expression_expected(rhs, context, field_type)
                            .unwrap();
                        assert_eq!(
                            typer.source_typed_index().get(source_id, rhs),
                            Some(typed_rhs)
                        );
                        assert!(matches!(parsed.ast.get(rhs).kind, TreeKind::Ident(_)));
                        let source_position = fields.len();
                        let template_parameter = template_params[source_position];
                        assert_eq!(typer.store().symbols.get(field).owner, Some(class));
                        assert_eq!(typer.store().symbols.get(field).kind, SymbolKind::Field);
                        let typed_target = match typer
                            .store()
                            .types
                            .get(typer.typed_ast().get(typed_rhs).ty)
                        {
                            Type::TermRef {
                                target: TermRefTarget::Symbol(symbol),
                                ..
                            } => *symbol,
                            other => panic!(
                                "typed field initializer retains exact parameter TermRef: {other:?}"
                            ),
                        };
                        assert_eq!(typed_target, constructor_params[source_position]);
                        fields.push((
                            field,
                            member_tree,
                            rhs,
                            typed_rhs,
                            template_parameter,
                            field_type,
                        ));
                        roots.push((member_tree, rhs, typed_rhs));
                    }
                    TreeKind::DefDef(definition) if member_tree != constructor_tree => {
                        let method = source_index.symbol_at(source_id, member_tree).unwrap();
                        typer.complete_symbol(method).unwrap();
                        let rhs = definition.rhs.unwrap();
                        let context = typer.expression_context_for(method).unwrap();
                        let typed_rhs = typer.type_expression(rhs, context).unwrap();
                        assert_eq!(
                            typer.source_typed_index().get(source_id, rhs),
                            Some(typed_rhs)
                        );
                        methods.push((method, member_tree, rhs, typed_rhs));
                        roots.push((member_tree, rhs, typed_rhs));
                    }
                    other => panic!("supported source member: {other:?}"),
                }
            }
            assert_eq!(fields.len(), 2);
            assert_eq!(methods.len(), 2);
            pair_data = Some((class, constructor, constructor_params, fields, methods));
        } else if name == "Counter" {
            counter_class = Some(class);
            counter_constructor = Some(constructor);
            counter_constructor_params = Some(constructor_params.clone());
            for &member_tree in &template.body {
                match &parsed.ast.get(member_tree).kind {
                    TreeKind::ValDef(definition) => {
                        let field = source_index.symbol_at(source_id, member_tree).unwrap();
                        typer.complete_symbol(field).unwrap();
                        let SymbolInfo::Complete(field_type) =
                            typer.store().symbols.get(field).info
                        else {
                            panic!("Counter field signature is complete");
                        };
                        let rhs = definition.rhs.unwrap();
                        let context = typer.field_initializer_context_for(field).unwrap();
                        let typed_rhs = typer
                            .type_expression_expected(rhs, context, field_type)
                            .unwrap();
                        assert_eq!(
                            typer.source_typed_index().get(source_id, rhs),
                            Some(typed_rhs)
                        );
                        counter_current = Some((field, member_tree, rhs, typed_rhs, field_type));
                        roots.push((member_tree, rhs, typed_rhs));
                    }
                    TreeKind::DefDef(definition) if member_tree != constructor_tree => {
                        let method = source_index.symbol_at(source_id, member_tree).unwrap();
                        typer.complete_symbol(method).unwrap();
                        let parameters = definition
                            .value_param_clauses
                            .iter()
                            .flatten()
                            .map(|&tree| {
                                let parameter = source_index.symbol_at(source_id, tree).unwrap();
                                typer.complete_symbol(parameter).unwrap();
                                parameter
                            })
                            .collect::<Vec<_>>();
                        let rhs = definition.rhs.unwrap();
                        let context = typer.expression_context_for(method).unwrap();
                        let typed_rhs = typer.type_expression(rhs, context).unwrap();
                        assert_eq!(
                            typer.source_typed_index().get(source_id, rhs),
                            Some(typed_rhs)
                        );
                        let method_name = typer
                            .store()
                            .names
                            .resolve(definition.name.as_name().text());
                        if method_name == "get" {
                            counter_get = Some(method);
                            counter_get_body = Some((method, member_tree, rhs, typed_rhs));
                            roots.push((member_tree, rhs, typed_rhs));
                        } else if method_name == "set" {
                            counter_set = Some((method, member_tree, parameters, rhs, typed_rhs));
                            roots.push((member_tree, rhs, typed_rhs));
                        } else {
                            panic!("exact Counter source has only get and set");
                        }
                    }
                    other => panic!("supported Counter source member: {other:?}"),
                }
            }
        } else {
            panic!("exact source contains only Counter and Pair");
        }
    }
    let (pair_class, pair_constructor, constructor_params, fields, methods) = pair_data.unwrap();
    let counter_class = counter_class.unwrap();
    assert_eq!(
        typer.store().symbols.get(pair_constructor).owner,
        Some(pair_class)
    );
    assert_eq!(
        typer
            .store()
            .symbols
            .get(counter_constructor.unwrap())
            .owner,
        Some(counter_class)
    );
    for (_, _, _, _, _, field_type) in &fields {
        assert!(matches!(
            typer.store().types.get(*field_type),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == counter_class
        ));
    }
    let counter_get = counter_get.unwrap();
    for (method_index, (method, _, _, typed_rhs)) in methods.iter().enumerate() {
        let SymbolInfo::Complete(result_type) = typer.store().symbols.get(*method).info else {
            panic!("read method signature is complete");
        };
        let Type::Method(signature) = typer.store().types.get(result_type) else {
            panic!("method signature is a Method type");
        };
        let result_symbol = match typer.store().types.get(signature.result) {
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => *symbol,
            other => panic!("method result retains a canonical source type reference: {other:?}"),
        };
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(*typed_rhs).ty),
            Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. } if *symbol == result_symbol
        ));
        let TreeKind::Apply(application) = &typer.typed_ast().get(*typed_rhs).kind else {
            panic!("both methods retain their original call roots");
        };
        let TreeKind::Select(selection) = &typer.typed_ast().get(application.function).kind else {
            panic!("both reads retain their selected Counter.get call");
        };
        let receiver = typer.typed_ast().get(selection.qualifier);
        assert!(matches!(
            typer.store().types.get(receiver.ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. }
                if *symbol == fields[method_index].0
        ));
        assert!(matches!(
            typer.store().types.get(typer.typed_ast().get(application.function).ty),
            Type::TermRef { target: TermRefTarget::Symbol(symbol), .. } if *symbol == counter_get
        ));
    }
    assert!(roots.iter().all(|(_, rhs, typed)| {
        typer.source_typed_index().get(source_id, *rhs) == Some(*typed)
    }));
    let field_receipts = fields
        .iter()
        .map(|(field, _, _, typed, template_alias, field_type)| {
            (
                *field,
                *template_alias,
                *field_type,
                *typed,
                typer.typed_ast().get(*typed).ty,
            )
        })
        .collect::<Vec<_>>();
    let method_receipts = methods
        .iter()
        .map(|(method, _, _, typed)| {
            let SymbolInfo::Complete(signature) = typer.store().symbols.get(*method).info else {
                unreachable!();
            };
            let Type::Method(signature) = typer.store().types.get(signature) else {
                unreachable!();
            };
            let result = signature.result;
            let result_symbol = match typer.store().types.get(result) {
                Type::TypeRef {
                    target: TypeRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                _ => unreachable!(),
            };
            (
                *method,
                result,
                result_symbol,
                *typed,
                typer.typed_ast().get(*typed).ty,
            )
        })
        .collect::<Vec<_>>();

    // Build the existing JVM-only plan from these exact borrowed identities,
    // typed roots, and source order. This does not call the paired convenience
    // API, which reparses the source in its own independent session.
    let (
        counter_current_field,
        counter_current_tree,
        counter_current_rhs,
        counter_current_typed,
        _,
    ) = counter_current.unwrap();
    let (
        counter_set_method,
        counter_set_tree,
        counter_set_parameters,
        counter_set_rhs,
        counter_set_typed,
    ) = counter_set.unwrap();
    let (counter_get_method, counter_get_tree, counter_get_rhs, counter_get_typed) =
        counter_get_body.unwrap();
    assert!(matches!(
        typer.typed_ast().get(counter_get_typed).kind,
        TreeKind::Ident(_)
    ));
    assert!(matches!(
        typer
            .store()
            .types
            .get(typer.typed_ast().get(counter_get_typed).ty),
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        } if *symbol == counter_current_field
    ));
    let [pair_first, pair_second] = constructor_params.as_slice() else {
        panic!("Pair has its two constructor-owned formals in source order");
    };
    let counter_constructor = counter_constructor.unwrap();
    let counter_constructor_params = counter_constructor_params.unwrap();
    let wrong_owner_formal = counter_constructor_params[0];
    let counter_jvm = JvmClassPlan {
        class: counter_class,
        fields: vec![JvmFieldPlan {
            field: counter_current_field,
            source_visibility: JvmFieldSourceVisibility::PlainPrivate,
            initializer: Some(counter_current_typed),
        }],
        methods: vec![
            JvmMethodPlan {
                method: counter_get_method,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(counter_get_typed),
            },
            JvmMethodPlan {
                method: counter_set_method,
                parameters: counter_set_parameters,
                locals: Vec::new(),
                body: Some(counter_set_typed),
            },
        ],
        initialization: JvmClassInitializationPlan::PrimaryConstructor(
            JvmPrimaryConstructorPlan::from_source(
                counter_class,
                counter_constructor,
                counter_constructor_params,
                &parsed.ast,
                &source_index,
            )
            .unwrap(),
        ),
    };
    let pair_jvm = JvmClassPlan {
        class: pair_class,
        fields: fields
            .iter()
            .map(|(field, _, _, typed, _, _)| JvmFieldPlan {
                field: *field,
                source_visibility: JvmFieldSourceVisibility::PlainPrivate,
                initializer: Some(*typed),
            })
            .collect(),
        methods: methods
            .iter()
            .map(|(method, _, _, typed)| JvmMethodPlan {
                method: *method,
                parameters: Vec::new(),
                locals: Vec::new(),
                body: Some(*typed),
            })
            .collect(),
        initialization: JvmClassInitializationPlan::PrimaryConstructor(
            JvmPrimaryConstructorPlan::from_source(
                pair_class,
                pair_constructor,
                constructor_params.clone(),
                &parsed.ast,
                &source_index,
            )
            .unwrap(),
        ),
    };
    let jvm_unit = JvmCompilationUnit {
        store: typer.store(),
        typed_ast: typer.typed_ast(),
        classes: vec![counter_jvm, pair_jvm],
    };
    let jvm_metadata = jvm_unit.metadata(&definitions).unwrap();
    assert_eq!(
        jvm_metadata
            .iter()
            .map(|class| class.identity().internal_name())
            .collect::<Vec<_>>(),
        ["example/Counter", "example/Pair"]
    );
    let pair_metadata = &jvm_metadata[1];
    let pair_constructor_metadata = pair_metadata.constructor().unwrap();
    let pair_constructor_slots = pair_constructor_metadata
        .parameters()
        .iter()
        .map(|parameter| (parameter.symbol(), parameter.slot(), parameter.width()))
        .collect::<Vec<_>>();
    assert_eq!(
        pair_constructor_slots,
        [(*pair_first, 1, 1), (*pair_second, 2, 1)]
    );
    let JvmClassInitializationPlan::PrimaryConstructor(pair_constructor_plan) =
        &jvm_unit.classes[1].initialization
    else {
        panic!("Pair retains its primary-constructor source plan");
    };
    let formal_proof = pair_constructor_plan
        .prove_ordered_reference_formals(
            &parsed.ast,
            &source_index,
            typer.store(),
            &definitions,
            pair_constructor_metadata,
        )
        .unwrap();
    let proof_plan = pair_constructor_plan.clone();
    let proof_metadata = pair_constructor_metadata.clone();
    assert_eq!(formal_proof.class(), pair_class);
    assert_eq!(formal_proof.constructor(), pair_constructor);
    assert_eq!(formal_proof.source_id(), source_id);
    assert_eq!(formal_proof.referenced_class(), counter_class);
    assert_eq!(
        formal_proof.constructor_parameters(),
        &[*pair_first, *pair_second]
    );
    assert_eq!(
        formal_proof.template_parameters(),
        &[fields[0].4, fields[1].4]
    );
    let template_formal_type_ids = formal_proof.template_parameters().map(|parameter| {
        let SymbolInfo::Complete(ty) = typer.store().symbols.get(parameter).info else {
            panic!("template formal signatures are complete in this session");
        };
        ty
    });
    assert_ne!(
        template_formal_type_ids[0], template_formal_type_ids[1],
        "different TypeIds still prove one canonical class identity"
    );
    assert_eq!(
        formal_proof.descriptor(),
        "(Lexample/Counter;Lexample/Counter;)V"
    );
    assert_eq!(
        formal_proof
            .jvm_parameters()
            .iter()
            .map(|parameter| (parameter.symbol(), parameter.slot(), parameter.width()))
            .collect::<Vec<_>>(),
        [(*pair_first, 1, 1), (*pair_second, 2, 1)]
    );
    for invalid_order in [
        vec![*pair_second, *pair_first],
        vec![*pair_first, *pair_first],
        vec![*pair_first],
        vec![wrong_owner_formal, *pair_second],
    ] {
        let error = JvmPrimaryConstructorPlan::from_source(
            pair_class,
            pair_constructor,
            invalid_order,
            &parsed.ast,
            &source_index,
        )
        .expect_err("reordered, duplicate, missing, and wrong-owner IDs must be rejected");
        assert!(
            format!("{error:?}").contains("ParameterIdentity"),
            "{error:?}"
        );
    }
    assert_eq!(
        pair_metadata
            .fields()
            .iter()
            .map(|field| (field.symbol(), field.name(), field.descriptor()))
            .collect::<Vec<_>>(),
        [
            (fields[0].0, "left", "Lexample/Counter;"),
            (fields[1].0, "right", "Lexample/Counter;")
        ]
    );
    assert_eq!(
        pair_metadata
            .methods()
            .iter()
            .map(|method| (method.symbol(), method.name()))
            .collect::<Vec<_>>(),
        [(methods[0].0, "leftValue"), (methods[1].0, "rightValue")]
    );
    let emitted = [counter_class, pair_class]
        .into_iter()
        .map(|class| dotty_jvm::emit_class(&jvm_unit, class, &definitions).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        emitted
            .iter()
            .map(|class| class.internal_name())
            .collect::<Vec<_>>(),
        ["example/Counter", "example/Pair"]
    );
    assert!(emitted.iter().all(|class| !class.bytes().is_empty()));
    let decoded_jvm_classes = emitted
        .iter()
        .map(|class| ClassFile::decode(&mut Reader::new(class.bytes())).unwrap())
        .collect::<Vec<_>>();
    assert!(
        decoded_jvm_classes
            .iter()
            .all(|class| class.version.major == 65 && class.version.minor == 0)
    );
    let pair_classfile = &decoded_jvm_classes[1];
    let pair_constructor_code = pair_classfile
        .methods
        .iter()
        .find(|method| {
            pair_classfile
                .constant_pool
                .utf8(method.name_index)
                .is_ok_and(|name| name == "<init>")
        })
        .and_then(|method| match method.attributes.as_slice() {
            [Attribute::Code(code)] => Some(code.code),
            _ => None,
        })
        .expect("the emitted Pair constructor has one Code attribute");
    let first_argument_load = pair_constructor_code
        .iter()
        .position(|opcode| *opcode == 0x2b)
        .expect("the first Pair input uses aload_1");
    let first_field_store = pair_constructor_code[first_argument_load..]
        .iter()
        .position(|opcode| *opcode == 0xb5)
        .map(|offset| first_argument_load + offset)
        .expect("the first input initializes a field");
    let second_argument_load = pair_constructor_code[first_field_store + 1..]
        .iter()
        .position(|opcode| *opcode == 0x2c)
        .map(|offset| first_field_store + 1 + offset)
        .expect("the second Pair input uses aload_2");
    let second_field_store = pair_constructor_code[second_argument_load..]
        .iter()
        .position(|opcode| *opcode == 0xb5)
        .map(|offset| second_argument_load + offset)
        .expect("the second input initializes a field");
    assert!(first_argument_load < first_field_store);
    assert!(first_field_store < second_argument_load);
    assert!(second_argument_load < second_field_store);
    println!(
        "same-session JVM-only result: classes={:?} Pair.constructor={:?} parameter_slots={pair_constructor_slots:?} fields={:?}",
        emitted
            .iter()
            .map(|class| (class.internal_name(), class.bytes().len()))
            .collect::<Vec<_>>(),
        pair_constructor_metadata.descriptor(),
        pair_metadata
            .fields()
            .iter()
            .map(|field| (field.symbol(), field.name(), field.descriptor()))
            .collect::<Vec<_>>()
    );

    // The direct class-state control borrows the exact same session stores,
    // source index, and typed binding index. It reaches signature validation;
    // later alias validation and TASTy lowering remain masked by that guard.
    let planned_classes = [counter_class, pair_class];
    let planned_methods = [
        counter_get_method,
        counter_set_method,
        methods[0].0,
        methods[1].0,
    ];
    let source_typed_index = typer.source_typed_index();
    let typed_bindings = SessionBindings {
        index: source_typed_index,
        classes: &planned_classes,
        methods: &planned_methods,
    };
    let pair_members = [
        ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
            symbol: fields[0].0,
            source_tree: fields[0].1,
            typed_initializer: fields[0].3,
            declared_type: fields[0].5,
        }),
        ClassMemberPicklePlan::Field(ClassFieldPicklePlan {
            symbol: fields[1].0,
            source_tree: fields[1].1,
            typed_initializer: fields[1].3,
            declared_type: fields[1].5,
        }),
        ClassMemberPicklePlan::Method {
            symbol: methods[0].0,
            source_tree: methods[0].1,
        },
        ClassMemberPicklePlan::Method {
            symbol: methods[1].0,
            source_tree: methods[1].1,
        },
    ];
    assert_eq!(
        pair_members
            .iter()
            .map(|member| match member {
                ClassMemberPicklePlan::Field(field) => field.symbol,
                ClassMemberPicklePlan::Method { symbol, .. } => *symbol,
            })
            .collect::<Vec<_>>(),
        [fields[0].0, fields[1].0, methods[0].0, methods[1].0]
    );
    let pair_constructor_tree = source_index
        .definition_of(pair_constructor)
        .and_then(|definition| match definition {
            dotty_core::SourceDefinition::Canonical { source, tree } if source == source_id => {
                Some(tree)
            }
            _ => None,
        })
        .unwrap();
    let class_state = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &typed_bindings,
        source: SourceMetadata {
            id: source_id,
            name: "Pair.scala".to_owned(),
        },
        class: pair_class,
        class_tree: source_index
            .definition_of(pair_class)
            .and_then(|definition| match definition {
                dotty_core::SourceDefinition::Canonical { source, tree } if source == source_id => {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap(),
        constructor: PrimaryConstructorInputPlan {
            symbol: pair_constructor,
            source_tree: pair_constructor_tree,
            parameter_clauses: vec![constructor_params.clone()],
        },
        members: &pair_members,
    };
    let validated = class_state
        .validate()
        .expect("the exact two-field alias class state validates");
    let reference_fields = validated.same_batch_reference_fields().collect::<Vec<_>>();
    assert_eq!(reference_fields.len(), 2);
    assert_eq!(
        reference_fields
            .iter()
            .map(|field| field.field())
            .collect::<Vec<_>>(),
        [fields[0].0, fields[1].0]
    );
    assert_eq!(
        reference_fields
            .iter()
            .map(|field| field.template_parameter_alias().unwrap())
            .collect::<Vec<_>>(),
        fields.iter().map(|field| field.4).collect::<Vec<_>>()
    );
    assert_ne!(
        reference_fields[0].template_parameter_alias(),
        reference_fields[1].template_parameter_alias()
    );
    let mut stale_members = pair_members;
    let ClassMemberPicklePlan::Field(stale_right) = &mut stale_members[1] else {
        panic!("the second source-ordered member is the right alias field");
    };
    stale_right.typed_initializer = fields[0].3;
    let stale_plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &typed_bindings,
        source: SourceMetadata {
            id: source_id,
            name: "Pair.scala".to_owned(),
        },
        class: pair_class,
        class_tree: class_state.class_tree,
        constructor: class_state.constructor.clone(),
        members: &stale_members,
    };
    assert!(matches!(
        stale_plan.validate(),
        Err(dotty_tasty_pickler::PickleError::FieldInitializerBinding { .. })
    ));
    let mut wrong_type_members = pair_members;
    let ClassMemberPicklePlan::Field(wrong_type) = &mut wrong_type_members[1] else {
        unreachable!()
    };
    wrong_type.declared_type = definitions.int;
    let wrong_type_plan = ClassStatePicklePlan {
        store: typer.store(),
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        typed_bindings: &typed_bindings,
        source: SourceMetadata {
            id: source_id,
            name: "Pair.scala".to_owned(),
        },
        class: pair_class,
        class_tree: class_state.class_tree,
        constructor: class_state.constructor.clone(),
        members: &wrong_type_members,
    };
    assert!(matches!(
        wrong_type_plan.validate(),
        Err(dotty_tasty_pickler::PickleError::FieldType { .. })
    ));
    println!("same-session class-state alias receipts validated in source order");

    let named_roots = [
        (
            "Counter.current initializer",
            counter_current_field,
            counter_current_tree,
            counter_current_rhs,
            counter_current_typed,
        ),
        (
            "Counter.get body",
            counter_get_method,
            counter_get_tree,
            counter_get_rhs,
            counter_get_typed,
        ),
        (
            "Counter.set body",
            counter_set_method,
            counter_set_tree,
            counter_set_rhs,
            counter_set_typed,
        ),
        (
            "Pair.left initializer",
            fields[0].0,
            fields[0].1,
            fields[0].2,
            fields[0].3,
        ),
        (
            "Pair.right initializer",
            fields[1].0,
            fields[1].1,
            fields[1].2,
            fields[1].3,
        ),
        (
            "Pair.leftValue body (field left)",
            methods[0].0,
            methods[0].1,
            methods[0].2,
            methods[0].3,
        ),
        (
            "Pair.rightValue body (field right)",
            methods[1].0,
            methods[1].1,
            methods[1].2,
            methods[1].3,
        ),
    ];
    println!("named same-session roots: {named_roots:?}");

    println!(
        "Rust source session SourceId={source_id:?} Counter={counter_class:?} get={counter_get:?} Pair={pair_class:?} constructor={pair_constructor:?} template_parameters={:?} constructor_formals={constructor_params:?} fields={field_receipts:?} methods={method_receipts:?} source-to-typed-roots={roots:?}",
        fields
            .iter()
            .map(|(_, _, _, _, alias, _)| *alias)
            .collect::<Vec<_>>()
    );

    // Corrupt the exact inputs explicitly in this test-only branch. The
    // validator must reject stale source syntax and then a wrong completed
    // constructor signature without rebuilding or guessing identities.
    drop(class_state);
    drop(jvm_unit);
    drop(typer);
    let first_source_parameter = formal_proof.source_parameters()[0];
    let original_parameter_node = parsed.ast.get(first_source_parameter).clone();
    let first_type_tree = match parsed.ast.get(first_source_parameter).kind {
        TreeKind::ValDef(ref parameter) => parameter.tpt,
        _ => unreachable!("proof retains source ValDef parameters"),
    };
    let original_type_tree = parsed.ast.get(first_type_tree).clone();
    let wrong_type_name = store.names.intern("DifferentCounter");
    parsed.ast.get_mut(first_type_tree).kind = TreeKind::Ident(dotty_core::ast::Ident {
        name: dotty_core::Name::new(wrong_type_name, dotty_core::Namespace::Type),
        backquoted: false,
    });
    let source_corruption = proof_plan
        .prove_ordered_reference_formals(
            &parsed.ast,
            &source_index,
            &store,
            &definitions,
            &proof_metadata,
        )
        .expect_err("source type syntax corruption must be rejected");
    assert!(
        format!("{source_corruption:?}").contains("OrderedReferenceFormalIdentity"),
        "{source_corruption:?}"
    );
    *parsed.ast.get_mut(first_type_tree) = original_type_tree;

    let TreeKind::ValDef(parameter) = &mut parsed.ast.get_mut(first_source_parameter).kind else {
        unreachable!("proof retains source ValDef parameters");
    };
    parameter.metadata.modifiers = vec![dotty_core::ast::Modifier::ParamAccessor];
    let source_metadata_corruption = proof_plan
        .prove_ordered_reference_formals(
            &parsed.ast,
            &source_index,
            &store,
            &definitions,
            &proof_metadata,
        )
        .expect_err("constructor formal modifier corruption must be rejected");
    assert!(
        format!("{source_metadata_corruption:?}").contains("OrderedReferenceFormalIdentity"),
        "{source_metadata_corruption:?}"
    );
    *parsed.ast.get_mut(first_source_parameter) = original_parameter_node;

    for (symbol, corrupted_visibility, label) in [
        (
            formal_proof.template_parameters()[0],
            Visibility::Public,
            "first template-owned parameter",
        ),
        (
            formal_proof.template_parameters()[1],
            Visibility::Public,
            "second template-owned parameter",
        ),
        (
            formal_proof.constructor_parameters()[0],
            Visibility::Private,
            "first constructor-owned formal",
        ),
        (
            formal_proof.constructor_parameters()[1],
            Visibility::Private,
            "second constructor-owned formal",
        ),
    ] {
        let original_visibility = store.symbols.get(symbol).visibility;
        store.symbols.get_mut(symbol).visibility = corrupted_visibility;
        let visibility_corruption = proof_plan
            .prove_ordered_reference_formals(
                &parsed.ast,
                &source_index,
                &store,
                &definitions,
                &proof_metadata,
            )
            .unwrap_err();
        assert!(
            format!("{visibility_corruption:?}").contains("OrderedReferenceFormalIdentity"),
            "{label}: {visibility_corruption:?}"
        );
        store.symbols.get_mut(symbol).visibility = original_visibility;
    }

    let SymbolInfo::Complete(constructor_signature) = store.symbols.get(pair_constructor).info
    else {
        panic!("the completed constructor signature remains present");
    };
    let Type::Method(signature) = store.types.get_mut(constructor_signature) else {
        panic!("the constructor retains its MethodType");
    };
    signature.params[1].ty = definitions.int;
    let typed_signature_corruption = proof_plan
        .prove_ordered_reference_formals(
            &parsed.ast,
            &source_index,
            &store,
            &definitions,
            &proof_metadata,
        )
        .expect_err("completed signature corruption must be rejected");
    assert!(
        format!("{typed_signature_corruption:?}").contains("OrderedReferenceFormalIdentity"),
        "{typed_signature_corruption:?}"
    );
}

#[test]
fn paired_batch_emits_the_authenticated_dual_reference_alias_shape() {
    let outputs = with_paired_jvm_tasty_batch_plan(
        SOURCE,
        "Pair.scala",
        SourceId::from_index(1874),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    (
                        class.tasty_path.clone(),
                        class.tasty_plan.pickle().unwrap().bytes().to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        },
    )
    .expect("the paired producer emits the exact Counter and Pair source batch");
    assert_eq!(outputs.len(), 2);
    assert!(
        outputs
            .iter()
            .any(|(path, _)| path.ends_with("/Counter.tasty"))
    );
    assert!(
        outputs
            .iter()
            .any(|(path, _)| path.ends_with("/Pair.tasty"))
    );
}
