use super::*;
use crate::{CheckedClassBodies, ResolvedMemberIndex};

const PROVIDER: SourceId = SourceId::from_index(8);
const CALLER: SourceId = SourceId::from_index(3);

fn index(provider: &str, caller: &str, reverse: bool, errors: usize) -> ResolvedMemberIndex {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let a = named(8, provider, &mut store, &mut packages);
    let b = named(3, caller, &mut store, &mut packages);
    let files = if reverse { [&b, &a] } else { [&a, &b] };
    let sources = NamedClassSources::new(files, &store).unwrap();
    let mut arena = prepare_bodies(&sources, &mut store, defs, &packages);
    let mut bodies = CheckedClassBodies::default();
    let mut failures = Vec::new();
    for (_, class) in sources.classes() {
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
        failures.extend(report.errors);
        bodies.record(body).unwrap();
    }
    assert_eq!(failures.len(), errors, "{failures:?}");
    let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    assert!(index.diagnostics().is_empty());
    index
}

fn pos(text: &str, token: &str) -> u32 {
    text.find(token).unwrap() as u32
}

#[test]
fn written_arguments_preserve_ranges_order_and_formal_identity() {
    let a = "class Ops { def diff(left: Int, right: Int): Int = left - right }";
    let b = "class Use { def a(o: Ops): Int = o.diff(right = 4, left = 9); def b(o: Ops): Int = o.diff(9, 4) }";
    let idx = index(a, b, false, 0);
    let formal = |name| idx.parameter_at(PROVIDER, pos(a, name)).unwrap();
    for (token, expected) in [
        (
            "diff(right",
            [
                (1, "right = 4", Some("right")),
                (0, "left = 9", Some("left")),
            ],
        ),
        ("diff(9", [(0, "9", None), (1, "4", None)]),
    ] {
        let offset = pos(b, token);
        let binding = idx.call_arguments_at(CALLER, offset + 1).unwrap();
        assert_eq!(binding.written.len(), 2);
        assert_eq!(binding.call.method, formal("left:").method);
        let slice = |r: crate::core::TextRange| &b[r.start() as usize..r.end() as usize];
        for (arg, (ordinal, spelling, label)) in binding.written.iter().zip(expected) {
            assert_eq!(arg.ordinal, ordinal);
            assert_eq!(arg.parameter, formal(["left:", "right:"][ordinal]));
            assert_eq!(slice(arg.range), spelling);
            assert_eq!(arg.label.map(slice), label);
        }
        assert!(idx.call_arguments_at(CALLER, offset + 4).is_none());
    }
}

#[test]
fn omissions_and_labels_belong_to_the_selected_overload() {
    let a = "class Ops { def diff(left: Int = 9, right: Int = 4): Int = left - right; def pick(right: Boolean, n: Int): Int = 1; def pick(n: Int, right: Int): Int = 2; def both(i: Int, b: Boolean): Int = 1; def both(b: Boolean, i: Int): Int = 2 }";
    let b = "class Use { def all(o: Ops): Int = o.diff(); def partial(o: Ops): Int = o.diff(right = 4); def bool(o: Ops): Int = o.pick(right = true, n = 1); def int(o: Ops): Int = o.pick(right = 4, n = 1); def bad(o: Ops): Int = o.diff(nope = 2); def ambiguous(o: Ops): Int = o.both(i = 1, b = true) }";
    for reverse in [false, true] {
        let idx = index(a, b, reverse, 2);
        let query = |token| idx.call_arguments_at(CALLER, pos(b, token));
        let all = query("diff()").unwrap();
        let partial = query("diff(right").unwrap();
        assert!(all.written.is_empty());
        assert_eq!(partial.written[0].ordinal, 1);
        let (site, info) = idx.symbol_info_at(PROVIDER, pos(a, "diff")).unwrap();
        let crate::SourceSymbolSignature::Method {
            parameters: Some(params),
            ..
        } = &info.signature
        else {
            panic!()
        };
        assert_eq!(site.symbol, all.call.method);
        assert_eq!(all.defaulted.len(), 2);
        for (i, d) in all.defaulted.iter().enumerate() {
            assert_eq!(d.ordinal, i);
            assert_eq!(
                d.parameter,
                idx.parameter_at(PROVIDER, pos(a, ["left: Int", "right: Int"][i]))
                    .unwrap()
            );
            assert_eq!(Some(d.getter), params[i].default_getter);
        }
        assert_eq!(&*partial.defaulted, &all.defaulted[..1]);
        for (token, target, ordinal) in [
            ("pick(right = true", "right: Boolean", 0),
            ("pick(right = 4", "right: Int):", 1),
        ] {
            let call = query(token).unwrap();
            let p = idx.parameter_at(PROVIDER, pos(a, target)).unwrap();
            assert_eq!(
                (
                    call.call.method,
                    call.written[0].parameter,
                    call.written[0].ordinal
                ),
                (p.method, p, ordinal)
            );
            assert!(call.defaulted.is_empty());
        }
        assert!(query("diff(nope").is_none());
        assert!(query("both(i").is_none());
    }
}

