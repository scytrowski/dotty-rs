use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use dotty_core::{NameInterner, SourceId, SourceText};
use dotty_lexer::ContextualScanner;
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};
use serde::Serialize;

const DEFAULT_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug)]
struct Options {
    roots: Vec<PathBuf>,
    output: Option<PathBuf>,
    timeout: Duration,
    source_version: Option<String>,
    source_revision: Option<String>,
    oracle_files: Option<usize>,
    oracle_failures: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    corpus_roots: Vec<String>,
    source_version: Option<String>,
    source_revision: Option<String>,
    scala_oracle_files: Option<usize>,
    scala_oracle_failures: Option<usize>,
    files_attempted: usize,
    files_parsed_without_diagnostics: usize,
    files_parsed_with_recoverable_diagnostics: usize,
    hard_parser_failures: usize,
    panics: usize,
    hangs: usize,
    scanner_diagnostics: usize,
    diagnostic_histogram: BTreeMap<String, usize>,
    first_failure_histogram: BTreeMap<String, FailureBucket>,
}

#[derive(Debug, Serialize)]
struct FailureBucket {
    count: usize,
    examples: Vec<String>,
}

#[derive(Debug)]
struct FileOutcome {
    path: String,
    status: Status,
    diagnostics: Vec<DiagnosticSummary>,
    scanner_diagnostics: usize,
}

#[derive(Debug)]
enum Status {
    Clean,
    RecoverableDiagnostics,
    ScannerFailure,
    Panic,
    Hang,
}

#[derive(Debug)]
struct DiagnosticSummary {
    kind: String,
    message: String,
}

fn main() {
    let options = match parse_options(env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            eprintln!(
                "usage: dotty-parser-corpus-report --root <dir>... [--output <file>] [--timeout-ms <n>]"
            );
            std::process::exit(2);
        }
    };

    let files = match discover_files(&options.roots) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("failed to discover corpus files: {error}");
            std::process::exit(1);
        }
    };

    let outcomes = files
        .iter()
        .map(|path| parse_one(path, &options.roots, options.timeout))
        .collect::<Vec<_>>();
    let report = build_report(
        &outcomes,
        &options.roots,
        options.source_version,
        options.source_revision,
        options.oracle_files,
        options.oracle_failures,
    );

    if let Some(output) = options.output {
        if let Err(error) = write_report(&output, &report) {
            eprintln!("failed to write report {}: {error}", output.display());
            std::process::exit(1);
        }
    }

    print_summary(&report);
    if report.hard_parser_failures != 0 {
        std::process::exit(1);
    }
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut roots = Vec::new();
    let mut output = None;
    let mut timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
    let mut source_version = None;
    let mut source_revision = None;
    let mut oracle_files = None;
    let mut oracle_failures = None;
    let mut args = args.peekable();

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => roots.push(PathBuf::from(next_argument(&mut args, "--root")?)),
            "--output" => output = Some(PathBuf::from(next_argument(&mut args, "--output")?)),
            "--timeout-ms" => {
                let value = next_argument(&mut args, "--timeout-ms")?;
                let millis = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid timeout in milliseconds: {value}"))?;
                if millis == 0 {
                    return Err("--timeout-ms must be greater than zero".to_owned());
                }
                timeout = Duration::from_millis(millis);
            }
            "--source-version" => {
                source_version = Some(next_argument(&mut args, "--source-version")?);
            }
            "--source-revision" => {
                source_revision = Some(next_argument(&mut args, "--source-revision")?);
            }
            "--oracle-files" => {
                let value = next_argument(&mut args, "--oracle-files")?;
                oracle_files = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| format!("invalid oracle file count: {value}"))?,
                );
            }
            "--oracle-failures" => {
                let value = next_argument(&mut args, "--oracle-failures")?;
                oracle_failures = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| format!("invalid oracle failure count: {value}"))?,
                );
            }
            "--help" | "-h" => {
                return Err(
                    "usage: dotty-parser-corpus-report --root <dir>... [--output <file>] [--timeout-ms <n>]".to_owned(),
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if roots.is_empty() {
        return Err("at least one --root is required".to_owned());
    }
    Ok(Options {
        roots,
        output,
        timeout,
        source_version,
        source_revision,
        oracle_files,
        oracle_failures,
    })
}

fn next_argument(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn discover_files(roots: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in roots {
        collect_scala_files(root, &mut files)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_scala_files(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if metadata.is_file() {
        if path
            .extension()
            .is_some_and(|extension| extension == "scala")
        {
            files.push(path.to_owned());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        collect_scala_files(&entry.path(), files)?;
    }
    Ok(())
}

fn parse_one(path: &Path, roots: &[PathBuf], timeout: Duration) -> FileOutcome {
    let display_path = display_path(path, roots);
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return FileOutcome {
                path: display_path,
                status: Status::ScannerFailure,
                diagnostics: vec![DiagnosticSummary {
                    kind: "IoError".to_owned(),
                    message: error.to_string(),
                }],
                scanner_diagnostics: 0,
            };
        }
    };

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_source(&source)));
        let _ = sender.send(result);
    });

    match receiver.recv_timeout(timeout) {
        Ok(Ok(outcome)) => FileOutcome {
            path: display_path,
            status: outcome.status,
            diagnostics: outcome.diagnostics,
            scanner_diagnostics: outcome.scanner_diagnostics,
        },
        Ok(Err(_)) => FileOutcome {
            path: display_path,
            status: Status::Panic,
            diagnostics: vec![DiagnosticSummary {
                kind: "Panic".to_owned(),
                message: "parser worker panicked".to_owned(),
            }],
            scanner_diagnostics: 0,
        },
        Err(mpsc::RecvTimeoutError::Timeout) => FileOutcome {
            path: display_path,
            status: Status::Hang,
            diagnostics: vec![DiagnosticSummary {
                kind: "Hang".to_owned(),
                message: format!("parser exceeded {} ms", timeout.as_millis()),
            }],
            scanner_diagnostics: 0,
        },
        Err(mpsc::RecvTimeoutError::Disconnected) => FileOutcome {
            path: display_path,
            status: Status::Panic,
            diagnostics: vec![DiagnosticSummary {
                kind: "Panic".to_owned(),
                message: "parser worker disconnected".to_owned(),
            }],
            scanner_diagnostics: 0,
        },
    }
}

