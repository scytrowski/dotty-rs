use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, JarClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::{Match, Tree, TreeKind, Untyped, UntypedNode};
use dotty_core::{
    Definitions, MemberRequest, Packages, ResolutionError, ResolverCheckpoint, SemanticStore,
    SourceId, SourceText, SymbolId, SymbolResolver, TextRange,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamerError, name_compilation_unit};
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};
use dotty_typer::{SourceTyper, TyperError};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResolverMetrics {
    resolver_package_requests: usize,
    package_successes: usize,
    package_unresolved: usize,
    package_errors: usize,
    package_source_reuse: usize,
    resolver_member_requests: usize,
    member_successes: usize,
    class_symbol_successes: usize,
    member_source_reuse: usize,
    member_unresolved: usize,
    member_errors: usize,
    packages: BTreeSet<String>,
    classes: BTreeSet<String>,
    members: BTreeSet<String>,
    unresolved_member_names: BTreeMap<String, usize>,
    member_error_kinds: BTreeMap<String, usize>,
}

impl ResolverMetrics {
    fn external_package_requests(&self) -> usize {
        debug_assert!(self.resolver_package_requests >= self.package_source_reuse);
        self.resolver_package_requests - self.package_source_reuse
    }

    fn external_member_requests(&self) -> usize {
        debug_assert!(self.resolver_member_requests >= self.member_source_reuse);
        self.resolver_member_requests - self.member_source_reuse
    }
}

struct AuditResolver<E: ClassPathEntry> {
    inner: ClasspathSymbolResolver<E>,
    metrics: Rc<RefCell<ResolverMetrics>>,
}

#[derive(Clone)]
struct SharedClassPath(Arc<CompositeClassPath>);

impl ClassPathEntry for SharedClassPath {
    fn find_class(
        &self,
        name: &dotty_classloader::classloader::BinaryName,
    ) -> Result<
        Option<dotty_classloader::classloader::ClassResource>,
        dotty_classloader::classloader::ClassPathError,
    > {
        self.0.find_class(name)
    }

    fn contains_package(
        &self,
        package: &[&str],
    ) -> Result<bool, dotty_classloader::classloader::ClassPathError> {
        self.0.contains_package(package)
    }
}

impl<E: ClassPathEntry> SymbolResolver for AuditResolver<E> {
    fn checkpoint(&self) -> ResolverCheckpoint {
        self.inner.checkpoint()
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        self.inner.rollback_to(store, checkpoint);
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner.resolve_member(store, request);
        let mut metrics = self.metrics.borrow_mut();
        metrics.resolver_member_requests += 1;
        match &result {
            Ok(Some(symbol)) => {
                let path = audit_symbol_path(store, *symbol);
                if matches!(
                    store.symbols.get(*symbol).origin,
                    dotty_core::SymbolOrigin::Source(_)
                ) {
                    metrics.member_source_reuse += 1;
                } else if is_external_symbol(store.symbols.get(*symbol).origin) {
                    metrics.member_successes += 1;
                    match store.symbols.get(*symbol).kind {
                        dotty_core::SymbolKind::Class
                        | dotty_core::SymbolKind::Trait
                        | dotty_core::SymbolKind::Object
                        | dotty_core::SymbolKind::ModuleClass => {
                            metrics.class_symbol_successes += 1;
                            metrics.classes.insert(path);
                        }
                        _ => {
                            metrics.members.insert(path);
                        }
                    }
                }
            }
            Ok(None) => {
                metrics.member_unresolved += 1;
                let name = store.names.resolve(request.name.text()).to_owned();
                *metrics.unresolved_member_names.entry(name).or_default() += 1;
            }
            Err(error) => {
                metrics.member_errors += 1;
                *metrics
                    .member_error_kinds
                    .entry(format!("{error:?}"))
                    .or_default() += 1;
            }
        }
        result
    }

    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner.resolve_package(store, path);
        let mut metrics = self.metrics.borrow_mut();
        metrics.resolver_package_requests += 1;
        match &result {
            Ok(Some(symbol)) => {
                if matches!(
                    store.symbols.get(*symbol).origin,
                    dotty_core::SymbolOrigin::Source(_)
                ) {
                    metrics.package_source_reuse += 1;
                } else {
                    metrics.package_successes += 1;
                    metrics.packages.insert(path.join("."));
                }
            }
            Ok(None) => metrics.package_unresolved += 1,
            Err(_) => metrics.package_errors += 1,
        }
        result
    }
}

fn audit_symbol_path(store: &SemanticStore, symbol: SymbolId) -> String {
    let mut parts = Vec::new();
    let mut current = Some(symbol);
    let mut visited = HashSet::new();
    while let Some(symbol) = current {
        if !visited.insert(symbol) {
            parts.push("<owner-cycle>".to_owned());
            break;
        }
        let entry = store.symbols.get(symbol);
        let name = store.names.resolve(entry.name.text());
        if !name.is_empty() {
            parts.push(name.to_owned());
        }
        current = entry.owner;
    }
    parts.reverse();
    parts.join(".")
}

