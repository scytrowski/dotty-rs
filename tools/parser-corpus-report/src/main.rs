use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use dotty_core::{Packages, SemanticStore, SourceId, SourceText};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};
use serde::{Deserialize, Serialize};

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
    namer: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    namer: Option<NamerReport>,
}

#[derive(Debug, Default, Serialize)]
struct NamerReport {
    files_parse_prevented_naming: usize,
    files_namer_ran: usize,
    files_namer_succeeded: usize,
    files_namer_returned_error: usize,
    files_named_after_parser_recovery: usize,
    files_with_invariant_failures: usize,
    namer_error_histogram: BTreeMap<String, NamerFailureBucket>,
    invariant_failure_histogram: BTreeMap<String, FailureBucket>,
}

#[derive(Debug, Serialize)]
struct NamerFailureBucket {
    count: usize,
    examples: Vec<NamerErrorExample>,
}

#[derive(Debug, Serialize)]
struct NamerErrorExample {
    path: String,
    message: String,
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
    namer: Option<NamerOutcome>,
}

#[derive(Debug, Serialize, Deserialize)]
enum NamerOutcome {
    Success { invariant_violations: Vec<String> },
    Error { kind: String, message: String },
}

#[derive(Debug, Serialize, Deserialize)]
enum Status {
    Clean,
    RecoverableDiagnostics,
    ScannerFailure,
    ProcessFailure,
    Panic,
    Hang,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiagnosticSummary {
    kind: String,
    message: String,
}

fn main() {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--worker")
    {
        if let Err(error) = run_worker(
            arguments.get(1),
            arguments.iter().any(|arg| arg == "--namer"),
        ) {
            eprintln!("worker failed: {error}");
            std::process::exit(2);
        }
        return;
    }

    let options = match parse_options(arguments.into_iter()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            eprintln!(
                "usage: dotty-parser-corpus-report --root <dir>... [--output <file>] [--timeout-ms <n>] [--source-version <v>] [--source-revision <sha>] [--oracle-files <n>] [--oracle-failures <n>] [--namer]"
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

    let outcomes = parse_files(&files, &options.roots, options.timeout, options.namer);
    let report = build_report(
        &outcomes,
        &options.roots,
        options.source_version,
        options.source_revision,
        options.oracle_files,
        options.oracle_failures,
        options.namer,
    );

    if let Some(output) = options.output
        && let Err(error) = write_report(&output, &report)
    {
        eprintln!("failed to write report {}: {error}", output.display());
        std::process::exit(1);
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
    let mut namer = false;
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
            "--namer" => namer = true,
            "--help" | "-h" => {
                return Err(
                    "usage: dotty-parser-corpus-report --root <dir>... [--output <file>] [--timeout-ms <n>] [--source-version <v>] [--source-revision <sha>] [--oracle-files <n>] [--oracle-failures <n>] [--namer]".to_owned(),
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
        namer,
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

fn parse_files(
    files: &[PathBuf],
    roots: &[PathBuf],
    timeout: Duration,
    namer: bool,
) -> Vec<FileOutcome> {
    if files.is_empty() {
        return Vec::new();
    }

    let worker_count = thread::available_parallelism()
        .map(|parallelism| parallelism.get().min(4))
        .unwrap_or(1)
        .min(files.len());
    let (job_sender, job_receiver) = mpsc::channel::<(usize, PathBuf)>();
    let (result_sender, result_receiver) = mpsc::channel::<(usize, FileOutcome)>();
    let job_receiver = Arc::new(Mutex::new(job_receiver));
    let mut workers = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let job_receiver = Arc::clone(&job_receiver);
        let result_sender = result_sender.clone();
        let roots = roots.to_vec();
        workers.push(thread::spawn(move || {
            loop {
                let job = job_receiver.lock().expect("job queue lock poisoned").recv();
                let Ok((index, path)) = job else {
                    break;
                };
                let outcome = parse_one(&path, &roots, timeout, namer);
                if result_sender.send((index, outcome)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(result_sender);

    for (index, path) in files.iter().cloned().enumerate() {
        job_sender
            .send((index, path))
            .expect("parser worker pool unexpectedly stopped");
    }
    drop(job_sender);

    let mut outcomes = result_receiver.into_iter().collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("parser worker thread panicked");
    }
    outcomes.sort_by_key(|(index, _)| *index);
    outcomes.into_iter().map(|(_, outcome)| outcome).collect()
}

fn parse_one(path: &Path, roots: &[PathBuf], timeout: Duration, namer: bool) -> FileOutcome {
    let display_path = display_path(path, roots);
    let executable = match env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return process_failure(display_path, "ProcessError", error.to_string()),
    };
    let mut command = Command::new(executable);
    command
        .arg("--worker")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if namer {
        command.arg("--namer");
    }

    match run_command_with_timeout(command, timeout) {
        Ok(CommandResult::Exited {
            success: true,
            stdout,
        }) => match serde_json::from_slice::<WorkerResult>(&stdout) {
            Ok(outcome) => FileOutcome {
                path: display_path,
                status: outcome.status,
                diagnostics: outcome.diagnostics,
                scanner_diagnostics: outcome.scanner_diagnostics,
                namer: outcome.namer,
            },
            Err(error) => process_failure(display_path, "WorkerProtocol", error.to_string()),
        },
        Ok(CommandResult::Exited { success: false, .. }) => process_failure(
            display_path,
            "WorkerProcess",
            "parser worker exited unsuccessfully",
        ),
        Ok(CommandResult::TimedOut) => FileOutcome {
            path: display_path,
            status: Status::Hang,
            diagnostics: vec![DiagnosticSummary {
                kind: "Hang".to_owned(),
                message: format!("parser exceeded {} ms", timeout.as_millis()),
            }],
            scanner_diagnostics: 0,
            namer: None,
        },
        Err(error) => process_failure(display_path, "ProcessError", error.to_string()),
    }
}

fn process_failure(path: String, kind: &str, message: impl Into<String>) -> FileOutcome {
    FileOutcome {
        path,
        status: Status::ProcessFailure,
        diagnostics: vec![DiagnosticSummary {
            kind: kind.to_owned(),
            message: message.into(),
        }],
        scanner_diagnostics: 0,
        namer: None,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerResult {
    status: Status,
    diagnostics: Vec<DiagnosticSummary>,
    scanner_diagnostics: usize,
    namer: Option<NamerOutcome>,
}

fn run_worker(path: Option<&String>, namer: bool) -> io::Result<()> {
    let path = path.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing path"))?;
    let result = match fs::read_to_string(path) {
        Ok(source) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                parse_source(&source, namer)
            })) {
                Ok(parsed) => WorkerResult {
                    status: parsed.status,
                    diagnostics: parsed.diagnostics,
                    scanner_diagnostics: parsed.scanner_diagnostics,
                    namer: parsed.namer,
                },
                Err(_) => WorkerResult {
                    status: Status::Panic,
                    diagnostics: vec![DiagnosticSummary {
                        kind: "Panic".to_owned(),
                        message: "parser worker panicked".to_owned(),
                    }],
                    scanner_diagnostics: 0,
                    namer: None,
                },
            }
        }
        Err(error) => WorkerResult {
            status: Status::ScannerFailure,
            diagnostics: vec![DiagnosticSummary {
                kind: "IoError".to_owned(),
                message: error.to_string(),
            }],
            scanner_diagnostics: 0,
            namer: None,
        },
    };
    serde_json::to_writer(io::stdout(), &result).map_err(io::Error::other)?;
    io::stdout().write_all(b"\n")
}

enum CommandResult {
    Exited { success: bool, stdout: Vec<u8> },
    TimedOut,
}

fn run_command_with_timeout(mut command: Command, timeout: Duration) -> io::Result<CommandResult> {
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .expect("worker stdout must be piped for the result protocol");
    let reader = thread::spawn(move || {
        let mut stdout = stdout;
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = reader
                    .join()
                    .map_err(|_| io::Error::other("worker stdout reader panicked"))??;
                return Ok(CommandResult::Exited {
                    success: status.success(),
                    stdout,
                });
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            let _ = reader.join();
            return Ok(CommandResult::TimedOut);
        }
        thread::sleep(Duration::from_millis(1));
    }
}

struct ParsedSource {
    status: Status,
    diagnostics: Vec<DiagnosticSummary>,
    scanner_diagnostics: usize,
    namer: Option<NamerOutcome>,
}

fn parse_source(source: &str, run_namer: bool) -> ParsedSource {
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
                namer: None,
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
                namer: None,
            };
        }
    };
    let mut store = SemanticStore::new();
    let result = parse_compilation_unit(
        source_text,
        SourceId::from_index(0),
        scanner,
        &mut store.names,
    );
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
    let namer = run_namer.then(|| {
        let mut packages = Packages::new();
        match name_compilation_unit(
            &result.ast,
            result.root,
            SourceId::from_index(0),
            "Corpus.scala",
            &mut store,
            &mut packages,
        ) {
            Ok(index) => NamerOutcome::Success {
                invariant_violations: index.validate(&store),
            },
            Err(error) => NamerOutcome::Error {
                kind: namer_error_kind(&error),
                message: error.to_string(),
            },
        }
    });
    ParsedSource {
        status,
        diagnostics,
        scanner_diagnostics,
        namer,
    }
}

fn namer_error_kind(error: &dotty_namer::NamerError) -> String {
    match error {
        dotty_namer::NamerError::RootIsNotPackage { .. } => "RootIsNotPackage",
        dotty_namer::NamerError::DuplicateSourceTreeSymbol { .. } => "DuplicateSourceTreeSymbol",
        dotty_namer::NamerError::DuplicateDerivedSourceTreeSymbol { .. } => {
            "DuplicateDerivedSourceTreeSymbol"
        }
        dotty_namer::NamerError::DuplicateExtensionPrefixClauses { .. } => {
            "DuplicateExtensionPrefixClauses"
        }
        dotty_namer::NamerError::DuplicateDeclarationScope { .. } => "DuplicateDeclarationScope",
        dotty_namer::NamerError::DuplicateDeclarationContext { .. } => {
            "DuplicateDeclarationContext"
        }
        dotty_namer::NamerError::MalformedAstShape { .. } => "MalformedAstShape",
        dotty_namer::NamerError::InvalidVisibilityQualifier { .. } => "InvalidVisibilityQualifier",
        dotty_namer::NamerError::UnsupportedGivenNameShape { .. } => "UnsupportedGivenNameShape",
        dotty_namer::NamerError::AnonymousGivenWithoutParents { .. } => {
            "AnonymousGivenWithoutParents"
        }
    }
    .to_owned()
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
    if name == "src"
        && let Some(parent) = root.parent().and_then(|parent| parent.file_name())
    {
        return Path::new(parent).join(name).display().to_string();
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
    collect_namer: bool,
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
        namer: collect_namer.then(NamerReport::default),
    };

    for outcome in outcomes {
        if let Some(namer) = &mut report.namer {
            match &outcome.namer {
                None => namer.files_parse_prevented_naming += 1,
                Some(NamerOutcome::Success {
                    invariant_violations,
                }) => {
                    namer.files_namer_ran += 1;
                    namer.files_namer_succeeded += 1;
                    if matches!(outcome.status, Status::RecoverableDiagnostics) {
                        namer.files_named_after_parser_recovery += 1;
                    }
                    if !invariant_violations.is_empty() {
                        namer.files_with_invariant_failures += 1;
                        for violation in invariant_violations {
                            let entry = namer
                                .invariant_failure_histogram
                                .entry(violation.clone())
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
                }
                Some(NamerOutcome::Error { kind, message }) => {
                    namer.files_namer_ran += 1;
                    namer.files_namer_returned_error += 1;
                    let entry = namer
                        .namer_error_histogram
                        .entry(kind.clone())
                        .or_insert_with(|| NamerFailureBucket {
                            count: 0,
                            examples: Vec::new(),
                        });
                    entry.count += 1;
                    if entry.examples.len() < 5 {
                        entry.examples.push(NamerErrorExample {
                            path: outcome.path.clone(),
                            message: message.clone(),
                        });
                    }
                }
            }
        }
        report.scanner_diagnostics += outcome.scanner_diagnostics;
        match outcome.status {
            Status::Clean => report.files_parsed_without_diagnostics += 1,
            Status::RecoverableDiagnostics => report.files_parsed_with_recoverable_diagnostics += 1,
            Status::ScannerFailure | Status::ProcessFailure | Status::Panic | Status::Hang => {
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
        } else if outcome.scanner_diagnostics != 0 {
            let entry = report
                .first_failure_histogram
                .entry("ScannerDiagnostics".to_owned())
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
        diagnostic.kind.to_string()
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
    if let Some(files) = report.scala_oracle_files {
        println!("  Scala oracle files: {files}");
    }
    if let Some(failures) = report.scala_oracle_failures {
        println!("  Scala oracle failures: {failures}");
    }
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
    if let Some(namer) = &report.namer {
        println!(
            "  files naming was prevented by scanning: {}",
            namer.files_parse_prevented_naming
        );
        println!("  files the namer processed: {}", namer.files_namer_ran);
        println!("  namer successes: {}", namer.files_namer_succeeded);
        println!("  typed namer errors: {}", namer.files_namer_returned_error);
        println!(
            "  successes after parser recovery: {}",
            namer.files_named_after_parser_recovery
        );
        println!(
            "  files with semantic invariant failures: {}",
            namer.files_with_invariant_failures
        );
        for (kind, bucket) in &namer.namer_error_histogram {
            println!("    {kind}: {}", bucket.count);
            for example in &bucket.examples {
                println!("      - {}: {}", example.path, example.message);
            }
        }
        for (invariant, bucket) in &namer.invariant_failure_histogram {
            println!("    invariant {invariant}: {}", bucket.count);
            for example in &bucket.examples {
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
        let parsed = parse_source("object C", false);
        assert!(matches!(parsed.status, Status::Clean));
        assert!(parsed.diagnostics.is_empty());
    }

    #[test]
    fn namer_runs_on_a_clean_compilation_unit() {
        let parsed = parse_source("object C", true);

        assert!(matches!(parsed.status, Status::Clean));
        assert!(matches!(parsed.namer, Some(NamerOutcome::Success { .. })));
    }

    #[test]
    fn namer_runs_on_a_recovered_compilation_unit() {
        let parsed = parse_source("object C { def = }", true);

        assert!(matches!(parsed.status, Status::RecoverableDiagnostics));
        assert!(parsed.namer.is_some());
    }

    #[test]
    fn report_counts_recovered_namer_successes_and_typed_errors_separately() {
        let outcomes = [
            FileOutcome {
                path: "clean.scala".to_owned(),
                status: Status::Clean,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: Some(NamerOutcome::Success {
                    invariant_violations: vec!["scope owner mismatch".to_owned()],
                }),
            },
            FileOutcome {
                path: "recovered.scala".to_owned(),
                status: Status::RecoverableDiagnostics,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: Some(NamerOutcome::Success {
                    invariant_violations: Vec::new(),
                }),
            },
            FileOutcome {
                path: "failed.scala".to_owned(),
                status: Status::Clean,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: Some(NamerOutcome::Error {
                    kind: "MalformedAstShape".to_owned(),
                    message: "bad shape".to_owned(),
                }),
            },
        ];
        let report = build_report(&outcomes, &[], None, None, None, None, true);
        let namer = report.namer.expect("namer report enabled");

        assert_eq!(namer.files_namer_ran, 3);
        assert_eq!(namer.files_namer_succeeded, 2);
        assert_eq!(namer.files_namer_returned_error, 1);
        assert_eq!(namer.files_named_after_parser_recovery, 1);
        assert_eq!(namer.namer_error_histogram["MalformedAstShape"].count, 1);
        assert_eq!(namer.files_with_invariant_failures, 1);
        assert_eq!(
            namer.invariant_failure_histogram["scope owner mismatch"].count,
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_the_worker_process_before_returning() {
        let mut command = Command::new("sleep");
        command.arg("30").stdout(Stdio::piped());
        let started = Instant::now();

        let result = run_command_with_timeout(command, Duration::from_millis(20))
            .expect("spawn and reap the worker process");

        assert!(matches!(result, CommandResult::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn unsupported_diagnostic_bucket_is_normalized() {
        let bucket = first_failure_bucket(&DiagnosticSummary {
            kind: "UnsupportedSyntax".to_owned(),
            message: "unsupported `class`  syntax".to_owned(),
        });
        assert_eq!(bucket, "UnsupportedSyntax: unsupported class syntax");
    }

    #[test]
    fn process_failure_is_not_counted_as_parser_panic() {
        let outcome = process_failure(
            "broken.scala".to_owned(),
            "WorkerProtocol",
            "invalid worker output",
        );
        let report = build_report(&[outcome], &[], None, None, None, None, false);

        assert_eq!(report.hard_parser_failures, 1);
        assert_eq!(report.panics, 0);
    }

    #[test]
    fn parser_panic_is_counted_separately_from_process_failure() {
        let outcome = FileOutcome {
            path: "panic.scala".to_owned(),
            status: Status::Panic,
            diagnostics: vec![DiagnosticSummary {
                kind: "Panic".to_owned(),
                message: "parser worker panicked".to_owned(),
            }],
            scanner_diagnostics: 0,
            namer: None,
        };
        let report = build_report(&[outcome], &[], None, None, None, None, false);

        assert_eq!(report.hard_parser_failures, 1);
        assert_eq!(report.panics, 1);
    }

    #[test]
    fn report_preserves_source_and_oracle_metadata() {
        let roots = vec![PathBuf::from("/tmp/scala3/library/src")];
        let report = build_report(
            &[],
            &roots,
            Some("3.9.0".to_owned()),
            Some("revision".to_owned()),
            Some(12),
            Some(1),
            false,
        );

        assert_eq!(report.corpus_roots, vec!["library/src"]);
        assert_eq!(report.source_version.as_deref(), Some("3.9.0"));
        assert_eq!(report.source_revision.as_deref(), Some("revision"));
        assert_eq!(report.scala_oracle_files, Some(12));
        assert_eq!(report.scala_oracle_failures, Some(1));
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "dotty-parser-corpus-report-{name}-{}",
            std::process::id()
        ))
    }
}
