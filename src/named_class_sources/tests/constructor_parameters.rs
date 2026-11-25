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

#[test]
fn only_committed_parent_reads_enter_the_index_once_after_a_blocked_retry() {
    use crate::typer::{SignatureCompletion, TyperError};
    for (ty, valid) in [("Int", true), ("Boolean", false)] {
        let text = format!("class Leaf(start: {ty}) extends Base(start)");
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let leaf = named(7, &text, &mut store, &mut packages);
        let base = named(3, "class Base(start: Int)", &mut store, &mut packages);
        let sources = NamedClassSources::new([&leaf, &base], &store).unwrap();
        prepare_constructors(&sources, &mut store, definitions, &packages);
        let mut arena = AstArena::new();
        let mut headers = ClassHeaderBindings::default();
        let bodies = CheckedClassBodies::default();
        let child = leaf.classes()[0].symbol;
        let parent = base.classes()[0].symbol;
        let read = text.rfind("start").unwrap() as u32;
        let decl = text.find("start").unwrap() as u32;
        assert!(
            matches!(sources.try_complete_class_header(child,&mut store,definitions,&packages,&mut arena,&mut headers), Ok(SignatureCompletion::BlockedOn(id)) if id == parent)
        );
        let check = |headers: &ClassHeaderBindings,
                     arena: &AstArena<Typed>,
                     store: &SemanticStore,
                     published: bool| {
            let index =
                ResolvedMemberIndex::build_with_headers(&sources, &bodies, headers, arena, store);
            let p = index.constructor_parameter_at(leaf.source(), decl).unwrap();
            assert_eq!(
                index.constructor_parameter_at(leaf.source(), read),
                published.then_some(p)
            );
            assert_eq!(
                index.references_to(p.parameter).len(),
                usize::from(published)
            );
            let base_p = index
                .constructor_parameter_at(base.source(), "class Base(".len() as u32)
                .unwrap();
            assert_ne!(base_p.parameter, p.parameter);
            assert!(index.references_to(base_p.parameter).is_empty());
            if published {
                let site = index.symbol_at(leaf.source(), read).unwrap();
                assert_eq!(index.references_to(p.parameter), &[site.usage.unwrap()]);
                assert_eq!(site.target, p.target);
            }
        };
        check(&headers, &arena, &store, false);
        assert!(matches!(
            sources.try_complete_class_header(
                parent,
                &mut store,
                definitions,
                &packages,
                &mut arena,
                &mut headers
            ),
            Ok(SignatureCompletion::Complete(_))
        ));
        for _ in 0..2 {
            let result = sources.try_complete_class_header(
                child,
                &mut store,
                definitions,
                &packages,
                &mut arena,
                &mut headers,
            );
            if valid {
                assert!(matches!(result, Ok(SignatureCompletion::Complete(_))));
            } else {
                assert!(matches!(
                    result,
                    Err(TyperError::ApplicationArgumentTypeMismatch { .. })
                ));
            }
            check(&headers, &arena, &store, valid);
        }
        assert!(
            ResolvedMemberIndex::build(&sources, &bodies, &arena, &store)
                .symbol_at(leaf.source(), read)
                .is_none()
        );
    }
}

#[test]
fn constructor_field_references_merge_labels_and_accesses_in_written_order() {
    let text = "class Use {
      def create(): Counter = { val count = 4; new Counter(count = count) }
      def read(c: Counter): Int = c.count
      def update(c: Counter): Unit = c.count = 7
      def again(c: Counter): Counter = new Counter(count = c.count)
    }";
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let provider = named(
        3,
        "class Counter(var count: Int)",
        &mut store,
        &mut packages,
    );
    let caller = named(1, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&caller, &provider], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    assert!(matches!(
        sources.try_complete_member_signature(
            provider.classes()[0].constructor_fields[0],
            &mut store,
            defs,
            &packages
        ),
        Ok(SignatureCompletion::Complete(_))
    ));
    let (body, report) = sources
        .check_class_body(
            caller.classes()[0].symbol,
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
    let source = caller.source();
    let positions: Vec<_> = text.match_indices("count").map(|(i, _)| i as u32).collect();
    let [local, label, rhs, read, write, last_label, last_read] = positions[..] else {
        panic!("unexpected source")
    };
    let site = index.symbol_at(source, label).unwrap();
    assert_eq!(
        (site.kind, site.target.source),
        (SymbolKind::Field, provider.source())
    );
    assert_eq!(
        index.symbol_at(source, rhs).unwrap().symbol,
        index.symbol_at(source, local).unwrap().symbol
    );
    assert_ne!(index.symbol_at(source, rhs).unwrap().symbol, site.symbol);
    use crate::ValueAccessKind::{Read, Write};
    for (pos, access) in [
        (label, None),
        (read, Some(Read)),
        (write, Some(Write)),
        (last_label, None),
        (last_read, Some(Read)),
    ] {
        assert_eq!(index.symbol_at(source, pos).unwrap().symbol, site.symbol);
        assert_eq!(index.value_access_at(source, pos), access);
    }
    assert_eq!(
        index
            .references_to(site.symbol)
            .iter()
            .map(|s| (s.source(), s.span().range().start()))
            .collect::<Vec<_>>(),
        [label, read, write, last_label, last_read].map(|i| (source, i))
    );
}
