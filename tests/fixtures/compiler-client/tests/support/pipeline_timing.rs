//! In-memory compilation from source text through the complete batch of class files.
use std::time::{Duration, Instant};

use super::{class_plans, complete_signatures, emit_batch, name_units, parsing, type_bodies};
use dotty::core::{Definitions, Packages, SemanticStore, SourceId};
use dotty::jvm::{GeneratedClass, JvmCompilationUnit};

fn dependent_sources(groups: usize) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for group in 0..groups {
        let leaf = format!("Leaf{group}");
        let first = format!("First{group}");
        let second = format!("Second{group}");
        let last = format!("Last{group}");
        // Submit consumers first so signature inference must follow each dependency.
        for (class, method) in [
            (
                last,
                format!("run(s: {second}, f: {first}, l: {leaf}) = s.run(f, l)"),
            ),
            (second, format!("run(f: {first}, l: {leaf}) = f.run(l)")),
            (first, format!("run(l: {leaf}) = l.run()")),
            (leaf, format!("run() = {{ val n = {group}; n + 1 }}")),
        ] {
            files.push((
                format!("{class}.scala"),
                format!("package batch\nclass {class}:\n  def {method}\n"),
            ));
        }
    }
    files
}

fn inputs(sources: &[(String, String)]) -> Vec<(SourceId, &str, &str)> {
    sources
        .iter()
        .enumerate()
        .map(|(i, (name, text))| (SourceId::from_index(i as u32), name.as_str(), text.as_str()))
        .collect()
}

struct Timings {
    setup: Duration,
    parse: Duration,
    bind_and_name: Duration,
    signatures: Duration,
    bodies: Duration,
    class_plans: Duration,
    prepare: Duration,
    emit: Duration,
    total: Duration,
}

fn compile_timed(
    files: &[(SourceId, &str, &str)],
    workers: usize,
) -> (Vec<GeneratedClass>, Timings) {
    let start = Instant::now();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let ready = Instant::now();
    let parsed = parsing::parse_files(files, workers);
    let parsed_at = Instant::now();
    let units = name_units(files, parsed, &mut store, &mut packages);
    let named = Instant::now();
    complete_signatures(&units, &mut store, definitions, &packages).unwrap();
    let completed = Instant::now();
    let (arena, typed) = type_bodies(&units, &mut store, definitions, &packages);
    let typed_at = Instant::now();
    let plan = JvmCompilationUnit {
        classes: class_plans(&units, typed, &arena, &store),
        store: &store,
        typed_ast: &arena,
    };
    let planned_at = Instant::now();
    let prepared = plan.prepare_emission(&definitions).unwrap();
    let prepared_at = Instant::now();
    let artifacts = emit_batch(&prepared, &plan.classes, workers);
    let end = Instant::now();
    (
        artifacts,
        Timings {
            setup: ready - start,
            parse: parsed_at - ready,
            bind_and_name: named - parsed_at,
            signatures: completed - named,
            bodies: typed_at - completed,
            class_plans: planned_at - typed_at,
            prepare: prepared_at - planned_at,
            emit: end - prepared_at,
            total: end - start,
        },
    )
}

#[test]
fn dependent_chain_emits_all_four_classes_with_one_and_four_workers() {
    let sources = dependent_sources(1);
    let files = inputs(&sources);
    let (serial, _) = compile_timed(&files, 1);
    assert!(serial.iter().map(|c| c.internal_name()).eq([
        "batch/Last0",
        "batch/Second0",
        "batch/First0",
        "batch/Leaf0"
    ]));
    assert_eq!(serial, compile_timed(&files, 4).0);
}

#[test]
#[ignore = "release-mode end-to-end compilation measurement"]
fn measure_dependent_pipeline() {
    eprintln!(
        "files,sample,workers,setup_us,parse_us,bind_and_name_us,signatures_us,bodies_us,class_plans_us,prepare_us,emit_us,total_us"
    );
    let cases = [4, 16, 64].map(|groups| {
        let sources = dependent_sources(groups);
        let files = inputs(&sources);
        let reference = compile_timed(&files, 1).0;
        assert_eq!(reference.len(), files.len());
        assert_eq!(reference, compile_timed(&files, 4).0);
        (sources, reference)
    });
    for sample in 0..9 {
        // Give every size an equal number of first, middle and last slots.
        for offset in 0..cases.len() {
            let (sources, reference) = &cases[(sample + offset) % cases.len()];
            let files = inputs(sources);
            let count = files.len();
            let order = if sample % 2 == 0 { [1, 4] } else { [4, 1] };
            for workers in order {
                let (artifacts, t) = compile_timed(&files, workers);
                assert_eq!(&artifacts, reference);
                eprintln!(
                    "{count},{sample},{workers},{},{},{},{},{},{},{},{},{}",
                    t.setup.as_micros(),
                    t.parse.as_micros(),
                    t.bind_and_name.as_micros(),
                    t.signatures.as_micros(),
                    t.bodies.as_micros(),
                    t.class_plans.as_micros(),
                    t.prepare.as_micros(),
                    t.emit.as_micros(),
                    t.total.as_micros()
                );
            }
        }
    }
}
