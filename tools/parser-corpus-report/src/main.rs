use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use dotty_core::ast::{AstArena, Modifier, TreeKind, Untyped, UntypedNode};
use dotty_core::{
    HardKeyword, Packages, SemanticStore, SourceId, SourceSemanticIndex, SourceText, SymbolId,
    SymbolKind, TokenKind,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};
use serde::{Deserialize, Serialize};

const DEFAULT_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug)]
struct Options {
    roots: Vec<PathBuf>,
    source_sets: Vec<SourceSetOptions>,
    output: Option<PathBuf>,
    timeout: Duration,
    source_version: Option<String>,
    source_revision: Option<String>,
    parser_revision: Option<String>,
    oracle_files: Option<usize>,
    oracle_failures: Option<usize>,
    namer: bool,
}

#[derive(Debug, Clone)]
struct SourceSetOptions {
    name: String,
    version: String,
    revision: String,
    repository: String,
    roots: Vec<PathBuf>,
    oracle_files: Option<usize>,
    oracle_failures: Option<usize>,
}

struct ReportMetadata<'a> {
    roots: &'a [PathBuf],
    source_sets: &'a [SourceSetOptions],
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
    source_sets: BTreeMap<String, SourceSetReport>,
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

impl Report {
    fn oracle_count_mismatches(&self) -> Vec<String> {
        let mut mismatches = Vec::new();
        if let Some(oracle_files) = self.scala_oracle_files
            && oracle_files != self.files_attempted
        {
            mismatches.push(format!(
                "aggregate has {} Rust files but {oracle_files} Scala oracle results",
                self.files_attempted
            ));
        }
        for (name, source_set) in &self.source_sets {
            if let Some(oracle_files) = source_set.scala_oracle_files
                && oracle_files != source_set.files_attempted
            {
                mismatches.push(format!(
                    "source set {name} has {} Rust files but {oracle_files} Scala oracle results",
                    source_set.files_attempted
                ));
            }
        }
        mismatches
    }
}

#[derive(Debug, Serialize)]
struct SourceSetReport {
    source_version: String,
    source_revision: String,
    repository: String,
    corpus_roots: Vec<String>,
    scala_oracle_files: Option<usize>,
    scala_oracle_failures: Option<usize>,
    files_attempted: usize,
    files_parsed_without_diagnostics: usize,
    files_parsed_with_recoverable_diagnostics: usize,
    hard_parser_failures: usize,
    scanner_diagnostics: usize,
    diagnostics: usize,
    diagnostic_histogram: BTreeMap<String, usize>,
    first_failure_histogram: BTreeMap<String, FailureBucket>,
}

impl SourceSetReport {
    fn from_options(options: &SourceSetOptions) -> Self {
        Self {
            source_version: options.version.clone(),
            source_revision: options.revision.clone(),
            repository: options.repository.clone(),
            corpus_roots: options.roots.iter().map(|root| root_label(root)).collect(),
            scala_oracle_files: options.oracle_files,
            scala_oracle_failures: options.oracle_failures,
            files_attempted: 0,
            files_parsed_without_diagnostics: 0,
            files_parsed_with_recoverable_diagnostics: 0,
            hard_parser_failures: 0,
            scanner_diagnostics: 0,
            diagnostics: 0,
            diagnostic_histogram: BTreeMap::new(),
            first_failure_histogram: BTreeMap::new(),
        }
    }

    fn record(&mut self, outcome: &FileOutcome) {
        self.files_attempted += 1;
        self.scanner_diagnostics += outcome.scanner_diagnostics;
        self.diagnostics += outcome.diagnostics.len();
        match outcome.status {
            Status::Clean => self.files_parsed_without_diagnostics += 1,
            Status::RecoverableDiagnostics => self.files_parsed_with_recoverable_diagnostics += 1,
            Status::ScannerFailure | Status::ProcessFailure | Status::Panic | Status::Hang => {
                self.hard_parser_failures += 1;
            }
        }
        for diagnostic in &outcome.diagnostics {
            *self
                .diagnostic_histogram
                .entry(diagnostic.kind.clone())
                .or_default() += 1;
        }
        let failure = outcome
            .diagnostics
            .first()
            .map(first_failure_bucket)
            .or_else(|| {
                (outcome.scanner_diagnostics != 0).then(|| "ScannerDiagnostics".to_owned())
            });
        if let Some(failure) = failure {
            let bucket = self
                .first_failure_histogram
                .entry(failure)
                .or_insert_with(|| FailureBucket {
                    count: 0,
                    examples: Vec::new(),
                });
            bucket.count += 1;
            if bucket.examples.len() < 5 {
                bucket.examples.push(outcome.path.clone());
            }
        }
    }
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
    enum_identity_audit: EnumIdentityAudit,
    export_handoff_audit: ExportHandoffAudit,
}

#[derive(Debug, Default, Serialize)]
struct EnumIdentityAudit {
    definitions_encountered: usize,
    class_identities_materialized: usize,
    companion_identity_pairs_materialized: usize,
    definitions_blocked_by_parser_recovery: usize,
    singleton_cases_encountered: usize,
    singleton_cases_materialized: usize,
    comma_group_singleton_cases_encountered: usize,
    comma_group_singleton_cases_materialized: usize,
    parameterized_cases_encountered: usize,
    parameterized_cases_materialized: usize,
    cases_blocked_by_parser_recovery: usize,
}

