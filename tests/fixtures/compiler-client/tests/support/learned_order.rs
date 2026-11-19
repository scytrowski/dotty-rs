//! Replay the previous session's completion order as a file scheduling hint.
use super::super::{NamedUnit, TyperError, complete_signatures_in_order};
use super::{
    Layout, Timings, compile_timed, compile_with_signatures, dependent_sources, inputs, name_units,
    parsing,
};
use dotty::core::{Definitions, Packages, SemanticStore, SourceId, SymbolInfo, Type};
use dotty::jvm::GeneratedClass;
use std::collections::HashMap;

type Input<'a> = (SourceId, &'a str, &'a str);

// One method per fixture file. Only caller-owned file keys survive a session.
#[derive(Clone, Default, Debug, PartialEq)]
struct LearnedOrder(Vec<String>);

impl LearnedOrder {
    fn complete(
        &mut self,
        files: &[Input<'_>],
        units: &[NamedUnit],
        store: &mut SemanticStore,
        defs: Definitions,
        packages: &Packages,
        mut on_blocked: impl FnMut(),
    ) -> Result<(), TyperError> {
        let by_method: HashMap<_, _> = units
            .iter()
            .zip(files)
            .map(|(unit, &(_, path, _))| (unit.method_symbol(), path))
            .collect();
        let mut next = Vec::with_capacity(units.len());
        let observe = |symbol, error: Option<&TyperError>| match error {
            Some(TyperError::MemberTypeUnavailable { .. }) => on_blocked(),
            None => {
                if let Some(&path) = by_method.get(&symbol) {
                    next.push(path.to_owned());
                }
            }
            _ => {}
        };

        if self.0.is_empty() {
            complete_signatures_in_order(units, units, store, defs, packages, observe)?;
        } else {
            let mut by_path: HashMap<_, _> = files
                .iter()
                .zip(units)
                .map(|(&(_, path, _), unit)| (path, unit))
                .collect();
            let mut roots = Vec::with_capacity(units.len());
            for path in &self.0 {
                if let Some(unit) = by_path.remove(path.as_str()) {
                    roots.push(unit);
                }
            }
            // Deleted files have no entry. New files follow in current input order.
            for &(_, path, _) in files {
                if let Some(unit) = by_path.remove(path) {
                    roots.push(unit);
                }
            }
            complete_signatures_in_order(units, roots, store, defs, packages, observe)?;
        }
        self.0 = next;
        Ok(())
    }

    fn compile(&mut self, files: &[Input<'_>], workers: usize) -> (Vec<GeneratedClass>, Timings) {
        compile_with_signatures(files, workers, |units, store, defs, packages| {
            self.complete(files, units, store, defs, packages, || {})
        })
    }
}

fn probe(files: &[Input<'_>], history: &mut LearnedOrder) -> usize {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        files,
        parsing::parse_files(files, 4),
        &mut store,
        &mut packages,
    );
    let mut blocked = 0;
    history
        .complete(files, &units, &mut store, defs, &packages, || blocked += 1)
        .unwrap();
    for unit in units {
        let SymbolInfo::Complete(ty) = store.symbols.info(unit.method_symbol()) else {
            panic!("incomplete method")
        };
        let Type::Method(signature) = store.types.get(*ty) else {
            panic!("expected method")
        };
        assert_eq!(signature.result, defs.int);
    }
    blocked
}

#[test]
fn history_survives_reordered_replaced_and_new_files() {
    let original = dependent_sources(2, Layout::Chains);
    let mut history = LearnedOrder::default();
    assert_eq!(probe(&inputs(&original), &mut history), 6);
    // Reverse registration, reassign all source IDs, remove the whole second
    // group, and add a method that introduces a previously unseen dependency.
    let mut changed = original[..4].to_vec();
    changed.reverse();
    changed.push((
        "Extra.scala".into(),
        "package batch\nclass Extra:\n  def run() = 7\n".into(),
    ));
    // Last has no callers, so its parameters can change without editing First or Second.
    changed[3].1 = "package batch\nclass Last0:\n  def run(x: Extra) = x.run()\n".into();
    let files = inputs(&changed);
    let reference = compile_timed(&files, 1).0;
    let mut check = history.clone();
    assert_eq!(probe(&files, &mut check), 1);
    assert_eq!(probe(&files, &mut check), 0);
    assert_eq!(history.compile(&files, 4).0, reference);
    assert_eq!(history, check);
    assert_eq!(history.0.len(), files.len());
}

#[test]
fn failed_signature_pass_preserves_history_and_cycle_diagnostic() {
    let original = [
        (
            SourceId::from_index(4),
            "A.scala",
            "class A { def left(b: B) = b.right(this) }",
        ),
        (
            SourceId::from_index(9),
            "B.scala",
            "class B { def right(a: A) = 1 }",
        ),
    ];
    let mut history = LearnedOrder::default();
    assert_eq!(probe(&original, &mut history), 1);
    let saved = history.clone();
    let files = [
        original[0],
        (
            original[1].0,
            "B.scala",
            "class B { def right(a: A) = a.left(this) }",
        ),
    ];
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        &files,
        parsing::parse_files(&files, 4),
        &mut store,
        &mut packages,
    );
    let error = history
        .complete(&files, &units, &mut store, defs, &packages, || {})
        .unwrap_err();
    assert!(
        matches!(error, TyperError::RecursiveInferredMethodResult {symbol} if symbol == units[1].method_symbol()),
        "{error:?}"
    );
    assert!(
        units
            .iter()
            .all(|u| matches!(store.symbols.info(u.method_symbol()), SymbolInfo::Missing))
    );
    assert_eq!(history, saved);
}

#[test]
#[ignore = "operation counts for learned and edited signature orders"]
fn probe_learned_order() {
    for groups in [4, 16, 64] {
        for (layout, previous) in [
            (Layout::Chains, Layout::Shared),
            (Layout::Shared, Layout::Chains),
        ] {
            let sources = dependent_sources(groups, layout);
            let files = inputs(&sources);
            let mut history = LearnedOrder::default();
            let cold = probe(&files, &mut history);
            let warm = probe(&files, &mut history);
            let mut edited = LearnedOrder::default();
            probe(&inputs(&dependent_sources(groups, previous)), &mut edited);
            let stale = probe(&files, &mut edited);
            let repaired = probe(&files, &mut edited);
            assert_eq!((warm, repaired), (0, 0));
            eprintln!(
                "counts,{layout:?},{},{cold},{warm},{stale},{repaired}",
                files.len()
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Baseline,
    Learn,
    Replay,
    Edited,
}

#[test]
#[ignore = "paired latency of learning, replaying and repairing signature order"]
fn measure_learned_order() {
    let cases: Vec<_> = [4, 16, 64]
        .into_iter()
        .flat_map(|groups| {
            let layouts = [Layout::Chains, Layout::Shared].map(|layout| {
                let sources = dependent_sources(groups, layout);
                let files = inputs(&sources);
                let reference = compile_timed(&files, 1).0;
                let mut history = LearnedOrder::default();
                assert_eq!(history.compile(&files, 4).0, reference);
                (layout, sources, reference, history)
            });
            let edited = [layouts[1].3.clone(), layouts[0].3.clone()];
            layouts.into_iter().zip(edited)
        })
        .collect();
    let modes = [Mode::Baseline, Mode::Learn, Mode::Replay, Mode::Edited];
    eprintln!("layout,files,round,workers,mode,signature_ns,total_ns");
    // Round zero warms every case. Balance position and preceding treatment
    // within each four measured rounds using a Williams design.
    for round in 0..=12 {
        for offset in 0..cases.len() {
            let case = (round + offset) % cases.len();
            let ((layout, sources, reference, learned), edited) = &cases[case];
            let files = inputs(sources);
            for workers in if round % 2 == 0 { [1, 4] } else { [4, 1] } {
                let shift = (round + case + usize::from(workers == 4)) % 4;
                for slot in [0, 1, 3, 2] {
                    let mode = modes[(slot + shift) % 4];
                    // Supply an independent snapshot of caller state to each sample.
                    let (classes, times) = match mode {
                        Mode::Baseline => compile_timed(&files, workers),
                        Mode::Learn => LearnedOrder::default().compile(&files, workers),
                        Mode::Replay => learned.clone().compile(&files, workers),
                        Mode::Edited => edited.clone().compile(&files, workers),
                    };
                    assert_eq!(&classes, reference);
                    if round > 0 {
                        eprintln!(
                            "{layout:?},{},{round},{workers},{mode:?},{},{}",
                            files.len(),
                            times.signatures.as_nanos(),
                            times.total.as_nanos()
                        );
                    }
                }
            }
        }
    }
}
