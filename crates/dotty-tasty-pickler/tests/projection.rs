#[allow(dead_code)]
mod support;

use dotty_core::ast::{TreeKind, TypeTree};
use dotty_core::*;
use dotty_tasty::tasty::*;
use dotty_tasty_pickler::*;
use support::Fixture;

fn context(f: &Fixture) -> EncodingContext<'_> {
    EncodingContext::new(
        &f.store,
        &f.definitions,
        &f.index,
        f.source,
        ProjectionLimits::default(),
    )
    .unwrap()
}

fn at(fragment: &ProjectedFragment, label: AstLabel) -> RawTree<'_> {
    let offset = fragment.ast.address(label).unwrap() as usize;
    RawTree::decode_with_base_offset(&mut Reader::new(&fragment.ast.bytes()[offset..]), offset)
        .unwrap()
}

fn external_meaning(tree: RawTree<'_>, names: &NameTable) -> (String, String) {
    let RawTree::NatAst {
        tag: TYPEREF_TAG,
        value,
        child,
        ..
    } = tree
    else {
        panic!("expected TYPEREF")
    };
    let RawTree::Leaf(SimpleTerm {
        tag: TERMREFPKG_TAG,
        value: TermValue::NameRef(package),
        ..
    }) = *child
    else {
        panic!("expected TERMREFpkg")
    };
    (names.render(package).unwrap(), names.render(value).unwrap())
}

fn bind_test_class(context: &mut EncodingContext<'_>, symbol: SymbolId, name: Name) {
    let name = context.intern_name(name).unwrap();
    let body = context.constructor_return_type().unwrap();
    context
        .bind_declaration(
            symbol,
            AstNode::Length {
                tag: TYPEDEF_TAG,
                fields: vec![AstField::Nat(name), AstField::Tree(body)],
            },
        )
        .unwrap();
}

#[test]
fn builtin_reference_meanings_match_the_frozen_scala_oracle() {
    let f = Fixture::new();
    let before = f.snapshot();
    let mut context = context(&f);
    let int = context.lower_type(f.definitions.int).unwrap();
    let unit = context.constructor_return_type().unwrap();
    let object = context.lower_type(f.definitions.object_type).unwrap();
    let projected = context.finish(&[int, unit, object]).unwrap();
    let meanings: Vec<_> = [int, unit, object]
        .into_iter()
        .map(|l| external_meaning(at(&projected, l), &projected.names))
        .collect();
    assert_eq!(
        meanings,
        [
            ("scala".into(), "Int".into()),
            ("scala".into(), "Unit".into()),
            ("java.lang".into(), "Object".into())
        ]
    );

    let oracle = TastyFile::parse_scala_3_9(include_bytes!(
        "../../../tests/fixtures/tasty_pickler/tasty/example/Answer.tasty"
    ))
    .unwrap();
    let payload = oracle.section(StandardSection::Asts).unwrap().payload;
    let oracle_meanings: Vec<_> = oracle
        .ast_nodes_with_tag(TYPEREF_TAG)
        .unwrap()
        .into_iter()
        .map(|node| {
            let tree = RawTree::decode(&mut Reader::new(&payload[node.offset..])).unwrap();
            external_meaning(tree, oracle.names())
        })
        .collect();
    for meaning in meanings {
        assert!(oracle_meanings.contains(&meaning), "{meaning:?}");
    }
    assert_eq!(f.snapshot(), before);
}

