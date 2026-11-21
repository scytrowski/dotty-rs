#[path = "support/parsing.rs"]
mod parsing;

use dotty::core::{NameInterner, SourceId, SourceText};
use dotty::lexer::ContextualScanner;
use dotty::parser::{ExpressionIssue, ParseIssue, parse_compilation_unit};

#[test]
fn binding_in_input_order_matches_sequential_trees_and_diagnostics() {
    let files = [
        (
            SourceId::from_index(8),
            "First.scala",
            r#"package shared
import foo.{bar as baz}
object A:
  private[shared] def bar(x: String): String = s"prefix ${x}"
"#,
        ),
        (
            SourceId::from_index(3),
            "Second.scala",
            "package shared\nclass B:\n  def bar(): Int = 42\n",
        ),
        (
            SourceId::from_index(5),
            "Broken.scala",
            "package shared\nclass C:\n  def bar = x + y +: z\n",
        ),
    ];
    let mut serial = NameInterner::new();
    let mut session = NameInterner::new();
    for table in [&mut session, &mut serial] {
        table.intern("preexisting");
        table.intern("bar");
    }

    // Deliver worker results in reverse file order, then bind in input order.
    let mut submitted = files;
    submitted.reverse();
    for (&(id, _, text), detached) in files
        .iter()
        .zip(parsing::parse_files(&submitted, 3).into_iter().rev())
    {
        let actual = detached.into_session(&mut session).syntax;
        let expected = parse_compilation_unit(
            SourceText::new(text).unwrap(),
            id,
            ContextualScanner::new(text).unwrap(),
            &mut serial,
        );
        assert_eq!(actual.root, expected.root);
        assert!(actual.ast.iter().eq(expected.ast.iter()), "{text}");
        assert_eq!(actual.diagnostics, expected.diagnostics);
        assert_eq!(actual.effective_features, expected.effective_features);
        if id == files[2].0 {
            let [diagnostic] = &actual.diagnostics[..] else {
                panic!("expected one operator error")
            };
            let ParseIssue::Expression(ExpressionIssue::MixedAssociativityOperators {
                left,
                right,
            }) = diagnostic.issue()
            else {
                panic!("unexpected diagnostic {diagnostic:?}")
            };
            assert_eq!(diagnostic.source(), id);
            assert_eq!(
                (session.resolve(left.text()), session.resolve(right.text())),
                ("+", "+:")
            );
        } else {
            assert!(actual.diagnostics.is_empty(), "{:?}", actual.diagnostics);
        }
    }
    assert!(serial.spellings().eq(session.spellings()));
}

#[test]
#[ignore = "release-mode parallel parser measurement"]
fn measure_parallel_parse() {
    use std::{fmt::Write, hint::black_box, time::Instant};

    let sources: Vec<_> = (0..64)
        .map(|file| {
            let mut source = format!("package shared\nclass C{file}:\n");
            for method in 0..64 {
                writeln!(
                    source,
                    "  def m{method}(shared: Int): Int = shared + {method}"
                )
                .unwrap();
            }
            source
        })
        .collect();
    let files: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| (SourceId::from_index(i as u32), "Measure.scala", s.as_str()))
        .collect();
    for _ in 0..5 {
        let mut serial = NameInterner::new();
        let start = Instant::now();
        let parsed: Vec<_> = files
            .iter()
            .map(|&(id, _, text)| {
                parse_compilation_unit(
                    SourceText::new(text).unwrap(),
                    id,
                    ContextualScanner::new(text).unwrap(),
                    &mut serial,
                )
            })
            .collect();
        let sequential = start.elapsed();
        assert!(parsed.iter().all(|unit| unit.diagnostics.is_empty()));
        drop(parsed);
        for workers in [1, 4] {
            let mut session = NameInterner::new();
            let start = Instant::now();
            let local = parsing::parse_files(&files, workers);
            let parsed = Instant::now();
            let results: Vec<_> = local
                .into_iter()
                .map(|unit| unit.into_session(&mut session))
                .collect();
            let bind = parsed.elapsed();
            assert!(
                black_box(&results)
                    .iter()
                    .all(|unit| unit.diagnostics.is_empty())
            );
            eprintln!(
                "workers={workers} serial={sequential:?} detached={:?} bind={bind:?}",
                parsed - start
            );
        }
    }
}
