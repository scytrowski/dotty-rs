use super::*;
use crate::core::{SourceDefinition, TreeKind};

#[test]
fn constructor_type_ranges_pair_each_written_parameter_with_its_derived_formal() {
    let text = "class Holder(seed: Base, val first: /* 😀 */ `Base`, var next: Peer)";
    let mut store = SemanticStore::new();
    let mut packages = Packages::new();
    let file = named(7, text, &mut store, &mut packages);
    let holder = &file.classes()[0];
    let Some(SourceDefinition::Canonical { source, tree }) =
        file.index().definition_of(holder.constructor)
    else {
        panic!("source constructor")
    };
    let TreeKind::DefDef(def) = &file.ast().get(tree).kind else {
        panic!("constructor definition")
    };
    let [clause] = def.value_param_clauses.as_slice() else {
        panic!("one clause")
    };
    assert_eq!(clause.len(), 3);
    for (&tree, (token, kind)) in clause.iter().zip([
        ("Base", SymbolKind::Parameter),
        ("`Base`", SymbolKind::Field),
        ("Peer", SymbolKind::Field),
    ]) {
        let TreeKind::ValDef(param) = &file.ast().get(tree).kind else {
            panic!("parameter")
        };
        let pos = file.ast().get(param.tpt).position.unwrap();
        let start = text.find(token).unwrap() as u32;
        assert_eq!(
            (
                pos.source(),
                pos.span().range().start(),
                pos.span().range().end()
            ),
            (file.source(), start, start + token.len() as u32)
        );
        let symbol = file.index().symbol_at(source, tree).unwrap();
        let formal = file
            .index()
            .derived_symbol_at(holder.constructor, source, tree)
            .unwrap();
        for (id, owner, expected_kind) in [
            (symbol, holder.symbol, kind),
            (formal, holder.constructor, SymbolKind::Parameter),
        ] {
            let entry = store.symbols.get(id);
            assert_eq!(
                (entry.owner, entry.kind, entry.name),
                (Some(owner), expected_kind, *param.name.as_name())
            );
        }
    }
}

#[test]
fn constructor_type_queries_resolve_completed_formals_and_exact_class_targets() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let other = named(8, "package other\nclass Base", &mut store, &mut packages);
    let model = named(
        2,
        "package demo\nclass Base; class Peer",
        &mut store,
        &mut packages,
    );
    let text = "package demo\nclass Holder(seed: Base, val first: `Base`, var next: Peer, count: Int, qualified: demo.Peer); class More(x: Base)(y: Base)";
    let file = named(3, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&other, &file, &model], &store).unwrap();
    let holder = &file.classes()[0];
    for class in file.classes() {
        sources
            .complete_constructor_signature(class.symbol, &mut store, defs, &packages)
            .unwrap();
    }
    let index =
        crate::ResolvedMemberIndex::build(&sources, &Default::default(), &AstArena::new(), &store);
    for (token, param, class) in [
        ("Base", "seed", &model.classes()[0]),
        ("`Base`", "first", &model.classes()[0]),
        ("Peer", "next", &model.classes()[1]),
    ] {
        let start = text.find(token).unwrap() as u32;
        let site = index.constructor_type_at(file.source(), start).unwrap();
        let source_param = index
            .symbol_at(file.source(), text.find(param).unwrap() as u32)
            .unwrap()
            .symbol;
        let Some(SourceDefinition::Canonical { source, tree }) =
            file.index().definition_of(source_param)
        else {
            panic!("source parameter")
        };
        assert_eq!(
            (site.constructor, site.parameter, Some(site.formal)),
            (
                holder.constructor,
                source_param,
                file.index()
                    .derived_symbol_at(holder.constructor, source, tree)
            )
        );
        assert_eq!(
            (site.class, site.target.source),
            (class.symbol, model.source())
        );
        assert_eq!(
            index
                .symbol_at(model.source(), site.target.name.start())
                .unwrap()
                .symbol,
            class.symbol
        );
        assert_eq!(site.origin.span().range().start(), start);
        assert_eq!(site.origin.span().range().end(), start + token.len() as u32);
        assert!(
            index
                .constructor_type_at(file.source(), start + token.len() as u32)
                .is_none()
        );
    }
    for token in ["seed", "Int", "demo.Peer", "x: Base", "y: Base"] {
        let start = text.find(token).unwrap() as u32;
        for offset in start..start + token.len() as u32 {
            assert!(index.constructor_type_at(file.source(), offset).is_none());
        }
    }
}
