//! Run with `cargo test --release -p dotty-lexer --test scaling -- --ignored --nocapture`.
use std::hint::black_box;
use std::time::{Duration, Instant};

use dotty_lexer::ContextualScanner;

fn median_scan(source: &str) -> Duration {
    let warmup = ContextualScanner::new(source).unwrap();
    assert!(warmup.diagnostics().is_empty());
    drop(warmup);
    let mut samples = [Duration::ZERO; 7];
    for sample in &mut samples {
        let start = Instant::now();
        black_box(ContextualScanner::new(black_box(source)).unwrap());
        *sample = start.elapsed();
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

#[test]
#[ignore = "manual release-mode scaling benchmark"]
fn argument_lists() {
    for count in [1000, 2000, 4000] {
        for separator in [", ", ",\n  "] {
            let arguments = vec!["1"; count].join(separator);
            let source = format!("val a = List({arguments})");
            println!(
                "{count} arguments, separator {separator:?}: {:?}",
                median_scan(&source)
            );
        }
    }
}

#[test]
#[ignore = "manual release-mode scaling benchmark"]
fn case_declarations() {
    for count in [1000, 2000, 4000] {
        let source: String = (0..count)
            .map(|n| format!("case class C{n}(x: Int)\n"))
            .collect();
        println!("{count} case classes: {:?}", median_scan(&source));
    }
}
