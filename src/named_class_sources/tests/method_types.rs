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

#[test]
fn a_completed_signature_survives_body_failure_and_a_rejected_signature_publishes_nothing() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let base = named(9, "class Base", &mut store, &mut packages);
    let text = "class Use { def only(n: Int): Base = new Base(); def badBody(x: Base): Base = only(false) }; class Mixed { def badType(x: Base): Missing = x; def good(x: Base): Base = x }";
    let caller = named(2, text, &mut store, &mut packages);
    let sources = NamedClassSources::new([&base, &caller], &store).unwrap();
    let mut arena = AstArena::new();
    let mut headers = ClassHeaderBindings::default();
    prepare_constructors(&sources, &mut store, defs, &packages);
    for (_, c) in sources.classes() {
        assert!(matches!(
            sources.try_complete_class_header(
                c.symbol,
                &mut store,
                defs,
                &packages,
                &mut arena,
                &mut headers
            ),
            Ok(SignatureCompletion::Complete(_))
        ));
    }
    let [used, mixed] = caller.classes() else {
        panic!("expected both classes")
    };
    let [bad, good] = mixed.methods().collect::<Vec<_>>()[..] else {
        panic!("expected two methods")
    };
    let build = |store: &SemanticStore, bodies: &CheckedClassBodies, arena: &AstArena<Typed>| {
        ResolvedMemberIndex::build(&sources, bodies, arena, store)
    };
    let mut bodies = CheckedClassBodies::default();
    let class = base.classes()[0].symbol;
    assert!(
        build(&store, &bodies, &arena)
            .references_to(class)
            .is_empty()
    );
    for method in used.methods().chain([good]) {
        assert!(matches!(
            sources.try_complete_member_signature(method.symbol, &mut store, defs, &packages),
            Ok(SignatureCompletion::Complete(_))
        ));
    }
    let error = sources
        .try_complete_member_signature(bad.symbol, &mut store, defs, &packages)
        .unwrap_err();
    assert!(
        matches!(error, MemberSignatureError::Typing{ member, error: TyperError::TypeNameNotFound{source,position:Some(pos),..}, .. }
        if member==bad.symbol && source==caller.source() && pos.span().range().start()==text.find("Missing").unwrap() as u32)
    );
    let before = build(&store, &bodies, &arena);
    let (body, report) = sources
        .check_class_body(
            used.symbol,
            &mut store,
            defs,
            &packages,
            &mut arena,
            crate::core::NoResolver,
        )
        .into_parts();
    assert!(
        matches!(report.errors.as_slice(),[crate::ClassBodyError::Member{member,error:TyperError::ApplicationArgumentTypeMismatch{argument_index:0,..},..}] if member.symbol()==used.methods().last().unwrap().symbol)
    );
    bodies.record(body).unwrap();
    let after = build(&store, &bodies, &arena);
    let offsets: Vec<_> = text.match_indices("Base").map(|(i, _)| i as u32).collect();
    let [
        result,
        allocation,
        input,
        bad_result,
        rejected,
        good_input,
        good_result,
    ] = offsets[..]
    else {
        panic!("annotation offsets")
    };
    for index in [&before, &after] {
        for pos in [result, input, bad_result, good_input, good_result] {
            assert_eq!(
                index.method_type_at(caller.source(), pos).unwrap().class,
                class
            );
        }
        for pos in [rejected, text.find("Missing").unwrap() as u32] {
            assert!(index.symbol_at(caller.source(), pos).is_none());
        }
    }
    assert_eq!(before.references_to(class).len(), 5);
    assert!(before.symbol_at(caller.source(), allocation).is_none());
    assert_eq!(after.references_to(class).len(), 6);
    assert_eq!(
        after
            .construction_at(caller.source(), allocation)
            .unwrap()
            .class,
        class
    );
}

