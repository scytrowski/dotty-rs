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