fn is_external_symbol(origin: dotty_core::SymbolOrigin) -> bool {
    matches!(
        origin,
        dotty_core::SymbolOrigin::Classfile(_) | dotty_core::SymbolOrigin::Tasty(_)
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum FailureFamily {
    ResolutionClasspathEnvironment,
    ParserNamer,
    UnsupportedExpressionSyntaxSemantics,
    LocalDeclarationDeferral,
    TypeRelationInferenceCompletion,
    #[default]
    Other,
}

impl FailureFamily {
    fn label(self) -> &'static str {
        match self {
            Self::ResolutionClasspathEnvironment => "resolution/classpath environment",
            Self::ParserNamer => "parser/namer",
            Self::UnsupportedExpressionSyntaxSemantics => "unsupported expression syntax/semantics",
            Self::LocalDeclarationDeferral => "local declaration deferral",
            Self::TypeRelationInferenceCompletion => "type relation/inference/completion",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FailureClassification {
    bucket: String,
    family: FailureFamily,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Audit {
    files_attempted: usize,
    parser_failed_files: usize,
    namer_failed_files: usize,
    file_read_failed_files: usize,
    file_read_failed_paths: BTreeSet<String>,
    recovered_parser_files: usize,
    local_definitions: usize,
    buckets: BTreeMap<String, usize>,
    local_defdefs: usize,
    typed_local_defdefs: usize,
    failures: BTreeMap<String, FailureBucket>,
    expression_forms: BTreeMap<String, usize>,
    type_tree_forms: BTreeMap<String, usize>,
    parser_diagnostics: BTreeMap<String, usize>,
    match_readiness: MatchReadiness,
    match_profile: MatchProfile,
}

impl Default for Audit {
    fn default() -> Self {
        Self {
            files_attempted: 0,
            parser_failed_files: 0,
            namer_failed_files: 0,
            file_read_failed_files: 0,
            file_read_failed_paths: BTreeSet::new(),
            recovered_parser_files: 0,
            local_definitions: 0,
            buckets: BTreeMap::new(),
            local_defdefs: 0,
            typed_local_defdefs: 0,
            failures: BTreeMap::new(),
            expression_forms: empty_expression_histogram(),
            type_tree_forms: empty_type_tree_histogram(),
            parser_diagnostics: BTreeMap::new(),
            match_readiness: MatchReadiness::default(),
            match_profile: MatchProfile::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MatchReadiness {
    first_blocker_methods: usize,
    matches: usize,
    cases: usize,
    guarded_cases: usize,
    first_blocker_errors: BTreeMap<String, usize>,
    first_blocker_error_files: BTreeMap<String, BTreeSet<String>>,
    pattern_shapes: BTreeMap<String, usize>,
    pattern_shape_files: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MatchProfile {
    matches: usize,
    cases: usize,
    guarded_cases: usize,
    pattern_roots: BTreeMap<String, usize>,
    pattern_root_files: BTreeMap<String, BTreeSet<String>>,
    typed_case_successes: BTreeMap<String, usize>,
    typed_case_success_files: BTreeMap<String, BTreeSet<String>>,
    typed_pattern_boundaries: BTreeMap<String, usize>,
    typed_case_failures: BTreeMap<String, usize>,
    typed_case_failure_files: BTreeMap<String, BTreeSet<String>>,
    extractor_protocol_successes: BTreeMap<String, usize>,
    extractor_roots: BTreeMap<String, usize>,
    extractor_dispatch: BTreeMap<String, usize>,
    extractor_type_applied: usize,
    extractor_argument_counts: BTreeMap<usize, usize>,
    extractor_nested_roots: BTreeMap<String, usize>,
    sequence_wildcards: usize,
    named_pattern_arguments: usize,
    empty_tuple_unit_patterns: usize,
    infix_pattern_forms: usize,
    extractor_files: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FailureBucket {
    family: FailureFamily,
    count: usize,
    files: BTreeSet<String>,
    examples: BTreeSet<String>,
}

impl Audit {
    fn merge(&mut self, other: Self) {
        self.files_attempted += other.files_attempted;
        self.parser_failed_files += other.parser_failed_files;
        self.namer_failed_files += other.namer_failed_files;
        self.file_read_failed_files += other.file_read_failed_files;
        self.file_read_failed_paths
            .extend(other.file_read_failed_paths);
        self.recovered_parser_files += other.recovered_parser_files;
        self.local_definitions += other.local_definitions;
        self.local_defdefs += other.local_defdefs;
        self.typed_local_defdefs += other.typed_local_defdefs;
        for (name, count) in other.expression_forms {
            *self.expression_forms.entry(name).or_default() += count;
        }
        for (name, count) in other.type_tree_forms {
            *self.type_tree_forms.entry(name).or_default() += count;
        }
        for (name, count) in other.parser_diagnostics {
            *self.parser_diagnostics.entry(name).or_default() += count;
        }
        self.match_readiness.merge(other.match_readiness);
        self.match_profile.merge(other.match_profile);
        for (name, count) in other.buckets {
            *self.buckets.entry(name).or_default() += count;
        }
        for (name, bucket) in other.failures {
            let target = self.failures.entry(name).or_default();
            target.family = bucket.family;
            target.count += bucket.count;
            target.files.extend(bucket.files);
            target.examples.extend(bucket.examples);
            target.examples = target.examples.iter().take(5).cloned().collect();
        }
    }
}

impl MatchReadiness {
    fn merge(&mut self, other: Self) {
        self.first_blocker_methods += other.first_blocker_methods;
        self.matches += other.matches;
        self.cases += other.cases;
        self.guarded_cases += other.guarded_cases;
        merge_counts(&mut self.first_blocker_errors, other.first_blocker_errors);
        for (error, files) in other.first_blocker_error_files {
            self.first_blocker_error_files
                .entry(error)
                .or_default()
                .extend(files);
        }
        for (shape, count) in other.pattern_shapes {
            *self.pattern_shapes.entry(shape).or_default() += count;
        }
        for (shape, files) in other.pattern_shape_files {
            self.pattern_shape_files
                .entry(shape)
                .or_default()
                .extend(files);
        }
    }
}

impl MatchProfile {
    fn merge(&mut self, other: Self) {
        self.matches += other.matches;
        self.cases += other.cases;
        self.guarded_cases += other.guarded_cases;
        self.sequence_wildcards += other.sequence_wildcards;
        self.named_pattern_arguments += other.named_pattern_arguments;
        self.empty_tuple_unit_patterns += other.empty_tuple_unit_patterns;
        self.infix_pattern_forms += other.infix_pattern_forms;
        self.extractor_type_applied += other.extractor_type_applied;
        self.extractor_files.extend(other.extractor_files);
        merge_counts(&mut self.pattern_roots, other.pattern_roots);
        merge_counts(&mut self.typed_case_successes, other.typed_case_successes);
        merge_file_sets(
            &mut self.typed_case_success_files,
            other.typed_case_success_files,
        );
        merge_counts(
            &mut self.typed_pattern_boundaries,
            other.typed_pattern_boundaries,
        );
        merge_counts(&mut self.typed_case_failures, other.typed_case_failures);
        merge_file_sets(
            &mut self.typed_case_failure_files,
            other.typed_case_failure_files,
        );
        merge_counts(
            &mut self.extractor_protocol_successes,
            other.extractor_protocol_successes,
        );
        merge_counts(&mut self.extractor_roots, other.extractor_roots);
        merge_counts(&mut self.extractor_dispatch, other.extractor_dispatch);
        merge_counts(
            &mut self.extractor_argument_counts,
            other.extractor_argument_counts,
        );
        merge_counts(
            &mut self.extractor_nested_roots,
            other.extractor_nested_roots,
        );
        for (shape, files) in other.pattern_root_files {
            self.pattern_root_files
                .entry(shape)
                .or_default()
                .extend(files);
        }
    }
}

fn merge_file_sets<K: Ord>(
    target: &mut BTreeMap<K, BTreeSet<String>>,
    source: BTreeMap<K, BTreeSet<String>>,
) {
    for (key, files) in source {
        target.entry(key).or_default().extend(files);
    }
}

fn merge_counts<K: Ord>(target: &mut BTreeMap<K, usize>, source: BTreeMap<K, usize>) {
    for (key, count) in source {
        *target.entry(key).or_default() += count;
    }
}

#[test]
#[ignore = "run tools/typer-classpath-corpus-audit/run with the pinned Scala checkout and explicit JAVA_HOME"]
fn pinned_scala39_local_definition_audit() {
    let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    let revision = git_revision(&root);
    assert_eq!(
        revision, "777528f19a58e794c9954a42f433373472ec57f8",
        "audit requires the repository's pinned Scala 3.9.0 source revision"
    );
    let classpath = audit_classpath_from_environment();
    let resolver_metrics = Rc::new(RefCell::new(ResolverMetrics::default()));
    let files = scala_files(&[root.join("library/src"), root.join("compiler/src")]);
    let mut audit = Audit::default();
    for (index, file) in files.iter().enumerate() {
        if index % 100 == 0 {
            eprintln!("audited {index}/{} Scala files", files.len());
        }
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let source = match fs::read_to_string(file) {
            Ok(source) => source,
            Err(_) => {
                audit.files_attempted += 1;
                audit.file_read_failed_files += 1;
                audit.file_read_failed_paths.insert(relative);
                continue;
            }
        };
        audit.merge(audit_source_with_classpath(
            &source,
            &relative,
            classpath.clone(),
            Rc::clone(&resolver_metrics),
        ));
    }

    println!("AUDIT_REPORT_BEGIN");
    println!("scala_revision={revision}");
    println!("files={}", audit.files_attempted);
    println!("parser_failed_files={}", audit.parser_failed_files);
    println!("namer_failed_files={}", audit.namer_failed_files);
    println!("file_read_failed_files={}", audit.file_read_failed_files);
    println!(
        "file_read_failed_paths={}",
        audit
            .file_read_failed_paths
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("recovered_parser_files={}", audit.recovered_parser_files);
    print_v1_deltas(&audit);
    println!("local_definitions={}", audit.local_definitions);
    let mut local_buckets = audit.buckets.iter().collect::<Vec<_>>();
    local_buckets.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (bucket, count) in local_buckets {
        println!("{bucket}={count}");
    }
    println!("expression_forms:");
    let mut expression_forms = audit.expression_forms.iter().collect::<Vec<_>>();
    expression_forms.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (form, count) in expression_forms {
        println!("  {form}={count}");
    }
    println!("type_tree_forms:");
    for (form, count) in &audit.type_tree_forms {
        println!("  {form}={count}");
    }
    println!("parser_diagnostics:");
    let mut parser_diagnostics = audit.parser_diagnostics.iter().collect::<Vec<_>>();
    parser_diagnostics.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (kind, count) in parser_diagnostics {
        println!("  {kind}={count}");
    }
    println!("local_defdefs={}", audit.local_defdefs);
    println!("typed_local_defdefs={}", audit.typed_local_defdefs);
    println!(
        "unsupported_expression_total={}",
        sum_buckets_with_prefix(&audit.failures, "UnsupportedExpression::")
    );
    println!(
        "UnsupportedTypeTree={}",
        sum_buckets_with_prefix(&audit.failures, "UnsupportedTypeTree::")
    );
    println!("unsupported_type_tree_failures:");
    for (name, bucket) in ordered_type_tree_failures(&audit) {
        println!(
            "  {name}: count={}, files={} [{}]",
            bucket.count,
            bucket.files.len(),
            bucket
                .examples
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "local_block_declaration_deferred={}",
        sum_buckets_with_prefix(&audit.failures, "LocalBlockDeclarationDeferred::")
    );
    println!(
        "no_successful_enclosing_method_typing={}",
        audit
            .failures
            .get("NoSuccessfulEnclosingMethodTyping")
            .map_or(0, |bucket| bucket.count)
    );
    println!(
        "import_qualifier_not_found={}",
        audit
            .failures
            .get("ImportQualifierNotFound")
            .map_or(0, |bucket| bucket.count)
    );
    println!(
        "external_name_or_member_resolution_failures={}",
        sum_buckets_with_prefix(&audit.failures, "TypeNameNotFound")
            + sum_buckets_with_prefix(&audit.failures, "TermNameNotFound")
            + sum_buckets_with_prefix(&audit.failures, "MemberNotFound")
            + sum_buckets_with_prefix(&audit.failures, "MemberLookup")
            + sum_buckets_with_prefix(&audit.failures, "SymbolResolution")
    );
    let mut family_counts = BTreeMap::<FailureFamily, usize>::new();
    for bucket in audit.failures.values() {
        *family_counts.entry(bucket.family).or_default() += bucket.count;
    }
    println!("failure_families:");
    let mut family_counts = family_counts.into_iter().collect::<Vec<_>>();
    family_counts.sort_by(|(family_a, count_a), (family_b, count_b)| {
        count_b
            .cmp(count_a)
            .then(family_a.label().cmp(family_b.label()))
    });
    for (family, count) in family_counts {
        println!("  {}={count}", family.label());
    }
    println!("local_defdef_failures:");
    let mut failures = audit.failures.iter().collect::<Vec<_>>();
    failures.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    for (name, bucket) in failures {
        println!(
            "  {name} [{}]: {} ({} files) [{}]",
            bucket.family.label(),
            bucket.count,
            bucket.files.len(),
            bucket
                .examples
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    print_ranked_gaps(&audit.failures);
    print_match_readiness(&audit.match_readiness);
    print_match_profile(&audit.match_profile);
    print_resolver_metrics(&resolver_metrics.borrow());
    println!("AUDIT_REPORT_END");
}

fn print_v1_deltas(audit: &Audit) {
    const V1_FILES: usize = 1_236;
    const V1_LOCAL_DEFINITIONS: usize = 23_218;
    const V1_LOCAL_METHODS: usize = 3_778;
    const V1_TYPED_LOCAL_METHODS: usize = 0;
    const V1_IMPORT_QUALIFIER_NOT_FOUND: usize = 2_046;
    const V1_UNSUPPORTED_EXPRESSION: usize = 572;
    const V1_LOCAL_DECLARATION_DEFERRED: usize = 272;
    const V1_NO_SUCCESSFUL_ENCLOSING_METHOD: usize = 483;
    const V1_PARENS: usize = 146;
    const V1_INFIX: usize = 146;
    const V1_LOCAL_IMPORT: usize = 139;

    let failures = &audit.failures;
    let current_import = failures
        .get("ImportQualifierNotFound")
        .map_or(0, |bucket| bucket.count);
    let current_unsupported = sum_buckets_with_prefix(failures, "UnsupportedExpression::");
    let current_deferred = sum_buckets_with_prefix(failures, "LocalBlockDeclarationDeferred::");
    let current_no_success = failures
        .get("NoSuccessfulEnclosingMethodTyping")
        .map_or(0, |bucket| bucket.count);
    println!("audit_v1_comparison:");
    println!(
        "  files_attempted={} (delta={})",
        audit.files_attempted,
        signed_delta(audit.files_attempted, V1_FILES)
    );
    println!(
        "  local_declarations={} (delta={})",
        audit.local_definitions,
        signed_delta(audit.local_definitions, V1_LOCAL_DEFINITIONS)
    );
    println!(
        "  local_methods={} (delta={})",
        audit.local_defdefs,
        signed_delta(audit.local_defdefs, V1_LOCAL_METHODS)
    );
    println!(
        "  typed_local_methods={} (delta={})",
        audit.typed_local_defdefs,
        signed_delta(audit.typed_local_defdefs, V1_TYPED_LOCAL_METHODS)
    );
    println!(
        "  ImportQualifierNotFound={} (delta={})",
        current_import,
        signed_delta(current_import, V1_IMPORT_QUALIFIER_NOT_FOUND)
    );
    println!(
        "  UnsupportedExpression_total={} (delta={})",
        current_unsupported,
        signed_delta(current_unsupported, V1_UNSUPPORTED_EXPRESSION)
    );
    println!(
        "  LocalBlockDeclarationDeferred={} (delta={})",
        current_deferred,
        signed_delta(current_deferred, V1_LOCAL_DECLARATION_DEFERRED)
    );
    println!(
        "  NoSuccessfulEnclosingMethodTyping={} (delta={})",
        current_no_success,
        signed_delta(current_no_success, V1_NO_SUCCESSFUL_ENCLOSING_METHOD)
    );
    println!("audit_581_feature_comparison:");
    for (bucket, baseline) in [
        ("UnsupportedExpression::Parens", V1_PARENS),
        ("UnsupportedExpression::InfixOp", V1_INFIX),
        ("LocalBlockDeclarationDeferred::import", V1_LOCAL_IMPORT),
    ] {
        let current = failures.get(bucket).map_or(0, |entry| entry.count);
        println!(
            "  {bucket}={current} (baseline={baseline}, delta={})",
            signed_delta(current, baseline)
        );
    }
}

fn signed_delta(current: usize, baseline: usize) -> String {
    let delta = current as i128 - baseline as i128;
    format!("{delta:+}")
}

fn sum_buckets_with_prefix(failures: &BTreeMap<String, FailureBucket>, prefix: &str) -> usize {
    failures
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(_, bucket)| bucket.count)
        .sum()
}

fn ordered_type_tree_failures(audit: &Audit) -> Vec<(&String, &FailureBucket)> {
    let mut failures = audit
        .failures
        .iter()
        .filter(|(name, _)| name.starts_with("UnsupportedTypeTree::"))
        .collect::<Vec<_>>();
    failures.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    failures
}

fn print_ranked_gaps(failures: &BTreeMap<String, FailureBucket>) {
    let mut ranked = failures
        .iter()
        .filter(|(name, bucket)| {
            !matches!(
                bucket.family,
                FailureFamily::ParserNamer | FailureFamily::ResolutionClasspathEnvironment
            ) && name.as_str() != "NoSuccessfulEnclosingMethodTyping"
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    println!("top_semantic_gaps:");
    for (rank, (name, bucket)) in ranked.iter().take(10).enumerate() {
        let category = implementation_category(name);
        println!(
            "  {}. {name}: count={}, files={}, category={}, examples={}",
            rank + 1,
            bucket.count,
            bucket.files.len(),
            category,
            bucket
                .examples
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("top_gap_implementation_scope_notes:");
    for (name, bucket) in ranked.iter().take(10) {
        let (slice, owner, prerequisites, non_goals) = scope_note_for_bucket(name);
        println!(
            "  {name} ({} occurrences, {} files): first_slice={slice}; owner={owner}; prerequisites={prerequisites}; non_goals={non_goals}",
            bucket.count,
            bucket.files.len()
        );
    }
    if let Some((name, bucket)) = ranked.first() {
        println!(
            "next_typer_increment_recommendation: implement a focused slice for {name} ({} occurrences in {} files); keep classpath materialization as a separate gate because the pinned audit resolved no external members",
            bucket.count,
            bucket.files.len()
        );
    }
}

fn scope_note_for_bucket(bucket: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match bucket {
        "UnsupportedExpression::Match" => (
            "type the scrutinee, each case pattern, and each case body against one expected result type",
            "dotty-typer/src/typer/expression/mod.rs, with a focused pattern helper",
            "the current Match/CaseDef AST and typed pattern representation",
            "exhaustivity checking, GADT refinement, and inferred match result unions",
        ),
        "UnsupportedExpression::InfixOp" => (
            "lower one infix node to the existing selected-member application path",
            "dotty-typer/src/typer/expression/mod.rs",
            "operator name, receiver, and right operand already present in the AST",
            "precedence parsing or general extension-method search",
        ),
        "UnsupportedExpression::Parens" => (
            "type the enclosed expression and preserve the wrapper source span",
            "dotty-typer/src/typer/expression",
            "the inner expression's ordinary typing context",
            "new syntax and semantic changes to the enclosed expression",
        ),
        "LocalBlockDeclarationDeferred::import" => (
            "resolve one local import qualifier and install its selector in the active block scope",
            "dotty-typer/src/typer/expression/blocks.rs",
            "the classpath resolver and transactional scope updates",
            "local type/class/object declarations and wildcard semantics beyond existing imports",
        ),
        "UnsupportedTypeTree" => (
            "lower the encountered type-tree shape into the existing core type model",
            "dotty-typer/src/typer/type_projection.rs",
            "the source type AST node and its named symbol/type metadata",
            "new parser syntax or broad type-model changes",
        ),
        _ => scope_note(implementation_category(bucket)),
    }
}

fn implementation_category(bucket: &str) -> &'static str {
    if bucket.starts_with("UnsupportedExpression::") {
        "expression typing"
    } else if bucket == "UnsupportedTypeTree" {
        "simple source lowering"
    } else if bucket.starts_with("LocalBlockDeclarationDeferred::") {
        "local declaration support"
    } else if bucket.contains("Pattern") || bucket.contains("Match") {
        "pattern typing"
    } else if bucket.contains("Inference")
        || bucket.contains("TypeRelation")
        || bucket.contains("Subtype")
    {
        "inference/type-relation support"
    } else if bucket.contains("Implicit") || bucket.contains("Contextual") {
        "implicit/contextual search"
    } else if bucket.starts_with("ImportQualifierNotFound")
        || bucket.starts_with("TypeNameNotFound")
        || bucket.starts_with("TermNameNotFound")
        || bucket.starts_with("MemberNotFound")
        || bucket.starts_with("MemberLookup")
    {
        "external resolution"
    } else {
        "other"
    }
}

fn scope_note(category: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match category {
        "expression typing" => (
            "lower one reported expression node through existing expression typing",
            "dotty-typer/src/typer/expression",
            "the parsed AST node and its child typing rules",
            "control-flow or inference redesign",
        ),
        "simple source lowering" => (
            "lower the exact reported type-tree form into the existing core type model",
            "dotty-typer/src/typer/type_tree.rs",
            "the source type AST node and symbol/type metadata",
            "new parser syntax or broad type-model changes",
        ),
        "local declaration support" => (
            "enter one local declaration kind transactionally in block typing",
            "dotty-typer/src/typer/expression/blocks.rs",
            "source symbol and scope metadata from dotty-core",
            "local classes, imports, or type definitions beyond the selected kind",
        ),
        "pattern typing" => (
            "type one pattern form against an already known expected type",
            "dotty-typer/src/typer/patterns",
            "the expected type and existing pattern AST shape",
            "exhaustivity analysis and match-result inference",
        ),
        "inference/type-relation support" => (
            "add the smallest missing relation or inference case with exact unsupported errors",
            "dotty-typer/src/types",
            "focused type relation regression cases",
            "general-purpose constraint solving",
        ),
        "implicit/contextual search" => (
            "resolve one explicit contextual argument shape with unique-candidate checks",
            "dotty-typer/src/typer/application",
            "typed contextual parameter and scope lookup",
            "implicit scope derivation and recursive search",
        ),
        "external resolution" => (
            "resolve the exact missing package, type, or member through the classpath port",
            "dotty-classloader plus the existing typer resolver boundary",
            "classpath fixture and canonical package/session identity",
            "source lowering and classloader redesign",
        ),
        _ => (
            "reproduce the exact error bucket with a focused semantic fixture",
            "the narrow module producing that TyperError",
            "the relevant source semantic metadata",
            "adjacent unsupported language features",
        ),
    }
}

#[test]
fn local_definition_audit_is_deterministic() {
    let source =
        "object Audit { def outer: Int = { val n = 1; def local[A](x: A) = x; local(n) } }";
    let first = audit_source(source, "Audit.scala");
    let second = audit_source(source, "Audit.scala");
    assert_eq!(first, second);
    assert_eq!(first.local_defdefs, 1);
    assert_eq!(first.typed_local_defdefs, 1);
    assert_eq!(first.buckets.get("local_val_defs"), Some(&1));
}

#[test]
fn resolver_request_metrics_exclude_source_reuse_from_external_attempts() {
    let metrics = ResolverMetrics {
        resolver_package_requests: 12,
        package_successes: 3,
        package_unresolved: 4,
        package_errors: 0,
        package_source_reuse: 5,
        resolver_member_requests: 8,
        member_successes: 2,
        member_source_reuse: 1,
        member_unresolved: 4,
        member_errors: 1,
        ..ResolverMetrics::default()
    };

    assert_eq!(metrics.external_package_requests(), 7);
    assert_eq!(
        metrics.external_package_requests(),
        metrics.package_successes + metrics.package_unresolved + metrics.package_errors
    );
    assert_eq!(
        metrics.resolver_package_requests,
        metrics.external_package_requests() + metrics.package_source_reuse
    );
    assert_eq!(metrics.external_member_requests(), 7);
    assert_eq!(
        metrics.external_member_requests(),
        metrics.member_successes + metrics.member_unresolved + metrics.member_errors
    );
    assert_eq!(
        metrics.resolver_member_requests,
        metrics.external_member_requests() + metrics.member_source_reuse
    );
}

#[test]
fn local_definition_audit_excludes_local_class_members() {
    let source = "object Audit { def outer: Int = { class Local { def member = 1 }; def local = 2; local } }";
    let audit = audit_source(source, "Audit.scala");

    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.buckets.get("local_def_defs"), Some(&1));
    assert_eq!(audit.buckets.get("local_classes"), Some(&1));
}

#[test]
fn local_definition_audit_counts_pattern_bindings_in_local_definitions() {
    let source = "object Audit { def outer: Int = { val captured @ _ = 1; 0 } }";
    let audit = audit_source(source, "Audit.scala");

    assert_eq!(
        audit.buckets.get("local_pattern_bindings"),
        Some(&1),
        "{audit:?}"
    );
}

#[test]
fn local_expression_audit_types_infix_calls_and_counts_them_structurally() {
    let source = "class Box { def combine(other: Box): Box = this }; object Audit { def outer(left: Box, right: Box): Box = { def local: Box = left `combine` right; local } }";
    let audit = audit_source(source, "Infix.scala");

    assert_eq!(audit.expression_forms.get("InfixOp"), Some(&1));
    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.typed_local_defdefs, 1, "{audit:?}");
    assert!(audit.failures.is_empty(), "{audit:?}");
}

#[test]
fn match_readiness_counts_unsupported_case_shapes_for_match_first_blockers() {
    let source = "object Audit { def outer(value: Int): Int = { def local: Int = value match { case Extractor(_) if true => 1; case _ => 2 }; local } }";
    let audit = audit_source(source, "Match.scala");

    assert_eq!(
        audit
            .failures
            .get("ExtractorQualifierNotFound")
            .map(|failure| failure.count),
        Some(1),
        "{audit:?}"
    );
    assert_eq!(audit.match_readiness.first_blocker_methods, 1);
    assert_eq!(audit.match_readiness.matches, 1);
    assert_eq!(audit.match_readiness.cases, 2);
    assert_eq!(audit.match_readiness.guarded_cases, 1);
    assert_eq!(
        audit
            .match_readiness
            .pattern_shapes
            .get("extractor-looking Apply/TypeApply"),
        Some(&1)
    );
    assert_eq!(
        audit
            .match_readiness
            .pattern_shapes
            .get("wildcard/identifier/bind"),
        Some(&1)
    );
}

#[test]
fn match_profile_counts_tuple_infix_and_nested_nary_extractor_shapes() {
    let source = "object Audit { def outer(value: Any): Any = value match { case Extractor(first, left | right, Nested(a, b, c)) => first; case (first, second) => first; case () => value; case left op right => left; case _ => value } }";
    let audit = audit_source(source, "PatternProfile.scala");

    assert_eq!(audit.match_profile.matches, 1);
    assert_eq!(
        audit
            .match_profile
            .pattern_roots
            .get("extractor-looking Apply"),
        Some(&1)
    );
    assert_eq!(audit.match_profile.pattern_roots.get("tuple"), Some(&2));
    assert_eq!(audit.match_profile.empty_tuple_unit_patterns, 1);
    assert_eq!(
        audit.match_profile.pattern_roots.get("infix pattern"),
        Some(&1)
    );
    assert_eq!(
        audit.match_profile.extractor_argument_counts.get(&3),
        Some(&2)
    );
    assert_eq!(
        audit
            .match_profile
            .extractor_nested_roots
            .get("alternative"),
        Some(&1)
    );
    assert_eq!(audit.match_profile.infix_pattern_forms, 1);
}

#[test]
fn local_expression_audit_types_supported_typed_patterns() {
    let source = "object Audit { def outer(value: Any): Int = { def local: Int = value match { case item: Int if true => item }; local } }";
    let audit = audit_source(source, "TypedPattern.scala");

    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.typed_local_defdefs, 1, "{audit:?}");
    assert!(audit.failures.is_empty(), "{audit:?}");
}

#[test]
fn local_expression_audit_reports_lambda_and_deferred_declaration_subkinds() {
    let lambda = audit_source(
        "object Audit { def outer: Int = { def local: Int = (x: Int) => x; 0 } }",
        "Lambda.scala",
    );
    assert!(
        lambda
            .failures
            .contains_key("UnsupportedExpression::Function"),
        "{lambda:?}"
    );

    let import = audit_source(
        "object Audit { def outer: Int = { import scala.collection; def local: Int = 1; local } }",
        "Import.scala",
    );
    assert!(
        import.failures.contains_key("ImportQualifierNotFound"),
        "{import:?}"
    );

    let type_definition = audit_source(
        "object Audit { def outer: Int = { type Local = Int; def local: Int = 1; local } }",
        "TypeDefinition.scala",
    );
    assert!(
        type_definition
            .failures
            .contains_key("LocalBlockDeclarationDeferred::type definition"),
        "{type_definition:?}"
    );
}

#[test]
fn local_expression_audit_classifies_missing_names_as_resolution_failures() {
    let audit = audit_source(
        "object Audit { def outer: Int = { def local: Missing = 1; local } }",
        "MissingType.scala",
    );

    let failure = audit
        .failures
        .get("TypeNameNotFound")
        .expect("missing type bucket");
    assert_eq!(
        failure.family,
        FailureFamily::ResolutionClasspathEnvironment
    );
}

#[test]
fn local_expression_audit_keeps_parser_and_namer_failures_distinct() {
    let namer = classify_namer_error(&NamerError::RootIsNotPackage { tree_index: 4 });
    assert_eq!(namer.bucket, "NamerError::RootIsNotPackage");
    assert_eq!(namer.family, FailureFamily::ParserNamer);

    let parser = classify_parse_diagnostic(ParseDiagnosticKind::ExpectedExpression);
    assert_eq!(parser.bucket, "ParserDiagnostic::ExpectedExpression");
    assert_eq!(parser.family, FailureFamily::ParserNamer);
    assert_ne!(namer.bucket, parser.bucket);
}

#[test]
fn unsupported_type_tree_failures_keep_exact_source_shapes() {
    let fixtures = [
        ("class C { def f(x: List[?]): Unit = () }", "TypeBoundsTree"),
        ("class C { def f(x: A | B): Unit = () }", "InfixOp::|"),
        ("class C { def f(x: A & B): Unit = () }", "InfixOp::&"),
        ("class C { def f(x: A + B): Unit = () }", "InfixOp::<other>"),
        (
            "class C { def f(x: Any, y: x.type): Unit = () }",
            "SingletonTypeTree",
        ),
        ("class C { def f(xs: T*): Unit = () }", "PostfixOp::*"),
        (
            "class C { def f(x: A { type X = B }): Unit = () }",
            "RefinedTypeTree",
        ),
        ("object C { type F = [X] =>> X }", "LambdaTypeTree"),
    ];

    for (source_text, expected_shape) in fixtures {
        let mut store = SemanticStore::new();
        let source = SourceId::from_index(0);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(
            parsed.diagnostics.is_empty(),
            "{source_text}: {:?}",
            parsed.diagnostics
        );
        let operators = source_operator_spellings(&parsed.ast, &store.names);
        let (tree, node) = parsed
            .ast
            .iter()
            .find(|(tree, node)| {
                type_tree_shape_label(&node.kind, tree.index(), &operators) == expected_shape
            })
            .unwrap_or_else(|| panic!("{source_text} did not produce {expected_shape}"));
        let error = TyperError::UnsupportedTypeTree {
            source,
            tree_index: tree.index(),
            tree_kind: tree_kind_label(&node.kind),
        };
        let failure = classify_typer_error(&error, &parsed.ast, &operators);

        assert_eq!(
            failure.bucket,
            format!("UnsupportedTypeTree::{expected_shape}")
        );
        assert_eq!(failure.family, FailureFamily::Other);
        assert_eq!(
            typed_case_failure_label(&error, &parsed.ast, &operators),
            format!("UnsupportedTypeTree::{expected_shape}")
        );
    }
}

#[test]
fn local_expression_histogram_excludes_parameters_and_type_trees() {
    let audit = audit_source("object Audit { def outer(x: Int): Int = x }", "Types.scala");

    assert_eq!(audit.expression_forms.get("Ident"), Some(&1));
    assert_eq!(audit.expression_forms.len(), EXPRESSION_FORMS.len());
    assert!(
        audit
            .expression_forms
            .values()
            .filter(|count| **count == 0)
            .count()
            > 0
    );
}

#[test]
fn declared_type_tree_histogram_is_structural_and_deterministic() {
    let source = "object Audit { def outer[A: Show](value: List[?], union: A | B, intersection: A & B, singleton: value.type, function: (A) => B, parens: (A), selected: pkg.Type, repeated: A*): A = { val refined: A { type Member = B } = value; type Higher = [X] =>> X; value.member + refined.member } }";
    let mut store = SemanticStore::new();
    let source_id = SourceId::from_index(0);
    let scanner = ContextualScanner::new(source).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source).unwrap(),
        source_id,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let first = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    let second = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    assert_eq!(first, second);
    assert_eq!(first.get("InfixOp::|"), Some(&1));
    assert_eq!(first.get("InfixOp::&"), Some(&1));
    assert_eq!(first.get("InfixOp::<other>"), Some(&0));
    assert_eq!(first.get("PostfixOp::*"), Some(&1));
    assert_eq!(first.get("SingletonTypeTree"), Some(&1));
    assert!(first["TypeBoundsTree"] > 0);
    assert!(first["RefinedTypeTree"] > 0);
    assert!(first["LambdaTypeTree"] > 0);
    assert!(first["ContextBoundTypeTree"] > 0);
    assert!(first["Function"] > 0);
    assert!(first["Parens"] > 0);
    assert_eq!(first.get("Select"), Some(&1));
}

#[test]
fn singleton_type_histogram_does_not_count_its_term_path() {
    let source = "class C { def f(value: Any, singleton: value.type): Unit = () }";
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source).unwrap(),
        SourceId::from_index(0),
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let histogram = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    assert_eq!(histogram.get("SingletonTypeTree"), Some(&1));
    assert_eq!(histogram.get("Ident"), Some(&2));
}

#[test]
fn declared_type_tree_histogram_counts_pattern_types_and_omits_empty_placeholders() {
    let source = "object Audit { def inferred = { val x = 1; x }; def patterned = { val (a, b): (A | B, C) = pair; val (x: A, y: B) = pair; val Some(z: D) = option } }";
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source).unwrap(),
        SourceId::from_index(0),
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let histogram = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    assert_eq!(histogram.get("InfixOp::|"), Some(&1));
    assert_eq!(histogram.get("TypeTree"), Some(&0));
    assert_eq!(histogram.get("Tuple"), Some(&1));
    assert_eq!(histogram.get("Ident"), Some(&6));
}

#[test]
fn local_expression_audit_schema_keeps_zero_count_forms_without_an_ast() {
    let audit = Audit::default();

    assert_eq!(audit.expression_forms.len(), EXPRESSION_FORMS.len());
    assert!(audit.expression_forms.values().all(|count| *count == 0));
    assert_eq!(audit.type_tree_forms.len(), TYPE_TREE_FORMS.len());
    assert!(audit.type_tree_forms.values().all(|count| *count == 0));
}

#[test]
fn local_expression_failure_examples_keep_the_smallest_five_paths() {
    let failure = FailureClassification {
        bucket: "UnsupportedExpression::InfixOp".to_owned(),
        family: FailureFamily::UnsupportedExpressionSyntaxSemantics,
    };
    let mut first = Audit::default();
    for path in [
        "z.scala", "y.scala", "x.scala", "w.scala", "v.scala", "a.scala",
    ] {
        record_failure(&mut first, failure.clone(), path);
    }
    let mut second = Audit::default();
    for path in [
        "a.scala", "v.scala", "w.scala", "x.scala", "y.scala", "z.scala",
    ] {
        record_failure(&mut second, failure.clone(), path);
    }

    assert_eq!(first, second);
    assert_eq!(
        first
            .failures
            .get("UnsupportedExpression::InfixOp")
            .unwrap()
            .examples
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        ["a.scala", "v.scala", "w.scala", "x.scala", "y.scala"]
    );
    assert_eq!(
        first
            .failures
            .get("UnsupportedExpression::InfixOp")
            .unwrap()
            .files
            .len(),
        6
    );
}

#[test]
fn unsupported_type_tree_failure_order_is_stable() {
    let fixtures = [
        (
            "UnsupportedTypeTree::TypeBoundsTree",
            2,
            ["b.scala", "a.scala"],
        ),
        ("UnsupportedTypeTree::InfixOp::|", 3, ["d.scala", "c.scala"]),
        (
            "UnsupportedTypeTree::SingletonTypeTree",
            2,
            ["f.scala", "e.scala"],
        ),
    ];
    let mut first = Audit::default();
    for (bucket, count, paths) in fixtures {
        let failure = FailureClassification {
            bucket: bucket.to_owned(),
            family: FailureFamily::Other,
        };
        for path in paths.into_iter().take(count.min(2)) {
            record_failure(&mut first, failure.clone(), path);
        }
        for _ in 2..count {
            record_failure(&mut first, failure.clone(), paths[0]);
        }
    }
    let mut second = Audit::default();
    for (bucket, count, paths) in fixtures.into_iter().rev() {
        let failure = FailureClassification {
            bucket: bucket.to_owned(),
            family: FailureFamily::Other,
        };
        for _ in 2..count {
            record_failure(&mut second, failure.clone(), paths[0]);
        }
        for path in paths.into_iter().take(count.min(2)).rev() {
            record_failure(&mut second, failure.clone(), path);
        }
    }

    assert_eq!(
        ordered_type_tree_failures(&first)
            .into_iter()
            .map(|(name, bucket)| (name.clone(), bucket.count))
            .collect::<Vec<_>>(),
        ordered_type_tree_failures(&second)
            .into_iter()
            .map(|(name, bucket)| (name.clone(), bucket.count))
            .collect::<Vec<_>>()
    );
}

fn audit_source(text: &str, path: &str) -> Audit {
    audit_source_inner(text, path, None)
}

fn audit_source_with_classpath(
    text: &str,
    path: &str,
    classpath: SharedClassPath,
    metrics: Rc<RefCell<ResolverMetrics>>,
) -> Audit {
    audit_source_inner(text, path, Some((classpath, metrics)))
}

fn probe_supported_match_cases(
    text: &str,
    path: &str,
    classpath: SharedClassPath,
    profile: &mut MatchProfile,
) -> BTreeMap<String, usize> {
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let Ok(scanner) = ContextualScanner::new(text) else {
        return BTreeMap::new();
    };
    let mut parsed = parse_compilation_unit(
        SourceText::new(text).expect("source text should be valid"),
        source,
        scanner,
        &mut store.names,
    );
    let type_operator_spellings = source_operator_spellings(&parsed.ast, &store.names);
    let mut namer_packages = Packages::new();
    let Ok(index) = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        path,
        &mut store,
        &mut namer_packages,
    ) else {
        return BTreeMap::new();
    };
    let method_ranges = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let method = index.symbol_at(source, tree)?;
            let rhs = definition.rhs?;
            let range = parsed.ast.get(rhs).position?.span().range();
            Some((method, range))
        })
        .collect::<Vec<_>>();
    let mut probe_specs = Vec::new();
    for (_, node) in parsed.ast.iter() {
        let TreeKind::Match(matched) = &node.kind else {
            continue;
        };
        let Some(match_range) = node.position.map(|position| position.span().range()) else {
            continue;
        };
        let Some((method, _)) = method_ranges
            .iter()
            .filter(|(_, range)| {
                range.start() <= match_range.start() && match_range.end() <= range.end()
            })
            .min_by_key(|(_, range)| range.end().saturating_sub(range.start()))
        else {
            continue;
        };
        for case_tree in &matched.cases {
            let Some(case_node) = parsed.ast.try_get(*case_tree) else {
                continue;
            };
            let TreeKind::CaseDef(case) = &case_node.kind else {
                continue;
            };
            let family = pattern_root_shape(&parsed.ast, &store.names, case.pattern);
            if !is_supported_case_family(&family) {
                continue;
            }
            probe_specs.push((
                *method,
                matched.selector,
                *case_tree,
                node.position,
                family,
                case.guard.is_some(),
                extractor_protocol_success(&parsed.ast, case.pattern),
            ));
        }
    }
    let probes = probe_specs
        .into_iter()
        .map(
            |(method, selector, case, position, family, guarded, protocol)| {
                let synthetic_match = parsed.ast.alloc(Tree {
                    kind: TreeKind::Match(Match {
                        selector,
                        cases: vec![case],
                    }),
                    position,
                    ty: (),
                });
                (method, synthetic_match, family, guarded, protocol)
            },
        )
        .collect::<Vec<_>>();

    let packages = Packages::new();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer = typer.with_resolver(Box::new(AuditResolver {
        inner: ClasspathSymbolResolver::new(
            classpath,
            definitions,
            LoadingSession::with_packages(Packages::new()),
        ),
        metrics: Rc::new(RefCell::new(ResolverMetrics::default())),
    }));

    let mut successes = [
        "wildcard",
        "variable/bind",
        "literal",
        "stable identifier/selection",
        "guarded supported case",
        "typed wildcard",
        "typed variable",
        "typed explicit Bind",
        "tuple",
        "alternative",
        "extractor-looking Apply",
        "extractor-looking TypeApply",
        "infix pattern",
    ]
    .into_iter()
    .map(|family| (family.to_owned(), 0))
    .collect::<BTreeMap<_, _>>();
    for (method, match_tree, family, guarded, protocol) in probes {
        let outcome = typer
            .expression_context_for(method)
            .and_then(|context| typer.type_expression(match_tree, context));
        match outcome {
            Ok(_) => {
                let bucket = family_success_bucket(&family).to_owned();
                *successes.entry(bucket.clone()).or_default() += 1;
                profile
                    .typed_case_success_files
                    .entry(bucket)
                    .or_default()
                    .insert(path.to_owned());
                if let Some(protocol) = protocol {
                    *profile
                        .extractor_protocol_successes
                        .entry(protocol.to_owned())
                        .or_default() += 1;
                }
                if guarded {
                    *successes
                        .entry("guarded supported case".to_owned())
                        .or_default() += 1;
                }
            }
            Err(error) => {
                let error_name =
                    typed_case_failure_label(&error, &parsed.ast, &type_operator_spellings);
                *profile
                    .typed_case_failures
                    .entry(error_name.clone())
                    .or_default() += 1;
                profile
                    .typed_case_failure_files
                    .entry(error_name)
                    .or_default()
                    .insert(path.to_owned());
                let boundary = match &error {
                    TyperError::TypedPatternRuntimeTestDeferred { .. } => {
                        Some("generic/erased or unsupported runtime test")
                    }
                    TyperError::TypedPatternRelationDeferred { .. } => {
                        Some("unsupported typed-pattern relation")
                    }
                    TyperError::TypedPatternTypeMismatch { .. } => {
                        Some("typed-pattern type mismatch")
                    }
                    TyperError::UnsupportedTypeTree { .. } => {
                        Some("unsupported type-tree projection")
                    }
                    _ => None,
                };
                if family.starts_with("typed")
                    && let Some(boundary) = boundary
                {
                    *profile
                        .typed_pattern_boundaries
                        .entry(boundary.to_owned())
                        .or_default() += 1;
                }
            }
        }
    }
    successes
}

fn is_supported_case_family(family: &str) -> bool {
    matches!(
        family,
        "wildcard"
            | "variable identifier"
            | "wildcard/identifier/bind"
            | "literal"
            | "stable identifier"
            | "stable selection"
            | "typed wildcard"
            | "typed variable"
            | "typed explicit Bind"
            | "tuple"
            | "alternative"
            | "extractor-looking Apply"
            | "extractor-looking TypeApply"
            | "infix pattern"
    )
}

fn family_success_bucket(family: &str) -> &'static str {
    match family {
        "wildcard" => "wildcard",
        "variable identifier" | "wildcard/identifier/bind" => "variable/bind",
        "literal" => "literal",
        "stable identifier" | "stable selection" => "stable identifier/selection",
        "typed wildcard" => "typed wildcard",
        "typed variable" => "typed variable",
        "typed explicit Bind" => "typed explicit Bind",
        "tuple" => "tuple",
        "alternative" => "alternative",
        "extractor-looking Apply" => "extractor-looking Apply",
        "extractor-looking TypeApply" => "extractor-looking TypeApply",
        "infix pattern" => "infix pattern",
        _ => "other",
    }
}

fn extractor_protocol_success(
    arena: &dotty_core::AstArena<Untyped>,
    pattern: dotty_core::TreeId<Untyped>,
) -> Option<&'static str> {
    match arena.try_get(pattern).map(|node| &node.kind) {
        Some(TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple))) if !tuple.elements.is_empty() => {
            Some("tuple extractor")
        }
        Some(TreeKind::Apply(application)) => {
            let dispatch = peel_type_applications(arena, application.function);
            let selected = matches!(
                arena.try_get(dispatch).map(|node| &node.kind),
                Some(TreeKind::Select(_))
            );
            match application.args.len() {
                0 => Some("Boolean extractor"),
                1 if selected => Some("selected unary extractor"),
                1 => Some("unary Option-like extractor"),
                2 => Some("binary product extractor"),
                _ => Some("N-ary product extractor"),
            }
        }
        Some(TreeKind::TypeApply(_)) => {
            let function = peel_type_applications(arena, pattern);
            match arena.try_get(function).map(|node| &node.kind) {
                Some(TreeKind::Ident(_)) | Some(TreeKind::Select(_)) => {
                    Some("type-applied extractor (unsupported by design)")
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn typed_case_failure_label(
    error: &TyperError,
    arena: &dotty_core::AstArena<Untyped>,
    operator_spellings: &BTreeMap<u32, String>,
) -> String {
    match error {
        TyperError::TuplePatternResolutionDeferred { issue, .. } => {
            format!("tuple::{issue:?}")
        }
        TyperError::ExtractorPatternArgumentUnsupported { issue, .. } => {
            format!("extractor argument::{issue:?}")
        }
        TyperError::UnsupportedTypeTree { .. } => {
            classify_typer_error(error, arena, operator_spellings).bucket
        }
        _ => typer_error_name(error).to_owned(),
    }
}

fn audit_source_inner(
    text: &str,
    path: &str,
    classpath: Option<(SharedClassPath, Rc<RefCell<ResolverMetrics>>)>,
) -> Audit {
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = match ContextualScanner::new(text) {
        Ok(scanner) => scanner,
        Err(_) => {
            return Audit {
                files_attempted: 1,
                parser_failed_files: 1,
                ..Audit::default()
            };
        }
    };
    let parsed = parse_compilation_unit(
        SourceText::new(text).expect("source text should be valid"),
        source,
        scanner,
        &mut store.names,
    );
    let mut audit = collect_local_nodes(&parsed.ast);
    audit.files_attempted = 1;
    audit.expression_forms = collect_expression_histogram(&parsed.ast);
    audit.type_tree_forms = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    audit.recovered_parser_files = usize::from(!parsed.diagnostics.is_empty());
    collect_match_profile(&parsed.ast, path, &store, &mut audit.match_profile);
    for diagnostic in &parsed.diagnostics {
        let failure = classify_parse_diagnostic(diagnostic.kind());
        *audit.parser_diagnostics.entry(failure.bucket).or_default() += 1;
    }
    let mut packages = Packages::new();
    let index = match name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        path,
        &mut store,
        &mut packages,
    ) {
        Ok(index) => index,
        Err(error) => {
            audit.namer_failed_files = 1;
            if audit.local_defdefs != 0 {
                let failure = classify_namer_error(&error);
                for _ in 0..audit.local_defdefs {
                    record_failure(&mut audit, failure.clone(), path);
                }
            }
            return audit;
        }
    };
    if audit.local_defdefs == 0 {
        return audit;
    }
    let probe_classpath = classpath.as_ref().map(|(classpath, _)| classpath.clone());

    let root_methods = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let rhs = definition.rhs?;
            let method = index.symbol_at(source, tree)?;
            let range = parsed.ast.get(rhs).position?.span().range();
            Some((method, rhs, range))
        })
        .collect::<Vec<_>>();

    let local_method_trees = local_method_trees(&parsed.ast);
    let match_pattern_shapes = parsed
        .ast
        .iter()
        .filter_map(|(_, node)| {
            let TreeKind::CaseDef(case) = &node.kind else {
                return None;
            };
            Some((
                case.pattern.index(),
                pattern_root_shape(&parsed.ast, &store.names, case.pattern),
            ))
        })
        .collect::<BTreeMap<_, _>>();

    let typer_packages = Packages::new();
    let type_operator_spellings = source_operator_spellings(&parsed.ast, &store.names);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &typer_packages,
    );
    if let Some((classpath, metrics)) = classpath {
        let resolver = ClasspathSymbolResolver::new(
            classpath,
            definitions,
            LoadingSession::with_packages(packages),
        );
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: resolver,
            metrics,
        }));
    }
    let mut root_failures = Vec::new();
    for (method, rhs, range) in root_methods {
        let outcome = typer
            .expression_context_for(method)
            .and_then(|context| typer.type_expression(rhs, context));
        if let Err(error) = outcome {
            let failure = classify_typer_error(&error, &parsed.ast, &type_operator_spellings);
            if matches!(
                error,
                TyperError::UnsupportedPattern { .. }
                    | TyperError::MalformedCaseDef { .. }
                    | TyperError::EmptyMatchCases { .. }
                    | TyperError::MatchSelectorTypeCannotBeAdapted { .. }
                    | TyperError::MatchCaseResultTypeCannotBeWidened { .. }
                    | TyperError::MatchCaseJoinUnsupported { .. }
                    | TyperError::MalformedVariablePattern { .. }
                    | TyperError::UnsupportedBindPatternBody { .. }
                    | TyperError::PatternTypeMismatch { .. }
                    | TyperError::PatternTypeRelationDeferred { .. }
                    | TyperError::TypedPatternTypeMismatch { .. }
                    | TyperError::TypedPatternRelationDeferred { .. }
                    | TyperError::TypedPatternRuntimeTestDeferred { .. }
                    | TyperError::MalformedStablePatternTarget { .. }
                    | TyperError::UnstablePatternValue { .. }
                    | TyperError::MalformedPatternBinding { .. }
                    | TyperError::WildcardPatternBindingRejected { .. }
                    | TyperError::PatternBindingOutsideCaseScope { .. }
                    | TyperError::PatternBindingScopeConflict { .. }
                    | TyperError::DuplicatePatternBinding { .. }
                    | TyperError::ExtractorQualifierNotFound { .. }
                    | TyperError::ExtractorQualifierMemberNotFound { .. }
                    | TyperError::ExtractorQualifierMemberAmbiguous { .. }
                    | TyperError::ExtractorQualifierShapeUnsupported { .. }
                    | TyperError::ExtractorQualifierNotValueLike { .. }
                    | TyperError::ExtractorQualifierNotStable { .. }
                    | TyperError::ExtractorUnapplyNotFound { .. }
                    | TyperError::ExtractorUnapplyOverloaded { .. }
                    | TyperError::ExtractorUnapplyPolymorphic { .. }
                    | TyperError::ExtractorUnapplyShapeUnsupported { .. }
                    | TyperError::ExtractorPatternConstraintDeferred { .. }
                    | TyperError::UnsupportedExtractorResultProtocol { .. }
                    | TyperError::ExtractorPatternArityUnsupported { .. }
                    | TyperError::BooleanExtractorPatternArityUnsupported { .. }
                    | TyperError::ExtractorResultMemberNotFound { .. }
                    | TyperError::ExtractorResultMemberOverloaded { .. }
                    | TyperError::ExtractorResultMemberUnsupported { .. }
                    | TyperError::ExtractorProductSelectorCountMismatch { .. }
                    | TyperError::UnsupportedExtractorProductProtocol { .. }
                    | TyperError::ExtractorPatternArgumentUnsupported { .. }
            ) {
                collect_match_readiness(
                    &parsed.ast,
                    rhs,
                    path,
                    match_first_error_label(&error, &match_pattern_shapes),
                    &mut audit.match_readiness,
                );
            }
            root_failures.push((range, failure));
        }
    }

    for (tree, range) in local_method_trees {
        if typer.source_typed_index().get(source, tree).is_some() {
            audit.typed_local_defdefs += 1;
        } else {
            let kind = root_failures
                .iter()
                .filter(|(parent, _)| {
                    parent.start() <= range.start() && range.end() <= parent.end()
                })
                .min_by_key(|(parent, _)| parent.end().saturating_sub(parent.start()))
                .map(|(_, failure)| failure.clone())
                .unwrap_or_else(|| FailureClassification {
                    bucket: "NoSuccessfulEnclosingMethodTyping".to_owned(),
                    family: FailureFamily::Other,
                });
            record_failure(&mut audit, kind, path);
        }
    }
    if let Some(classpath) = probe_classpath {
        let successes =
            probe_supported_match_cases(text, path, classpath, &mut audit.match_profile);
        audit.match_profile.typed_case_successes = successes;
    }
    audit
}

fn print_match_readiness(readiness: &MatchReadiness) {
    println!("match_readiness:");
    println!(
        "  first_blocker_methods={}",
        readiness.first_blocker_methods
    );
    println!(
        "  structural_matches_in_first_blocker_methods={}",
        readiness.matches
    );
    println!("  cases_in_first_blocker_methods={}", readiness.cases);
    println!("  guarded_cases={}", readiness.guarded_cases);
    println!(
        "  unguarded_cases={}",
        readiness.cases - readiness.guarded_cases
    );
    println!("  first_blocker_errors:");
    for (error, count) in &readiness.first_blocker_errors {
        let files = readiness
            .first_blocker_error_files
            .get(error)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {error}={count} files=[{}]", files.join(", "));
    }
    println!("  pattern_root_shapes:");
    for (shape, count) in &readiness.pattern_shapes {
        let files = readiness
            .pattern_shape_files
            .get(shape)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {shape}={count} files=[{}]", files.join(", "));
    }
}

fn collect_match_readiness(
    arena: &dotty_core::AstArena<Untyped>,
    root: dotty_core::TreeId<Untyped>,
    path: &str,
    first_error: String,
    readiness: &mut MatchReadiness,
) {
    readiness.first_blocker_methods += 1;
    *readiness
        .first_blocker_errors
        .entry(first_error.clone())
        .or_default() += 1;
    readiness
        .first_blocker_error_files
        .entry(first_error)
        .or_default()
        .insert(path.to_owned());
    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    let mut visited = HashSet::new();
    let mut pending = VecDeque::from([root]);
    while let Some(tree) = pending.pop_front() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = nodes.get(&tree.index()) else {
            continue;
        };
        if let TreeKind::Match(matched) = &node.kind {
            readiness.matches += 1;
            for case_tree in &matched.cases {
                let Some(case_node) = nodes.get(&case_tree.index()) else {
                    continue;
                };
                let TreeKind::CaseDef(case) = &case_node.kind else {
                    continue;
                };
                readiness.cases += 1;
                if case.guard.is_some() {
                    readiness.guarded_cases += 1;
                }
                let Some(pattern_node) = nodes.get(&case.pattern.index()) else {
                    continue;
                };
                let shape = match &pattern_node.kind {
                    TreeKind::Ident(_) | TreeKind::Bind(_) => "wildcard/identifier/bind",
                    TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
                        "literal"
                    }
                    TreeKind::Typed(_) => "typed pattern",
                    TreeKind::Alternative(_) => "alternative",
                    TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => "tuple",
                    TreeKind::Apply(_) | TreeKind::TypeApply(_) => {
                        "extractor-looking Apply/TypeApply"
                    }
                    TreeKind::UnApply(_) => "UnApply",
                    _ => "other",
                };
                *readiness
                    .pattern_shapes
                    .entry(shape.to_owned())
                    .or_default() += 1;
                readiness
                    .pattern_shape_files
                    .entry(shape.to_owned())
                    .or_default()
                    .insert(path.to_owned());
            }
        }
        pending.extend(term_expression_children(&node.kind));
        if let TreeKind::DefDef(definition) = &node.kind {
            pending.extend(definition.rhs);
        }
    }
}