#[test]
fn selected_overload_retains_its_own_ordered_signature_in_either_file_order() {
    use crate::SourceSymbolSignature::Method;
    use crate::core::SymbolInfo;
    let methods = "class Ops { def pick(n: Int, b: Boolean): Boolean = b; def pick(b: Boolean, n: Int): Int = n; def empty() = 1; def plain = true; def curried(n: Int)(b: Boolean): Int = n }";
    let calls = "class Use { def a(o: Ops): Boolean = o.pick(1, false); def b(o: Ops): Int = o.pick(true, 2); def bad(o: Ops): Int = o.pick(true, false); def nested(o: Ops): Int = o.pick(o.pick(1, false), o.empty()); def id(o: Ops): Int = 0; def construct(): Int = id(new Ops()) }";
    for reverse in [false, true] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let ops = named(8, methods, &mut store, &mut packages);
        let caller = named(3, calls, &mut store, &mut packages);
        let files = if reverse {
            [&caller, &ops]
        } else {
            [&ops, &caller]
        };
        let sources = NamedClassSources::new(files, &store).unwrap();
        let before = ResolvedMemberIndex::build(
            &sources,
            &CheckedClassBodies::default(),
            &AstArena::new(),
            &store,
        );
        let decl = methods.find("pick").unwrap() as u32;
        assert!(before.symbol_at(ops.source(), decl).is_some());
        assert!(before.symbol_info_at(ops.source(), decl).is_none());

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
        assert_eq!(report.errors.len(), 1);
        let mut bodies = CheckedClassBodies::default();
        bodies.record(body).unwrap();
        let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
        for (call, ordinal, expected, result_ty) in [
            (
                "pick(1",
                0,
                [("n", defs.int), ("b", defs.boolean)],
                defs.boolean,
            ),
            (
                "pick(true, 2",
                1,
                [("b", defs.boolean), ("n", defs.int)],
                defs.int,
            ),
        ] {
            let pos = calls.find(call).unwrap() as u32;
            let (site, info) = index.symbol_info_at(caller.source(), pos).unwrap();
            assert_eq!(
                site.symbol,
                ops.classes()[0].methods().nth(ordinal).unwrap().symbol
            );
            assert_eq!(&*info.name, "pick");
            let Method {
                ty,
                parameters: Some(params),
                result,
            } = &info.signature
            else {
                panic!("{info:?}")
            };
            assert_eq!(store.symbols.info(site.symbol), &SymbolInfo::Complete(*ty));
            assert_eq!(
                params
                    .iter()
                    .map(|p| (&*p.name, p.ty.ty))
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(result.ty, result_ty);
            let after_comma = pos + calls[pos as usize..].find(',').unwrap() as u32 + 1;
            for (cursor, active) in [(pos + 5, 0), (after_comma - 1, 0), (after_comma, 1)] {
                let selected = index.call_signature_at(caller.source(), cursor).unwrap();
                assert_eq!(selected.call.method, site.symbol);
                assert_eq!(selected.ty, *ty);
                assert!(std::ptr::eq(selected.parameters, &**params));
                assert_eq!(selected.active_parameter, Some(active));
                assert_eq!(selected.parameters[active].ty.ty, expected[active].1);
                assert_eq!(selected.result.ty, result_ty);
            }
            assert!(std::ptr::eq(
                index
                    .symbol_info_at(ops.source(), site.target.name.start())
                    .unwrap()
                    .1,
                info
            ));
        }
        let help = |s: &str, delta: usize| {
            index.call_signature_at(caller.source(), (calls.rfind(s).unwrap() + delta) as u32)
        };
        assert!(help("pick(true, false)", 5).is_none());
        assert!(help("pick(true, false)", 4).is_none());
        assert!(help("new Ops()", 8).is_none());
        assert_eq!(help("id(new", 3).unwrap().active_parameter, Some(0));
        let inner = help("1, false),", 3).unwrap();
        assert_eq!(
            (inner.result.ty, inner.active_parameter),
            (defs.boolean, Some(1))
        );
        let outer = help("), o.empty", 2).unwrap();
        assert_eq!(
            (outer.result.ty, outer.active_parameter),
            (defs.int, Some(1))
        );
        let empty = help("empty()", 6).unwrap();
        assert_eq!((empty.name, empty.active_parameter), ("empty", None));
        assert!(empty.parameters.is_empty());
        let at = |s: &str| index.symbol_info_at(ops.source(), methods.find(s).unwrap() as u32);
        assert!(
            matches!(&at("empty").unwrap().1.signature, Method { parameters: Some(p), result, .. } if p.is_empty() && result.ty == defs.int)
        );
        assert!(
            matches!(&at("plain").unwrap().1.signature, Method { parameters: None, result, .. } if result.ty == defs.boolean)
        );
        assert!(at("curried").is_none());
        assert!(
            index
                .symbol_info_at(
                    caller.source(),
                    calls.find("pick(true, false)").unwrap() as u32
                )
                .is_none()
        );
    }
}

