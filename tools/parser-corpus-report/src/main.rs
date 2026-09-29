use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use dotty_core::ast::{AstArena, Modifier, TreeKind, Untyped, UntypedNode};
use dotty_core::{Packages, SemanticStore, SourceId, SourceSemanticIndex, SourceText, TokenKind};
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
    parser_revision: Option<String>,
    oracle_files: Option<usize>,
    oracle_failures: Option<usize>,
    namer: bool,
}

struct ReportMetadata<'a> {
    roots: &'a [PathBuf],
    source_version: Option<String>,
    source_revision: Option<String>,
    parser_revision: Option<String>,
    oracle_files: Option<usize>,
    oracle_failures: Option<usize>,
    collect_namer: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    corpus_roots: Vec<String>,
    source_version: Option<String>,
    source_revision: Option<String>,
    parser_revision: Option<String>,
    scala_oracle_files: Option<usize>,
    scala_oracle_failures: Option<usize>,
    files_attempted: usize,
    files_parsed_without_diagnostics: usize,
    files_parsed_with_recoverable_diagnostics: usize,
    hard_parser_failures: usize,
    process_failures: usize,
    panics: usize,
    hangs: usize,
    scanner_diagnostics: usize,
    diagnostic_histogram: BTreeMap<String, usize>,
    first_failure_histogram: BTreeMap<String, FailureBucket>,
    capture_checking_cohorts: BTreeMap<String, ParserCohortReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namer: Option<NamerReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deferred_features: Option<BTreeMap<String, DeferredFeatureBucket>>,
}