fn collect_match_profile(
    arena: &dotty_core::AstArena<Untyped>,
    path: &str,
    store: &SemanticStore,
    profile: &mut MatchProfile,
) {
    for (_, node) in arena.iter() {
        let TreeKind::Match(matched) = &node.kind else {
            continue;
        };
        profile.matches += 1;
        for case_tree in &matched.cases {
            let Some(case_node) = arena.try_get(*case_tree) else {
                continue;
            };
            let TreeKind::CaseDef(case) = &case_node.kind else {
                continue;
            };
            profile.cases += 1;
            profile.guarded_cases += usize::from(case.guard.is_some());
            let shape = pattern_root_shape(arena, &store.names, case.pattern);
            *profile.pattern_roots.entry(shape.clone()).or_default() += 1;
            profile
                .pattern_root_files
                .entry(shape)
                .or_default()
                .insert(path.to_owned());
            if matches!(
                arena.try_get(case.pattern).map(|pattern| &pattern.kind),
                Some(TreeKind::Apply(_)) | Some(TreeKind::TypeApply(_))
            ) {
                profile.extractor_files.insert(path.to_owned());
            }
            collect_pattern_features(arena, &store.names, case.pattern, profile);
        }
    }
}

fn pattern_root_shape(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    tree: dotty_core::TreeId<Untyped>,
) -> String {
    let Some(node) = arena.try_get(tree) else {
        return "other".to_owned();
    };
    match &node.kind {
        TreeKind::Ident(ident) if names.resolve(ident.name.text()) == "_" && !ident.backquoted => {
            "wildcard".to_owned()
        }
        TreeKind::Ident(ident) if is_variable_pattern_ident(names, ident.name, ident.backquoted) => {
            "variable identifier".to_owned()
        }
        TreeKind::Ident(_) => "stable identifier".to_owned(),
        TreeKind::Select(_) => "stable selection".to_owned(),
        TreeKind::Bind(binding)
            if arena.try_get(binding.body).is_some_and(|body| {
                matches!(&body.kind, TreeKind::Typed(typed) if is_wildcard_or_variable_pattern(arena, names, typed.expr))
            }) =>
        {
            "typed explicit Bind".to_owned()
        }
        TreeKind::Bind(_) => "wildcard/identifier/bind".to_owned(),
        TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
            "literal".to_owned()
        }
        TreeKind::Typed(typed) => {
            let Some(expr) = arena.try_get(typed.expr) else {
                return "typed pattern".to_owned();
            };
            match &expr.kind {
                TreeKind::Ident(ident)
                    if names.resolve(ident.name.text()) == "_" && !ident.backquoted =>
                {
                    "typed wildcard".to_owned()
                }
                TreeKind::Ident(ident)
                    if is_variable_pattern_ident(names, ident.name, ident.backquoted) =>
                {
                    "typed variable".to_owned()
                }
                _ => "typed pattern".to_owned(),
            }
        }
        TreeKind::Alternative(_) => "alternative".to_owned(),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => "tuple".to_owned(),
        TreeKind::Apply(_) => "extractor-looking Apply".to_owned(),
        TreeKind::TypeApply(_) => "extractor-looking TypeApply".to_owned(),
        TreeKind::UnApply(_) => "UnApply".to_owned(),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => "infix pattern".to_owned(),
        _ => "other".to_owned(),
    }
}

