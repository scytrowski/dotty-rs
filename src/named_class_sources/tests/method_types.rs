use super::*;
use crate::{CheckedClassBodies, MethodTypeRole, ResolvedMemberIndex};

#[test]
fn method_type_positions_follow_canonical_formals_and_results() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let decoy = named(5, "package other\nclass Base", &mut store, &mut packages);
    let provider = named(
        2,
        "package demo\nclass Base; class Peer",
        &mut store,
        &mut packages,
    );
    let text = "package demo\nclass Use { def choose(z: Peer, a: /*in*/ `Base`): /*out*/ Base = a; def infer(x: Base) = x }";
    let caller = named(7, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&decoy, &provider, &caller], &store).unwrap();
    let arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let index =
        ResolvedMemberIndex::build(&sources, &CheckedClassBodies::default(), &arena, &store);
    let methods: Vec<_> = caller.classes()[0].methods().collect();
    let [base, peer] = provider.classes() else {
        panic!("classes")
    };
    for (method, search, token, parameter, class) in [
        (0, "Peer,", "Peer", Some("z:"), peer),
        (0, "`Base`", "`Base`", Some("a:"), base),
        (0, "Base =", "Base", None, base),
        (1, "Base)", "Base", Some("x:"), base),
    ] {
        let pos = text.find(search).unwrap() as u32;
        let site = index.method_type_at(caller.source(), pos).unwrap();
        let role = parameter.map_or(MethodTypeRole::Result, |s| {
            MethodTypeRole::Parameter(
                index
                    .parameter_at(caller.source(), text.find(s).unwrap() as u32)
                    .unwrap()
                    .parameter,
            )
        });
        assert_eq!(
            (site.method, site.role, site.class),
            (methods[method].symbol, role, class.symbol)
        );
        assert_eq!(site.target.source, provider.source());
        assert_eq!(
            (
                site.origin.span().range().start(),
                site.origin.span().range().end()
            ),
            (pos, pos + token.len() as u32)
        );
        assert!(
            index
                .method_type_at(caller.source(), pos + token.len() as u32)
                .is_none()
        );
    }
    assert!(
        index
            .method_type_at(caller.source(), text.rfind("= x").unwrap() as u32)
            .is_none()
    );
}

#[test]
fn qualified_names_and_multiple_lists_have_no_simple_method_type_sites() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let model = named(1, "package model\nclass Base", &mut store, &mut packages);
    let text = "package model\nclass Use { def qualified(v: model.Base): model.Base = v; def curried(v: Base)(n: Int): Base = v }";
    let caller = named(4, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&model, &caller], &store).unwrap();
    let arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let index =
        ResolvedMemberIndex::build(&sources, &CheckedClassBodies::default(), &arena, &store);
    for (pos, _) in text.match_indices("Base") {
        assert!(index.method_type_at(caller.source(), pos as u32).is_none());
    }
}

#[test]
fn class_references_interleave_annotations_allocations_and_inheritance() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let model = named(
        9,
        "class Base; class First extends Base",
        &mut store,
        &mut packages,
    );
    let text = "class Use { def copy(x: Base): Base = new Base() }; class Last extends Base";
    let caller = named(2, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&model, &caller], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
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
    let base = model.classes()[0].symbol;
    let uses = index.references_to(base);
    assert_eq!(uses.len(), 5);
    assert_eq!(uses[0].source(), model.source());
    let positions: Vec<_> = text.match_indices("Base").map(|(i, _)| i as u32).collect();
    for (span, pos) in uses[1..].iter().zip(positions) {
        assert_eq!(
            (span.source(), span.span().range().start()),
            (caller.source(), pos)
        );
        let site = index.symbol_at(caller.source(), pos).unwrap();
        assert_eq!(
            (site.kind, site.symbol, site.usage),
            (SymbolKind::Class, base, Some(*span))
        );
        assert_eq!(site.target.source, model.source());
    }
}
