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
