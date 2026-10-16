#[allow(dead_code)]
mod support;

use dotty_core::ast::{Literal, Modifier, TreeKind};
use dotty_core::{Constant, SymbolFlags, SymbolInfo, SymbolKind, Type, Visibility};
use dotty_tasty::tasty::*;
use dotty_tasty_pickler::*;
use support::Fixture;

fn emit(f: &Fixture) -> Result<DeclarationSkeleton, PickleError> {
    f.plan().validate()?.declaration_skeleton()
}

#[test]
fn canonical_declarations_have_bound_addresses_and_explicitly_omitted_bodies() {
    let f = Fixture::new();
    let before = f.snapshot();
    let output = emit(&f).unwrap();
    let file = TastyFile::parse_and_validate_scala_3_9(output.provisional_bytes()).unwrap();
    assert!(file.header().tooling_version.contains("incomplete"));
    assert_eq!(output.intended_tasty_path(), "example/Answer.tasty");
    assert_eq!(output.source().id, f.source);
    assert_eq!(file.source_file().unwrap(), Some("Answer.scala"));
    assert_eq!(
        output.omitted_bodies(),
        &[OmittedMethodBody {
            method: f.methods[0].symbol,
            typed_root: f.body
        }]
    );
    let index = file.ast_address_index().unwrap();
    for (binding, (symbol, owner, tag)) in output.declarations().iter().zip([
        (f.class, f.package, TYPEDEF_TAG),
        (f.constructor.symbol, f.class, DEFDEF_TAG),
        (f.methods[0].symbol, f.class, DEFDEF_TAG),
    ]) {
        assert_eq!((binding.symbol, binding.owner), (symbol, owner));
        assert_eq!(index.get_node(binding.address).unwrap().tag, tag);
        let name = index
            .get(binding.address)
            .unwrap()
            .decode_definition()
            .unwrap()
            .name();
        assert_eq!(
            file.render_name(name).unwrap(),
            f.store
                .names
                .resolve(f.store.symbols.get(symbol).name.text())
        );
    }
    assert_eq!(output.declarations().len(), 3);
    assert_eq!(file.ast_nodes_with_tag(INTCONST_TAG).unwrap(), []);
    assert_eq!(file.ast_nodes_with_tag(ELIDED_TAG).unwrap(), []);
    assert_eq!(file.ast_nodes_with_tag(UNITCONST_TAG).unwrap(), []);
    assert_eq!(file.ast_nodes_with_tag(METHODTYPE_TAG).unwrap(), []);
    let ctor = index
        .get(output.declarations()[1].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert_eq!(ctor.clauses, [EMPTYCLAUSE_TAG]);
    assert!(ctor.parameters.is_empty());
    assert!(ctor.rhs.is_none());
    assert_eq!(ctor.tail, [DefinitionTail::Modifier(STABLE_TAG)]);
    let method = index
        .get(output.declarations()[2].address)
        .unwrap()
        .decode_defdef_body()
        .unwrap();
    assert!(method.clauses.is_empty());
    assert!(method.parameters.is_empty());
    assert!(method.rhs.is_none());
    assert!(method.tail.is_empty());
    assert!(matches!(
        method.return_type,
        RawTree::NatAst {
            tag: IDENTTPT_TAG,
            ..
        }
    ));
    assert_eq!(f.snapshot(), before);
}

#[derive(Debug, PartialEq, Eq)]
struct Shape {
    tag: u8,
    names: Vec<String>,
    children: Vec<Shape>,
}

fn shape(file: &TastyFile<'_>, tree: RawTree<'_>, depth: usize) -> Shape {
    assert!(depth < 64);
    let mut names = Vec::new();
    let mut children = Vec::new();
    let tag = match tree {
        RawTree::Leaf(leaf) => match leaf.tag {
            SHAREDTYPE_TAG => {
                let address = leaf.ast_ref().unwrap().address as usize;
                let payload = file.section(StandardSection::Asts).unwrap().payload;
                let target = RawTree::decode_with_base_offset(
                    &mut Reader::new(&payload[address..]),
                    address,
                )
                .unwrap();
                return shape(file, target, depth + 1);
            }
            TERMREFPKG_TAG => {
                names.push(file.render_name(leaf.name_ref().unwrap()).unwrap());
                leaf.tag
            }
            EMPTYCLAUSE_TAG | STABLE_TAG => leaf.tag,
            tag => panic!("unexpected leaf {tag}"),
        },
        RawTree::Ast {
            tag: NEW_TAG,
            child,
            ..
        } => {
            children.push(shape(file, *child, depth + 1));
            NEW_TAG
        }
        RawTree::NatAst {
            tag, value, child, ..
        } => {
            assert!(matches!(tag, TYPEREF_TAG | IDENTTPT_TAG));
            names.push(file.render_name(value).unwrap());
            children.push(shape(file, *child, depth + 1));
            tag
        }
        RawTree::LengthNode(raw) => {
            match raw.tag {
                PACKAGE_TAG => {
                    let package = raw.decode_package().unwrap();
                    children.push(shape(file, package.path, depth + 1));
                    for stat in package.stats.iter() {
                        children.push(shape(file, RawTree::LengthNode(stat.clone()), depth + 1));
                    }
                }
                TYPEDEF_TAG => {
                    let DefinitionBody::TypeDef {
                        name,
                        type_or_template,
                        tail,
                    } = raw.decode_definition_body().unwrap()
                    else {
                        panic!()
                    };
                    names.push(file.render_name(name).unwrap());
                    children.push(shape(file, type_or_template, depth + 1));
                    for tail in tail {
                        match tail {
                            DefinitionTail::Modifier(tag) => children.push(Shape {
                                tag,
                                names: vec![],
                                children: vec![],
                            }),
                            DefinitionTail::Annotation(raw) => {
                                // The reduced #1190 oracle permits omitting only
                                // the compiler's injected SourceFile annotation.
                                let annotation = raw.decode_annotation().unwrap();
                                assert_eq!(
                                    shape(file, annotation.tycon, depth + 1),
                                    Shape {
                                        tag: TYPEREF_TAG,
                                        names: vec!["SourceFile".into()],
                                        children: vec![Shape {
                                            tag: TERMREFPKG_TAG,
                                            names: vec!["scala.annotation.internal".into()],
                                            children: vec![],
                                        }],
                                    }
                                );
                            }
                            other => panic!("unexpected class tail {other:?}"),
                        }
                    }
                }
                TEMPLATE_TAG => {
                    let template = raw.decode_template_structure().unwrap();
                    assert!(template.type_params.is_empty());
                    assert!(template.term_params.is_empty());
                    assert!(template.self_def.is_none());
                    assert!(!template.split_clause);
                    for child in template.parents.into_iter().chain(template.stats) {
                        children.push(shape(file, child, depth + 1));
                    }
                }
                DEFDEF_TAG => {
                    let definition = raw.decode_defdef_body().unwrap();
                    let name = file.render_name(definition.name).unwrap();
                    assert!(definition.parameters.is_empty());
                    for clause in definition.clauses {
                        children.push(Shape {
                            tag: clause,
                            names: vec![],
                            children: vec![],
                        });
                    }
                    children.push(shape(file, definition.return_type, depth + 1));
                    if name == "<init>" {
                        assert!(definition.rhs.is_none());
                    } else if let Some(rhs) = definition.rhs {
                        // This is the sole intentional declaration/body gap:
                        // preserve signatures, normalize only the real Int RHS.
                        assert!(matches!(
                            rhs,
                            RawTree::Leaf(SimpleTerm {
                                tag: INTCONST_TAG,
                                value: TermValue::Int(_),
                                ..
                            })
                        ));
                    }
                    names.push(name);
                    for tail in definition.tail {
                        let DefinitionTail::Modifier(tag) = tail else {
                            panic!("unexpected method tail")
                        };
                        children.push(Shape {
                            tag,
                            names: vec![],
                            children: vec![],
                        });
                    }
                }
                APPLY_TAG => {
                    let apply = raw.decode_apply().unwrap();
                    children.push(shape(file, apply.function, depth + 1));
                    for arg in apply.arguments {
                        children.push(shape(file, arg, depth + 1));
                    }
                }
                SELECTIN_TAG => {
                    let select = raw.decode_select_in().unwrap();
                    names.push(format!(
                        "{:?}",
                        file.names().render_signed_name(select.name).unwrap()
                    ));
                    children.push(shape(file, select.qualifier, depth + 1));
                    children.push(shape(file, select.underlying_type, depth + 1));
                }
                tag => panic!("unexpected length node {tag}"),
            }
            raw.tag
        }
        tree => panic!("unexpected tree {tree:?}"),
    };
    Shape {
        tag,
        names,
        children,
    }
}

fn declaration_dump(bytes: &[u8]) -> Shape {
    let file = TastyFile::parse_and_validate_scala_3_9(bytes).unwrap();
    let roots = file.asts().unwrap();
    assert_eq!(roots.len(), 1);
    shape(&file, RawTree::LengthNode(roots.get(0).unwrap().clone()), 0)
}

#[test]
fn canonical_declaration_dump_matches_the_independent_scala_oracle() {
    let f = Fixture::new();
    let output = emit(&f).unwrap();
    let oracle = include_bytes!("../../../tests/fixtures/tasty_pickler/tasty/example/Answer.tasty");
    assert_eq!(
        declaration_dump(output.provisional_bytes()),
        declaration_dump(oracle)
    );
}

#[test]
fn body_values_are_not_serialized_or_replaced_by_a_placeholder() {
    let mut f = Fixture::new();
    let first = emit(&f).unwrap();
    f.source_ast.get_mut(f.rhs).kind = TreeKind::Literal(Literal {
        value: Constant::Int(7),
    });
    f.typed_ast.get_mut(f.body).kind = TreeKind::Literal(Literal {
        value: Constant::Int(7),
    });
    let ty = f.typed_ast.get(f.body).ty;
    *f.store.types.get_mut(ty) = Type::Constant(Constant::Int(7));
    let second = emit(&f).unwrap();
    assert_eq!(first, second);
    assert_eq!(second.omitted_bodies()[0].typed_root, f.body);
}

#[test]
fn source_filename_is_metadata_and_output_path_follows_canonical_owners() {
    let f = Fixture::new();
    let mut plan = f.plan();
    plan.source.name = "sources/Original.scala".into();
    let output = plan.validate().unwrap().declaration_skeleton().unwrap();
    assert_eq!(output.intended_tasty_path(), "example/Answer.tasty");
    assert_eq!(output.source().name, "sources/Original.scala");
    let file = TastyFile::parse_and_validate_scala_3_9(output.provisional_bytes()).unwrap();
    assert_eq!(file.source_file().unwrap(), Some("sources/Original.scala"));
}

#[test]
fn semantic_flags_and_visibility_are_rejected_instead_of_silently_dropped() {
    for edit in [
        |f: &mut Fixture| f.store.symbols.get_mut(f.class).flags = SymbolFlags::FINAL,
        |f: &mut Fixture| f.store.symbols.get_mut(f.class).visibility = Visibility::Private,
    ] {
        let mut f = Fixture::new();
        edit(&mut f);
        let before = f.snapshot();
        assert_eq!(
            emit(&f).err(),
            Some(PickleError::Unsupported {
                feature: UnsupportedFeature::SymbolMetadata,
                symbol: Some(f.class)
            })
        );
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn unsupported_source_class_kind_is_rejected_before_skeleton_emission() {
    let mut f = Fixture::new();
    f.store.symbols.get_mut(f.class).kind = SymbolKind::Trait;
    let TreeKind::TypeDef(class) = &mut f.source_ast.get_mut(f.class_tree).kind else {
        panic!()
    };
    class.metadata.modifiers.push(Modifier::Trait);
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::Unsupported {
            feature: UnsupportedFeature::SourceMetadata,
            symbol: Some(f.class)
        })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn incomplete_class_and_template_are_typed_failures_without_mutation() {
    let mut f = Fixture::new();
    f.store.symbols.set_info(f.class, SymbolInfo::Missing);
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::IncompleteSymbol {
            symbol: f.class,
            info: SymbolInfo::Missing
        })
    );
    assert_eq!(f.snapshot(), before);
    let mut f = Fixture::new();
    f.source_ast.get_mut(f.template).kind = TreeKind::TypeTree(dotty_core::ast::TypeTree);
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::SourceShape {
            tree: f.template,
            expected: SourceRole::Template
        })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn foreign_method_owner_and_bad_constructor_signature_are_typed_failures() {
    let mut f = Fixture::new();
    let method = f.methods[0].symbol;
    f.store.symbols.get_mut(method).owner = None;
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::Owner {
            symbol: method,
            expected: Some(f.class),
            actual: None
        })
    );
    assert_eq!(f.snapshot(), before);
    let mut f = Fixture::new();
    f.store.symbols.set_info(
        f.constructor.symbol,
        SymbolInfo::Complete(f.definitions.unit),
    );
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::MalformedType {
            ty: f.definitions.unit,
            role: TypeRole::ConstructorSignature
        })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn duplicate_identity_and_ambiguous_class_name_cannot_emit_a_partial_result() {
    let mut f = Fixture::new();
    f.methods.push(f.methods[0].clone());
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::DuplicateDeclaration {
            symbol: f.methods[0].symbol
        })
    );
    assert_eq!(f.snapshot(), before);
    let mut f = Fixture::new();
    let duplicate = f.store.symbols.alloc(f.store.symbols.get(f.class).clone());
    let name = f.store.symbols.get(f.class).name;
    let scope = f.index.scope_of(f.package).unwrap();
    f.store.scopes.get_mut(scope).enter(name, duplicate);
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::ScopeMembers { owner: f.package })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn projection_limits_propagate_and_leave_the_compilation_session_unchanged() {
    let f = Fixture::new();
    let plan = f.plan();
    let checked = plan.validate().unwrap();
    let before = f.snapshot();
    assert!(
        matches!(checked.declaration_skeleton_with_limits(ProjectionLimits { wire: AstBuildLimits { max_bytes: 1, ..AstBuildLimits::default() }, ..ProjectionLimits::default() }).err(), Some(PickleError::Construction { cause }) if *cause == AstBuildError::Limit("bytes"))
    );
    assert_eq!(
        checked
            .declaration_skeleton_with_limits(ProjectionLimits {
                max_names: 0,
                ..ProjectionLimits::default()
            })
            .err(),
        Some(PickleError::LimitExceeded {
            kind: LimitKind::Names,
            limit: 0
        })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn independent_sessions_produce_identical_declaration_order_and_bytes() {
    assert_eq!(
        emit(&Fixture::new()).unwrap(),
        emit(&Fixture::new()).unwrap()
    );
}

#[test]
fn a_missing_real_body_is_an_error_even_though_skeleton_emission_omits_bodies() {
    let mut f = Fixture::new();
    f.methods[0].body = None;
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::MissingBody {
            method: f.methods[0].symbol
        })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn unicode_class_identity_controls_the_wire_name_and_intended_output_path() {
    let mut f = Fixture::new();
    let name = dotty_core::Name::new(f.store.names.intern("日本語"), dotty_core::Namespace::Type);
    f.store.symbols.get_mut(f.class).name = name;
    let TreeKind::TypeDef(class) = &mut f.source_ast.get_mut(f.class_tree).kind else {
        panic!()
    };
    class.name = dotty_core::TypeName::new(name.text());
    let scope = f.index.scope_of(f.package).unwrap();
    f.store.scopes.get_mut(scope).remove(f.class);
    f.store.scopes.get_mut(scope).enter(name, f.class);
    let output = emit(&f).unwrap();
    assert_eq!(output.intended_tasty_path(), "example/日本語.tasty");
    let file = TastyFile::parse_and_validate_scala_3_9(output.provisional_bytes()).unwrap();
    let index = file.ast_address_index().unwrap();
    let name = index
        .get(output.declarations()[0].address)
        .unwrap()
        .decode_definition()
        .unwrap()
        .name();
    assert_eq!(file.render_name(name).unwrap(), "日本語");
}

#[test]
fn a_missing_member_symbol_cannot_produce_a_skeleton() {
    let mut f = Fixture::new();
    let mark = f.store.checkpoint();
    let stale = f
        .store
        .symbols
        .alloc(f.store.symbols.get(f.methods[0].symbol).clone());
    f.store.rollback_to(mark);
    f.methods[0].symbol = stale;
    let before = f.snapshot();
    assert_eq!(
        emit(&f).err(),
        Some(PickleError::InvalidSymbol { symbol: stale })
    );
    assert_eq!(f.snapshot(), before);
}