#[test]
fn incomplete_prefixes_keep_both_overloads_until_the_third_argument() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let ops = named(
        1,
        "class Ops { def pick(a: Int, b: Boolean, n: Int, tail: Boolean): Int = n; def pick(a: Int, b: Boolean, flag: Boolean, tail: Int): Int = tail; private def pick(a: Boolean): Boolean = a }; class Other { def pick(a: Int): Int = a }",
        &mut store,
        &mut packages,
    );
    let sources = NamedClassSources::new([&ops], &store).unwrap();
    prepare_bodies(&sources, &mut store, defs, &packages);
    let methods: Vec<_> = ops.classes()[0].methods().map(|m| m.symbol).collect();
    let id = SourceId::from_index(4);
    let text = "class Use { def run(ops: Ops): Int = ops.pick(1, true, false, ";
    let parsed = crate::parse_source(id, text, Default::default())
        .unwrap()
        .bind(&mut store.names);
    assert!(!parsed.diagnostics.is_empty());
    let index =
        crate::IncompleteCallIndex::build(&sources, [("Use.scala", parsed)], store, defs, packages);
    for (prefix, slot, expected) in [
        ("pick(", 0, &methods[..2]),
        ("1,", 1, &methods[..2]),
        ("true,", 2, &methods[..2]),
        ("false,", 3, &methods[1..2]),
    ] {
        let cursor = (text.find(prefix).unwrap() + prefix.len()) as u32;
        let query = index.at(id, cursor).unwrap();
        assert_eq!(query.active_parameter, slot);
        assert_eq!(
            query
                .candidates
                .iter()
                .map(|c| c.method)
                .collect::<Vec<_>>(),
            expected
        );
        for candidate in query.candidates {
            assert_eq!(candidate.parameters.len(), 4);
            assert_eq!(candidate.target.source, ops.source());
            assert_eq!(candidate.result.ty, defs.int);
        }
    }
    assert_eq!(
        index.at(id, text.len() as u32).unwrap().candidates[0].parameters[3]
            .ty
            .ty,
        defs.int
    );
    assert!(index.at(ops.source(), text.len() as u32).is_none());
}

