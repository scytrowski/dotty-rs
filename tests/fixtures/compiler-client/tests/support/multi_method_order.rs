//! Four independent method chains per group, sharing the same four class files.
use super::{LearnedOrder, compile_timed, inputs, measure_cases, probe};
use dotty::classfile::{ClassFile, Reader};
use std::fmt::Write;

#[path = "method_history.rs"]
mod method_history;

#[derive(Clone, Copy, Debug)]
enum Graph {
    Chains,
    Shared,
    Crossed,
}

const CASES: [(Graph, Graph); 3] = [
    (Graph::Chains, Graph::Shared),
    (Graph::Shared, Graph::Chains),
    (Graph::Crossed, Graph::Chains),
];

fn sources(groups: usize, graph: Graph) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for group in 0..groups {
        let target = if matches!(graph, Graph::Shared) {
            groups / 2
        } else {
            group
        };
        for file in 0..4 {
            let class = format!("C{group}_{file}");
            let mut text = format!("package batch\nclass {class}:\n");
            for lane in 0..4 {
                let reverse = matches!(graph, Graph::Crossed) && lane % 2 == 1;
                let physical = |level: usize| if reverse { 3 - level } else { level };
                let level = physical(file);
                let params: Vec<_> = (level + 1..4)
                    .map(|k| format!("p{k}: C{target}_{}", physical(k)))
                    .collect();
                let body = if level == 3 {
                    format!("{{ val n = {group}; n + {lane} }}")
                } else {
                    let args: Vec<_> = (level + 2..4).map(|k| format!("p{k}")).collect();
                    format!("p{}.run{lane}({})", level + 1, args.join(", "))
                };
                writeln!(text, "  def run{lane}({}) = {body}", params.join(", ")).unwrap();
            }
            files.push((format!("{class}.scala"), text));
        }
    }
    files
}

fn counts(groups: usize, graph: Graph, previous: Graph) -> [usize; 4] {
    let sources_now = sources(groups, graph);
    let files = inputs(&sources_now);
    let mut history = LearnedOrder::default();
    let cold = probe(&files, &mut history);
    assert_eq!(history.0.len(), files.len());
    let replay = probe(&files, &mut history);
    let mut edited = LearnedOrder::default();
    probe(&inputs(&sources(groups, previous)), &mut edited);
    let stale = probe(&files, &mut edited);
    let next = probe(&files, &mut edited);
    [cold, replay, stale, next]
}

#[test]
fn file_schedule_keeps_all_methods_in_declaration_order() {
    for (graph, _) in CASES {
        let sources = sources(2, graph);
        let files = inputs(&sources);
        let reference = compile_timed(&files, 1).0;
        assert_eq!(reference.len(), files.len());
        let mut history = LearnedOrder::default();
        assert_eq!(history.compile(&files, 4).0, reference);
        assert_eq!(history.compile(&files, 4).0, reference);
        for artifact in reference {
            let class = ClassFile::decode(&mut Reader::new(artifact.bytes())).unwrap();
            let names: Vec<_> = class
                .methods
                .iter()
                .map(|m| class.constant_pool.utf8(m.name_index).unwrap())
                .collect();
            assert_eq!(names, ["<init>", "run0", "run1", "run2", "run3"]);
        }
    }
}

#[test]
#[ignore = "count multi-method signature retries outside the timing runs"]
fn probe_multi_method_order() {
    for groups in [4, 16, 64] {
        for (graph, previous) in CASES {
            let [cold, replay, stale, next] = counts(groups, graph, previous);
            eprintln!(
                "counts,{graph:?},{},{cold},{replay},{stale},{next}",
                groups * 4
            );
        }
    }
}

#[test]
#[ignore = "paired latency of file order reuse with four methods per file"]
fn measure_multi_method_order() {
    measure_cases(&CASES, sources);
}

// Crossed dependencies can change the file hint even on identical inputs.
#[test]
fn repeat_builds_reach_a_stable_file_hint() {
    for (graph, expected) in [
        (Graph::Chains, [24, 0, 0, 0]),
        (Graph::Shared, [12, 0, 0, 0]),
        (Graph::Crossed, [12, 12, 20, 20]),
    ] {
        let sources = sources(2, graph);
        let files = inputs(&sources);
        let mut history = LearnedOrder::default();
        for expected in &expected[..2] {
            assert_eq!(probe(&files, &mut history), *expected);
        }
        let stable = history.clone();
        for expected in &expected[2..] {
            assert_eq!(probe(&files, &mut history), *expected);
            assert_eq!(history, stable);
        }
    }
}

#[test]
#[ignore = "paired stable crossed-graph comparison at 256 files and four workers"]
fn measure_stable_crossed_order() {
    let sources = sources(64, Graph::Crossed);
    let files = inputs(&sources);
    let reference = compile_timed(&files, 1).0;
    let mut stable = LearnedOrder::default();
    assert_eq!(probe(&files, &mut stable), 384);
    assert_eq!(probe(&files, &mut stable), 384);
    let mut next = stable.clone();
    assert_eq!(probe(&files, &mut next), 640);
    assert_eq!(next, stable);
    eprintln!("layout,files,round,workers,mode,signature_ns,total_ns");
    for round in 0..=12 {
        for replay in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let (mode, (classes, times)) = if replay {
                ("Stable", stable.clone().compile(&files, 4))
            } else {
                ("Baseline", compile_timed(&files, 4))
            };
            assert_eq!(classes, reference);
            if round > 0 {
                eprintln!(
                    "Crossed,256,{round},4,{mode},{},{}",
                    times.signatures.as_nanos(),
                    times.total.as_nanos()
                );
            }
        }
    }
}