fn is_variable_pattern_ident(
    names: &dotty_core::NameInterner,
    name: dotty_core::Name,
    backquoted: bool,
) -> bool {
    if backquoted || !name.is_term() {
        return false;
    }
    let spelling = names.resolve(name.text());
    if matches!(spelling, "_" | "true" | "false" | "null") {
        return false;
    }
    spelling
        .chars()
        .next()
        .is_some_and(|first| first == '_' || (first.is_alphabetic() && first.is_lowercase()))
}

fn is_wildcard_or_variable_pattern(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    tree: dotty_core::TreeId<Untyped>,
) -> bool {
    arena.try_get(tree).is_some_and(|node| match &node.kind {
        TreeKind::Ident(ident) => {
            !ident.backquoted
                && (names.resolve(ident.name.text()) == "_"
                    || is_variable_pattern_ident(names, ident.name, false))
        }
        _ => false,
    })
}

fn collect_pattern_features(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    root: dotty_core::TreeId<Untyped>,
    profile: &mut MatchProfile,
) {
    if matches!(
        arena.try_get(root).map(|node| &node.kind),
        Some(TreeKind::TypeApply(_))
    ) {
        collect_extractor_profile(arena, names, root, profile);
    }
    let mut visited = HashSet::new();
    let mut pending = vec![root];
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = arena.try_get(tree) else {
            continue;
        };
        match &node.kind {
            TreeKind::NamedArg(named) => {
                profile.named_pattern_arguments += 1;
                pending.push(named.arg);
            }
            TreeKind::Apply(application) => {
                collect_extractor_profile(arena, names, tree, profile);
                pending.extend(application.args.iter().copied());
            }
            TreeKind::TypeApply(application) => pending.push(application.function),
            TreeKind::Typed(typed) => pending.push(typed.expr),
            TreeKind::Bind(binding) => pending.push(binding.body),
            TreeKind::Alternative(alternative) => {
                pending.extend(alternative.alternatives.iter().copied());
            }
            TreeKind::UnApply(unapply) => pending.extend(unapply.patterns.iter().copied()),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                if tuple.elements.is_empty() {
                    profile.empty_tuple_unit_patterns += 1;
                }
                pending.extend(tuple.elements.iter().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                profile.infix_pattern_forms += 1;
                pending.extend([infix.left, infix.right]);
            }
            TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)) => {
                if names.resolve(postfix.op.text()) == "*" {
                    profile.sequence_wildcards += 1;
                }
                pending.push(postfix.operand);
            }
            _ => {}
        }
    }
}