#[test]
fn incomplete_calls_respect_failed_arguments_receiver_scope_and_written_arity() {
    for (body, has_candidate) in [
        ("ops.pick(1, ", true),
        ("ops.pick(false, ", false),
        ("ops.pick(unknown, ", false),
        ("ops.pick(1, true, ", false),
        ("other.pick(1, ", false),
        ("{ val ops = other; ops.pick(1, ", false),
        ("{ val broken = unknown; ops.pick(1, ", false),
        ("{ def local: Int = 1; ops.pick(1, ", false),
        ("{ lazy val local = 1; ops.pick(1, ", false),
        ("{ import other.*; ops.pick(1, ", false),
        ("ops.pick(ops.pick(1, ", true),
    ] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let packages = Packages::new();
        let providers = NamedClassSources::new([], &store).unwrap();
        let id = SourceId::from_index(5);
        let text = format!(
            "class Ops {{ def pick(n: Int, b: Boolean): Int = n }}; class Other; class Use {{ def run(ops: Ops, other: Other): Int = {body}"
        );
        let parsed = crate::parse_source(id, &text, Default::default())
            .unwrap()
            .bind(&mut store.names);
        let index = crate::IncompleteCallIndex::build(
            &providers,
            [("Use.scala", parsed)],
            store,
            defs,
            packages,
        );
        assert_eq!(
            index.at(id, text.len() as u32).is_some(),
            has_candidate,
            "{body}"
        );
    }
}

#[test]
fn unfinished_calls_use_the_local_receiver_and_typed_third_argument() {
    for (local, ordinal) in [("val third: Int = 2", 0), ("var third = false", 1)] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let ops = named(
            1,
            "class Ops { def pick(a: Int, b: Boolean, n: Int, tail: Boolean): Int = n; def pick(a: Int, b: Boolean, flag: Boolean, tail: Int): Int = tail }; class Other { def pick(a: Int, b: Boolean, n: Int, wrong: Int): Int = n }",
            &mut store,
            &mut packages,
        );
        let sources = NamedClassSources::new([&ops], &store).unwrap();
        prepare_bodies(&sources, &mut store, defs, &packages);
        let expected = ops.classes()[0].methods().nth(ordinal).unwrap().symbol;
        let id = SourceId::from_index(4);
        let text = format!(
            "class Use {{ def run(ops: Other, actual: Ops): Int = {{ val ops = actual; {local}; ops.pick(1, true, third, "
        );
        let parsed = crate::parse_source(id, &text, Default::default())
            .unwrap()
            .bind(&mut store.names);
        let index = crate::IncompleteCallIndex::build(
            &sources,
            [("Use.scala", parsed)],
            store,
            defs,
            packages,
        );
        let help = index.at(id, text.len() as u32).unwrap();
        assert_eq!(help.active_parameter, 3);
        let [candidate] = &help.candidates[..] else {
            panic!("{help:?}")
        };
        assert_eq!(candidate.method, expected);
        assert_eq!(
            candidate.parameters[3].ty.ty,
            [defs.boolean, defs.int][ordinal]
        );
    }
}

