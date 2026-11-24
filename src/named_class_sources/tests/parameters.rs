use super::*;
use crate::{CheckedClassBodies, ResolvedMemberIndex};

#[test]
fn overloaded_methods_own_their_exact_parameter_declarations_before_body_checking() {
    let text = "class A(x: Int) { def pick(x: Int): Int = 1; def pick(x: Boolean): Int = 2; def curried(y: Int)(z: Int): Int = y }";
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
    assert!(index.diagnostics().is_empty());
    let mut parameters = Vec::new();
    for ((offset, _), method) in text
        .match_indices("pick(x")
        .zip(file.classes()[0].methods())
    {
        let start = offset as u32 + 5;
        let p = index.parameter_at(file.source(), start).unwrap();
        assert_eq!(p.method, method.symbol);
        assert_eq!(p.target.name.start(), start);
        assert_eq!(p.target.name.end(), start + 1);
        let full = p.target.declaration;
        assert_eq!(
            &text[full.start() as usize..full.end() as usize],
            if parameters.is_empty() {
                "x: Int"
            } else {
                "x: Boolean"
            }
        );
        assert_eq!(store.symbols.get(p.parameter).owner, Some(p.method));
        assert!(index.parameter_at(file.source(), start + 1).is_none());
        assert!(index.references_to(p.parameter).is_empty());
        parameters.push(p);
    }
    assert_eq!(parameters.len(), 2);
    assert_ne!(parameters[0].parameter, parameters[1].parameter);
    assert_ne!(parameters[0].method, parameters[1].method);
    for name in ["x:", "y:", "z:"] {
        assert!(
            index
                .parameter_at(file.source(), text.find(name).unwrap() as u32)
                .is_none()
        );
    }
}

#[test]
fn committed_parameter_reads_respect_shadowing_and_partial_body_rollback() {
    let text = "class A { var count: Int = 0; def set(count: Int): Unit = this.count = count; def read(count: Int): Int = { val before = count; { val count = 2; count }; before + count }; def bad(count: Int): Int = count + false }";
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = named(7, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&file], &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let class = &file.classes()[0];
    let (body, report) = sources
        .check_class_body(
            class.symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    assert_eq!(report.errors.len(), 1, "{report:?}");
    let mut bodies = CheckedClassBodies::default();
    bodies.record(body).unwrap();
    let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    assert!(index.diagnostics().is_empty());
    let names: Vec<_> = text.match_indices("count").map(|(i, _)| i as u32).collect();
    let [
        field,
        setter,
        lhs,
        rhs,
        reader,
        before,
        local,
        local_read,
        after,
        bad,
        bad_read,
    ] = names[..]
    else {
        panic!("fixture names")
    };
    let at = |offset| index.parameter_at(file.source(), offset);
    let set = at(setter).unwrap();
    let read = at(reader).unwrap();
    assert_ne!(set.parameter, read.parameter);
    assert_eq!(at(rhs), Some(set));
    assert_eq!(at(before), Some(read));
    assert_eq!(at(after), Some(read));
    for pos in [field, lhs, local, local_read, bad_read] {
        assert!(at(pos).is_none(), "offset {pos}");
    }
    assert_eq!(
        index.field_at(file.source(), lhs).unwrap().kind,
        crate::FieldAccessKind::Write
    );
    assert!(index.references_to(at(bad).unwrap().parameter).is_empty());
    let offsets = |p| {
        index
            .references_to(p)
            .iter()
            .map(|s| s.span().range().start())
            .collect::<Vec<_>>()
    };
    assert_eq!(offsets(set.parameter), [rhs]);
    assert_eq!(offsets(read.parameter), [before, after]);
}

#[test]
fn unified_symbol_sites_retain_only_revision_data_and_member_lookup_stays_scoped() {
    use crate::core::SymbolKind;
    let source = SourceId::from_index(7);
    let text =
        "class A { val n: Int = 1; def add(x: Int): Int = x + n; def result(): Int = add(2) }";
    let index = {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let file = named(7, text, &mut store, &mut packages);
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
        assert!(report.errors.is_empty());
        let mut bodies = CheckedClassBodies::default();
        bodies.record(body).unwrap();
        ResolvedMemberIndex::build(&sources, &bodies, &arena, &store)
    };
    for (declaration, expression, kind) in [
        ("add(x", "add(2)", SymbolKind::Method),
        ("n: Int", "n;", SymbolKind::Field),
        ("x: Int", "x +", SymbolKind::Parameter),
    ] {
        let decl = text.find(declaration).unwrap() as u32;
        let usage = text.find(expression).unwrap() as u32;
        let target = index.symbol_at(source, decl).unwrap();
        let site = index.symbol_at(source, usage).unwrap();
        assert_eq!(target.kind, kind);
        assert_eq!(target.usage, None);
        assert_eq!(site.symbol, target.symbol);
        assert_eq!(site.target, target.target);
        assert_eq!(site.kind, kind);
        let span = site.usage.unwrap();
        assert_eq!(span.span().range().start(), usage);
        assert_eq!(index.references_to(site.symbol), &[span]);
        assert_eq!(
            index.member_at(source, usage).is_some(),
            kind != SymbolKind::Parameter
        );
        assert_eq!(
            index.member_at(source, decl).is_some(),
            kind != SymbolKind::Parameter
        );
    }
}
