use std::{env, fs, process};

use dotty_lexer::ContextualScanner;
use dotty_token::{HardKeyword, Punctuation, Token, TokenKind};

fn main() {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: lexer-dump <source-file>");
        process::exit(2);
    };

    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("failed to read {path}: {error}");
            process::exit(1);
        }
    };
    let scanner = match ContextualScanner::new(&source) {
        Ok(scanner) => scanner,
        Err(error) => {
            eprintln!("failed to scan {path}: {error}");
            process::exit(1);
        }
    };

    println!("kind\tstart\tend");
    for token in scanner.tokens() {
        let start = token.span.start() as usize;
        let end = token.span.end() as usize;
        let spelling = source.get(start..end).unwrap_or("");
        println!(
            "{}\t{}\t{}",
            normalized_kind(token, spelling),
            token.span.start(),
            token.span.end()
        );
    }
}

fn normalized_kind(token: &Token, spelling: &str) -> String {
    match token.kind {
        TokenKind::Error => "error".to_owned(),
        TokenKind::Identifier => "identifier".to_owned(),
        TokenKind::BackquotedIdentifier => "backquoted identifier".to_owned(),
        TokenKind::Quote => "'".to_owned(),
        TokenKind::QuoteId => "quoted identifier".to_owned(),
        TokenKind::XmlStart => "$XMLSTART$<".to_owned(),
        TokenKind::Operator => "operator".to_owned(),
        TokenKind::Keyword(keyword) => keyword_spelling(keyword, spelling),
        TokenKind::Punctuation(punctuation) => punctuation_spelling(punctuation),
        TokenKind::ColonOp | TokenKind::ColonFollow | TokenKind::ColonEol => "':'".to_owned(),
        TokenKind::CaseClass => "case class".to_owned(),
        TokenKind::CaseObject => "case object".to_owned(),
        TokenKind::EndMarker => "end".to_owned(),
        TokenKind::CharLiteral => "character literal".to_owned(),
        TokenKind::IntegerLiteral => "integer literal".to_owned(),
        TokenKind::DecimalLiteral => "decimal literal".to_owned(),
        TokenKind::ExponentLiteral => "exponent literal".to_owned(),
        TokenKind::LongLiteral => "long literal".to_owned(),
        TokenKind::FloatLiteral => "float literal".to_owned(),
        TokenKind::DoubleLiteral => "double literal".to_owned(),
        TokenKind::StringLiteral => "string literal".to_owned(),
        TokenKind::InterpolationId => "string interpolator".to_owned(),
        TokenKind::StringPart => "string part".to_owned(),
        TokenKind::Newline | TokenKind::Newlines => "end of statement".to_owned(),
        TokenKind::Indent => "indent".to_owned(),
        TokenKind::Outdent => "unindent".to_owned(),
        TokenKind::Eof => "eof".to_owned(),
    }
}

fn keyword_spelling(keyword: HardKeyword, source_spelling: &str) -> String {
    if !source_spelling.is_empty() {
        source_spelling.to_owned()
    } else {
        format!("{keyword:?}").to_ascii_lowercase()
    }
}

fn punctuation_spelling(punctuation: Punctuation) -> String {
    let spelling = match punctuation {
        Punctuation::Comma => ",",
        Punctuation::Semicolon => ";",
        Punctuation::Dot => ".",
        Punctuation::Colon => ":",
        Punctuation::LeftParen => "(",
        Punctuation::RightParen => ")",
        Punctuation::LeftBracket => "[",
        Punctuation::RightBracket => "]",
        Punctuation::LeftBrace => "{",
        Punctuation::RightBrace => "}",
    };
    format!("'{spelling}'")
}