#[test]
fn a_named_label_resolves_to_the_formal_and_its_value_keeps_its_lexical_symbol() {
    let a = "class Ops { def diff(left: Int = 9, right: Int = 4): Int = left - right }";
    let b = "class Use { def run(o: Ops): Int = { val right = 4; o.diff(`right` = o.diff(right = right), left = 9) } }";
    let idx = index(a, b, true, 0);
    let target = idx.parameter_at(PROVIDER, pos(a, "right:")).unwrap();
    let local = idx.symbol_at(CALLER, pos(b, "right = 4")).unwrap();
    let at = |offset| idx.symbol_info_at(CALLER, offset).unwrap().0;
    for (token, width) in [("`right`", 7), ("right = right", 5)] {
        let start = pos(b, token);
        for offset in start..start + width {
            assert_eq!(idx.parameter_at(CALLER, offset), Some(target));
            let site = at(offset);
            assert_eq!(
                (site.symbol, site.target),
                (target.parameter, target.target)
            );
            let range = site.usage.unwrap().span().range();
            assert_eq!((range.start(), range.end()), (start, start + width));
            assert_eq!(idx.value_access_at(CALLER, offset), None);
        }
        for offset in [start + width, start + width + 1] {
            assert!(idx.symbol_at(CALLER, offset).is_none());
        }
    }
    let rhs = pos(b, "right),");
    assert_eq!(at(rhs).symbol, local.symbol);
    assert_eq!(
        idx.value_access_at(CALLER, rhs),
        Some(crate::ValueAccessKind::Read)
    );
}

#[test]
fn references_merge_only_written_labels_with_reads_in_file_and_token_order() {
    let a = "class Ops { def pick(right: Int = 4): Int = right + pick(right = right); def pick(right: Boolean): Int = 1 }";
    let b = "class Use { def a(o: Ops): Int = o.pick(right = 1); def b(o: Ops): Int = o.pick(right = true); def c(o: Ops): Int = o.pick(); def bad(o: Ops): Int = o.pick(right = 3, right = 4) }";
    for reverse in [false, true] {
        let idx = index(a, b, reverse, 1);
        let references = |formal| {
            let p = idx.parameter_at(PROVIDER, pos(a, formal)).unwrap();
            idx.references_to(p.parameter)
                .iter()
                .map(|s| (s.source(), s.span().range().start()))
                .collect::<Vec<_>>()
        };
        let provider = ["right +", "right = right", "right);"].map(|name| (PROVIDER, pos(a, name)));
        let caller = [(CALLER, pos(b, "right = 1"))];
        let expected = if reverse {
            [caller.as_slice(), provider.as_slice()]
        } else {
            [provider.as_slice(), caller.as_slice()]
        };
        assert_eq!(references("right: Int"), expected.concat());
        assert_eq!(
            references("right: Boolean"),
            [(CALLER, pos(b, "right = true"))]
        );
    }
}
