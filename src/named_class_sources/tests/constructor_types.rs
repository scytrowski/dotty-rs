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

#[test]
fn class_references_interleave_constructor_types_with_method_types_parents_and_allocations() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let model_text = "class Base; class Peer; class Other(x: Base)";
    let text = "class Holder(seed: Base, val first: Base, var next: Peer) extends Base { def make(): Base = new Base() }; class Last(x: Base)";
    let model = named(9, model_text, &mut store, &mut packages);
    let file = named(2, text, &mut store, &mut packages);
    for order in [[&model, &file], [&file, &model]] {
        let sources = NamedClassSources::new(order, &store).unwrap();
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
        let mut bodies = crate::CheckedClassBodies::default();
        bodies.record(body).unwrap();
        let index = crate::ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
        for (class, token) in model.classes().iter().zip(["Base", "Peer"]) {
            let mut expected = Vec::new();
            for source in order {
                let (line, skip) = if source.source() == model.source() {
                    (model_text, 1)
                } else {
                    (text, 0)
                };
                for (pos, _) in line.match_indices(token).skip(skip) {
                    let site = index.symbol_at(source.source(), pos as u32).unwrap();
                    assert_eq!(
                        (site.symbol, site.kind, site.target.source),
                        (class.symbol, SymbolKind::Class, model.source())
                    );
                    let usage = site.usage.unwrap();
                    assert_eq!(
                        (
                            usage.source(),
                            usage.span().range().start(),
                            usage.span().range().end()
                        ),
                        (source.source(), pos as u32, (pos + token.len()) as u32)
                    );
                    expected.push(usage);
                }
            }
            assert_eq!(index.references_to(class.symbol), expected);
        }
    }
}

#[test]
fn constructor_annotations_follow_signature_success_independently_of_siblings_and_bodies() {
    use crate::{ClassHeaderError, ResolvedMemberIndex};
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let base = named(9, "class Base", &mut store, &mut packages);
    let text = "class Good(x: Base) { def only(n: Int): Int = n; def broken(): Int = only(false) }; class Bad(a: Base, b: Missing)";
    let file = named(3, text, &mut store, &mut packages);
    let [good, bad] = file.classes() else {
        panic!("two constructors")
    };
    let class = base.classes()[0].symbol;
    let sources = NamedClassSources::new([&base, &file], &store).unwrap();
    let mut arena = AstArena::new();
    let mut bodies = crate::CheckedClassBodies::default();
    let before = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    assert!(before.references_to(class).is_empty());
    for id in [class, good.symbol] {
        sources
            .complete_constructor_signature(id, &mut store, defs, &packages)
            .unwrap();
    }
    let ClassHeaderError::Typing(error) = sources
        .complete_constructor_signature(bad.symbol, &mut store, defs, &packages)
        .unwrap_err()
    else {
        panic!("typing error")
    };
    assert_eq!(
        (error.class(), error.member()),
        (Some(bad.symbol), Some(bad.constructor))
    );
    assert!(
        matches!(error.error(), TyperError::TypeNameNotFound {source,..} if *source == file.source())
    );
    let pos = error.location().unwrap();
    let range = pos.span().range();
    assert_eq!(pos.source(), file.source());
    assert_eq!(
        &text[range.start() as usize..range.end() as usize],
        "Missing"
    );
    let check = |index: ResolvedMemberIndex| {
        let site = index
            .constructor_type_at(file.source(), text.find("Base").unwrap() as u32)
            .unwrap();
        assert_eq!((site.constructor, site.class), (good.constructor, class));
        assert_eq!(index.references_to(class), &[site.origin]);
        for offset in [text.rfind("Base").unwrap() as u32, range.start()] {
            assert!(index.symbol_at(file.source(), offset).is_none());
        }
    };
    check(ResolvedMemberIndex::build(
        &sources, &bodies, &arena, &store,
    ));
    let mut headers = ClassHeaderBindings::default();
    for id in [class, good.symbol] {
        assert!(matches!(
            sources.try_complete_class_header(
                id,
                &mut store,
                defs,
                &packages,
                &mut arena,
                &mut headers
            ),
            Ok(SignatureCompletion::Complete(_))
        ));
    }
    for method in good.methods() {
        assert!(matches!(
            sources.try_complete_member_signature(method.symbol, &mut store, defs, &packages),
            Ok(SignatureCompletion::Complete(_))
        ));
    }
    let (body, report) = sources
        .check_class_body(
            good.symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    assert!(
        matches!(report.errors.as_slice(),[crate::ClassBodyError::Member {member,error:TyperError::ApplicationArgumentTypeMismatch {argument_index:0,..},..}] if member.symbol()==good.methods().last().unwrap().symbol)
    );
    bodies.record(body).unwrap();
    check(ResolvedMemberIndex::build(
        &sources, &bodies, &arena, &store,
    ));
}