#[derive(Debug, Default, Serialize)]
struct ParserCohortReport {
    files: usize,
    clean: usize,
    recoverable: usize,
    hard_failures: usize,
    scanner_diagnostics: usize,
    diagnostic_histogram: BTreeMap<String, usize>,
    first_failure_histogram: BTreeMap<String, FailureBucket>,
    files_with_raw_caret_character: usize,
    files_with_caret_operator_token: usize,
    files_with_parser_capture_syntax: usize,
    raw_caret_without_operator_examples: Vec<String>,
    operator_without_capture_ast_examples: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct NamerReport {
    files_parse_prevented_naming: usize,
    files_namer_ran: usize,
    files_namer_succeeded: usize,
    files_namer_returned_error: usize,
    files_named_after_parser_recovery: usize,
    files_with_namer_errors_after_parser_recovery: usize,
    files_with_invariant_failures: usize,
    failed_calls_with_transaction_residue: usize,
    namer_success_percent: f64,
    namer_typed_error_percent: f64,
    clean_parse_vs_namer_success_delta: isize,
    namer_error_histogram: BTreeMap<String, NamerFailureBucket>,
    invariant_failure_histogram: BTreeMap<String, FailureBucket>,
}

#[derive(Debug, Serialize)]
struct DeferredFeatureBucket {
    files: usize,
    occurrences: usize,
    materialized_occurrences: usize,
    deferred_occurrences: usize,
    examples: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct FeatureCount {
    occurrences: usize,
    materialized: usize,
}

#[derive(Debug, Serialize)]
struct NamerFailureBucket {
    count: usize,
    clean_parse_failures: usize,
    recovered_parse_failures: usize,
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
    deferred_features: BTreeMap<String, FeatureCount>,
    capture_checking_enabled: Option<bool>,
    has_raw_caret_character: bool,
    has_caret_operator_token: bool,
    has_parser_capture_syntax: bool,
}

#[derive(Debug, Serialize, Deserialize)]
enum NamerOutcome {
    Success {
        invariant_violations: Vec<String>,
    },
    Error {
        kind: String,
        message: String,
        transaction_residue: bool,
    },
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
                "usage: dotty-parser-corpus-report --root <dir>... [--output <file>] [--timeout-ms <n>] [--source-version <v>] [--source-revision <sha>] [--parser-revision <sha>] [--oracle-files <n>] [--oracle-failures <n>] [--namer]"
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
        ReportMetadata {
            roots: &options.roots,
            source_version: options.source_version,
            source_revision: options.source_revision,
            parser_revision: options.parser_revision,
            oracle_files: options.oracle_files,
            oracle_failures: options.oracle_failures,
            collect_namer: options.namer,
        },
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
    let mut parser_revision = None;
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
            "--parser-revision" => {
                parser_revision = Some(next_argument(&mut args, "--parser-revision")?);
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
                    "usage: dotty-parser-corpus-report --root <dir>... [--output <file>] [--timeout-ms <n>] [--source-version <v>] [--source-revision <sha>] [--parser-revision <sha>] [--oracle-files <n>] [--oracle-failures <n>] [--namer]".to_owned(),
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
        parser_revision,
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
                deferred_features: outcome.deferred_features,
                capture_checking_enabled: outcome.capture_checking_enabled,
                has_raw_caret_character: outcome.has_raw_caret_character,
                has_caret_operator_token: outcome.has_caret_operator_token,
                has_parser_capture_syntax: outcome.has_parser_capture_syntax,
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
            deferred_features: BTreeMap::new(),
            capture_checking_enabled: None,
            has_raw_caret_character: false,
            has_caret_operator_token: false,
            has_parser_capture_syntax: false,
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
        deferred_features: BTreeMap::new(),
        capture_checking_enabled: None,
        has_raw_caret_character: false,
        has_caret_operator_token: false,
        has_parser_capture_syntax: false,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerResult {
    status: Status,
    diagnostics: Vec<DiagnosticSummary>,
    scanner_diagnostics: usize,
    namer: Option<NamerOutcome>,
    deferred_features: BTreeMap<String, FeatureCount>,
    capture_checking_enabled: Option<bool>,
    has_raw_caret_character: bool,
    has_caret_operator_token: bool,
    has_parser_capture_syntax: bool,
}

fn run_worker(path: Option<&String>, namer: bool) -> io::Result<()> {
    let path = path.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing path"))?;
    let source_file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Corpus.scala");
    let result = match fs::read_to_string(path) {
        Ok(source) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                parse_source(&source, source_file_name, namer)
            })) {
                Ok(parsed) => WorkerResult {
                    status: parsed.status,
                    diagnostics: parsed.diagnostics,
                    scanner_diagnostics: parsed.scanner_diagnostics,
                    namer: parsed.namer,
                    deferred_features: parsed.deferred_features,
                    capture_checking_enabled: parsed.capture_checking_enabled,
                    has_raw_caret_character: parsed.has_raw_caret_character,
                    has_caret_operator_token: parsed.has_caret_operator_token,
                    has_parser_capture_syntax: parsed.has_parser_capture_syntax,
                },
                Err(_) => WorkerResult {
                    status: Status::Panic,
                    diagnostics: vec![DiagnosticSummary {
                        kind: "Panic".to_owned(),
                        message: "parser worker panicked".to_owned(),
                    }],
                    scanner_diagnostics: 0,
                    namer: None,
                    deferred_features: BTreeMap::new(),
                    capture_checking_enabled: None,
                    has_raw_caret_character: false,
                    has_caret_operator_token: false,
                    has_parser_capture_syntax: false,
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
            deferred_features: BTreeMap::new(),
            capture_checking_enabled: None,
            has_raw_caret_character: false,
            has_caret_operator_token: false,
            has_parser_capture_syntax: false,
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
    deferred_features: BTreeMap<String, FeatureCount>,
    capture_checking_enabled: Option<bool>,
    has_raw_caret_character: bool,
    has_caret_operator_token: bool,
    has_parser_capture_syntax: bool,
}

fn parse_source(source: &str, source_file_name: &str, run_namer: bool) -> ParsedSource {
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
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: None,
                has_raw_caret_character: source.contains('^'),
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
            };
        }
    };
    let scanner_diagnostics = scanner.diagnostics().len();
    let has_caret_operator_token = scanner.tokens().iter().any(|token| {
        token.kind == TokenKind::Operator
            && source.get(token.span.start() as usize..token.span.end() as usize) == Some("^")
    });
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
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: None,
                has_raw_caret_character: source.contains('^'),
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
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
    let capture_checking_enabled = Some(result.effective_features.capture_checking);
    let has_parser_capture_syntax = has_capture_syntax_nodes(&result.ast, &store.names, source);
    let status = if diagnostics.is_empty() && scanner_diagnostics == 0 {
        Status::Clean
    } else {
        Status::RecoverableDiagnostics
    };
    let mut deferred_features = BTreeMap::new();
    let namer = run_namer.then(|| {
        let mut packages = Packages::new();
        let store_checkpoint = store.checkpoint();
        let package_mark = packages.mark();
        match name_compilation_unit(
            &result.ast,
            result.root,
            SourceId::from_index(0),
            source_file_name,
            &mut store,
            &mut packages,
        ) {
            Ok(index) => {
                deferred_features =
                    collect_deferred_features(&result.ast, &diagnostics, Some((&index, &store)));
                NamerOutcome::Success {
                    invariant_violations: index.validate(&store, &packages),
                }
            }
            Err(error) => {
                deferred_features = collect_deferred_features(&result.ast, &diagnostics, None);
                NamerOutcome::Error {
                    kind: namer_error_kind(&error),
                    message: error.to_string(),
                    transaction_residue: store.checkpoint() != store_checkpoint
                        || packages.mark() != package_mark,
                }
            }
        }
    });
    ParsedSource {
        status,
        diagnostics,
        scanner_diagnostics,
        namer,
        deferred_features,
        capture_checking_enabled,
        has_raw_caret_character: source.contains('^'),
        has_caret_operator_token,
        has_parser_capture_syntax,
    }
}

fn has_capture_syntax_nodes(
    arena: &AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    source: &str,
) -> bool {
    arena.iter().any(|(_, tree)| match &tree.kind {
        TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(_)) => true,
        TreeKind::Annotated(annotation) => {
            let TreeKind::New(new) = &arena.get(annotation.annotation).kind else {
                return false;
            };
            let is_capture_marker = matches!(
                type_name_path(arena, new.tpt, names).as_deref(),
                Some(
                    "scala.annotation.retains"
                        | "scala.annotation.retainsCap"
                        | "scala.annotation.internal.reachCapability"
                        | "scala.annotation.internal.onlyCapability"
                )
            );
            is_capture_marker
                && tree.position.is_some_and(|position| {
                    let range = position.span().range();
                    source
                        .get(range.start() as usize..range.end() as usize)
                        .is_some_and(|spelling| spelling.contains('^'))
                })
        }
        _ => false,
    })
}