fn collect_extractor_profile(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    root: dotty_core::TreeId<Untyped>,
    profile: &mut MatchProfile,
) {
    let root_is_type_apply = matches!(
        arena.try_get(root).map(|node| &node.kind),
        Some(TreeKind::TypeApply(_))
    );
    *profile
        .extractor_roots
        .entry(
            if root_is_type_apply {
                "TypeApply root"
            } else {
                "Apply root"
            }
            .to_owned(),
        )
        .or_default() += 1;
    let mut cursor = root;
    let mut type_applied = false;
    while let Some(node) = arena.try_get(cursor) {
        match &node.kind {
            TreeKind::TypeApply(application) => {
                type_applied = true;
                cursor = application.function;
            }
            TreeKind::Apply(application) => {
                type_applied |= count_type_applications(arena, application.function) > 0;
                *profile
                    .extractor_argument_counts
                    .entry(application.args.len())
                    .or_default() += 1;
                for argument in &application.args {
                    let shape = pattern_root_shape(arena, names, *argument);
                    *profile.extractor_nested_roots.entry(shape).or_default() += 1;
                }
                let function = peel_type_applications(arena, application.function);
                let dispatch = match arena.try_get(function).map(|node| &node.kind) {
                    Some(TreeKind::Ident(_)) => "simple extractor identifier",
                    Some(TreeKind::Select(_)) => "selected extractor",
                    _ => "other extractor function shape",
                };
                *profile
                    .extractor_dispatch
                    .entry(dispatch.to_owned())
                    .or_default() += 1;
                if type_applied {
                    profile.extractor_type_applied += 1;
                }
                return;
            }
            _ => return,
        }
    }
    if type_applied {
        profile.extractor_type_applied += 1;
    }
}

fn count_type_applications(
    arena: &dotty_core::AstArena<Untyped>,
    mut tree: dotty_core::TreeId<Untyped>,
) -> usize {
    let mut count = 0;
    while let Some(node) = arena.try_get(tree) {
        let TreeKind::TypeApply(application) = &node.kind else {
            break;
        };
        count += 1;
        tree = application.function;
    }
    count
}

fn peel_type_applications(
    arena: &dotty_core::AstArena<Untyped>,
    mut tree: dotty_core::TreeId<Untyped>,
) -> dotty_core::TreeId<Untyped> {
    while let Some(node) = arena.try_get(tree) {
        let TreeKind::TypeApply(application) = &node.kind else {
            break;
        };
        tree = application.function;
    }
    tree
}

fn print_match_profile(profile: &MatchProfile) {
    println!("match_corpus_profile:");
    println!("  matches={}", profile.matches);
    println!("  cases={}", profile.cases);
    println!("  guarded_cases={}", profile.guarded_cases);
    println!(
        "  unguarded_cases={}",
        profile.cases - profile.guarded_cases
    );
    println!("  pattern_root_shapes:");
    for (shape, count) in &profile.pattern_roots {
        let files = profile
            .pattern_root_files
            .get(shape)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {shape}={count} files=[{}]", files.join(", "));
    }
    println!("  typed_case_successes:");
    for (family, count) in &profile.typed_case_successes {
        let files = profile
            .typed_case_success_files
            .get(family)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {family}={count} files=[{}]", files.join(", "));
    }
    println!("  successful_extractor_protocols:");
    for (protocol, count) in &profile.extractor_protocol_successes {
        println!("    {protocol}={count}");
    }
    println!("  typed_pattern_boundaries:");
    for (boundary, count) in &profile.typed_pattern_boundaries {
        println!("    {boundary}={count}");
    }
    println!("  typed_case_first_failures:");
    let unsupported_type_tree_total = profile
        .typed_case_failures
        .iter()
        .filter(|(failure, _)| failure.starts_with("UnsupportedTypeTree::"))
        .map(|(_, count)| *count)
        .sum::<usize>();
    println!("    UnsupportedTypeTree={unsupported_type_tree_total}");
    for (failure, count) in &profile.typed_case_failures {
        let files = profile
            .typed_case_failure_files
            .get(failure)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {failure}={count} files=[{}]", files.join(", "));
    }
    let mut ranked = profile
        .typed_case_failures
        .iter()
        .map(|(failure, count)| (failure.as_str(), *count))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    println!("  typed_case_first_failure_top_10:");
    for (failure, count) in ranked.into_iter().take(10) {
        println!("    {failure}={count}");
    }
    println!("  extractor_root_shapes:");
    for (shape, count) in &profile.extractor_roots {
        println!("    {shape}={count}");
    }
    println!("  extractor_dispatch:");
    for (shape, count) in &profile.extractor_dispatch {
        println!("    {shape}={count}");
    }
    println!(
        "  extractor_type_applied={}",
        profile.extractor_type_applied
    );
    println!("  extractor_argument_counts:");
    for (count, occurrences) in &profile.extractor_argument_counts {
        println!("    {count}={occurrences}");
    }
    println!("  extractor_nested_argument_roots:");
    for (shape, count) in &profile.extractor_nested_roots {
        println!("    {shape}={count}");
    }
    println!(
        "  sequence_wildcard_occurrences={}",
        profile.sequence_wildcards
    );
    println!(
        "  named_pattern_arguments={}",
        profile.named_pattern_arguments
    );
    println!(
        "  empty_tuple_unit_patterns={}",
        profile.empty_tuple_unit_patterns
    );
    println!("  infix_pattern_forms={}", profile.infix_pattern_forms);
    println!(
        "  extractor_representative_files=[{}]",
        profile
            .extractor_files
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn audit_classpath_from_environment() -> SharedClassPath {
    let java_home = PathBuf::from(
        std::env::var_os("JAVA_HOME").expect("JAVA_HOME must identify the JDK used by the audit"),
    );
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .expect("SCALA39_JDK_RELEASE must explicitly select the JDK classfile release")
        .parse::<u16>()
        .expect("SCALA39_JDK_RELEASE must be a numeric Java feature release");
    let classpath = std::env::var_os("SCALA39_CLASSPATH").expect(
        "SCALA39_CLASSPATH must list the pinned Scala artifact jars; ambient CLASSPATH is ignored",
    );
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
        JdkClassPath::new(java_home.join("jmods"))
            .expect("JAVA_HOME/jmods should be readable by the classpath loader"),
    )];
    let jars = std::env::split_paths(&classpath).collect::<Vec<_>>();
    assert!(
        !jars.is_empty(),
        "SCALA39_CLASSPATH must contain Scala jars"
    );
    for jar in jars {
        assert!(
            jar.is_file(),
            "classpath jar does not exist: {}",
            jar.display()
        );
        entries.push(Box::new(
            JarClassPath::new(jar.clone(), release)
                .unwrap_or_else(|error| panic!("cannot index {}: {error}", jar.display())),
        ));
    }
    SharedClassPath(Arc::new(CompositeClassPath::new(entries)))
}

fn print_resolver_metrics(metrics: &ResolverMetrics) {
    println!("resolver_metrics:");
    println!(
        "  resolver_package_requests={}",
        metrics.resolver_package_requests
    );
    println!(
        "  external_package_requests={}",
        metrics.external_package_requests()
    );
    println!("  external_package_successes={}", metrics.package_successes);
    println!(
        "  external_package_unresolved={}",
        metrics.package_unresolved
    );
    println!("  external_package_errors={}", metrics.package_errors);
    println!("  source_package_reuse={}", metrics.package_source_reuse);
    println!(
        "  resolver_member_requests={}",
        metrics.resolver_member_requests
    );
    println!(
        "  external_member_requests={}",
        metrics.external_member_requests()
    );
    println!("  external_member_successes={}", metrics.member_successes);
    println!(
        "  external_class_symbol_successes={}",
        metrics.class_symbol_successes
    );
    println!(
        "  external_non_class_member_successes={}",
        metrics
            .member_successes
            .saturating_sub(metrics.class_symbol_successes)
    );
    println!("  source_member_reuse={}", metrics.member_source_reuse);
    println!("  external_member_unresolved={}", metrics.member_unresolved);
    println!("  external_member_errors={}", metrics.member_errors);
    println!("  distinct_packages={}", metrics.packages.len());
    println!("  distinct_classes={}", metrics.classes.len());
    println!("  distinct_members={}", metrics.members.len());
    println!(
        "  classloader_success_gate={}",
        if !metrics.classes.is_empty() && !metrics.members.is_empty() {
            "passed"
        } else {
            "BLOCKED: external members not materialized"
        }
    );
    println!("resolver_581_comparison:");
    for (name, current, baseline) in [
        (
            "external_package_successes",
            metrics.package_successes,
            1_965,
        ),
        (
            "external_package_unresolved",
            metrics.package_unresolved,
            5_816,
        ),
        ("external_package_errors", metrics.package_errors, 0),
        ("external_class_materializations", metrics.classes.len(), 0),
        (
            "external_non_class_member_successes",
            metrics
                .member_successes
                .saturating_sub(metrics.class_symbol_successes),
            0,
        ),
        (
            "external_member_unresolved",
            metrics.member_unresolved,
            3_884,
        ),
        ("external_member_errors", metrics.member_errors, 91),
        ("distinct_packages", metrics.packages.len(), 23),
    ] {
        println!(
            "  {name}={current} (baseline={baseline}, delta={})",
            signed_delta(current, baseline)
        );
    }
    println!("  member_error_kinds:");
    for (kind, count) in &metrics.member_error_kinds {
        println!("    {kind}={count}");
    }
    println!("  most_requested_unresolved_member_names:");
    let mut unresolved = metrics.unresolved_member_names.iter().collect::<Vec<_>>();
    unresolved.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (name, count) in unresolved.into_iter().take(20) {
        println!("    {name}={count}");
    }
}

