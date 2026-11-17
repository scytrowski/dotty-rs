//! Run with `cargo test --release -p dotty --test parser_scaling -- --ignored --nocapture`.
use std::fmt::Write;
use std::time::{Duration, Instant};

use dotty_core::{NameInterner, SourceId, SourceText};
use dotty_lexer::ContextualScanner;
use dotty_parser::parse_compilation_unit;

fn median_parse(source: &str) -> Duration {
    let text = SourceText::new(source).unwrap();
    let mut samples = [Duration::ZERO; 5];
    for sample in &mut samples {
        let mut names = NameInterner::new();
        let scanner = ContextualScanner::new(source).unwrap();
        assert!(scanner.diagnostics().is_empty());
        let start = Instant::now();
        let result = parse_compilation_unit(text, SourceId::from_index(0), scanner, &mut names);
        *sample = start.elapsed();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

#[test]
#[ignore = "manual release-mode parser scaling benchmark"]
fn repeated_method_bodies() {
    for body in [r#"s"amount $$5 $x""#, "try a catch { case e => b }"] {
        for count in [250, 500, 1000, 2000] {
            let mut source = String::from("object A {\n");
            for i in 0..count {
                writeln!(source, "def m{i} = {body}").unwrap();
            }
            source.push('}');
            println!("{count} methods, {body}: {:?}", median_parse(&source));
        }
    }
}

#[test]
#[ignore = "manual release-mode parser scaling benchmark"]
fn adjacent_indented_templates() {
    for count in [500, 1000, 2000, 4000] {
        let mut source = String::new();
        for i in 0..count {
            writeln!(source, "object A{i}:\n  val x = 1").unwrap();
        }
        println!("{count} indented templates: {:?}", median_parse(&source));
    }
}