struct ParsedSource {
    status: Status,
    diagnostics: Vec<DiagnosticSummary>,
    scanner_diagnostics: usize,
}

fn parse_source(source: &str) -> ParsedSource {
    let scanner = match ContextualScanner::new(source) {
        Ok(scanner) => scanner,
        Err(error) => {
            return ParsedSource {
                status: Status::ScannerFailure,
                diagnostics: vec![DiagnosticSummary {
                    kind: "ScannerError".to_owned(),
                    message: error.to_string(),
                }],
                scanner_diagnostics: 0,
            };
        }
    };
    let scanner_diagnostics = scanner.diagnostics().len();
    let source_text = match SourceText::new(source) {
        Ok(source_text) => source_text,
        Err(error) => {
            return ParsedSource {
                status: Status::ScannerFailure,
                diagnostics: vec![DiagnosticSummary {
                    kind: "SourceTextError".to_owned(),
                    message: error.to_string(),
                }],
                scanner_diagnostics,
            };
        }
    };
    let mut names = NameInterner::new();
    let result = parse_compilation_unit(source_text, SourceId::from_index(0), scanner, &mut names);
    let diagnostics = result
        .diagnostics
        .iter()
        .map(|diagnostic| DiagnosticSummary {
            kind: diagnostic_kind_name(diagnostic.kind()).to_owned(),
            message: diagnostic.message().to_owned(),
        })
        .collect::<Vec<_>>();
    let status = if diagnostics.is_empty() && scanner_diagnostics == 0 {
        Status::Clean
    } else {
        Status::RecoverableDiagnostics
    };
    ParsedSource {
        status,
        diagnostics,
        scanner_diagnostics,
    }
}

fn display_path(path: &Path, roots: &[PathBuf]) -> String {
    for root in roots {
        if let Ok(relative) = path.strip_prefix(root) {
            return Path::new(&root_label(root))
                .join(relative)
                .display()
                .to_string();
        }
    }
    path.display().to_string()
}

fn root_label(root: &Path) -> String {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("root");
    if name == "src" {
        if let Some(parent) = root.parent().and_then(|parent| parent.file_name()) {
            return Path::new(parent).join(name).display().to_string();
        }
    }
    name.to_owned()
}