fn type_name_path(
    arena: &AstArena<Untyped>,
    tree_id: dotty_core::TreeId<Untyped>,
    names: &dotty_core::NameInterner,
) -> Option<String> {
    match &arena.get(tree_id).kind {
        TreeKind::Ident(ident) => Some(names.resolve(ident.name.text()).to_owned()),
        TreeKind::Select(select) => Some(format!(
            "{}.{}",
            type_name_path(arena, select.qualifier, names)?,
            names.resolve(select.name.text())
        )),
        TreeKind::AppliedTypeTree(applied) => type_name_path(arena, applied.tpt, names),
        _ => None,
    }
}

fn collect_deferred_features(
    arena: &AstArena<Untyped>,
    diagnostics: &[DiagnosticSummary],
    named: Option<(&SourceSemanticIndex, &SemanticStore)>,
) -> BTreeMap<String, FeatureCount> {
    fn record(
        features: &mut BTreeMap<String, FeatureCount>,
        name: &str,
        occurrences: usize,
        materialized: impl Fn(usize) -> bool,
    ) {
        let bucket = features.entry(name.to_owned()).or_default();
        bucket.occurrences += occurrences;
        for index in 0..occurrences {
            bucket.materialized += usize::from(materialized(index));
        }
    }

    let mut features = BTreeMap::new();
    for name in [
        "enum_definitions",
        "enum_cases",
        "case_class_synthetic_apis",
        "context_bound_evidence_synthesis",
        "export_forwarders",
        "derives_clauses",
        "local_definitions",
        "source_annotations",
    ] {
        features.entry(name.to_owned()).or_default();
    }
    let source = SourceId::from_index(0);
    let method_rhs_ranges = arena
        .iter()
        .filter_map(|(_, tree)| match &tree.kind {
            TreeKind::DefDef(definition) => definition.rhs,
            _ => None,
        })
        .filter_map(|rhs| arena.get(rhs).position.map(|span| span.span().range()))
        .collect::<Vec<_>>();

    for (tree_id, tree) in arena.iter() {
        let has_symbol = named.is_some_and(|(index, _)| index.symbol_at(source, tree_id).is_some());
        match &tree.kind {
            TreeKind::TypeDef(definition) => {
                if definition.metadata.modifiers.contains(&Modifier::Enum) {
                    record(&mut features, "enum_definitions", 1, |_| has_symbol);
                }
                if definition.metadata.modifiers.contains(&Modifier::EnumCase) {
                    record(&mut features, "enum_cases", 1, |_| has_symbol);
                }
                if definition.metadata.modifiers.contains(&Modifier::Case)
                    && matches!(arena.get(definition.rhs).kind, TreeKind::Template(_))
                {
                    record(&mut features, "case_class_synthetic_apis", 1, |_| false);
                }
                record(
                    &mut features,
                    "source_annotations",
                    definition.metadata.annotations.len(),
                    |_| false,
                );
            }
            TreeKind::ValDef(definition) => record(
                &mut features,
                "source_annotations",
                definition.metadata.annotations.len(),
                |_| false,
            ),
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
                if definition.modifiers.modifiers.contains(&Modifier::EnumCase) {
                    for case in &definition.patterns {
                        let mapped = named
                            .is_some_and(|(index, _)| index.symbol_at(source, *case).is_some());
                        record(&mut features, "enum_cases", 1, |_| mapped);
                    }
                }
                record(
                    &mut features,
                    "source_annotations",
                    definition.modifiers.annotations.len(),
                    |_| false,
                );
            }
            TreeKind::DefDef(definition) => {
                record(
                    &mut features,
                    "source_annotations",
                    definition.metadata.annotations.len(),
                    |_| false,
                );
            }
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(module)) => {
                if module.metadata.modifiers.contains(&Modifier::EnumCase) {
                    record(&mut features, "enum_cases", 1, |_| has_symbol);
                }
                record(
                    &mut features,
                    "source_annotations",
                    module.metadata.annotations.len(),
                    |_| false,
                );
            }
            TreeKind::Template(template) => {
                record(
                    &mut features,
                    "derives_clauses",
                    template.metadata.derives.len(),
                    |_| false,
                );
            }
            TreeKind::Export(export) => {
                record(
                    &mut features,
                    "export_forwarders",
                    export.selectors.len(),
                    |_| false,
                );
            }
            TreeKind::PhaseSpecific(UntypedNode::ContextBounds(bounds)) => {
                record(
                    &mut features,
                    "context_bound_evidence_synthesis",
                    bounds.context_bounds.len(),
                    |count_index| {
                        named.is_some_and(|(source_index, _)| {
                            bounds
                                .context_bounds
                                .get(count_index)
                                .is_some_and(|tree| source_index.symbol_at(source, *tree).is_some())
                        })
                    },
                );
            }
            _ => {}
        }

        if matches!(
            &tree.kind,
            TreeKind::ValDef(_)
                | TreeKind::DefDef(_)
                | TreeKind::TypeDef(_)
                | TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_))
        ) && tree.position.is_some_and(|span| {
            let position = span.span().range();
            method_rhs_ranges
                .iter()
                .any(|rhs| rhs.start() <= position.start() && position.end() <= rhs.end())
        }) {
            record(&mut features, "local_definitions", 1, |_| has_symbol);
        }
    }

    for diagnostic in diagnostics {
        if diagnostic.kind == "UnsupportedSyntax" {
            let bucket = format!("parser_blocked: {}", normalize_message(&diagnostic.message));
            record(&mut features, &bucket, 1, |_| false);
        }
    }

    features
}