fn collect_local_nodes(arena: &dotty_core::AstArena<Untyped>) -> Audit {
    let local_stats = local_stat_trees(arena);
    let local_pattern_binds = local_pattern_bind_trees(arena);
    let parameter_trees = arena
        .iter()
        .flat_map(|(_, node)| match &node.kind {
            TreeKind::DefDef(definition) => definition
                .type_params
                .iter()
                .copied()
                .chain(definition.value_param_clauses.iter().flatten().copied())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<HashSet<_>>();
    let mut audit = Audit::default();
    for (tree, node) in arena.iter() {
        if parameter_trees.contains(&tree) {
            continue;
        }
        if !local_stats.contains(&tree) && !local_pattern_binds.contains(&tree) {
            continue;
        }
        let bucket = match &node.kind {
            TreeKind::ValDef(_) => Some("local_val_defs"),
            TreeKind::DefDef(_) => Some("local_def_defs"),
            TreeKind::TypeDef(definition) => {
                if matches!(arena.get(definition.rhs).kind, TreeKind::Template(_)) {
                    Some("local_classes")
                } else {
                    Some("local_type_defs")
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => Some("local_objects"),
            TreeKind::Import(_) => Some("local_imports"),
            TreeKind::Bind(_) => Some("local_pattern_bindings"),
            _ => None,
        };
        if let Some(bucket) = bucket {
            *audit.buckets.entry(bucket.to_owned()).or_default() += 1;
            audit.local_definitions += 1;
            if bucket == "local_def_defs" {
                audit.local_defdefs += 1;
            }
        }
    }
    audit
}

fn local_method_trees(
    arena: &dotty_core::AstArena<Untyped>,
) -> Vec<(dotty_core::TreeId<Untyped>, TextRange)> {
    local_stat_trees(arena)
        .into_iter()
        .filter_map(|tree| {
            let node = arena.get(tree);
            matches!(node.kind, TreeKind::DefDef(_))
                .then_some((tree, node.position?.span().range()))
        })
        .collect()
}

fn local_stat_trees(arena: &dotty_core::AstArena<Untyped>) -> HashSet<dotty_core::TreeId<Untyped>> {
    arena
        .iter()
        .flat_map(|(_, node)| match &node.kind {
            TreeKind::Block(block) => block.stats.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn local_pattern_type_trees(
    arena: &dotty_core::AstArena<Untyped>,
) -> HashSet<dotty_core::TreeId<Untyped>> {
    let mut pending = local_stat_trees(arena)
        .into_iter()
        .flat_map(|tree| match &arena.get(tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => definition.patterns.clone(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut visited = HashSet::new();
    let mut types = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        match &arena.get(tree).kind {
            TreeKind::Typed(typed) => {
                types.insert(typed.tpt);
                pending.push(typed.expr);
            }
            TreeKind::Bind(binding) => pending.push(binding.body),
            TreeKind::Alternative(alternative) => {
                pending.extend(alternative.alternatives.iter().copied());
            }
            TreeKind::UnApply(unapply) => pending.extend(unapply.patterns.iter().copied()),
            TreeKind::Apply(application) => pending.extend(application.args.iter().copied()),
            TreeKind::NamedArg(argument) => pending.push(argument.arg),
            TreeKind::Annotated(annotated) => pending.push(annotated.expr),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                pending.extend(tuple.elements.iter().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                pending.push(infix.left);
                pending.push(infix.right);
            }
            _ => {}
        }
    }
    types
}

fn local_pattern_bind_trees(
    arena: &dotty_core::AstArena<Untyped>,
) -> HashSet<dotty_core::TreeId<Untyped>> {
    let roots = local_stat_trees(arena)
        .into_iter()
        .flat_map(|tree| match &arena.get(tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => definition.patterns.clone(),
            _ => Vec::new(),
        });
    let mut pending = roots.collect::<Vec<_>>();
    let mut visited = HashSet::new();
    let mut binds = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        match &arena.get(tree).kind {
            TreeKind::Bind(binding) => {
                binds.insert(tree);
                pending.push(binding.body);
            }
            TreeKind::Alternative(alternative) => {
                pending.extend(alternative.alternatives.iter().copied());
            }
            TreeKind::UnApply(unapply) => pending.extend(unapply.patterns.iter().copied()),
            TreeKind::Typed(typed) => pending.push(typed.expr),
            TreeKind::Annotated(annotated) => pending.push(annotated.expr),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                pending.extend(tuple.elements.iter().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                pending.push(infix.left);
                pending.push(infix.right);
            }
            _ => {}
        }
    }
    binds
}

fn record_failure(audit: &mut Audit, failure: FailureClassification, path: &str) {
    let bucket = audit.failures.entry(failure.bucket).or_default();
    bucket.family = failure.family;
    bucket.count += 1;
    bucket.files.insert(path.to_owned());
    bucket.examples.insert(path.to_owned());
    bucket.examples = bucket.examples.iter().take(5).cloned().collect();
}

fn classify_typer_error(
    error: &TyperError,
    arena: &dotty_core::AstArena<Untyped>,
    operator_spellings: &BTreeMap<u32, String>,
) -> FailureClassification {
    match error {
        TyperError::UnsupportedExpression { tree_index, .. } => {
            let node = arena
                .iter()
                .find(|(tree, _)| tree.index() == *tree_index)
                .map(|(_, node)| &node.kind);
            if let Some(TreeKind::PhaseSpecific(UntypedNode::Error(error))) = node {
                return FailureClassification {
                    bucket: format!("Parser::ErrorNode::{}", parser_error_node_name(error.kind)),
                    family: FailureFamily::ParserNamer,
                };
            }
            let form = node.map_or("Unknown", tree_kind_label);
            FailureClassification {
                bucket: format!("UnsupportedExpression::{form}"),
                family: FailureFamily::UnsupportedExpressionSyntaxSemantics,
            }
        }
        TyperError::LocalBlockDeclarationDeferred { kind, .. } => FailureClassification {
            bucket: format!("LocalBlockDeclarationDeferred::{kind}"),
            family: FailureFamily::LocalDeclarationDeferral,
        },
        TyperError::UnsupportedTypeTree { tree_index, .. } => {
            let kind = arena
                .iter()
                .find(|(tree, _)| tree.index() == *tree_index)
                .map(|(_, node)| &node.kind);
            let shape = kind.map_or("<other>", |kind| {
                type_tree_shape_label(kind, *tree_index, operator_spellings)
            });
            FailureClassification {
                bucket: format!("UnsupportedTypeTree::{shape}"),
                family: FailureFamily::Other,
            }
        }
        TyperError::ImportQualifierNotFound { .. }
        | TyperError::TypeNameNotFound { .. }
        | TyperError::TermNameNotFound { .. }
        | TyperError::SymbolResolution { .. }
        | TyperError::MemberNotFound { .. }
        | TyperError::MemberLookup(_) => FailureClassification {
            bucket: typer_error_name(error).to_owned(),
            family: FailureFamily::ResolutionClasspathEnvironment,
        },
        _ => {
            let bucket = typer_error_name(error);
            let family = if is_type_relation_inference_or_completion(bucket) {
                FailureFamily::TypeRelationInferenceCompletion
            } else {
                FailureFamily::Other
            };
            FailureClassification {
                bucket: bucket.to_owned(),
                family,
            }
        }
    }
}

fn source_operator_spellings(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
) -> BTreeMap<u32, String> {
    arena
        .iter()
        .filter_map(|(tree, node)| {
            let operator = match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => Some(infix.op),
                TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)) => Some(postfix.op),
                _ => None,
            }?;
            Some((tree.index(), names.resolve(operator.text()).to_owned()))
        })
        .collect()
}

fn type_tree_shape_label(
    kind: &TreeKind<Untyped>,
    tree_index: u32,
    operator_spellings: &BTreeMap<u32, String>,
) -> &'static str {
    match kind {
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => {
            match operator_spellings.get(&tree_index).map(String::as_str) {
                Some("|") => "InfixOp::|",
                Some("&") => "InfixOp::&",
                _ => "InfixOp::<other>",
            }
        }
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => {
            match operator_spellings.get(&tree_index).map(String::as_str) {
                Some("*") => "PostfixOp::*",
                _ => "PostfixOp::<other>",
            }
        }
        _ => tree_kind_label(kind),
    }
}

fn classify_namer_error(error: &NamerError) -> FailureClassification {
    let bucket = match error {
        NamerError::RootIsNotPackage { .. } => "NamerError::RootIsNotPackage",
        NamerError::DuplicateSourceTreeSymbol { .. } => "NamerError::DuplicateSourceTreeSymbol",
        NamerError::DuplicateDerivedSourceTreeSymbol { .. } => {
            "NamerError::DuplicateDerivedSourceTreeSymbol"
        }
        NamerError::ConflictingSourceProvenance { .. } => "NamerError::ConflictingSourceProvenance",
        NamerError::DuplicateExtensionPrefixClauses { .. } => {
            "NamerError::DuplicateExtensionPrefixClauses"
        }
        NamerError::DuplicateSourceExportSite { .. } => "NamerError::DuplicateSourceExportSite",
        NamerError::DuplicateDeclarationScope { .. } => "NamerError::DuplicateDeclarationScope",
        NamerError::DuplicateDeclarationContext { .. } => "NamerError::DuplicateDeclarationContext",
        NamerError::MalformedAstShape { .. } => "NamerError::MalformedAstShape",
        NamerError::InvalidVisibilityQualifier { protected, .. } => {
            if *protected {
                "NamerError::InvalidVisibilityQualifier::protected"
            } else {
                "NamerError::InvalidVisibilityQualifier::private"
            }
        }
        NamerError::UnsupportedGivenNameShape {
            unsupported_kind, ..
        } => {
            return FailureClassification {
                bucket: format!("NamerError::UnsupportedGivenNameShape::{unsupported_kind}"),
                family: FailureFamily::ParserNamer,
            };
        }
        NamerError::AnonymousGivenWithoutParents { .. } => {
            "NamerError::AnonymousGivenWithoutParents"
        }
    };
    FailureClassification {
        bucket: bucket.to_owned(),
        family: FailureFamily::ParserNamer,
    }
}

fn match_first_error_label(error: &TyperError, pattern_shapes: &BTreeMap<u32, String>) -> String {
    match error {
        TyperError::UnsupportedPattern { pattern_kind, .. } => {
            format!("UnsupportedPattern::{}", pattern_kind.as_str())
        }
        TyperError::UnsupportedBindPatternBody { .. } => {
            "UnsupportedPattern::binding body".to_owned()
        }
        TyperError::MalformedVariablePattern { .. }
        | TyperError::MalformedPatternBinding { .. }
        | TyperError::WildcardPatternBindingRejected { .. }
        | TyperError::PatternBindingOutsideCaseScope { .. }
        | TyperError::PatternBindingScopeConflict { .. }
        | TyperError::DuplicatePatternBinding { .. }
        | TyperError::PatternBindingInAlternative { .. } => {
            format!("pattern binding: {}", typer_error_name(error))
        }
        TyperError::PatternTypeMismatch { tree_index, .. }
        | TyperError::PatternTypeRelationDeferred { tree_index, .. } => {
            let shape = pattern_shapes
                .get(tree_index)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned());
            format!("{shape}: {}", typer_error_name(error))
        }
        TyperError::TypedPatternTypeMismatch { .. }
        | TyperError::TypedPatternRelationDeferred { .. }
        | TyperError::TypedPatternRuntimeTestDeferred { .. } => {
            format!("typed pattern: {}", typer_error_name(error))
        }
        _ => typer_error_name(error).to_owned(),
    }
}

fn typer_error_name(error: &TyperError) -> &'static str {
    match error {
        TyperError::UnknownSymbol { .. } => "UnknownSymbol",
        TyperError::SourceProvenanceMissing { .. } => "SourceProvenanceMissing",
        TyperError::DeclarationContextMissing { .. } => "DeclarationContextMissing",
        TyperError::SourceContextMissing { .. } => "SourceContextMissing",
        TyperError::ExpressionOwnerMissing { .. } => "ExpressionOwnerMissing",
        TyperError::ExpressionOwnerKindUnsupported { .. } => "ExpressionOwnerKindUnsupported",
        TyperError::ExpressionMethodScopeMissing { .. } => "ExpressionMethodScopeMissing",
        TyperError::ExpressionMethodScopeOwnerMismatch { .. } => {
            "ExpressionMethodScopeOwnerMismatch"
        }
        TyperError::ExpressionOwnerSourceContextMissing { .. } => {
            "ExpressionOwnerSourceContextMissing"
        }
        TyperError::ExpressionOwnerDeclarationContextMissing { .. } => {
            "ExpressionOwnerDeclarationContextMissing"
        }
        TyperError::ExpressionLocalScopeMissing { .. } => "ExpressionLocalScopeMissing",
        TyperError::ExpressionLocalScopeStackMissing { .. } => "ExpressionLocalScopeStackMissing",
        TyperError::ExpressionLocalScopeStackForeign { .. } => "ExpressionLocalScopeStackForeign",
        TyperError::UnsupportedImportContext { .. } => "UnsupportedImportContext",
        TyperError::ImportQualifierNotFound { .. } => "ImportQualifierNotFound",
        TyperError::ImportQualifierNotStable { .. } => "ImportQualifierNotStable",
        TyperError::MalformedSourceImport { .. } => "MalformedSourceImport",
        TyperError::UnsupportedSymbolCompletion { .. } => "UnsupportedSymbolCompletion",
        TyperError::MissingClassScope { .. } => "MissingClassScope",
        TyperError::MalformedClassInfo { .. } => "MalformedClassInfo",
        TyperError::AnonymousClassInstantiationDeferred { .. } => {
            "AnonymousClassInstantiationDeferred"
        }
        TyperError::NewTargetNotClass { .. } => "NewTargetNotClass",
        TyperError::TraitInstantiation { .. } => "TraitInstantiation",
        TyperError::AbstractClassInstantiation { .. } => "AbstractClassInstantiation",
        TyperError::ModuleInstantiation { .. } => "ModuleInstantiation",
        TyperError::PackageInstantiation { .. } => "PackageInstantiation",
        TyperError::TypeParameterInstantiation { .. } => "TypeParameterInstantiation",
        TyperError::NewClassInfoUnavailable { .. } => "NewClassInfoUnavailable",
        TyperError::ConstructorLookupNormalization { .. } => "ConstructorLookupNormalization",
        TyperError::MalformedConstructorBucket { .. } => "MalformedConstructorBucket",
        TyperError::MalformedConstructorCandidate { .. } => "MalformedConstructorCandidate",
        TyperError::ConstructorApplicationUnavailable { .. } => "ConstructorApplicationUnavailable",
        TyperError::ConstructorOverloadResolutionDeferred { .. } => {
            "ConstructorOverloadResolutionDeferred"
        }
        TyperError::ConstructorApplicationNoApplicable { .. } => {
            "ConstructorApplicationNoApplicable"
        }
        TyperError::AmbiguousConstructorApplication { .. } => "AmbiguousConstructorApplication",
        TyperError::ConstructorOverloadResolutionRequiresUnsupportedCandidate { .. } => {
            "ConstructorOverloadResolutionRequiresUnsupportedCandidate"
        }
        TyperError::ConstructorPolymorphicApplicationDeferred { .. } => {
            "ConstructorPolymorphicApplicationDeferred"
        }
        TyperError::ConstructorCallableNotPolymorphic { .. } => "ConstructorCallableNotPolymorphic",
        TyperError::ConstructorTypeArgumentArityMismatch { .. } => {
            "ConstructorTypeArgumentArityMismatch"
        }
        TyperError::ConstructorInstanceClassMismatch { .. } => "ConstructorInstanceClassMismatch",
        TyperError::ConstructorPolyInstantiationFailed { .. } => {
            "ConstructorPolyInstantiationFailed"
        }
        TyperError::ConstructorResultTypeMismatch { .. } => "ConstructorResultTypeMismatch",
        TyperError::ConstructorResultTypeCheckUnsupported { .. } => {
            "ConstructorResultTypeCheckUnsupported"
        }
        TyperError::ConstructorTypeArgumentBoundViolation { .. } => {
            "ConstructorTypeArgumentBoundViolation"
        }
        TyperError::UnsupportedConstructorTypeArgumentBounds { .. } => {
            "UnsupportedConstructorTypeArgumentBounds"
        }
        TyperError::ConstructorTypeArgumentBoundCheckUnsupported { .. } => {
            "ConstructorTypeArgumentBoundCheckUnsupported"
        }
        TyperError::UnconstrainedConstructorTypeParameter { .. } => {
            "UnconstrainedConstructorTypeParameter"
        }
        TyperError::UnsupportedRawGenericSecondaryConstructorInference { .. } => {
            "UnsupportedRawGenericSecondaryConstructorInference"
        }
        TyperError::ConflictingConstructorInferenceConstraints { .. } => {
            "ConflictingConstructorInferenceConstraints"
        }
        TyperError::UnsupportedConstructorInferenceShape { .. } => {
            "UnsupportedConstructorInferenceShape"
        }
        TyperError::UnableToFinalizeRawGenericNewInstanceType { .. } => {
            "UnableToFinalizeRawGenericNewInstanceType"
        }
        TyperError::MalformedClassParent { .. } => "MalformedClassParent",
        TyperError::UnresolvedParentClassKind { .. } => "UnresolvedParentClassKind",
        TyperError::HigherKindedTypeParameterDeferred { .. } => "HigherKindedTypeParameterDeferred",
        TyperError::HigherKindedTypeAliasDeferred { .. } => "HigherKindedTypeAliasDeferred",
        TyperError::InvalidCompletedBounds { .. } => "InvalidCompletedBounds",
        TyperError::OpaqueAliasDeferred { .. } => "OpaqueAliasDeferred",
        TyperError::RecursiveInferredMethodResult { .. } => "RecursiveInferredMethodResult",
        TyperError::InferredMethodResultRightHandSideMissing { .. } => {
            "InferredMethodResultRightHandSideMissing"
        }
        TyperError::InvalidInferredMethodResult { .. } => "InvalidInferredMethodResult",
        TyperError::RightAssociativeExtensionDeferred { .. } => "RightAssociativeExtensionDeferred",
        TyperError::RightAssociativeInfixDeferred { .. } => "RightAssociativeInfixDeferred",
        TyperError::InfixOperatorMustBeTerm { .. } => "InfixOperatorMustBeTerm",
        TyperError::MethodParameterSymbolMissing { .. } => "MethodParameterSymbolMissing",
        TyperError::LocalMethodSignatureDeferred { .. } => "LocalMethodSignatureDeferred",
        TyperError::LocalMethodInferredResultDeferred { .. } => "LocalMethodInferredResultDeferred",
        TyperError::MethodTypeParameterSymbolMissing { .. } => "MethodTypeParameterSymbolMissing",
        TyperError::ExtensionPrefixClausesMissing { .. } => "ExtensionPrefixClausesMissing",
        TyperError::MalformedMethodClause { .. } => "MalformedMethodClause",
        TyperError::ConstructorOwnerNotClassLike { .. } => "ConstructorOwnerNotClassLike",
        TyperError::MalformedConstructorOwnerInfo { .. } => "MalformedConstructorOwnerInfo",
        TyperError::MalformedConstructorOwner { .. } => "MalformedConstructorOwner",
        TyperError::SecondaryConstructorTypeParametersUnsupported { .. } => {
            "SecondaryConstructorTypeParametersUnsupported"
        }
        TyperError::DeferredSymbolCompletion { .. } => "DeferredSymbolCompletion",
        TyperError::SymbolAlreadyErrored { .. } => "SymbolAlreadyErrored",
        TyperError::UnsupportedTypeTree { .. } => "UnsupportedTypeTree",
        TyperError::SourceTypeProjectionDepthExceeded { .. } => "SourceTypeProjectionDepthExceeded",
        TyperError::MissingDeclaredType { .. } => "MissingDeclaredType",
        TyperError::MalformedSourceAst { .. } => "MalformedSourceAst",
        TyperError::SymbolSourceKindMismatch { .. } => "SymbolSourceKindMismatch",
        TyperError::TreeOutsideArena { .. } => "TreeOutsideArena",
        TyperError::TypeNameNotFound { .. } => "TypeNameNotFound",
        TyperError::WrongTypeNameKind { .. } => "WrongTypeNameKind",
        TyperError::SymbolResolution { .. } => "SymbolResolution",
        TyperError::AmbiguousTypeName { .. } => "AmbiguousTypeName",
        TyperError::DuplicateSourceTypeCacheEntry { .. } => "DuplicateSourceTypeCacheEntry",
        TyperError::TypeRebinding { .. } => "TypeRebinding",
        TyperError::SourceClassTypeParametersProvenanceMissing { .. } => {
            "SourceClassTypeParametersProvenanceMissing"
        }
        TyperError::MalformedSourceClassTypeParameters { .. } => {
            "MalformedSourceClassTypeParameters"
        }
        TyperError::ClassTypeParameterSymbolMissing { .. } => "ClassTypeParameterSymbolMissing",
        TyperError::ReceiverDoesNotDenoteExpectedClass { .. } => {
            "ReceiverDoesNotDenoteExpectedClass"
        }
        TyperError::ReceiverGenericArityMismatch { .. } => "ReceiverGenericArityMismatch",
        TyperError::ConstructorTargetGenericArityMismatch { .. } => {
            "ConstructorTargetGenericArityMismatch"
        }
        TyperError::RawGenericSourceReceiverUnsupported { .. } => {
            "RawGenericSourceReceiverUnsupported"
        }
        TyperError::ExternalGenericInstantiationDeferred { .. } => {
            "ExternalGenericInstantiationDeferred"
        }
        TyperError::MemberTypeUnavailable { .. } => "MemberTypeUnavailable",
        TyperError::TypeSubstitution(..) => "TypeSubstitution",
        TyperError::TypeNormalization(..) => "TypeNormalization",
        TyperError::ConflictingTypedExpression { .. } => "ConflictingTypedExpression",
        TyperError::StringLiteralTypingDeferred { .. } => "StringLiteralTypingDeferred",
        TyperError::NullLiteralTypingDeferred { .. } => "NullLiteralTypingDeferred",
        TyperError::IntegerLiteralOutOfRange { .. } => "IntegerLiteralOutOfRange",
        TyperError::FloatingLiteralInvalid { .. } => "FloatingLiteralInvalid",
        TyperError::ThisOwnerNotEnclosing { .. } => "ThisOwnerNotEnclosing",
        TyperError::ThisOwnerCycle { .. } => "ThisOwnerCycle",
        TyperError::TermNameNotFound { .. } => "TermNameNotFound",
        TyperError::AmbiguousTermReference { .. } => "AmbiguousTermReference",
        TyperError::OverloadedReferenceDeferred { .. } => "OverloadedReferenceDeferred",
        TyperError::ObjectTermReferenceDeferred { .. } => "ObjectTermReferenceDeferred",
        TyperError::ObjectModuleClassUnavailable { .. } => "ObjectModuleClassUnavailable",
        TyperError::UnsupportedTermReference { .. } => "UnsupportedTermReference",
        TyperError::MemberNotFound { .. } => "MemberNotFound",
        TyperError::OverloadedSelectionDeferred { .. } => "OverloadedSelectionDeferred",
        TyperError::ApplicationCalleeNotMethod { .. } => "ApplicationCalleeNotMethod",
        TyperError::UnconstrainedTypeParameter { .. } => "UnconstrainedTypeParameter",
        TyperError::ConflictingInferenceConstraints { .. } => "ConflictingInferenceConstraints",
        TyperError::UnsupportedInferenceShape { .. } => "UnsupportedInferenceShape",
        TyperError::UnsupportedPolymorphicApplicationShape { .. } => {
            "UnsupportedPolymorphicApplicationShape"
        }
        TyperError::GenericOverloadResolutionDeferred { .. } => "GenericOverloadResolutionDeferred",
        TyperError::InferredTypeArgumentBoundViolation { .. } => {
            "InferredTypeArgumentBoundViolation"
        }
        TyperError::UnsupportedInferredTypeArgumentBounds { .. } => {
            "UnsupportedInferredTypeArgumentBounds"
        }
        TyperError::InferredTypeArgumentBoundCheckUnsupported { .. } => {
            "InferredTypeArgumentBoundCheckUnsupported"
        }
        TyperError::ExplicitTypeApplicationCalleeNotPoly { .. } => {
            "ExplicitTypeApplicationCalleeNotPoly"
        }
        TyperError::ExplicitTypeApplicationArityMismatch { .. } => {
            "ExplicitTypeApplicationArityMismatch"
        }
        TyperError::ExplicitTypeArgumentBoundViolation { .. } => {
            "ExplicitTypeArgumentBoundViolation"
        }
        TyperError::UnsupportedExplicitTypeArgumentBounds { .. } => {
            "UnsupportedExplicitTypeArgumentBounds"
        }
        TyperError::ExplicitTypeArgumentBoundCheckUnsupported { .. } => {
            "ExplicitTypeArgumentBoundCheckUnsupported"
        }
        TyperError::OverloadedTypeApplicationDeferred { .. } => "OverloadedTypeApplicationDeferred",
        TyperError::PolyInstantiation(..) => "PolyInstantiation",
        TyperError::TypeArgumentTreeCannotBeReified { .. } => "TypeArgumentTreeCannotBeReified",
        TyperError::TypeAscriptionTreeCannotBeReified { .. } => "TypeAscriptionTreeCannotBeReified",
        TyperError::ApplicationMethodKindMismatch { .. } => "ApplicationMethodKindMismatch",
        TyperError::UnsupportedApplicationMethodKind { .. } => "UnsupportedApplicationMethodKind",
        TyperError::UsingApplicationDeferred { .. } => "UsingApplicationDeferred",
        TyperError::ApplicationArityMismatch { .. } => "ApplicationArityMismatch",
        TyperError::ErasedApplicationParameterDeferred { .. } => {
            "ErasedApplicationParameterDeferred"
        }
        TyperError::VarargsApplicationParameterDeferred { .. } => {
            "VarargsApplicationParameterDeferred"
        }
        TyperError::VarargsParameterReferenceDeferred { .. } => "VarargsParameterReferenceDeferred",
        TyperError::ByNameApplicationParameterDeferred { .. } => {
            "ByNameApplicationParameterDeferred"
        }
        TyperError::DependentMethodApplicationDeferred { .. } => {
            "DependentMethodApplicationDeferred"
        }
        TyperError::ApplicationArgumentTypeMismatch { .. } => "ApplicationArgumentTypeMismatch",
        TyperError::ApplicationArgumentConformanceUnsupported { .. } => {
            "ApplicationArgumentConformanceUnsupported"
        }
        TyperError::ExpectedExpressionTypeMismatch { .. } => "ExpectedExpressionTypeMismatch",
        TyperError::ExpectedExpressionConformanceUnsupported { .. } => {
            "ExpectedExpressionConformanceUnsupported"
        }
        TyperError::AssignmentLhsNotAssignable { .. } => "AssignmentLhsNotAssignable",
        TyperError::AssignmentTargetImmutable { .. } => "AssignmentTargetImmutable",
        TyperError::AssignmentTargetKindUnsupported { .. } => "AssignmentTargetKindUnsupported",
        TyperError::WritableAssignmentTypeUnavailable { .. } => "WritableAssignmentTypeUnavailable",
        TyperError::IfConditionTypeMismatch { .. } => "IfConditionTypeMismatch",
        TyperError::IfConditionConformanceUnsupported { .. } => "IfConditionConformanceUnsupported",
        TyperError::IfBranchTypeCannotBeWidened { .. } => "IfBranchTypeCannotBeWidened",
        TyperError::IfBranchJoinUnsupported { .. } => "IfBranchJoinUnsupported",
        TyperError::IfChildTreeOutsideArena { .. } => "IfChildTreeOutsideArena",
        TyperError::WhileConditionTypeMismatch { .. } => "WhileConditionTypeMismatch",
        TyperError::WhileConditionConformanceUnsupported { .. } => {
            "WhileConditionConformanceUnsupported"
        }
        TyperError::WhileChildTreeOutsideArena { .. } => "WhileChildTreeOutsideArena",
        TyperError::ReturnOutsideSupportedMethod { .. } => "ReturnOutsideSupportedMethod",
        TyperError::ReturnMethodProvenanceMalformed { .. } => "ReturnMethodProvenanceMalformed",
        TyperError::ReturnInInferredResultMethodDeferred { .. } => {
            "ReturnInInferredResultMethodDeferred"
        }
        TyperError::MalformedReturnTarget { .. } => "MalformedReturnTarget",
        TyperError::NonLocalReturnDeferred { .. } => "NonLocalReturnDeferred",
        TyperError::ReturnExpressionTypeMismatch { .. } => "ReturnExpressionTypeMismatch",
        TyperError::ReturnExpressionConformanceUnsupported { .. } => {
            "ReturnExpressionConformanceUnsupported"
        }
        TyperError::MalformedReturnOwnerChain { .. } => "MalformedReturnOwnerChain",
        TyperError::MalformedReturnMethodSignature { .. } => "MalformedReturnMethodSignature",
        TyperError::OverloadApplicationNoApplicable { .. } => "OverloadApplicationNoApplicable",
        TyperError::AmbiguousOverloadApplication { .. } => "AmbiguousOverloadApplication",
        TyperError::OverloadResolutionRequiresUnsupportedCandidate { .. } => {
            "OverloadResolutionRequiresUnsupportedCandidate"
        }
        TyperError::MalformedOverloadCandidate { .. } => "MalformedOverloadCandidate",
        TyperError::MixedApplicationCandidateKinds { .. } => "MixedApplicationCandidateKinds",
        TyperError::TypeSelectionInExpression { .. } => "TypeSelectionInExpression",
        TyperError::MemberLookup(..) => "MemberLookup",
        TyperError::UnsupportedExpression { .. } => "UnsupportedExpression",
        TyperError::UnsupportedPattern { .. } => "UnsupportedPattern",
        TyperError::TuplePatternResolutionDeferred { .. } => "TuplePatternResolutionDeferred",
        TyperError::TuplePatternTypeMismatch { .. } => "TuplePatternTypeMismatch",
        TyperError::TuplePatternRelationDeferred { .. } => "TuplePatternRelationDeferred",
        TyperError::InfixPatternDeferred { .. } => "InfixPatternDeferred",
        TyperError::PatternBindingInAlternative { .. } => "PatternBindingInAlternative",
        TyperError::PatternAlternativeJoinUnsupported { .. } => "PatternAlternativeJoinUnsupported",
        TyperError::ExtractorQualifierNotFound { .. } => "ExtractorQualifierNotFound",
        TyperError::ExtractorQualifierMemberNotFound { .. } => "ExtractorQualifierMemberNotFound",
        TyperError::ExtractorQualifierMemberAmbiguous { .. } => "ExtractorQualifierMemberAmbiguous",
        TyperError::ExtractorQualifierShapeUnsupported { .. } => {
            "ExtractorQualifierShapeUnsupported"
        }
        TyperError::ExtractorQualifierNotValueLike { .. } => "ExtractorQualifierNotValueLike",
        TyperError::ExtractorQualifierNotStable { .. } => "ExtractorQualifierNotStable",
        TyperError::ExtractorUnapplyNotFound { .. } => "ExtractorUnapplyNotFound",
        TyperError::ExtractorUnapplyOverloaded { .. } => "ExtractorUnapplyOverloaded",
        TyperError::ExtractorUnapplyPolymorphic { .. } => "ExtractorUnapplyPolymorphic",
        TyperError::ExtractorUnapplyShapeUnsupported { .. } => "ExtractorUnapplyShapeUnsupported",
        TyperError::ExtractorPatternConstraintDeferred { .. } => {
            "ExtractorPatternConstraintDeferred"
        }
        TyperError::UnsupportedExtractorResultProtocol { .. } => {
            "UnsupportedExtractorResultProtocol"
        }
        TyperError::ExtractorPatternArityUnsupported { .. } => "ExtractorPatternArityUnsupported",
        TyperError::BooleanExtractorPatternArityUnsupported { .. } => {
            "BooleanExtractorPatternArityUnsupported"
        }
        TyperError::ExtractorResultMemberNotFound { .. } => "ExtractorResultMemberNotFound",
        TyperError::ExtractorResultMemberOverloaded { .. } => "ExtractorResultMemberOverloaded",
        TyperError::ExtractorResultMemberUnsupported { .. } => "ExtractorResultMemberUnsupported",
        TyperError::ExtractorProductSelectorCountMismatch { .. } => {
            "ExtractorProductSelectorCountMismatch"
        }
        TyperError::UnsupportedExtractorProductProtocol { .. } => {
            "UnsupportedExtractorProductProtocol"
        }
        TyperError::ExtractorPatternArgumentUnsupported { .. } => {
            "ExtractorPatternArgumentUnsupported"
        }
        TyperError::MalformedCaseDef { .. } => "MalformedCaseDef",
        TyperError::MalformedPatternBinding { .. } => "MalformedPatternBinding",
        TyperError::WildcardPatternBindingRejected { .. } => "WildcardPatternBindingRejected",
        TyperError::PatternBindingOutsideCaseScope { .. } => "PatternBindingOutsideCaseScope",
        TyperError::PatternBindingScopeConflict { .. } => "PatternBindingScopeConflict",
        TyperError::DuplicatePatternBinding { .. } => "DuplicatePatternBinding",
        TyperError::EmptyMatchCases { .. } => "EmptyMatchCases",
        TyperError::MatchSelectorTypeCannotBeAdapted { .. } => "MatchSelectorTypeCannotBeAdapted",
        TyperError::MatchCaseResultTypeCannotBeWidened { .. } => {
            "MatchCaseResultTypeCannotBeWidened"
        }
        TyperError::MatchCaseJoinUnsupported { .. } => "MatchCaseJoinUnsupported",
        TyperError::MalformedVariablePattern { .. } => "MalformedVariablePattern",
        TyperError::UnsupportedBindPatternBody { .. } => "UnsupportedBindPatternBody",
        TyperError::PatternTypeMismatch { .. } => "PatternTypeMismatch",
        TyperError::PatternTypeRelationDeferred { .. } => "PatternTypeRelationDeferred",
        TyperError::TypedPatternTypeMismatch { .. } => "TypedPatternTypeMismatch",
        TyperError::TypedPatternRelationDeferred { .. } => "TypedPatternRelationDeferred",
        TyperError::TypedPatternRuntimeTestDeferred { .. } => "TypedPatternRuntimeTestDeferred",
        TyperError::MalformedStablePatternTarget { .. } => "MalformedStablePatternTarget",
        TyperError::UnstablePatternValue { .. } => "UnstablePatternValue",
        TyperError::LocalBlockDeclarationDeferred { .. } => "LocalBlockDeclarationDeferred",
        TyperError::InvalidInferredLocalValueType { .. } => "InvalidInferredLocalValueType",
        TyperError::LocalValueRightHandSideMissing { .. } => "LocalValueRightHandSideMissing",
        TyperError::RecursiveLocalValueInitializer { .. } => "RecursiveLocalValueInitializer",
        TyperError::LocalValueOutsideBlock { .. } => "LocalValueOutsideBlock",
        TyperError::DuplicateLocalValue { .. } => "DuplicateLocalValue",
        TyperError::LocalValueTypeMismatch { .. } => "LocalValueTypeMismatch",
        TyperError::LocalValueConformanceUnsupported { .. } => "LocalValueConformanceUnsupported",
        TyperError::ExpressionTypeCannotBeWidened { .. } => "ExpressionTypeCannotBeWidened",
        TyperError::TermReferenceCannotBeWidened { .. } => "TermReferenceCannotBeWidened",
        TyperError::TermReferencePrefixMismatch { .. } => "TermReferencePrefixMismatch",
        TyperError::UnstableSelectionPrefix { .. } => "UnstableSelectionPrefix",
    }
}

fn is_type_relation_inference_or_completion(bucket: &str) -> bool {
    matches!(
        bucket.split("::").next().unwrap_or(bucket),
        "TypeRebinding"
            | "TypeSubstitution"
            | "TypeNormalization"
            | "ConstructorLookupNormalization"
            | "ConstructorResultTypeCheckUnsupported"
            | "ConstructorTypeArgumentBoundCheckUnsupported"
            | "ConflictingConstructorInferenceConstraints"
            | "UnsupportedConstructorInferenceShape"
            | "UnableToFinalizeRawGenericNewInstanceType"
            | "InvalidCompletedBounds"
            | "RecursiveInferredMethodResult"
            | "InvalidInferredMethodResult"
            | "MethodParameterSymbolMissing"
            | "LocalMethodSignatureDeferred"
            | "LocalMethodInferredResultDeferred"
            | "MethodTypeParameterSymbolMissing"
            | "MalformedMethodClause"
            | "DeferredSymbolCompletion"
            | "ClassTypeParameterSymbolMissing"
            | "ReceiverGenericArityMismatch"
            | "ConstructorTargetGenericArityMismatch"
            | "ExternalGenericInstantiationDeferred"
            | "MemberTypeUnavailable"
            | "UnconstrainedTypeParameter"
            | "ConflictingInferenceConstraints"
            | "UnsupportedInferenceShape"
            | "UnsupportedPolymorphicApplicationShape"
            | "GenericOverloadResolutionDeferred"
            | "InferredTypeArgumentBoundViolation"
            | "UnsupportedInferredTypeArgumentBounds"
            | "InferredTypeArgumentBoundCheckUnsupported"
            | "ExplicitTypeApplicationCalleeNotPoly"
            | "ExplicitTypeApplicationArityMismatch"
            | "ExplicitTypeArgumentBoundViolation"
            | "UnsupportedExplicitTypeArgumentBounds"
            | "ExplicitTypeArgumentBoundCheckUnsupported"
            | "OverloadedTypeApplicationDeferred"
            | "PolyInstantiation"
            | "ApplicationMethodKindMismatch"
            | "UnsupportedApplicationMethodKind"
            | "ApplicationArityMismatch"
            | "ErasedApplicationParameterDeferred"
            | "VarargsApplicationParameterDeferred"
            | "VarargsParameterReferenceDeferred"
            | "ByNameApplicationParameterDeferred"
            | "DependentMethodApplicationDeferred"
            | "ApplicationArgumentTypeMismatch"
            | "ApplicationArgumentConformanceUnsupported"
            | "ExpectedExpressionTypeMismatch"
            | "ExpectedExpressionConformanceUnsupported"
            | "WritableAssignmentTypeUnavailable"
            | "IfConditionTypeMismatch"
            | "IfConditionConformanceUnsupported"
            | "IfBranchTypeCannotBeWidened"
            | "IfBranchJoinUnsupported"
            | "WhileConditionTypeMismatch"
            | "WhileConditionConformanceUnsupported"
            | "ReturnExpressionTypeMismatch"
            | "ReturnExpressionConformanceUnsupported"
            | "OverloadApplicationNoApplicable"
            | "AmbiguousOverloadApplication"
            | "OverloadResolutionRequiresUnsupportedCandidate"
            | "MalformedOverloadCandidate"
            | "MixedApplicationCandidateKinds"
            | "TypeArgumentTreeCannotBeReified"
            | "TypeAscriptionTreeCannotBeReified"
            | "InvalidInferredLocalValueType"
            | "LocalValueTypeMismatch"
            | "LocalValueConformanceUnsupported"
            | "ExpressionTypeCannotBeWidened"
            | "TermReferenceCannotBeWidened"
            | "TermReferencePrefixMismatch"
            | "UnstableSelectionPrefix"
    ) || bucket.starts_with("Constructor")
        || bucket.starts_with("Class")
        || bucket.starts_with("Method")
        || bucket.starts_with("Application")
        || bucket.starts_with("Overload")
}

fn parser_error_node_name(kind: dotty_core::ast::ErrorNodeKind) -> &'static str {
    match kind {
        dotty_core::ast::ErrorNodeKind::MissingExpression => "MissingExpression",
        dotty_core::ast::ErrorNodeKind::MissingType => "MissingType",
        dotty_core::ast::ErrorNodeKind::MissingPattern => "MissingPattern",
        dotty_core::ast::ErrorNodeKind::UnexpectedToken => "UnexpectedToken",
    }
}

fn parse_diagnostic_name(kind: ParseDiagnosticKind) -> &'static str {
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

fn classify_parse_diagnostic(kind: ParseDiagnosticKind) -> FailureClassification {
    FailureClassification {
        bucket: format!("ParserDiagnostic::{}", parse_diagnostic_name(kind)),
        family: FailureFamily::ParserNamer,
    }
}

fn tree_kind_label(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::Literal(_) => "Literal",
        TreeKind::Apply(_) => "Apply",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::New(_) => "New",
        TreeKind::Typed(_) => "Typed",
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Assign(_) => "Assign",
        TreeKind::Block(_) => "Block",
        TreeKind::If(_) => "If",
        TreeKind::Match(_) => "Match",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::Return(_) => "Return",
        TreeKind::While(_) => "While",
        TreeKind::Try(_) => "Try",
        TreeKind::Closure(_) => "Closure",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::Template(_) => "Template",
        TreeKind::PackageDef(_) => "PackageDef",
        TreeKind::Import(_) => "Import",
        TreeKind::Export(_) => "Export",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::SingletonTypeTree(_) => "SingletonTypeTree",
        TreeKind::AppliedTypeTree(_) => "AppliedTypeTree",
        TreeKind::RefinedTypeTree(_) => "RefinedTypeTree",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::MatchTypeTree(_) => "MatchTypeTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::UnApply(_) => "UnApply",
        TreeKind::Annotated(_) => "Annotated",
        TreeKind::Quote(_) => "Quote",
        TreeKind::Splice(_) => "Splice",
        TreeKind::QuotePattern(_) => "QuotePattern",
        TreeKind::SplicePattern(_) => "SplicePattern",
        TreeKind::Inlined(_) => "Inlined",
        TreeKind::PhaseSpecific(extra) => match extra {
            UntypedNode::Error(_) => "Error",
            UntypedNode::ModuleDef(_) => "ModuleDef",
            UntypedNode::Function(_) => "Function",
            UntypedNode::FunctionWithMods(_) => "FunctionWithMods",
            UntypedNode::CapturesAndResult(_) => "CapturesAndResult",
            UntypedNode::PolyFunction(_) => "PolyFunction",
            UntypedNode::InfixOp(_) => "InfixOp",
            UntypedNode::PrefixOp(_) => "PrefixOp",
            UntypedNode::PostfixOp(_) => "PostfixOp",
            UntypedNode::Parens(_) => "Parens",
            UntypedNode::Tuple(_) => "Tuple",
            UntypedNode::ForYield(_) => "ForYield",
            UntypedNode::ForDo(_) => "ForDo",
            UntypedNode::GenFrom(_) => "GenFrom",
            UntypedNode::GenAlias(_) => "GenAlias",
            UntypedNode::PatDef(_) => "PatDef",
            UntypedNode::ExtensionMethods(_) => "ExtensionMethods",
            UntypedNode::InterpolatedString(_) => "InterpolatedString",
            UntypedNode::ContextBounds(_) => "ContextBounds",
            UntypedNode::ContextBoundTypeTree(_) => "ContextBoundTypeTree",
            UntypedNode::Number(_) => "Number",
            UntypedNode::Throw(_) => "Throw",
            UntypedNode::InlineIf(_) => "InlineIf",
            UntypedNode::InlineMatch(_) => "InlineMatch",
            UntypedNode::ParsedTry(_) => "ParsedTry",
        },
    }
}

