use dotty_core::token::TokenKind;
use dotty_lexer::{ContextualScanner, RawItem, RawLexer, RawTokenKind};

const FRAGMENTS: &[&str] = &[
    "a",
    "é",
    "𐐀",
    " ",
    "\t",
    "\n",
    "\r\n",
    "// comment\n",
    "/* block */",
    "_",
    "if",
    "=>",
    "::",
    "'a'",
    "' '",
    "\"text\"",
    "s\"$value\"",
    "<tag/>",
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    "\\",
    "$",
    "\"\"\"text\"\"\"",
    "`quoted-name`",
    ":",
    "?",
    "|",
];

#[test]
fn raw_lexer_property_inputs_reach_eof_with_bounded_utf8_spans() {
    for seed in 0..512u64 {
        let source = generated_source(seed, 24);
        let mut lexer = RawLexer::new(&source).expect("generated UTF-8 source is valid");
        let mut previous_end = 0;
        let mut saw_eof = false;

        for step in 0..4096 {
            let item = lexer
                .next()
                .unwrap_or_else(|error| panic!("seed {seed}, step {step}: {error}"));
            let Some(item) = item else {
                break;
            };
            let span = match &item {
                RawItem::Token(token) => token.span,
                RawItem::Trivia(trivia) => trivia.span,
            };
            assert!(
                source
                    .get(span.start() as usize..span.end() as usize)
                    .is_some(),
                "seed {seed}, invalid UTF-8 span {span:?} in {source:?}"
            );
            assert!(
                span.start() >= previous_end,
                "seed {seed}, overlapping span {span:?} after {previous_end} in {source:?}"
            );
            previous_end = span.end();
            if matches!(&item, RawItem::Token(token) if token.kind == RawTokenKind::Eof) {
                saw_eof = true;
                assert_eq!(span.start() as usize, source.len(), "seed {seed}");
                break;
            }
        }

        assert!(saw_eof, "seed {seed} did not reach EOF for {source:?}");
        assert!(
            lexer.next().expect("post-EOF read cannot fail").is_none(),
            "seed {seed} emitted an item after EOF"
        );
        assert_diagnostics_are_bounded(&source, lexer.diagnostics(), seed);
    }
}

#[test]
fn contextual_scanner_property_inputs_reach_eof_with_bounded_spans() {
    for seed in 0..512u64 {
        let source = generated_source(seed ^ 0x9e37_79b9, 20);
        let mut scanner = ContextualScanner::new(&source)
            .unwrap_or_else(|error| panic!("seed {seed}: failed to scan {source:?}: {error}"));
        let token_count = scanner.tokens().len();
        assert!(token_count > 0, "seed {seed} produced no tokens");

        for token in scanner.tokens() {
            assert!(
                source
                    .get(token.span.start() as usize..token.span.end() as usize)
                    .is_some(),
                "seed {seed}, invalid token span {:?} in {source:?}",
                token.span
            );
        }
        assert_eq!(
            scanner.tokens().last().map(|token| token.kind),
            Some(TokenKind::Eof),
            "seed {seed}"
        );

        let mut steps = 0;
        while scanner.next().is_some() {
            steps += 1;
            assert!(
                steps <= token_count,
                "seed {seed} scanner did not terminate"
            );
        }
        assert_eq!(steps, token_count, "seed {seed} skipped scanner tokens");
        assert_diagnostics_are_bounded(&source, scanner.diagnostics(), seed);
    }
}

#[test]
fn unterminated_string_recovery_keeps_the_public_raw_stream_terminating() {
    assert_reaches_eof("\"unterminated");
}

#[test]
fn unterminated_block_comment_recovery_keeps_the_public_raw_stream_terminating() {
    assert_reaches_eof("/* unterminated");
}

#[test]
fn unterminated_xml_recovery_keeps_the_public_scanner_terminating() {
    let source = "<root>body";
    let scanner = ContextualScanner::new(source).expect("XML recovery is diagnostic-based");

    assert_eq!(
        scanner.tokens().last().map(|token| token.kind),
        Some(TokenKind::Eof)
    );
    assert_diagnostics_are_bounded(source, scanner.diagnostics(), 0);
}

#[test]
fn raw_regex_interpolators_do_not_report_string_escape_diagnostics() {
    let source = r#"val splitter = raw"([^=]+)=(.+)".r
val errorId = raw"E?(\d+)".r"#;
    let scanner = ContextualScanner::new(source).expect("raw regex source scans");

    assert!(scanner.diagnostics().is_empty());
    assert_eq!(
        scanner
            .tokens()
            .iter()
            .filter(|token| token.kind == TokenKind::InterpolationId)
            .count(),
        2
    );
    assert_eq!(
        scanner
            .tokens()
            .iter()
            .filter(|token| token.kind == TokenKind::StringPart)
            .count(),
        2
    );

    let ordinary = ContextualScanner::new(r#"s"\d+""#)
        .expect("ordinary interpolated string scans with diagnostics");
    assert_eq!(ordinary.diagnostics().len(), 1);
    assert_eq!(
        ordinary.diagnostics()[0].message(),
        "invalid escape character"
    );
}

fn assert_reaches_eof(source: &str) {
    let mut lexer = RawLexer::new(source).expect("source is valid UTF-8");
    let mut saw_eof = false;
    for step in 0..128 {
        let item = lexer
            .next()
            .unwrap_or_else(|error| panic!("step {step}: {error}"));
        let Some(item) = item else {
            break;
        };
        if matches!(&item, RawItem::Token(token) if token.kind == RawTokenKind::Eof) {
            saw_eof = true;
            break;
        }
    }
    assert!(saw_eof, "raw lexer did not recover to EOF for {source:?}");
    assert_diagnostics_are_bounded(source, lexer.diagnostics(), 0);
}

fn assert_diagnostics_are_bounded(
    source: &str,
    diagnostics: &[dotty_core::diagnostics::Diagnostic],
    seed: u64,
) {
    for diagnostic in diagnostics {
        assert!(
            source
                .get(diagnostic.span().start() as usize..diagnostic.span().end() as usize)
                .is_some(),
            "seed {seed}, invalid diagnostic span {:?} in {source:?}",
            diagnostic.span()
        );
    }
}

fn generated_source(mut state: u64, fragments: usize) -> String {
    let mut source = String::new();
    for _ in 0..fragments {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        source.push_str(FRAGMENTS[(state >> 32) as usize % FRAGMENTS.len()]);
    }
    source
}
