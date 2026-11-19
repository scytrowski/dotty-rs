//! Scheduling experiments in the external client, using one source method per file.
use std::collections::{HashMap, HashSet};

use super::super::{NamedUnit, SignatureCompletion, complete_signatures_in_order};
use super::{
    Layout, Timings, compile_timed, compile_with_signatures, dependent_sources, inputs, name_units,
    parsing,
};
use dotty::core::{Definitions, Packages, SemanticStore, SourceId, SymbolInfo, Type};
use dotty::jvm::GeneratedClass;

#[derive(Clone, Copy, Debug)]
enum Order {
    Callers,
    Callees,
}

impl Order {
    fn roots(self, groups: usize) -> Vec<usize> {
        match self {
            // The existing client's file order: Last, Second, First, Leaf in each group.
            Self::Callers => (0..4 * groups).collect(),
            // Only this fixture knows its four levels in advance.
            Self::Callees => (0..4)
                .rev()
                .flat_map(|level| (0..groups).map(move |group| 4 * group + level))
                .collect(),
        }
    }
}

fn compile_order(
    files: &[(SourceId, &str, &str)],
    workers: usize,
    roots: &[usize],
) -> (Vec<GeneratedClass>, Timings) {
    compile_with_signatures(files, workers, |units, store, defs, packages| {
        complete_signatures_in_order(
            units,
            roots.iter().map(|&i| &units[i]),
            store,
            defs,
            packages,
            |_, _| {},
        )
    })
}

// Visit all consumers at each level before visiting any of their producers.
// Each non-leaf fixture method has exactly one cross-file call. On its first
// attempt the public API therefore identifies its edge even in the shared layout.
fn ready_widths(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    defs: Definitions,
    packages: &Packages,
) -> Vec<usize> {
    for unit in units {
        unit.typer(store, defs, packages)
            .complete_symbol(unit.class)
            .unwrap();
    }
    let mut edges = HashMap::new();
    let mut remaining: HashSet<_> = units.iter().map(NamedUnit::method_symbol).collect();
    let mut visited = HashSet::new();
    for level in 0..4 {
        for unit in units.iter().skip(level).step_by(4) {
            let method = unit.method_symbol();
            visited.insert(method);
            match unit
                .typer(store, defs, packages)
                .try_complete_signature(method, |symbol| remaining.contains(&symbol))
            {
                Ok(SignatureCompletion::Complete(_)) => assert_eq!(level, 3),
                Ok(SignatureCompletion::BlockedOn(symbol)) => {
                    assert!(level < 3 && remaining.contains(&symbol) && !visited.contains(&symbol));
                    edges.insert(method, symbol);
                }
                Err(error) => panic!("unexpected dependency probe failure: {error:?}"),
            }
        }
    }
    let mut widths = Vec::new();
    while !remaining.is_empty() {
        let ready: Vec<_> = remaining
            .iter()
            .copied()
            .filter(|m| {
                edges
                    .get(m)
                    .is_none_or(|target| !remaining.contains(target))
            })
            .collect();
        assert!(!ready.is_empty(), "cycle in the measured dependency graph");
        widths.push(ready.len());
        for method in ready {
            remaining.remove(&method);
        }
    }
    widths
}

fn check_counts(groups: usize, layout: Layout) {
    let sources = dependent_sources(groups, layout);
    let files = inputs(&sources);
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        &files,
        parsing::parse_files(&files, 4),
        &mut store,
        &mut packages,
    );
    let widths = ready_widths(&units, &mut store, defs, &packages);
    assert_eq!(widths, [groups; 4]);
    for order in [Order::Callers, Order::Callees] {
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 4),
            &mut store,
            &mut packages,
        );
        let mut seen = HashSet::new();
        let mut targets = HashSet::new();
        let (mut calls, mut blocked, mut retries) = (0, 0, 0);
        complete_signatures_in_order(
            &units,
            order.roots(groups).iter().map(|&i| &units[i]),
            &mut store,
            defs,
            &packages,
            |symbol, outcome| {
                calls += 1;
                if !seen.insert(symbol) {
                    retries += 1;
                }
                if let SignatureCompletion::BlockedOn(symbol) = outcome {
                    blocked += 1;
                    targets.insert(symbol);
                }
            },
        )
        .unwrap();
        let expected = match (order, layout) {
            (Order::Callers, Layout::Chains) => 3 * groups,
            (Order::Callers, Layout::Shared) => 3,
            (Order::Callees, _) => 0,
        };
        assert_eq!(
            (blocked, retries, targets.len()),
            (expected, expected, expected)
        );
        assert_eq!(seen.len(), 2 * files.len()); // One class and one method per source.
        assert_eq!(calls, 2 * files.len() + retries);
        for unit in &units {
            let SymbolInfo::Complete(ty) = store.symbols.info(unit.method_symbol()) else {
                panic!("incomplete method")
            };
            let Type::Method(signature) = store.types.get(*ty) else {
                panic!("expected method signature")
            };
            assert_eq!(
                signature.result, defs.int,
                "inferred result for {:?}",
                unit.source
            );
        }
        eprintln!(
            "counts,{layout:?},{},{order:?},{calls},{blocked},{retries},{},{widths:?}",
            files.len(),
            targets.len()
        );
    }
}

#[test]
fn signature_order_counts_and_types() {
    for layout in [Layout::Chains, Layout::Shared] {
        check_counts(2, layout);
    }
}

#[test]
#[ignore = "collect signature counts and readiness separately from latency"]
fn signature_order_probe() {
    for groups in [4, 16, 64] {
        for layout in [Layout::Chains, Layout::Shared] {
            check_counts(groups, layout);
        }
    }
}

#[test]
#[ignore = "paired release-mode signature scheduling experiment"]
fn measure_signature_order() {
    let cases: Vec<_> = [4, 16, 64]
        .into_iter()
        .flat_map(|groups| {
            [Layout::Chains, Layout::Shared].map(|layout| {
                let sources = dependent_sources(groups, layout);
                let files = inputs(&sources);
                let orders =
                    [Order::Callers, Order::Callees].map(|order| (order, order.roots(groups)));
                let reference = compile_timed(&files, 1).0;
                for (_, roots) in &orders {
                    for workers in [1, 4] {
                        assert_eq!(compile_order(&files, workers, roots).0, reference);
                    }
                }
                (layout, sources, orders, reference)
            })
        })
        .collect();
    eprintln!("layout,files,round,workers,order,signature_ns,total_ns");
    for round in 0..12 {
        for offset in 0..cases.len() {
            let case = (round + offset) % cases.len();
            let (layout, sources, orders, reference) = &cases[case];
            let files = inputs(sources);
            for workers in if round % 2 == 0 { [1, 4] } else { [4, 1] } {
                for index in if (round + case + usize::from(workers == 4)).is_multiple_of(2) {
                    [0, 1]
                } else {
                    [1, 0]
                } {
                    let (order, roots) = &orders[index];
                    let (classes, times) = compile_order(&files, workers, roots);
                    assert_eq!(&classes, reference);
                    eprintln!(
                        "{layout:?},{},{round},{workers},{order:?},{},{}",
                        files.len(),
                        times.signatures.as_nanos(),
                        times.total.as_nanos()
                    );
                }
            }
        }
    }
}