fn expression_form(kind: &TreeKind<Untyped>) -> Option<&'static str> {
    match kind {
        TreeKind::Ident(_) => Some("Ident"),
        TreeKind::Select(_) => Some("Select"),
        TreeKind::Apply(_) => Some("Apply"),
        TreeKind::TypeApply(_) => Some("TypeApply"),
        TreeKind::New(_) => Some("New"),
        TreeKind::Typed(_) => Some("Typed"),
        TreeKind::Assign(_) => Some("Assign"),
        TreeKind::Block(_) => Some("Block"),
        TreeKind::If(_) => Some("If"),
        TreeKind::Match(_) => Some("Match"),
        TreeKind::Return(_) => Some("Return"),
        TreeKind::While(_) => Some("While"),
        TreeKind::Try(_) => Some("Try"),
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => Some("Function"),
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(_)) => Some("PolyFunction"),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => Some("InfixOp"),
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_)) => Some("PrefixOp"),
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => Some("PostfixOp"),
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => Some("Parens"),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => Some("Tuple"),
        TreeKind::PhaseSpecific(UntypedNode::ForYield(_)) => Some("ForYield"),
        TreeKind::PhaseSpecific(UntypedNode::ForDo(_)) => Some("ForDo"),
        TreeKind::PhaseSpecific(UntypedNode::Throw(_)) => Some("Throw"),
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(_)) => Some("ParsedTry"),
        TreeKind::PhaseSpecific(UntypedNode::InlineIf(_)) => Some("InlineIf"),
        TreeKind::PhaseSpecific(UntypedNode::InlineMatch(_)) => Some("InlineMatch"),
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(_)) => Some("InterpolatedString"),
        _ => None,
    }
}