#[test]
fn object_constructor_signed_name_matches_the_oracle_without_a_fake_symbol() {
    let f = Fixture::new();
    let before = f.snapshot();
    let mut c = context(&f);
    let name = c.object_constructor_name().unwrap();
    assert_eq!(c.object_constructor_name().unwrap(), name);
    let projected = c.finish(&[]).unwrap();
    let actual = projected.names.render_signed_name(name).unwrap();
    let oracle = TastyFile::parse_scala_3_9(include_bytes!(
        "../../../tests/fixtures/tasty_pickler/tasty/example/Answer.tasty"
    ))
    .unwrap();
    assert!(
        oracle
            .names()
            .iter()
            .filter(|(_, raw)| matches!(raw, RawName::Signed { .. }))
            .any(|(name, _)| oracle.names().render_signed_name(name).unwrap() == actual)
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn qualified_unicode_packages_and_namespace_tags_are_preserved() {
    let mut f = Fixture::new();
    let mut packages = Packages::new();
    let entries = packages.enter(
        &mut f.store,
        SymbolOrigin::Source(f.source),
        &["δοκιμή", "日本語"],
    );
    let package = entries[1].symbol;
    let term_name = Name::new(f.store.names.intern("café"), Namespace::Term);
    let type_name = Name::new(term_name.text(), Namespace::Type);
    let term_ref = f
        .store
        .types
        .alloc(Type::term_ref(f.definitions.no_prefix, package));
    let type_ref = f
        .store
        .types
        .alloc(Type::type_ref(f.definitions.no_prefix, package));
    let mut context = context(&f);
    assert_eq!(
        context.intern_name(term_name).unwrap(),
        context.intern_name(type_name).unwrap()
    );
    let term = context.lower_type(term_ref).unwrap();
    let ty = context.lower_type(type_ref).unwrap();
    let result = context.finish(&[term, ty]).unwrap();
    for (label, tag) in [(term, TERMREFPKG_TAG), (ty, TYPEREFPKG_TAG)] {
        let RawTree::Leaf(leaf) = at(&result, label) else {
            panic!()
        };
        assert_eq!(leaf.tag, tag);
        assert_eq!(
            result.names.render(leaf.name_ref().unwrap()).unwrap(),
            "δοκιμή.日本語"
        );
    }
    assert!(
        result
            .names
            .iter()
            .any(|(_, n)| matches!(n, RawName::Qualified { .. }))
    );
}

#[test]
fn exact_type_ids_share_and_equal_but_distinct_types_do_not() {
    let mut f = Fixture::new();
    let distinct = f
        .store
        .types
        .alloc(f.store.types.get(f.definitions.int).clone());
    let mut context = context(&f);
    let first = context.lower_type(f.definitions.int).unwrap();
    let shared = context.lower_type(f.definitions.int).unwrap();
    let different = context.lower_type(distinct).unwrap();
    let result = context.finish(&[first, shared, different]).unwrap();
    assert_eq!(
        at(&result, shared).ast_ref(),
        Some(AstRef {
            kind: AstRefKind::SharedType,
            address: result.ast.address(first).unwrap()
        })
    );
    assert!(matches!(
        at(&result, different),
        RawTree::NatAst {
            tag: TYPEREF_TAG,
            ..
        }
    ));
    assert_eq!(
        external_meaning(at(&result, different), &result.names),
        ("scala".into(), "Int".into())
    );
    assert!(matches!(
        context.finish(&[shared, first, different]),
        Err(PickleError::Construction { .. })
    ));
}

#[test]
fn explicit_result_syntax_and_constructor_unit_are_distinct_from_semantic_types() {
    let f = Fixture::new();
    let mut context = context(&f);
    let int = context.int_type_syntax().unwrap();
    let unit = context.constructor_return_syntax().unwrap();
    let result = context.finish(&[int, unit]).unwrap();
    for (label, expected) in [(int, "Int"), (unit, "Unit")] {
        let RawTree::NatAst {
            tag: IDENTTPT_TAG,
            value,
            child,
            ..
        } = at(&result, label)
        else {
            panic!()
        };
        assert_eq!(result.names.get_utf8(value), Some(expected));
        assert_eq!(
            external_meaning(*child, &result.names),
            ("scala".into(), expected.into())
        );
    }
    let mut ordinary = self::context(&f);
    assert_eq!(
        ordinary.lower_type(f.definitions.unit).err(),
        Some(PickleError::UnsupportedTypeReference {
            ty: f.definitions.unit
        })
    );
    assert_eq!(
        ordinary.finish(&[]).err(),
        Some(PickleError::ProjectionAborted)
    );
}

#[test]
fn an_ordinary_method_clause_preserves_parameter_order_names_and_type_sharing() {
    let mut f = Fixture::new();
    let x = TermName::new(f.store.names.intern("x"));
    let y = TermName::new(f.store.names.intern("日本語"));
    let method = f.store.types.alloc(Type::Method(MethodType {
        params: [x, y]
            .map(|name| MethodParam {
                name,
                ty: f.definitions.int,
                erased: false,
                varargs: false,
            })
            .to_vec(),
        result: f.definitions.int,
        kind: MethodKind::Plain,
    }));
    let mut context = context(&f);
    let method_label = context.lower_type(method).unwrap();
    let result = context.finish(&[method_label]).unwrap();
    let RawTree::LengthNode(raw) = at(&result, method_label) else {
        panic!()
    };
    let decoded = raw.decode_method_type().unwrap();
    assert_eq!(decoded.modifiers, []);
    assert_eq!(
        decoded
            .type_names
            .iter()
            .map(|n| result.names.render(n.name).unwrap())
            .collect::<Vec<_>>(),
        ["x", "日本語"]
    );
    let result_address = result.ast.address(method_label).unwrap() as usize + 2;
    for param in decoded.type_names {
        assert_eq!(
            param.type_or_bounds.ast_ref(),
            Some(AstRef {
                kind: AstRefKind::SharedType,
                address: result_address as u32
            })
        );
    }
}

#[test]
fn local_forward_reference_binds_by_symbol_and_validates_the_definition_tag() {
    let mut f = Fixture::new();
    let ty = f
        .store
        .types
        .alloc(Type::type_ref(f.definitions.no_prefix, f.class));
    let mut context = context(&f);
    let declaration = context.reserve_declaration(f.class).unwrap();
    let reference = context.lower_type(ty).unwrap();
    assert!(
        matches!(context.finish(&[reference]).err(), Some(PickleError::Construction { cause }) if matches!(*cause, AstBuildError::Unbound(_)))
    );
    bind_test_class(&mut context, f.class, f.store.symbols.get(f.class).name);
    let result = context.finish(&[reference, declaration]).unwrap();
    assert_eq!(
        at(&result, reference).ast_ref(),
        Some(AstRef {
            kind: AstRefKind::TypeRefDirect,
            address: result.ast.address(declaration).unwrap()
        })
    );
    assert!(
        matches!(context.bind_declaration(f.class, AstNode::Length { tag: TYPEDEF_TAG, fields: vec![] }), Err(PickleError::Construction { cause }) if matches!(*cause, AstBuildError::AlreadyBound(_)))
    );

    let mut wrong = self::context(&f);
    wrong.reserve_declaration(f.class).unwrap();
    assert_eq!(
        wrong.bind_declaration(
            f.class,
            AstNode::Length {
                tag: DEFDEF_TAG,
                fields: vec![]
            }
        ),
        Err(PickleError::Reference {
            symbol: f.class,
            reason: ReferenceError::DeclarationTag
        })
    );
}

#[test]
fn equal_names_under_different_package_owners_keep_distinct_local_targets() {
    let mut f = Fixture::new();
    let mut packages = Packages::new();
    let package = packages.enter(&mut f.store, SymbolOrigin::Source(f.source), &["other"])[0];
    let class_name = f.store.symbols.get(f.class).name;
    let second = support::source_symbol(
        &mut f.store,
        class_name,
        SymbolKind::Class,
        package.symbol,
        f.source,
        SymbolInfo::Missing,
    );
    let scope = f.store.scopes.alloc(Scope::new(Some(second)));
    let info = f.store.types.alloc(Type::ClassInfo(ClassInfo {
        class: second,
        prefix: f.definitions.no_prefix,
        parents: vec![f.definitions.object_type],
        declarations: scope,
        self_type: None,
    }));
    f.store.symbols.set_info(second, SymbolInfo::Complete(info));
    let name = f.store.symbols.get(second).name;
    f.store.scopes.get_mut(package.scope).enter(name, second);
    f.index.record_scope(package.symbol, package.scope).unwrap();
    f.index.record_scope(second, scope).unwrap();
    let tree = support::node(&mut f.source_ast, TreeKind::TypeTree(TypeTree));
    f.index.record_symbol(f.source, tree, second).unwrap();
    let mut context = context(&f);
    let a = context.reserve_declaration(f.class).unwrap();
    let b = context.reserve_declaration(second).unwrap();
    let ar = context.local_reference(f.class, None).unwrap();
    let br = context.local_reference(second, None).unwrap();
    bind_test_class(&mut context, f.class, name);
    bind_test_class(&mut context, second, name);
    let result = context.finish(&[ar, br, a, b]).unwrap();
    assert_ne!(result.ast.address(a), result.ast.address(b));
    assert_eq!(
        at(&result, ar).ast_ref().unwrap().address,
        result.ast.address(a).unwrap()
    );
    assert_eq!(
        at(&result, br).ast_ref().unwrap().address,
        result.ast.address(b).unwrap()
    );
    assert_eq!(
        result
            .names
            .iter()
            .filter(|(_, n)| n.as_utf8() == Some("Answer"))
            .count(),
        1
    );
}

fn dependency(f: &mut Fixture) -> SymbolId {
    let name = Name::new(f.store.names.intern("Box"), Namespace::Type);
    let origin = SymbolOrigin::Tasty(f.store.origins.register_tasty());
    let mut symbol = f.store.symbols.get(f.class).clone();
    symbol.name = name;
    symbol.origin = origin;
    let symbol = f.store.symbols.alloc(symbol);
    let scope = f.store.scopes.alloc(Scope::new(Some(symbol)));
    let info = f.store.types.alloc(Type::ClassInfo(ClassInfo {
        class: symbol,
        prefix: f.definitions.no_prefix,
        parents: vec![f.definitions.object_type],
        declarations: scope,
        self_type: None,
    }));
    f.store.symbols.set_info(symbol, SymbolInfo::Complete(info));
    f.store
        .scopes
        .get_mut(f.index.scope_of(f.package).unwrap())
        .enter(name, symbol);
    symbol
}

#[test]
fn registered_external_classes_require_exact_owner_scope_and_keep_the_semantic_prefix() {
    let mut f = Fixture::new();
    let symbol = dependency(&mut f);
    let prefix = f
        .store
        .types
        .alloc(Type::term_ref(f.definitions.no_prefix, f.package));
    let ty = f.store.types.alloc(Type::type_ref(prefix, symbol));
    let mut context = context(&f);
    context
        .register_external(symbol, f.index.scope_of(f.package).unwrap())
        .unwrap();
    let first = context.lower_type(ty).unwrap();
    let explicit = context.external_reference(symbol).unwrap();
    let result = context.finish(&[first, explicit]).unwrap();
    for label in [first, explicit] {
        assert_eq!(
            external_meaning(at(&result, label), &result.names),
            ("example".into(), "Box".into())
        );
    }
}

#[test]
fn external_ambiguity_wrong_scope_and_unregistered_identity_are_typed_errors() {
    let mut f = Fixture::new();
    let symbol = dependency(&mut f);
    let mut c = context(&f);
    assert_eq!(
        c.external_reference(symbol).err(),
        Some(PickleError::Reference {
            symbol,
            reason: ReferenceError::NotRegistered
        })
    );
    let mut c = context(&f);
    assert_eq!(
        c.register_external(symbol, f.scope),
        Err(PickleError::Reference {
            symbol,
            reason: ReferenceError::Owner
        })
    );
    let name = f.store.symbols.get(symbol).name;
    let scope = f.index.scope_of(f.package).unwrap();
    f.store.scopes.get_mut(scope).enter(name, f.class);
    let mut c = context(&f);
    assert_eq!(
        c.register_external(symbol, scope),
        Err(PickleError::Reference {
            symbol,
            reason: ReferenceError::Ambiguous
        })
    );
}

#[test]
fn type_cycles_unsupported_forms_and_missing_types_are_explicit_and_poison_the_context() {
    let mut f = Fixture::new();
    let cyclic = f.store.types.alloc(Type::NoPrefix);
    let name = TermName::new(f.store.names.intern("x"));
    *f.store.types.get_mut(cyclic) = Type::Method(MethodType {
        params: vec![MethodParam {
            name,
            ty: cyclic,
            erased: false,
            varargs: false,
        }],
        result: f.definitions.int,
        kind: MethodKind::Plain,
    });
    let unsupported = f.store.types.alloc(Type::Applied {
        tycon: f.definitions.int,
        args: vec![],
    });
    let no_type = f.store.types.alloc(Type::NoType);
    for (ty, expected) in [
        (cyclic, PickleError::CyclicType { ty: cyclic }),
        (
            unsupported,
            PickleError::UnsupportedTypeReference { ty: unsupported },
        ),
        (no_type, PickleError::IncompleteType { ty: no_type }),
    ] {
        let before = f.snapshot();
        let mut c = context(&f);
        assert_eq!(c.lower_type(ty).err(), Some(expected));
        assert_eq!(
            c.lower_type(f.definitions.int).err(),
            Some(PickleError::ProjectionAborted)
        );
        assert_eq!(c.finish(&[]).err(), Some(PickleError::ProjectionAborted));
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn projection_depth_fuel_and_name_budgets_have_typed_failures() {
    let mut f = Fixture::new();
    let method = f.store.types.alloc(Type::Method(MethodType {
        params: vec![],
        result: f.definitions.int,
        kind: MethodKind::Plain,
    }));
    for (limits, kind, limit) in [
        (
            ProjectionLimits {
                max_type_depth: 0,
                ..ProjectionLimits::default()
            },
            LimitKind::TypeDepth,
            0,
        ),
        (
            ProjectionLimits {
                max_items: 3,
                ..ProjectionLimits::default()
            },
            LimitKind::Items,
            3,
        ),
        (
            ProjectionLimits {
                max_names: 0,
                ..ProjectionLimits::default()
            },
            LimitKind::Names,
            0,
        ),
    ] {
        let mut c =
            EncodingContext::new(&f.store, &f.definitions, &f.index, f.source, limits).unwrap();
        assert_eq!(
            c.lower_type(method).err(),
            Some(PickleError::LimitExceeded { kind, limit })
        );
    }
    let name = Name::new(f.store.names.intern(&"x".repeat(4097)), Namespace::Term);
    let mut c = context(&f);
    assert_eq!(
        c.intern_name(name).err(),
        Some(PickleError::LimitExceeded {
            kind: LimitKind::NameBytes,
            limit: 4096
        })
    );
}

#[test]
fn stale_ids_and_incomplete_declarations_are_rejected_without_panics() {
    let mut f = Fixture::new();
    let mark = f.store.checkpoint();
    let stale = f.store.types.alloc(Type::NoPrefix);
    let stale_symbol = f.store.symbols.alloc(f.store.symbols.get(f.class).clone());
    f.store.rollback_to(mark);
    let mut c = context(&f);
    assert_eq!(
        c.lower_type(stale).err(),
        Some(PickleError::InvalidType { ty: stale })
    );
    let mut c = context(&f);
    assert_eq!(
        c.reserve_declaration(stale_symbol).err(),
        Some(PickleError::InvalidSymbol {
            symbol: stale_symbol
        })
    );
    f.store.symbols.set_info(f.class, SymbolInfo::Missing);
    let mut c = context(&f);
    assert_eq!(
        c.reserve_declaration(f.class).err(),
        Some(PickleError::IncompleteSymbol {
            symbol: f.class,
            info: SymbolInfo::Missing
        })
    );
}

#[test]
fn repeated_independent_contexts_produce_identical_names_and_bytes() {
    let f = Fixture::new();
    let project = || {
        let mut c = context(&f);
        let a = c.lower_type(f.definitions.int).unwrap();
        let b = c.int_type_syntax().unwrap();
        let p = c.package_reference(f.package).unwrap();
        c.finish(&[a, b, p]).unwrap()
    };
    assert_eq!(project(), project());
}

#[test]
fn validated_plan_seeds_exact_declaration_reservations_without_mutation() {
    let f = Fixture::new();
    let before = f.snapshot();
    let plan = f.plan();
    let validated = plan.validate().unwrap();
    let mut c = EncodingContext::for_plan(&validated, ProjectionLimits::default()).unwrap();
    let labels: Vec<_> = [f.class, f.constructor.symbol, f.methods[0].symbol]
        .map(|s| c.declaration_label(s).unwrap())
        .into();
    assert_ne!(labels[0], labels[1]);
    assert_ne!(labels[1], labels[2]);
    assert_eq!(
        c.reserve_declaration(f.class).err(),
        Some(PickleError::DuplicateDeclaration { symbol: f.class })
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn local_term_references_keep_direct_and_prefixed_address_roles_separate() {
    let mut f = Fixture::new();
    let this = f.store.types.alloc(Type::ThisType { class: f.class });
    let method = f.methods[0].symbol;
    let mut c = context(&f);
    let class_label = c.reserve_declaration(f.class).unwrap();
    let method_label = c.reserve_declaration(method).unwrap();
    let direct = c.local_reference(method, None).unwrap();
    let prefixed = c.local_reference(method, Some(this)).unwrap();
    bind_test_class(&mut c, f.class, f.store.symbols.get(f.class).name);
    let name = c.intern_name(f.store.symbols.get(method).name).unwrap();
    let result = c.int_type_syntax().unwrap();
    c.bind_declaration(
        method,
        AstNode::Length {
            tag: DEFDEF_TAG,
            fields: vec![AstField::Nat(name), AstField::Tree(result)],
        },
    )
    .unwrap();
    let fragment = c
        .finish(&[direct, prefixed, class_label, method_label])
        .unwrap();
    let address = fragment.ast.address(method_label).unwrap();
    assert_eq!(
        at(&fragment, direct).ast_ref(),
        Some(AstRef {
            kind: AstRefKind::TermRefDirect,
            address
        })
    );
    let RawTree::NatAst {
        tag: TERMREFSYMBOL_TAG,
        value,
        child,
        ..
    } = at(&fragment, prefixed)
    else {
        panic!()
    };
    assert_eq!(value, address);
    assert!(matches!(*child, RawTree::Ast { tag: THIS_TAG, .. }));
}

#[test]
fn namespace_owner_and_class_scope_mismatches_are_typed_failures() {
    let mut f = Fixture::new();
    let method = f.methods[0].symbol;
    let wrong_namespace = f
        .store
        .types
        .alloc(Type::type_ref(f.definitions.no_prefix, method));
    assert_eq!(
        context(&f).lower_type(wrong_namespace).err(),
        Some(PickleError::Reference {
            symbol: method,
            reason: ReferenceError::Namespace
        })
    );
    let wrong_prefix = f.store.types.alloc(Type::ThisType {
        class: f.definitions.object_class,
    });
    let mut c = context(&f);
    c.reserve_declaration(method).unwrap();
    assert_eq!(
        c.local_reference(method, Some(wrong_prefix)).err(),
        Some(PickleError::Reference {
            symbol: method,
            reason: ReferenceError::Owner
        })
    );
    f.store.scopes.get_mut(f.scope).owner = Some(f.package);
    assert_eq!(
        context(&f).reserve_declaration(f.class).err(),
        Some(PickleError::Reference {
            symbol: f.class,
            reason: ReferenceError::Owner
        })
    );
}

#[test]
fn package_owner_cycles_and_nonpackage_owners_are_rejected() {
    let mut f = Fixture::new();
    f.store.symbols.get_mut(f.package).owner = Some(f.package);
    assert_eq!(
        context(&f).package_reference(f.package).err(),
        Some(PickleError::OwnerCycle { symbol: f.package })
    );
    let mut f = Fixture::new();
    f.store.symbols.get_mut(f.package).owner = Some(f.class);
    assert_eq!(
        context(&f).package_reference(f.package).err(),
        Some(PickleError::Reference {
            symbol: f.class,
            reason: ReferenceError::Owner
        })
    );
}

#[test]
fn unsupported_method_kinds_and_parameter_metadata_are_not_silently_dropped() {
    let mut f = Fixture::new();
    let name = TermName::new(f.store.names.intern("x"));
    let nested = f.store.types.alloc(Type::Method(MethodType {
        params: vec![],
        result: f.definitions.int,
        kind: MethodKind::Plain,
    }));
    for (kind, erased, varargs, result) in [
        (MethodKind::Implicit, false, false, f.definitions.int),
        (MethodKind::Contextual, false, false, f.definitions.int),
        (MethodKind::Plain, true, false, f.definitions.int),
        (MethodKind::Plain, false, true, f.definitions.int),
        (MethodKind::Plain, false, false, nested),
    ] {
        let ty = f.store.types.alloc(Type::Method(MethodType {
            params: vec![MethodParam {
                name,
                ty: f.definitions.int,
                erased,
                varargs,
            }],
            result,
            kind,
        }));
        assert_eq!(
            context(&f).lower_type(ty).err(),
            Some(PickleError::UnsupportedTypeReference { ty })
        );
    }
}

#[test]
fn malformed_external_prefixes_cannot_be_replaced_with_a_plausible_package() {
    let mut f = Fixture::new();
    let symbol = dependency(&mut f);
    let malformed = f
        .store
        .types
        .alloc(Type::term_ref(f.definitions.int, f.package));
    let ty = f.store.types.alloc(Type::type_ref(malformed, symbol));
    let mut c = context(&f);
    c.register_external(symbol, f.index.scope_of(f.package).unwrap())
        .unwrap();
    assert_eq!(
        c.lower_type(ty).err(),
        Some(PickleError::Reference {
            symbol: f.package,
            reason: ReferenceError::Owner
        })
    );
}
