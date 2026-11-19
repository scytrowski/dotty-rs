//! Compare three caller-owned schedules over consecutive, fresh semantic sessions.
use super::super::{
    Input, LearnedOrder, NamedUnit, SignatureCompletion, Timings, TyperError, compile_timed,
    compile_with_signatures, inputs, name_units, parsing,
};
use super::{Graph, sources};
use crate::{complete_methods_in_order, complete_signatures_in_order};
use dotty::core::{Definitions, Packages, SemanticStore, SymbolInfo, Type};
use dotty::jvm::GeneratedClass;
use std::collections::HashMap;

#[derive(Default, Debug, PartialEq, Clone)]
struct MethodHistory {
    // Paths are unique logical source keys supplied by this client.
    sources: Vec<(String, String)>,
    // Index into the private snapshot above and ordinal in that file's source declarations.
    order: Vec<(usize, usize)>,
}

impl MethodHistory {
    fn complete(
        &mut self,
        files: &[Input<'_>],
        units: &[NamedUnit],
        store: &mut SemanticStore,
        defs: Definitions,
        packages: &Packages,
        mut blocked: impl FnMut(),
    ) -> Result<crate::TypedHeaders, TyperError> {
        let by_symbol: HashMap<_, _> = units
            .iter()
            .enumerate()
            .flat_map(|(file, unit)| {
                unit.method_symbols()
                    .enumerate()
                    .map(move |(ordinal, symbol)| (symbol, (file, ordinal)))
            })
            .collect();
        let mut next_order = Vec::with_capacity(by_symbol.len());
        let mut preferred = Vec::with_capacity(self.order.len());
        if !self.order.is_empty() {
            let current: HashMap<_, _> = files
                .iter()
                .zip(units)
                .map(|(&(_, path, text), unit)| (path, (text, unit)))
                .collect();
            let unchanged: Vec<_> = self
                .sources
                .iter()
                .map(|(path, old)| {
                    current
                        .get(path.as_str())
                        .filter(|(text, _)| *text == old)
                        .map(|&(_, unit)| unit)
                })
                .collect();
            for &(file, ordinal) in &self.order {
                if let Some(unit) = unchanged[file] {
                    preferred.push(unit.symbol(unit.methods[ordinal]));
                }
            }
        }
        // Always visit current roots as well. New or changed methods can still
        // be reached earlier by the common scheduler's dependency fallback.
        let roots = preferred
            .into_iter()
            .chain(units.iter().flat_map(NamedUnit::method_symbols));
        let headers =
            complete_methods_in_order(units, roots, store, defs, packages, |symbol, outcome| {
                match outcome {
                    SignatureCompletion::BlockedOn(_) => blocked(),
                    SignatureCompletion::Complete(_) => {
                        if let Some(&key) = by_symbol.get(&symbol) {
                            next_order.push(key);
                        }
                    }
                }
            })?;
        self.sources = files
            .iter()
            .map(|&(_, path, text)| (path.to_owned(), text.to_owned()))
            .collect();
        self.order = next_order;
        Ok(headers)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Strategy {
    Ordinary,
    File(LearnedOrder),
    Method(MethodHistory),
}

impl Strategy {
    fn all() -> [Self; 3] {
        [
            Self::Ordinary,
            Self::File(LearnedOrder::default()),
            Self::Method(MethodHistory::default()),
        ]
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Ordinary => "Ordinary",
            Self::File(_) => "File",
            Self::Method(_) => "Method",
        }
    }
    fn complete(
        &mut self,
        files: &[Input<'_>],
        units: &[NamedUnit],
        store: &mut SemanticStore,
        defs: Definitions,
        packages: &Packages,
        mut blocked: impl FnMut(),
    ) -> Result<crate::TypedHeaders, TyperError> {
        match self {
            Self::Ordinary => {
                complete_signatures_in_order(units, units, store, defs, packages, |_, outcome| {
                    if matches!(outcome, SignatureCompletion::BlockedOn(_)) {
                        blocked();
                    }
                })
            }
            Self::File(history) => history.complete(files, units, store, defs, packages, blocked),
            Self::Method(history) => history.complete(files, units, store, defs, packages, blocked),
        }
    }
    fn compile(&mut self, files: &[Input<'_>], workers: usize) -> (Vec<GeneratedClass>, Timings) {
        compile_with_signatures(files, workers, |units, store, defs, packages| {
            self.complete(files, units, store, defs, packages, || {})
        })
    }
    fn probe(&mut self, files: &[Input<'_>]) -> usize {
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
        self.complete(files, &units, &mut store, defs, &packages, || blocked += 1)
            .unwrap();
        for method in units.iter().flat_map(NamedUnit::method_symbols) {
            let SymbolInfo::Complete(ty) = store.symbols.info(method) else {
                panic!("missing signature")
            };
            let Type::Method(sig) = store.types.get(*ty) else {
                panic!("expected method type")
            };
            assert_eq!(sig.result, defs.int);
        }
        blocked
    }
}

fn edited(original: &[(String, String)]) -> Vec<(String, String)> {
    let mut changed = original.to_vec();
    // Change one consumer's dependency to a newly added overload in another file.
    let last = changed.len() - 1;
    let group = last / 4;
    let mut methods: Vec<_> = original[0].1.lines().skip(2).map(str::to_owned).collect();
    methods[0] = format!("  def run0(x: C{group}_3) = x.run0(7)");
    methods.reverse();
    changed[0].1 = format!("package batch\nclass C0_0:\n{}\n", methods.join("\n"));
    changed[last].1.push_str("  def run0(x: Int) = x + 1\n");
    changed
}

const STAGES: [&str; 5] = ["Cold", "Replay", "Stable", "Edited", "Repaired"];
const GRAPHS: [Graph; 3] = [Graph::Chains, Graph::Shared, Graph::Crossed];

#[test]
fn method_hints_follow_current_declarations_across_edits() {
    use dotty::classfile::{Attribute, ClassFile, ConstantPoolEntry, ConstantPoolIndex, Reader};
    let original = sources(2, Graph::Crossed);
    let mut changed = edited(&original);
    changed.reverse(); // Reassign both file positions and source IDs.
    let before = inputs(&original);
    let after = inputs(&changed);
    let reference = compile_timed(&after, 1).0;
    let class = ClassFile::decode(&mut Reader::new(reference[0].bytes())).unwrap();
    let overloads: Vec<_> = class
        .methods
        .iter()
        .filter(|m| class.constant_pool.utf8(m.name_index) == Ok("run0"))
        .map(|m| class.constant_pool.utf8(m.descriptor_index).unwrap())
        .collect();
    assert_eq!(overloads, ["()I", "(I)I"]);
    let caller = reference
        .iter()
        .find(|c| c.internal_name() == "batch/C0_0")
        .unwrap();
    let class = ClassFile::decode(&mut Reader::new(caller.bytes())).unwrap();
    let pool = &class.constant_pool;
    let run = class
        .methods
        .iter()
        .find(|m| pool.utf8(m.name_index) == Ok("run0"))
        .unwrap();
    let [Attribute::Code(code)] = run.attributes.as_slice() else {
        panic!("missing caller code")
    };
    let [.., 0xb6, hi, lo, 0xac] = code.code else {
        panic!("expected invokevirtual and ireturn")
    };
    let index = ConstantPoolIndex(u16::from_be_bytes([*hi, *lo]));
    let Some(ConstantPoolEntry::Methodref {
        class_index,
        name_and_type_index,
    }) = pool.get(index)
    else {
        panic!("expected method reference")
    };
    assert_eq!(pool.class_name(*class_index), Ok("batch/C1_3"));
    assert_eq!(
        pool.name_and_type(*name_and_type_index),
        Ok(("run0", "(I)I"))
    );
    for mut strategy in Strategy::all() {
        assert_eq!(strategy.probe(&before), 12);
        let expected = if matches!(strategy, Strategy::Method(_)) {
            0
        } else {
            12
        };
        assert_eq!(strategy.probe(&before), expected);
        assert_eq!(strategy.compile(&after, 4).0, reference);
        let retries = strategy.probe(&after);
        if matches!(strategy, Strategy::Method(_)) {
            assert_eq!(retries, 0);
        }
    }
}

#[test]
fn introduced_cycle_reports_reentered_method_and_preserves_each_history() {
    use dotty::core::SourceId;
    let a = (
        SourceId::from_index(4),
        "A.scala",
        "class A { def left(b: B) = b.right(this) }",
    );
    let b = (
        SourceId::from_index(2),
        "B.scala",
        "class B { def right(a: A) = 1 }",
    );
    let files = [a, (b.0, b.1, "class B { def right(a: A) = a.left(this) }")];
    for mut strategy in Strategy::all() {
        strategy.probe(&[a, b]);
        let saved = strategy.clone();
        // The changed B file loses its method hint. Its file hint still leads.
        let expected = usize::from(matches!(strategy, Strategy::File(_)));
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 1),
            &mut store,
            &mut packages,
        );
        let err = strategy
            .complete(&files, &units, &mut store, defs, &packages, || {})
            .unwrap_err();
        assert!(
            matches!(err, TyperError::RecursiveInferredMethodResult { symbol } if symbol == units[expected].method_symbol()),
            "{err:?}"
        );
        assert!(
            units
                .iter()
                .flat_map(NamedUnit::method_symbols)
                .all(|m| matches!(store.symbols.info(m), SymbolInfo::Missing))
        );
        assert_eq!(strategy, saved);
    }
}

#[test]
#[ignore = "signature retry counts outside the latency measurement"]
fn probe_method_history() {
    for groups in [4, 16, 64] {
        for graph in GRAPHS {
            let sources = sources(groups, graph);
            let changed = edited(&sources);
            for mut strategy in Strategy::all() {
                for (i, stage) in STAGES.iter().enumerate() {
                    let retries = strategy.probe(&inputs(if i < 3 { &sources } else { &changed }));
                    eprintln!(
                        "counts,{graph:?},{},{},{stage},{retries}",
                        groups * 4,
                        strategy.name()
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "paired compilation times for ordinary, file and method schedules"]
fn measure_method_history() {
    let mut cases = Vec::new();
    for groups in [4, 16, 64] {
        for graph in GRAPHS {
            let original = sources(groups, graph);
            let edited = edited(&original);
            let batches = [original, edited].map(|sources| {
                let reference = compile_timed(&inputs(&sources), 1).0;
                (sources, reference)
            });
            cases.push((graph, batches));
        }
    }
    eprintln!("graph,files,round,workers,strategy,stage,signature_ns,total_ns");
    for round in 0..=12 {
        for offset in 0..cases.len() {
            let case = (round + offset) % cases.len();
            let (graph, batches) = &cases[case];
            for workers in if round % 2 == 0 { [1, 4] } else { [4, 1] } {
                let mut strategies = Strategy::all();
                for (i, stage) in STAGES.iter().enumerate() {
                    let (sources, reference) = &batches[usize::from(i >= 3)];
                    let files = inputs(sources);
                    // All six permutations per six rounds at each stage and configuration.
                    let n = round + case + i + workers;
                    let order = if (n / 3) % 2 == 0 {
                        [0, 1, 2]
                    } else {
                        [2, 1, 0]
                    };
                    for slot in order {
                        let strategy = &mut strategies[(n + slot) % 3];
                        let (classes, time) = strategy.compile(&files, workers);
                        assert_eq!(&classes, reference);
                        if round > 0 {
                            eprintln!(
                                "{graph:?},{},{round},{workers},{},{stage},{},{}",
                                files.len(),
                                strategy.name(),
                                time.signatures.as_nanos(),
                                time.total.as_nanos()
                            );
                        }
                    }
                }
            }
        }
    }
}