fn namer_error_kind(error: &dotty_namer::NamerError) -> String {
    match error {
        dotty_namer::NamerError::RootIsNotPackage { .. } => "RootIsNotPackage",
        dotty_namer::NamerError::DuplicateSourceTreeSymbol { .. } => "DuplicateSourceTreeSymbol",
        dotty_namer::NamerError::DuplicateDerivedSourceTreeSymbol { .. } => {
            "DuplicateDerivedSourceTreeSymbol"
        }
        dotty_namer::NamerError::ConflictingSourceProvenance { .. } => {
            "ConflictingSourceProvenance"
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

fn build_report(outcomes: &[FileOutcome], metadata: ReportMetadata<'_>) -> Report {
    let mut report = Report {
        schema_version: 4,
        corpus_roots: metadata.roots.iter().map(|root| root_label(root)).collect(),
        source_version: metadata.source_version,
        source_revision: metadata.source_revision,
        parser_revision: metadata.parser_revision,
        scala_oracle_files: metadata.oracle_files,
        scala_oracle_failures: metadata.oracle_failures,
        files_attempted: outcomes.len(),
        files_parsed_without_diagnostics: 0,
        files_parsed_with_recoverable_diagnostics: 0,
        hard_parser_failures: 0,
        process_failures: 0,
        panics: 0,
        hangs: 0,
        scanner_diagnostics: 0,
        diagnostic_histogram: BTreeMap::new(),
        first_failure_histogram: BTreeMap::new(),
        capture_checking_cohorts: BTreeMap::new(),
        namer: metadata.collect_namer.then(NamerReport::default),
        deferred_features: metadata.collect_namer.then(|| {
            [
                "enum_definitions",
                "enum_cases",
                "case_class_synthetic_apis",
                "context_bound_evidence_synthesis",
                "export_forwarders",
                "derives_clauses",
                "local_definitions",
                "source_annotations",
            ]
            .into_iter()
            .map(|name| {
                (
                    name.to_owned(),
                    DeferredFeatureBucket {
                        files: 0,
                        occurrences: 0,
                        materialized_occurrences: 0,
                        deferred_occurrences: 0,
                        examples: Vec::new(),
                    },
                )
            })
            .collect()
        }),
    };

    for outcome in outcomes {
        let cohort_name = match outcome.capture_checking_enabled {
            Some(true) => "enabled",
            Some(false) => "disabled",
            None => "unknown",
        };
        let cohort = report
            .capture_checking_cohorts
            .entry(cohort_name.to_owned())
            .or_default();
        cohort.files += 1;
        cohort.scanner_diagnostics += outcome.scanner_diagnostics;
        cohort.files_with_raw_caret_character += usize::from(outcome.has_raw_caret_character);
        cohort.files_with_caret_operator_token += usize::from(outcome.has_caret_operator_token);
        cohort.files_with_parser_capture_syntax += usize::from(outcome.has_parser_capture_syntax);
        if outcome.has_raw_caret_character
            && !outcome.has_caret_operator_token
            && cohort.raw_caret_without_operator_examples.len() < 20
        {
            cohort
                .raw_caret_without_operator_examples
                .push(outcome.path.clone());
        }
        if outcome.has_caret_operator_token
            && !outcome.has_parser_capture_syntax
            && cohort.operator_without_capture_ast_examples.len() < 20
        {
            cohort
                .operator_without_capture_ast_examples
                .push(outcome.path.clone());
        }
        match outcome.status {
            Status::Clean => cohort.clean += 1,
            Status::RecoverableDiagnostics => cohort.recoverable += 1,
            Status::ScannerFailure | Status::ProcessFailure | Status::Panic | Status::Hang => {
                cohort.hard_failures += 1;
            }
        }
        for diagnostic in &outcome.diagnostics {
            *cohort
                .diagnostic_histogram
                .entry(diagnostic.kind.clone())
                .or_default() += 1;
        }
        if let Some(diagnostic) = outcome.diagnostics.first() {
            let bucket = first_failure_bucket(diagnostic);
            let entry = cohort
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
            let entry = cohort
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

        if let Some(features) = &mut report.deferred_features {
            for (name, counts) in &outcome.deferred_features {
                if counts.occurrences == 0 {
                    continue;
                }
                let bucket =
                    features
                        .entry(name.clone())
                        .or_insert_with(|| DeferredFeatureBucket {
                            files: 0,
                            occurrences: 0,
                            materialized_occurrences: 0,
                            deferred_occurrences: 0,
                            examples: Vec::new(),
                        });
                bucket.files += 1;
                bucket.occurrences += counts.occurrences;
                bucket.materialized_occurrences += counts.materialized;
                if matches!(&outcome.namer, Some(NamerOutcome::Success { .. }))
                    && !name.starts_with("parser_blocked:")
                {
                    bucket.deferred_occurrences += counts.occurrences - counts.materialized;
                }
                if bucket.examples.len() < 5 {
                    bucket.examples.push(outcome.path.clone());
                }
            }
        }
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
                Some(NamerOutcome::Error {
                    kind,
                    message,
                    transaction_residue,
                }) => {
                    namer.files_namer_ran += 1;
                    namer.files_namer_returned_error += 1;
                    if *transaction_residue {
                        namer.failed_calls_with_transaction_residue += 1;
                    }
                    let entry = namer
                        .namer_error_histogram
                        .entry(kind.clone())
                        .or_insert_with(|| NamerFailureBucket {
                            count: 0,
                            clean_parse_failures: 0,
                            recovered_parse_failures: 0,
                            examples: Vec::new(),
                        });
                    entry.count += 1;
                    if matches!(outcome.status, Status::RecoverableDiagnostics) {
                        entry.recovered_parse_failures += 1;
                        namer.files_with_namer_errors_after_parser_recovery += 1;
                    } else if matches!(outcome.status, Status::Clean) {
                        entry.clean_parse_failures += 1;
                    }
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
                if matches!(outcome.status, Status::ProcessFailure) {
                    report.process_failures += 1;
                }
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
    if let Some(namer) = &mut report.namer {
        let total = namer.files_namer_ran as f64;
        namer.namer_success_percent = if total == 0.0 {
            0.0
        } else {
            namer.files_namer_succeeded as f64 * 100.0 / total
        };
        namer.namer_typed_error_percent = if total == 0.0 {
            0.0
        } else {
            namer.files_namer_returned_error as f64 * 100.0 / total
        };
        namer.clean_parse_vs_namer_success_delta =
            namer.files_namer_succeeded as isize - report.files_parsed_without_diagnostics as isize;
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
    if let Some(revision) = &report.parser_revision {
        println!("  dotty-rs revision: {revision}");
    }
    if let Some(revision) = &report.source_revision {
        println!("  Scala source revision: {revision}");
    }
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
    println!("  process failures: {}", report.process_failures);
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
    for (policy, cohort) in &report.capture_checking_cohorts {
        println!(
            "  capture-checking {policy}: {} files ({} clean, {} recoverable, {} hard failures)",
            cohort.files, cohort.clean, cohort.recoverable, cohort.hard_failures
        );
        println!(
            "    raw ^ marker: {}, lexer ^ token: {}, parser-confirmed capture syntax: {}",
            cohort.files_with_raw_caret_character,
            cohort.files_with_caret_operator_token,
            cohort.files_with_parser_capture_syntax
        );
        for path in &cohort.raw_caret_without_operator_examples {
            println!("    raw-only ^ example: {path}");
        }
        for path in &cohort.operator_without_capture_ast_examples {
            println!("    unconfirmed ^ token example: {path}");
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
            "  typed namer errors after parser recovery: {}",
            namer.files_with_namer_errors_after_parser_recovery
        );
        println!("  namer success rate: {:.2}%", namer.namer_success_percent);
        println!(
            "  typed-error rate: {:.2}%",
            namer.namer_typed_error_percent
        );
        println!(
            "  namer successes minus clean parses: {}",
            namer.clean_parse_vs_namer_success_delta
        );
        println!(
            "  successes after parser recovery: {}",
            namer.files_named_after_parser_recovery
        );
        println!(
            "  files with semantic invariant failures: {}",
            namer.files_with_invariant_failures
        );
        println!(
            "  failed calls with transaction residue: {}",
            namer.failed_calls_with_transaction_residue
        );
        for (kind, bucket) in &namer.namer_error_histogram {
            println!(
                "    {kind}: {} ({} clean parse, {} recovered parse)",
                bucket.count, bucket.clean_parse_failures, bucket.recovered_parse_failures
            );
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
    if let Some(features) = &report.deferred_features {
        println!("  deferred source-feature inventory:");
        for (feature, bucket) in features {
            println!(
                "    {feature}: {} occurrences in {} files ({} materialized, {} deferred)",
                bucket.occurrences,
                bucket.files,
                bucket.materialized_occurrences,
                bucket.deferred_occurrences
            );
            for example in &bucket.examples {
                println!("      - {example}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_report_metadata(collect_namer: bool) -> ReportMetadata<'static> {
        ReportMetadata {
            roots: &[],
            source_version: None,
            source_revision: None,
            parser_revision: None,
            oracle_files: None,
            oracle_failures: None,
            collect_namer,
        }
    }

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
        let parsed = parse_source("object C", "Test.scala", false);
        assert!(matches!(parsed.status, Status::Clean));
        assert!(parsed.diagnostics.is_empty());
        assert_eq!(parsed.capture_checking_enabled, Some(false));
    }

    #[test]
    fn corpus_worker_observes_capture_policy_enabled_by_global_import() {
        let parsed = parse_source(
            "import language.experimental.captureChecking\ntype F = A -> {cap} B",
            "Capture.scala",
            false,
        );

        assert_eq!(parsed.capture_checking_enabled, Some(true));
    }

    #[test]
    fn corpus_worker_recognizes_capture_syntax_in_the_ast() {
        let parsed = parse_source(
            "import language.experimental.captureChecking\ntype F = A -> {cap} B\ntype R = T^{cap}",
            "Capture.scala",
            false,
        );

        assert_eq!(parsed.capture_checking_enabled, Some(true));
        assert!(parsed.has_caret_operator_token);
        assert!(parsed.has_parser_capture_syntax);
    }

    #[test]
    fn caret_characters_in_comments_and_strings_are_not_capture_syntax_tokens() {
        let parsed = parse_source("// ^\nval marker = \"^\"", "Marker.scala", false);

        assert!(parsed.has_raw_caret_character);
        assert!(!parsed.has_caret_operator_token);
        assert!(!parsed.has_parser_capture_syntax);
    }

    #[test]
    fn explicit_retains_annotation_is_not_counted_as_capture_syntax() {
        let parsed = parse_source(
            "import language.experimental.captureChecking\ntype T = A @scala.annotation.retains",
            "ExplicitAnnotation.scala",
            false,
        );

        assert!(!parsed.has_parser_capture_syntax);
    }

    #[test]
    fn namer_runs_on_a_clean_compilation_unit() {
        let parsed = parse_source("object C", "C.scala", true);

        assert!(matches!(parsed.status, Status::Clean));
        assert!(matches!(parsed.namer, Some(NamerOutcome::Success { .. })));
    }

    #[test]
    fn namer_runs_on_a_recovered_compilation_unit() {
        let parsed = parse_source("object C { def = }", "C.scala", true);

        assert!(matches!(parsed.status, Status::RecoverableDiagnostics));
        assert!(parsed.namer.is_some());
    }

    #[test]
    fn conflicting_source_provenance_has_a_typed_report_bucket() {
        let mut store = SemanticStore::new();
        let symbol = Packages::new().enter(&mut store, dotty_core::SymbolOrigin::Synthetic, &["p"])
            [0]
        .symbol;
        let mut arena = AstArena::<Untyped>::new();
        let tree = arena.alloc(dotty_core::Tree {
            kind: TreeKind::PhaseSpecific(UntypedNode::Error(dotty_core::ast::ErrorNode {
                kind: dotty_core::ast::ErrorNodeKind::UnexpectedToken,
            })),
            position: None,
            ty: (),
        });
        let source = SourceId::from_index(1);
        let error = dotty_namer::NamerError::ConflictingSourceProvenance {
            symbol,
            existing: dotty_core::SourceDefinition::Canonical { source, tree },
            attempted: dotty_core::SourceDefinition::Derived { source, tree },
        };

        assert_eq!(namer_error_kind(&error), "ConflictingSourceProvenance");
    }

    #[test]
    fn deferred_inventory_counts_materialized_enum_case_identities() {
        let parsed = parse_source("enum Color { case Red, Green }", "Color.scala", true);
        let enum_definition = parsed
            .deferred_features
            .get("enum_definitions")
            .expect("enum definition counted");
        let enum_cases = parsed
            .deferred_features
            .get("enum_cases")
            .expect("enum cases counted");

        assert_eq!(enum_definition.occurrences, 1);
        assert_eq!(enum_definition.materialized, 1);
        assert_eq!(enum_cases.occurrences, 2);
        assert_eq!(enum_cases.materialized, 2);
    }

    #[test]
    fn deferred_inventory_records_case_class_synthetic_api_gap() {
        let parsed = parse_source("case class Box(value: Int)", "Box.scala", true);
        let cases = parsed
            .deferred_features
            .get("case_class_synthetic_apis")
            .expect("case class synthetic API recorded");

        assert_eq!(cases.occurrences, 1);
        assert_eq!(cases.materialized, 0);
    }

    #[test]
    fn deferred_inventory_counts_context_bound_evidence() {
        let parsed = parse_source(
            "object C { type F = [A: Ordering] => A => A }",
            "C.scala",
            true,
        );
        let evidence = parsed.deferred_features.get("context_bound_evidence_synthesis").unwrap_or_else(|| {
            panic!("context bound evidence not counted: status={:?}, diagnostics={:?}, features={:?}", parsed.status, parsed.diagnostics, parsed.deferred_features)
        });

        assert_eq!(
            evidence.occurrences, 1,
            "status={:?}, diagnostics={:?}, features={:?}",
            parsed.status, parsed.diagnostics, parsed.deferred_features
        );
        assert_eq!(evidence.materialized, 0);
    }

    #[test]
    fn deferred_inventory_counts_each_context_bound_evidence_candidate() {
        let parsed = parse_source(
            "object C { type F = [A: {Ordering, Show}] => A => A }",
            "C.scala",
            true,
        );
        let evidence = parsed
            .deferred_features
            .get("context_bound_evidence_synthesis")
            .expect("context-bound evidence candidates counted");

        assert_eq!(evidence.occurrences, 2);
        assert_eq!(evidence.materialized, 0);
    }

    #[test]
    fn deferred_inventory_counts_derives_metadata() {
        let parsed = parse_source("class C derives CanEqual", "C.scala", true);
        let derives = parsed
            .deferred_features
            .get("derives_clauses")
            .expect("derives metadata counted");

        assert_eq!(derives.occurrences, 1);
        assert_eq!(derives.materialized, 0);
    }

    #[test]
    fn deferred_inventory_counts_export_selectors_as_missing_forwarders() {
        let parsed = parse_source(
            "object A { def value: Int = 1 }; object B { export A.value }",
            "Exports.scala",
            true,
        );
        let exports = parsed
            .deferred_features
            .get("export_forwarders")
            .expect("export forwarder occurrence counted");

        assert_eq!(exports.occurrences, 1);
        assert_eq!(exports.materialized, 0);
    }

    #[test]
    fn deferred_inventory_counts_local_definitions_inside_method_bodies() {
        let parsed = parse_source(
            "object C { def outer: Int = { val local = 1; def inner: Int = local; inner } }",
            "C.scala",
            true,
        );
        let locals = parsed
            .deferred_features
            .get("local_definitions")
            .expect("local definitions counted");

        assert_eq!(locals.occurrences, 2);
        assert_eq!(locals.materialized, 0);
    }

    #[test]
    fn deferred_inventory_counts_source_annotations_not_completed_by_namer() {
        let parsed = parse_source(
            "@deprecated(\"use newer\", \"3.9\") class C",
            "C.scala",
            true,
        );
        let annotations = parsed
            .deferred_features
            .get("source_annotations")
            .expect("source annotation counted");

        assert_eq!(annotations.occurrences, 1);
        assert_eq!(annotations.materialized, 0);
    }

    #[test]
    fn package_objects_are_no_longer_counted_as_parser_blockers() {
        let parsed = parse_source("package object syntax { val x = 1 }", "package.scala", true);
        assert!(parsed.diagnostics.is_empty());
        assert!(
            !parsed
                .deferred_features
                .contains_key("package_objects_blocked_by_parser")
        );
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
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: Some(false),
                has_raw_caret_character: false,
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
            },
            FileOutcome {
                path: "recovered.scala".to_owned(),
                status: Status::RecoverableDiagnostics,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: Some(NamerOutcome::Success {
                    invariant_violations: Vec::new(),
                }),
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: Some(true),
                has_raw_caret_character: false,
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
            },
            FileOutcome {
                path: "failed.scala".to_owned(),
                status: Status::RecoverableDiagnostics,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: Some(NamerOutcome::Error {
                    kind: "MalformedAstShape".to_owned(),
                    message: "bad shape".to_owned(),
                    transaction_residue: false,
                }),
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: None,
                has_raw_caret_character: false,
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
            },
        ];
        let report = build_report(&outcomes, test_report_metadata(true));
        assert!(
            report
                .deferred_features
                .as_ref()
                .expect("feature inventory enabled")
                .contains_key("context_bound_evidence_synthesis")
        );
        let namer = report.namer.expect("namer report enabled");

        assert_eq!(namer.files_namer_ran, 3);
        assert_eq!(namer.files_namer_succeeded, 2);
        assert_eq!(namer.files_namer_returned_error, 1);
        assert_eq!(namer.files_named_after_parser_recovery, 1);
        assert_eq!(namer.files_with_namer_errors_after_parser_recovery, 1);
        assert_eq!(namer.namer_success_percent, 200.0 / 3.0);
        assert_eq!(namer.clean_parse_vs_namer_success_delta, 1);
        assert_eq!(namer.namer_error_histogram["MalformedAstShape"].count, 1);
        assert_eq!(
            namer.namer_error_histogram["MalformedAstShape"].clean_parse_failures,
            0
        );
        assert_eq!(
            namer.namer_error_histogram["MalformedAstShape"].recovered_parse_failures,
            1
        );
        assert_eq!(namer.files_with_invariant_failures, 1);
        assert_eq!(namer.failed_calls_with_transaction_residue, 0);
        assert_eq!(
            namer.invariant_failure_histogram["scope owner mismatch"].count,
            1
        );
    }

    #[test]
    fn report_partitions_files_by_effective_capture_checking_policy() {
        let outcomes = [
            FileOutcome {
                path: "enabled.scala".to_owned(),
                status: Status::Clean,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: None,
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: Some(true),
                has_raw_caret_character: false,
                has_caret_operator_token: true,
                has_parser_capture_syntax: true,
            },
            FileOutcome {
                path: "candidate.scala".to_owned(),
                status: Status::RecoverableDiagnostics,
                diagnostics: vec![DiagnosticSummary {
                    kind: "ExpectedType".to_owned(),
                    message: "expected a type".to_owned(),
                }],
                scanner_diagnostics: 0,
                namer: None,
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: Some(false),
                has_raw_caret_character: true,
                has_caret_operator_token: true,
                has_parser_capture_syntax: false,
            },
            FileOutcome {
                path: "comment.scala".to_owned(),
                status: Status::Clean,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: None,
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: Some(false),
                has_raw_caret_character: true,
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
            },
            process_failure("unknown.scala".to_owned(), "WorkerError", "failed"),
        ];

        let report = build_report(&outcomes, test_report_metadata(false));

        assert_eq!(report.capture_checking_cohorts["enabled"].files, 1);
        assert_eq!(report.capture_checking_cohorts["enabled"].clean, 1);
        assert_eq!(
            report.capture_checking_cohorts["enabled"].files_with_caret_operator_token,
            1
        );
        assert_eq!(
            report.capture_checking_cohorts["enabled"].files_with_parser_capture_syntax,
            1
        );
        assert_eq!(report.capture_checking_cohorts["disabled"].files, 2);
        assert_eq!(report.capture_checking_cohorts["disabled"].recoverable, 1);
        assert_eq!(report.capture_checking_cohorts["disabled"].clean, 1);
        assert_eq!(
            report.capture_checking_cohorts["disabled"].diagnostic_histogram["ExpectedType"],
            1
        );
        assert_eq!(
            report.capture_checking_cohorts["disabled"].operator_without_capture_ast_examples,
            ["candidate.scala"]
        );
        assert_eq!(
            report.capture_checking_cohorts["disabled"].raw_caret_without_operator_examples,
            ["comment.scala"]
        );
        assert_eq!(report.capture_checking_cohorts["unknown"].hard_failures, 1);
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
        let report = build_report(&[outcome], test_report_metadata(false));

        assert_eq!(report.hard_parser_failures, 1);
        assert_eq!(report.process_failures, 1);
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
            deferred_features: BTreeMap::new(),
            capture_checking_enabled: None,
            has_raw_caret_character: false,
            has_caret_operator_token: false,
            has_parser_capture_syntax: false,
        };
        let report = build_report(&[outcome], test_report_metadata(false));

        assert_eq!(report.hard_parser_failures, 1);
        assert_eq!(report.panics, 1);
    }

    #[test]
    fn report_preserves_source_and_oracle_metadata() {
        let roots = vec![PathBuf::from("/tmp/scala3/library/src")];
        let report = build_report(
            &[],
            ReportMetadata {
                roots: &roots,
                source_version: Some("3.9.0".to_owned()),
                source_revision: Some("revision".to_owned()),
                parser_revision: Some("parser revision".to_owned()),
                oracle_files: Some(12),
                oracle_failures: Some(1),
                collect_namer: false,
            },
        );

        assert_eq!(report.corpus_roots, vec!["library/src"]);
        assert_eq!(report.source_version.as_deref(), Some("3.9.0"));
        assert_eq!(report.source_revision.as_deref(), Some("revision"));
        assert_eq!(report.parser_revision.as_deref(), Some("parser revision"));
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