const EXPRESSION_FORMS: &[&str] = &[
    "Ident",
    "Select",
    "Apply",
    "TypeApply",
    "New",
    "Typed",
    "Assign",
    "Block",
    "If",
    "Match",
    "Return",
    "While",
    "Try",
    "Function",
    "PolyFunction",
    "InfixOp",
    "PrefixOp",
    "PostfixOp",
    "Parens",
    "Tuple",
    "ForYield",
    "ForDo",
    "Throw",
    "ParsedTry",
    "InterpolatedString",
];

fn empty_expression_histogram() -> BTreeMap<String, usize> {
    EXPRESSION_FORMS
        .iter()
        .map(|form| ((*form).to_owned(), 0))
        .collect()
}

fn collect_expression_histogram(arena: &dotty_core::AstArena<Untyped>) -> BTreeMap<String, usize> {
    let mut histogram = empty_expression_histogram();
    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    for (_, node) in arena.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        if let Some(rhs) = definition.rhs {
            let mut visited = HashSet::new();
            let mut pending = VecDeque::from([rhs]);
            while let Some(tree) = pending.pop_front() {
                if !visited.insert(tree) {
                    continue;
                }
                let Some(node) = nodes.get(&tree.index()) else {
                    continue;
                };
                if let Some(form) = expression_form(&node.kind) {
                    *histogram.entry(form.to_owned()).or_default() += 1;
                }
                pending.extend(term_expression_children(&node.kind));
            }
        }
    }
    histogram
}

const TYPE_TREE_FORMS: &[&str] = &[
    "Ident",
    "Select",
    "TypeTree",
    "AppliedTypeTree",
    "ByNameTypeTree",
    "TypeBoundsTree",
    "SingletonTypeTree",
    "RefinedTypeTree",
    "LambdaTypeTree",
    "MatchTypeTree",
    "Annotated",
    "InfixOp::|",
    "InfixOp::&",
    "InfixOp::<other>",
    "PostfixOp::*",
    "PostfixOp::<other>",
    "Function",
    "FunctionWithMods",
    "ContextBoundTypeTree",
    "Parens",
    "Tuple",
];

fn empty_type_tree_histogram() -> BTreeMap<String, usize> {
    TYPE_TREE_FORMS
        .iter()
        .map(|form| ((*form).to_owned(), 0))
        .collect()
}

fn collect_declared_type_tree_histogram(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
) -> BTreeMap<String, usize> {
    let operators = source_operator_spellings(arena, names);
    let local_stats = local_stat_trees(arena);
    let mut roots = Vec::new();
    roots.extend(local_pattern_type_trees(arena));
    for (tree, node) in arena.iter() {
        match &node.kind {
            TreeKind::DefDef(definition) => {
                roots.push(definition.tpt);
                roots.extend(definition.type_params.iter().copied());
                roots.extend(definition.value_param_clauses.iter().flatten().filter_map(
                    |parameter| match &arena.get(*parameter).kind {
                        TreeKind::ValDef(parameter) => Some(parameter.tpt),
                        _ => None,
                    },
                ));
            }
            TreeKind::ValDef(definition) if local_stats.contains(&tree) => {
                roots.push(definition.tpt);
            }
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition))
                if local_stats.contains(&tree) =>
            {
                roots.push(definition.tpt);
            }
            TreeKind::TypeDef(definition)
                if local_stats.contains(&tree)
                    && !matches!(arena.get(definition.rhs).kind, TreeKind::Template(_)) =>
            {
                roots.push(definition.rhs);
            }
            _ => {}
        }
    }

    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    let mut histogram = empty_type_tree_histogram();
    let mut visited = HashSet::new();
    let mut pending = roots;
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = nodes.get(&tree.index()) else {
            continue;
        };
        if matches!(node.kind, TreeKind::TypeTree(_))
            && node
                .position
                .is_some_and(|position| position.span().range().is_empty())
        {
            continue;
        }
        if let Some(form) = type_tree_form(&node.kind, tree.index(), &operators) {
            *histogram.entry(form.to_owned()).or_default() += 1;
        }
        pending.extend(type_tree_children(&node.kind));
    }
    histogram
}

fn type_tree_form(
    kind: &TreeKind<Untyped>,
    tree_index: u32,
    operators: &BTreeMap<u32, String>,
) -> Option<&'static str> {
    match kind {
        TreeKind::Ident(_) => Some("Ident"),
        TreeKind::Select(_) => Some("Select"),
        TreeKind::TypeTree(_) => Some("TypeTree"),
        TreeKind::AppliedTypeTree(_) => Some("AppliedTypeTree"),
        TreeKind::ByNameTypeTree(_) => Some("ByNameTypeTree"),
        TreeKind::TypeBoundsTree(_) => Some("TypeBoundsTree"),
        TreeKind::SingletonTypeTree(_) => Some("SingletonTypeTree"),
        TreeKind::RefinedTypeTree(_) => Some("RefinedTypeTree"),
        TreeKind::LambdaTypeTree(_) => Some("LambdaTypeTree"),
        TreeKind::MatchTypeTree(_) => Some("MatchTypeTree"),
        TreeKind::Annotated(_) => Some("Annotated"),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => {
            Some(match operators.get(&tree_index).map(String::as_str) {
                Some("|") => "InfixOp::|",
                Some("&") => "InfixOp::&",
                _ => "InfixOp::<other>",
            })
        }
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => {
            Some(match operators.get(&tree_index).map(String::as_str) {
                Some("*") => "PostfixOp::*",
                _ => "PostfixOp::<other>",
            })
        }
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => Some("Function"),
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_)) => Some("FunctionWithMods"),
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(_)) => {
            Some("ContextBoundTypeTree")
        }
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => Some("Parens"),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => Some("Tuple"),
        _ => None,
    }
}

fn type_tree_children(kind: &TreeKind<Untyped>) -> Vec<dotty_core::TreeId<Untyped>> {
    match kind {
        TreeKind::Select(node) => vec![node.qualifier],
        TreeKind::AppliedTypeTree(node) => std::iter::once(node.tpt)
            .chain(node.args.iter().copied())
            .collect(),
        TreeKind::ByNameTypeTree(node) => vec![node.result],
        TreeKind::TypeBoundsTree(node) => [node.low, node.high, node.alias]
            .into_iter()
            .flatten()
            .collect(),
        // The reference is a term path inside the singleton type, not another
        // type-tree node for this structural histogram.
        TreeKind::SingletonTypeTree(_) => Vec::new(),
        TreeKind::RefinedTypeTree(node) => std::iter::once(node.tpt)
            .chain(node.refinements.iter().copied())
            .collect(),
        TreeKind::LambdaTypeTree(node) => node
            .type_params
            .iter()
            .copied()
            .chain([node.body])
            .collect(),
        TreeKind::MatchTypeTree(node) => node
            .bound
            .into_iter()
            .chain([node.selector])
            .chain(node.cases.iter().copied())
            .collect(),
        TreeKind::Annotated(node) => vec![node.expr],
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(node)) => vec![node.left, node.right],
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(node)) => vec![node.operand],
        TreeKind::PhaseSpecific(UntypedNode::Function(node)) => {
            node.params.iter().copied().chain([node.body]).collect()
        }
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(node)) => {
            node.params.iter().copied().chain([node.result]).collect()
        }
        TreeKind::PhaseSpecific(UntypedNode::ContextBounds(node)) => std::iter::once(node.bounds)
            .chain(node.context_bounds.iter().copied())
            .collect(),
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(node)) => vec![node.bound],
        TreeKind::PhaseSpecific(UntypedNode::Parens(node)) => vec![node.inner],
        TreeKind::PhaseSpecific(UntypedNode::Tuple(node)) => node.elements.clone(),
        TreeKind::ValDef(node) => vec![node.tpt],
        TreeKind::DefDef(node) => node
            .type_params
            .iter()
            .copied()
            .chain(node.value_param_clauses.iter().flatten().copied())
            .chain([node.tpt])
            .collect(),
        TreeKind::TypeDef(node) => vec![node.rhs],
        TreeKind::CaseDef(node) => vec![node.pattern, node.body],
        _ => Vec::new(),
    }
}

fn term_expression_children(kind: &TreeKind<Untyped>) -> Vec<dotty_core::TreeId<Untyped>> {
    let mut children = Vec::new();
    match kind {
        TreeKind::Select(node) => children.push(node.qualifier),
        TreeKind::Super(node) => children.push(node.qual),
        TreeKind::Apply(node) => {
            children.push(node.function);
            children.extend(node.args.iter().copied());
        }
        TreeKind::TypeApply(node) => children.push(node.function),
        TreeKind::Typed(node) => children.push(node.expr),
        TreeKind::NamedArg(node) => children.push(node.arg),
        TreeKind::Assign(node) => children.extend([node.lhs, node.rhs]),
        TreeKind::Block(node) => {
            children.extend(node.stats.iter().copied());
            children.push(node.expr);
        }
        TreeKind::If(node) => children.extend([node.cond, node.then_branch, node.else_branch]),
        TreeKind::Match(node) => {
            children.push(node.selector);
            children.extend(node.cases.iter().copied());
        }
        TreeKind::CaseDef(node) => {
            if let Some(guard) = node.guard {
                children.push(guard);
            }
            children.push(node.body);
        }
        TreeKind::Return(node) => children.extend(node.expr),
        TreeKind::While(node) => children.extend([node.cond, node.body]),
        TreeKind::Try(node) => {
            children.push(node.expr);
            children.extend(node.cases.iter().copied());
            children.extend(node.finalizer);
        }
        TreeKind::Closure(node) => children.extend(node.env.iter().copied()),
        TreeKind::ValDef(node) => children.extend(node.rhs),
        TreeKind::DefDef(_) | TreeKind::TypeDef(_) | TreeKind::Template(_) => {}
        TreeKind::PhaseSpecific(UntypedNode::Function(node)) => children.push(node.body),
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(node)) => children.push(node.body),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(node)) => {
            children.extend([node.left, node.right]);
        }
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(node)) => children.push(node.operand),
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(node)) => children.push(node.operand),
        TreeKind::PhaseSpecific(UntypedNode::Parens(node)) => children.push(node.inner),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(node)) => {
            children.extend(node.elements.iter().copied());
        }
        TreeKind::PhaseSpecific(UntypedNode::ForYield(node)) => {
            children.extend(node.enums.iter().copied());
            children.push(node.body);
        }
        TreeKind::PhaseSpecific(UntypedNode::ForDo(node)) => {
            children.extend(node.enums.iter().copied());
            children.push(node.body);
        }
        TreeKind::PhaseSpecific(UntypedNode::GenFrom(node)) => children.push(node.expr),
        TreeKind::PhaseSpecific(UntypedNode::GenAlias(node)) => children.push(node.expr),
        TreeKind::PhaseSpecific(UntypedNode::PatDef(node)) => children.extend(node.rhs),
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(node)) => {
            children.extend(node.parts.iter().copied());
        }
        TreeKind::PhaseSpecific(UntypedNode::Throw(node)) => children.push(node.expr),
        TreeKind::PhaseSpecific(UntypedNode::InlineIf(node)) => {
            children.extend([node.cond, node.then_branch, node.else_branch]);
        }
        TreeKind::PhaseSpecific(UntypedNode::InlineMatch(node)) => {
            children.push(node.selector);
            children.extend(node.cases.iter().copied());
        }
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(node)) => {
            children.push(node.expr);
            children.extend(node.handler);
            children.extend(node.finalizer);
        }
        TreeKind::PhaseSpecific(
            UntypedNode::Error(_)
            | UntypedNode::ModuleDef(_)
            | UntypedNode::FunctionWithMods(_)
            | UntypedNode::CapturesAndResult(_)
            | UntypedNode::ExtensionMethods(_)
            | UntypedNode::ContextBounds(_)
            | UntypedNode::ContextBoundTypeTree(_)
            | UntypedNode::Number(_),
        ) => {}
        TreeKind::Quote(node) => children.push(node.body),
        TreeKind::Splice(node) => children.push(node.expr),
        TreeKind::Inlined(node) => {
            children.extend(node.bindings.iter().copied());
            children.push(node.expansion);
        }
        TreeKind::Annotated(node) => children.push(node.expr),
        TreeKind::Ident(_)
        | TreeKind::This(_)
        | TreeKind::Literal(_)
        | TreeKind::New(_)
        | TreeKind::PackageDef(_)
        | TreeKind::Import(_)
        | TreeKind::Export(_)
        | TreeKind::TypeTree(_)
        | TreeKind::SingletonTypeTree(_)
        | TreeKind::AppliedTypeTree(_)
        | TreeKind::RefinedTypeTree(_)
        | TreeKind::LambdaTypeTree(_)
        | TreeKind::MatchTypeTree(_)
        | TreeKind::ByNameTypeTree(_)
        | TreeKind::TypeBoundsTree(_)
        | TreeKind::Bind(_)
        | TreeKind::Alternative(_)
        | TreeKind::UnApply(_)
        | TreeKind::QuotePattern(_)
        | TreeKind::SplicePattern(_) => {}
    }
    children
}

fn scala_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        if metadata.is_file() {
            if path
                .extension()
                .is_some_and(|extension| extension == "scala")
            {
                files.push(path.to_owned());
            }
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            visit(&entry, files);
        }
    }
    let mut files = Vec::new();
    for root in roots {
        visit(root, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn git_revision(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git should be available")
        .stdout
        .into_iter()
        .map(char::from)
        .collect::<String>()
        .trim()
        .to_owned()
}