#[test]
fn unfinished_initializer_and_checked_call_select_the_same_source_overload() {
    for declaration in ["val result", "var result: Int"] {
        for (third, tail, ordinal) in [("2", "false", 0), ("false", "2", 1)] {
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let ops = named(
                1,
                "class Ops { def pick(a: Int, b: Boolean, n: Int, tail: Boolean): Int = n; def pick(a: Int, b: Boolean, flag: Boolean, tail: Int): Int = tail }",
                &mut store,
                &mut packages,
            );
            // Both callers share the same provider symbols and types, so the
            // committed and provisional identities can be compared directly.
            let prefix = format!(
                "def run(ops: Ops): Int = {{ val third = {third}; {declaration} = ops.pick(1, true, third, "
            );
            let closed = format!("class Checked {{ {prefix}{tail}); result }} }}");
            let checked = named(2, &closed, &mut store, &mut packages);
            let sources = NamedClassSources::new([&ops, &checked], &store).unwrap();
            let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
            let (body, report) = sources
                .check_class_body(
                    checked.classes()[0].symbol,
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
            let resolved = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
            let cursor = closed.find("third, ").unwrap() + "third, ".len();
            let selected = resolved
                .call_signature_at(checked.source(), cursor as u32)
                .unwrap();
            assert_eq!(selected.active_parameter, Some(3));
            assert_eq!(
                selected.call.method,
                ops.classes()[0].methods().nth(ordinal).unwrap().symbol
            );
            assert_eq!(
                selected.parameters[3].ty.ty,
                [defs.boolean, defs.int][ordinal]
            );

            let id = SourceId::from_index(3);
            let open = format!("class Editing {{ {prefix}");
            let parsed = crate::parse_source(id, &open, Default::default())
                .unwrap()
                .bind(&mut store.names);
            let pending = crate::IncompleteCallIndex::build(
                &sources,
                [("Editing.scala", parsed)],
                store,
                defs,
                packages,
            );
            let help = pending.at(id, open.len() as u32).unwrap();
            assert_eq!(help.active_parameter, 3);
            let [candidate] = help.candidates.as_slice() else {
                panic!("{help:?}")
            };
            assert_eq!(candidate.method, selected.call.method);
            assert_eq!(candidate.parameters[3].ty.ty, selected.parameters[3].ty.ty);
        }
    }
}

const PICKS: &str = "def pick(a: Int, b: Boolean, n: Int, tail: Boolean): Int = n; def pick(a: Int, b: Boolean, flag: Boolean, tail: Int): Int = tail";

fn unfinished_in_one_file(text: &str) -> (crate::IncompleteCallIndex, SourceId) {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let sources = NamedClassSources::new([], &store).unwrap();
    let id = SourceId::from_index(1);
    let parsed = crate::parse_source(id, text, Default::default())
        .unwrap()
        .bind(&mut store.names);
    assert!(!parsed.diagnostics.is_empty());
    (
        crate::IncompleteCallIndex::build(
            &sources,
            [("Use.scala", parsed)],
            store,
            defs,
            Packages::new(),
        ),
        id,
    )
}

#[test]
fn bare_initializer_call_narrows_only_after_its_third_argument() {
    let text = format!(
        "class Use {{ {PICKS}; def run: Int = {{ val third = false; val result = pick(1, true, third, "
    );
    let (index, id) = unfinished_in_one_file(&text);
    let names: Vec<_> = text
        .match_indices("def pick")
        .map(|(i, _)| i as u32 + 4)
        .collect();
    for (suffix, slot, expected) in [
        ("pick(", 0, &names[..]),
        ("1,", 1, &names[..]),
        ("true,", 2, &names[..]),
        ("third,", 3, &names[1..]),
    ] {
        let cursor = text.rfind(suffix).unwrap() + suffix.len();
        let help = index.at(id, cursor as u32).unwrap();
        assert_eq!(help.active_parameter, slot);
        assert_eq!(
            help.candidates
                .iter()
                .map(|c| c.target.name.start())
                .collect::<Vec<_>>(),
            expected
        );
    }
    assert_eq!(
        index.at(id, text.len() as u32).unwrap().candidates[0].parameters[3]
            .ty
            .name
            .as_ref(),
        "Int"
    );
}

#[test]
fn unfinished_calls_offer_plain_private_overloads_only_inside_the_owner() {
    let methods = "def pick(n: Int, tail: Int): Int = n; private def pick(b: Boolean, tail: Boolean): Boolean = b";
    for (caller, prefix, expected) in [
        ("", "this.pick(", vec!["Int", "Boolean"]),
        ("", "pick(false, ", vec!["Boolean"]),
        ("", "other.pick(false, ", vec!["Boolean"]),
        ("}; class Outside {", "other.pick(", vec!["Int"]),
        ("}; class Outside {", "other.pick(false, ", vec![]),
    ] {
        let text = format!("class Ops {{ {methods}; {caller} def run(other: Ops): Int = {prefix}");
        let (index, id) = unfinished_in_one_file(&text);
        let help = index.at(id, text.len() as u32);
        let names = help.as_ref().map(|h| {
            h.candidates
                .iter()
                .map(|c| c.parameters[1].ty.name.as_ref())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            names.as_deref(),
            (!expected.is_empty()).then_some(expected.as_slice()),
            "{text}"
        );
    }
}