#[derive(Debug, Default, Serialize)]
struct ExportHandoffAudit {
    syntax_occurrences: usize,
    sites_recorded: usize,
    sites_blocked_by_parser_recovery: usize,
    sites_in_method_bodies: usize,
    forwarders_synthesized: usize,
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
    source_set: Option<String>,
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
        .is_some_and(|argument| argument == "--list-production-roots")
    {
        let Some(root) = arguments.get(1) else {
            eprintln!("--list-production-roots requires a project root");
            std::process::exit(2);
        };
        match discover_production_roots(Path::new(root)) {
            Ok(roots) => {
                for root in roots {
                    println!("{}", root.display());
                }
            }
            Err(error) => {
                eprintln!("failed to discover production roots: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
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
                "usage: dotty-parser-corpus-report [--source-set <name@version@revision@repository-url> --root <dir>...]... [--root <dir>...] [--output <file>] [--timeout-ms <n>] [--source-version <v>] [--source-revision <sha>] [--parser-revision <sha>] [--oracle-files <n>] [--oracle-failures <n>] [--namer]"
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

    let source_set_names = match source_set_names_for_files(&files, &options.source_sets) {
        Ok(names) => names,
        Err(error) => {
            eprintln!("invalid source-set roots: {error}");
            std::process::exit(2);
        }
    };
    let outcomes = parse_files(
        &files,
        &options.roots,
        &source_set_names,
        options.timeout,
        options.namer,
    );
    let report = build_report(
        &outcomes,
        ReportMetadata {
            roots: &options.roots,
            source_sets: &options.source_sets,
            source_version: options.source_version,
            source_revision: options.source_revision,
            parser_revision: options.parser_revision,
            oracle_files: options.oracle_files,
            oracle_failures: options.oracle_failures,
            collect_namer: options.namer,
        },
    );

    let oracle_count_mismatches = report.oracle_count_mismatches();
    if !oracle_count_mismatches.is_empty() {
        for mismatch in oracle_count_mismatches {
            eprintln!("corpus/oracle count mismatch: {mismatch}");
        }
        std::process::exit(1);
    }

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
    let mut source_sets: Vec<SourceSetOptions> = Vec::new();
    let mut active_source_set: Option<usize> = None;
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
            "--root" => {
                let root = PathBuf::from(next_argument(&mut args, "--root")?);
                roots.push(root.clone());
                if let Some(index) = active_source_set {
                    source_sets[index].roots.push(root);
                }
            }
            "--source-set" => {
                let value = next_argument(&mut args, "--source-set")?;
                let mut fields = value.splitn(4, '@');
                let name = fields.next().unwrap_or_default();
                let version = fields.next().unwrap_or_default();
                let revision = fields.next().unwrap_or_default();
                let repository = fields.next().unwrap_or_default();
                if name.is_empty()
                    || version.is_empty()
                    || revision.is_empty()
                    || repository.is_empty()
                {
                    return Err(format!(
                        "invalid source set {value:?}; expected <name>@<version>@<revision>@<repository-url>"
                    ));
                }
                if source_sets
                    .iter()
                    .any(|source_set: &SourceSetOptions| source_set.name == name)
                {
                    return Err(format!("duplicate source-set name: {name}"));
                }
                source_sets.push(SourceSetOptions {
                    name: name.to_owned(),
                    version: version.to_owned(),
                    revision: revision.to_owned(),
                    repository: repository.to_owned(),
                    roots: Vec::new(),
                    oracle_files: None,
                    oracle_failures: None,
                });
                active_source_set = Some(source_sets.len() - 1);
            }
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
                let count = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid oracle file count: {value}"))?;
                if let Some(index) = active_source_set {
                    source_sets[index].oracle_files = Some(count);
                } else {
                    oracle_files = Some(count);
                }
            }
            "--oracle-failures" => {
                let value = next_argument(&mut args, "--oracle-failures")?;
                let count = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid oracle failure count: {value}"))?;
                if let Some(index) = active_source_set {
                    source_sets[index].oracle_failures = Some(count);
                } else {
                    oracle_failures = Some(count);
                }
            }
            "--namer" => namer = true,
            "--help" | "-h" => {
                return Err(
                    "usage: dotty-parser-corpus-report [--source-set <name@version@revision@repository-url> --root <dir>...]... [--root <dir>...] [--output <file>] [--timeout-ms <n>] [--source-version <v>] [--source-revision <sha>] [--parser-revision <sha>] [--oracle-files <n>] [--oracle-failures <n>] [--namer]".to_owned(),
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if roots.is_empty() {
        return Err("at least one --root is required".to_owned());
    }
    for source_set in &source_sets {
        if source_set.roots.is_empty() {
            return Err(format!("source set {} has no --root", source_set.name));
        }
    }
    let roots = roots
        .into_iter()
        .map(|root| fs::canonicalize(&root).map_err(|error| format!("{}: {error}", root.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let source_sets = source_sets
        .into_iter()
        .map(|mut source_set| {
            source_set.roots = source_set
                .roots
                .into_iter()
                .map(|root| {
                    fs::canonicalize(&root).map_err(|error| format!("{}: {error}", root.display()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(source_set)
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Options {
        roots,
        source_sets,
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
    for file in &mut files {
        *file = fs::canonicalize(&*file)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn discover_production_roots(project_root: &Path) -> io::Result<Vec<PathBuf>> {
    fn walk(project_root: &Path, path: &Path, roots: &mut Vec<PathBuf>) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Ok(());
        }
        if !metadata.is_dir() {
            return Ok(());
        }
        if path != project_root && excluded_production_path(project_root, path) {
            return Ok(());
        }
        let is_scala_source_root = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "scala" || name.starts_with("scala-3"))
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "main")
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|name| name == "src");
        if is_scala_source_root {
            roots.push(fs::canonicalize(path)?);
            return Ok(());
        }
        let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            walk(project_root, &entry.path(), roots)?;
        }
        Ok(())
    }

    fn excluded_production_path(project_root: &Path, path: &Path) -> bool {
        path.strip_prefix(project_root)
            .ok()
            .into_iter()
            .flat_map(Path::components)
            .filter_map(|component| component.as_os_str().to_str())
            .any(|component| {
                matches!(
                    component,
                    ".git"
                        | "target"
                        | "test"
                        | "tests"
                        | "docs"
                        | "documentation"
                        | "example"
                        | "examples"
                        | "bench"
                        | "benchmarks"
                        | "scalafix"
                ) || component.ends_with("-example")
            })
    }

    let project_root = fs::canonicalize(project_root)?;
    let mut roots = Vec::new();
    walk(&project_root, &project_root, &mut roots)?;
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn source_set_names_for_files(
    files: &[PathBuf],
    source_sets: &[SourceSetOptions],
) -> Result<Vec<Option<String>>, String> {
    files
        .iter()
        .map(|file| {
            let matching_sets = source_sets
                .iter()
                .filter(|source_set| source_set.roots.iter().any(|root| file.starts_with(root)))
                .map(|source_set| source_set.name.as_str())
                .collect::<Vec<_>>();
            match matching_sets.as_slice() {
                [] => Ok(None),
                [name] => Ok(Some((*name).to_owned())),
                _ => Err(format!(
                    "{} is included in multiple source sets: {}",
                    file.display(),
                    matching_sets.join(", ")
                )),
            }
        })
        .collect()
}

fn collect_scala_files(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
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
    source_set_names: &[Option<String>],
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
    let source_set_names = Arc::new(source_set_names.to_vec());
    let mut workers = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let job_receiver = Arc::clone(&job_receiver);
        let result_sender = result_sender.clone();
        let source_set_names = Arc::clone(&source_set_names);
        let roots = roots.to_vec();
        workers.push(thread::spawn(move || {
            loop {
                let job = job_receiver.lock().expect("job queue lock poisoned").recv();
                let Ok((index, path)) = job else {
                    break;
                };
                let mut outcome = parse_one(&path, &roots, timeout, namer);
                outcome.source_set = source_set_names[index].clone();
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
                source_set: None,
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
            source_set: None,
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
        source_set: None,
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
    let case_keyword_positions = scanner
        .tokens()
        .iter()
        .filter(|token| token.kind == TokenKind::Keyword(HardKeyword::Case))
        .map(|token| token.span.start())
        .collect::<Vec<_>>();
    let export_keyword_positions = scanner
        .tokens()
        .iter()
        .filter(|token| token.kind == TokenKind::Keyword(HardKeyword::Export))
        .map(|token| token.span.start())
        .collect::<Vec<_>>();
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
                deferred_features = collect_deferred_features(
                    &result.ast,
                    &diagnostics,
                    &case_keyword_positions,
                    &export_keyword_positions,
                    Some((&index, &store)),
                );
                NamerOutcome::Success {
                    invariant_violations: {
                        let mut violations = index.validate(&store, &packages);
                        violations.extend(validate_namer_audit_invariants(
                            &result.ast,
                            SourceId::from_index(0),
                            &index,
                            &store,
                            source,
                            !diagnostics.is_empty(),
                        ));
                        violations
                    },
                }
            }
            Err(error) => {
                deferred_features = collect_deferred_features(
                    &result.ast,
                    &diagnostics,
                    &case_keyword_positions,
                    &export_keyword_positions,
                    None,
                );
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
    case_keyword_positions: &[u32],
    export_keyword_positions: &[u32],
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
        "enum_definitions_encountered",
        "enum_class_identities",
        "enum_companion_identity_pairs",
        "enum_definitions_blocked_by_parser_recovery",
        "enum_singleton_cases",
        "enum_comma_group_singleton_cases",
        "enum_parameterized_cases",
        "enum_cases_blocked_by_parser_recovery",
        "case_class_synthetic_apis",
        "context_bound_evidence_synthesis",
        "export_syntax_occurrences",
        "export_sites_recorded",
        "export_sites_blocked_by_parser_recovery",
        "export_sites_in_method_bodies",
        "export_forwarders_synthesized",
        "derives_clauses",
        "local_definitions",
        "source_annotations",
    ] {
        features.entry(name.to_owned()).or_default();
    }
    let source = SourceId::from_index(0);
    let scope_members = collect_scope_members(arena);
    let enum_case_ranges = arena
        .iter()
        .filter_map(|(_, tree)| {
            let is_case = match &tree.kind {
                TreeKind::TypeDef(definition) => {
                    definition.metadata.modifiers.contains(&Modifier::EnumCase)
                }
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)) => {
                    definition.metadata.modifiers.contains(&Modifier::EnumCase)
                }
                TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
                    definition.modifiers.modifiers.contains(&Modifier::EnumCase)
                }
                _ => false,
            };
            (is_case)
                .then(|| tree.position.map(|span| span.span().range()))
                .flatten()
        })
        .map(|range| (range.start(), range.end()))
        .collect::<Vec<_>>();
    let enum_ranges = arena
        .iter()
        .filter_map(|(_, tree)| {
            let is_enum = matches!(&tree.kind, TreeKind::TypeDef(definition)
                if definition.metadata.modifiers.contains(&Modifier::Enum));
            is_enum
                .then(|| tree.position.map(|span| span.span().range()))
                .flatten()
        })
        .map(|range| (range.start(), range.end()))
        .collect::<Vec<_>>();
    let match_case_ranges = arena
        .iter()
        .filter_map(|(_, tree)| {
            matches!(tree.kind, TreeKind::CaseDef(_))
                .then(|| tree.position.map(|span| span.span().range()))
                .flatten()
        })
        .map(|range| (range.start(), range.end()))
        .collect::<Vec<_>>();
    let blocked_enum_case_clauses = case_keyword_positions
        .iter()
        .filter(|&&position| {
            if match_case_ranges
                .iter()
                .any(|&(start, end)| start <= position && position < end)
            {
                return false;
            }
            let smallest_enum = enum_ranges
                .iter()
                .filter(|&&(start, end)| start <= position && position < end)
                .min_by_key(|&&(start, end)| end - start);
            let Some(&(enum_start, enum_end)) = smallest_enum else {
                return false;
            };
            !enum_case_ranges.iter().any(|&(start, end)| {
                enum_start <= start && end <= enum_end && start <= position && position < end
            })
        })
        .count();
    let export_ranges = arena
        .iter()
        .filter_map(|(_, tree)| {
            matches!(tree.kind, TreeKind::Export(_))
                .then(|| tree.position.map(|span| span.span().range()))
                .flatten()
        })
        .map(|range| (range.start(), range.end()))
        .collect::<Vec<_>>();
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
                    let symbol_materialized =
                        named.is_some_and(|(index, _)| index.symbol_at(source, tree_id).is_some());
                    let class_identity = named.is_some_and(|(index, store)| {
                        index.symbol_at(source, tree_id).is_some_and(|symbol| {
                            let semantic = store.symbols.get(symbol);
                            semantic.kind == SymbolKind::Class
                                && semantic.flags.contains(dotty_core::SymbolFlags::ENUM)
                        })
                    });
                    let companion_pair = named.is_some_and(|(index, store)| {
                        index
                            .symbol_at(source, tree_id)
                            .and_then(|symbol| enum_companion_identity_pair(symbol, index, store))
                            .is_some()
                    });
                    record(&mut features, "enum_definitions_encountered", 1, |_| true);
                    record(&mut features, "enum_class_identities", 1, |_| {
                        class_identity
                    });
                    record(&mut features, "enum_companion_identity_pairs", 1, |_| {
                        companion_pair
                    });
                    let parser_blocked = enum_identity_is_parser_blocked(
                        !diagnostics.is_empty(),
                        symbol_materialized,
                        scope_members.contains(&tree_id),
                        tree.position.is_some_and(|span| {
                            let position = span.span().range();
                            method_rhs_ranges.iter().any(|rhs| {
                                rhs.start() <= position.start() && position.end() <= rhs.end()
                            })
                        }),
                    ) && named.is_some();
                    record(
                        &mut features,
                        "enum_definitions_blocked_by_parser_recovery",
                        usize::from(parser_blocked),
                        |_| false,
                    );
                    let blocked_case_count = if parser_blocked {
                        match &arena.get(definition.rhs).kind {
                            TreeKind::Template(template) => template
                                .body
                                .iter()
                                .map(|member| match &arena.get(*member).kind {
                                    TreeKind::TypeDef(case)
                                        if case
                                            .metadata
                                            .modifiers
                                            .contains(&Modifier::EnumCase) =>
                                    {
                                        1
                                    }
                                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(case))
                                        if case
                                            .metadata
                                            .modifiers
                                            .contains(&Modifier::EnumCase) =>
                                    {
                                        1
                                    }
                                    TreeKind::PhaseSpecific(UntypedNode::PatDef(case))
                                        if case
                                            .modifiers
                                            .modifiers
                                            .contains(&Modifier::EnumCase) =>
                                    {
                                        case.patterns.len()
                                    }
                                    _ => 0,
                                })
                                .sum(),
                            _ => 0,
                        }
                    } else {
                        0
                    };
                    record(
                        &mut features,
                        "enum_cases_blocked_by_parser_recovery",
                        blocked_case_count,
                        |_| false,
                    );
                }
                if definition.metadata.modifiers.contains(&Modifier::EnumCase) {
                    record(&mut features, "enum_parameterized_cases", 1, |_| has_symbol);
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
                        record(&mut features, "enum_comma_group_singleton_cases", 1, |_| {
                            mapped
                        });
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
                    record(&mut features, "enum_singleton_cases", 1, |_| has_symbol);
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
            TreeKind::Export(_) => {
                record(&mut features, "export_sites_recorded", 1, |_| {
                    named.is_some_and(|(index, _)| index.export_site_at(source, tree_id).is_some())
                });
                record(&mut features, "export_sites_in_method_bodies", 1, |_| {
                    tree.position.is_some_and(|span| {
                        let position = span.span().range();
                        method_rhs_ranges.iter().any(|rhs| {
                            rhs.start() <= position.start() && position.end() <= rhs.end()
                        })
                    })
                });
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

    record(
        &mut features,
        "export_syntax_occurrences",
        export_keyword_positions.len(),
        |i| {
            export_keyword_positions.get(i).is_some_and(|position| {
                export_ranges
                    .iter()
                    .any(|&(start, end)| start <= *position && *position < end)
            })
        },
    );
    let parsed_export_tokens = export_keyword_positions
        .iter()
        .filter(|&&position| {
            export_ranges
                .iter()
                .any(|&(start, end)| start <= position && position < end)
        })
        .count();
    record(
        &mut features,
        "export_sites_blocked_by_parser_recovery",
        export_keyword_positions
            .len()
            .saturating_sub(parsed_export_tokens),
        |_| false,
    );
    record(
        &mut features,
        "enum_cases_blocked_by_parser_recovery",
        blocked_enum_case_clauses,
        |_| false,
    );

    for diagnostic in diagnostics {
        if diagnostic.kind == "UnsupportedSyntax" {
            let bucket = format!("parser_blocked: {}", normalize_message(&diagnostic.message));
            record(&mut features, &bucket, 1, |_| false);
        }
    }

    features
}

fn enum_identity_is_parser_blocked(
    parser_recovered: bool,
    identity_materialized: bool,
    is_scope_member: bool,
    is_inside_method_body: bool,
) -> bool {
    parser_recovered && !identity_materialized && !is_scope_member && !is_inside_method_body
}

fn collect_scope_members(arena: &AstArena<Untyped>) -> HashSet<dotty_core::TreeId<Untyped>> {
    let mut attached = HashSet::new();
    for (_, parent) in arena.iter() {
        let members = match &parent.kind {
            TreeKind::Template(template) => Some(template.body.as_slice()),
            TreeKind::PackageDef(package) => Some(package.stats.as_slice()),
            _ => None,
        };
        let Some(members) = members else {
            continue;
        };
        for &member in members {
            let contains_member = match (parent.position, arena.get(member).position) {
                (Some(parent_span), Some(member_span)) => {
                    let parent = parent_span.span().range();
                    let member = member_span.span().range();
                    parent.start() <= member.start() && member.end() <= parent.end()
                }
                _ => true,
            };
            if contains_member {
                attached.insert(member);
            }
        }
    }
    attached
}

fn enum_companion_identity_pair(
    enum_id: SymbolId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
) -> Option<(SymbolId, SymbolId)> {
    let enum_symbol = store.symbols.get(enum_id);
    let owner = enum_symbol.owner?;
    let object = enum_symbol.links.companion?;
    let object_symbol = store.symbols.get(object);
    if object_symbol.kind != SymbolKind::Object
        || object_symbol.owner != Some(owner)
        || object_symbol.links.companion != Some(enum_id)
    {
        return None;
    }

    let owner_scope = store.scopes.get(index.scope_of(owner)?);
    let enum_name = store.names.resolve(enum_symbol.name.text());
    let objects = owner_scope
        .entered_symbols()
        .filter(|candidate| {
            let symbol = store.symbols.get(*candidate);
            symbol.kind == SymbolKind::Object
                && symbol.owner == Some(owner)
                && store.names.resolve(symbol.name.text()) == enum_name
        })
        .collect::<Vec<_>>();
    if objects.as_slice() != [object] {
        return None;
    }

    let module_class_name = format!("{enum_name}$");
    let module_classes = owner_scope
        .entered_symbols()
        .filter(|candidate| {
            let symbol = store.symbols.get(*candidate);
            symbol.kind == SymbolKind::ModuleClass
                && symbol.owner == Some(owner)
                && store.names.resolve(symbol.name.text()) == module_class_name
                && index.scope_of(*candidate).is_some()
        })
        .collect::<Vec<_>>();
    let [module_class] = module_classes.as_slice() else {
        return None;
    };
    Some((object, *module_class))
}

fn validate_namer_audit_invariants(
    arena: &AstArena<Untyped>,
    source: SourceId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
    source_text: &str,
    parser_recovered: bool,
) -> Vec<String> {
    let mut violations = Vec::new();
    let scope_members = collect_scope_members(arena);
    let method_rhs_ranges = arena
        .iter()
        .filter_map(|(_, tree)| match &tree.kind {
            TreeKind::DefDef(definition) => definition.rhs,
            _ => None,
        })
        .filter_map(|rhs| arena.get(rhs).position.map(|span| span.span().range()))
        .collect::<Vec<_>>();
    for (tree_id, tree) in arena.iter() {
        let tree_is_inside_method_rhs = tree.position.is_some_and(|span| {
            let position = span.span().range();
            method_rhs_ranges
                .iter()
                .any(|rhs| rhs.start() <= position.start() && position.end() <= rhs.end())
        });
        let TreeKind::TypeDef(definition) = &tree.kind else {
            if matches!(tree.kind, TreeKind::Export(_))
                && !tree_is_inside_method_rhs
                && index.export_site_at(source, tree_id).is_none()
            {
                violations.push(format!(
                    "export source {}/tree {} has no semantic handoff record",
                    source.index(),
                    tree_id.index()
                ));
            }
            continue;
        };
        if tree_is_inside_method_rhs || !definition.metadata.modifiers.contains(&Modifier::Enum) {
            continue;
        }
        let Some(enum_id) = index.symbol_at(source, tree_id) else {
            let snippet = tree
                .position
                .and_then(|span| {
                    let range = span.span().range();
                    source_text.get(range.start() as usize..range.end() as usize)
                })
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if enum_identity_is_parser_blocked(
                parser_recovered,
                false,
                scope_members.contains(&tree_id),
                tree_is_inside_method_rhs,
            ) {
                continue;
            }
            let snippet = snippet.chars().take(180).collect::<String>();
            violations.push(format!(
                "enum source {}/tree {} has no class identity for `{snippet}`",
                source.index(),
                tree_id.index()
            ));
            continue;
        };
        let enum_symbol = store.symbols.get(enum_id);
        if enum_symbol.kind != SymbolKind::Class
            || !enum_symbol.flags.contains(dotty_core::SymbolFlags::ENUM)
        {
            violations.push(format!(
                "enum source {}/tree {} does not map to an enum class identity",
                source.index(),
                tree_id.index()
            ));
        }
        let Some((_, companion_module_class)) = enum_companion_identity_pair(enum_id, index, store)
        else {
            violations.push(format!(
                "enum source {}/tree {} has no unique, consistently linked companion pair",
                source.index(),
                tree_id.index()
            ));
            continue;
        };
        let TreeKind::Template(template) = &arena.get(definition.rhs).kind else {
            continue;
        };
        for member in &template.body {
            match &arena.get(*member).kind {
                TreeKind::TypeDef(case)
                    if case.metadata.modifiers.contains(&Modifier::EnumCase) =>
                {
                    let Some(case_id) = index.symbol_at(source, *member) else {
                        violations.push(format!(
                            "parameterized enum case source {}/tree {} has no class identity",
                            source.index(),
                            member.index()
                        ));
                        continue;
                    };
                    let case_symbol = store.symbols.get(case_id);
                    if case_symbol.kind != SymbolKind::Class
                        || case_symbol.owner != Some(companion_module_class)
                        || !case_symbol.flags.contains(dotty_core::SymbolFlags::CASE)
                        || !case_symbol.flags.contains(dotty_core::SymbolFlags::ENUM)
                        || !case_symbol.flags.contains(dotty_core::SymbolFlags::FINAL)
                    {
                        violations.push(format!(
                            "parameterized enum case source {}/tree {} is not owned by its enum companion module class",
                            source.index(),
                            member.index()
                        ));
                    }
                }
                TreeKind::PhaseSpecific(UntypedNode::ModuleDef(case))
                    if case.metadata.modifiers.contains(&Modifier::EnumCase) =>
                {
                    validate_enum_singleton_case(
                        source,
                        *member,
                        companion_module_class,
                        index,
                        store,
                        &mut violations,
                    );
                }
                TreeKind::PhaseSpecific(UntypedNode::PatDef(case))
                    if case.modifiers.modifiers.contains(&Modifier::EnumCase) =>
                {
                    for pattern in &case.patterns {
                        validate_enum_singleton_case(
                            source,
                            *pattern,
                            companion_module_class,
                            index,
                            store,
                            &mut violations,
                        );
                    }
                }
                _ => {}
            }
        }
    }
    violations
}

fn validate_enum_singleton_case(
    source: SourceId,
    tree: dotty_core::TreeId<Untyped>,
    companion_module_class: SymbolId,
    index: &SourceSemanticIndex,
    store: &SemanticStore,
    violations: &mut Vec<String>,
) {
    let Some(case_id) = index.symbol_at(source, tree) else {
        violations.push(format!(
            "singleton enum case source {}/tree {} has no object identity",
            source.index(),
            tree.index()
        ));
        return;
    };
    let case_symbol = store.symbols.get(case_id);
    if case_symbol.kind != SymbolKind::Object || case_symbol.owner != Some(companion_module_class) {
        violations.push(format!(
            "singleton enum case source {}/tree {} is not owned by its enum companion module class",
            source.index(),
            tree.index()
        ));
    }
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
        dotty_namer::NamerError::DuplicateSourceExportSite { .. } => "DuplicateSourceExportSite",
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
    let components = root
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect::<Vec<_>>();
    if let Some(index) = components
        .iter()
        .rposition(|part| part.starts_with("cats-v") || part.starts_with("cats-effect-v"))
    {
        return components[index + 1..].join("/");
    }
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("root");
    if root
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|part| part == "main")
        && root
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|part| part == "src")
    {
        let mut components = root
            .components()
            .rev()
            .take(5)
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        components.reverse();
        return components.join("/");
    }
    if name == "src"
        && let Some(parent) = root.parent().and_then(|parent| parent.file_name())
    {
        return Path::new(parent).join(name).display().to_string();
    }
    name.to_owned()
}

fn build_report(outcomes: &[FileOutcome], metadata: ReportMetadata<'_>) -> Report {
    let mut report = Report {
        schema_version: 6,
        corpus_roots: metadata.roots.iter().map(|root| root_label(root)).collect(),
        source_version: metadata.source_version,
        source_revision: metadata.source_revision,
        source_sets: metadata
            .source_sets
            .iter()
            .map(|source_set| {
                (
                    source_set.name.clone(),
                    SourceSetReport::from_options(source_set),
                )
            })
            .collect(),
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
                "case_class_synthetic_apis",
                "context_bound_evidence_synthesis",
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
        if let Some(source_set) = &outcome.source_set
            && let Some(source_set_report) = report.source_sets.get_mut(source_set)
        {
            source_set_report.record(outcome);
        }
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
                if counts.occurrences == 0 || is_namer_audit_metric_feature(name) {
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
                    && !name.ends_with("_blocked_by_parser_recovery")
                {
                    bucket.deferred_occurrences += counts.occurrences - counts.materialized;
                }
                if bucket.examples.len() < 5 {
                    bucket.examples.push(outcome.path.clone());
                }
            }
        }
        if let Some(namer) = &mut report.namer {
            let count = |name: &str| {
                outcome
                    .deferred_features
                    .get(name)
                    .copied()
                    .unwrap_or_default()
            };
            let definitions = count("enum_definitions_encountered");
            let classes = count("enum_class_identities");
            let companions = count("enum_companion_identity_pairs");
            let singleton_cases = count("enum_singleton_cases");
            let comma_group_cases = count("enum_comma_group_singleton_cases");
            let parameterized_cases = count("enum_parameterized_cases");
            namer.enum_identity_audit.definitions_encountered += definitions.occurrences;
            namer
                .enum_identity_audit
                .definitions_blocked_by_parser_recovery +=
                count("enum_definitions_blocked_by_parser_recovery").occurrences;
            namer.enum_identity_audit.class_identities_materialized += classes.materialized;
            namer
                .enum_identity_audit
                .companion_identity_pairs_materialized += companions.materialized;
            namer.enum_identity_audit.singleton_cases_encountered += singleton_cases.occurrences;
            namer.enum_identity_audit.singleton_cases_materialized += singleton_cases.materialized;
            namer
                .enum_identity_audit
                .comma_group_singleton_cases_encountered += comma_group_cases.occurrences;
            namer
                .enum_identity_audit
                .comma_group_singleton_cases_materialized += comma_group_cases.materialized;
            namer.enum_identity_audit.parameterized_cases_encountered +=
                parameterized_cases.occurrences;
            namer.enum_identity_audit.parameterized_cases_materialized +=
                parameterized_cases.materialized;
            namer.enum_identity_audit.cases_blocked_by_parser_recovery +=
                count("enum_cases_blocked_by_parser_recovery").occurrences;

            let export_syntax = count("export_syntax_occurrences");
            namer.export_handoff_audit.syntax_occurrences += export_syntax.occurrences;
            namer.export_handoff_audit.sites_recorded +=
                count("export_sites_recorded").materialized;
            namer.export_handoff_audit.sites_blocked_by_parser_recovery +=
                count("export_sites_blocked_by_parser_recovery").occurrences;
            namer.export_handoff_audit.sites_in_method_bodies +=
                count("export_sites_in_method_bodies").materialized;
            // Namer only records export sites; forwarder synthesis is owned by
            // a later typed phase and must remain zero in this gate.
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

fn is_namer_audit_metric_feature(name: &str) -> bool {
    name.starts_with("enum_") || name.starts_with("export_")
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
    for (name, source_set) in &report.source_sets {
        println!(
            "  source set {name} {} @ {}: {} files ({} clean, {} recoverable, {} hard failures, {} diagnostics)",
            source_set.source_version,
            source_set.source_revision,
            source_set.files_attempted,
            source_set.files_parsed_without_diagnostics,
            source_set.files_parsed_with_recoverable_diagnostics,
            source_set.hard_parser_failures,
            source_set.diagnostics,
        );
        if let Some(files) = source_set.scala_oracle_files {
            println!("    Scala oracle files: {files}");
        }
        if let Some(failures) = source_set.scala_oracle_failures {
            println!("    Scala oracle failures: {failures}");
        }
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
        let enums = &namer.enum_identity_audit;
        println!(
            "  enum identities: {} definitions; {} enum classes and {} companion object/module-class pairs materialized; {} definitions blocked by parser recovery",
            enums.definitions_encountered,
            enums.class_identities_materialized,
            enums.companion_identity_pairs_materialized,
            enums.definitions_blocked_by_parser_recovery
        );
        println!(
            "  enum cases: singleton {}/{}, comma-group singleton {}/{}, parameterized {}/{}; {} case clauses blocked by parser recovery",
            enums.singleton_cases_materialized,
            enums.singleton_cases_encountered,
            enums.comma_group_singleton_cases_materialized,
            enums.comma_group_singleton_cases_encountered,
            enums.parameterized_cases_materialized,
            enums.parameterized_cases_encountered,
            enums.cases_blocked_by_parser_recovery
        );
        let exports = &namer.export_handoff_audit;
        println!(
            "  exports: {} syntax occurrences, {} sites recorded, {} blocked by parser recovery, {} inside method bodies, {} forwarders synthesized",
            exports.syntax_occurrences,
            exports.sites_recorded,
            exports.sites_blocked_by_parser_recovery,
            exports.sites_in_method_bodies,
            exports.forwarders_synthesized
        );
    }
    if let Some(features) = &report.deferred_features {
        println!("  deferred source-feature inventory:");
        for (feature, bucket) in features {
            if is_namer_audit_metric_feature(feature) {
                continue;
            }
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
            source_sets: &[],
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
    fn discovers_shared_and_scala3_production_roots_only() {
        let root = unique_temp_dir("production-roots");
        let included = [
            "core/shared/src/main/scala",
            "core/src/main/scala-3",
            "testkit/src/main/scala",
        ];
        let excluded = [
            "core/src/main/scala-2.13",
            "tests/src/main/scala",
            "docs/src/main/scala",
            "graalvm-example/src/main/scala",
            "benchmarks/src/main/scala",
            "scalafix/rules/src/main/scala",
            "core/target/generated/src/main/scala",
        ];
        for relative in included.into_iter().chain(excluded) {
            fs::create_dir_all(root.join(relative)).expect("create source root fixture");
        }

        let roots = discover_production_roots(&root).expect("discover production roots");
        let relatives = roots
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .expect("root is under fixture")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            relatives,
            included.into_iter().map(PathBuf::from).collect::<Vec<_>>()
        );

        fs::remove_dir_all(root).expect("remove temp root");
    }

    #[cfg(unix)]
    #[test]
    fn source_discovery_does_not_follow_symlinks_outside_the_checkout() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("source-symlink");
        let external = unique_temp_dir("source-symlink-external");
        fs::create_dir_all(root.join("core/src/main/scala")).expect("create project root");
        fs::create_dir_all(external.join("escaped/src/main/scala")).expect("create external root");
        fs::write(
            external.join("escaped/src/main/scala/Outside.scala"),
            "object Outside",
        )
        .expect("write external source");
        symlink(external.join("escaped"), root.join("linked-module")).expect("create symlink");
        symlink(
            external.join("escaped/src/main/scala/Outside.scala"),
            root.join("core/src/main/scala/Outside.scala"),
        )
        .expect("create source symlink");

        assert_eq!(
            discover_production_roots(&root).expect("discover production roots"),
            vec![fs::canonicalize(root.join("core/src/main/scala")).unwrap()]
        );
        let files =
            discover_files(&[root.join("core/src/main/scala")]).expect("discover source files");
        assert!(files.is_empty());

        fs::remove_dir_all(root).expect("remove project root");
        fs::remove_dir_all(external).expect("remove external root");
    }

    #[test]
    fn repeated_roots_do_not_duplicate_files_and_source_sets_are_disjoint() {
        let root = unique_temp_dir("source-sets");
        fs::create_dir_all(&root).expect("create temp corpus");
        fs::write(root.join("one.scala"), "object One").expect("write source");

        let roots = vec![root.clone(), root.clone()];
        let files = discover_files(&roots).expect("discover files");
        assert_eq!(files.len(), 1);

        let source_set = SourceSetOptions {
            name: "cats".to_owned(),
            version: "2.13.0".to_owned(),
            revision: "a".repeat(40),
            repository: "https://github.com/typelevel/cats".to_owned(),
            roots: vec![root.clone()],
            oracle_files: None,
            oracle_failures: None,
        };
        assert_eq!(
            source_set_names_for_files(&files, std::slice::from_ref(&source_set))
                .expect("assign source set"),
            vec![Some("cats".to_owned())]
        );
        let overlapping = [
            source_set.clone(),
            SourceSetOptions {
                name: "cats-effect".to_owned(),
                ..source_set
            },
        ];
        assert!(
            source_set_names_for_files(&files, &overlapping)
                .expect_err("overlapping source sets must be rejected")
                .contains("multiple source sets")
        );

        fs::remove_dir_all(root).expect("remove temp corpus");
    }

    #[test]
    fn source_set_cli_metadata_is_scoped_to_the_active_set() {
        let root = unique_temp_dir("source-set-cli");
        fs::create_dir_all(&root).expect("create temp root");
        let options = parse_options(
            [
                "--source-version".to_owned(),
                "3.9.0".to_owned(),
                "--source-revision".to_owned(),
                "scala-revision".to_owned(),
                "--oracle-files".to_owned(),
                "10".to_owned(),
                "--source-set".to_owned(),
                "cats@v2.13.0@cats-revision@https://github.com/typelevel/cats".to_owned(),
                "--root".to_owned(),
                root.to_string_lossy().into_owned(),
                "--oracle-files".to_owned(),
                "20".to_owned(),
                "--oracle-failures".to_owned(),
                "2".to_owned(),
            ]
            .into_iter(),
        )
        .expect("parse source set arguments");

        assert_eq!(options.source_version.as_deref(), Some("3.9.0"));
        assert_eq!(options.source_revision.as_deref(), Some("scala-revision"));
        assert_eq!(options.oracle_files, Some(10));
        assert_eq!(options.source_sets.len(), 1);
        assert_eq!(options.source_sets[0].name, "cats");
        assert_eq!(options.source_sets[0].version, "v2.13.0");
        assert_eq!(options.source_sets[0].revision, "cats-revision");
        assert_eq!(
            options.source_sets[0].repository,
            "https://github.com/typelevel/cats"
        );
        assert_eq!(options.source_sets[0].oracle_files, Some(20));
        assert_eq!(options.source_sets[0].oracle_failures, Some(2));
        assert_eq!(
            options.source_sets[0].roots,
            vec![fs::canonicalize(&root).unwrap()]
        );

        fs::remove_dir_all(root).expect("remove temp root");
    }

    #[test]
    fn source_set_report_keeps_provenance_and_aggregates_diagnostics() {
        let source_set = SourceSetOptions {
            name: "cats".to_owned(),
            version: "2.13.0".to_owned(),
            revision: "a".repeat(40),
            repository: "https://github.com/typelevel/cats".to_owned(),
            roots: vec![PathBuf::from("/tmp/cats/core/src/main/scala")],
            oracle_files: Some(2),
            oracle_failures: Some(1),
        };
        let outcomes = [
            FileOutcome {
                path: "cats/core/src/main/scala/clean.scala".to_owned(),
                source_set: Some("cats".to_owned()),
                status: Status::Clean,
                diagnostics: Vec::new(),
                scanner_diagnostics: 0,
                namer: None,
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: None,
                has_raw_caret_character: false,
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
            },
            FileOutcome {
                path: "cats/core/src/main/scala/recovered.scala".to_owned(),
                source_set: Some("cats".to_owned()),
                status: Status::RecoverableDiagnostics,
                diagnostics: vec![DiagnosticSummary {
                    kind: "ExpectedType".to_owned(),
                    message: "expected a type".to_owned(),
                }],
                scanner_diagnostics: 0,
                namer: None,
                deferred_features: BTreeMap::new(),
                capture_checking_enabled: None,
                has_raw_caret_character: false,
                has_caret_operator_token: false,
                has_parser_capture_syntax: false,
            },
            FileOutcome {
                path: "cats/core/src/main/scala/hard.scala".to_owned(),
                source_set: Some("cats".to_owned()),
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
        ];
        let report = build_report(
            &outcomes,
            ReportMetadata {
                roots: &source_set.roots,
                source_sets: std::slice::from_ref(&source_set),
                source_version: None,
                source_revision: None,
                parser_revision: None,
                oracle_files: None,
                oracle_failures: None,
                collect_namer: false,
            },
        );

        let set = &report.source_sets["cats"];
        assert_eq!(set.source_version, "2.13.0");
        assert_eq!(set.source_revision, "a".repeat(40));
        assert_eq!(set.repository, "https://github.com/typelevel/cats");
        assert_eq!(set.scala_oracle_files, Some(2));
        assert_eq!(set.scala_oracle_failures, Some(1));
        assert_eq!(set.files_attempted, 3);
        assert_eq!(set.files_parsed_without_diagnostics, 1);
        assert_eq!(set.files_parsed_with_recoverable_diagnostics, 1);
        assert_eq!(set.hard_parser_failures, 1);
        assert_eq!(set.diagnostics, 2);
        assert_eq!(set.diagnostic_histogram["ExpectedType"], 1);
        assert_eq!(set.diagnostic_histogram["Panic"], 1);
    }

    #[test]
    fn oracle_file_count_mismatches_are_reported_per_source_set_and_aggregate() {
        let source_set = SourceSetOptions {
            name: "cats".to_owned(),
            version: "v2.13.0".to_owned(),
            revision: "a".repeat(40),
            repository: "https://github.com/typelevel/cats".to_owned(),
            roots: vec![PathBuf::from("/tmp/cats/core/src/main/scala")],
            oracle_files: Some(3),
            oracle_failures: Some(0),
        };
        let mut report = build_report(
            &[],
            ReportMetadata {
                roots: &source_set.roots,
                source_sets: std::slice::from_ref(&source_set),
                source_version: None,
                source_revision: None,
                parser_revision: None,
                oracle_files: Some(5),
                oracle_failures: Some(0),
                collect_namer: false,
            },
        );
        report.files_attempted = 4;
        report.source_sets.get_mut("cats").unwrap().files_attempted = 2;

        assert_eq!(
            report.oracle_count_mismatches(),
            vec![
                "aggregate has 4 Rust files but 5 Scala oracle results",
                "source set cats has 2 Rust files but 3 Scala oracle results",
            ]
        );
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
    fn enum_audit_splits_definition_companion_and_case_identities() {
        let parsed = parse_source(
            "enum Color { case Red, Green; case Yellow; case Blue(value: Int) }",
            "Color.scala",
            true,
        );
        let definitions = parsed
            .deferred_features
            .get("enum_definitions_encountered")
            .expect("enum definition counted");
        let classes = parsed
            .deferred_features
            .get("enum_class_identities")
            .expect("enum class identity counted");
        let companions = parsed
            .deferred_features
            .get("enum_companion_identity_pairs")
            .expect("enum companion identity pair counted");
        let singleton_cases = parsed
            .deferred_features
            .get("enum_singleton_cases")
            .expect("singleton enum cases counted");
        let comma_cases = parsed
            .deferred_features
            .get("enum_comma_group_singleton_cases")
            .expect("comma-group enum cases counted");
        let parameterized_cases = parsed
            .deferred_features
            .get("enum_parameterized_cases")
            .expect("parameterized enum cases counted");

        assert_eq!(definitions.occurrences, 1);
        assert_eq!(classes.materialized, 1);
        assert_eq!(companions.materialized, 1);
        assert_eq!(singleton_cases.occurrences, 1);
        assert_eq!(singleton_cases.materialized, 1);
        assert_eq!(comma_cases.occurrences, 2);
        assert_eq!(comma_cases.materialized, 2);
        assert_eq!(parameterized_cases.occurrences, 1);
        assert_eq!(parameterized_cases.materialized, 1);
        let Some(NamerOutcome::Success {
            invariant_violations,
        }) = parsed.namer
        else {
            panic!("valid enum fixture should be named successfully");
        };
        assert!(invariant_violations.is_empty(), "{invariant_violations:?}");
    }

    #[test]
    fn enum_identity_audit_only_classifies_recovered_orphans_as_parser_blocked() {
        assert!(enum_identity_is_parser_blocked(true, false, false, false));
        assert!(!enum_identity_is_parser_blocked(false, false, false, false));
        assert!(!enum_identity_is_parser_blocked(true, true, false, false));
        assert!(!enum_identity_is_parser_blocked(true, false, true, false));
        assert!(!enum_identity_is_parser_blocked(true, false, false, true));
    }

    #[test]
    fn recovered_top_level_enum_keeps_its_package_identity() {
        let parsed = parse_source(
            "enum Color { case Red }\nobject Broken { def = }",
            "RecoveredTopLevelEnum.scala",
            true,
        );

        assert!(matches!(parsed.status, Status::RecoverableDiagnostics));
        assert_eq!(
            parsed
                .deferred_features
                .get("enum_class_identities")
                .expect("top-level enum identity measured")
                .materialized,
            1
        );
        assert_eq!(
            parsed
                .deferred_features
                .get("enum_definitions_blocked_by_parser_recovery")
                .expect("parser-blocked definitions measured")
                .occurrences,
            0
        );
        let Some(NamerOutcome::Success {
            invariant_violations,
        }) = parsed.namer
        else {
            panic!("recovered top-level enum should remain nameable");
        };
        assert!(invariant_violations.is_empty(), "{invariant_violations:?}");
    }

    #[test]
    fn enum_audit_counts_case_syntax_lost_during_parser_recovery() {
        let parsed = parse_source("enum Color { case }", "Color.scala", true);
        let blocked = parsed
            .deferred_features
            .get("enum_cases_blocked_by_parser_recovery")
            .expect("parser-blocked enum case clause counted");
        assert_eq!(blocked.occurrences, 1);
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
    fn export_audit_distinguishes_recorded_sites_from_forwarders() {
        let parsed = parse_source(
            "object A { def value: Int = 1 }; object B { export A.value }",
            "Exports.scala",
            true,
        );
        let syntax = parsed
            .deferred_features
            .get("export_syntax_occurrences")
            .expect("export syntax occurrence counted");
        let sites = parsed
            .deferred_features
            .get("export_sites_recorded")
            .expect("export semantic handoff counted");
        let blocked = parsed
            .deferred_features
            .get("export_sites_blocked_by_parser_recovery")
            .expect("parser-blocked export sites counted");
        let forwarders = parsed
            .deferred_features
            .get("export_forwarders_synthesized")
            .expect("forwarder synthesis count exists");
        let method_sites = parsed
            .deferred_features
            .get("export_sites_in_method_bodies")
            .expect("method-body export sites counted");

        assert_eq!(syntax.occurrences, 1);
        assert_eq!(syntax.materialized, 1);
        assert_eq!(sites.occurrences, 1);
        assert_eq!(sites.materialized, 1);
        assert_eq!(blocked.occurrences, 0);
        assert_eq!(method_sites.materialized, 0);
        assert_eq!(forwarders.occurrences, 0);
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
                source_set: None,
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
                source_set: None,
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
                source_set: None,
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
                source_set: None,
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
                source_set: None,
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
                source_set: None,
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
            source_set: None,
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
                source_sets: &[],
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
