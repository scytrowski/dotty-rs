//! In-memory compilation from source text through the complete batch of class files.
#[path = "learned_order.rs"]
mod learned_order;
#[path = "signature_order.rs"]
mod signature_order;

use std::time::{Duration, Instant};

use super::{class_plans, complete_signatures, emit_batch, name_units, parsing, type_bodies};
use dotty::core::{Definitions, Packages, SemanticStore, SourceId};
use dotty::jvm::{GeneratedClass, JvmCompilationUnit};

#[derive(Clone, Copy, Debug)]
enum Layout {
    Chains,
    Shared,
}

fn dependent_sources(groups: usize, layout: Layout) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for group in 0..groups {
        let target = match layout {
            Layout::Chains => group,
            Layout::Shared => groups / 2,
        };
        let leaf = format!("Leaf{target}");
        let first = format!("First{target}");
        let second = format!("Second{target}");
        // Submit consumers first so signature inference must follow each dependency.
        for (class, method) in [
            (
                format!("Last{group}"),
                format!("run(s: {second}, f: {first}, l: {leaf}) = s.run(f, l)"),
            ),
            (
                format!("Second{group}"),
                format!("run(f: {first}, l: {leaf}) = f.run(l)"),
            ),
            (format!("First{group}"), format!("run(l: {leaf}) = l.run()")),
            (
                format!("Leaf{group}"),
                format!("run() = {{ val n = {group}; n + 1 }}"),
            ),
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
    emit_workers: Duration,
    total: Duration,
}

fn compile_timed(
    files: &[(SourceId, &str, &str)],
    workers: usize,
) -> (Vec<GeneratedClass>, Timings) {
    compile_with_signatures(files, workers, complete_signatures)
}

fn compile_with_signatures(
    files: &[(SourceId, &str, &str)],
    workers: usize,
    complete: impl FnOnce(
        &[super::NamedUnit],
        &mut SemanticStore,
        Definitions,
        &Packages,
    ) -> Result<(), super::TyperError>,
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
    complete(&units, &mut store, definitions, &packages).unwrap();
    let completed = Instant::now();
    let (arena, typed) = type_bodies(&units, &mut store, definitions, &packages);
    let typed_at = Instant::now();
    let plan = JvmCompilationUnit {
        classes: class_plans(&units, typed, &arena, &store).unwrap(),
        store: &store,
        typed_ast: &arena,
    };
    let planned_at = Instant::now();
    let prepared = plan.prepare_emission(&definitions).unwrap();
    let prepared_at = Instant::now();
    let (artifacts, emit_workers) = emit_batch(&prepared, &plan.classes, workers);
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
            emit_workers,
            total: end - start,
        },
    )
}

#[test]
fn dependent_chain_emits_all_four_classes_with_one_and_four_workers() {
    let sources = dependent_sources(1, Layout::Chains);
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
fn callers_in_different_groups_invoke_the_same_three_targets() {
    use dotty::classfile::{Attribute, ClassFile, ConstantPoolEntry, ConstantPoolIndex, Reader};

    let sources = dependent_sources(2, Layout::Shared);
    let files = inputs(&sources);
    let (classes, _) = compile_timed(&files, 1);
    assert_eq!(classes, compile_timed(&files, 4).0);
    for (level, owner, descriptor) in [
        ("First", "batch/Leaf1", "()I"),
        ("Second", "batch/First1", "(Lbatch/Leaf1;)I"),
        ("Last", "batch/Second1", "(Lbatch/First1;Lbatch/Leaf1;)I"),
    ] {
        for group in 0..2 {
            let caller = format!("batch/{level}{group}");
            let artifact = classes
                .iter()
                .find(|c| c.internal_name() == caller)
                .unwrap();
            let class = ClassFile::decode(&mut Reader::new(artifact.bytes())).unwrap();
            let pool = &class.constant_pool;
            let run = class
                .methods
                .iter()
                .find(|m| pool.utf8(m.name_index) == Ok("run"))
                .unwrap();
            let [Attribute::Code(code)] = run.attributes.as_slice() else {
                panic!("expected Code")
            };
            let [0x2b, .., 0xb6, hi, lo, 0xac] = code.code else {
                panic!("expected parameter receiver, invokevirtual and ireturn")
            };
            let index = ConstantPoolIndex(u16::from_be_bytes([*hi, *lo]));
            let Some(ConstantPoolEntry::Methodref {
                class_index,
                name_and_type_index,
            }) = pool.get(index)
            else {
                panic!("expected Methodref")
            };
            assert_eq!(pool.class_name(*class_index), Ok(owner));
            assert_eq!(
                pool.name_and_type(*name_and_type_index),
                Ok(("run", descriptor))
            );
        }
    }
}

#[test]
#[ignore = "release-mode end-to-end compilation measurement"]
fn measure_dependent_pipeline() {
    eprintln!(
        "layout,files,sample,workers,setup_us,parse_us,bind_and_name_us,signatures_us,bodies_us,class_plans_us,prepare_us,emit_us,emit_workers_us,total_us"
    );
    let cases = [4, 16, 64].map(|groups| {
        [Layout::Chains, Layout::Shared].map(|layout| {
            let sources = dependent_sources(groups, layout);
            let files = inputs(&sources);
            let reference = compile_timed(&files, 1).0;
            assert_eq!(reference.len(), files.len());
            assert_eq!(reference, compile_timed(&files, 4).0);
            (layout, sources, reference)
        })
    });
    for sample in 0..9 {
        for offset in 0..cases.len() {
            let layouts = &cases[(sample + offset) % cases.len()];
            let order = if (sample + offset) % 2 == 0 {
                [0, 1]
            } else {
                [1, 0]
            };
            for index in order {
                let (layout, sources, reference) = &layouts[index];
                let files = inputs(sources);
                let count = files.len();
                let workers_order = if sample % 2 == 0 { [1, 4] } else { [4, 1] };
                for workers in workers_order {
                    let (artifacts, t) = compile_timed(&files, workers);
                    assert_eq!(&artifacts, reference);
                    eprintln!(
                        "{layout:?},{count},{sample},{workers},{},{},{},{},{},{},{},{},{},{}",
                        t.setup.as_micros(),
                        t.parse.as_micros(),
                        t.bind_and_name.as_micros(),
                        t.signatures.as_micros(),
                        t.bodies.as_micros(),
                        t.class_plans.as_micros(),
                        t.prepare.as_micros(),
                        t.emit.as_micros(),
                        t.emit_workers.as_micros(),
                        t.total.as_micros()
                    );
                }
            }
        }
    }
}