fn build_report(
    outcomes: &[FileOutcome],
    roots: &[PathBuf],
    source_version: Option<String>,
    source_revision: Option<String>,
    oracle_files: Option<usize>,
    oracle_failures: Option<usize>,
) -> Report {
    let mut report = Report {
        schema_version: 1,
        corpus_roots: roots.iter().map(|root| root_label(root)).collect(),
        source_version,
        source_revision,
        scala_oracle_files: oracle_files,
        scala_oracle_failures: oracle_failures,
        files_attempted: outcomes.len(),
        files_parsed_without_diagnostics: 0,
        files_parsed_with_recoverable_diagnostics: 0,
        hard_parser_failures: 0,
        panics: 0,
        hangs: 0,
        scanner_diagnostics: 0,
        diagnostic_histogram: BTreeMap::new(),
        first_failure_histogram: BTreeMap::new(),
    };

    for outcome in outcomes {
        report.scanner_diagnostics += outcome.scanner_diagnostics;
        match outcome.status {
            Status::Clean => report.files_parsed_without_diagnostics += 1,
            Status::RecoverableDiagnostics => report.files_parsed_with_recoverable_diagnostics += 1,
            Status::ScannerFailure | Status::Panic | Status::Hang => {
                report.hard_parser_failures += 1;
                if matches!(outcome.status, Status::Panic) {
                    report.panics += 1;
                }
                if matches!(outcome.status, Status::Hang) {
                    report.hangs += 1;
                }
            }
        }

        for diagnostic in &outcome.diagnostics {
            *report
                .diagnostic_histogram
                .entry(diagnostic.kind.clone())
                .or_default() += 1;
        }
        if let Some(diagnostic) = outcome.diagnostics.first() {
            let bucket = first_failure_bucket(diagnostic);
            let entry = report
                .first_failure_histogram
                .entry(bucket)
                .or_insert_with(|| FailureBucket {
                    count: 0,
                    examples: Vec::new(),
                });
            entry.count += 1;
            if entry.examples.len() < 5 {
                entry.examples.push(outcome.path.clone());
            }
        }
    }
    report
}

fn first_failure_bucket(diagnostic: &DiagnosticSummary) -> String {
    if diagnostic.kind == "UnsupportedSyntax" {
        format!(
            "UnsupportedSyntax: {}",
            normalize_message(&diagnostic.message)
        )
    } else {
        format!("{}", diagnostic.kind)
    }
}

fn normalize_message(message: &str) -> String {
    message
        .split_whitespace()
        .map(|word| word.trim_matches(|character: char| matches!(character, '`' | '\'' | '"')))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn diagnostic_kind_name(kind: ParseDiagnosticKind) -> &'static str {
    match kind {
        ParseDiagnosticKind::ExpectedToken => "ExpectedToken",
        ParseDiagnosticKind::UnexpectedToken => "UnexpectedToken",
        ParseDiagnosticKind::ExpectedExpression => "ExpectedExpression",
        ParseDiagnosticKind::ExpectedType => "ExpectedType",
        ParseDiagnosticKind::ExpectedPattern => "ExpectedPattern",
        ParseDiagnosticKind::UnsupportedSyntax => "UnsupportedSyntax",
        ParseDiagnosticKind::UnboundPlaceholderParameter => "UnboundPlaceholderParameter",
    }
}

fn write_report(path: &Path, report: &Report) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(path)?;
    serde_json::to_writer_pretty(file, report).map_err(io::Error::other)
}

fn print_summary(report: &Report) {
    println!("Parser corpus report");
    println!("  files attempted: {}", report.files_attempted);
    println!(
        "  parsed without diagnostics: {}",
        report.files_parsed_without_diagnostics
    );
    println!(
        "  parsed with recoverable diagnostics: {}",
        report.files_parsed_with_recoverable_diagnostics
    );
    println!("  hard parser failures: {}", report.hard_parser_failures);
    println!("  panics: {}", report.panics);
    println!("  hangs: {}", report.hangs);
    println!("  scanner diagnostics: {}", report.scanner_diagnostics);
    if !report.diagnostic_histogram.is_empty() {
        println!("  diagnostic histogram:");
        for (kind, count) in &report.diagnostic_histogram {
            println!("    {kind}: {count}");
        }
    }
    if !report.first_failure_histogram.is_empty() {
        println!("  first-failure buckets:");
        for (bucket, failure) in &report.first_failure_histogram {
            println!("    {bucket}: {}", failure.count);
            for example in &failure.examples {
                println!("      - {example}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_scala_files_in_sorted_order() {
        let root = unique_temp_dir("discover");
        fs::create_dir_all(root.join("nested")).expect("create temp corpus");
        fs::write(root.join("z.scala"), "z").expect("write source");
        fs::write(root.join("nested/a.scala"), "a").expect("write source");
        fs::write(root.join("ignored.txt"), "ignored").expect("write file");

        let files = discover_files(std::slice::from_ref(&root)).expect("discover files");
        assert_eq!(
            files,
            vec![root.join("nested/a.scala"), root.join("z.scala")]
        );
        fs::remove_dir_all(root).expect("remove temp corpus");
    }

    #[test]
    fn clean_source_has_no_diagnostics() {
        let parsed = parse_source("object C");
        assert!(matches!(parsed.status, Status::Clean));
        assert!(parsed.diagnostics.is_empty());
    }

    #[test]
    fn unsupported_diagnostic_bucket_is_normalized() {
        let bucket = first_failure_bucket(&DiagnosticSummary {
            kind: "UnsupportedSyntax".to_owned(),
            message: "unsupported `class`  syntax".to_owned(),
        });
        assert_eq!(bucket, "UnsupportedSyntax: unsupported class syntax");
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "dotty-parser-corpus-report-{name}-{}",
            std::process::id()
        ))
    }
}
