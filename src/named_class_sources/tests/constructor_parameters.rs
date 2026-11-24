use super::*;
use crate::core::{SourceDefinition, SymbolKind};
use crate::{CheckedClassBodies, ResolvedMemberIndex};

#[test]
fn constructor_parameters_pair_the_exact_source_and_formal_across_classes() {
    let text = "class A(z: Int, a: Int); class B(z: Int, a: Int)";
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let file = named(7, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let index = ResolvedMemberIndex::build(
        &sources,
        &CheckedClassBodies::default(),
        &AstArena::new(),
        &store,
    );
    assert!(index.diagnostics().is_empty(), "{:?}", index.diagnostics());
    let mut ids = std::collections::HashSet::new();
    for (class, start) in file
        .classes()
        .iter()
        .zip(text.match_indices('(').map(|(i, _)| i + 1))
    {
        for offset in [start, start + 8] {
            let p = index
                .constructor_parameter_at(file.source(), offset as u32)
                .unwrap();
            assert_eq!((p.class, p.constructor), (class.symbol, class.constructor));
            assert_eq!(p.target.name.start(), offset as u32);
            assert_eq!(p.target.name.end(), offset as u32 + 1);
            let Some(SourceDefinition::Canonical { source, tree }) =
                file.index().definition_of(p.parameter)
            else {
                panic!("source formal")
            };
            assert_eq!(
                file.index()
                    .derived_symbol_at(class.constructor, source, tree),
                Some(p.formal)
            );
            for (symbol, owner) in [(p.parameter, p.class), (p.formal, p.constructor)] {
                assert!(ids.insert(symbol));
                assert_eq!(
                    (
                        store.symbols.get(symbol).kind,
                        store.symbols.get(symbol).owner
                    ),
                    (SymbolKind::Parameter, Some(owner))
                );
            }
            assert!(index.parameter_at(source, offset as u32).is_none());
        }
    }
}

#[test]
fn unsupported_constructor_forms_do_not_publish_plain_parameters() {
    let text = "class A(val field: Int, var state: Int, defaulted: Int = 1); class B(first: Int)(second: Int); class C(using context: Int)";
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let file = named(1, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let index = ResolvedMemberIndex::build(
        &sources,
        &CheckedClassBodies::default(),
        &AstArena::new(),
        &store,
    );
    assert!(index.diagnostics().is_empty());
    for name in ["field", "state", "defaulted", "first", "second", "context"] {
        let offset = text.find(name).unwrap() as u32;
        assert!(
            index
                .constructor_parameter_at(file.source(), offset)
                .is_none()
        );
        assert_eq!(
            index.member_at(file.source(), offset).is_some(),
            matches!(name, "field" | "state")
        );
    }
}

#[test]
fn initializer_and_method_reads_share_the_source_parameter_and_respect_shadowing() {
    let text = "class A(start: Int, unused: Int) { private val initial: Int = start; def read(): Int = start; def shadow(start: Int): Int = start; def local(): Int = { val start = 2; start } }";
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(9, text, &mut store, &mut packages);
    let source = file.source();
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let (body, report) = sources
        .check_class_body(
            file.classes()[0].symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    assert!(report.errors.is_empty(), "{report:?}");
    let mut bodies = CheckedClassBodies::default();
    bodies.record(body).unwrap();
    let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    let tokens: Vec<_> = text.match_indices("start").map(|(i, _)| i as u32).collect();
    let [decl, initializer, read, method, param_use, local, local_use] = tokens[..] else {
        panic!("fixture names")
    };
    let p = index.constructor_parameter_at(source, decl).unwrap();
    for pos in [initializer, read] {
        assert_eq!(index.constructor_parameter_at(source, pos), Some(p));
        let site = index.symbol_at(source, pos).unwrap();
        assert_eq!(
            (site.symbol, site.kind, site.target),
            (p.parameter, SymbolKind::Parameter, p.target)
        );
        assert_eq!(
            index.value_access_at(source, pos),
            Some(crate::ValueAccessKind::Read)
        );
    }
    let sites = |id| {
        index
            .references_to(id)
            .iter()
            .map(|span| span.span().range().start())
            .collect::<Vec<_>>()
    };
    assert_eq!(sites(p.parameter), [initializer, read]);
    assert!(sites(p.formal).is_empty());
    for (decl, usage, kind) in [
        (method, param_use, SymbolKind::Parameter),
        (local, local_use, SymbolKind::Local),
    ] {
        let other = index.symbol_at(source, decl).unwrap();
        assert_ne!(other.symbol, p.parameter);
        assert_eq!(other.kind, kind);
        assert_eq!(sites(other.symbol), [usage]);
        assert!(index.constructor_parameter_at(source, usage).is_none());
    }
    let unused = index
        .constructor_parameter_at(source, text.find("unused").unwrap() as u32)
        .unwrap();
    assert!(sites(unused.parameter).is_empty());
}
